// SPDX-License-Identifier: GPL-3.0-or-later
//! "Export backing track" (APP_API section 7): an offline render of the song with the
//! singer's key and mix, through the same jk-dsp graph as playback, single-threaded, at
//! the song's sample rate, streamed from the song file (never the whole song in memory).
//!
//! * `Wav32f`: 32-bit float WAV (RF64 when the data exceeds 4 GiB). Never scaled: the
//!   file holds exactly the rendered samples (bit-identical to `jk_dsp::render`).
//! * `Flac24`: 24-bit FLAC. A float render can exceed full scale and an integer file
//!   cannot hold that, so the export has two passes: pass 1 renders into a temporary
//!   32-bit float file next to the output and measures the peak; pass 2 scales the whole
//!   file by one constant just enough to fit (only if needed), rounds to 24 bits and
//!   encodes. The gain is reported (`applied_gain_db`) so the UI can say so: visible, not
//!   hidden processing. No dither (24-bit rounding error is -144 dBFS). Finally the file
//!   is decoded back and its audio MD5 checked. The temporary file is removed in every
//!   case (about 85 MB for 4 minutes of 44.1 kHz stereo).
//!
//! Output goes to `<out>.tmp-<random>` and is renamed over `out` when complete.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use jk_dsp::{ExtraBlock, GraphConfig, PitchGraph, StemBlock, CHANNELS};

use crate::api::*;
use crate::flac_write::FlacEncoder;
use crate::source::{slot_buffers, SongSource, SLOT_EXTRA_PITCHED, SLOT_EXTRA_UNPITCHED};

const BLOCK: usize = 512;
/// Largest 24-bit sample as a float on the decoder's grid (`8388607 / 2^23`).
const FULL_SCALE_24: f64 = 8_388_607.0 / 8_388_608.0;

fn write_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new("write_failed", "The export could not be written (disk full, or the drive was removed?).", e.to_string())
}

fn cancelled() -> EngineError {
    EngineError::new("cancelled", "Export cancelled.", "")
}

/// Receives each block of rendered output (left, right).
pub(crate) type Sink<'a> = dyn FnMut(&[f32], &[f32]) -> Result<(), EngineError> + 'a;

/// Render the whole song through the graph, handing each block of output frames
/// (planar, song frames in order, latency removed) to `sink`. Returns frames rendered.
pub(crate) fn render_stream(
    song: &Path,
    p: &MixParams,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    sink: &mut Sink<'_>,
) -> Result<(u32, u64), EngineError> {
    let mut src = SongSource::open(song)?;
    let info = src.info().clone();
    let mut cfg = GraphConfig::new(info.sample_rate);
    cfg.max_block = BLOCK;
    let mut graph = PitchGraph::new(cfg, p.to_dsp()).map_err(EngineError::internal)?;
    let lat = graph.latency() as u64;
    let frames = info.frames;
    let total = frames + lat;
    let mut bufs = slot_buffers(BLOCK);
    let zeros = vec![0.0f32; BLOCK];
    let (mut ol, mut or) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let used = info.used;
    let extra = info.has_extra();
    let mut pos = 0u64;
    let mut written = 0u64;
    let mut last_progress = 0u64;
    while pos < total {
        if cancel.load(Relaxed) {
            return Err(cancelled());
        }
        let n = BLOCK.min((total - pos) as usize);
        if pos < frames {
            src.read(&mut bufs, n)?;
        } else {
            for s in bufs.iter_mut() {
                for c in s.iter_mut() {
                    c[..n].iter_mut().for_each(|v| *v = 0.0);
                }
            }
        }
        {
            let slot = |s: usize, c: usize| -> &[f32] { if used[s] { &bufs[s][c][..] } else { &zeros[..] } };
            let blk = StemBlock { stems: std::array::from_fn(|s| [slot(s, 0), slot(s, 1)]) };
            let x = ExtraBlock {
                pitched: [slot(SLOT_EXTRA_PITCHED, 0), slot(SLOT_EXTRA_PITCHED, 1)],
                unpitched: [slot(SLOT_EXTRA_UNPITCHED, 0), slot(SLOT_EXTRA_UNPITCHED, 1)],
            };
            graph.process_ext(&blk, if extra { Some(&x) } else { None }, n, [&mut ol[..n], &mut or[..n]]);
        }
        // Output frame (pos + i) is song frame (pos + i - L).
        let skip = lat.saturating_sub(pos).min(n as u64) as usize;
        let keep = (n - skip).min((frames - written) as usize);
        if keep > 0 {
            sink(&ol[skip..skip + keep], &or[skip..skip + keep])?;
            written += keep as u64;
        }
        pos += n as u64;
        if pos - last_progress >= 44_100 {
            last_progress = pos;
            progress(pos as f32 / total as f32);
        }
    }
    let st = graph.stats();
    if st.shift_shortfall_frames != 0 {
        return Err(EngineError::internal(format!("Rubber Band shortfall of {} frames", st.shift_shortfall_frames)));
    }
    Ok((info.sample_rate, written))
}

fn temp_for(out: &Path) -> PathBuf {
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    let nonce = crate::shared::now_ns() ^ ((std::process::id() as u64) << 32);
    out.with_file_name(format!("{name}.tmp-{nonce:016x}"))
}

/// Removes the temporary file unless disarmed.
struct TempGuard(Option<PathBuf>);
impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub fn render_offline(
    song: &Path,
    p: &MixParams,
    out: &Path,
    fmt: ExportFormat,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<RenderReport, EngineError> {
    let tmp = temp_for(out);
    let mut guard = TempGuard(Some(tmp.clone()));
    let report = match fmt {
        ExportFormat::Wav32f => {
            let file = File::create(&tmp).map_err(write_err)?;
            let mut w = WavF32Writer::new(BufWriter::with_capacity(1 << 20, file), 0, CHANNELS as u16).map_err(write_err)?;
            let mut peak = 0.0f32;
            let mut ilv = Vec::with_capacity(2 * BLOCK);
            let (rate, frames) = render_stream(song, p, cancel, &mut |x| progress(x * 0.99), &mut |l, r| {
                ilv.clear();
                for i in 0..l.len() {
                    peak = peak.max(l[i].abs()).max(r[i].abs());
                    ilv.push(l[i]);
                    ilv.push(r[i]);
                }
                w.write(&ilv).map_err(write_err)
            })?;
            let bw = w.finish(rate).map_err(write_err)?;
            let f = bw.into_inner().map_err(|e| write_err(e.error()))?;
            f.sync_all().map_err(write_err)?;
            drop(f);
            RenderReport { peak, applied_gain_db: 0.0, frames, sample_rate: rate, channels: CHANNELS as u16 }
        }
        ExportFormat::Flac24 => {
            // Pass 1: render once into a temporary float file next to the output while
            // measuring the peak (rendering at a key other than 0 costs ~0.12 x real time
            // on the 9700X, so a second render would double the export time).
            // "<out>.tmp-<nonce>.f32": keeps the nonce (with_extension would replace it,
            // so two exports to the same name would share one temporary file).
            let raw = {
                let t = temp_for(out);
                let n = t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                t.with_file_name(format!("{n}.f32"))
            };
            let raw_guard = TempGuard(Some(raw.clone()));
            let mut peak = 0.0f32;
            let (rate, frames) = {
                let f = File::create(&raw).map_err(write_err)?;
                let mut w = BufWriter::with_capacity(1 << 20, f);
                let mut bytes = Vec::with_capacity(8 * BLOCK);
                let r = render_stream(song, p, cancel, &mut |x| progress(x * 0.85), &mut |l, r| {
                    bytes.clear();
                    for i in 0..l.len() {
                        peak = peak.max(l[i].abs()).max(r[i].abs());
                        bytes.extend_from_slice(&l[i].to_le_bytes());
                        bytes.extend_from_slice(&r[i].to_le_bytes());
                    }
                    w.write_all(&bytes).map_err(write_err)
                })?;
                w.flush().map_err(write_err)?;
                r
            };
            let gain = if peak as f64 > FULL_SCALE_24 { FULL_SCALE_24 / peak as f64 } else { 1.0 };
            // Pass 2: scale (only if needed), round to 24 bits, encode.
            let file = File::create(&tmp).map_err(write_err)?;
            let mut enc = FlacEncoder::new(BufWriter::with_capacity(1 << 20, file), CHANNELS, 24, rate, Some(frames))
                .map_err(write_err)?;
            {
                let mut rd = std::io::BufReader::with_capacity(1 << 20, File::open(&raw).map_err(write_err)?);
                let mut buf = vec![0u8; 8 * 8192];
                let mut ints = Vec::with_capacity(2 * 8192);
                let mut left = frames;
                while left > 0 {
                    if cancel.load(Relaxed) {
                        return Err(cancelled());
                    }
                    let n = left.min(8192) as usize;
                    std::io::Read::read_exact(&mut rd, &mut buf[..8 * n]).map_err(write_err)?;
                    ints.clear();
                    for b in buf[..8 * n].as_chunks::<4>().0 {
                        let v = f32::from_le_bytes(*b);
                        let q = (v as f64 * gain * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0);
                        ints.push(q as i32);
                    }
                    enc.write_interleaved(&ints).map_err(write_err)?;
                    left -= n as u64;
                    progress(0.85 + 0.1 * (1.0 - left as f32 / frames.max(1) as f32));
                }
            }
            drop(raw_guard);
            let (bw, summary) = enc.finish().map_err(write_err)?;
            let f = bw.into_inner().map_err(|e| write_err(e.error()))?;
            f.sync_all().map_err(write_err)?;
            drop(f);
            if summary.frames != frames {
                return Err(EngineError::internal("FLAC frame count mismatch"));
            }
            // Decode back: frame count and the audio MD5 must match.
            verify_flac(&tmp, frames)?;
            progress(0.99);
            RenderReport {
                peak,
                applied_gain_db: if gain < 1.0 { 20.0 * gain.log10() } else { 0.0 },
                frames,
                sample_rate: rate,
                channels: CHANNELS as u16,
            }
        }
    };
    std::fs::rename(&tmp, out).map_err(write_err)?;
    guard.0 = None;
    progress(1.0);
    Ok(report)
}

/// Decode a whole FLAC file with symphonia and check its length and MD5.
fn verify_flac(path: &Path, frames: u64) -> Result<(), EngineError> {
    let f = File::open(path).map_err(write_err)?;
    let len = f.metadata().map_err(write_err)?.len();
    let reader = jk_format::ChunkReader::new(f, 0, len);
    let mut dec = jk_format::decode::StemDecoder::new(reader, true).map_err(|e| EngineError::internal(format!("export decode: {e}")))?;
    let mut block = Vec::new();
    let mut n = 0u64;
    while let Some((_, k)) = dec.next_block(&mut block).map_err(|e| EngineError::internal(format!("export decode: {e}")))? {
        n += k as u64;
    }
    if n != frames {
        return Err(EngineError::internal(format!("export decodes to {n} frames, expected {frames}")));
    }
    if dec.finish() != Some(true) {
        return Err(EngineError::internal("export FLAC MD5 check failed"));
    }
    Ok(())
}

/// Streaming 32-bit float WAV writer (WAVE_FORMAT_IEEE_FLOAT). Reserves a JUNK chunk
/// that becomes the `ds64` chunk of an RF64 file (EBU Tech 3306) when the data exceeds
/// 4 GiB.
pub struct WavF32Writer<W: Write + Seek> {
    w: W,
    channels: u16,
    data_bytes: u64,
}

impl<W: Write + Seek> WavF32Writer<W> {
    /// `rate` may be 0 here and given to `finish` instead.
    pub fn new(mut w: W, rate: u32, channels: u16) -> std::io::Result<Self> {
        let mut h = Vec::with_capacity(80);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"JUNK");
        h.extend_from_slice(&28u32.to_le_bytes());
        h.extend_from_slice(&[0u8; 28]);
        Self::fmt_chunk(&mut h, rate, channels);
        h.extend_from_slice(b"data");
        h.extend_from_slice(&0u32.to_le_bytes());
        w.write_all(&h)?;
        Ok(WavF32Writer { w, channels, data_bytes: 0 })
    }

    fn fmt_chunk(h: &mut Vec<u8>, rate: u32, channels: u16) {
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&18u32.to_le_bytes());
        h.extend_from_slice(&3u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&rate.to_le_bytes());
        h.extend_from_slice(&(rate * channels as u32 * 4).to_le_bytes());
        h.extend_from_slice(&(channels * 4).to_le_bytes());
        h.extend_from_slice(&32u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // cbSize
    }

    /// Append interleaved samples.
    pub fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        let mut bytes = [0u8; 4096];
        for chunk in samples.chunks(1024) {
            for (i, v) in chunk.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            self.w.write_all(&bytes[..chunk.len() * 4])?;
        }
        self.data_bytes += samples.len() as u64 * 4;
        Ok(())
    }

    /// Patch the header (sizes, rate) and flush. Returns the inner writer.
    pub fn finish(mut self, rate: u32) -> std::io::Result<W> {
        let header_len = 12 + 36 + 26 + 8u64;
        let riff = header_len - 8 + self.data_bytes;
        let mut fmt = Vec::new();
        Self::fmt_chunk(&mut fmt, rate, self.channels);
        self.w.seek(SeekFrom::Start(0))?;
        if riff > u32::MAX as u64 || self.data_bytes > u32::MAX as u64 {
            let frames = self.data_bytes / (4 * self.channels as u64);
            self.w.write_all(b"RF64")?;
            self.w.write_all(&u32::MAX.to_le_bytes())?;
            self.w.write_all(b"WAVE")?;
            self.w.write_all(b"ds64")?;
            self.w.write_all(&28u32.to_le_bytes())?;
            self.w.write_all(&riff.to_le_bytes())?;
            self.w.write_all(&self.data_bytes.to_le_bytes())?;
            self.w.write_all(&frames.to_le_bytes())?;
            self.w.write_all(&0u32.to_le_bytes())?; // table length
            self.w.write_all(&fmt)?;
            self.w.write_all(b"data")?;
            self.w.write_all(&u32::MAX.to_le_bytes())?;
        } else {
            self.w.write_all(b"RIFF")?;
            self.w.write_all(&(riff as u32).to_le_bytes())?;
            self.w.write_all(b"WAVE")?;
            self.w.seek(SeekFrom::Current(36))?; // JUNK chunk stays
            self.w.write_all(&fmt)?;
            self.w.write_all(b"data")?;
            self.w.write_all(&(self.data_bytes as u32).to_le_bytes())?;
        }
        self.w.seek(SeekFrom::End(0))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

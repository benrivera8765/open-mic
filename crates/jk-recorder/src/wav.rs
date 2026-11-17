// SPDX-License-Identifier: GPL-3.0-or-later
//! Progressive 32-bit float WAV writer that turns into RF64 above 4 GiB.
//!
//! # File layout
//!
//! ```text
//! offset  RIFF (< 4 GiB)                    RF64 (>= 4 GiB, EBU Tech 3306)
//!      0  "RIFF"                            "RF64"
//!      4  u32 file length - 8               0xFFFF_FFFF
//!      8  "WAVE"                            "WAVE"
//!     12  "JUNK" (28 bytes, zeros)          "ds64" (28 bytes):
//!     20                                      u64 RIFF size, u64 data size,
//!                                             u64 sample frames, u32 table length = 0
//!     48  "fmt " IEEE float (tag 3), or WAVE_FORMAT_EXTENSIBLE with the float
//!         sub-format GUID for more than two channels
//!         "fact" u32 sample frames          "fact" 0xFFFF_FFFF
//!         "data" u32 data size              "data" 0xFFFF_FFFF
//!         little-endian f32 samples, interleaved
//! ```
//!
//! The `JUNK` chunk reserves the room for a `ds64` chunk from the start (the
//! method EBU Tech 3306 recommends), so promotion to RF64 rewrites a few header
//! bytes in place instead of moving gigabytes of audio.
//!
//! # Progressive writing
//!
//! Samples are appended through a buffered writer. [`WavWriter::checkpoint`]
//! flushes and patches the sizes in the header, so if a USB stick is pulled or
//! the app dies, the file on disk is a valid WAV of everything up to the last
//! checkpoint (the recorder checkpoints about once a second). [`repair`] fixes
//! the header of a file that was cut off between checkpoints.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// `WAVE_FORMAT_IEEE_FLOAT`.
pub const FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// `WAVE_FORMAT_EXTENSIBLE`.
pub const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT` {00000003-0000-0010-8000-00AA00389B71},
/// in the byte order it has inside a WAV file.
const SUBTYPE_IEEE_FLOAT: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Size of the `ds64` chunk body we reserve (RIFF size, data size, sample count:
/// three u64; table length: one u32).
const DS64_BODY_LEN: u32 = 28;
/// Offset of the first byte of the reserved `JUNK`/`ds64` body.
const DS64_BODY_OFFSET: u64 = 20;
/// Default promotion point: the RIFF size field is a u32, and 0xFFFF_FFFF is the
/// RF64 marker value, so anything at or above it must be RF64.
const RIFF_LIMIT: u64 = 0xFFFF_FFFF;

/// What to write in the `fmt ` chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavSpec {
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channel count (1..=65535).
    pub channels: u16,
    /// `dwChannelMask` for WAVE_FORMAT_EXTENSIBLE (used only above 2 channels).
    /// `None` writes 0 ("no speaker positions"), except for the common 5.1 and 7.1
    /// counts, which get the standard masks.
    pub channel_mask: Option<u32>,
}

impl WavSpec {
    /// Stereo or mono float WAV at `sample_rate`.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            channel_mask: None,
        }
    }

    fn block_align(&self) -> u64 {
        4 * u64::from(self.channels)
    }
}

/// Final numbers after [`WavWriter::finalize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavSummary {
    /// Sample frames in the file.
    pub frames: u64,
    /// Total file length in bytes.
    pub file_bytes: u64,
    /// True if the file was written as RF64.
    pub rf64: bool,
}

/// Progressive float WAV / RF64 writer. See the module documentation.
pub struct WavWriter {
    out: BufWriter<File>,
    spec: WavSpec,
    fact_value_offset: u64,
    data_size_offset: u64,
    data_start: u64,
    data_bytes: u64,
    rf64: bool,
    riff_limit: u64,
    scratch: Vec<u8>,
}

impl std::fmt::Debug for WavWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WavWriter")
            .field("spec", &self.spec)
            .field("data_bytes", &self.data_bytes)
            .field("rf64", &self.rf64)
            .finish()
    }
}

impl WavWriter {
    /// Create a new file at `path` (fails if it already exists, so a recording
    /// can never overwrite another one) and write the header.
    pub fn create(path: &Path, spec: WavSpec) -> io::Result<Self> {
        Self::create_with_limit(path, spec, RIFF_LIMIT)
    }

    /// Like [`WavWriter::create`] but with a custom RF64 promotion point, so
    /// tests can exercise RF64 without writing 4 GiB.
    #[doc(hidden)]
    pub fn create_with_limit(path: &Path, spec: WavSpec, riff_limit: u64) -> io::Result<Self> {
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WAV needs at least one channel and a non-zero rate",
            ));
        }
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let (header, fact_value_offset, data_size_offset) = build_header(&spec);
        let mut out = BufWriter::with_capacity(1 << 18, file);
        out.write_all(&header)?;
        let mut w = Self {
            out,
            spec,
            fact_value_offset,
            data_size_offset,
            data_start: header.len() as u64,
            data_bytes: 0,
            rf64: false,
            riff_limit: riff_limit.min(RIFF_LIMIT),
            scratch: Vec::new(),
        };
        // Make the empty file valid on disk straight away.
        w.checkpoint()?;
        Ok(w)
    }

    /// The format being written.
    pub fn spec(&self) -> WavSpec {
        self.spec
    }

    /// Sample frames written so far (including any not yet flushed).
    pub fn frames_written(&self) -> u64 {
        self.data_bytes / self.spec.block_align()
    }

    /// File length once everything buffered is flushed.
    pub fn file_bytes(&self) -> u64 {
        self.data_start + self.data_bytes
    }

    /// True once the file has been promoted to RF64.
    pub fn is_rf64(&self) -> bool {
        self.rf64
    }

    /// Append interleaved samples. `samples.len()` must be a multiple of the
    /// channel count. The values are stored bit-for-bit (no clamping: float WAV
    /// can hold values beyond full scale).
    pub fn write_interleaved(&mut self, samples: &[f32]) -> io::Result<()> {
        if !samples
            .len()
            .is_multiple_of(usize::from(self.spec.channels))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} samples is not a whole number of {}-channel frames",
                    samples.len(),
                    self.spec.channels
                ),
            ));
        }
        self.scratch.clear();
        self.scratch.reserve(samples.len() * 4);
        for s in samples {
            self.scratch.extend_from_slice(&s.to_le_bytes());
        }
        self.out.write_all(&self.scratch)?;
        self.data_bytes += self.scratch.len() as u64;
        Ok(())
    }

    /// Append `frames` frames of digital silence (used to fill timestamp gaps).
    pub fn write_silence(&mut self, frames: u64) -> io::Result<()> {
        const CHUNK: usize = 16 * 1024;
        let zeros = [0u8; CHUNK];
        let mut remaining = frames * self.spec.block_align();
        while remaining > 0 {
            let n = remaining.min(CHUNK as u64) as usize;
            self.out.write_all(&zeros[..n])?;
            remaining -= n as u64;
        }
        self.data_bytes += frames * self.spec.block_align();
        Ok(())
    }

    /// Flush buffered samples and patch the header sizes so the file on disk is
    /// a complete, valid WAV/RF64 of everything written so far.
    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.out.flush()?;
        let file_len = self.data_start + self.data_bytes;
        let riff_size = file_len - 8;
        let frames = self.frames_written();
        if !self.rf64 && riff_size >= self.riff_limit {
            self.rf64 = true;
        }
        let f = self.out.get_mut();
        if self.rf64 {
            patch(f, 0, b"RF64")?;
            patch(f, 4, &u32::MAX.to_le_bytes())?;
            patch(f, 12, b"ds64")?;
            let mut body = [0u8; DS64_BODY_LEN as usize];
            body[0..8].copy_from_slice(&riff_size.to_le_bytes());
            body[8..16].copy_from_slice(&self.data_bytes.to_le_bytes());
            body[16..24].copy_from_slice(&frames.to_le_bytes());
            body[24..28].copy_from_slice(&0u32.to_le_bytes()); // no extra size table
            patch(f, DS64_BODY_OFFSET, &body)?;
            patch(f, self.fact_value_offset, &u32::MAX.to_le_bytes())?;
            patch(f, self.data_size_offset, &u32::MAX.to_le_bytes())?;
        } else {
            patch(f, 4, &(riff_size as u32).to_le_bytes())?;
            patch(
                f,
                self.fact_value_offset,
                &(frames.min(u64::from(u32::MAX)) as u32).to_le_bytes(),
            )?;
            patch(
                f,
                self.data_size_offset,
                &(self.data_bytes as u32).to_le_bytes(),
            )?;
        }
        f.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Ask the OS to put the data on the device (used by the recorder every few
    /// seconds, because USB sticks get pulled).
    pub fn sync(&mut self) -> io::Result<()> {
        self.checkpoint()?;
        self.out.get_ref().sync_data()
    }

    /// Final checkpoint, flush to the device, close.
    pub fn finalize(mut self) -> io::Result<WavSummary> {
        self.checkpoint()?;
        self.out.get_ref().sync_all()?;
        Ok(WavSummary {
            frames: self.frames_written(),
            file_bytes: self.file_bytes(),
            rf64: self.rf64,
        })
    }
}

fn patch(f: &mut File, offset: u64, bytes: &[u8]) -> io::Result<()> {
    f.seek(SeekFrom::Start(offset))?;
    f.write_all(bytes)
}

/// Build the header; returns (bytes, offset of the fact value, offset of the
/// data size field).
fn build_header(spec: &WavSpec) -> (Vec<u8>, u64, u64) {
    let extensible = spec.channels > 2;
    let mut h = Vec::with_capacity(128);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&0u32.to_le_bytes()); // patched by checkpoint()
    h.extend_from_slice(b"WAVE");
    // Reserved room for ds64 (EBU Tech 3306 section 3.3 "JUNK" method).
    h.extend_from_slice(b"JUNK");
    h.extend_from_slice(&DS64_BODY_LEN.to_le_bytes());
    h.extend_from_slice(&[0u8; DS64_BODY_LEN as usize]);
    debug_assert_eq!(h.len(), 48);

    let block_align = 4 * spec.channels;
    let byte_rate = spec.sample_rate * u32::from(block_align);
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&(if extensible { 40u32 } else { 18u32 }).to_le_bytes());
    h.extend_from_slice(
        &(if extensible {
            FORMAT_EXTENSIBLE
        } else {
            FORMAT_IEEE_FLOAT
        })
        .to_le_bytes(),
    );
    h.extend_from_slice(&spec.channels.to_le_bytes());
    h.extend_from_slice(&spec.sample_rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes()); // bits per sample
    if extensible {
        h.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        h.extend_from_slice(&32u16.to_le_bytes()); // valid bits per sample
        let mask = spec.channel_mask.unwrap_or(match spec.channels {
            6 => 0x3F,  // 5.1: FL FR FC LFE BL BR
            8 => 0x63F, // 7.1: 5.1 + SL SR
            _ => 0,
        });
        h.extend_from_slice(&mask.to_le_bytes());
        h.extend_from_slice(&SUBTYPE_IEEE_FLOAT);
    } else {
        h.extend_from_slice(&0u16.to_le_bytes()); // cbSize = 0
    }
    // A non-PCM WAV must carry a fact chunk with the frame count.
    h.extend_from_slice(b"fact");
    h.extend_from_slice(&4u32.to_le_bytes());
    let fact_value_offset = h.len() as u64;
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"data");
    let data_size_offset = h.len() as u64;
    h.extend_from_slice(&0u32.to_le_bytes());
    (h, fact_value_offset, data_size_offset)
}

// ---------------------------------------------------------------------------
// Reading (tests, the probe, and header repair). Accepts any float WAV/RF64,
// not only our own layout: unknown chunks are skipped.
// ---------------------------------------------------------------------------

/// What [`read_header`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
    /// True if the container is RF64.
    pub rf64: bool,
    /// Byte offset of the first sample.
    pub data_offset: u64,
    /// Data length in bytes according to the header (RF64: from ds64).
    pub data_bytes: u64,
    /// Sample frames according to the data length.
    pub frames: u64,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Parse the header of a 32-bit float WAV or RF64 file.
pub fn read_header(path: &Path) -> io::Result<WavInfo> {
    let mut f = File::open(path)?;
    read_header_from(&mut f)
}

fn read_header_from(f: &mut File) -> io::Result<WavInfo> {
    let file_len = f.metadata()?.len();
    let mut id = [0u8; 12];
    f.read_exact(&mut id)?;
    let rf64 = match &id[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return Err(invalid("not a RIFF/RF64 file")),
    };
    if &id[8..12] != b"WAVE" {
        return Err(invalid("not a WAVE file"));
    }
    let mut ds64_data: Option<u64> = None;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (tag, channels, rate, bits)
    let mut pos = 12u64;
    loop {
        if pos + 8 > file_len {
            return Err(invalid("no data chunk"));
        }
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)?;
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]);
        let body = pos + 8;
        match &ch[0..4] {
            b"ds64" => {
                let mut b = [0u8; 24];
                f.read_exact(&mut b)?;
                ds64_data = Some(u64::from_le_bytes(b[8..16].try_into().unwrap()));
            }
            b"fmt " => {
                let mut b = vec![0u8; size.min(64) as usize];
                f.read_exact(&mut b)?;
                if b.len() < 16 {
                    return Err(invalid("fmt chunk too short"));
                }
                let mut tag = u16::from_le_bytes([b[0], b[1]]);
                let channels = u16::from_le_bytes([b[2], b[3]]);
                let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let bits = u16::from_le_bytes([b[14], b[15]]);
                if tag == FORMAT_EXTENSIBLE && b.len() >= 40 && b[24..40] == SUBTYPE_IEEE_FLOAT {
                    tag = FORMAT_IEEE_FLOAT;
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) =
                    fmt.ok_or_else(|| invalid("data chunk before fmt chunk"))?;
                if tag != FORMAT_IEEE_FLOAT || bits != 32 || channels == 0 {
                    return Err(invalid(format!(
                        "not 32-bit float (format tag {tag:#06x}, {bits} bits)"
                    )));
                }
                let data_bytes = if rf64 && size == u32::MAX {
                    ds64_data.ok_or_else(|| invalid("RF64 without ds64"))?
                } else {
                    u64::from(size)
                };
                let block = 4 * u64::from(channels);
                return Ok(WavInfo {
                    sample_rate: rate,
                    channels,
                    rf64,
                    data_offset: body,
                    data_bytes,
                    frames: data_bytes / block,
                });
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        pos = body + u64::from(size) + u64::from(size & 1);
    }
}

/// Read a whole 32-bit float WAV/RF64 into memory (for tests and the probe; the
/// app itself never loads recordings whole).
pub fn read_wav_f32(path: &Path) -> io::Result<(WavInfo, Vec<f32>)> {
    let mut f = File::open(path)?;
    let info = read_header_from(&mut f)?;
    f.seek(SeekFrom::Start(info.data_offset))?;
    let mut bytes = vec![
        0u8;
        usize::try_from(info.frames * 4 * u64::from(info.channels))
            .map_err(|_| invalid("too large"))?
    ];
    f.read_exact(&mut bytes)?;
    let samples = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Ok((info, samples))
}

/// Result of [`repair`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairReport {
    /// Frames the header claimed before the repair.
    pub frames_before: u64,
    /// Frames present after the repair (whole frames actually on disk).
    pub frames_after: u64,
    /// Bytes of a trailing partial frame that were cut off.
    pub truncated_bytes: u64,
    /// True if the repaired file is RF64.
    pub rf64: bool,
}

/// Fix the header of a recording written by [`WavWriter`] that was cut off
/// between checkpoints (stick pulled, app killed): count the whole frames that
/// are really on disk, drop a trailing partial frame, and rewrite the sizes.
/// Only files in this writer's layout (reserved ds64 room at offset 12, data
/// chunk last) are accepted.
pub fn repair(path: &Path) -> io::Result<RepairReport> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let info = read_header_from(&mut f)?;
    let mut tag = [0u8; 4];
    f.seek(SeekFrom::Start(12))?;
    f.read_exact(&mut tag)?;
    if &tag != b"JUNK" && &tag != b"ds64" {
        return Err(invalid(
            "not written by jk-recorder (no reserved ds64 room); refusing to repair",
        ));
    }
    let file_len = f.metadata()?.len();
    let block = 4 * u64::from(info.channels);
    let on_disk = file_len.saturating_sub(info.data_offset);
    let whole = on_disk / block * block;
    let truncated = on_disk - whole;
    if truncated > 0 {
        f.set_len(info.data_offset + whole)?;
    }
    let frames = whole / block;
    let riff_size = info.data_offset + whole - 8;
    // The fact chunk sits right before the data chunk header in our layout.
    let data_size_offset = info.data_offset - 4;
    let fact_value_offset = info.data_offset - 12;
    let rf64 = riff_size >= RIFF_LIMIT;
    if rf64 {
        patch(&mut f, 0, b"RF64")?;
        patch(&mut f, 4, &u32::MAX.to_le_bytes())?;
        patch(&mut f, 12, b"ds64")?;
        let mut body = [0u8; DS64_BODY_LEN as usize];
        body[0..8].copy_from_slice(&riff_size.to_le_bytes());
        body[8..16].copy_from_slice(&whole.to_le_bytes());
        body[16..24].copy_from_slice(&frames.to_le_bytes());
        patch(&mut f, DS64_BODY_OFFSET, &body)?;
        patch(&mut f, fact_value_offset, &u32::MAX.to_le_bytes())?;
        patch(&mut f, data_size_offset, &u32::MAX.to_le_bytes())?;
    } else {
        // Also undo an RF64 promotion if what survived is below 4 GiB: a
        // plain RIFF file must say "RIFF" and must not carry a ds64 chunk.
        patch(&mut f, 0, b"RIFF")?;
        patch(&mut f, 4, &(riff_size as u32).to_le_bytes())?;
        patch(&mut f, 12, b"JUNK")?;
        patch(&mut f, DS64_BODY_OFFSET, &[0u8; DS64_BODY_LEN as usize])?;
        patch(&mut f, fact_value_offset, &(frames as u32).to_le_bytes())?;
        patch(&mut f, data_size_offset, &(whole as u32).to_le_bytes())?;
    }
    f.sync_all()?;
    Ok(RepairReport {
        frames_before: info.frames,
        frames_after: frames,
        truncated_bytes: truncated,
        rf64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A unique temp path per test (no external temp-dir crate needed).
    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "jk-recorder-test-{}-{name}.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn ramp(frames: usize, channels: usize) -> Vec<f32> {
        // Includes values beyond full scale (|x| up to 1.25) and a negative
        // zero, which must survive bit-exactly.
        let mut v: Vec<f32> = (0..frames * channels)
            .map(|i| ((i as f32) * 0.37).sin() * 1.25)
            .collect();
        v[0] = -0.0;
        v
    }

    #[test]
    fn stereo_float_round_trip_is_bit_exact() {
        let p = temp_path("stereo");
        let samples = ramp(1000, 2);
        let mut w = WavWriter::create(&p, WavSpec::new(48_000, 2)).unwrap();
        w.write_interleaved(&samples[..600]).unwrap();
        w.write_interleaved(&samples[600..]).unwrap();
        let s = w.finalize().unwrap();
        assert_eq!(
            s,
            WavSummary {
                frames: 1000,
                file_bytes: 94 + 8000,
                rf64: false
            }
        );
        let (info, back) = read_wav_f32(&p).unwrap();
        assert_eq!(
            (info.sample_rate, info.channels, info.rf64, info.frames),
            (48_000, 2, false, 1000)
        );
        assert_eq!(info.data_offset, 94);
        assert!(
            back.iter()
                .zip(&samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        // Header fields a strict reader checks.
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(&bytes[12..16], b"JUNK");
        assert_eq!(
            u16::from_le_bytes(bytes[56..58].try_into().unwrap()),
            FORMAT_IEEE_FLOAT
        );
        assert_eq!(
            u32::from_le_bytes(bytes[64..68].try_into().unwrap()),
            48_000 * 8
        ); // byte rate
        assert_eq!(&bytes[74..78], b"fact");
        assert_eq!(u32::from_le_bytes(bytes[82..86].try_into().unwrap()), 1000);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn refuses_to_overwrite() {
        let p = temp_path("exists");
        std::fs::write(&p, b"keep me").unwrap();
        assert!(WavWriter::create(&p, WavSpec::new(48_000, 2)).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"keep me");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn promotes_to_rf64_past_the_limit() {
        let p = temp_path("rf64");
        // Promote once the RIFF size reaches 1000 bytes instead of 4 GiB.
        let mut w = WavWriter::create_with_limit(&p, WavSpec::new(44_100, 2), 1000).unwrap();
        let samples = ramp(300, 2); // 2400 bytes of data
        w.write_interleaved(&samples).unwrap();
        w.write_silence(5).unwrap();
        let s = w.finalize().unwrap();
        assert!(s.rf64);
        assert_eq!(s.frames, 305);
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[0..4], b"RF64");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            u32::MAX
        );
        assert_eq!(&bytes[12..16], b"ds64");
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 28);
        assert_eq!(
            u64::from_le_bytes(bytes[20..28].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(
            u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
            305 * 8
        );
        assert_eq!(u64::from_le_bytes(bytes[36..44].try_into().unwrap()), 305);
        assert_eq!(
            u32::from_le_bytes(bytes[90..94].try_into().unwrap()),
            u32::MAX
        ); // data size marker
        let (info, back) = read_wav_f32(&p).unwrap();
        assert!(info.rf64);
        assert_eq!(info.frames, 305);
        assert!(
            back[..600]
                .iter()
                .zip(&samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert!(back[600..].iter().all(|&x| x.to_bits() == 0));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn multichannel_uses_extensible_float() {
        let p = temp_path("six");
        let mut w = WavWriter::create(&p, WavSpec::new(48_000, 6)).unwrap();
        w.write_interleaved(&ramp(10, 6)).unwrap();
        assert!(w.write_interleaved(&[0.0; 7]).is_err()); // not whole frames
        w.finalize().unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(
            u16::from_le_bytes(bytes[56..58].try_into().unwrap()),
            FORMAT_EXTENSIBLE
        );
        assert_eq!(u32::from_le_bytes(bytes[76..80].try_into().unwrap()), 0x3F);
        assert_eq!(bytes[80..96], SUBTYPE_IEEE_FLOAT);
        let (info, back) = read_wav_f32(&p).unwrap();
        assert_eq!((info.channels, info.frames, back.len()), (6, 10, 60));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn checkpoint_leaves_a_valid_file_and_repair_recovers_the_rest() {
        let p = temp_path("progressive");
        let mut w = WavWriter::create(&p, WavSpec::new(48_000, 2)).unwrap();
        w.write_interleaved(&ramp(100, 2)).unwrap();
        w.checkpoint().unwrap();
        // Another 50 frames reach the disk but the header is never patched,
        // plus 3 stray bytes of a partial frame: a pulled USB stick.
        w.write_interleaved(&ramp(50, 2)).unwrap();
        w.out.flush().unwrap();
        w.out.get_mut().write_all(&[1, 2, 3]).unwrap();
        w.out.get_mut().flush().unwrap();
        drop(w);
        let info = read_header(&p).unwrap();
        assert_eq!(info.frames, 100, "header reflects the last checkpoint");
        let r = repair(&p).unwrap();
        assert_eq!(
            r,
            RepairReport {
                frames_before: 100,
                frames_after: 150,
                truncated_bytes: 3,
                rf64: false
            }
        );
        let (info, back) = read_wav_f32(&p).unwrap();
        assert_eq!(info.frames, 150);
        assert_eq!(back.len(), 300);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn repair_turns_an_rf64_file_below_4_gib_back_into_plain_riff() {
        // A file promoted to RF64 whose surviving data is below 4 GiB (here:
        // promoted early through the test limit) must come out of repair as
        // a consistent RIFF file: "RIFF" magic, JUNK (not ds64) at offset 12.
        let p = temp_path("rf64-repair");
        let samples = ramp(300, 2);
        let mut w = WavWriter::create_with_limit(&p, WavSpec::new(48_000, 2), 1000).unwrap();
        w.write_interleaved(&samples).unwrap();
        assert!(w.finalize().unwrap().rf64);
        let r = repair(&p).unwrap();
        assert!(!r.rf64);
        assert_eq!(r.frames_after, 300);
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        assert_eq!(&bytes[12..16], b"JUNK");
        assert!(bytes[20..48].iter().all(|&b| b == 0));
        let (info, back) = read_wav_f32(&p).unwrap();
        assert!(!info.rf64);
        assert_eq!(info.frames, 300);
        assert!(
            back.iter()
                .zip(&samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        std::fs::remove_file(&p).unwrap();
    }
}

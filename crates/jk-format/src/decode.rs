//! Decoding stem chunks with symphonia (feature `symphonia`, on by default).
//!
//! This is a small, allocation-tolerant helper for tools and tests (verification,
//! conformance, offline checks). The real-time engine builds its own reader on the same
//! [`ChunkReader`] + `MediaSource` adapter (`jk_engine::FlacStem`, which works around the
//! same symphonia seek problem the same way, see [`StemDecoder::seek`]).

use symphonia_bundle_flac::{FlacDecoder, FlacReader};
use symphonia_core::codecs::CodecParameters;
use symphonia_core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia_core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia_core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia_core::units::Timestamp;

use crate::chunk_reader::ChunkReader;
use crate::error::{FormatError, Result};
use crate::song::SongFile;

/// Full scale of a 24-bit sample: the true float is `integer / 8388608 * gain`.
pub const FULL_SCALE_24: f64 = 8_388_608.0;

fn dec_err(e: symphonia_core::errors::Error) -> FormatError {
    FormatError::damaged(format!("FLAC stream: {e}"))
}

/// Decodes one FLAC stem chunk packet by packet, yielding the true integer sample
/// values (for 24-bit stems: -8388608..=8388607), interleaved.
///
/// Packets come out contiguous or not at all: each packet must start where the previous
/// one ended (after a seek: where the seek landed), otherwise [`StemDecoder::next_block`]
/// returns an error instead of samples from the wrong place.
pub struct StemDecoder {
    reader: FlacReader<'static>,
    decoder: FlacDecoder,
    /// Another view of the same stem bytes; every seek opens a fresh demuxer on a new
    /// view of it (see [`StemDecoder::seek`]).
    source: ChunkReader,
    track_id: u32,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: u32,
    /// Total frames from STREAMINFO, if known.
    pub total_frames: Option<u64>,
    shift: u32,
    scratch: Vec<i32>,
    /// First frame of the next packet, once known (`None` before the first packet of
    /// the stream, which may start anywhere).
    next_ts: Option<u64>,
}

/// A FLAC demuxer on a stem chunk, reading from its first frame.
fn open_reader(source: ChunkReader) -> Result<FlacReader<'static>> {
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    FlacReader::try_new(mss, FormatOptions::default()).map_err(dec_err)
}

impl StemDecoder {
    /// Opens the FLAC stream in `reader`. With `verify_md5`, [`StemDecoder::finish`]
    /// compares the decoded audio with the STREAMINFO MD5.
    pub fn new(reader: ChunkReader, verify_md5: bool) -> Result<StemDecoder> {
        let source = reader.reopen()?;
        let reader = open_reader(reader)?;
        let track = reader.tracks().first().ok_or_else(|| FormatError::damaged("FLAC stream has no track"))?;
        let track_id = track.id;
        let total_frames = track.num_frames;
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            return Err(FormatError::damaged("FLAC stream has no audio parameters"));
        };
        let mut opts = AudioDecoderOptions::default();
        opts.verify = verify_md5;
        let decoder = FlacDecoder::try_new(&params, &opts).map_err(dec_err)?;
        let p = decoder.codec_params();
        let sample_rate = p.sample_rate.unwrap_or(0);
        let channels = p.channels.as_ref().map(|c| c.count()).unwrap_or(0);
        let bits_per_sample = p.bits_per_sample.unwrap_or(0);
        if !(1..=32).contains(&bits_per_sample) || channels == 0 {
            return Err(FormatError::damaged("FLAC stream has invalid bits per sample or channels"));
        }
        Ok(StemDecoder {
            reader,
            decoder,
            source,
            track_id,
            sample_rate,
            channels,
            bits_per_sample,
            total_frames,
            // The decoder left-justifies samples in i32; shift back to the true value.
            shift: 32 - bits_per_sample,
            scratch: Vec::new(),
            next_ts: None,
        })
    }

    /// Decodes the next packet. Replaces `out` with its interleaved integer samples and
    /// returns (first frame index of the packet, frames). `None` at the end.
    pub fn next_block(&mut self, out: &mut Vec<i32>) -> Result<Option<(u64, usize)>> {
        loop {
            let Some(packet) = self.reader.next_packet().map_err(dec_err)? else { return Ok(None) };
            if packet.track_id != self.track_id {
                continue;
            }
            let ts = packet.pts.get().max(0) as u64;
            if let Some(want) = self.next_ts
                && ts != want
            {
                return Err(FormatError::damaged(format!(
                    "FLAC stream: packet starts at frame {ts}, expected frame {want} (frames missing or out of order)"
                )));
            }
            let buf = self.decoder.decode(&packet).map_err(dec_err)?;
            let frames = buf.frames();
            self.next_ts = Some(ts + frames as u64);
            buf.copy_to_vec_interleaved::<i32>(&mut self.scratch);
            out.clear();
            out.extend(self.scratch.iter().map(|&s| s >> self.shift));
            return Ok(Some((ts, frames)));
        }
    }

    /// Seeks so that the next block contains `frame`; returns the first frame index of
    /// the packet the reader landed on. Sample-accurate positioning is done by the
    /// caller by skipping `frame - returned` frames of the next block.
    ///
    /// **Every seek starts a fresh demuxer.** symphonia-bundle-flac 0.6.1's packet
    /// parser keeps its state (half-built packets, the last frame header) across a seek
    /// when the seek lands exactly on a frame start: its `resync` only resets the parser
    /// if the read position moved, and a SEEKTABLE point or the first frame puts it
    /// exactly on a frame. The first packet after such a seek is right; later packets
    /// are assembled from stale fragments or skip frames, without any error (measured on
    /// the demo song: after reading a second from frame 0, a seek to frame 1 jumped from
    /// frame 4096 to frame 45056). A new demuxer on a new view of the same chunk has no
    /// state to go stale; the cost is reading the FLAC metadata blocks again (about a
    /// kilobyte) and one duplicated file handle. Packets after the seek are also checked
    /// for continuity (see [`StemDecoder::next_block`]).
    pub fn seek(&mut self, frame: u64) -> Result<u64> {
        let mut reader = open_reader(self.source.reopen()?)?;
        // A fresh demuxer already stands on the first frame; seeking to 0 would only
        // search for it again.
        let landed = if frame == 0 {
            None
        } else {
            let to = SeekTo::Timestamp { ts: Timestamp::new(frame as i64), track_id: self.track_id };
            Some(reader.seek(SeekMode::Accurate, to).map_err(dec_err)?.actual_ts.get().max(0) as u64)
        };
        self.reader = reader;
        self.decoder.reset();
        self.next_ts = landed;
        Ok(landed.unwrap_or(0))
    }

    /// `Some(true)` when the decoded audio matched the STREAMINFO MD5, `Some(false)` when
    /// not, `None` when verification was off or the stream has no MD5. Only meaningful
    /// after decoding the whole stream from the start without seeking.
    pub fn finish(&mut self) -> Option<bool> {
        self.decoder.finalize().verify_ok
    }
}

/// Decodes a whole stem into planar `f32` (`[channel][frame]`), applying the stored
/// gain. Loads the whole stem into memory: for tools and tests, not for playback.
pub fn decode_stem_f32(song: &SongFile, role: &str) -> Result<Vec<Vec<f32>>> {
    let (stem, reader) = song.stem_reader(role)?;
    let mut dec = StemDecoder::new(reader, false)?;
    // 24-bit stems (the only kind v1.0 writers produce): integer / 8388608 * gain.
    let scale = stem.gain / 2f64.powi(dec.bits_per_sample as i32 - 1);
    let mut planes = vec![Vec::new(); dec.channels];
    let mut block = Vec::new();
    while let Some((_, frames)) = dec.next_block(&mut block)? {
        for f in 0..frames {
            for (c, plane) in planes.iter_mut().enumerate() {
                plane.push((block[f * dec.channels + c] as f64 * scale) as f32);
            }
        }
    }
    Ok(planes)
}

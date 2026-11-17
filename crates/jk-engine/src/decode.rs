// SPDX-License-Identifier: GPL-3.0-or-later
//! FLAC stem decoding with sample-accurate seeking (symphonia).
//!
//! Integer FLAC samples are converted to f32 by symphonia (24-bit: `x / 2^23`, exact in
//! f32). Mono files are presented as two identical channels. The stream is read
//! incrementally from its source (a file, or a stem chunk of a `.jksong` through
//! `jk_format::ChunkReader`); nothing is decoded ahead of what `read` asks for.

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

#[derive(Debug)]
pub enum DecodeError {
    Io(PathBuf, std::io::Error),
    Format(PathBuf, String),
    Unsupported(PathBuf, &'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            DecodeError::Format(p, e) => write!(f, "{}: {e}", p.display()),
            DecodeError::Unsupported(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}
impl std::error::Error for DecodeError {}

/// Makes a fresh media source for the same stream (reopens the file, or a new view of
/// a song-file chunk).
pub type SourceFactory = Box<dyn Fn() -> std::io::Result<Box<dyn MediaSource>> + Send>;

/// A streaming FLAC reader that hands out planar stereo f32.
///
/// **Seeking rebuilds the demuxer.** symphonia-bundle-flac 0.6.1 keeps its packet
/// parser's state across a seek when the seek lands exactly on a frame start (its
/// `resync` only soft-resets the parser if the read position moved, and a SEEKTABLE point
/// or the first frame puts it exactly on a frame). The first packet after such a seek is
/// right, but later packets are assembled from stale fragments and decode to wrong
/// samples without any error (measured on the demo song: 45,900 of 50,000 samples wrong
/// after `seek(0)` following a read). So a stem that knows how to reopen its source
/// starts a new reader + decoder for every seek; the cost is reading the FLAC metadata
/// again (about a kilobyte).
pub struct FlacStem {
    path: PathBuf,
    reopen: Option<SourceFactory>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    bits_per_sample: u32,
    total_frames: Option<u64>,
    /// Decoded but not yet handed out: planar, `pending[ch][pending_pos..]`.
    pending: Vec<Vec<f32>>,
    pending_pos: usize,
    /// Frame index of the next frame `read` will return.
    position: u64,
    /// After a seek: the frame the next `read` must start at. Packets are trimmed against
    /// their own timestamps, so this does not depend on where the demuxer landed.
    seek_target: Option<u64>,
    eof: bool,
}

impl FlacStem {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let p = path.to_path_buf();
        let factory: SourceFactory = Box::new(move || Ok(Box::new(File::open(&p)?) as Box<dyn MediaSource>));
        Self::from_factory(factory, path)
    }

    /// Open a FLAC stream through a factory that can make fresh sources of the same bytes
    /// (used on every seek, see the type documentation). `name` is for error messages.
    pub fn from_factory(factory: SourceFactory, name: &Path) -> Result<Self, DecodeError> {
        let source = factory().map_err(|e| DecodeError::Io(name.into(), e))?;
        let mut s = Self::from_source(source, name)?;
        s.reopen = Some(factory);
        Ok(s)
    }

    /// Open a FLAC stream from one symphonia media source. Without a factory, seeks use
    /// the demuxer's own seek (see the type documentation for its limitation).
    pub fn from_source(source: Box<dyn MediaSource>, name: &Path) -> Result<Self, DecodeError> {
        let (format, decoder, track_id, params) = Self::build(source, name)?;
        let path = name;
        let sample_rate = params.sample_rate.ok_or(DecodeError::Unsupported(path.into(), "unknown sample rate"))?;
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(0);
        if channels == 0 || channels > 2 {
            return Err(DecodeError::Unsupported(path.into(), "stems must be mono or stereo"));
        }
        let bits_per_sample = params.bits_per_sample.unwrap_or(0);
        let total_frames = format.default_track(TrackType::Audio).and_then(|t| t.num_frames);
        Ok(FlacStem {
            path: path.into(),
            reopen: None,
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            bits_per_sample,
            total_frames,
            pending: vec![Vec::new(), Vec::new()],
            pending_pos: 0,
            position: 0,
            seek_target: None,
            eof: false,
        })
    }

    /// Demuxer + decoder for a source.
    #[allow(clippy::type_complexity)]
    fn build(
        source: Box<dyn MediaSource>,
        path: &Path,
    ) -> Result<(Box<dyn FormatReader>, Box<dyn AudioDecoder>, u32, symphonia::core::codecs::audio::AudioCodecParameters), DecodeError> {
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| DecodeError::Format(path.into(), e.to_string()))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecodeError::Unsupported(path.into(), "no audio track"))?;
        let track_id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(DecodeError::Unsupported(path.into(), "no audio codec parameters"))?
            .clone();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| DecodeError::Format(path.into(), e.to_string()))?;
        Ok((format, decoder, track_id, params))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn bits_per_sample(&self) -> u32 {
        self.bits_per_sample
    }
    pub fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Seek so that the next `read` returns frame `frame` exactly.
    pub fn seek(&mut self, frame: u64) -> Result<(), DecodeError> {
        if let Some(f) = &self.reopen {
            // A fresh demuxer and decoder (see the type documentation).
            let source = f().map_err(|e| DecodeError::Io(self.path.clone(), e))?;
            let (format, decoder, track_id, _) = Self::build(source, &self.path)?;
            self.format = format;
            self.decoder = decoder;
            self.track_id = track_id;
        }
        if frame > 0 || self.reopen.is_none() {
            self.format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Timestamp { ts: Timestamp::new(frame as i64), track_id: self.track_id },
                )
                .map_err(|e| DecodeError::Format(self.path.clone(), format!("seek: {e}")))?;
        }
        self.decoder.reset();
        // Accurate mode lands on the packet at or before the target; the frames before the
        // target are dropped in `decode_next` using each packet's timestamp.
        self.seek_target = Some(frame);
        self.pending.iter_mut().for_each(|p| p.clear());
        self.pending_pos = 0;
        self.position = frame;
        self.eof = false;
        Ok(())
    }

    /// Decode the next packet into `pending`. Returns false at end of stream.
    fn decode_next(&mut self) -> Result<bool, DecodeError> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(false),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(e) => return Err(DecodeError::Format(self.path.clone(), e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(b) => b,
                Err(SymError::DecodeError(e)) => {
                    return Err(DecodeError::Format(self.path.clone(), format!("corrupt FLAC frame: {e}")))
                }
                Err(e) => return Err(DecodeError::Format(self.path.clone(), e.to_string())),
            };
            let frames = buf.frames();
            for p in self.pending.iter_mut() {
                p.resize(frames, 0.0);
            }
            if self.channels == 1 {
                let (a, b) = self.pending.split_at_mut(1);
                buf.copy_to_slice_planar::<f32, _>(&mut a[..]);
                b[0].copy_from_slice(&a[0]);
            } else {
                buf.copy_to_slice_planar::<f32, _>(&mut self.pending[..]);
            }
            self.pending_pos = 0;
            if let Some(target) = self.seek_target {
                let start = packet.pts.get().max(0) as u64;
                if start > target {
                    return Err(DecodeError::Format(self.path.clone(), "seek landed after the target frame".into()));
                }
                if start + frames as u64 <= target {
                    continue;
                }
                self.pending_pos = (target - start) as usize;
                self.seek_target = None;
            }
            return Ok(true);
        }
    }

    /// Read up to `max` frames into `out[ch][off..off+n]`. Returns `n`; 0 means end of stream.
    pub fn read(&mut self, out: &mut [Vec<f32>; 2], off: usize, max: usize) -> Result<usize, DecodeError> {
        let mut done = 0;
        while done < max {
            if self.pending_pos >= self.pending[0].len() && (self.eof || !self.decode_next()?) {
                self.eof = true;
                break;
            }
            let n = (self.pending[0].len() - self.pending_pos).min(max - done);
            for c in 0..2 {
                out[c][off + done..off + done + n]
                    .copy_from_slice(&self.pending[c][self.pending_pos..self.pending_pos + n]);
            }
            self.pending_pos += n;
            done += n;
        }
        self.position += done as u64;
        Ok(done)
    }

    /// Decode a whole file (tests, offline render).
    pub fn decode_all(path: &Path) -> Result<(u32, [Vec<f32>; 2]), DecodeError> {
        let mut s = FlacStem::open(path)?;
        let cap = s.total_frames().unwrap_or(0) as usize;
        let mut out = [Vec::with_capacity(cap), Vec::with_capacity(cap)];
        let mut buf = [vec![0.0f32; 65_536], vec![0.0f32; 65_536]];
        loop {
            let n = s.read(&mut buf, 0, 65_536)?;
            if n == 0 {
                break;
            }
            out[0].extend_from_slice(&buf[0][..n]);
            out[1].extend_from_slice(&buf[1][..n]);
        }
        Ok((s.sample_rate(), out))
    }
}

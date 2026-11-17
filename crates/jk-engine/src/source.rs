// SPDX-License-Identifier: GPL-3.0-or-later
//! A `.jksong` file as a stream of graph inputs.
//!
//! Each stem is decoded incrementally from its chunk of the song file (jk-format's
//! `ChunkReader`, a positional-read view with its own file handle, fed to symphonia's
//! FLAC decoder as a `MediaSource`): never a whole song in memory. The stored gain is
//! applied (`decoded_integer / 8388608 * gain`, SONG_FORMAT 4.2, computed in f64 like
//! jk-format's reference decoder), and each stem is routed to a graph slot by its role:
//!
//! | role             | slot                         |
//! |------------------|------------------------------|
//! | `lead_vocal`     | lead (guide vocal level)     |
//! | `backing_vocals` | backing (on/off, level)      |
//! | `drums`          | drums (level, "shift drums") |
//! | `other`          | other (level)                |
//! | anything else    | forward-compatibility rule: unity gain when `in_karaoke_mix`, through the pitch shifter when `pitched` (extra pitched / unpitched bus), not played otherwise |
//!
//! A role that appears twice (not allowed by the spec) is summed into its slot.
//! Missing roles are silence.

use std::path::{Path, PathBuf};

use crate::api::{EngineError, StemInfo};
use crate::decode::FlacStem;

/// Graph input slots. 0..4 are jk-dsp's `Stem` order; 4 and 5 are the extra buses.
pub const SLOT_LEAD: usize = 0;
pub const SLOT_BACKING: usize = 1;
pub const SLOT_DRUMS: usize = 2;
pub const SLOT_OTHER: usize = 3;
pub const SLOT_EXTRA_PITCHED: usize = 4;
pub const SLOT_EXTRA_UNPITCHED: usize = 5;
pub const SLOTS: usize = 6;

/// Planar stereo buffers for every slot: `bufs[slot][channel][frame]`.
pub type SlotBuffers = [[Vec<f32>; 2]; SLOTS];

pub fn slot_buffers(frames: usize) -> SlotBuffers {
    std::array::from_fn(|_| [vec![0.0; frames], vec![0.0; frames]])
}

/// What the engine needs to know about a song before playing it.
#[derive(Clone, Debug)]
pub struct SongInfo {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    /// Every stem in the file, with the flags the engine applies.
    pub stems: Vec<StemInfo>,
    /// Slots that receive audio.
    pub used: [bool; SLOTS],
}

impl SongInfo {
    pub fn duration_s(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }

    /// The slots that carry audio, in slot order.
    pub fn active_slots(&self) -> Vec<usize> {
        (0..SLOTS).filter(|&s| self.used[s]).collect()
    }

    /// True when the song has stems of roles the v1.0 graph has no slot for.
    pub fn has_extra(&self) -> bool {
        self.used[SLOT_EXTRA_PITCHED] || self.used[SLOT_EXTRA_UNPITCHED]
    }
}

struct StemDecoder {
    dec: FlacStem,
    slot: usize,
    /// 1.0 means "leave the samples untouched".
    gain: f64,
}

/// Streaming reader of a song's stems, routed to graph slots.
pub struct SongSource {
    info: SongInfo,
    stems: Vec<StemDecoder>,
    /// Decoder output for one stem (planar stereo), reused.
    scratch: [Vec<f32>; 2],
    position: u64,
}

fn decode_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        "song_damaged",
        "This song file is damaged. Copy it again from the original, or make it again in the Studio.",
        e.to_string(),
    )
}

/// Which slot a stem goes to (`None`: not played).
fn slot_for(role: &str, in_mix: bool, pitched: bool) -> Option<usize> {
    match role {
        "lead_vocal" => Some(SLOT_LEAD),
        "backing_vocals" => Some(SLOT_BACKING),
        "drums" => Some(SLOT_DRUMS),
        "other" => Some(SLOT_OTHER),
        _ if !in_mix => None,
        _ if pitched => Some(SLOT_EXTRA_PITCHED),
        _ => Some(SLOT_EXTRA_UNPITCHED),
    }
}

impl SongSource {
    /// Open a song file and every stem's decoder (positioned at frame 0).
    pub fn open(path: &Path) -> Result<SongSource, EngineError> {
        let song = std::sync::Arc::new(jk_format::SongFile::open(path).map_err(|e| EngineError::from_format(&e))?);
        let m = song.manifest().clone();
        if m.sample_rate == 0 || !(m.channels == 1 || m.channels == 2) {
            return Err(decode_err("manifest: invalid sample rate or channel count"));
        }
        let mut stems = Vec::new();
        let mut infos = Vec::new();
        let mut used = [false; SLOTS];
        for st in &m.stems {
            let in_mix = st.in_karaoke_mix();
            let pitched = st.pitched();
            infos.push(StemInfo { role: st.role.clone(), in_karaoke_mix: in_mix, pitched });
            let Some(slot) = slot_for(&st.role, in_mix, pitched) else { continue };
            if st.codec != "flac" {
                return Err(decode_err(format!("stem '{}' uses codec '{}', not flac", st.role, st.codec)));
            }
            if !(st.gain.is_finite() && st.gain > 0.0) {
                return Err(decode_err(format!("stem '{}' has an invalid gain", st.role)));
            }
            // A factory of fresh views of the stem chunk (each with its own handle on the
            // same open file), so every seek can start a new demuxer (see FlacStem).
            let (song2, chunk) = (song.clone(), st.chunk.clone());
            let factory: crate::decode::SourceFactory = Box::new(move || {
                song2
                    .chunk_reader(&chunk)
                    .map(|r| Box::new(r) as Box<dyn symphonia::core::io::MediaSource>)
                    .map_err(|e| std::io::Error::other(e.to_string()))
            });
            let name = PathBuf::from(format!("{}#{}", path.display(), st.chunk));
            let dec = FlacStem::from_factory(factory, &name).map_err(decode_err)?;
            if dec.sample_rate() != m.sample_rate {
                return Err(decode_err(format!(
                    "stem '{}' is {} Hz but the song is {} Hz",
                    st.role,
                    dec.sample_rate(),
                    m.sample_rate
                )));
            }
            used[slot] = true;
            stems.push(StemDecoder { dec, slot, gain: st.gain });
        }
        let info = SongInfo {
            path: path.to_path_buf(),
            sample_rate: m.sample_rate,
            channels: m.channels,
            frames: m.duration_frames,
            stems: infos,
            used,
        };
        Ok(SongSource { info, stems, scratch: [vec![0.0; 4096], vec![0.0; 4096]], position: 0 })
    }

    pub fn info(&self) -> &SongInfo {
        &self.info
    }

    /// Frame the next `read` starts at.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Position every decoder so the next `read` starts at `frame` (sample-accurate).
    pub fn seek(&mut self, frame: u64) -> Result<(), EngineError> {
        let frame = frame.min(self.info.frames);
        for s in &mut self.stems {
            s.dec.seek(frame).map_err(decode_err)?;
        }
        self.position = frame;
        Ok(())
    }

    /// Read the next `n` frames of every used slot into `out[slot][ch][..n]`. Frames past
    /// the end of the song (or of a short stem) are zeros. Returns the frames of real
    /// song audio delivered (`< n` only at the end of the song).
    pub fn read(&mut self, out: &mut SlotBuffers, n: usize) -> Result<usize, EngineError> {
        if self.scratch[0].len() < n {
            self.scratch = [vec![0.0; n], vec![0.0; n]];
        }
        for slot in 0..SLOTS {
            if self.info.used[slot] {
                for c in 0..2 {
                    out[slot][c][..n].iter_mut().for_each(|v| *v = 0.0);
                }
            }
        }
        let song_left = self.info.frames.saturating_sub(self.position).min(n as u64) as usize;
        for s in &mut self.stems {
            let mut got = 0;
            while got < song_left {
                let k = s.dec.read(&mut self.scratch, got, song_left - got).map_err(decode_err)?;
                if k == 0 {
                    break; // a stem shorter than the song: the rest stays silent
                }
                got += k;
            }
            for c in 0..2 {
                let dst = &mut out[s.slot][c][..got];
                let src = &self.scratch[c][..got];
                if s.gain == 1.0 {
                    for (d, x) in dst.iter_mut().zip(src) {
                        *d += *x;
                    }
                } else {
                    for (d, x) in dst.iter_mut().zip(src) {
                        *d += (*x as f64 * s.gain) as f32;
                    }
                }
            }
        }
        self.position += song_left as u64;
        Ok(song_left)
    }
}

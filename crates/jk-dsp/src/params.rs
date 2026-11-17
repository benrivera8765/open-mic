// SPDX-License-Identifier: GPL-3.0-or-later
//! Mix and key parameters, and the stem layout the graph expects.

/// Number of audio channels the graph processes. Stems are stereo; a mono stem is fed as
/// the same slice twice by the caller.
pub const CHANNELS: usize = 2;

/// Lowest and highest key shift offered (WORK_ORDER section 4.5: at least +-12).
pub const MIN_SEMITONES: i32 = -12;
pub const MAX_SEMITONES: i32 = 12;

/// The four stems of a song, in the order used everywhere in this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stem {
    Lead = 0,
    Backing = 1,
    Drums = 2,
    Other = 3,
}

impl Stem {
    pub const ALL: [Stem; 4] = [Stem::Lead, Stem::Backing, Stem::Drums, Stem::Other];
    pub fn name(self) -> &'static str {
        match self {
            Stem::Lead => "lead",
            Stem::Backing => "backing",
            Stem::Drums => "drums",
            Stem::Other => "other",
        }
    }
}

/// One block of planar stem audio: `stems[stem][channel][frame]`, every slice at least as
/// long as the block.
#[derive(Clone, Copy)]
pub struct StemBlock<'a> {
    pub stems: [[&'a [f32]; CHANNELS]; 4],
}

/// Stems of roles the graph has no slot for (roles added by newer song files), summed
/// by the caller at unity gain: `pitched` goes into the pitch bus, `unpitched` into the
/// drum bus (SONG_FORMAT 4.2 forward-compatibility rule). Slices at least block long.
#[derive(Clone, Copy)]
pub struct ExtraBlock<'a> {
    pub pitched: [&'a [f32]; CHANNELS],
    pub unpitched: [&'a [f32]; CHANNELS],
}

/// What the singer controls. All gains are linear amplitude factors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixParams {
    /// Guide vocal = the original lead singer, 0..1. Default 0 (WORK_ORDER section 11, #8).
    pub guide: f32,
    /// Backing vocals on/off and level.
    pub backing_on: bool,
    pub backing: f32,
    /// Everything else (instruments).
    pub other: f32,
    /// Drums level.
    pub drums: f32,
    /// Send the drums through the pitch shifter too. Default off (WORK_ORDER section 4.5).
    pub shift_drums: bool,
    /// Key shift in semitones, `MIN_SEMITONES..=MAX_SEMITONES`.
    pub semitones: i32,
}

impl Default for MixParams {
    fn default() -> Self {
        MixParams {
            guide: 0.0,
            backing_on: true,
            backing: 1.0,
            other: 1.0,
            drums: 1.0,
            shift_drums: false,
            semitones: 0,
        }
    }
}

impl MixParams {
    /// Target linear gain for each stem, in [`Stem`] order.
    pub fn stem_gains(&self) -> [f32; 4] {
        [
            self.guide,
            if self.backing_on { self.backing } else { 0.0 },
            self.drums,
            self.other,
        ]
    }

    pub fn clamped(mut self) -> Self {
        self.semitones = self.semitones.clamp(MIN_SEMITONES, MAX_SEMITONES);
        for g in [&mut self.guide, &mut self.backing, &mut self.other, &mut self.drums] {
            if !g.is_finite() || *g < 0.0 {
                *g = 0.0;
            }
            *g = g.min(4.0);
        }
        self
    }
}

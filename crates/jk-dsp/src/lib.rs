// SPDX-License-Identifier: GPL-3.0-or-later
//! # jk-dsp
//!
//! Jukeoke's deterministic audio graph (docs/ARCHITECTURE.md section 4): stem gains, the
//! pitch bus (lead x guide + backing + other) through Rubber Band R3, the drum bus delayed
//! by exactly the pitch path latency, the bit-transparent 0-semitone bypass with an
//! equal-power crossfade on key change, optional "shift drums", the output resampler (only
//! when the device rate differs from the song rate) and an offline renderer.
//!
//! No threads, no I/O, no OS calls, no allocation after construction in the processing
//! path. The real-time player (jk-engine) and the offline renderer run the same code.

// Index loops over channels and frames are the clearest form for DSP code.
#![allow(clippy::needless_range_loop)]

pub mod analysis;
pub mod signals;
pub mod graph;
pub mod params;
pub mod render;
pub mod resample;
pub mod ring;
pub mod shifter;
pub mod smooth;

pub use graph::{DspError, GraphConfig, GraphStats, PitchGraph};
pub use params::{MixParams, Stem, StemBlock, CHANNELS, MAX_SEMITONES, MIN_SEMITONES};
pub use render::{render, resample_whole, ParamEvent, RenderOptions, RenderOutput, StemsRef};
pub use resample::OutputResampler;

/// Version of the vendored Rubber Band Library.
pub const RUBBERBAND_VERSION: &str = jk_rubberband::RUBBERBAND_VERSION;

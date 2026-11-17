// SPDX-License-Identifier: GPL-3.0-or-later
//! # jk-engine (prototype)
//!
//! Jukeoke's real-time player: FLAC stem decoding with sample-accurate seeking
//! ([`decode`]), per-stem SPSC rings, a render-ahead worker running the jk-dsp graph
//! ([`player`]), device output ([`output`]: WASAPI on Windows, Core Audio via cpal on
//! macOS), the playback clock and the underrun counter ([`clock`]).

// Index loops over channels and frames are the clearest form for audio code.
#![allow(clippy::needless_range_loop)]

pub mod clock;
pub mod decode;
pub mod flac_write;
pub mod output;
pub mod player;

pub use clock::Clock;
pub use decode::{DecodeError, FlacStem};
pub use flac_write::encode_flac;
pub use output::{DeviceInfo, OutputRequest};
pub use player::{Player, PlayerConfig, PlayerError, StemFiles, WorkerStats};

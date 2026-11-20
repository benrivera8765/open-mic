// SPDX-License-Identifier: GPL-3.0-or-later
//! # jk-engine
//!
//! Jukeoke's real-time player. The public contract with the app is [`api`] (re-exported
//! here): [`Engine`], [`render_offline`] and their types. Behind it:
//!
//! * [`source`]: `.jksong` stems streamed from the file through jk-format's
//!   `ChunkReader` (symphonia FLAC), stored gains applied, roles mapped to the graph.
//! * [`engine`]: control, stem reader, render-ahead worker, house-keeping thread.
//! * [`output`]: device output (WASAPI on Windows, Core Audio via cpal on macOS, a
//!   null device for tests), the lock-free device-side "filler" and the playback clock.
//! * [`export`]: offline render for "Export backing track".
//! * [`keycache`]: key change by rendering ahead into a disk cache (prerender mode).
//! * [`keyheard`]: the key that is heard at the DAC, and its recent history.
//! * [`flac_write`]: an in-house FLAC encoder (fixtures, exports).
//! * [`demo`]: the generated demo song (self-check, ticker ground truth).
//! * [`tap`]: the output tap, a lock-free copy of what the device plays with DAC time
//!   stamps (practice mode's echo reference).

// Index loops over channels and frames are the clearest form for audio code.
#![allow(clippy::needless_range_loop)]

pub mod api;
pub mod decode;
pub mod demo;
pub mod devices;
pub mod engine;
pub mod export;
pub mod flac_write;
pub mod keycache;
pub mod keyheard;
pub mod output;
pub mod shared;
pub mod source;
pub mod tap;

pub use api::{
    render_offline, DeviceInfo, DeviceStatus, Engine, EngineConfig, EngineError, EngineEvent, ExportFormat,
    LoadedSong, MixParams, PlaybackState, RenderReport, Snapshot, StemInfo,
};
pub use decode::{DecodeError, FlacStem};
pub use engine::Diagnostics;
pub use flac_write::{encode_flac, FlacEncoder};
pub use keycache::{
    benchmark_realtime_factor, choose_key_mode, KeyMode, KeyPrepare, KeyStatus, AUTO_MIN_REALTIME_FACTOR,
};
pub use keyheard::{KeyPending, KeySegment};
pub use output::{OutputRequest, StreamInfo};
pub use shared::now_ns;
pub use tap::{Anchor as TapAnchor, OutputTap, TapRead, TapReader, TAP_CHANNELS, TAP_FRAMES};

// SPDX-License-Identifier: GPL-3.0-or-later
//! State shared lock-free between the control thread (API calls), the render worker and
//! the device callback. Plain atomics only; floats are stored as their bit patterns.

use std::sync::atomic::{fence, AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Monotonic nanoseconds since the first use of the engine (for timestamps).
pub fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// An `f64` in an `AtomicU64`.
#[derive(Default)]
pub struct AtomicF64(AtomicU64);
impl AtomicF64 {
    pub fn new(v: f64) -> Self {
        AtomicF64(AtomicU64::new(v.to_bits()))
    }
    pub fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Acquire))
    }
    pub fn store(&self, v: f64) {
        self.0.store(v.to_bits(), Release)
    }
}

/// An `f32` in an `AtomicU32`.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
}

/// `Shared::filler_seg` packs the filler's generation (high bits) and the frames it
/// consumed of that generation's segment (low `FILLER_SEG_BITS` bits).
pub const FILLER_SEG_BITS: u32 = 40;

/// `Shared::key_route` values.
pub const ROUTE_BYPASS: u32 = 0;
pub const ROUTE_SHIFT: u32 = 1;
pub const ROUTE_CACHED: u32 = 2;

/// Everything the threads share.
#[derive(Default)]
pub struct Shared {
    // ---- written by control, read by the device callback ----------------------------
    /// The user wants sound (play) or not (pause).
    pub want_play: AtomicBool,
    /// Generation of the segment the callback should play. Bumped (Release) on every
    /// load / seek / device switch, *after* `target_pos` is stored.
    pub target_gen: AtomicU64,
    /// Song time (s) where segment `target_gen` starts.
    pub target_pos: AtomicF64,
    /// Master volume, linear.
    pub master: AtomicF32,

    // ---- written by the device callback --------------------------------------------
    /// Song time (s) of the sample leaving the DAC at the last callback.
    pub dac_pos: AtomicF64,
    /// Generation the DAC position belongs to (0 = nothing yet).
    pub dac_gen: AtomicU64,
    /// Sequence counter over `dac_pos` + `dac_gen` (odd while the callback writes them),
    /// so readers get a matching pair ([`Shared::publish_dac`], [`Shared::dac`]).
    pub dac_seq: AtomicU64,
    /// `now_ns()` of the last callback.
    pub dac_ns: AtomicU64,
    /// Generation whose end marker the callback reached (song played to the end).
    pub ended_gen: AtomicU64,
    /// Output latency reported at the last callback (device frames).
    pub latency_frames: AtomicU32,
    /// Callbacks that found the output ring empty while playing.
    pub underruns: AtomicU64,
    pub underrun_frames: AtomicU64,
    /// WASAPI shared mode: endpoint buffer found empty (device thread late).
    pub device_starved: AtomicU64,
    pub callbacks: AtomicU64,
    pub max_callback_ns: AtomicU64,
    /// Device position from the OS clock (Windows IAudioClock) and the matching
    /// performance-counter time (100 ns units), first and latest, for drift in ppm.
    pub os_pos_first: AtomicU64,
    pub os_qpc_first: AtomicU64,
    pub os_pos_last: AtomicU64,
    pub os_qpc_last: AtomicU64,
    /// The filler's mode at the last callback (diagnostics): 0 flush, 1 primed,
    /// 2 running, 3 fade-out, 4 paused, 5 ended.
    pub filler_mode: AtomicU32,
    /// Generation the filler plays and the frames it consumed of that segment, packed
    /// (see [`FILLER_SEG_BITS`]); the worker derives its lead from it.
    pub filler_seg: AtomicU64,
    /// Set by the device thread when its stream failed (device lost).
    pub device_failed: AtomicBool,
    /// Test hook: ask the device thread to fail as if the device had been removed.
    pub simulate_device_loss: AtomicBool,

    // ---- written by the render worker ------------------------------------------------
    pub blocks: AtomicU64,
    pub busy_ns: AtomicU64,
    pub max_block_ns: AtomicU64,
    /// Lowest output-ring fill (device frames) seen while playing, since the last reset.
    pub min_ahead_frames: AtomicU64,
    /// The worker needed stem audio the reader had not decoded yet.
    pub reader_starved: AtomicU64,
    /// Output peak per channel over the last ~0.3 s (pre-master).
    pub peak: [AtomicF32; 2],
    /// Output went above full scale (master included) since the last reset.
    pub clipped: AtomicBool,
    /// Bumped by control on every load, mix change and master change: the worker then
    /// clears `clipped` and ignores the audio of the old mix still in the graph.
    pub clip_epoch: AtomicU64,
    /// Key changes that had to wait for a pre-built Rubber Band instance (blocks).
    pub deferred_blocks: AtomicU64,
    /// Rubber Band instances the render worker constructed itself (must stay 0).
    pub inline_constructions: AtomicU64,
    /// Rubber Band output not ready in time (frames; must stay 0).
    pub shortfall_frames: AtomicU64,
    /// Generation of the segment the worker is producing.
    pub worker_gen: AtomicU64,
    /// Why the worker last did not produce a block (diagnostics): 0 produced, 1 no
    /// output/song/segment, 2 waiting for a Rubber Band spare, 3 marker ring full,
    /// 4 song ended, 5 output ring full (ahead), 6 waiting for stems.
    pub worker_state: AtomicU32,
    /// Key heard at the worker's output.
    pub audible_semitones: AtomicI32,

    /// Key changes the worker could not report because its key log was full (the house
    /// thread drains it every 5 ms; must stay 0).
    pub key_log_lost: AtomicU64,

    /// Key cache (prerender mode): the worker's next input frame, the route its graph
    /// heads for (`ROUTE_*`), the cache entry tag of that route when cached, the tag of
    /// a feed waiting to be switched to, and cached blocks that were missing.
    pub worker_song_in: AtomicU64,
    pub key_route: AtomicU32,
    pub key_route_tag: AtomicU64,
    pub key_pending_tag: AtomicU64,
    pub cache_misses: AtomicU64,
    /// Test hook: the render worker's graph and the key renderer run this many times
    /// slower (x100; 100 or less = normal), simulating a slow computer.
    pub debug_slowdown_x100: AtomicU32,

    // ---- written by the reader -------------------------------------------------------
    /// A stem could not be decoded (message in the control state).
    pub reader_failed: AtomicBool,

    // ---- output tap (practice mode) ----------------------------------------------------
    /// Created by control on the first `Engine::output_tap` call, then written by the
    /// device callback while a reader is attached (see [`crate::tap`]). `OnceLock::get`
    /// is one atomic load, so the callback never waits on it.
    pub tap: OnceLock<Arc<crate::tap::OutputTap>>,
}

impl Shared {
    pub fn new(master: f32) -> Shared {
        let s = Shared::default();
        s.master.store(master);
        s.min_ahead_frames.store(u64::MAX, Relaxed);
        s
    }

    /// Device callback (the only writer): publish the song time and generation of the
    /// sample leaving the DAC as one pair. Plain atomic stores (a sequence lock), so the
    /// callback never waits.
    pub fn publish_dac(&self, pos: f64, generation: u64) {
        let s = self.dac_seq.load(Relaxed);
        self.dac_seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        self.dac_pos.store(pos);
        self.dac_gen.store(generation, Release);
        self.dac_seq.store(s.wrapping_add(2), Release);
    }

    /// The DAC position and the generation it belongs to, read as one pair (never a
    /// position of one segment with the generation of another).
    pub fn dac(&self) -> (f64, u64) {
        loop {
            let s1 = self.dac_seq.load(Acquire);
            if s1 & 1 == 0 {
                let pos = self.dac_pos.load();
                let generation = self.dac_gen.load(Relaxed);
                fence(Acquire);
                if self.dac_seq.load(Relaxed) == s1 {
                    return (pos, generation);
                }
            }
            std::hint::spin_loop();
        }
    }

    /// Reset the per-song counters (on load).
    pub fn reset_counters(&self) {
        for a in [
            &self.underruns,
            &self.underrun_frames,
            &self.device_starved,
            &self.blocks,
            &self.busy_ns,
            &self.max_block_ns,
            &self.reader_starved,
            &self.deferred_blocks,
            &self.shortfall_frames,
            &self.cache_misses,
        ] {
            a.store(0, Relaxed);
        }
        self.min_ahead_frames.store(u64::MAX, Relaxed);
        self.clipped.store(false, Relaxed);
        self.clip_epoch.fetch_add(1, Release);
    }
}

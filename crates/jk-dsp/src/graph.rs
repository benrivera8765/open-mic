// SPDX-License-Identifier: GPL-3.0-or-later
//! The song-rate signal graph (docs/ARCHITECTURE.md section 4):
//!
//! ```text
//!  lead x guide ----\
//!  backing x level --+--> PITCH BUS --+--> [bypass: delay L] ------------\
//!  other x level ----/                +--> [Rubber Band R3 @ key, aligned to L] --+--> out
//!  drums x level ------> DRUM BUS --> [delay L] (or into the pitch bus with "shift drums")
//! ```
//!
//! * **One latency L for every path.** Output frame `u` of every path corresponds to input
//!   frame `u` delayed by exactly `L = longest_fft * 9/8` frames (4608 at 44.1/48 kHz). The
//!   drums, the bypass and the shifted band therefore line up to the sample (verified by
//!   the impulse tests), and the playback clock subtracts `L`.
//! * **0 semitones is bit-transparent:** the output is the plain mix
//!   `(lead*g + backing*g + other*g) + drums*g`, delayed by `L`. Rubber Band is not running.
//! * **Key change = prime + crossfade.** Each Rubber Band instance runs at one fixed pitch
//!   (its latency depends on the pitch). A change requested when input frame `t` is next
//!   starts a fresh instance at the new pitch, primed with the real audio of the
//!   `prime_len` frames before `t`, and fed alongside the old path from then on. The output
//!   crossfades (equal power, 40 ms) from the old path to the new one starting at output
//!   frame `t`, i.e. `L` later, when the new path's output is ready. Priming therefore costs
//!   only `prime_len` frames of Rubber Band work in one block (an earlier version primed
//!   `L + prime_len` frames to crossfade at once and produced 69 ms blocks under CPU load).
//!   A change requested while a transition runs waits for it to finish (only the latest
//!   request is kept).
//! * **Prerendered key (key-change render-ahead cache, WORK_ORDER 4.5).** A third kind of
//!   route, `Cached(slot)`, plays pitch-path output that a background renderer made ahead
//!   of time with this same graph (`GraphConfig::pitched_only`). The caller passes the
//!   cached frames with each block (aligned with the input frames, like the stems), so
//!   the cached route at output frame `u` is `cache[u] + drums[u]`: exactly the
//!   operations of the real-time `Shift` route, hence bit-identical output for the same
//!   start point. With [`PitchGraph::set_auto_route`]`(false)` ("prerender policy") the
//!   graph never starts a real-time shifter by itself: routes change only on request
//!   ([`PitchGraph::request_cached`], [`PitchGraph::request_bypass`]), with the same
//!   40 ms equal-power crossfade, and a missing cache block
//!   ([`PitchGraph::cache_miss`]) fades to the bypass so that the fade ends where the
//!   data ends (possible because `latency() > crossfade_len()`).
//! * Deterministic: no threads, no I/O, no clocks, no allocation after construction.
//!   Gains ramp per sample and crossfades are counted in samples, so the result does not
//!   depend on block size (checked by the tests).

use crate::params::{ExtraBlock, MixParams, Stem, StemBlock, CHANNELS};
use crate::ring::AbsRing;
use crate::shifter::{r3_longest_fft, ShiftPath};
use crate::smooth::{EqualPowerTable, GainRamp};
use jk_rubberband::{Stretcher, StretcherConfig};

/// How many never-used Rubber Band instances the graph can hold for upcoming key
/// changes, and how many replaced ones it can hold until the caller collects them.
pub const MAX_SPARE_STRETCHERS: usize = 4;

/// Slots for pitched material rendered ahead (the key cache): the one heard and the one
/// being switched to.
pub const CACHE_SLOTS: usize = 2;

/// Construction parameters for [`PitchGraph`].
#[derive(Clone, Copy, Debug)]
pub struct GraphConfig {
    /// The song's sample rate (the graph runs at the song rate; resampling to the device
    /// rate happens after it, and only if the rates differ).
    pub sample_rate: u32,
    /// Largest block passed to [`PitchGraph::process`].
    pub max_block: usize,
    /// Key-change crossfade length.
    pub crossfade_ms: f64,
    /// Gain-change ramp length.
    pub gain_ramp_ms: f64,
    /// May the graph construct a Rubber Band instance itself when a key change needs one
    /// and none was offered? `true` for the offline renderer. The real-time engine sets
    /// `false`: a key change then waits (a few ms) until a helper thread has offered a
    /// pre-built instance through [`PitchGraph::offer_stretcher`], so the render worker
    /// never constructs (allocates) one. Output is identical either way.
    pub inline_stretchers: bool,
    /// Output only the pitch path (no drum bus): what the key cache stores. The cached
    /// route of a playback graph adds the live drum bus back, so the sum is the same
    /// arithmetic as the real-time route. Default `false`.
    pub pitched_only: bool,
}

impl GraphConfig {
    pub fn new(sample_rate: u32) -> Self {
        GraphConfig {
            sample_rate,
            max_block: 1024,
            crossfade_ms: 40.0,
            gain_ramp_ms: 20.0,
            inline_stretchers: true,
            pitched_only: false,
        }
    }
}

/// Counters for diagnostics and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphStats {
    /// Frames for which Rubber Band's output was not ready and silence was used instead.
    /// Must stay 0; non-zero means `L` is too small for this configuration.
    pub shift_shortfall_frames: u64,
    /// Number of key/route transitions started.
    pub transitions: u64,
    /// Rubber Band instances the graph constructed itself (0 in the real-time engine,
    /// except after a `reset` with no spare offered, which the engine avoids).
    pub inline_constructions: u64,
    /// Replaced instances the graph had to free itself because the retired slots were
    /// full (0 when the caller collects them with `take_retired`).
    pub inline_drops: u64,
    /// Blocks in which a key change waited for a pre-built instance.
    pub deferred_blocks: u64,
    /// Cached-route blocks whose data was missing (the graph faded to the bypass).
    pub cache_misses: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Bypass,
    Shift(usize),
    Cached(usize),
}

/// What a route is, for callers (diagnostics, the engine's cache feeds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteInfo {
    /// Key 0: the plain mix, delayed by the graph latency.
    Bypass,
    /// A real-time Rubber Band path at this key.
    Shift { semitones: i32 },
    /// Prerendered pitch-path material from cache slot `slot`.
    Cached { slot: usize, semitones: i32 },
}

/// A change of the key heard that the graph has scheduled in its output (Work Order 2,
/// 4.6: practice mode follows the key that is *heard*, not the one asked for). See
/// [`PitchGraph::scheduled_switch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySwitch {
    /// Stream frame (counted from the last reset, like
    /// [`PitchGraph::frames_processed`]) from which the new key is heard; the output
    /// carries stream frame `at` [`PitchGraph::latency`] frames after it was fed in. It is
    /// the crossfade's midpoint, where the equal-power fade gives both keys the same
    /// weight, and where [`PitchGraph::audible_semitones`] switches too. It can lie before
    /// the next input frame (the fade began before it) but never before the next output
    /// frame, so a switch is always known before any of its audio leaves the graph.
    pub at: u64,
    /// The key heard from `at` on.
    pub semitones: i32,
}

/// One slot of prerendered pitch-path material, on the graph's absolute timeline.
struct CacheSlot {
    hist: AbsRing,
    /// The cached material already contains the drums ("shift drums" was on).
    drums_in: bool,
    semitones: i32,
}

#[derive(Clone, Copy, Debug)]
struct Crossfade {
    from: Route,
    to: Route,
    /// Absolute output frame where the crossfade starts (fade position 0).
    start: u64,
}

/// Errors from the DSP crate.
#[derive(Debug)]
pub enum DspError {
    RubberBand(jk_rubberband::Error),
    Resampler(String),
    Invalid(&'static str),
}

impl std::fmt::Display for DspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DspError::RubberBand(e) => write!(f, "{e}"),
            DspError::Resampler(e) => write!(f, "resampler: {e}"),
            DspError::Invalid(e) => write!(f, "invalid input: {e}"),
        }
    }
}
impl std::error::Error for DspError {}

impl From<jk_rubberband::Error> for DspError {
    fn from(e: jk_rubberband::Error) -> Self {
        DspError::RubberBand(e)
    }
}

/// See the module documentation.
pub struct PitchGraph {
    cfg: GraphConfig,
    latency: usize,
    prime_len: usize,
    /// Absolute index of the next input frame.
    in_pos: u64,
    /// Absolute index of stream frame 0 (history below it reads as silence).
    base: u64,
    hist_bus: AbsRing,
    hist_drums: AbsRing,
    paths: [ShiftPath; 2],
    current: Route,
    xfade: Option<Crossfade>,
    table: EqualPowerTable,
    gains: [GainRamp; 4],
    params: MixParams,
    /// Never-used Rubber Band instances for upcoming key changes.
    spares: [Option<Stretcher>; MAX_SPARE_STRETCHERS],
    /// Replaced instances waiting to be freed by the caller (off the real-time thread).
    retired: [Option<Stretcher>; MAX_SPARE_STRETCHERS],
    /// Prerendered pitch-path material (key cache).
    cached: [CacheSlot; CACHE_SLOTS],
    /// `true` (default): routes follow the parameters (real-time key changes). `false`:
    /// prerender policy, routes change only on request.
    auto_route: bool,
    /// Prerender policy: the requested route, started at the next block (after any
    /// running crossfade).
    route_req: Option<Route>,
    // Scratch, allocated once.
    bus: [Vec<f32>; CHANNELS],
    drums: [Vec<f32>; CHANNELS],
    a: [Vec<f32>; CHANNELS],
    b: [Vec<f32>; CHANNELS],
    stats: GraphStats,
}

impl PitchGraph {
    pub fn new(cfg: GraphConfig, params: MixParams) -> Result<Self, DspError> {
        if cfg.max_block == 0 || cfg.max_block > 8192 {
            return Err(DspError::Invalid("max_block must be 1..=8192"));
        }
        let fft = r3_longest_fft(cfg.sample_rate);
        // Measured worst-case output lag of R3 (HighQuality) is about fft - 55 samples, plus
        // the transient trim (at most about +0.01 * fft) and up to RB_QUANTUM - 1 frames of
        // input buffering in the shift path; fft/8 covers it. See the `probe` example and
        // the `no_shortfall` test.
        let latency = fft + fft / 8;
        // Real audio fed to a fresh instance before the frame where its output is first
        // used: twice Rubber Band's preferred start pad (fft/2 at pitch scale 1).
        let prime_len = fft;
        let cap = (latency + prime_len + 2 * cfg.max_block + fft).next_power_of_two();
        let base = cap as u64;
        let mk = |n| -> [Vec<f32>; CHANNELS] { [vec![0.0; n], vec![0.0; n]] };
        let ramp = ((cfg.gain_ramp_ms * cfg.sample_rate as f64 / 1000.0).round() as u32).max(1);
        let xfade_len = ((cfg.crossfade_ms * cfg.sample_rate as f64 / 1000.0).round() as usize).max(1);
        let params = params.clamped();
        let g = params.stem_gains();
        let mut graph = PitchGraph {
            cfg,
            latency,
            prime_len,
            in_pos: base,
            base,
            hist_bus: AbsRing::new(CHANNELS, cap, base),
            hist_drums: AbsRing::new(CHANNELS, cap, base),
            paths: [
                ShiftPath::new(cfg.sample_rate, cap)?,
                ShiftPath::new(cfg.sample_rate, cap)?,
            ],
            current: Route::Bypass,
            xfade: None,
            table: EqualPowerTable::new(xfade_len),
            gains: [
                GainRamp::new(g[0], ramp),
                GainRamp::new(g[1], ramp),
                GainRamp::new(g[2], ramp),
                GainRamp::new(g[3], ramp),
            ],
            params,
            spares: Default::default(),
            retired: Default::default(),
            cached: [
                CacheSlot { hist: AbsRing::new(CHANNELS, cap, base), drums_in: false, semitones: 0 },
                CacheSlot { hist: AbsRing::new(CHANNELS, cap, base), drums_in: false, semitones: 0 },
            ],
            auto_route: true,
            route_req: None,
            bus: mk(cfg.max_block),
            drums: mk(cfg.max_block),
            a: mk(cfg.max_block),
            b: mk(cfg.max_block),
            stats: GraphStats::default(),
        };
        graph.reset();
        Ok(graph)
    }

    pub fn config(&self) -> &GraphConfig {
        &self.cfg
    }

    /// Total latency of every path, in song frames. Output frame `n` of [`process`]
    /// (counting from the first call after `new`/`reset`) is input frame `n - latency`.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Frames of real audio fed to a new Rubber Band instance before the frame where its
    /// output is first used.
    pub fn prime_len(&self) -> usize {
        self.prime_len
    }

    pub fn crossfade_len(&self) -> usize {
        self.table.len()
    }

    pub fn stats(&self) -> GraphStats {
        self.stats
    }

    pub fn params(&self) -> MixParams {
        self.params
    }

    /// Frames processed since the last reset.
    pub fn frames_processed(&self) -> u64 {
        self.in_pos - self.base
    }

    /// The key currently heard at the output (the target of a running crossfade counts
    /// once the crossfade is more than half way).
    pub fn audible_semitones(&self) -> i32 {
        let u = self.in_pos - self.latency as u64;
        let route = match self.xfade {
            Some(x) if u >= x.start + self.table.len() as u64 / 2 => x.to,
            Some(x) => x.from,
            None => self.current,
        };
        self.route_semitones(route)
    }

    /// The key change scheduled by the running crossfade (`None` when none runs). A
    /// crossfade is created `latency()` frames before its first output frame, so the
    /// caller learns of every switch well before it is heard; the switch stays reported
    /// until the crossfade has finished. The target of a crossfade can change once, when
    /// a cached key's data goes missing ([`cache_miss`](Self::cache_miss)): the same `at`
    /// is then reported with the bypass key (0).
    pub fn scheduled_switch(&self) -> Option<KeySwitch> {
        let x = self.xfade?;
        let mid = x.start + self.table.len() as u64 / 2;
        Some(KeySwitch { at: mid.saturating_sub(self.base), semitones: self.route_semitones(x.to) })
    }

    fn route_semitones(&self, r: Route) -> i32 {
        match r {
            Route::Bypass => 0,
            Route::Shift(s) => self.paths[s].semitones,
            Route::Cached(k) => self.cached[k].semitones,
        }
    }

    fn route_info(&self, r: Route) -> RouteInfo {
        match r {
            Route::Bypass => RouteInfo::Bypass,
            Route::Shift(s) => RouteInfo::Shift { semitones: self.paths[s].semitones },
            Route::Cached(k) => RouteInfo::Cached { slot: k, semitones: self.cached[k].semitones },
        }
    }

    /// The route heard now and, during a crossfade, the route being faded to.
    pub fn route(&self) -> (RouteInfo, Option<RouteInfo>) {
        match self.xfade {
            Some(x) => (self.route_info(x.from), Some(self.route_info(x.to))),
            None => (self.route_info(self.current), None),
        }
    }

    /// Is cache slot `k` heard, being faded in or out, or requested?
    pub fn cache_slot_in_use(&self, k: usize) -> bool {
        let r = Route::Cached(k);
        self.current == r || self.route_req == Some(r) || self.xfade.is_some_and(|x| x.from == r || x.to == r)
    }

    /// A cache slot that is neither heard nor requested (to load the next key into).
    pub fn free_cache_slot(&self) -> Option<usize> {
        (0..CACHE_SLOTS).find(|&k| !self.cache_slot_in_use(k))
    }

    /// `true`: routes follow the parameters (real-time key changes, the default).
    /// `false`: prerender policy: the graph never starts a real-time shifter by itself;
    /// the route heard now (even a running `Shift`) stays until
    /// [`request_cached`](Self::request_cached) or [`request_bypass`](Self::request_bypass).
    /// Turning it back on makes the graph move to the parameters' key the usual way
    /// (prime + crossfade from whatever is heard, a cached route included).
    pub fn set_auto_route(&mut self, on: bool) {
        self.auto_route = on;
        self.route_req = None;
    }

    pub fn auto_route(&self) -> bool {
        self.auto_route
    }

    /// Prerender policy: crossfade to cache slot `slot` at the next block. The caller
    /// passes that slot's frames from that block on (see [`process_cached`](Self::process_cached)).
    /// `drums_in`: the cached material already holds the drums.
    pub fn request_cached(&mut self, slot: usize, semitones: i32, drums_in: bool) {
        if slot >= CACHE_SLOTS || self.auto_route {
            return;
        }
        if !self.cache_slot_in_use(slot) {
            self.cached[slot].drums_in = drums_in;
            self.cached[slot].semitones = semitones;
        }
        self.route_req = Some(Route::Cached(slot));
    }

    /// Prerender policy: crossfade to the bypass (key 0) at the next block.
    pub fn request_bypass(&mut self) {
        if !self.auto_route {
            self.route_req = Some(Route::Bypass);
        }
    }

    /// Reset (see [`reset`](Self::reset)) and, in the prerender policy, start on cache slot
    /// `slot` (no crossfade: used at a seek, which the engine fades in itself).
    pub fn reset_cached(&mut self, slot: usize, semitones: i32, drums_in: bool) {
        self.reset();
        if slot < CACHE_SLOTS && !self.auto_route {
            self.cached[slot].drums_in = drums_in;
            self.cached[slot].semitones = semitones;
            self.current = Route::Cached(slot);
        }
    }

    /// Cache slot `slot` has no data from the next input frame `t` on. If it is heard,
    /// the graph crossfades to the bypass so that the fade ends at output frame `t`
    /// (the frames before `t` are already in the slot's history). Rare by design (the
    /// engine switches to a cached key only well ahead of the renderer); never a click
    /// unless it happens during the 40 ms of a crossfade into that slot.
    pub fn cache_miss(&mut self, slot: usize) {
        let r = Route::Cached(slot);
        if self.route_req == Some(r) {
            self.route_req = None;
        }
        let t = self.in_pos;
        let u_next = t - self.latency as u64;
        let len = self.table.len() as u64;
        match self.xfade.as_mut() {
            None if self.current == r => {
                self.stats.cache_misses += 1;
                self.stats.transitions += 1;
                self.xfade = Some(Crossfade { from: r, to: Route::Bypass, start: t.saturating_sub(len).max(u_next) });
            }
            Some(x) if x.to == r => {
                self.stats.cache_misses += 1;
                x.to = Route::Bypass;
            }
            _ => {}
        }
    }

    pub fn is_crossfading(&self) -> bool {
        self.xfade.is_some()
    }

    /// Forget all audio (seek / song change). Gains jump to their targets; if the key is
    /// not 0 the shifter is restarted with silence as its history (Rubber Band's own
    /// zero "start pad"). For a click-free seek, feed some real audio from before the
    /// seek point and discard the first `latency + preroll` output frames.
    pub fn reset(&mut self) {
        self.in_pos = self.base;
        self.hist_bus.clear(self.base);
        self.hist_drums.clear(self.base);
        let g = self.params.stem_gains();
        for (ramp, v) in self.gains.iter_mut().zip(g) {
            ramp.snap(v);
        }
        for s in &mut self.cached {
            s.hist.clear(self.base);
        }
        self.xfade = None;
        self.route_req = None;
        self.current = Route::Bypass;
        if self.auto_route && self.params.semitones != 0 {
            let from = self.in_pos - (self.latency + self.prime_len) as u64;
            self.restart_path(0, self.params.semitones, self.params.shift_drums, from);
            self.prime_path(0, from, self.in_pos);
            self.current = Route::Shift(0);
        }
    }

    /// The Rubber Band configuration of this graph's shifters (for building spares).
    pub fn stretcher_config(&self) -> StretcherConfig {
        ShiftPath::stretcher_config(self.cfg.sample_rate)
    }

    /// Hand the graph a never-used Rubber Band instance built with
    /// [`stretcher_config`](Self::stretcher_config) (normally on a helper thread). Gives
    /// it back when the graph already holds [`MAX_SPARE_STRETCHERS`].
    pub fn offer_stretcher(&mut self, s: Stretcher) -> Result<(), Stretcher> {
        match self.spares.iter_mut().find(|x| x.is_none()) {
            Some(slot) => {
                *slot = Some(s);
                Ok(())
            }
            None => Err(s),
        }
    }

    /// Spare instances held.
    pub fn spare_count(&self) -> usize {
        self.spares.iter().filter(|x| x.is_some()).count()
    }

    /// A replaced instance to free off the real-time thread, if any.
    pub fn take_retired(&mut self) -> Option<Stretcher> {
        self.retired.iter_mut().find_map(|x| x.take())
    }

    /// True when [`reset`](Self::reset) would have to construct a Rubber Band instance
    /// (key not 0, no spare, path used). The real-time engine waits for a spare first.
    pub fn reset_needs_stretcher(&self) -> bool {
        self.auto_route && self.params.semitones != 0 && self.paths[0].needs_fresh() && self.spare_count() == 0
    }

    /// True when a key change is waiting for a spare instance (`inline_stretchers` off).
    pub fn waiting_for_stretcher(&self) -> bool {
        self.auto_route && self.xfade.is_none() && !self.route_matches(self.current) && !self.can_start_transition()
    }

    fn take_spare(&mut self) -> Option<Stretcher> {
        self.spares.iter_mut().find_map(|x| x.take())
    }

    fn put_retired(&mut self, s: Stretcher) {
        match self.retired.iter_mut().find(|x| x.is_none()) {
            Some(slot) => *slot = Some(s),
            None => {
                self.stats.inline_drops += 1;
                drop(s);
            }
        }
    }

    /// Restart path `slot` (see `ShiftPath::restart`), using a spare when it needs a
    /// fresh instance.
    fn restart_path(&mut self, slot: usize, semitones: i32, shift_drums: bool, from: u64) {
        let fresh = if self.paths[slot].needs_fresh() { self.take_spare() } else { None };
        let (retired, unused, constructed) = self.paths[slot].restart(semitones, shift_drums, from, fresh);
        if constructed {
            self.stats.inline_constructions += 1;
        }
        if let Some(r) = retired {
            self.put_retired(r);
        }
        if let Some(u) = unused
            && let Err(u) = self.offer_stretcher(u)
        {
            self.put_retired(u);
        }
    }

    /// Can the transition to the current target route start now?
    fn can_start_transition(&self) -> bool {
        if self.params.semitones == 0 || self.cfg.inline_stretchers {
            return true;
        }
        let slot = match self.current {
            Route::Shift(s) => 1 - s,
            Route::Bypass | Route::Cached(_) => 0,
        };
        !self.paths[slot].needs_fresh() || self.spare_count() > 0
    }

    /// Set every parameter. Gains start ramping at the next input frame `t`; a key or
    /// routing change crossfades starting at output frame `t`, so both are heard
    /// `latency()` frames later.
    pub fn set_params(&mut self, params: &MixParams) {
        self.set_gains(params);
        self.set_route(params.semitones, params.shift_drums);
    }

    /// Stem gains only (guide, backing, other, drums).
    pub fn set_gains(&mut self, params: &MixParams) {
        let p = params.clamped();
        self.params.guide = p.guide;
        self.params.backing_on = p.backing_on;
        self.params.backing = p.backing;
        self.params.other = p.other;
        self.params.drums = p.drums;
        for (ramp, v) in self.gains.iter_mut().zip(self.params.stem_gains()) {
            ramp.set_target(v);
        }
    }

    /// Key and drum routing only.
    pub fn set_route(&mut self, semitones: i32, shift_drums: bool) {
        let p = MixParams { semitones, shift_drums, ..self.params }.clamped();
        self.params.semitones = p.semitones;
        self.params.shift_drums = p.shift_drums;
    }

    fn route_matches(&self, route: Route) -> bool {
        match route {
            Route::Bypass => self.params.semitones == 0,
            Route::Shift(s) => {
                self.params.semitones != 0
                    && self.paths[s].semitones == self.params.semitones
                    && self.paths[s].drums_in == self.params.shift_drums
            }
            // The automatic (real-time) policy never targets a cached route.
            Route::Cached(_) => false,
        }
    }

    /// Feed history `[from, to)` into path `slot` (used right after `restart`).
    fn prime_path(&mut self, slot: usize, from: u64, to: u64) {
        let mut pos = from;
        let chunk = self.cfg.max_block;
        while pos < to {
            let n = ((to - pos) as usize).min(chunk);
            for c in 0..CHANNELS {
                self.hist_bus.read_channel(c, pos, &mut self.a[c][..n]);
                self.hist_drums.read_channel(c, pos, &mut self.b[c][..n]);
            }
            let bus = [&self.a[0][..n], &self.a[1][..n]];
            let drums = [&self.b[0][..n], &self.b[1][..n]];
            self.paths[slot].feed(&bus, &drums, n);
            pos += n as u64;
        }
    }

    /// Start the transition to the current target route. `t` is the first input frame of
    /// this block; the crossfade starts at output frame `t`.
    fn start_transition(&mut self, t: u64) {
        let to = if self.params.semitones == 0 {
            Route::Bypass
        } else {
            let slot = match self.current {
                Route::Shift(s) => 1 - s,
                Route::Bypass | Route::Cached(_) => 0,
            };
            let from = t - self.prime_len as u64;
            self.restart_path(slot, self.params.semitones, self.params.shift_drums, from);
            self.prime_path(slot, from, t);
            Route::Shift(slot)
        };
        self.stats.transitions += 1;
        self.xfade = Some(Crossfade { from: self.current, to, start: t });
    }

    /// Write route `r`'s output for `[u, u + n)` into `out`.
    #[allow(clippy::too_many_arguments)]
    fn render_route(
        route: Route,
        paths: &[ShiftPath; 2],
        cached: &[CacheSlot; CACHE_SLOTS],
        pitched_only: bool,
        hist_bus: &AbsRing,
        hist_drums: &AbsRing,
        stats: &mut GraphStats,
        u: u64,
        n: usize,
        out: &mut [Vec<f32>; CHANNELS],
    ) {
        match route {
            Route::Bypass => {
                for c in 0..CHANNELS {
                    hist_bus.read_channel(c, u, &mut out[c][..n]);
                    hist_drums.add_channel_into(c, u, &mut out[c][..n]);
                }
            }
            Route::Shift(s) => {
                let p = &paths[s];
                if p.has(u, n) {
                    for c in 0..CHANNELS {
                        p.read_channel(c, u, &mut out[c][..n]);
                    }
                } else {
                    stats.shift_shortfall_frames += n as u64;
                    for c in 0..CHANNELS {
                        out[c][..n].iter_mut().for_each(|v| *v = 0.0);
                    }
                }
                if !p.drums_in && !pitched_only {
                    for c in 0..CHANNELS {
                        hist_drums.add_channel_into(c, u, &mut out[c][..n]);
                    }
                }
            }
            Route::Cached(k) => {
                // Same arithmetic as the Shift route: pitch-path output, then + drums.
                let cs = &cached[k];
                for c in 0..CHANNELS {
                    cs.hist.read_channel(c, u, &mut out[c][..n]);
                }
                if !cs.drums_in && !pitched_only {
                    for c in 0..CHANNELS {
                        hist_drums.add_channel_into(c, u, &mut out[c][..n]);
                    }
                }
            }
        }
    }

    /// Process one block of `frames` frames (`frames <= max_block`). `out[ch]` must hold
    /// at least `frames` samples. Output is the graph input delayed by [`latency`](Self::latency).
    pub fn process(&mut self, input: &StemBlock, frames: usize, out: [&mut [f32]; CHANNELS]) {
        self.process_ext(input, None, frames, out)
    }

    /// [`process`](Self::process) with stems of roles the graph has no slot for (from a
    /// newer song file), summed per bus at unity gain by the caller (SONG_FORMAT 4.2
    /// forward-compatibility rule). `None` gives exactly the same bits as `process`.
    pub fn process_ext(
        &mut self,
        input: &StemBlock,
        extra: Option<&ExtraBlock>,
        frames: usize,
        out: [&mut [f32]; CHANNELS],
    ) {
        self.process_cached(input, extra, [None, None], frames, out)
    }

    /// [`process_ext`](Self::process_ext) plus prerendered pitch-path frames for each
    /// cache slot, aligned with this block's input frames (`None` = no data: silence,
    /// which only matters if that slot is heard; report it with
    /// [`cache_miss`](Self::cache_miss) before this call).
    pub fn process_cached(
        &mut self,
        input: &StemBlock,
        extra: Option<&ExtraBlock>,
        cached: [Option<[&[f32]; CHANNELS]>; CACHE_SLOTS],
        frames: usize,
        out: [&mut [f32]; CHANNELS],
    ) {
        assert!(frames <= self.cfg.max_block, "block larger than max_block");
        if frames == 0 {
            return;
        }
        let t = self.in_pos;
        let u = t - self.latency as u64;

        // 1. Mix the buses with per-sample gain ramps. Summation order is fixed:
        //    bus = (lead*g + backing*g) + other*g; drums separately.
        let lead = &input.stems[Stem::Lead as usize];
        let back = &input.stems[Stem::Backing as usize];
        let drum = &input.stems[Stem::Drums as usize];
        let other = &input.stems[Stem::Other as usize];
        for i in 0..frames {
            let gl = self.gains[Stem::Lead as usize].tick();
            let gb = self.gains[Stem::Backing as usize].tick();
            let gd = self.gains[Stem::Drums as usize].tick();
            let go = self.gains[Stem::Other as usize].tick();
            for c in 0..CHANNELS {
                self.bus[c][i] = lead[c][i] * gl + back[c][i] * gb + other[c][i] * go;
                self.drums[c][i] = drum[c][i] * gd;
            }
        }
        if let Some(x) = extra {
            // Added last, so the v1.0 summation order (and its bits) is unchanged.
            for c in 0..CHANNELS {
                for i in 0..frames {
                    self.bus[c][i] += x.pitched[c][i];
                    self.drums[c][i] += x.unpitched[c][i];
                }
            }
        }
        {
            let [b0, b1] = &self.bus;
            self.hist_bus.push(&[&b0[..frames], &b1[..frames]], frames);
            let [d0, d1] = &self.drums;
            self.hist_drums.push(&[&d0[..frames], &d1[..frames]], frames);
        }
        for (slot, data) in self.cached.iter_mut().zip(cached) {
            match data {
                Some([l, r]) => slot.hist.push(&[&l[..frames], &r[..frames]], frames),
                None => slot.hist.push_silence(frames),
            }
        }

        // 2. Key / routing change: start a transition if none is running (and, in the
        //    real-time engine, once a pre-built Rubber Band instance is at hand).
        if self.xfade.is_none() {
            if self.auto_route {
                if !self.route_matches(self.current) {
                    if self.can_start_transition() {
                        self.start_transition(t);
                    } else {
                        self.stats.deferred_blocks += 1;
                    }
                }
            } else if let Some(r) = self.route_req.take()
                && r != self.current
            {
                // Prerender policy: an explicit switch (to a cached key or the bypass);
                // the same crossfade as a real-time key change, starting at output t.
                self.stats.transitions += 1;
                self.xfade = Some(Crossfade { from: self.current, to: r, start: t });
            }
        }

        // 3. Feed this block to every running shifter.
        let mut running = [false; 2];
        if let Route::Shift(s) = self.current {
            running[s] = true;
        }
        if let Some(Crossfade { to: Route::Shift(s), .. }) = self.xfade {
            running[s] = true;
        }
        for s in 0..2 {
            if running[s] {
                let bus = [&self.bus[0][..frames], &self.bus[1][..frames]];
                let drums = [&self.drums[0][..frames], &self.drums[1][..frames]];
                self.paths[s].feed(&bus, &drums, frames);
            }
        }

        // 4. Output for [u, u + frames).
        let [out_l, out_r] = out;
        match self.xfade {
            None => {
                Self::render_route(
                    self.current, &self.paths, &self.cached, self.cfg.pitched_only, &self.hist_bus, &self.hist_drums,
                    &mut self.stats, u, frames, &mut self.a,
                );
                out_l[..frames].copy_from_slice(&self.a[0][..frames]);
                out_r[..frames].copy_from_slice(&self.a[1][..frames]);
            }
            Some(x) => {
                Self::render_route(
                    x.from, &self.paths, &self.cached, self.cfg.pitched_only, &self.hist_bus, &self.hist_drums,
                    &mut self.stats, u, frames, &mut self.a,
                );
                let len = self.table.len() as u64;
                let end = u + frames as u64;
                if end <= x.start {
                    // Transition pending: the new path is primed and running; its output
                    // is not used yet.
                    out_l[..frames].copy_from_slice(&self.a[0][..frames]);
                    out_r[..frames].copy_from_slice(&self.a[1][..frames]);
                } else {
                    Self::render_route(
                        x.to, &self.paths, &self.cached, self.cfg.pitched_only, &self.hist_bus, &self.hist_drums,
                        &mut self.stats, u, frames, &mut self.b,
                    );
                    for i in 0..frames {
                        let abs = u + i as u64;
                        if abs < x.start {
                            out_l[i] = self.a[0][i];
                            out_r[i] = self.a[1][i];
                        } else if abs >= x.start + len {
                            out_l[i] = self.b[0][i];
                            out_r[i] = self.b[1][i];
                        } else {
                            let k = (abs - x.start) as usize;
                            let go = self.table.fade_out[k];
                            let gi = self.table.fade_in[k];
                            out_l[i] = self.a[0][i] * go + self.b[0][i] * gi;
                            out_r[i] = self.a[1][i] * go + self.b[1][i] * gi;
                        }
                    }
                    if end >= x.start + len {
                        self.current = x.to;
                        self.xfade = None;
                    }
                }
            }
        }
        self.in_pos += frames as u64;
    }

    /// Diagnostics: Rubber Band's start pad / start delay for the path currently heard.
    pub fn shifter_delays(&self) -> Option<(usize, usize)> {
        match self.current {
            Route::Bypass | Route::Cached(_) => None,
            Route::Shift(s) => Some((self.paths[s].preferred_start_pad(), self.paths[s].start_delay())),
        }
    }

    /// Diagnostics: how far ahead of the read position Rubber Band's output is (frames).
    /// Negative would mean a shortfall.
    pub fn shifter_headroom(&self) -> Option<i64> {
        let u = self.in_pos as i64 - self.latency as i64;
        match self.current {
            Route::Bypass | Route::Cached(_) => None,
            Route::Shift(s) => Some(self.paths[s].available_end() as i64 - u),
        }
    }
}

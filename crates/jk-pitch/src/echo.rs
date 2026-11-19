// SPDX-License-Identifier: GPL-3.0-or-later
//! The app's own playback in the microphone input: detection and removal (WORK_ORDER-2 4.4
//! "Music in the input", 4.10 "two ways to give the app a clean voice" and "The delay comes
//! for free", traps 4 and 6).
//!
//! A podcast mixer such as the owner's may send its whole mix to the computer, and with it
//! the song the app is playing: over USB (loopback, digital: fixed delay and level, maybe the
//! mixer's EQ), over the 3.5 mm cable into AUX-IN (analog: fixed delay, level and colouring,
//! plus a small clock drift between the two devices, up to about 100 ppm) or over Bluetooth
//! (a delay of 100-300 ms that wanders). The app knows exactly what it played, so it can
//! find that music in the input, measure it, and take it out again.
//!
//! [`EchoCanceller`] takes the reference (what the app played, as the output mix, one or two
//! channels) and the input channel that carries the voice, block by block, and gives back
//! the input with the returned music removed, plus an [`EchoReport`] and, for any stretch of
//! input, a [`Trust`] verdict for the pitch tracker. Pure DSP: no threads, no I/O, and no
//! allocation after [`EchoCanceller::new`].
//!
//! # Detection ([`detect`])
//!
//! Both signals are low-passed and decimated to about 8 kHz. Every 128 ms the last 256 ms of
//! input (Hann window) is cross-correlated with the reference over every lag from 0 to
//! `max_delay_s` (one FFT each); the cross-spectrum is averaged over about 0.3 s, 1 s and
//! 4 s, and each average is weighted by the regularised smoothed coherence normaliser
//! (GCC-SCOT), so the correlation peak is sharp whatever the music's spectrum and does not
//! care how loud the echo is. The decision statistic is the peak's height over the RMS of
//! the rest of the correlation ("z"); "present" also needs the peak to stay at one lag
//! (within 3 ms) over about a second. The singer is not correlated with the reference, so
//! singing only lowers z; silent music contributes no evidence (hops with a silent reference
//! are skipped, nothing decays); the guide vocal is part of the reference, so a returned
//! guide vocal is found like the rest of the song (trap 6). The 0.3 s average copes with a
//! delay that wanders (Bluetooth).
//!
//! The peak lag, refined between samples, is the delay at each hop; a straight line through
//! the last 20 s of them gives the clock drift (its slope) and how much the delay wanders
//! around it. The level is the power of the part of the input the reference predicts
//! (|S_xr|^2 / S_rr, bias-corrected), relative to the reference.
//!
//! # Removal ([`filter`], [`refline`])
//!
//! Once the echo is found, a partitioned-block frequency-domain NLMS filter (one set of
//! partitions per reference channel, so a mixer that returns only the left channel is
//! modelled exactly) learns the echo path: level, colouring and the fine delay, starting
//! `pre_s` before the measured delay and covering `filter_len_s`. It runs as two paths: a
//! background filter that always learns, with Valin's double-talk robust learning rate, and
//! a foreground filter that makes the output and takes the background's weights only when
//! they are clearly better and nobody sings (see [`filter`]).
//!
//! The reference is read from a delay line at a fractional position that slides with the
//! clock drift, so the echo stays still in the filter's view (nothing is resampled up front).
//! The drift comes from the filter itself: right after each foreground update, the peak of
//! its impulse response gives the delay to a small fraction of a sample, and a line through
//! the last 12 s of these gives the slope; until that line is long and steady, the
//! correlation's line is used. A filter held while somebody sings is not measured (it has not
//! followed the drift). If the correlation's delay disagrees with the filter's by more than
//! `relock_ms` for three hops running (a Bluetooth jump), or the echo is about to leave the
//! filter's span, the filter starts again at the new delay. Two restarts within a minute
//! mean the delay wanders.
//!
//! # The gate for the tracker ([`Trust`])
//!
//! Even good removal leaves some music. With the guide vocal on and nobody singing, that
//! residue still carries the right notes (trap 6), and the tracker's level gate is absolute.
//! So for every block the canceller compares the output power with the left-over echo it
//! expects: the echo estimate times the share of it that removal leaves, measured on blocks
//! whose input is all returned music (a floor of that ratio, with a margin), or the whole
//! expected echo when the delay wanders. A block counts as the singer only when the output is
//! `gate_margin_db` above that and the input holds more than the echo expected; otherwise
//! it is [`Trust::Music`], and the app must treat any pitch there as no pitch.
//! [`EchoCanceller::trust_window`] answers for the span a pitch event describes. Before the
//! canceller knows (the first quarter second of music, the start-up of the filter) the
//! verdict is [`Trust::Unknown`].
//!
//! The report's suppression and [`RemovalQuality`] use the plain mean of the same ratio
//! (conservative: it counts bursts of left-over music), the residual compared with the
//! singer's level once somebody has sung, else with the tracker's level gate.
//!
//! # Contract with the caller
//!
//! - Same sample rate for reference and input (resample the reference to the input's rate).
//! - Both streams continuous and sample-aligned by the caller to its best knowledge (the
//!   reference sample that left the DAC when the input sample was captured), with no
//!   per-block re-anchoring to jittery time stamps: the drift is measured here. If input
//!   samples were lost, feed zeros for them; on a device change call [`EchoCanceller::reset`].
//! - Delays are searched from 0 to `max_delay_s`: the echo must not arrive before the
//!   reference. If the caller's clocks are uncertain, it hands the reference a little early
//!   (for example 50 ms) and subtracts that from [`EchoReport::delay_ms`].
//! - The output lags the input by [`EchoCanceller::latency`] samples (about 5 ms).

pub(crate) mod detect;
pub(crate) mod filter;
pub(crate) mod refline;
pub mod testpath;

#[cfg(test)]
mod tests;

use detect::{Detector, Fit, Thresholds, fit_history};
use filter::{BlockStats, Pbfdaf};
use refline::{HALF, RefLine};

/// Whether the app's own playback is in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// Not enough music heard yet to say.
    Searching,
    /// The playback does not come back (or too weakly to find).
    Absent,
    /// The playback comes back in the input.
    Present,
}

/// How steady the delay of the returned playback is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayStability {
    /// Not measured long enough yet (about 5 s of music).
    Unknown,
    /// Fixed, or moving only in a straight line (clock drift): cable or USB.
    Stable,
    /// Jumping or wandering: Bluetooth, or something else that buffers.
    Wandering,
}

/// Whether removal is good enough to grade a performance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalQuality {
    /// No playback comes back: nothing to remove.
    NotNeeded,
    /// Playback found; removal is starting up or not yet measured.
    Checking,
    /// Removed well: what is left cannot move the marker. Grade normally (and say the mixer
    /// sends the music back).
    Good,
    /// Mostly removed: grade, but mark the result as less certain.
    Marginal,
    /// Not removed well enough (a wandering delay, or too much left): warn instead of grading.
    Unusable,
}

/// Verdict for a stretch of input, for the pitch tracker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// No returned playback (or no music playing): believe the tracker.
    Clean,
    /// Playback was removed and the output holds more than its left-overs: the singer.
    Singer,
    /// The output holds nothing but left-over music (and noise): any pitch is not the
    /// singer's. Treat as "no pitch".
    Music,
    /// Not known yet (the first moments, while detection and removal start, or while the
    /// left-overs have not been measured on any stretch without the singer). Treat as "no
    /// pitch" like [`Trust::Music`], for the marker and the grade: returned music may still
    /// be in the output (trap 6). This is what docs/benchmarks/practice-echo.md measured.
    Unknown,
}

/// Settings. Durations in seconds, levels in dB, so they mean the same at every rate.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EchoConfig {
    /// Channels of the reference handed to [`EchoCanceller::process`] (1 or 2). Two lets the
    /// filter model a mixer that returns only one side of the stereo mix.
    pub reference_channels: usize,
    /// Longest delay searched (Bluetooth returns arrive 100-300+ ms late).
    pub max_delay_s: f64,
    /// Length of the adaptive filter: covers the colouring of the return path and the drift
    /// before it is measured.
    pub filter_len_s: f64,
    /// Part of the filter placed before the measured delay.
    pub pre_s: f64,
    /// Mono reference level (dBFS, in the detection band) below which the music counts as
    /// silent: such stretches carry no evidence.
    pub music_floor_dbfs: f64,
    /// Peak-to-floor ratio of the 0.3 s correlation that declares the echo present.
    pub z_on_fast: f32,
    /// Peak-to-floor ratio of the 1 s correlation that declares the echo present.
    pub z_on: f32,
    /// Peak-to-floor ratio of the 4 s correlation that declares the echo present.
    pub z_on_long: f32,
    /// Below this (1 s correlation) for `hold_s` of music, a present echo is declared gone.
    pub z_off: f32,
    /// Peak-to-floor ratio needed to measure the delay at a hop.
    pub z_track: f32,
    /// Music heard without finding the echo before declaring it absent.
    pub absent_after_s: f64,
    /// Music heard without the echo before a present echo is declared gone.
    pub hold_s: f64,
    /// A delay measured by the correlation this far (ms) from where the filter has the
    /// echo, three hops running (about 0.4 s), restarts the filter at the new delay.
    pub relock_ms: f64,
    /// Output this many dB above the expected residual echo (plus noise) counts as the
    /// singer.
    pub gate_margin_db: f32,
    /// Before anybody has sung: residual echo (dBFS) at or below which removal counts as
    /// good (below the tracker's level gate, so left-overs alone cannot make a pitch).
    pub good_residual_dbfs: f32,
    /// Before anybody has sung: residual echo (dBFS) above which removal counts as unusable.
    pub usable_residual_dbfs: f32,
    /// Once the singer's level is known: residual echo this far (dB) or more under the
    /// singer counts as good (it cannot move the marker).
    pub good_below_voice_db: f32,
    /// Once the singer's level is known: residual echo less than this far (dB) under the
    /// singer counts as unusable.
    pub usable_below_voice_db: f32,
    /// Wander (RMS around the drift line, ms) above which the delay counts as wandering.
    pub wander_ms: f64,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self {
            reference_channels: 2,
            max_delay_s: 0.6,
            filter_len_s: 0.024,
            pre_s: 0.002,
            music_floor_dbfs: -70.0,
            z_on_fast: 12.0,
            z_on: 12.0,
            z_on_long: 9.0,
            z_off: 6.0,
            z_track: 8.0,
            absent_after_s: 3.0,
            hold_s: 3.0,
            relock_ms: 2.0,
            gate_margin_db: 6.0,
            good_residual_dbfs: -60.0,
            usable_residual_dbfs: -48.0,
            good_below_voice_db: 25.0,
            usable_below_voice_db: 15.0,
            wander_ms: 0.25,
        }
    }
}

/// What the canceller knows about the returned playback.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EchoReport {
    pub presence: Presence,
    /// Confidence in `presence`, 0..1.
    pub confidence: f32,
    /// Peak-to-floor ratio of the correlation (the largest of the 0.3 s, 1 s and 4 s
    /// averages).
    pub z: f32,
    /// Returned playback power relative to the reference power, dB.
    pub level_db: Option<f32>,
    /// Returned playback level at the input, dBFS RMS.
    pub echo_dbfs: Option<f32>,
    /// How late the playback comes back, relative to the reference as handed in, ms. This is
    /// the device delay the scorer needs (WO2 4.10 "The delay comes for free").
    pub delay_ms: Option<f64>,
    /// Clock drift between output and input, ppm (positive: the delay grows).
    pub drift_ppm: Option<f64>,
    /// RMS of the delay around the drift line over the last 20 s, ms.
    pub wander_ms: Option<f64>,
    pub stability: DelayStability,
    /// The adaptive filter is running.
    pub removing: bool,
    /// The filter has converged far enough to measure its own leakage.
    pub adapted: bool,
    /// Estimated suppression of the returned playback, dB.
    pub suppression_db: Option<f32>,
    /// Estimated returned playback left in the output, dBFS RMS.
    pub residual_dbfs: Option<f32>,
    /// The singer's level at the output (dBFS RMS, over blocks judged to be the singer),
    /// once somebody has sung.
    pub voice_dbfs: Option<f32>,
    pub quality: RemovalQuality,
    /// Times the filter restarted at a new delay.
    pub relocks: u32,
    /// Seconds of music analysed.
    pub music_s: f64,
}

/// Blocks of trust history kept (power of two).
const TRUST_RING: usize = 1024;
/// Drift fit needs this span of filter delay measurements before it is applied.
const DRIFT_MIN_SPAN_S: f64 = 3.0;
/// Filter delay measurements kept (one per detector hop, about 0.128 s).
const FHIST: usize = 128;
/// Span of the filter delay fit, seconds.
const FFIT_WINDOW_S: f64 = 12.0;
/// Filter delay spread (RMS around the drift line) above which the drift is not applied
/// and the delay counts as wandering, seconds.
const FFIT_STEADY_S: f64 = 0.000_05;
/// Restarts remembered for the wander verdict, and how long, seconds.
const RELOCK_MEMORY: usize = 4;
const RELOCK_MEMORY_S: f64 = 60.0;
/// A block counts as "only the returned music" when the input's power is at most this
/// factor (1 dB) above the echo estimate's: then nobody sings louder than the left-overs.
const ECHO_ONLY: f32 = 1.26;
/// The left-over echo ratio over such blocks follows lower values within this time (s) and
/// higher ones only within `RATIO_RISE_S`: it settles near the lower part of the ratio's
/// spread, so a singer quieter than the returned music, who slips into some of these blocks,
/// does not inflate it much (a floor tracker, as used for noise estimation).
const RATIO_FALL_S: f64 = 0.3;
const RATIO_RISE_S: f64 = 3.0;
/// Averaging time of the plain mean of the same ratio, s. The floor suits the gate (a
/// decision per block, robust to a singer slipping in); the mean, which also counts the
/// bursts of left-over music when the song changes while the filter is held for a singer,
/// is what the suppression report and the quality verdict use (the floor overstated the
/// suppression by up to 20 dB in such runs; see docs/benchmarks/practice-echo.md).
const RATIO_MEAN_S: f64 = 4.0;
/// Echo-only time needed before the ratio is used, s.
const RATIO_MIN_S: f64 = 0.3;
/// Margin on the measured ratio for its fluctuation from moment to moment (3 dB; see
/// docs/benchmarks/practice-echo.md for the calibration).
const RATIO_BIAS: f32 = 2.0;

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

/// Detection and removal of the app's own playback. See the module documentation.
pub struct EchoCanceller {
    cfg: EchoConfig,
    rate: u32,
    ch: usize,
    det: Detector,
    line: RefLine,
    filt: Pbfdaf,
    b: usize,
    pre: usize,
    /// Fixed delay of the input path inside the canceller (samples): `pre + HALF`, so the
    /// filter can see a little before the measured delay and the interpolator a little after.
    in_delay: usize,
    in_ring: Vec<f32>,
    in_pos: usize,
    blk_x: Vec<f32>,
    blk_d: Vec<f32>,
    out_buf: Vec<f32>,
    fill: usize,
    frame: [f32; 2],
    /// Input samples consumed.
    n_in: u64,
    locked: bool,
    /// Integer part of the filter's reference read delay at lock (samples).
    read_delay: f64,
    /// Detector delay at lock (seconds).
    lock_delay_s: f64,
    /// Drift compensation since lock (samples) and its rate (samples per sample).
    tau: f64,
    drift: f64,
    off_count: u32,
    relocks: u32,
    /// Input times (s) of the latest restarts at a new delay.
    relock_t: [f64; RELOCK_MEMORY],
    /// Delay measured from the adapted filter's impulse response (s) over time (s).
    fhist_t: [f64; FHIST],
    fhist_d: [f64; FHIST],
    fhist_len: usize,
    fhist_pos: usize,
    ffit: Option<Fit>,
    /// Latest delay measured from the filter (s), and the impulse-response peak (taps).
    fdelay: Option<f64>,
    fpeak: Option<f64>,
    /// Output-filter updates counted at the last delay measurement (only a freshly updated
    /// filter is measured: one held while somebody sings has not followed the drift), and
    /// measurements in a row rejected as far from the line.
    upd_seen: u32,
    outliers: u32,
    ir_e: Vec<f32>,
    // Gate state (per-block powers, smoothed over about 10 ms).
    a_gate: f32,
    pd_s: f32,
    pe_s: f32,
    py_s: f32,
    /// Mono reference energy of the block being filled, and a peak envelope of it that holds
    /// for about 0.7 s (longer than the longest delay searched): "music was just playing".
    blk_ref: f32,
    ref_env: f32,
    ref_decay: f32,
    /// Input, output and echo-estimate power smoothed over about 50 ms, and the floor of
    /// output / echo estimate over blocks that hold only the returned music: the share of the
    /// echo estimate that removal leaves in the output. (20 ms finds the music alone in
    /// shorter breaths, but puts the floor 8-9 dB lower: measured on generated music.)
    a_ratio: f32,
    a_ratio_fall: f32,
    a_ratio_rise: f32,
    pd_r: f32,
    pe_r: f32,
    py_r: f32,
    hint_r: f32,
    /// The same over about 10 ms, for the gate.
    hint_s: f32,
    /// The delay is judged to wander (updated every detector hop).
    wandering: bool,
    ratio_avg: f32,
    ratio_mean: f32,
    a_ratio_mean: f32,
    ratio_blocks: u32,
    ratio_min_blocks: u32,
    nf_cur: f32,
    nf_prev: f32,
    nf_blocks: u32,
    nf_window: u32,
    gate_k: f32,
    // Long-term powers for the level and suppression measured on echo-only blocks.
    a_long: f64,
    py_long: f64,
    px_long: f64,
    pd_music: f64,
    pe_music: f64,
    /// Output power per block over blocks judged to be the singer (4 s average), and
    /// such blocks counted.
    pe_voice: f64,
    voice_blocks: u32,
    trust: [Trust; TRUST_RING],
    /// Input index of the first sample of the newest block in `trust`, and blocks recorded.
    trust_last_start: i64,
    trust_count: u64,
    imp: Vec<f32>,
}

impl EchoCanceller {
    /// A canceller for input and reference at `rate` Hz with the default settings.
    pub fn new(rate: u32) -> Self {
        Self::with_config(rate, EchoConfig::default())
    }

    /// A canceller for input and reference at `rate` Hz. Everything is allocated here.
    ///
    /// Panics if `rate` is below 8 kHz or `reference_channels` is not 1 or 2.
    pub fn with_config(rate: u32, cfg: EchoConfig) -> Self {
        assert!(rate >= 8_000, "sample rate {rate} Hz is too low");
        assert!(
            (1..=2).contains(&cfg.reference_channels),
            "reference_channels must be 1 or 2"
        );
        let fs = f64::from(rate);
        let ch = cfg.reference_channels;
        let b = 1usize << ((fs * 0.002_67).log2().round() as u32);
        let p = ((cfg.filter_len_s * fs / b as f64).ceil() as usize).max(1);
        let pre = (cfg.pre_s * fs).round() as usize;
        let in_delay = pre + HALF;
        let th = Thresholds {
            z_on_fast: cfg.z_on_fast,
            z_on: cfg.z_on,
            z_on_long: cfg.z_on_long,
            z_off: cfg.z_off,
            z_track: cfg.z_track,
            absent_after_s: cfg.absent_after_s,
            hold_s: cfg.hold_s,
            music_floor: 10f64.powf(cfg.music_floor_dbfs / 10.0),
        };
        let history = in_delay + ((cfg.max_delay_s + 0.25) * fs) as usize;
        let blk_s = b as f64 / fs;
        Self {
            det: Detector::new(rate, cfg.max_delay_s, th),
            line: RefLine::new(ch, history),
            filt: Pbfdaf::new(rate, b, p, ch),
            rate,
            ch,
            b,
            pre,
            in_delay,
            in_ring: vec![0.0; in_delay.max(1)],
            in_pos: 0,
            blk_x: vec![0.0; ch * b],
            blk_d: vec![0.0; b],
            out_buf: vec![0.0; b],
            fill: 0,
            frame: [0.0; 2],
            n_in: 0,
            locked: false,
            read_delay: 0.0,
            lock_delay_s: 0.0,
            tau: 0.0,
            drift: 0.0,
            off_count: 0,
            relocks: 0,
            relock_t: [f64::NEG_INFINITY; RELOCK_MEMORY],
            fhist_t: [0.0; FHIST],
            fhist_d: [0.0; FHIST],
            fhist_len: 0,
            fhist_pos: 0,
            ffit: None,
            fdelay: None,
            fpeak: None,
            upd_seen: 0,
            outliers: 0,
            ir_e: vec![0.0; p * b],
            a_gate: (1.0 - (-blk_s / 0.010).exp()) as f32,
            pd_s: 0.0,
            pe_s: 0.0,
            py_s: 0.0,
            blk_ref: 0.0,
            ref_env: 0.0,
            ref_decay: (-blk_s / 0.7).exp() as f32,
            a_ratio: (1.0 - (-blk_s / 0.050).exp()) as f32,
            a_ratio_fall: (1.0 - (-blk_s / RATIO_FALL_S).exp()) as f32,
            a_ratio_rise: (1.0 - (-blk_s / RATIO_RISE_S).exp()) as f32,
            pd_r: 0.0,
            pe_r: 0.0,
            py_r: 0.0,
            hint_r: 0.0,
            hint_s: 0.0,
            wandering: false,
            ratio_avg: 1.0,
            ratio_mean: 1.0,
            a_ratio_mean: (1.0 - (-blk_s / RATIO_MEAN_S).exp()) as f32,
            ratio_blocks: 0,
            ratio_min_blocks: (RATIO_MIN_S / blk_s).round() as u32,
            nf_cur: f32::MAX,
            nf_prev: f32::MAX,
            nf_blocks: 0,
            nf_window: (1.0 / blk_s).round() as u32,
            gate_k: 10f32.powf(cfg.gate_margin_db / 10.0),
            a_long: 1.0 - (-blk_s / 4.0).exp(),
            py_long: 0.0,
            px_long: 0.0,
            pd_music: 0.0,
            pe_music: 0.0,
            pe_voice: 0.0,
            voice_blocks: 0,
            trust: [Trust::Unknown; TRUST_RING],
            trust_last_start: 0,
            trust_count: 0,
            imp: vec![0.0; p * b],
            cfg,
        }
    }

    /// The settings in use.
    pub fn config(&self) -> &EchoConfig {
        &self.cfg
    }

    /// Sample rate in Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Output sample `j` is the cleaned input sample `j - latency()`.
    pub fn latency(&self) -> usize {
        self.in_delay + self.b
    }

    /// Forget everything (a new input or output device). Does not allocate.
    pub fn reset(&mut self) {
        self.det.reset();
        self.line.clear();
        self.filt.reset(true);
        self.in_ring.fill(0.0);
        self.in_pos = 0;
        self.blk_x.fill(0.0);
        self.blk_d.fill(0.0);
        self.out_buf.fill(0.0);
        self.fill = 0;
        self.n_in = 0;
        self.locked = false;
        self.read_delay = 0.0;
        self.lock_delay_s = 0.0;
        self.tau = 0.0;
        self.drift = 0.0;
        self.off_count = 0;
        self.relocks = 0;
        self.relock_t = [f64::NEG_INFINITY; RELOCK_MEMORY];
        self.clear_fhist();
        self.pd_s = 0.0;
        self.pe_s = 0.0;
        self.py_s = 0.0;
        self.blk_ref = 0.0;
        self.ref_env = 0.0;
        self.clear_ratio();
        self.nf_cur = f32::MAX;
        self.nf_prev = f32::MAX;
        self.nf_blocks = 0;
        self.py_long = 0.0;
        self.px_long = 0.0;
        self.pd_music = 0.0;
        self.pe_music = 0.0;
        self.pe_voice = 0.0;
        self.voice_blocks = 0;
        self.trust = [Trust::Unknown; TRUST_RING];
        self.trust_last_start = 0;
        self.trust_count = 0;
    }

    /// Process a block. `reference` is interleaved with `reference_channels` samples per
    /// frame and holds as many frames as `input` has samples; `out` receives the input with
    /// the returned playback removed, [`EchoCanceller::latency`] samples late (the first
    /// output samples of a stream are zeros). Any block length. Never allocates.
    ///
    /// Panics if the lengths disagree.
    pub fn process(&mut self, reference: &[f32], input: &[f32], out: &mut [f32]) {
        assert_eq!(reference.len(), input.len() * self.ch, "reference frames");
        assert_eq!(out.len(), input.len(), "output length");
        for (i, &x) in input.iter().enumerate() {
            let x = if x.is_finite() { x } else { 0.0 };
            let fr = &reference[i * self.ch..(i + 1) * self.ch];
            let mono = if self.ch == 2 {
                0.5 * (fr[0] + fr[1])
            } else {
                fr[0]
            };
            let mono = if mono.is_finite() { mono } else { 0.0 };
            self.blk_ref += mono * mono;
            self.line.push(fr);
            if self.det.push(x, mono) {
                self.on_hop();
            }
            // The input, delayed by `in_delay`.
            let d = self.in_ring[self.in_pos];
            self.in_ring[self.in_pos] = x;
            self.in_pos = (self.in_pos + 1) % self.in_ring.len();
            // The filter's reference for that delayed sample.
            if self.locked {
                self.tau += self.drift;
                let pos = (self.line.written() - 1) as f64 - (self.read_delay + self.tau);
                let mut fr = self.frame;
                self.line.read(pos, &mut fr[..self.ch]);
                for c in 0..self.ch {
                    self.blk_x[c * self.b + self.fill] = fr[c];
                }
            }
            out[i] = self.out_buf[self.fill];
            self.blk_d[self.fill] = d;
            self.fill += 1;
            self.n_in += 1;
            if self.fill == self.b {
                self.fill = 0;
                self.block_done();
            }
        }
    }

    /// A block of `b` samples is complete.
    fn block_done(&mut self) {
        let st = if self.locked {
            // Hold the output filter while the input holds more than the echo expected (the
            // filter's estimate, or the detector's level while the filter is still poor): a
            // singer louder than about 6 dB under the returned music.
            let expected = self.py_r.max(self.hint_r);
            let quiet = !self.filt.adapted() || self.pd_r <= ECHO_ONLY * expected;
            Some(
                self.filt
                    .step(&self.blk_x, &self.blk_d, &mut self.out_buf, quiet),
            )
        } else {
            self.out_buf.copy_from_slice(&self.blk_d);
            None
        };
        let start = self.n_in as i64 - self.b as i64 - self.in_delay as i64;
        self.ref_env = self.blk_ref.max(self.ref_env * self.ref_decay);
        self.blk_ref = 0.0;
        let t = self.block_trust(st);
        self.trust[(self.trust_count as usize) % TRUST_RING] = t;
        self.trust_last_start = start;
        self.trust_count += 1;
    }

    /// Trust verdict and running statistics for a finished block.
    fn block_trust(&mut self, st: Option<BlockStats>) -> Trust {
        let presence = self.det.state().presence;
        let st = match st {
            Some(s) => s,
            None => {
                let pd: f32 = self.blk_d.iter().map(|v| v * v).sum();
                return match presence {
                    Presence::Absent => Trust::Clean,
                    // Searching, or found but not yet locked: unknown while music plays (or
                    // just played) and the input is not silent. From the very first block:
                    // the detector's first analysis comes only after a quarter of a second.
                    _ => {
                        let floor = 10f64.powf(self.cfg.music_floor_dbfs / 10.0) as f32;
                        let music = self.ref_env > floor * self.b as f32;
                        if music && pd > 1e-9 * self.b as f32 {
                            Trust::Unknown
                        } else {
                            Trust::Clean
                        }
                    }
                };
            }
        };
        let a = self.a_gate;
        self.pd_s += a * (st.pd - self.pd_s);
        self.pe_s += a * (st.pe - self.pe_s);
        self.py_s += a * (st.py - self.py_s);
        // Noise floor: minimum of the smoothed output power over two one-second windows.
        self.nf_cur = self.nf_cur.min(self.pe_s);
        self.nf_blocks += 1;
        if self.nf_blocks >= self.nf_window {
            self.nf_prev = self.nf_cur;
            self.nf_cur = f32::MAX;
            self.nf_blocks = 0;
        }
        let noise = self.nf_cur.min(self.nf_prev).min(self.pe_s);
        let al = self.a_long;
        self.py_long += al * (f64::from(st.py) - self.py_long);
        self.px_long += al * (f64::from(st.px) - self.px_long);
        let ref_active = st.px > 1e-9 * self.b as f32;
        // Left-over echo ratio, measured where the input is all returned music (its power
        // explained by the echo estimate): there the output holds only what removal left.
        let ar = self.a_ratio;
        self.pd_r += ar * (st.pd - self.pd_r);
        self.pe_r += ar * (st.pe - self.pe_r);
        self.py_r += ar * (st.py - self.py_r);
        // The echo the detector expects in this block (its level times the mono reference
        // power: the reference channels' sum over two for a stereo reference).
        let hint = self.det.state().level.unwrap_or(0.0) as f32;
        let px_mono = st.px / self.ch as f32;
        self.hint_r += ar * (hint * px_mono - self.hint_r);
        self.hint_s += self.a_gate * (hint * px_mono - self.hint_s);
        if self.filt.adapted()
            && ref_active
            && self.py_r > 1e-12 * self.b as f32
            && self.pd_r <= ECHO_ONLY * self.py_r
        {
            let r = (self.pe_r / self.py_r).min(1.0);
            if self.ratio_blocks == 0 {
                self.ratio_avg = r;
                self.ratio_mean = r;
            } else {
                self.ratio_mean += self.a_ratio_mean * (r - self.ratio_mean);
                let a = if r < self.ratio_avg {
                    self.a_ratio_fall
                } else {
                    self.a_ratio_rise
                };
                self.ratio_avg += a * (r - self.ratio_avg);
            }
            self.ratio_blocks = self.ratio_blocks.saturating_add(1);
        }
        let Some(ratio) = self.residual_ratio() else {
            return Trust::Unknown;
        };
        let mut residual = ratio * self.py_s;
        if self.wandering {
            // The delay wanders (Bluetooth): what removal leaves cannot be measured, so the
            // gate assumes nothing was removed, with 3 dB more for the detector's level, which
            // a wandering delay biases low (measured up to 5 dB). A singer clearly louder than
            // the returned music still counts; left-over music never does.
            residual = residual.max(2.0 * self.hint_s);
        }
        // An input fully explained by the echo expected (the filter's estimate, or the
        // detector's level) holds nobody louder than about 6 dB under the music: music, even
        // if the output looks louder than the left-overs measured so far. This covers the
        // moments the left-overs jump before the measurement follows (an instrument entering
        // in a band the filter has not learnt yet).
        let explained = self.pd_s <= ECHO_ONLY * self.py_s.max(self.hint_s);
        let voice = !explained && self.pe_s > self.gate_k * residual.max(noise) + 1e-12;
        if voice {
            if self.voice_blocks == 0 {
                self.pe_voice = f64::from(st.pe);
            } else {
                self.pe_voice += al * (f64::from(st.pe) - self.pe_voice);
            }
            self.voice_blocks = self.voice_blocks.saturating_add(1);
        }
        if !voice && ref_active {
            // Echo-only block: what removal achieved, measured.
            self.pd_music += al * (f64::from(st.pd) - self.pd_music);
            self.pe_music += al * (f64::from(st.pe) - self.pe_music);
        }
        if voice { Trust::Singer } else { Trust::Music }
    }

    /// Input time in seconds.
    fn now_s(&self) -> f64 {
        self.n_in as f64 / f64::from(self.rate)
    }

    fn clear_fhist(&mut self) {
        self.fhist_len = 0;
        self.fhist_pos = 0;
        self.ffit = None;
        self.fdelay = None;
        self.fpeak = None;
        self.outliers = 0;
    }

    /// Fractional tap of the filter's impulse-response peak (parabola through the peak and
    /// its neighbours), measured on the response to what the channels have in common (the
    /// sum of the channels' responses, squared). With a stereo reference whose channels are
    /// nearly alike, the filter can hold any amount of "+x on the left, -x on the right"
    /// without changing its output; that part is no echo and cancels in the sum (measured:
    /// such a spike at tap 0 restarted the filter twice in a cable run). None unless the peak
    /// clearly stands out (at least 10 times the mean tap energy): a filter disturbed by
    /// singing must not move the measured delay.
    fn ir_peak(&mut self) -> Option<f64> {
        self.ir_e.fill(0.0);
        let mut imp = std::mem::take(&mut self.imp);
        for c in 0..self.ch {
            self.filt.impulse(c, &mut imp);
            for (e, h) in self.ir_e.iter_mut().zip(&imp) {
                *e += h;
            }
        }
        for e in self.ir_e.iter_mut() {
            *e *= *e;
        }
        self.imp = imp;
        // The peak is searched on the energy smoothed over five taps (weights 1 2 3 2 1): the
        // echo's main lobe spans several taps, while the per-bin learning rates leave
        // single-tap spikes where the filter's blocks join (every `b` taps; measured about
        // -25 dB under the echo, but able to win while the echo is weak). The first and last
        // few taps are left out: a filter cut short of a long low-frequency tail piles up
        // energy at its edges.
        let e = &self.ir_e;
        let edge = 4.min(e.len() / 4);
        let (mut best, mut peak) = (edge, 0.0f32);
        for j in edge..e.len() - edge {
            let v = e[j - 2] + 2.0 * e[j - 1] + 3.0 * e[j] + 2.0 * e[j + 1] + e[j + 2];
            if v > peak {
                peak = v;
                best = j;
            }
        }
        let peak = e[best];
        let mean = e.iter().sum::<f32>() / e.len() as f32;
        if peak <= 1e-20 || peak < 10.0 * mean {
            return None;
        }
        let mut j = best as f64;
        if best > 0 && best + 1 < e.len() {
            let (ym, y0, yp) = (
                f64::from(e[best - 1]),
                f64::from(peak),
                f64::from(e[best + 1]),
            );
            let den = ym - 2.0 * y0 + yp;
            if den < -1e-30 {
                j += (0.5 * (ym - yp) / den).clamp(-0.5, 0.5);
            }
        }
        Some(j)
    }

    fn clear_ratio(&mut self) {
        self.pd_r = 0.0;
        self.pe_r = 0.0;
        self.py_r = 0.0;
        self.hint_r = 0.0;
        self.hint_s = 0.0;
        self.ratio_avg = 1.0;
        self.ratio_mean = 1.0;
        self.ratio_blocks = 0;
    }

    /// Mean left-over echo as a share of the echo estimate, for the report, once measured on
    /// enough echo-only blocks since the filter (re)started.
    fn mean_residual_ratio(&self) -> Option<f32> {
        (self.ratio_blocks >= self.ratio_min_blocks).then_some(self.ratio_mean)
    }

    /// Expected left-over echo as a share of the echo estimate (with its margin), once
    /// measured on enough echo-only blocks since the filter (re)started.
    fn residual_ratio(&self) -> Option<f32> {
        (self.ratio_blocks >= self.ratio_min_blocks).then(|| (self.ratio_avg * RATIO_BIAS).min(1.0))
    }

    /// A detector hop finished: lock, measure the delay from the filter, follow the drift,
    /// restart at a new delay, or let go.
    fn on_hop(&mut self) {
        let ds = *self.det.state();
        self.wandering = ds.presence == Presence::Present
            && (self.recent_relocks() >= 2
                || ds.fit.is_some_and(|f| {
                    f.span_s >= 4.0 && f.n >= 10 && f.rms_s > self.cfg.wander_ms * 1e-3
                }));
        let fs = f64::from(self.rate);
        match ds.presence {
            Presence::Present => {
                if !self.locked {
                    if let Some(d) = ds.delay_s {
                        self.lock(d);
                    }
                    return;
                }
                let now = self.now_s();
                // 1. The delay as the adapted filter sees it: where its impulse response
                //    peaks, plus where the reference is read. Precise (a small fraction of a
                //    sample), and blind to the singer: only a filter updated since the last
                //    measurement is measured (it is held while somebody sings).
                let fresh = self.filt.updates() != self.upd_seen;
                if fresh
                    && self.filt.adapted()
                    && self.residual_ratio().is_some_and(|r| r < 0.3)
                    && let Some(j) = self.ir_peak()
                {
                    self.upd_seen = self.filt.updates();
                    let d = (self.read_delay + self.tau - self.in_delay as f64 + j) / fs;
                    // Plausible: near the line so far, or (no line yet) within 1 ms of where
                    // the echo was put at the (re)start.
                    let near = match self.ffit {
                        Some(f) => (d - f.delay_now_s).abs() < 0.000_5,
                        None => (d - (self.lock_delay_s + self.tau / fs)).abs() < 0.001,
                    };
                    if near {
                        self.outliers = 0;
                        self.fhist_t[self.fhist_pos] = now;
                        self.fhist_d[self.fhist_pos] = d;
                        self.fhist_pos = (self.fhist_pos + 1) % FHIST;
                        self.fhist_len = (self.fhist_len + 1).min(FHIST);
                        self.fdelay = Some(d);
                        self.fpeak = Some(j);
                        self.ffit = fit_history(
                            &self.fhist_t[..self.fhist_len],
                            &self.fhist_d[..self.fhist_len],
                            now,
                            FFIT_WINDOW_S,
                        );
                        // Keep the echo where the filter expects it (slowly; the measured
                        // delay does not depend on where the reference is read).
                        let err = j - self.pre as f64;
                        if err.abs() > 2.0 {
                            self.tau += (0.05 * err).clamp(-0.02, 0.02);
                        }
                        // The echo about to leave the filter's span: start again around it.
                        let span = self.ir_e.len() as f64;
                        if j < 0.25 * self.pre as f64 || j > span - 0.001 * fs {
                            self.relock(d, now);
                            return;
                        }
                    } else {
                        // Far from the line: an outlier, unless it keeps happening (then the
                        // line was wrong: start it again).
                        self.outliers += 1;
                        if self.outliers >= 8 {
                            self.clear_fhist();
                        }
                    }
                }
                // Clock drift: slide the reference read position at the measured rate, so
                // the echo stands still in the filter's view. The filter's own line when it
                // is long and steady, else the correlation's (independent of the filter, a
                // few ppm less precise).
                if let Some(f) = self.ffit
                    && f.span_s >= DRIFT_MIN_SPAN_S
                    && f.n >= 15
                    && f.rms_s < FFIT_STEADY_S
                {
                    self.drift = f.slope;
                } else if let Some(f) = ds.fit
                    && f.span_s >= 6.0
                    && f.n >= 30
                    && f.rms_s < self.cfg.wander_ms * 1e-3
                {
                    self.drift = f.slope;
                }
                // 2. A jump (Bluetooth, a device that re-buffered): the coarse delay from
                //    the correlation disagrees with where the filter is, several hops running.
                if let Some(d) = ds.delay_s {
                    let expected = match self.ffit {
                        Some(f) if f.n >= 8 => f.delay_now_s,
                        _ => self.lock_delay_s + self.tau / fs,
                    };
                    if (d - expected).abs() > self.cfg.relock_ms * 1e-3 {
                        self.off_count += 1;
                        if self.off_count >= 3 {
                            self.relock(d, now);
                        }
                    } else {
                        self.off_count = 0;
                    }
                }
            }
            Presence::Absent => {
                if self.locked {
                    self.locked = false;
                    self.filt.reset(true);
                    self.drift = 0.0;
                    self.tau = 0.0;
                    self.clear_fhist();
                }
            }
            Presence::Searching => {}
        }
    }

    /// Restart the filter at a new delay (counted for the wander verdict).
    fn relock(&mut self, delay_s: f64, now: f64) {
        self.relocks += 1;
        self.relock_t.copy_within(1.., 0);
        self.relock_t[RELOCK_MEMORY - 1] = now;
        self.lock(delay_s);
    }

    /// Restarts in the last [`RELOCK_MEMORY_S`].
    fn recent_relocks(&self) -> usize {
        let now = self.now_s();
        self.relock_t
            .iter()
            .filter(|&&t| now - t <= RELOCK_MEMORY_S)
            .count()
    }

    /// Start (or restart) the filter for an echo `delay_s` late.
    fn lock(&mut self, delay_s: f64) {
        let fs = f64::from(self.rate);
        let dr = (self.in_delay as f64 + delay_s * fs - self.pre as f64).round();
        self.read_delay = dr.max(HALF as f64 + 1.0);
        self.lock_delay_s = delay_s;
        self.tau = 0.0;
        self.drift = 0.0;
        self.off_count = 0;
        self.clear_fhist();
        self.clear_ratio();
        self.upd_seen = self.filt.updates();
        self.filt.reset(true);
        // Start-up steps follow the echo the detector measured (in its band).
        let hint = self.det.state().level.unwrap_or(1.0);
        self.filt.set_hint(hint as f32);
        self.locked = true;
    }

    /// The verdict for input samples `start..end` (input sample indices counted from the
    /// first sample given): the verdict of most blocks in the span. [`Trust::Unknown`] for a
    /// span outside the history kept (about 2.7 s at 48 kHz).
    ///
    /// For a tracker running on the output, an event at output sample `t` describes input
    /// samples around `t - latency()`.
    pub fn trust_window(&self, start: i64, end: i64) -> Trust {
        if self.trust_count == 0 || end <= start {
            return Trust::Unknown;
        }
        let b = self.b as i64;
        let newest = self.trust_count as i64 - 1;
        let oldest = (newest - TRUST_RING as i64 + 1).max(0);
        let mut counts = [0u32; 4];
        let mut any = false;
        // Block k (0 = first recorded) starts at trust_last_start - (newest - k) * b.
        let k0 = newest - (self.trust_last_start - start).div_euclid(b);
        let k1 = newest - (self.trust_last_start - (end - 1)).div_euclid(b);
        for k in k0.max(oldest)..=k1.min(newest) {
            let t = self.trust[(k as usize) % TRUST_RING];
            counts[t as usize] += 1;
            any = true;
        }
        if !any {
            return Trust::Unknown;
        }
        let order = [Trust::Clean, Trust::Singer, Trust::Music, Trust::Unknown];
        let total: u32 = counts.iter().sum();
        // Any unknown block makes the span unknown only if unknown blocks are the majority;
        // "music" wins ties against "singer" (never let left-overs pass as the singer).
        let (mut best, mut best_n) = (Trust::Unknown, 0);
        for t in order {
            let c = counts[t as usize];
            if c > best_n || (c == best_n && t == Trust::Music) {
                best = t;
                best_n = c;
            }
        }
        if counts[Trust::Music as usize] * 2 >= total {
            return Trust::Music;
        }
        best
    }

    /// The measured delay of the returned playback in seconds, for the scorer's device
    /// delay (WO2 4.10 "The delay comes for free"), when it is present: from the adapted
    /// filter when it has measured it (a small fraction of a sample), else from the
    /// correlation (about 0.1 ms).
    pub fn measured_delay_s(&self) -> Option<f64> {
        let ds = self.det.state();
        if ds.presence != Presence::Present {
            return None;
        }
        if self.locked {
            match (self.ffit, self.fdelay) {
                (Some(f), _) if f.n >= 4 && f.rms_s < FFIT_STEADY_S => return Some(f.delay_now_s),
                (_, Some(d)) => return Some(d),
                _ => {}
            }
        }
        match ds.fit {
            Some(f) if f.span_s >= 2.0 && f.rms_s < self.cfg.wander_ms * 1e-3 => {
                Some(f.delay_now_s)
            }
            _ => ds.delay_s,
        }
    }

    /// The filter's current impulse response for reference channel `c` (diagnostics):
    /// tap `pre_s * rate` is where the measured delay put the echo.
    pub fn impulse_response(&mut self, c: usize) -> &[f32] {
        let mut imp = std::mem::take(&mut self.imp);
        self.filt.impulse(c, &mut imp);
        self.imp = imp;
        &self.imp
    }

    /// Everything known about the returned playback now.
    pub fn report(&self) -> EchoReport {
        let ds = self.det.state();
        let fs = f64::from(self.rate);
        let z = ds.z_short.max(ds.z_long).max(ds.z_fast);
        let sig = |z: f32| 1.0 / (1.0 + (-(z - 0.5 * (self.cfg.z_on_long + self.cfg.z_off))).exp());
        let confidence = match ds.presence {
            Presence::Searching => 0.0,
            Presence::Present => sig(z),
            Presence::Absent => {
                (1.0 - sig(z)) * (ds.music_s / self.cfg.absent_after_s).min(1.0) as f32
            }
        };
        let present = ds.presence == Presence::Present;
        let adapted = self.locked && self.filt.adapted();
        // Level: from the filter's echo estimate once adapted (whole band), else from the
        // detector (its band).
        let level = if adapted && self.px_long > 1e-30 {
            // px sums the reference channels; the reference power is that of their mean.
            let ref_block = ds.ref_ms * self.b as f64;
            Some(self.py_long / ref_block.max(1e-30))
        } else {
            ds.level
        };
        let level_db = if present {
            level.map(|l| db(l) as f32)
        } else {
            None
        };
        let echo_dbfs = level_db.map(|l| l + db(ds.ref_ms) as f32);
        let fit = if present { ds.fit } else { None };
        // The filter's own measurement once it has one long enough, else the correlation's.
        let ffit = if present && self.locked {
            self.ffit.filter(|f| f.span_s >= 4.0 && f.n >= 20)
        } else {
            None
        };
        let stability = if present && self.recent_relocks() >= 2 {
            DelayStability::Wandering
        } else if let Some(f) = ffit {
            if f.rms_s > FFIT_STEADY_S {
                DelayStability::Wandering
            } else {
                DelayStability::Stable
            }
        } else {
            match fit {
                Some(f) if f.span_s >= 4.0 && f.n >= 10 => {
                    if f.rms_s > self.cfg.wander_ms * 1e-3 {
                        DelayStability::Wandering
                    } else {
                        DelayStability::Stable
                    }
                }
                _ => DelayStability::Unknown,
            }
        };
        let suppression_db = if adapted {
            self.mean_residual_ratio().map(|r| -db(f64::from(r)) as f32)
        } else {
            None
        };
        let residual_dbfs = match (echo_dbfs, suppression_db) {
            (Some(e), Some(s)) => Some(e - s),
            _ => None,
        };
        let quality = if !present {
            RemovalQuality::NotNeeded
        } else if stability == DelayStability::Wandering {
            RemovalQuality::Unusable
        } else if !adapted || residual_dbfs.is_none() {
            RemovalQuality::Checking
        } else {
            let r = residual_dbfs.unwrap_or(0.0);
            let (good, usable) = match self.voice_dbfs() {
                Some(v) => (
                    v - self.cfg.good_below_voice_db,
                    v - self.cfg.usable_below_voice_db,
                ),
                None => (self.cfg.good_residual_dbfs, self.cfg.usable_residual_dbfs),
            };
            if r <= good {
                RemovalQuality::Good
            } else if r <= usable {
                RemovalQuality::Marginal
            } else {
                RemovalQuality::Unusable
            }
        };
        let delay_ms = if present {
            self.measured_delay_s().map(|d| d * 1e3)
        } else {
            None
        };
        let _ = fs;
        EchoReport {
            presence: ds.presence,
            confidence,
            z,
            level_db,
            echo_dbfs,
            delay_ms,
            drift_ppm: ffit
                .or(fit.filter(|f| f.span_s >= 4.0))
                .map(|f| f.slope * 1e6),
            wander_ms: ffit
                .or(fit.filter(|f| f.span_s >= 4.0))
                .map(|f| f.rms_s * 1e3),
            stability,
            removing: self.locked,
            adapted,
            suppression_db,
            residual_dbfs,
            voice_dbfs: self.voice_dbfs(),
            quality,
            relocks: self.relocks,
            music_s: ds.music_s,
        }
    }

    /// The singer's level at the output (dBFS RMS), after at least half a second judged to be
    /// the singer.
    fn voice_dbfs(&self) -> Option<f32> {
        let half_s = (0.5 * f64::from(self.rate) / self.b as f64) as u32;
        (self.voice_blocks >= half_s && self.pe_voice > 0.0)
            .then(|| db(self.pe_voice / self.b as f64) as f32)
    }

    /// Suppression measured on blocks the gate judged to hold left-over music only (input
    /// power / output power), dB. For diagnostics and the benchmark.
    pub fn measured_suppression_db(&self) -> Option<f32> {
        (self.pe_music > 1e-30 && self.pd_music > 1e-30)
            .then(|| db(self.pd_music / self.pe_music) as f32)
    }

    /// Filter restarts by the divergence guard (diagnostics).
    pub fn filter_resets(&self) -> u32 {
        self.filt.resets()
    }

    /// The correlation's delay line fit (drift ppm, RMS ms, span s), for diagnostics.
    pub fn detector_fit(&self) -> Option<(f64, f64, f64)> {
        self.det
            .state()
            .fit
            .map(|f| (f.slope * 1e6, f.rms_s * 1e3, f.span_s))
    }

    /// Times the output filter took the learning filter's weights (diagnostics).
    pub fn filter_updates(&self) -> u32 {
        self.filt.updates()
    }

    /// Leakage the learning filter uses for its step (diagnostics).
    pub fn filter_leak(&self) -> f32 {
        self.filt.leak()
    }
}

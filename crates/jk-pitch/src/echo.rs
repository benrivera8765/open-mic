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
//! A second, spectral look at the output ([`spectral`], "tier 2") recognises the singer by
//! energy standing above the most music there can be in each frequency bin, which needs no
//! measured left-overs: it keeps a high note quieter than the music it sits in, and works
//! with continuous singing from the first second (whose left-over share comes from the
//! spectral regression until the singer pauses). With a wandering delay the gate compares
//! with the loudest the echo can be within +-40 ms of the filter's position.
//!
//! # Changes of the echo path
//!
//! An echo-path gain tracker compares the input with the filter's echo estimate over the
//! last half second (and the last 84 ms). A return switched off (the mixer's loopback, what
//! the warning asks for) is dropped within about half a second with nobody singing and
//! within about three with a singer (removal suspended, a fresh detector must confirm the
//! echo within 2 s); the detector's scale-invariant averages are cleared so they do not keep
//! a vanished echo for tens of seconds. A path that got louder (the OS or mixer volume)
//! scales the expected echo and holds the level comparison for half a second. The engine's
//! output tap is taken after the master volume, so the app's own volume control changes
//! reference and return alike and is not a path change. Measurements:
//! docs/benchmarks/practice-echo.md sections 10-12.
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
pub(crate) mod spectral;
pub mod testpath;

#[cfg(test)]
mod tests;

use detect::{Detector, Fit, Thresholds, fit_history};
use filter::{BlockStats, Pbfdaf};
use refline::{HALF, RefLine};
use spectral::Spectral;

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
    /// Times the returned playback was found gone while it was being removed (the mixer's
    /// loopback switched off, or turned down by more than about 12 dB): "present" was
    /// dropped at once instead of waiting for the correlation to fade.
    #[serde(default)]
    pub lost: u32,
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
/// Margin on the share of the echo left measured by the spectral look's regression during
/// singing (6 dB): noisier than the measurement on music without singing; measured on
/// continuous singing it was up to 7 dB under the true left-overs without a margin.
const REGRESSION_BIAS: f32 = 4.0;
/// Echo path gain tracker: chunks of about this length (s) of input x echo estimate and
/// echo estimate x echo estimate, and how many are kept (about 0.5 s).
const PG_CHUNK_S: f64 = 0.021;
const PG_CHUNKS: usize = 24;
/// The newest chunks of the tracker (about 84 ms) also give a fast path gain, used only to
/// see a path that got louder at once (the OS or mixer volume turned up), before the half
/// second average follows.
const PG_FAST: usize = 4;
/// After the path was last seen significantly louder than the filter learnt, the per-block
/// comparison may not call a block the singer for this long (s): what removal leaves is
/// not known until the filter has re-learnt the path band by band (measured: with only the
/// gain in the bound, a return turned up 6 dB still showed 1.8 % pitch frames with nobody
/// singing in the 5 s after). The spectral look, whose bound holds for a path up to about
/// 9 dB louder, still recognises the singer meanwhile.
const LOUDER_HOLD_S: f64 = 0.5;
/// The returned playback is gone when the path gain (amplitude, relative to what the
/// filter learnt) is below this with 3 standard errors to spare (-12 dB), or when the
/// input holds less than [`GONE_POWER`] of the echo the detector expects (-25 dB), or less
/// than a tenth of it while down at the noise floor (with nobody singing, a return switched
/// off leaves only the floor). Measured with a plain -10 dB rule: a return turned down
/// 20 dB (OS volume) was taken for "off" at once, and the left-overs showed as pitch for
/// about a second until it was found again (3.8 % of the frames in the 5 s after); now
/// such a drop goes the way of the gain test (removal suspended, the echo confirmed again
/// within the grace time, "present" kept). With the -25 dB rule alone a -40 dB return
/// switched off took 19 s to clear on one song (the floor is not 25 dB under it).
const GONE_GAIN: f64 = 0.25;
const GONE_POWER: f64 = 0.003;
/// Below this path gain (2 standard errors to spare) subtracting the estimate would add
/// more than it removes ((1 - g)^2 > g^2): the input passes through untouched.
const BYPASS_GAIN: f64 = 0.5;
/// The power test says "gone" at once only when the filter has run this long since it
/// started (s), so that the detector's level is settled; earlier it only suspends removal
/// (see [`GRACE_S`]). Measured: 0.3 s after a start, a level overestimated while a loud
/// singer held a note took a -30 dB return for gone.
const GONE_POWER_AFTER_S: f64 = 4.0;
/// After the gain test says "gone" the canceller stops removing and the detector starts
/// afresh; the echo is declared absent only if the fresh correlation has not found it again
/// within this time (s). A filter that went wrong (it learnt part of a voice) also fails
/// the gain test: then the echo is found again in under a second and the filter restarts,
/// without the warning ever dropping.
const GRACE_S: f64 = 2.0;
/// With a wandering delay (Bluetooth) the gate compares the output with the loudest the
/// echo can be anywhere within this many seconds either side of where the filter expects
/// it (the Bluetooth model jumps up to 25 ms; the filter restarts at a new delay only
/// about 0.4 s after a jump). Measured: with the echo expected only at the filter's delay,
/// up to 1.31 % of the frames with nobody singing were shown as pitch (a drum hit arriving
/// while the expected echo was quiet).
const WANDER_SPAN_S: f64 = 0.040;
/// With a wandering delay the gate takes the echo as this much louder than the detector's
/// level (6 dB; the level is biased low by a wandering delay, measured up to 5.5 dB).
/// Measured on the held-out songs: with 3 dB, up to 2.56 % of the frames with nobody
/// singing were shown as pitch.
const WANDER_BIAS: f32 = 4.0;

/// What the echo path gain tracker concluded at the end of a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathVerdict {
    Keep,
    /// The gain test failed: gone, or the filter went wrong. Confirm with a fresh detector.
    Suspect,
    /// The input holds far less than the echo expected: gone.
    Gone,
}

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
    /// The echo the detector expects (its level times the reference), same averaging: with
    /// `py_long`, how much of the echo the output filter has captured.
    hint_long: f64,
    pd_music: f64,
    pe_music: f64,
    /// Output power per block over blocks judged to be the singer (4 s average), and
    /// such blocks counted.
    pe_voice: f64,
    voice_blocks: u32,
    trust: [Trust; TRUST_RING],
    /// Output power of each block in `trust` (the singer's level when the spectral look
    /// decides a block afterwards).
    trust_pe: Vec<f32>,
    /// Input index of the first sample of the newest block in `trust`, and blocks recorded.
    trust_last_start: i64,
    trust_count: u64,
    imp: Vec<f32>,
    /// Spectral look at the output (voice evidence that does not need the left-overs
    /// measured, and the left-over share during singing; see [`spectral`]).
    spec: Spectral,
    /// Mono aligned reference of the block being finished.
    blk_xm: Vec<f32>,
    /// Echo path gain tracker (see `path_update`): blocks per chunk, the current chunk's
    /// sums (input x estimate, estimate x estimate, input power and expected echo power
    /// over blocks where the expected echo stands above the noise, and those blocks), and
    /// the ring of finished chunks.
    pg_chunk_blocks: u32,
    pg_blocks: u32,
    pg_acc: [f64; 6],
    pg_acc_n: u32,
    pg_ring: [[f64; 6]; PG_CHUNKS],
    pg_ring_n: [u32; PG_CHUNKS],
    pg_len: usize,
    pg_pos: usize,
    /// Latest path gain and its standard error, when measured, and the same over the newest
    /// [`PG_FAST`] chunks.
    path: Option<(f64, f64)>,
    path_fast: Option<(f64, f64)>,
    /// Input time (s) until which the per-block comparison may not trust a block (see
    /// [`LOUDER_HOLD_S`]).
    louder_hold: f64,
    /// The path gain is so low that the input passes through untouched.
    bypass: bool,
    /// Input time (s) the filter last started.
    lock_t: f64,
    lost: u32,
    /// After a failed gain test: input time (s) until which the fresh detector may confirm
    /// the echo (see [`GRACE_S`]); removal is stopped meanwhile.
    grace_until: Option<f64>,
    /// Wandering delay: the mono reference power per block read [`WANDER_SPAN_S`] ahead of
    /// the filter's position (`wide_lead` samples), the block being filled, a ring of the
    /// last blocks, and how many of them span twice [`WANDER_SPAN_S`].
    wide_lead: f64,
    wide_blk: f32,
    wide_ring: Vec<f32>,
    wide_pos: usize,
    wide_blocks: usize,
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
            hint_long: 0.0,
            pd_music: 0.0,
            pe_music: 0.0,
            pe_voice: 0.0,
            voice_blocks: 0,
            trust: [Trust::Unknown; TRUST_RING],
            trust_pe: vec![0.0; TRUST_RING],
            trust_last_start: 0,
            trust_count: 0,
            imp: vec![0.0; p * b],
            spec: Spectral::new(rate, b),
            blk_xm: vec![0.0; b],
            pg_chunk_blocks: ((PG_CHUNK_S / blk_s).round() as u32).max(1),
            pg_blocks: 0,
            pg_acc: [0.0; 6],
            pg_acc_n: 0,
            pg_ring: [[0.0; 6]; PG_CHUNKS],
            pg_ring_n: [0; PG_CHUNKS],
            pg_len: 0,
            pg_pos: 0,
            path: None,
            path_fast: None,
            louder_hold: 0.0,
            bypass: false,
            lock_t: 0.0,
            lost: 0,
            grace_until: None,
            wide_lead: (WANDER_SPAN_S * f64::from(rate)).round(),
            wide_blk: 0.0,
            wide_ring: vec![0.0; ((2.0 * WANDER_SPAN_S / blk_s).ceil() as usize + 1).max(1)],
            wide_pos: 0,
            wide_blocks: ((2.0 * WANDER_SPAN_S / blk_s).ceil() as usize + 1).max(1),
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
        self.hint_long = 0.0;
        self.pd_music = 0.0;
        self.pe_music = 0.0;
        self.pe_voice = 0.0;
        self.voice_blocks = 0;
        self.trust = [Trust::Unknown; TRUST_RING];
        self.trust_pe.fill(0.0);
        self.trust_last_start = 0;
        self.trust_count = 0;
        self.spec.reset(true);
        self.clear_path();
        self.lock_t = 0.0;
        self.lost = 0;
        self.grace_until = None;
        self.wide_blk = 0.0;
        self.wide_ring.fill(0.0);
        self.wide_pos = 0;
    }

    /// Forget the echo path gain tracker (a new filter).
    fn clear_path(&mut self) {
        self.pg_blocks = 0;
        self.pg_acc = [0.0; 6];
        self.pg_acc_n = 0;
        self.pg_len = 0;
        self.pg_pos = 0;
        self.path = None;
        self.path_fast = None;
        self.bypass = false;
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
                if self.wandering {
                    // The reference ahead of the filter's position (nearest sample: only
                    // its power is used), for the gate's wider echo bound.
                    let m = self.line.mono_at((pos + self.wide_lead).round() as i64);
                    self.wide_blk += m * m;
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
        let mut st = if self.locked {
            // Hold the output filter while the input holds more than the echo expected (the
            // filter's estimate, or the detector's level while the filter is still poor): a
            // singer louder than about 6 dB under the returned music.
            // A path significantly louder than the filter learnt (see `block_trust`) counts
            // too, so the output filter can take the re-learnt weights.
            let louder = self.louder().map_or(1.0, |g| g * g);
            let expected = self.py_r.max(self.hint_r) * louder as f32;
            let quiet = !self.filt.adapted() || self.pd_r <= ECHO_ONLY * expected;
            Some(
                self.filt
                    .step(&self.blk_x, &self.blk_d, &mut self.out_buf, quiet),
            )
        } else {
            self.out_buf.copy_from_slice(&self.blk_d);
            None
        };
        if let Some(s) = st.as_mut() {
            let verdict = self.path_block(s);
            if verdict == PathVerdict::Gone {
                // The returned playback is gone (loopback switched off): stop removing at
                // once, the input passes through, and the verdict is "clean".
                self.echo_gone();
                self.out_buf.copy_from_slice(&self.blk_d);
                st = None;
            } else {
                if verdict == PathVerdict::Suspect {
                    self.suspend();
                }
                if self.bypass {
                    // The path gain fell under half (or removal is suspended): subtracting
                    // the estimate would add more music than it removes.
                    self.out_buf.copy_from_slice(&self.blk_d);
                    s.pe = s.pd;
                }
            }
        }
        self.wide_ring[self.wide_pos] = self.wide_blk;
        self.wide_pos = (self.wide_pos + 1) % self.wide_ring.len();
        self.wide_blk = 0.0;
        let start = self.n_in as i64 - self.b as i64 - self.in_delay as i64;
        self.ref_env = self.blk_ref.max(self.ref_env * self.ref_decay);
        self.blk_ref = 0.0;
        let t = self.block_trust(st);
        let slot = (self.trust_count as usize) % TRUST_RING;
        self.trust[slot] = t;
        self.trust_pe[slot] = st.map_or(0.0, |s| s.pe);
        self.trust_last_start = start;
        self.trust_count += 1;
        if let Some(s) = st {
            self.spectral_block(s.px > 1e-9 * self.b as f32);
        }
    }

    /// Feed the finished (locked) block to the spectral look; when a frame is analysed and
    /// finds the singer, the blocks around its centre that the per-block comparison could
    /// not trust become [`Trust::Singer`] (tier 2 of the gate, see [`spectral`]). A frame
    /// is centred about 21 ms before the newest block, so a verdict can change for that
    /// long after a block was recorded; a pitch tracker's event for a time arrives later
    /// than that (the tracker needs half its 43 ms window after it, plus its smoothing).
    fn spectral_block(&mut self, music: bool) {
        let b = self.b;
        for i in 0..b {
            self.blk_xm[i] = if self.ch == 2 {
                0.5 * (self.blk_x[i] + self.blk_x[b + i])
            } else {
                self.blk_x[i]
            };
        }
        let gain = self.det.state().level.unwrap_or(0.0) as f32;
        let Some(f) = self.spec.push(
            &self.out_buf,
            self.filt.fore_estimate(),
            &self.blk_xm,
            gain,
            self.wandering,
            music,
        ) else {
            return;
        };
        // The background filter does not learn while the output is the singer (nor while
        // removal is suspended).
        self.filt.set_frozen(f.voice || self.grace_until.is_some());
        // A wandering delay (Bluetooth) misaligns the reference with the echo by up to the
        // size of a jump, so the bound per bin does not hold: measured, 1.4 % of the frames
        // with nobody singing would have been trusted (0.3 % without the spectral look).
        // There the gate keeps assuming nothing was removed, and the app warns.
        if !f.voice || self.wandering {
            return;
        }
        // Blocks j back from the newest (0) whose centre lies within half a hop of the
        // frame's centre: block j's centre is (j + 1/2) blocks before the newest block's
        // end, the frame's centre `o` blocks before it.
        let o = self.spec.len() / (2 * b);
        let hop = self.spec.hop_blocks();
        let al = self.a_long;
        for j in o.saturating_sub(hop / 2)..o + hop / 2 {
            if j as u64 >= self.trust_count || j >= TRUST_RING {
                continue;
            }
            let idx = ((self.trust_count - 1 - j as u64) as usize) % TRUST_RING;
            if matches!(self.trust[idx], Trust::Music | Trust::Unknown) {
                self.trust[idx] = Trust::Singer;
                let pe = f64::from(self.trust_pe[idx]);
                if self.voice_blocks == 0 {
                    self.pe_voice = pe;
                } else {
                    self.pe_voice += al * (pe - self.pe_voice);
                }
                self.voice_blocks = self.voice_blocks.saturating_add(1);
            }
        }
    }

    /// Echo path gain tracker, once per locked block. In chunks of about 21 ms it sums the
    /// input times the output filter's echo estimate and the estimate squared; over the last
    /// half second their ratio is the gain of the echo path relative to what the filter
    /// learnt (1 while the return is unchanged, 0 once it is gone; a singer adds noise but
    /// no bias, and the spread of the chunks gives its standard error). The detector cannot
    /// say this quickly: its correlation is normalised, so it forgets a vanished echo only
    /// as fast as new input outweighs it. Also sums the input's power against the echo the
    /// filter (or, before it has learnt, the detector) expects, which says "gone" when
    /// nobody sings even after the filter's divergence guard has reset it.
    ///
    /// Sets the bypass and says whether the returned playback looks gone.
    fn path_block(&mut self, st: &BlockStats) -> PathVerdict {
        let b = self.b;
        if st.px <= 1e-9 * b as f32 || self.grace_until.is_some() {
            // Silent music: no evidence either way, nothing ages. Suspended: the detector
            // decides.
            return PathVerdict::Keep;
        }
        let adapted = self.filt.adapted();
        let y = self.filt.fore_estimate();
        let (mut dy, mut yy) = (0.0f64, 0.0f64);
        for i in 0..b {
            dy += f64::from(self.blk_d[i]) * f64::from(y[i]);
            yy += f64::from(y[i]) * f64::from(y[i]);
        }
        if adapted && !self.wandering {
            self.pg_acc[0] += dy;
            self.pg_acc[1] += yy;
            self.pg_acc[4] += f64::from(st.pd);
        }
        // The echo expected in the input: the filter's estimate once it has learnt the path
        // (exact in band), and the detector's level times the reference; the power test
        // takes the smaller of the two sums, so neither a filter that went wrong (its
        // estimate too large) nor a reference with much energy outside the detector's band
        // can make it say "gone".
        let hint = self.det.state().level.unwrap_or(0.0) * f64::from(st.px) / self.ch as f64;
        let expected = if adapted && yy > 0.0 { yy } else { hint };
        let noise = f64::from(self.nf_cur.min(self.nf_prev));
        if noise < 1e30 && expected.min(hint) > 10.0 * noise {
            self.pg_acc[2] += f64::from(st.pd);
            self.pg_acc[3] += expected;
            self.pg_acc[5] += hint;
            self.pg_acc_n += 1;
        }
        self.pg_blocks += 1;
        if self.pg_blocks < self.pg_chunk_blocks {
            return PathVerdict::Keep;
        }
        self.pg_ring[self.pg_pos] = self.pg_acc;
        self.pg_ring_n[self.pg_pos] = self.pg_acc_n;
        self.pg_pos = (self.pg_pos + 1) % PG_CHUNKS;
        self.pg_len = (self.pg_len + 1).min(PG_CHUNKS);
        self.pg_blocks = 0;
        self.pg_acc = [0.0; 6];
        self.pg_acc_n = 0;
        self.path_update()
    }

    /// A chunk finished: the path gain, its standard error, the bypass and the verdict
    /// (see [`EchoCanceller::path_block`]).
    fn path_update(&mut self) -> PathVerdict {
        let ring = &self.pg_ring[..self.pg_len];
        let (mut sdy, mut syy, mut spd_a, mut n) = (0.0f64, 0.0f64, 0.0f64, 0usize);
        for c in ring.iter().filter(|c| c[1] > 0.0) {
            sdy += c[0];
            syy += c[1];
            spd_a += c[4];
            n += 1;
        }
        self.path = None;
        // Only while the echo estimate is a fair part of the input (at most 6 dB under it).
        // Under a much louder singer the gain is not measurable: a voice holding a note near
        // a partial of the music correlates with it for longer than the chunks, so their
        // spread understates the error (measured: a high "ee" 26 dB over a -30 dB return
        // was taken for "gone" twice in 45 s). A return that weak cannot move the marker,
        // and the singer's next pause settles it.
        if n >= PG_CHUNKS / 2 && syy > 1e-20 && syy >= 0.25 * spd_a {
            let g = sdy / syy;
            let ss: f64 = ring
                .iter()
                .filter(|c| c[1] > 0.0)
                .map(|c| (c[0] - g * c[1]).powi(2))
                .sum();
            let se = (ss / (syy * syy) * n as f64 / (n - 1) as f64).sqrt();
            self.path = Some((g, se));
        }
        // The fast gain over the newest chunks (same rule for when it is measurable).
        self.path_fast = None;
        if self.pg_len >= PG_FAST {
            let (mut fdy, mut fyy, mut fpd) = (0.0f64, 0.0f64, 0.0f64);
            let mut cs = [[0.0f64; 2]; PG_FAST];
            for (k, c2) in cs.iter_mut().enumerate() {
                let c = &self.pg_ring[(self.pg_pos + PG_CHUNKS - 1 - k) % PG_CHUNKS];
                fdy += c[0];
                fyy += c[1];
                fpd += c[4];
                *c2 = [c[0], c[1]];
            }
            if cs.iter().all(|c| c[1] > 0.0) && fyy > 1e-20 && fyy >= 0.25 * fpd {
                let g = fdy / fyy;
                let ss: f64 = cs.iter().map(|c| (c[0] - g * c[1]).powi(2)).sum();
                let se = (ss / (fyy * fyy) * PG_FAST as f64 / (PG_FAST - 1) as f64).sqrt();
                self.path_fast = Some((g, se));
            }
        }
        if self.louder().is_some() {
            self.louder_hold = self.now_s() + LOUDER_HOLD_S;
        }
        self.bypass = self
            .path
            .is_some_and(|(g, se)| g + 2.0 * se < BYPASS_GAIN);
        let gone_gain = n + 2 >= PG_CHUNKS
            && self
                .path
                .is_some_and(|(g, se)| g + 3.0 * se < GONE_GAIN);
        let (spd, sexp) = ring
            .iter()
            .fold((0.0, 0.0), |a, c| (a.0 + c[2], a.1 + c[3].min(c[5])));
        let counted: u32 = self.pg_ring_n[..self.pg_len].iter().sum();
        let blk_s = self.b as f64 / f64::from(self.rate);
        // Gone: far under the echo expected, or 10 dB under it and down at the noise floor
        // (a weak return switched off leaves the floor, which is not 25 dB under it; a loud
        // return turned down 20 dB is still far above the floor).
        let floor = f64::from(self.nf_cur.min(self.nf_prev));
        let at_floor = floor < 1e30 && spd < 4.0 * floor * f64::from(counted);
        let gone_power = f64::from(counted) * blk_s >= 0.25
            && (spd < GONE_POWER * sexp || (spd < 0.1 * sexp && at_floor));
        if gone_power && self.now_s() - self.lock_t >= GONE_POWER_AFTER_S {
            PathVerdict::Gone
        } else if gone_gain || gone_power {
            PathVerdict::Suspect
        } else {
            PathVerdict::Keep
        }
    }

    /// The echo path's gain when it is significantly louder than what the filter learnt
    /// (the half-second gain 2 standard errors over 1, or the fast gain 3 standard errors
    /// over 1.4: four chunks give a rough error, and a singer makes them noisy), else
    /// `None`.
    fn louder(&self) -> Option<f64> {
        let slow = self.path.filter(|&(g, se)| g - 2.0 * se > 1.0).map(|p| p.0);
        let fast = self.path_fast.filter(|&(g, se)| g - 3.0 * se > 1.4).map(|p| p.0);
        match (slow, fast) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
        .map(|g| g.min(4.0))
    }

    /// The gain test failed: stop removing (the input passes through, the background does
    /// not learn) and let a fresh detector confirm the echo within [`GRACE_S`]
    /// (`on_hop`). Meanwhile the gate expects the whole echo the detector measured
    /// before, and the spectral look still recognises a singer above it.
    fn suspend(&mut self) {
        self.det.forget_echo(true);
        self.filt.reset(false);
        self.filt.set_frozen(true);
        self.clear_fhist();
        self.clear_ratio();
        self.clear_path();
        self.bypass = true;
        self.grace_until = Some(self.now_s() + GRACE_S);
    }

    /// The returned playback is gone: count it, forget the echo (the detector says
    /// "absent" at once and finds a return that comes back as at the start) and stop
    /// removing.
    fn echo_gone(&mut self) {
        self.lost += 1;
        self.grace_until = None;
        self.det.forget_echo(false);
        self.locked = false;
        self.filt.reset(true);
        self.drift = 0.0;
        self.tau = 0.0;
        self.off_count = 0;
        self.wandering = false;
        self.clear_fhist();
        self.clear_ratio();
        self.spec.reset(false);
        self.clear_path();
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
        let hint_blk = self.det.state().level.unwrap_or(0.0) * f64::from(st.px) / self.ch as f64;
        self.hint_long += al * (hint_blk - self.hint_long);
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
            && !self.bypass
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
        // The echo path gain tracker (see `path_block`): the share of the expected echo
        // still in the input (its upper bound), and a significant change of the path since
        // the filter learnt it, whose left-over is (1 - g)^2 of the estimate until the
        // filter follows.
        let (scale, change) = match (self.louder(), self.path) {
            // The path got significantly louder (the OS or mixer volume turned up): the
            // echo in the input is g^2 of what the filter learnt and the left-over (g-1)^2
            // of the estimate until the filter follows. Measured: without this, a return
            // turned up 6 dB showed 2.6 % pitch frames with nobody singing in the 5 s
            // after (left-overs as loud as the old echo passed for the singer until the
            // half-second gain caught up).
            (Some(g), _) => ((g * g) as f32, ((g - 1.0).powi(2)).min(1.0) as f32),
            (None, Some((g, se))) => (
                (g + 2.0 * se).clamp(0.0, 1.0).powi(2) as f32,
                (((1.0 - g).abs() - 2.0 * se).max(0.0)).powi(2).min(1.0) as f32,
            ),
            (None, None) => (1.0, 0.0),
        };
        let mut expected = scale * self.py_s.max(self.hint_s);
        // Wandering delay: the loudest the echo can be within the span it may have moved
        // to (the detector's level times the largest reference block power within
        // +-WANDER_SPAN_S of the filter's position).
        let wide = if self.wandering {
            let peak = self.wide_ring[..self.wide_blocks].iter().copied().fold(0.0f32, f32::max);
            let w = hint * peak;
            expected = expected.max(w);
            w
        } else {
            0.0
        };
        let residual = if self.bypass {
            // Nothing is subtracted: the output holds whatever echo is still there.
            expected
        } else {
            let mut residual = if let Some(ratio) = self.residual_ratio() {
                ratio.max(change) * self.py_s
            } else if let Some(share) = self.regression_share() {
                // No stretch of music without singing yet: the share of the echo left in
                // the output from the spectral look's regression, applied to the echo the
                // detector expects (it does not depend on what the filter has learnt).
                (change * self.py_s).max(share * self.hint_s)
            } else {
                // Not measured yet: only the spectral look (tier 2) can trust a block.
                return Trust::Unknown;
            };
            if self.wandering {
                // The delay wanders (Bluetooth): what removal leaves cannot be measured, so
                // the gate assumes nothing was removed, with [`WANDER_BIAS`] more for the
                // detector's level, which a wandering delay biases low (measured up to
                // 5.5 dB). A singer clearly louder than the returned music still counts;
                // left-over music should not.
                residual = residual.max(WANDER_BIAS * self.hint_s.max(wide));
            }
            residual
        };
        // An input fully explained by the echo expected (the filter's estimate, or the
        // detector's level) holds nobody louder than about 6 dB under the music: music, even
        // if the output looks louder than the left-overs measured so far. This covers the
        // moments the left-overs jump before the measurement follows (an instrument entering
        // in a band the filter has not learnt yet). A singer quieter than the returned music
        // is recognised by the spectral look instead (tier 2), which compares the output
        // with the most music there can be in each frequency bin.
        let explained = self.pd_s <= ECHO_ONLY * expected;
        let voice = !explained
            && self.pe_s > self.gate_k * residual.max(noise) + 1e-12
            && self.now_s() >= self.louder_hold;
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
    /// enough echo-only blocks since the filter (re)started; else the spectral look's
    /// regression (continuous singing leaves no echo-only stretch), as a share of the echo.
    fn mean_residual_ratio(&self) -> Option<f32> {
        if self.ratio_blocks >= self.ratio_min_blocks {
            return Some(self.ratio_mean);
        }
        self.regression_share()
    }

    /// Share of the echo the detector expects that the output filter's estimate holds
    /// (4 s averages; 0..1). Near 1 once the filter has learnt the path; low while a filter
    /// held during continuous singing has learnt little.
    fn captured(&self) -> f64 {
        if self.hint_long > 1e-30 {
            (self.py_long / self.hint_long).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Expected left-over echo as a share of the echo estimate (with its margin), once
    /// measured on enough echo-only blocks since the filter (re)started.
    fn residual_ratio(&self) -> Option<f32> {
        (self.ratio_blocks >= self.ratio_min_blocks).then(|| (self.ratio_avg * RATIO_BIAS).min(1.0))
    }

    /// Share of the echo (the echo the detector expects, not the filter's estimate) left in
    /// the output, from the spectral look's regression of the output's power fluctuations
    /// on the reference's ([`spectral`]), with [`REGRESSION_BIAS`]. Bands the filter has not
    /// learnt count whole. Used until the left-overs can be measured on music without
    /// singing.
    fn regression_share(&self) -> Option<f32> {
        if !self.locked || self.grace_until.is_some() {
            return None;
        }
        let g = self.det.state().level? as f32;
        if g <= 0.0 {
            return None;
        }
        self.spec
            .leak_slope()
            .map(|s| (s / g * REGRESSION_BIAS).clamp(1e-6, 1.0))
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
        if let Some(until) = self.grace_until {
            // Removal suspended after a failed gain test: the fresh detector found the echo
            // again (the filter had gone wrong: restart it where the echo is), or did not
            // in time (gone).
            if ds.presence == Presence::Present && !self.det.probation() {
                let d = ds.delay_s.unwrap_or(self.lock_delay_s + self.tau / fs);
                self.lock(d);
            } else if self.now_s() >= until {
                self.echo_gone();
            }
            return;
        }
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
                    && self
                        .residual_ratio()
                        .or(self.regression_share())
                        .is_some_and(|r| r < 0.3)
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
        self.spec.reset(false);
        self.clear_path();
        self.grace_until = None;
        self.lock_t = self.now_s();
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
        let level = if adapted && self.px_long > 1e-30 && self.captured() >= 0.5 {
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
            lost: self.lost,
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

    /// One line of internal state for benchmark traces (allocates; never on a real-time
    /// path).
    #[doc(hidden)]
    pub fn debug_line(&self) -> String {
        format!(
            "ratio_blocks {} ratio_avg {:.4} ratio_mean {:.4} regression {:?} captured {:.3} path {:?} bypass {} upd {} voice_blocks {}",
            self.ratio_blocks,
            self.ratio_avg,
            self.ratio_mean,
            self.regression_share(),
            self.captured(),
            self.path.map(|(g, se)| ((g * 1000.0).round() / 1000.0, (se * 1000.0).round() / 1000.0)),
            self.bypass,
            self.filt.updates(),
            self.voice_blocks
        )
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

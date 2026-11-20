// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming pitch tracker for the singer's microphone (WORK_ORDER-2 4.4 and 6, "Live tracker").
//!
//! The analysis is the McLeod Pitch Method (P. McLeod and G. Wyvill, "A smarter way to find
//! pitch", ICMC 2005), written directly on `realfft`. Which of its candidate periods is the
//! pitch is decided over time, on a periodicity measure that tells octaves apart better
//! than the NSDF does on real voices (WO2 Stage 0 verifier, DECISIONS D-238):
//!
//! 1. The input (one mono channel, chosen by the caller: channels are never summed) goes
//!    through a 4th-order Butterworth high-pass at 55 Hz (below C2 = 65.4 Hz), which takes
//!    the bulk of handling thumps, popped consonants, DC and 50 Hz hum away, and a
//!    4th-order Butterworth low-pass at 3.5 kHz, so a bright mixer EQ, sibilance and the air
//!    band do not lower the clarity of a sung vowel, and the same band is analysed at every
//!    sample rate.
//! 2. Every `hop` (10 ms) the last `window` of filtered samples (42.7 ms: 2048 samples at
//!    48 kHz, sized in *time*, so 16, 44.1, 48 and 96 kHz give the same answer) is analysed:
//!    the normalised square difference function (NSDF) `n(t) = 2 r(t) / m(t)`, with `r` the
//!    autocorrelation (one forward and one inverse real FFT of the zero-padded window) and
//!    `m(t) = sum x[j]^2 + x[j+t]^2` (updated incrementally per lag). A second NSDF is
//!    computed from the spectrum less an estimate of its late reverberation (the same bin
//!    50 ms earlier, decayed as a room with RT60 1.5 s would decay it, over-subtracted;
//!    `derev_rt60_s`): it only ever raises a candidate's clarity, so a new note is seen
//!    through the tail of the one before it, while pitch and octave come from the plain one.
//! 3. Candidates: the "key maximum" (highest local maximum) of every positive region of the
//!    NSDF after the zero-lag lobe reaching half the highest, refined by parabolic
//!    interpolation; at most six per frame. Each is scored by the periodicity of the
//!    *flattened* spectrum at its period (the autocorrelation of `|X|^0.5` up to 5 kHz, see
//!    `periodicity_at`), less `octave_cost` per octave it lies below the paper's
//!    own choice (the first key maximum reaching `peak_threshold` = 0.9 times the highest).
//!    A candidate with another at two or three times its pitch also needs peaks at its own
//!    harmonics (its odd ones for an octave), or it loses `odd_cost`: breath through a
//!    narrow formant fills a high note's subharmonic with noise but puts no peaks there.
//! 4. Path search: a Viterbi search over the candidates of the last frames. A step down
//!    between frames costs `jump_cost` per octave, a step up `jump_up_cost`. The frame
//!    `lookahead_frames` (2) behind the newest is the one decided and reported. While a
//!    note is shown the path is pinned to what was shown, so the marker cannot flip octave
//!    between two decisions, and a held note keeps its octave through stretches where a
//!    reverb tail, a second voice or a rough voice makes the octave below look more
//!    periodic. A note starts on the octave of the newest window (see `anchored`).
//! 5. Singing / not singing, on the decided frame: a level gate with hysteresis (-55 / -60
//!    dBFS) and clarity (the NSDF value at the chosen period) with hysteresis. A note starts
//!    either confidently (`clarity_on` = 0.93, the next frame continuing it at
//!    `clarity_off`) or steadily (`steady_frames` = 5 frames, from 20 ms before the decided
//!    one to the newest, each at least `clarity_steady` = 0.8, within `steady_step_cents`
//!    of the one before and above `steady_min_hz`: a real voice whose clarity stays under
//!    0.93 holds a steady pitch; breath, whispers and thumps do not; every frame must also
//!    have a harmonic series, `steady_harmonic_min`, which a whispered vowel lacks). Clear
//!    onsets may start early (`early_start`): when the newer frames already hold the
//!    confident evidence, the decided frame needs only `start_floor`, which shows a note up
//!    to two hops sooner. It keeps going at `clarity_off` = 0.75 while the pitch continues
//!    the line (`max_step_cents`). A decaying room tail (the level 15 dB under the recent
//!    peak and falling every frame, `tail_drop_db`) ends a note and starts none.
//! 6. A three-point median of the decided frame, the frame reported before it and the one
//!    after it on the path smooths the reported pitch.
//!
//! Output: one [`PitchEvent`] per hop (time in input samples, Hz or `None`, clarity). After
//! [`Tracker::new`] nothing is allocated: the FFT plans, buffers, candidate history and
//! scratch are owned by the tracker and events are handed to a caller closure.
//!
//! Delay (measured in docs/benchmarks/practice-tracker.md): an event is ready
//! `lookahead_frames` hops (20 ms) after the end of the window it describes. From a note's
//! onset that is the part of the window the new note must fill before its clarity passes
//! the start threshold (about 20 ms in the middle of the range, the whole 42.7 ms window for
//! the lowest notes), plus the wait for the next hop (0..10 ms), plus the look-ahead.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub mod rig;
pub mod testvoice;

#[cfg(test)]
mod tests;

/// Tracker settings. Every duration is in seconds and every frequency in Hz, so the same
/// settings mean the same thing at any sample rate.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TrackerConfig {
    /// Length of the analysis window. 2048 samples at 48 kHz: long enough that the NSDF of
    /// a C2 (period 15.3 ms) is computed over 1.8 periods (WO2 trap 5).
    pub window_s: f64,
    /// Time between two estimates.
    pub hop_s: f64,
    /// Lowest pitch reported. 62 Hz: just below C2 (65.4 Hz) and above 60 Hz mains hum.
    pub min_hz: f64,
    /// Highest pitch reported. 1400 Hz: a little above C6 (1046.5 Hz) plus vibrato.
    pub max_hz: f64,
    /// MPM's relative peak threshold k: the first key maximum reaching k times the highest
    /// key maximum is the paper's choice of period (the reference for `octave_cost`).
    pub peak_threshold: f32,
    /// Clarity needed to start reporting a pitch confidently. 0.93: the highest clarity
    /// measured on generated breath, whisper, consonants, room noise, thumps and pops was
    /// 0.913, and a held voice reaches it within about 25 ms
    /// (docs/benchmarks/practice-tracker.md).
    pub clarity_on: f32,
    /// Clarity needed to keep reporting a pitch once started (hysteresis), as long as the
    /// pitch continues the line (`max_step_cents`). Keeps breathy and vibrato notes whole.
    pub clarity_off: f32,
    /// Window RMS (after the high-pass), in dBFS, needed to start reporting a pitch.
    pub level_on_dbfs: f32,
    /// Window RMS needed to keep reporting a pitch once started.
    pub level_off_dbfs: f32,
    /// Corner of the 4th-order Butterworth high-pass applied before analysis.
    pub highpass_hz: f64,
    /// Corner of the 4th-order Butterworth low-pass applied before analysis (0 = none). At
    /// 4 kHz a C6 keeps three harmonics; above, a voice is mostly breath and sibilance.
    pub lowpass_hz: f64,
    /// Frames in a row (the decided one and the look-ahead frames after it) needed for a
    /// confident start: the first at `clarity_on`, the others at `clarity_off`, each
    /// continuing the previous one's pitch (minimum duration). At most
    /// `lookahead_frames + 1`.
    pub min_voiced_frames: u32,
    /// Largest pitch change between two analysis frames (10 ms apart) that counts as the
    /// same sung line: needed for a confident start, and for a frame below `clarity_on` to
    /// keep a note going. 150 cents is several times the fastest vibrato (+-50 cents at 6 Hz
    /// moves at most 19 cents per 10 ms) and glide (an octave in 0.1 s is 120).
    pub max_step_cents: f32,
    /// Candidate key maxima below this share of the frame's highest one are not considered.
    pub candidate_floor: f32,
    /// Score lost by a candidate per octave its period lies above the paper's choice: how
    /// much more periodic a lower octave must look to be preferred within a frame.
    pub octave_cost: f32,
    /// Exponent applied to the magnitude spectrum before the autocorrelation that scores
    /// the candidates (2 = the plain autocorrelation, 1 = magnitude, smaller = flatter;
    /// see `periodicity_at`). 0 scores candidates by their NSDF value instead.
    pub flat_power: f32,
    /// Highest frequency in the flattened spectrum: the same band at every sample rate
    /// (below the 8 kHz Nyquist frequency of a 16 kHz input).
    pub flat_max_hz: f64,
    /// Score lost by the path per octave of pitch change DOWN between two frames
    /// (proportional to the interval; steps within 50 cents are free). With the path pinned
    /// to what was shown, a held note goes down an octave only when the octave below
    /// scores better by more than about 0.07 on average over the decided frame and the two
    /// look-ahead frames: a reverb tail, a second voice or a rough stretch rarely manages
    /// that for long, while after a real leap down the new octave scores 0.1 to 0.3 better
    /// and is followed within 80 ms (generated legato leaps, every vowel). Larger values hold
    /// the marker an octave high after a leap down to a vowel whose second harmonic is
    /// strong ("oo" from D4 to D3 was never followed at 0.4).
    pub jump_cost: f32,
    /// The same per octave of pitch change UP. Small: after an upward leap the old, lower
    /// octave stays almost as periodic (it is a period of the new note too), so a large
    /// cost would hold the marker an octave low after every leap.
    pub jump_up_cost: f32,
    /// Frames between the newest analysis and the frame decided (and reported): the path
    /// search looks this far ahead. 1..=`MAX_LOOKAHEAD`; each frame adds `hop_s` of delay.
    pub lookahead_frames: u32,
    /// Clarity every frame of a steady start must reach (see the module documentation).
    /// 1.0 or more turns steady starts off.
    pub clarity_steady: f32,
    /// Frames a steady start needs, counted back from the newest: at least
    /// `lookahead_frames + 1` (the decided frame and the look-ahead); more also asks the
    /// frames before the decided one to be steady (a longer stretch, no added delay). At
    /// most `MAX_LOOKAHEAD + 2`.
    pub steady_frames: u32,
    /// Largest pitch change between consecutive frames of a steady start.
    pub steady_step_cents: f32,
    /// Steady starts only above this pitch. Handling thumps and popped consonants live
    /// below about 160 Hz; there a note must start confidently, which needs a longer
    /// periodic stretch (WO2 4.10: they must never show as notes).
    pub steady_min_hz: f64,
    /// Early start (WO2 tracker robustness, onset delay): the evidence for a confident start may come
    /// from the newer frames of the look-ahead. Two adjacent frames anywhere from the decided
    /// frame to the newest, one at `clarity_on` and the other at `clarity_off`, on one line;
    /// the frames from that pair back to the decided frame continuing the line at
    /// `start_floor`. A clear onset is then shown as soon as the newest windows hold it,
    /// not when the decided (older) window does: up to `lookahead_frames` hops sooner, with
    /// the same amount of evidence as the plain confident start. Not at pitches below
    /// `steady_min_hz` (handling thumps), not while the newest window's level still rises
    /// by more than `early_rise_db` (the attack still fills the windows) and not when a
    /// frame holds a fair candidate an octave above its own (`early_octave_veto`): the first
    /// windows of a high "ee" / "oo" can look an octave low for 30 ms. Measured on the
    /// development set: onsets shown within 100 ms 84.8 -> 88.2 %; without the last two
    /// conditions generated whole notes had 32-42 octave-off onset events per rate instead
    /// of 3-9. Off: the plain rule only.
    pub early_start: bool,
    /// Clarity the decided frame (and any frame between it and the confirming pair) needs
    /// in an early start. Its window holds the start of the note and some of what came
    /// before, so it is less periodic than the confirming frames.
    pub start_floor: f32,
    /// See `early_start`: a frame with a candidate an octave above its chosen one at
    /// this clarity or more does not confirm an early start (2 = never vetoed).
    pub early_octave_veto: f32,
    /// See `early_start`: the most the newest frame's level may lie above the frame
    /// before it (dB) for an early start; a larger rise means the note's attack is still
    /// filling the windows.
    pub early_rise_db: f32,
    /// Reverb tail (WO2 4.10 "Reverb must not upset the tracker"): a frame is a decaying tail
    /// when its level is at least this far below the recent peak level and the level has
    /// fallen by at least `tail_slope_db` between each of the last `tail_frames` frames. A
    /// tail ends a shown note and cannot start one; after a tail, a note starts only after a
    /// new onset (the level `tail_rise_db` above the lowest level of the tail), a level that
    /// stopped falling, or a clear pitch away from the last note. 15 dB / 4 frames, measured
    /// on the development set: reverb false alarms 11.5 -> 7.6 % (RT60 0.8 s) and 18.0 ->
    /// 13.9 % (1.5 s), 1.58 -> 1.03 % on clean input, for 1.2 points of raw pitch accuracy
    /// on clean input (the ends of notes on processed stems, where the reference calls the
    /// stem's own reverb voiced). 0 turns the rule off.
    pub tail_drop_db: f32,
    /// Smallest fall in level per frame (10 ms) through the look-ahead frames that counts as
    /// a decaying tail. A room with RT60 1.5 s decays by 0.4 dB per 10 ms.
    pub tail_slope_db: f32,
    /// Fall of the recent peak level per frame (its release), dB.
    pub peak_release_db: f32,
    /// Rise above the lowest level of a tail that counts as a new onset, dB.
    pub tail_rise_db: f32,
    /// Consecutive falling steps between frames, counted back from the newest, that a
    /// tail needs (at least `lookahead_frames`).
    pub tail_frames: u32,
    /// Harmonic evidence for subharmonics (WO2 tracker robustness, breathy voices): a
    /// candidate with another candidate at two or three times its pitch must show peaks at
    /// its own harmonics that the higher one does not have (its odd harmonics for an
    /// octave), at least `odd_ratio_min` times the power halfway between them, or its score
    /// loses `odd_cost`. Breath through a narrow first formant fills the band around a
    /// high note's subharmonic with noise but puts no peaks at its odd harmonics; a real
    /// low voice with weak odd harmonics (15 dB down) still has them as peaks over the
    /// valleys. 0 turns the check off.
    pub odd_cost: f32,
    /// See `odd_cost`: the least ratio of power at the subharmonic's own harmonics to the
    /// power halfway between them.
    pub odd_ratio_min: f32,
    /// Smallest flattened-spectrum periodicity (see `periodicity_at`) of every
    /// frame of a steady start. A sung vowel has many harmonics and scores high; noise
    /// through a narrow formant (a whispered vowel) is nearly periodic over one window in
    /// the NSDF but has no harmonic series. 0 turns the check off.
    pub steady_harmonic_min: f32,
    /// Late-reverberation suppression ("dereverberation lite", WO2 4.10): before the
    /// autocorrelation, every spectrum bin below `flat_max_hz` loses an estimate of the late
    /// reverberation, the power the same bin had `derev_delay_frames` frames earlier decayed
    /// as a room with this RT60 would decay it (Lebart et al. 2001), times `derev_over`, the
    /// bin keeping at least `derev_floor` of its power. On a steady dry voice every
    /// harmonic loses the same share, which leaves the NSDF unchanged; the tail of the
    /// previous note in the window of a new one is removed. The suppressed spectrum only
    /// lifts clarity: the candidates, their pitch and their octave scores come from the
    /// plain spectrum, and each candidate's clarity (`Cand::n`) is the higher of the plain
    /// NSDF and the NSDF of the suppressed spectrum at its period (one more inverse FFT per
    /// frame). Feeding the suppressed spectrum to the pitch and candidates too cost 6
    /// points of raw pitch accuracy on the development set ("musical noise"). 0 turns it
    /// off.
    pub derev_rt60_s: f32,
    /// Over-subtraction factor of the late-reverberation estimate.
    pub derev_over: f32,
    /// Least share of a bin's power kept by the suppression.
    pub derev_floor: f32,
    /// Frames between the window that gives the estimate and the window it is taken from
    /// (5 = 50 ms: past the early reflections, and the two windows do not overlap).
    pub derev_delay_frames: u32,
    /// Candidates below this pitch keep their plain clarity. Most pitched
    /// sound in a backing track that is not a voice is the bass line, and the suppression,
    /// which isolates what is new over what is sustained, lifts it the most.
    pub derev_min_hz: f64,
    /// Candidates whose flattened-spectrum periodicity is below this keep their plain
    /// clarity: spectral subtraction leaves random isolated peaks in noise ("musical
    /// noise"), which can make a whispered vowel look periodic.
    pub derev_per_min: f32,
}

/// Most candidate periods kept per analysis frame.
const MAX_CANDS: usize = 6;
/// Most frames of look-ahead (`TrackerConfig::lookahead_frames`).
pub const MAX_LOOKAHEAD: usize = 3;
/// Analysis frames kept for the path search and the steady-start rule: the look-ahead
/// frames, the decided frame and the frames before it.
const RING: usize = MAX_LOOKAHEAD + 2;
/// "The path starts here" in `Slot::back`.
const NO_BACK: u8 = u8::MAX;

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            window_s: 2048.0 / 48_000.0,
            hop_s: 0.010,
            min_hz: 62.0,
            max_hz: 1400.0,
            peak_threshold: 0.9,
            clarity_on: 0.93,
            clarity_off: 0.75,
            level_on_dbfs: -55.0,
            level_off_dbfs: -60.0,
            highpass_hz: 55.0,
            lowpass_hz: 3500.0,
            min_voiced_frames: 2,
            max_step_cents: 150.0,
            candidate_floor: 0.5,
            octave_cost: 0.06,
            flat_power: 0.5,
            flat_max_hz: 5000.0,
            jump_cost: 0.2,
            jump_up_cost: 0.1,
            lookahead_frames: 2,
            clarity_steady: 0.8,
            steady_frames: 5,
            steady_step_cents: 40.0,
            steady_min_hz: 160.0,
            early_start: true,
            start_floor: 0.75,
            early_octave_veto: 0.4,
            early_rise_db: 1.0,
            tail_drop_db: 15.0,
            tail_slope_db: 0.3,
            peak_release_db: 0.1,
            tail_rise_db: 3.0,
            tail_frames: 4,
            derev_rt60_s: 1.5,
            derev_over: 2.0,
            derev_floor: 0.1,
            derev_delay_frames: 5,
            derev_min_hz: 130.0,
            derev_per_min: 0.13,
            steady_harmonic_min: 0.09,
            odd_cost: 0.3,
            odd_ratio_min: 2.0,
        }
    }
}

/// One estimate, produced every hop.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PitchEvent {
    /// The moment this estimate describes, in input samples counted from the first sample
    /// given to the tracker (at the input's own rate): the centre of the decided analysis
    /// window. Compare this, not `ready_at`, with the song: it is where the sound was sung.
    /// Negative for the first few events of a stream (their windows reach back before the
    /// first sample).
    pub time: i64,
    /// The input sample count at which this event became available: the end of the newest
    /// analysis window. `ready_at - time` is constant (half a window plus the look-ahead).
    pub ready_at: u64,
    /// The pitch sung, or `None` when the input is not a sung (pitched) sound.
    pub hz: Option<f32>,
    /// MPM clarity (NSDF value, 0..1, 1 = perfectly periodic) of the period chosen for the
    /// described frame, or of its most periodic candidate when none was chosen. Given also
    /// when `hz` is `None`, for diagnostics.
    pub clarity: f32,
    /// RMS level of the described analysis window after the high-pass, in dBFS (-200 for
    /// digital silence).
    pub level_dbfs: f32,
}

/// One candidate period of an analysis frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cand {
    hz: f32,
    /// Interpolated NSDF value at the period (its clarity).
    n: f32,
    /// Periodicity less the octave cost (see `TrackerConfig::octave_cost`).
    score: f32,
    /// Flattened-spectrum periodicity at the period (the NSDF value when that is off).
    per: f32,
    /// The plain NSDF value (`n` without the late-reverberation suppression, see
    /// `TrackerConfig::derev_rt60_s`).
    n0: f32,
    /// Lies half an octave or more below the paper's choice of period (MPM's own pick).
    below: bool,
}

/// One analysis frame in the path-search history.
#[derive(Clone, Copy, Debug)]
struct Slot {
    cands: [Cand; MAX_CANDS],
    /// Candidates in use; 0 = nothing pitched here (silence, too quiet, no peak): a path
    /// cannot pass through this frame.
    len: usize,
    /// Mean square of the window (linear power).
    power: f64,
    /// The same in dB (-200 for digital silence).
    level_db: f32,
    /// Highest key maximum of the NSDF (0 when none).
    clarity: f32,
    /// Best path score ending at each candidate, relative to the best one (0).
    acc: [f32; MAX_CANDS],
    /// The previous frame's candidate on that best path, or `NO_BACK`.
    back: [u8; MAX_CANDS],
}

impl Slot {
    const EMPTY: Slot = Slot {
        cands: [Cand {
            hz: 0.0,
            n: 0.0,
            score: 0.0,
            per: 0.0,
            n0: 0.0,
            below: false,
        }; MAX_CANDS],
        len: 0,
        power: 0.0,
        level_db: -200.0,
        clarity: 0.0,
        acc: [0.0; MAX_CANDS],
        back: [NO_BACK; MAX_CANDS],
    };

    /// Index of the candidate with the best path score (the slot must have candidates).
    fn best(&self) -> usize {
        let mut b = 0;
        for i in 1..self.len {
            if self.acc[i] > self.acc[b] {
                b = i;
            }
        }
        b
    }

    /// The candidate with the highest clarity, if any.
    fn clearest(&self) -> Option<Cand> {
        self.cands[..self.len]
            .iter()
            .copied()
            .max_by(|a, b| a.n.total_cmp(&b.n))
    }
}

/// Streaming pitch tracker for one mono channel. See the module documentation.
pub struct Tracker {
    cfg: TrackerConfig,
    rate: u32,
    /// Analysis window length in samples.
    win: usize,
    /// Samples between two analyses.
    hop: usize,
    /// `lookahead_frames`, checked.
    look: usize,
    /// `tail_frames`, checked.
    tail_steps: usize,
    /// Lag range searched for the period (samples), inclusive.
    lag_lo: usize,
    lag_hi: usize,
    /// High-pass (two biquad sections) and analysis low-pass (two sections, or none).
    hp: [Biquad; 2],
    lp: [Biquad; 2],
    use_lp: bool,
    /// The last `win` filtered samples, circular; `ring_pos` is the oldest.
    ring: Vec<f32>,
    ring_pos: usize,
    since_hop: usize,
    consumed: u64,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    /// Zero-padded window (FFT input), then the autocorrelation (inverse FFT output).
    fft_buf: Vec<f32>,
    acf: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// The window in time order (kept: the FFT input buffer is destroyed by the transform).
    frame: Vec<f32>,
    /// NSDF for lags `0..=lag_hi + 1`.
    nsdf: Vec<f32>,
    /// Flattened spectrum and its autocorrelation (see `periodicity_at`), and
    /// whether that is valid for the current frame.
    flat_spec: Vec<Complex<f32>>,
    acf_flat: Vec<f32>,
    flat_ok: bool,
    /// Spectrum bins below `flat_max_hz`.
    flat_bins: usize,
    /// Power spectrum of the current frame below `flat_max_hz` (see `odd_cost`).
    pow_spec: Vec<f32>,
    /// Late-reverberation suppression (see `TrackerConfig::derev_rt60_s`): the observed
    /// power of the first `flat_bins` bins of the last `derev_delay_frames` frames (row
    /// `frame % derev_delay_frames`), and the decay factor over that delay (0 = off).
    derev_hist: Vec<f32>,
    derev_decay: f32,
    /// The suppressed power spectrum, its autocorrelation and its NSDF (empty when the
    /// suppression is off).
    derev_spec: Vec<Complex<f32>>,
    derev_acf: Vec<f32>,
    derev_nsdf: Vec<f32>,
    /// `derev_nsdf` holds the current frame's suppressed NSDF.
    derev_live: bool,
    /// Linear power thresholds of the level gate.
    power_on: f64,
    power_off: f64,
    /// Path-search history; frame `k` lives in `slots[k % RING]`.
    slots: [Slot; RING],
    /// Analysis frames done since the last reset.
    frames: u64,
    /// Whether the previously reported frame was voiced, and its (unsmoothed) pitch.
    voiced: bool,
    prev_hz: f32,
    /// Recent peak level (dB) over the newest frames, released by `peak_release_db` per
    /// frame (see `TrackerConfig::tail_drop_db`).
    peak_db: f32,
    /// A decaying tail was found and no new onset has come since; the lowest level seen in
    /// it.
    in_tail: bool,
    tail_floor_db: f32,
    /// Frames in a row, up to the newest, whose level fell by `tail_slope_db` or more.
    fall_run: u32,
}

impl Tracker {
    /// A tracker for a mono stream at `rate` Hz with the default settings.
    pub fn new(rate: u32) -> Self {
        Self::with_config(rate, TrackerConfig::default())
    }

    /// A tracker for a mono stream at `rate` Hz. Everything the tracker needs is allocated
    /// here; [`Tracker::process`] never allocates.
    ///
    /// Panics if `rate` is below 8 kHz or the settings are inconsistent (a window shorter
    /// than two periods of `min_hz`, `max_hz` above 0.45 x `rate`, a zero hop, a look-ahead
    /// outside 1..=`MAX_LOOKAHEAD`, or a start rule needing more frames than it can see).
    pub fn with_config(rate: u32, cfg: TrackerConfig) -> Self {
        assert!(
            rate >= 8_000,
            "sample rate {rate} Hz is too low for the tracker"
        );
        let fs = f64::from(rate);
        let win = (cfg.window_s * fs).round() as usize;
        let hop = (cfg.hop_s * fs).round() as usize;
        assert!(hop >= 1, "hop must be at least one sample");
        assert!(
            cfg.max_hz <= 0.45 * fs,
            "max_hz must stay below 0.45 x the sample rate"
        );
        assert!(
            cfg.min_hz > 0.0 && cfg.min_hz < cfg.max_hz,
            "bad pitch range"
        );
        let look = cfg.lookahead_frames as usize;
        assert!(
            (1..=MAX_LOOKAHEAD).contains(&look),
            "lookahead_frames must be 1..={MAX_LOOKAHEAD}"
        );
        assert!(
            (1..=look + 1).contains(&(cfg.min_voiced_frames as usize)),
            "min_voiced_frames must be 1..=lookahead_frames + 1"
        );
        assert!(
            (look + 1..=RING).contains(&(cfg.steady_frames as usize)),
            "steady_frames must be lookahead_frames + 1 ..= {RING}"
        );
        // Lags searched: from just below the period of max_hz to just above that of min_hz.
        let lag_lo = ((fs / cfg.max_hz).floor() as usize).max(2);
        let lag_hi = (fs / cfg.min_hz).ceil() as usize + 1;
        assert!(
            2 * lag_hi <= win + 2,
            "window too short for min_hz: needs about two periods"
        );
        // Linear (not circular) autocorrelation for lags up to win - 1 needs N >= 2 win - 1.
        let fft_len = (2 * win).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(fft_len);
        let c2r = planner.plan_fft_inverse(fft_len);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let db_to_power = |db: f32| 10f64.powf(f64::from(db) / 10.0);
        let mut t = Self {
            rate,
            win,
            hop,
            look,
            tail_steps: (cfg.tail_frames as usize).max(look),
            lag_lo,
            lag_hi,
            hp: Biquad::butterworth_highpass4(fs, cfg.highpass_hz),
            lp: Biquad::butterworth_lowpass4(fs, cfg.lowpass_hz.min(0.45 * fs)),
            use_lp: cfg.lowpass_hz > 0.0 && cfg.lowpass_hz < 0.45 * fs,
            ring: vec![0.0; win],
            ring_pos: 0,
            since_hop: 0,
            consumed: 0,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            frame: vec![0.0; win],
            nsdf: vec![0.0; lag_hi + 2],
            flat_spec: r2c.make_output_vec(),
            acf_flat: c2r.make_output_vec(),
            flat_ok: false,
            flat_bins: ((cfg.flat_max_hz.min(0.5 * fs) / fs * fft_len as f64).ceil() as usize)
                .min(fft_len / 2 + 1),
            pow_spec: vec![
                0.0;
                ((cfg.flat_max_hz.min(0.5 * fs) / fs * fft_len as f64).ceil() as usize)
                    .min(fft_len / 2 + 1)
            ],
            derev_hist: if cfg.derev_rt60_s > 0.0 {
                let bins = ((cfg.flat_max_hz.min(0.5 * fs) / fs * fft_len as f64).ceil() as usize)
                    .min(fft_len / 2 + 1);
                vec![0.0; bins * cfg.derev_delay_frames.max(1) as usize]
            } else {
                Vec::new()
            },
            // Power decays by 60 dB in RT60: over d frames of hop_s, 10^(-6 d hop / RT60).
            derev_decay: if cfg.derev_rt60_s > 0.0 {
                10f64.powf(
                    -6.0 * f64::from(cfg.derev_delay_frames.max(1)) * cfg.hop_s
                        / f64::from(cfg.derev_rt60_s),
                ) as f32
            } else {
                0.0
            },
            derev_spec: if cfg.derev_rt60_s > 0.0 {
                r2c.make_output_vec()
            } else {
                Vec::new()
            },
            derev_acf: if cfg.derev_rt60_s > 0.0 {
                c2r.make_output_vec()
            } else {
                Vec::new()
            },
            derev_nsdf: if cfg.derev_rt60_s > 0.0 {
                vec![0.0; lag_hi + 2]
            } else {
                Vec::new()
            },
            derev_live: false,
            fft_buf: r2c.make_input_vec(),
            acf: c2r.make_output_vec(),
            spectrum: r2c.make_output_vec(),
            r2c,
            c2r,
            power_on: db_to_power(cfg.level_on_dbfs),
            power_off: db_to_power(cfg.level_off_dbfs),
            slots: [Slot::EMPTY; RING],
            frames: 0,
            voiced: false,
            prev_hz: 0.0,
            peak_db: -200.0,
            in_tail: false,
            tail_floor_db: -200.0,
            fall_run: 0,
            cfg,
        };
        t.reset();
        t
    }

    /// The settings in use.
    pub fn config(&self) -> &TrackerConfig {
        &self.cfg
    }

    /// Input sample rate in Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Analysis window length in samples at this rate.
    pub fn window_len(&self) -> usize {
        self.win
    }

    /// Samples between two events at this rate.
    pub fn hop_len(&self) -> usize {
        self.hop
    }

    /// `ready_at - time` of every event, in samples: how far the moment an event describes
    /// lies behind the moment it is available (half a window plus the look-ahead).
    pub fn describe_delay(&self) -> u64 {
        (self.win / 2 + self.look * self.hop) as u64
    }

    /// Samples consumed since construction or the last [`Tracker::reset`].
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Forget all history (for example after the input device changed). Sample counting
    /// restarts at zero. Does not allocate.
    pub fn reset(&mut self) {
        for s in self.hp.iter_mut().chain(self.lp.iter_mut()) {
            s.clear();
        }
        self.ring.fill(0.0);
        self.ring_pos = 0;
        self.since_hop = 0;
        self.consumed = 0;
        self.slots = [Slot::EMPTY; RING];
        self.derev_hist.fill(0.0);
        self.frames = 0;
        self.voiced = false;
        self.prev_hz = 0.0;
        self.peak_db = -200.0;
        self.in_tail = false;
        self.tail_floor_db = -200.0;
        self.fall_run = 0;
    }

    /// Feed a block of mono samples (any length, including empty). `on_event` is called once
    /// for every hop boundary crossed inside the block, in time order. Never allocates.
    /// Non-finite samples (a broken driver) are treated as silence.
    pub fn process(&mut self, block: &[f32], mut on_event: impl FnMut(PitchEvent)) {
        for &x in block {
            self.push(x, &mut on_event);
        }
    }

    /// Feed one channel of an interleaved block (`channels` samples per frame; the voice on
    /// `channel`). The other channels are ignored, never summed in (WO2 trap 14).
    ///
    /// Panics if `channel >= channels`.
    pub fn process_interleaved(
        &mut self,
        block: &[f32],
        channels: usize,
        channel: usize,
        mut on_event: impl FnMut(PitchEvent),
    ) {
        assert!(channel < channels, "channel {channel} of {channels}");
        for &x in block.iter().skip(channel).step_by(channels) {
            self.push(x, &mut on_event);
        }
    }

    #[inline]
    fn push(&mut self, x: f32, on_event: &mut impl FnMut(PitchEvent)) {
        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        let first = self.hp[0].tick(x);
        let mut y = self.hp[1].tick(first);
        if self.use_lp {
            let a = self.lp[0].tick(y);
            y = self.lp[1].tick(a);
        }
        self.ring[self.ring_pos] = y as f32;
        self.ring_pos += 1;
        if self.ring_pos == self.win {
            self.ring_pos = 0;
        }
        self.consumed += 1;
        self.since_hop += 1;
        if self.since_hop == self.hop {
            self.since_hop = 0;
            let ev = self.step();
            on_event(ev);
        }
    }

    /// The slot of the frame `back` frames before the newest (0 = newest), if that frame
    /// exists.
    fn slot(&self, back: usize) -> Option<&Slot> {
        let n = self.frames.checked_sub(1 + back as u64)?;
        Some(&self.slots[(n % RING as u64) as usize])
    }

    /// Analyse the current window, extend the path search, decide the frame `look` frames
    /// back and run the voicing logic on it: one event.
    fn step(&mut self) -> PitchEvent {
        let mut slot = self.analyse();
        // Too quiet or too aperiodic ever to be voiced (silence, a breath or a consonant
        // before a note): the path cannot pass through this frame, so a note's first frames
        // are not pulled towards whatever period the noise before it suggested.
        let min_clarity = self
            .cfg
            .clarity_off
            .min(self.cfg.clarity_on)
            .min(self.cfg.clarity_steady)
            .min(if self.cfg.early_start {
                self.cfg.start_floor
            } else {
                1.0
            });
        if slot.power < self.power_off || slot.clarity < min_clarity {
            slot.len = 0;
        }
        let prev = self.slot(0).copied();
        relax(prev.as_ref(), &mut slot, |a, b| self.step_cost(a, b));
        self.slots[(self.frames % RING as u64) as usize] = slot;
        self.frames += 1;

        // Reverb tails (see `TrackerConfig::tail_drop_db`): the recent peak level follows the
        // newest frame; the decided frame is a tail when it lies `tail_drop_db` under that
        // peak and the level has fallen between each of the last `tail_frames` frames.
        let look = self.look;
        let lv = |t: &Self, k: usize| t.slot(k).map_or(-200.0, |s| s.level_db);
        self.peak_db = lv(self, 0).max(self.peak_db - self.cfg.peak_release_db);
        // Consecutive frames, up to the newest, each at least tail_slope_db quieter than
        // the one before.
        if lv(self, 0) <= lv(self, 1) - self.cfg.tail_slope_db {
            self.fall_run = self.fall_run.saturating_add(1);
        } else {
            self.fall_run = 0;
        }
        let tail = self.cfg.tail_drop_db > 0.0
            && lv(self, look) <= self.peak_db - self.cfg.tail_drop_db
            && self.fall_run as usize >= self.tail_steps;
        if tail && !self.in_tail {
            self.in_tail = true;
            self.tail_floor_db = lv(self, look);
        }
        if self.in_tail {
            // A new onset (the level rising out of the tail) ends the tail state, and so
            // do a level that has stopped falling over the last RING - 1 frames (40 ms) and
            // a clear pitch away from the last one shown: a room tail keeps decaying on the
            // last note's pitch, a note sung legato after it (perhaps quieter, on a weaker
            // vowel) does neither.
            let newest = lv(self, 0);
            let new_pitch = self.prev_hz > 0.0
                && self.slot(0).and_then(Slot::clearest).is_some_and(|c| {
                    c.n >= self.cfg.clarity_off
                        && (1200.0 * (c.hz / self.prev_hz).log2()).abs() > self.cfg.max_step_cents
                });
            if newest >= self.tail_floor_db + self.cfg.tail_rise_db
                || newest >= lv(self, RING - 1) - self.cfg.tail_slope_db
                || new_pitch
            {
                self.in_tail = false;
            } else {
                self.tail_floor_db = self.tail_floor_db.min(newest);
            }
        }

        // Pick the path: candidate index per frame, k = 0 the newest .. look + 1. While a
        // note is being shown it is the Viterbi path (pinned to what was shown); before a
        // note starts it is anchored on the newest frame (see `anchored`).
        let path = if self.voiced {
            self.backtrack()
        } else {
            self.anchored()
        };
        let cand = |k: usize| -> Option<Cand> {
            let i = path[k]?;
            self.slot(k).map(|s| s.cands[i])
        };
        let level_ok = |k: usize, start: bool| -> bool {
            let gate = if start { self.power_on } else { self.power_off };
            self.slot(k).is_some_and(|s| s.power >= gate)
        };
        let step_cents = |a: Cand, b: Cand| (1200.0 * (a.hz / b.hz).log2()).abs();

        // Voicing of the decided frame (k = look).
        let decided = cand(look);
        let voiced = match decided {
            None => false,
            Some(c) if self.voiced => {
                // A jump (more than max_step_cents) needs the plain clarity at clarity_on;
                // the suppression's help counts only for continuing the line.
                !tail
                    && level_ok(look, false)
                    && (c.n0 >= self.cfg.clarity_on
                        || (c.n >= self.cfg.clarity_off
                            && (1200.0 * (c.hz / self.prev_hz).log2()).abs()
                                <= self.cfg.max_step_cents))
            }
            // No start inside a decaying tail (until a new onset).
            Some(_) if self.in_tail || tail => false,
            Some(c) => {
                // Confident start: this frame at clarity_on, the next frames continuing it.
                let confident = c.n >= self.cfg.clarity_on && level_ok(look, true) && {
                    let mut prev = c;
                    (1..self.cfg.min_voiced_frames as usize).all(|j| {
                        cand(look - j).is_some_and(|d| {
                            let ok = d.n >= self.cfg.clarity_off
                                && level_ok(look - j, false)
                                && step_cents(d, prev) <= self.cfg.max_step_cents;
                            prev = d;
                            ok
                        })
                    })
                };
                // Early start (see `TrackerConfig::early_start`): a confirming pair of
                // adjacent frames (j + 1, j) in the look-ahead, one at clarity_on and the
                // other at clarity_off on one line, and every frame from the pair back to the
                // decided one continuing that line at start_floor. On the anchored path each
                // frame is already within max_step_cents of the newer one.
                // A frame that also holds a fair candidate an octave above its chosen one
                // (an onset whose octave is not yet clear) does not confirm an early start.
                let octave_up = |k: usize, c: Cand| -> bool {
                    self.slot(k).is_some_and(|s| {
                        s.cands[..s.len].iter().any(|d| {
                            d.n >= self.cfg.early_octave_veto
                                && (1200.0 * (d.hz / (2.0 * c.hz)).log2()).abs() <= 50.0
                        })
                    })
                };
                let early = !confident && self.cfg.early_start && {
                    // Frames whose chosen period is an octave below MPM's own pick do not
                    // confirm an early start: in the first windows of a high "ee" or "oo"
                    // the octave below looks more periodic for 20-40 ms; such a note
                    // starts by the plain rule, on the octave of a fuller window.
                    let pair = |j: usize| -> bool {
                        let (Some(a), Some(b)) = (cand(j + 1), cand(j)) else {
                            return false;
                        };
                        if a.below || b.below || octave_up(j + 1, a) || octave_up(j, b) {
                            return false;
                        }
                        let on =
                            |d: Cand, k: usize| d.n >= self.cfg.clarity_on && level_ok(k, true);
                        let off =
                            |d: Cand, k: usize| d.n >= self.cfg.clarity_off && level_ok(k, false);
                        // Not below steady_min_hz, where handling thumps live: there a note
                        // starts by the plain rule (a longer periodic stretch).
                        ((on(a, j + 1) && off(b, j)) || (on(b, j) && off(a, j + 1)))
                            && step_cents(a, b) <= self.cfg.max_step_cents
                            && f64::from(a.hz.min(b.hz)) >= self.cfg.steady_min_hz
                    };
                    // The newest window must hold no fair candidate an octave above its
                    // own either, and the level must have stopped rising (the attack of
                    // the note is over: the newest window is about as full as the next).
                    let settled = self
                        .slot(0)
                        .is_some_and(|s| s.level_db - lv(self, 1) <= self.cfg.early_rise_db)
                        && cand(0).is_some_and(|d| !octave_up(0, d));
                    settled
                        && (0..look).any(|j| {
                            pair(j)
                                && (j + 1..=look).all(|k| {
                                    cand(k).is_some_and(|d| {
                                        d.n >= self.cfg.start_floor
                                            && level_ok(k, false)
                                            && !d.below
                                    })
                                })
                        })
                };
                // Steady start: every frame from the newest back over `steady_frames` (the
                // look-ahead, the decided frame and the frames before it) clear enough, loud
                // enough, above `steady_min_hz`, harmonic enough and on one pitch.
                let steady = !confident && !early && {
                    let mut prev: Option<Cand> = None;
                    (0..self.cfg.steady_frames as usize).all(|k| {
                        cand(k).is_some_and(|d| {
                            let ok = d.n >= self.cfg.clarity_steady
                                && d.per >= self.cfg.steady_harmonic_min
                                && f64::from(d.hz) >= self.cfg.steady_min_hz
                                && level_ok(k, true)
                                && prev
                                    .is_none_or(|p| step_cents(d, p) <= self.cfg.steady_step_cents);
                            prev = Some(d);
                            ok
                        })
                    })
                };
                confident || early || steady
            }
        };

        // Reported pitch: median of the decided frame, the frame before it (if it was
        // reported voiced) and the frame after it on the path (if it continues the line).
        let hz = decided.filter(|_| voiced).map(|c| {
            let next = cand(look - 1).filter(|d| {
                d.n >= self.cfg.clarity_off
                    && level_ok(look - 1, false)
                    && step_cents(*d, c) <= self.cfg.max_step_cents
            });
            match (self.voiced.then_some(self.prev_hz), next) {
                (Some(a), Some(d)) => median3(a, c.hz, d.hz),
                // The start of a run: the newer window holds more of the new note.
                (None, Some(d)) => d.hz,
                _ => c.hz,
            }
        });
        self.voiced = voiced;
        if let (Some(c), Some(i), true) = (decided, path[look], voiced) {
            self.prev_hz = c.hz;
            self.pin(look, i);
        }

        let ready_at = self.consumed;
        let (clarity, power) = match self.slot(look) {
            Some(s) => (decided.or(s.clearest()).map_or(s.clarity, |c| c.n), s.power),
            None => (0.0, 0.0),
        };
        PitchEvent {
            time: ready_at as i64 - self.describe_delay() as i64,
            ready_at,
            hz,
            clarity,
            level_dbfs: if power > 1e-20 {
                (10.0 * power.log10()) as f32
            } else {
                -200.0
            },
        }
    }

    /// Fixes the path through candidate `idx` of the frame `k` frames before the newest
    /// (the frame just reported voiced) and recomputes the path scores of the newer frames.
    /// Every later decision then continues what was shown: fixed-lag decoding alone could
    /// switch octave between two consecutive decisions when a new frame changes the best
    /// path.
    fn pin(&mut self, k: usize, idx: usize) {
        let Some(n) = self.frames.checked_sub(1 + k as u64) else {
            return;
        };
        let at = |f: u64| (f % RING as u64) as usize;
        let s = &mut self.slots[at(n)];
        for (j, a) in s.acc[..s.len].iter_mut().enumerate() {
            *a = if j == idx { 0.0 } else { PINNED_OUT };
        }
        for f in n + 1..self.frames {
            let prev = self.slots[at(f - 1)];
            let mut slot = self.slots[at(f)];
            relax(Some(&prev), &mut slot, |a, b| self.step_cost(a, b));
            self.slots[at(f)] = slot;
        }
    }

    /// Path cost of a pitch step between consecutive frames: `jump_cost` per octave down,
    /// `jump_up_cost` per octave up, free within 50 cents (vibrato, glides), capped at two
    /// octaves.
    fn step_cost(&self, from_hz: f32, to_hz: f32) -> f32 {
        let c = 1200.0 * (to_hz / from_hz).log2();
        if c.abs() <= 50.0 {
            0.0
        } else if c < 0.0 {
            self.cfg.jump_cost * c.abs().min(2400.0) / 1200.0
        } else {
            self.cfg.jump_up_cost * c.min(2400.0) / 1200.0
        }
    }

    /// The path used before a note starts: the newest frame's best-scoring candidate,
    /// followed back frame by frame to the candidate nearest in pitch, as long as that is
    /// within `max_step_cents` (`None` from the first frame that does not continue it).
    ///
    /// The first windows of a note hold the silence before it and the vocal tract's
    /// ringing at the first formant; for high "oo" and "ee" notes that makes an octave
    /// below look more periodic for a frame or two (n = 0.93 at twice the period against
    /// 0.85 at the period, 10 ms after the onset of a generated C5 "oo"). The newest window
    /// holds the most of the note, so its octave is the one the note starts in. The
    /// Viterbi path takes over once the note is shown.
    fn anchored(&self) -> [Option<usize>; RING] {
        let mut out = [None; RING];
        let Some(newest) = self.slot(0).filter(|s| s.len > 0) else {
            return out;
        };
        let mut i = 0;
        for j in 1..newest.len {
            if newest.cands[j].score > newest.cands[i].score {
                i = j;
            }
        }
        out[0] = Some(i);
        let mut hz = newest.cands[i].hz;
        for (k, o) in out.iter_mut().enumerate().skip(1) {
            let Some(s) = self.slot(k) else { break };
            let near = s.cands[..s.len]
                .iter()
                .enumerate()
                .map(|(j, c)| (j, (1200.0 * (c.hz / hz).log2()).abs()))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match near {
                Some((j, d)) if d <= self.cfg.max_step_cents => {
                    *o = Some(j);
                    hz = s.cands[j].hz;
                }
                _ => break,
            }
        }
        out
    }

    /// The chosen candidate of each of the last `RING` frames (index 0 = newest): the
    /// best path through the newest frame, followed back. Where a path starts (the frame
    /// before it could not be passed through) the best path ending in the earlier frame is
    /// followed instead.
    fn backtrack(&self) -> [Option<usize>; RING] {
        let mut out = [None; RING];
        let mut next: Option<usize> = None;
        for (k, o) in out.iter_mut().enumerate() {
            let Some(s) = self.slot(k) else { break };
            if s.len == 0 {
                next = None;
                continue;
            }
            let i = next.filter(|&i| i < s.len).unwrap_or_else(|| s.best());
            *o = Some(i);
            next = (s.back[i] != NO_BACK).then_some(usize::from(s.back[i]));
        }
        out
    }

    /// McLeod Pitch Method on the current window: the frame's candidates, power and
    /// clarity (path fields not yet filled).
    fn analyse(&mut self) -> Slot {
        let win = self.win;
        let mut slot = Slot::EMPTY;
        // Window in time order: the ring's oldest sample is at ring_pos.
        let (newer, older) = self.ring.split_at(self.ring_pos);
        self.frame[..older.len()].copy_from_slice(older);
        self.frame[older.len()..].copy_from_slice(newer);

        let energy: f64 = self
            .frame
            .iter()
            .map(|&v| f64::from(v) * f64::from(v))
            .sum();
        slot.power = energy / win as f64;
        slot.level_db = if slot.power > 1e-20 {
            (10.0 * slot.power.log10()) as f32
        } else {
            -200.0
        };
        // This frame's row of the late-reverberation history (it holds the frame
        // `derev_delay_frames` before this one until it is overwritten below).
        let derev_rows = self.cfg.derev_delay_frames.max(1) as usize;
        let derev_row = (self.frames % derev_rows as u64) as usize * self.flat_bins;
        let derev = self.derev_decay > 0.0;
        // Digital silence or near it: nothing to analyse (and no division by ~0 below).
        if energy < 1e-12 {
            if derev {
                self.derev_hist[derev_row..derev_row + self.flat_bins].fill(0.0);
            }
            return slot;
        }

        // Autocorrelation r(t) for every lag, via |FFT|^2 of the zero-padded window.
        self.fft_buf[..win].copy_from_slice(&self.frame);
        self.fft_buf[win..].fill(0.0);
        if self
            .r2c
            .process_with_scratch(&mut self.fft_buf, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            return slot;
        }
        // Power spectrum for the autocorrelation; a flattened (compressed) copy, 0 to
        // flat_max_hz only (the same band at every rate), for scoring the candidates.
        let flat_exp = 0.5 * self.cfg.flat_power;
        let flat_bins = self.flat_bins;
        for (k, (c, f)) in self
            .spectrum
            .iter_mut()
            .zip(self.flat_spec.iter_mut())
            .enumerate()
        {
            let p = c.re * c.re + c.im * c.im;
            if let Some(w) = self.pow_spec.get_mut(k) {
                *w = p;
            }
            // Late reverberation: what this bin held derev_delay_frames ago, decayed; the
            // suppressed power `q`.
            if derev {
                let mut q = p;
                if k < flat_bins {
                    let h = &mut self.derev_hist[derev_row + k];
                    let late = self.derev_decay * self.cfg.derev_over * *h;
                    *h = p;
                    if p > 1e-30 {
                        q *= (1.0 - late / p).max(self.cfg.derev_floor);
                    }
                }
                // Bins above the suppressed band keep their plain power.
                self.derev_spec[k] = Complex::new(q, 0.0);
            }
            *c = Complex::new(p, 0.0);
            let v = if flat_exp > 0.0 && k < flat_bins {
                compress(p, flat_exp)
            } else {
                0.0
            };
            *f = Complex::new(v, 0.0);
        }
        // The inverse transform reports (harmless) non-zero imaginary DC/Nyquist parts as an
        // error; they were zeroed above, so any error here is a real one.
        if self
            .c2r
            .process_with_scratch(&mut self.spectrum, &mut self.acf, &mut self.scratch)
            .is_err()
        {
            return slot;
        }
        self.flat_ok = flat_exp > 0.0
            && self
                .c2r
                .process_with_scratch(&mut self.flat_spec, &mut self.acf_flat, &mut self.scratch)
                .is_ok()
            && self.acf_flat[0] > 0.0;
        // realfft's inverse is unnormalised: r(t) = acf[t] / N. The NSDF is a ratio of r to
        // m, so scale m by N instead of r.
        let n_fft = self.fft_buf.len() as f64;
        // The suppressed spectrum's autocorrelation is that of a quieter signal: its NSDF
        // scales m by the share of the energy kept (acf[0] / (N energy)), so a uniform gain
        // leaves the NSDF as it was.
        let derev3 = derev
            && self
                .c2r
                .process_with_scratch(&mut self.derev_spec, &mut self.derev_acf, &mut self.scratch)
                .is_ok()
            && self.derev_acf[0] > 0.0;
        let n_derev = if derev3 {
            f64::from(self.derev_acf[0]) / energy
        } else {
            1.0
        };

        // NSDF n(t) = 2 r(t) / m(t), m(t) = sum_{j < win - t} x[j]^2 + x[j + t]^2.
        let mut m = 2.0 * energy;
        self.nsdf[0] = 1.0;
        for t in 1..self.nsdf.len() {
            let a = f64::from(self.frame[t - 1]);
            let b = f64::from(self.frame[win - t]);
            m -= a * a + b * b;
            self.nsdf[t] = if m > 1e-12 * energy {
                (2.0 * f64::from(self.acf[t]) / (m * n_fft)) as f32
            } else {
                0.0
            };
            if derev3 {
                self.derev_nsdf[t] = if m > 1e-12 * energy {
                    (2.0 * f64::from(self.derev_acf[t]) / (m * n_derev)) as f32
                } else {
                    0.0
                };
            }
        }
        self.derev_live = derev3;

        self.collect_candidates(&mut slot);
        slot
    }

    /// Fills `slot.cands` from the NSDF: the key maxima reaching `candidate_floor` x the
    /// highest, inside the pitch range, shortest periods first (a perfectly periodic sound
    /// has a key maximum at every multiple of its period, all about equally high, so the
    /// shortest ones are the ones that matter), with the highest key maximum always among
    /// them. Score: the flattened-spectrum periodicity at the period (the NSDF value when
    /// `flat_power` is 0), less `octave_cost * log2(period / paper's period)` for periods
    /// longer than the paper's choice (the first key maximum reaching `peak_threshold` x
    /// the highest).
    fn collect_candidates(&self, slot: &mut Slot) {
        let n = &self.nsdf;
        let mut top = f32::NEG_INFINITY;
        self.key_maxima(|t| top = top.max(n[t]));
        if !top.is_finite() || top <= 0.0 {
            return;
        }
        slot.clarity = top.min(1.0);
        let floor = self.cfg.candidate_floor * top;
        let paper = self.cfg.peak_threshold * top;
        let fs = f64::from(self.rate);
        let mut paper_hz: Option<f32> = None;
        let mut top_seen = false;
        self.key_maxima(|t| {
            if n[t] < floor {
                return;
            }
            let (lag, value) = self.interpolate(t);
            let hz = fs / lag;
            if n[t] >= paper && paper_hz.is_none() {
                paper_hz = Some(hz as f32);
            }
            if !(self.cfg.min_hz..=self.cfg.max_hz).contains(&hz) {
                return;
            }
            // With the late-reverberation suppression the clarity is the higher of the
            // plain NSDF and the NSDF of the suppressed spectrum near the same lag (above
            // derev_min_hz; see also derev_per_min below).
            let n0 = value;
            let value = if self.derev_live && hz >= self.cfg.derev_min_hz {
                let d = &self.derev_nsdf;
                value.max(d[t - 1].max(d[t]).max(d[t + 1]).min(1.0))
            } else {
                value
            };
            let c = Cand {
                hz: hz as f32,
                n: value,
                score: 0.0,
                per: 0.0,
                n0,
                below: false,
            };
            let is_top = n[t] >= top;
            if slot.len < MAX_CANDS - usize::from(!top_seen) || (is_top && !top_seen) {
                // The last place is kept for the highest key maximum until it is seen.
                if slot.len < MAX_CANDS {
                    slot.cands[slot.len] = c;
                    slot.len += 1;
                }
                top_seen |= is_top;
            }
        });
        let reference = paper_hz.unwrap_or(f32::MAX);
        for c in &mut slot.cands[..slot.len] {
            let octaves_below = (reference / c.hz).log2().max(0.0);
            c.below = octaves_below >= 0.5;
            let periodicity = if self.flat_ok {
                periodicity_at(&self.acf_flat, self.win, fs / f64::from(c.hz))
            } else {
                c.n
            };
            c.per = periodicity;
            c.score = periodicity - self.cfg.octave_cost * octaves_below;
            // The suppressed clarity is believed only for a candidate with a harmonic
            // series: spectral subtraction leaves random isolated peaks in noise ("musical
            // noise"), which can make a whispered vowel look periodic.
            if c.per < self.cfg.derev_per_min {
                c.n = c.n0;
            }
            slot.clarity = slot.clarity.max(c.n);
        }
        // Subharmonics without harmonic evidence (see `TrackerConfig::odd_cost`).
        if self.cfg.odd_cost > 0.0 {
            for i in 0..slot.len {
                let low = slot.cands[i].hz;
                let multiple = slot.cands[..slot.len].iter().find_map(|d| {
                    [2u32, 3]
                        .into_iter()
                        .find(|&m| (1200.0 * (d.hz / (m as f32 * low)).log2()).abs() <= 50.0)
                });
                if let Some(m) = multiple
                    && self.own_harmonic_ratio(low, m) < self.cfg.odd_ratio_min
                {
                    slot.cands[i].score -= self.cfg.odd_cost;
                }
            }
        }
    }

    /// Power at the harmonics of `f0` that are not multiples of `m` (up to 3 kHz), over the
    /// power halfway between neighbouring harmonics of `f0` (see `TrackerConfig::odd_cost`);
    /// peaks are the highest of three bins around the frequency. Large (100) when nothing
    /// can be measured.
    fn own_harmonic_ratio(&self, f0: f32, m: u32) -> f32 {
        let bin_hz = f64::from(self.rate) / self.fft_buf.len() as f64;
        let at = |f: f64| -> f64 {
            let b = (f / bin_hz).round() as usize;
            if b < 1 || b + 1 >= self.pow_spec.len() {
                return f64::NAN;
            }
            f64::from(
                self.pow_spec[b - 1]
                    .max(self.pow_spec[b])
                    .max(self.pow_spec[b + 1]),
            )
        };
        let (mut peaks, mut valleys) = (0.0f64, 0.0f64);
        let f0 = f64::from(f0);
        let mut h = 1u32;
        while f64::from(h) * f0 <= 3000.0 {
            if !h.is_multiple_of(m) {
                let (p, a, b) = (
                    at(f64::from(h) * f0),
                    at((f64::from(h) - 0.5) * f0),
                    at((f64::from(h) + 0.5) * f0),
                );
                if p.is_finite() && a.is_finite() && b.is_finite() {
                    peaks += p;
                    valleys += 0.5 * (a + b);
                }
            }
            h += 1;
        }
        if valleys > 0.0 {
            (peaks / valleys) as f32
        } else {
            100.0
        }
    }

    /// Parabolic interpolation of the NSDF peak at integer lag `t`: (lag, value).
    fn interpolate(&self, t: usize) -> (f64, f32) {
        let n = &self.nsdf;
        let (a, b, c) = (n[t - 1], n[t], n[t + 1]);
        let den = a - 2.0 * b + c;
        let delta = if den.abs() > 1e-12 {
            (0.5 * (a - c) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let peak = b - 0.25 * (a - c) * delta;
        (t as f64 + f64::from(delta), peak.clamp(0.0, 1.0))
    }

    /// Calls `visit` with the lag of every key maximum in the searched lag range, in order
    /// of lag. A key maximum is the highest local maximum of a positive region of the
    /// NSDF; the zero-lag lobe and the first negative region are skipped (McLeod and
    /// Wyvill, section 5).
    fn key_maxima(&self, mut visit: impl FnMut(usize)) {
        let n = &self.nsdf;
        let last = n.len() - 1; // n[last] exists only to test a local maximum at last - 1
        let mut t = 1;
        while t < last && n[t] > 0.0 {
            t += 1;
        }
        while t < last && n[t] <= 0.0 {
            t += 1;
        }
        let mut region_max: Option<usize> = None;
        while t < last {
            if n[t] > 0.0 {
                if n[t] > n[t - 1]
                    && n[t] >= n[t + 1]
                    && (self.lag_lo..=self.lag_hi).contains(&t)
                    && region_max.is_none_or(|r| n[t] > n[r])
                {
                    region_max = Some(t);
                }
            } else if let Some(r) = region_max.take() {
                // End of a positive region: r is its key maximum.
                visit(r);
            }
            t += 1;
        }
        // A region still open at the end of the lag range.
        if let Some(r) = region_max {
            visit(r);
        }
    }
}

/// Periodicity at `lag` measured on a flattened spectrum: `acf` is the autocorrelation of
/// `|X|^flat_power` (for a `win`-sample window), normalised by its value at lag 0 and corrected for the shrinking
/// overlap of the window with itself (so a perfectly periodic sound gives about 1 at
/// every multiple of its period). Linear interpolation between the two nearest lags.
///
/// Compressing the spectrum before the autocorrelation weighs weak harmonics more, so
/// the odd harmonics that tell an octave from its double count even when they are
/// 15 dB down (a real voice whose fundamental and odd harmonics are weak is otherwise
/// reported an octave high), while noise concentrated in a narrow band (breath
/// through a formant) no longer looks periodic at a multiple of the period (otherwise
/// a breathy high note is reported an octave or a twelfth low).
///
/// Measured on the real-voice development set (docs/benchmarks/practice-tracker.md
/// section 13): scored by the NSDF value instead, 0.92 % of the frames both voiced were
/// an octave off and the marker jumped an octave 130 times inside a held pitch; scored
/// by this, 0.47 % and 63 times.
fn periodicity_at(acf: &[f32], win: usize, lag: f64) -> f32 {
    let i = lag.floor() as usize;
    let w = win as f64;
    if i + 1 >= win {
        return 0.0;
    }
    let at = |t: usize| f64::from(acf[t]) * w / (w - t as f64);
    let frac = lag - i as f64;
    let v = (at(i) * (1.0 - frac) + at(i + 1) * frac) / f64::from(acf[0]);
    v.clamp(-1.0, 1.0) as f32
}

/// Path score given to the candidates a pinned frame did not choose: far below any path
/// through the chosen one, yet finite so the arithmetic stays well-defined.
const PINNED_OUT: f32 = -1.0e6;

/// One Viterbi step into `slot`: each candidate's best path score through `prev` (its own
/// score plus the best of `prev`'s path scores less the cost of the pitch step), normalised
/// so the best is 0. A path starts afresh (`NO_BACK`) when `prev` has no candidates.
fn relax(prev: Option<&Slot>, slot: &mut Slot, step_cost: impl Fn(f32, f32) -> f32) {
    let prev = prev.filter(|p| p.len > 0);
    let mut top = f32::NEG_INFINITY;
    for i in 0..slot.len {
        let c = slot.cands[i];
        let (mut acc, mut back) = (c.score, NO_BACK);
        if let Some(p) = prev {
            let mut best = f32::NEG_INFINITY;
            for j in 0..p.len {
                let v = p.acc[j] - step_cost(p.cands[j].hz, c.hz);
                if v > best {
                    best = v;
                    back = j as u8;
                }
            }
            acc += best;
        }
        slot.acc[i] = acc;
        slot.back[i] = back;
        top = top.max(acc);
    }
    for a in &mut slot.acc[..slot.len] {
        *a -= top;
    }
}

/// `p^e` for a power spectrum value `p >= 0`. The exponents in use are done with square
/// roots: `powf` from the MinGW runtime costs about 200 ns per call on this build PC, which
/// made the flattened spectrum alone cost 9 ms per second of audio.
#[inline]
fn compress(p: f32, e: f32) -> f32 {
    if e == 0.25 {
        p.sqrt().sqrt()
    } else if e == 0.5 {
        p.sqrt()
    } else if e == 1.0 {
        p
    } else {
        p.powf(e)
    }
}

/// Median of three values.
fn median3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).min(a.min(b).max(c))
}

/// Direct-form-I biquad in f64 (a 55 Hz corner at 96 kHz needs the precision).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// High-pass section with quality `q` at `fc` Hz (bilinear transform, pre-warped, so
    /// the response is the same at every sample rate).
    fn highpass(fs: f64, fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    /// Low-pass section with quality `q` at `fc` Hz (bilinear transform, pre-warped).
    fn lowpass(fs: f64, fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    /// 4th-order Butterworth low-pass as two sections.
    fn butterworth_lowpass4(fs: f64, fc: f64) -> [Self; 2] {
        [
            Self::lowpass(fs, fc, 0.541_196_100_146_197),
            Self::lowpass(fs, fc, 1.306_562_964_876_376_5),
        ]
    }

    /// 4th-order Butterworth high-pass as two sections (Q = 0.5412 and 1.3066).
    fn butterworth_highpass4(fs: f64, fc: f64) -> [Self; 2] {
        [
            Self::highpass(fs, fc, 0.541_196_100_146_197),
            Self::highpass(fs, fc, 1.306_562_964_876_376_5),
        ]
    }

    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Flush denormals so a long silence never slows the filter down.
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        self.y1
    }
}

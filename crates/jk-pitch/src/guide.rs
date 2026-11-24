// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody guide builder (Work Order 2, section 4.1): isolated lead vocal -> notes.
//!
//! Two stages, usable separately so that any pitch tracker can be compared through the
//! same note segmentation (docs/benchmarks/practice-guide.md):
//!
//! 1. [`pitch_curve`]: a pYIN-style tracker (Mauch & Dixon 2014: YIN difference function,
//!    a distribution of thresholds turning each frame into weighted pitch candidates,
//!    then an HMM over pitch bins x voiced/unvoiced decoded with Viterbi). Output: one
//!    frame every [`GuideConfig::hop_s`] with the pitch in Hz (0 = unvoiced), the
//!    probability that the frame is voiced, and the frame's level.
//! 2. [`segment`]: pitch curve -> notes. Cuts at voicing gaps, at level dips (syllable
//!    boundaries heard in the audio) and at pitch changes (a penalised piecewise-constant
//!    fit after vibrato has been flattened), keeps vibrato inside one note, folds scoops
//!    and slides into the note they lead to, takes the median pitch of each note's steady
//!    part (fractions of a semitone, never rounded to a key), marks speech-like stretches
//!    as unpitched, gives every note a confidence, estimates the recording's tuning and
//!    checks every note's octave on the spectrum by a second, independent method, then
//!    against the melody around it (a note left "ok" inside a run the spectrum flagged).
//!
//! [`build`] runs both. Pure functions: no threads, no I/O, so the same code runs on
//! Windows and macOS and on any computer without the Studio.
//!
//! Notes come from the audio only. Lyric timings never move or delete a note; mapping
//! notes to words is done later by overlap.

mod octave;
mod pyin;
mod resample;
mod segment;
pub mod words;

use serde::Serialize;

/// What made a guide: stored with it so a better method can replace old guides later.
pub const METHOD: &str = "jk-pitch pyin+notes";
/// Bumped whenever the builder's output can change for the same input.
/// 0.2.0 (WO2 Stage 2): the phrase-context octave check after the spectral one
/// (`octave::context_check`); notes, pitches and the curve are unchanged from 0.1.0.
/// 0.3.0 (owner, 2026-11-23): notes are put in the octave the voice's spectrum and the
/// melody round them agree on, before the checks (`choose_octaves`); the curve under a
/// moved note moves with it.
pub const VERSION: &str = "0.3.0";

/// Tunables. [`GuideConfig::default`] is what songs use; the benchmark documents why.
#[derive(Clone, Debug, Serialize)]
pub struct GuideConfig {
    /// Lowest pitch searched (Hz). 60 Hz is below C2 (65.4 Hz).
    pub fmin_hz: f64,
    /// Highest pitch searched (Hz). 1100 Hz is above C6 (1046.5 Hz).
    pub fmax_hz: f64,
    /// Frame step of the pitch curve (s).
    pub hop_s: f64,
    /// Analysis window of the YIN difference function (s). 32 ms holds 2.6 periods of E2.
    pub window_s: f64,
    /// HMM pitch-bin size in cents. Output pitch is not quantised to it: each voiced frame
    /// takes the exact candidate nearest to the decoded bin.
    pub bin_cents: f64,
    /// pYIN: probability of switching between voiced and unvoiced per frame.
    pub voicing_switch_prob: f64,
    /// pYIN: fastest pitch movement allowed between frames (octaves per second).
    pub max_transition_octaves_per_s: f64,
    /// Note cut: penalty per extra note in the piecewise-constant fit (cents^2).
    pub note_penalty: f64,
    /// Shortest note kept (s).
    pub min_note_s: f64,
    /// A level dip this deep (dB) between two louder parts inside a voiced stretch cuts it.
    pub dip_db: f64,
    /// Voiced frames quieter than the vocal's loud level by more than this (dB) are ignored
    /// (separation bleed, reverb tails).
    pub floor_db: f64,
    /// A note whose pitch keeps moving faster than this (robust slope, cents per second)
    /// is speech-like; sung notes (vibrato flattened) drift far less.
    pub speech_cents_per_s: f64,
    /// A piece is unpitched when less than this share of it lies in steady notes.
    pub speech_steady_share: f64,
    /// A phrase (gaps under 0.3 s) at least this share unpitched becomes unpitched as a whole.
    pub spoken_phrase_share: f64,
    /// A note whose confidence is below this is "low confidence" (drawn dim, not graded).
    pub low_confidence: f32,
    /// The tuning offset is used (`tuning.use_cents` in the file) only when its strength
    /// reaches this; below it the recording has no consistent tuning (rap songs measure
    /// 0.11-0.16) and readers use A = 440 Hz (SONG_FORMAT 10.1, D-237).
    pub tuning_min_strength: f64,
    /// A note flagged by the octave check has its confidence multiplied by this (so it is
    /// drawn dim and not graded).
    pub octave_flag_factor: f32,
    /// Phrase-context octave check after the spectral one (builder 0.2.0): 0 off, 1 the
    /// run rule (default), 2 the run rule and the isolated-jump rule (octave::context_check).
    pub octave_context: f64,
    /// The melody this far either side of a note is its context (s).
    pub octave_context_s: f64,
    /// Neighbours this close (s) count as touching for the run rule.
    pub octave_run_gap_s: f64,
    /// The isolated-jump rule looks at notes at most this long (s).
    pub octave_isolated_max_s: f64,
    /// Builder 0.3.0: choose each pitched note's octave from its spectrum and the melody
    /// round it, and move it (`choose_octaves`): 1 on (default), 0 off (as 0.2.0).
    pub octave_choose: f64,
    /// `choose_octaves`: cost per dB a candidate's spectral support falls short of the best
    /// candidate's (per unit of note weight).
    pub octave_choose_spectrum: f64,
}

impl GuideConfig {
    /// Sets one numeric field by name (benchmark sweeps). Returns false for an unknown name.
    pub fn set(&mut self, name: &str, value: f64) -> bool {
        match name {
            "note_penalty" => self.note_penalty = value,
            "min_note_s" => self.min_note_s = value,
            "dip_db" => self.dip_db = value,
            "floor_db" => self.floor_db = value,
            "speech_cents_per_s" => self.speech_cents_per_s = value,
            "speech_steady_share" => self.speech_steady_share = value,
            "spoken_phrase_share" => self.spoken_phrase_share = value,
            "voicing_switch_prob" => self.voicing_switch_prob = value,
            "window_s" => self.window_s = value,
            "low_confidence" => self.low_confidence = value as f32,
            "tuning_min_strength" => self.tuning_min_strength = value,
            "octave_flag_factor" => self.octave_flag_factor = value as f32,
            "octave_context" => self.octave_context = value,
            "octave_context_s" => self.octave_context_s = value,
            "octave_run_gap_s" => self.octave_run_gap_s = value,
            "octave_isolated_max_s" => self.octave_isolated_max_s = value,
            "octave_choose" => self.octave_choose = value,
            "octave_choose_spectrum" => self.octave_choose_spectrum = value,
            _ => return false,
        }
        true
    }
}

impl Default for GuideConfig {
    fn default() -> Self {
        GuideConfig {
            fmin_hz: 60.0,
            fmax_hz: 1100.0,
            hop_s: 0.01,
            window_s: 0.032,
            bin_cents: 20.0,
            voicing_switch_prob: 0.01,
            max_transition_octaves_per_s: 35.92,
            note_penalty: 16_000.0,
            min_note_s: 0.04,
            dip_db: 6.0,
            floor_db: 35.0,
            speech_cents_per_s: 600.0,
            speech_steady_share: 0.25,
            spoken_phrase_share: 0.6,
            low_confidence: 0.5,
            tuning_min_strength: 0.3,
            octave_flag_factor: 0.4,
            octave_context: octave::CONTEXT_RUN,
            octave_context_s: 2.0,
            octave_run_gap_s: 0.1,
            octave_isolated_max_s: 0.5,
            octave_choose: 1.0,
            octave_choose_spectrum: 16.0,
        }
    }
}

impl GuideConfig {
    /// The tuning readers use: `cents` when `strength >= tuning_min_strength`, else 0.
    pub fn use_cents(&self, cents: f64, strength: f64) -> f64 {
        if strength >= self.tuning_min_strength { cents } else { 0.0 }
    }
}

/// A pitch curve on a regular frame grid: frame `k` is centred on `k * hop_s` seconds.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PitchCurve {
    pub hop_s: f64,
    /// Pitch in Hz; 0 where the tracker says unvoiced.
    pub f0_hz: Vec<f32>,
    /// Voicing probability or tracker confidence, 0..1.
    pub conf: Vec<f32>,
    /// Frame level in dBFS (mono analysis signal, `window_s` long).
    pub level_db: Vec<f32>,
}

impl PitchCurve {
    pub fn len(&self) -> usize {
        self.f0_hz.len()
    }
    pub fn is_empty(&self) -> bool {
        self.f0_hz.is_empty()
    }
}

/// Whether a note is sung on a pitch or not (spoken, rapped, shouted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Pitched,
    Unpitched,
}

/// Result of the independent octave check of one note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OctaveCheck {
    /// The spectrum agrees with the tracked pitch.
    Ok,
    /// Strong energy halfway between the harmonics: the true pitch may be an octave lower.
    MaybeHigh,
    /// The odd harmonics are missing: the true pitch may be an octave higher.
    MaybeLow,
    /// Not checked (unpitched note, or too short).
    Unchecked,
}

/// One note of the guide.
#[derive(Clone, Debug, Serialize)]
pub struct Note {
    pub start_s: f64,
    pub end_s: f64,
    /// Pitch as measured, in MIDI units with fractions (69.0 = A440). Includes the
    /// recording's tuning: a song at A=436 Hz has its A4 notes near 68.84.
    /// Meaningless for unpitched notes (their median voiced pitch, or 0).
    pub midi: f64,
    /// 0..1. Below [`GuideConfig::low_confidence`] the note is drawn dim and not graded.
    pub confidence: f32,
    pub kind: NoteKind,
    pub octave: OctaveCheck,
    /// Time (s) at the start of the note spent gliding into it (scoop or slide), already
    /// included in `start_s`. The pitch curve shows the glide itself.
    pub glide_in_s: f64,
}

/// The melody guide of one vocal.
#[derive(Clone, Debug, Serialize)]
pub struct Guide {
    pub method: &'static str,
    pub version: &'static str,
    pub notes: Vec<Note>,
    /// The recording's tuning against A=440 Hz, in cents (-50..50): A=436 Hz is -15.8.
    pub tuning_cents: f64,
    /// How consistent the notes are about that tuning, 0..1 (the resultant length of the
    /// notes' offsets on the semitone circle). Below about 0.3 the estimate is a guess.
    pub tuning_strength: f64,
    /// Notes the octave check disagreed with.
    pub octave_flags: usize,
    pub curve: PitchCurve,
}

impl Guide {
    pub fn pitched(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter().filter(|n| n.kind == NoteKind::Pitched)
    }
}

/// Mono analysis signal shared by the stages: a downmix decimated to about 16 kHz.
pub struct Analysis {
    pub samples: Vec<f32>,
    pub sample_rate: f64,
}

/// Downmixes interleaved audio (`channels` channels) and decimates it to about 16 kHz
/// (integer factor, windowed-sinc low-pass), the rate both stages analyse.
pub fn prepare(interleaved: &[f32], channels: usize, sample_rate: u32) -> Analysis {
    let mono = resample::downmix(interleaved, channels.max(1));
    let (samples, rate) = resample::decimate_to_about(&mono, sample_rate as f64, 16_000.0);
    Analysis {
        samples,
        sample_rate: rate,
    }
}

/// Stage 1: the pYIN-style pitch curve of a prepared signal.
pub fn pitch_curve(a: &Analysis, cfg: &GuideConfig) -> PitchCurve {
    pyin::track(&a.samples, a.sample_rate, cfg)
}

/// Frame levels only (for a curve that came from another tracker).
pub fn levels(a: &Analysis, cfg: &GuideConfig, frames: usize) -> Vec<f32> {
    pyin::frame_levels(&a.samples, a.sample_rate, cfg, frames)
}

/// Stage 2: notes from a pitch curve (any tracker) plus the signal it came from (for the
/// level dips, already in `curve.level_db`, and the octave check).
pub fn segment(mut curve: PitchCurve, a: &Analysis, cfg: &GuideConfig) -> Guide {
    let mut notes = segment::notes(&curve, cfg);
    let mut flags = 0;
    let checker = octave::OctaveChecker::new(a.sample_rate);
    if cfg.octave_choose >= 1.0 {
        choose_octaves(&mut notes, &mut curve, &checker, &a.samples, cfg.octave_choose_spectrum);
    }
    for n in notes.iter_mut() {
        if n.kind != NoteKind::Pitched {
            continue;
        }
        n.octave = checker.check(&a.samples, n.start_s + n.glide_in_s, n.end_s, n.midi);
        if matches!(n.octave, OctaveCheck::MaybeHigh | OctaveCheck::MaybeLow) {
            flags += 1;
            n.confidence *= cfg.octave_flag_factor;
        }
    }
    // The second, independent look: the melody around each note (builder 0.2.0).
    for i in octave::context_check(&mut notes, cfg.octave_context, cfg.octave_context_s, cfg.octave_run_gap_s, cfg.octave_isolated_max_s) {
        flags += 1;
        notes[i].confidence *= cfg.octave_flag_factor;
    }
    let (tuning_cents, tuning_strength) = segment::tuning(&notes, cfg);
    Guide {
        method: METHOD,
        version: VERSION,
        notes,
        tuning_cents,
        tuning_strength,
        octave_flags: flags,
        curve,
    }
}

/// Octaves `choose_octaves` may move a note by (semitones).
const CHOOSE_SHIFTS: [f64; 5] = [-24.0, -12.0, 0.0, 12.0, 24.0];
/// Cost of moving a note at all (per unit weight); two octaves cost a little more.
const CHOOSE_MOVE: f64 = 20.0;
/// The same for the melody-only opinion (as jk-guide singable's SHIFT_COST).
const CHOOSE_MELODY_MOVE: f64 = 40.0;
const CHOOSE_TWO_OCTAVES: f64 = 1.15;
/// Leaps: a little per semitone, plus the square of the excess over a fifth.
const CHOOSE_FREE_LEAP: f64 = 7.0;
const CHOOSE_LEAP_SLOPE: f64 = 0.6;
/// Range: the square of the excess over 13 semitones from the local centre (weighted
/// median of the notes within 10 s).
const CHOOSE_FREE_RANGE: f64 = 13.0;
const CHOOSE_RANGE: f64 = 2.0;
const CHOOSE_WINDOW_S: f64 = 10.0;
/// Notes further than this from the song's main register do not count towards a local
/// centre; a window with less singing than CHOOSE_LOCAL_MIN_WEIGHT left uses the register.
const CHOOSE_REGISTER_SPAN: f64 = 15.0;
const CHOOSE_LOCAL_MIN_WEIGHT: f64 = 1.0;
/// Notes closer than this are fully linked; the link fades out by CHOOSE_LINK_FADE_S.
const CHOOSE_LINK_GAP_S: f64 = 0.6;
const CHOOSE_LINK_FADE_S: f64 = 2.5;
/// Nothing is moved outside a human voice (C2 .. C7).
const CHOOSE_VOICE: std::ops::RangeInclusive<f64> = 36.0..=96.0;

/// Builder 0.3.0 (owner, 2026-11-23, "Seasons - Acoustic Version": "sunlight" at F#2
/// under an E4 line): puts every pitched note in the octave its spectrum and the melody
/// round it agree on, and moves it there (with the pitch curve under it). The tracker
/// sometimes reads a whole run an octave or two off, often with high confidence (a bass
/// note leaking into the separated vocal under the singer makes the mix repeat at the
/// bass's period). Two Viterbi passes over the pitched notes each choose a shift of 0,
/// +-12 or +-24 semitones per note, and a note moves only where both choose the same one.
/// The first minimises: a move cost; the shortfall of the candidate's
/// spectral support (`OctaveChecker::support`, voice band only) against the best
/// candidate's; leaps between linked notes (a little per semitone, the square of the
/// excess over a fifth, fading out across rests); and distance beyond 13 semitones from
/// the local centre of the melody. The second uses the melody alone (no spectrum, a
/// dearer move). The spectral and phrase checks then judge the moved
/// notes at their new pitch, so a corrected note is not dimmed. Returns how many moved.
/// The same alignment without the spectrum runs in every reader (jk-guide singable), so
/// guides made by older builders are corrected too.
pub(crate) fn choose_octaves(notes: &mut [Note], curve: &mut PitchCurve, checker: &octave::OctaveChecker, x: &[f32], spectrum_cost: f64) -> usize {
    let idx: Vec<usize> = (0..notes.len()).filter(|&i| notes[i].kind == NoteKind::Pitched && notes[i].midi > 0.0).collect();
    let n = idx.len();
    if n < 3 {
        return 0;
    }
    let dur = |i: usize| notes[i].end_s - notes[i].start_s;
    let w: Vec<f64> = idx.iter().map(|&i| (dur(i) / 0.25).clamp(0.25, 2.0) * (0.4 + 0.6 * f64::from(notes[i].confidence).clamp(0.0, 1.0))).collect();
    let mid: Vec<f64> = idx.iter().map(|&i| 0.5 * (notes[i].start_s + notes[i].end_s)).collect();
    // Spectral support per candidate (dB, relative to the best candidate: <= 0).
    let spec: Vec<[f64; 5]> = idx
        .iter()
        .map(|&i| {
            let m = notes[i].midi;
            let cands: Vec<f64> = CHOOSE_SHIFTS.iter().map(|s| m + s).collect();
            let mut out = [0.0; 5];
            if let Some(v) = checker.support(x, notes[i].start_s + notes[i].glide_in_s, notes[i].end_s, &cands) {
                let best = v.iter().copied().filter(|x| x.is_finite()).fold(f64::NEG_INFINITY, f64::max);
                if best.is_finite() {
                    for k in 0..5 {
                        out[k] = if v[k].is_finite() { v[k] - best } else { -30.0 };
                    }
                }
            }
            out
        })
        .collect();
    let median = |mut v: Vec<(f64, f64)>| -> Option<f64> {
        v.sort_by(|p, q| p.0.total_cmp(&q.0));
        let total: f64 = v.iter().map(|p| p.1).sum();
        let mut acc = 0.0;
        for (m, wt) in &v {
            acc += wt;
            if acc >= total / 2.0 {
                return Some(*m);
            }
        }
        v.last().map(|p| p.0)
    };
    // The song's main register: the 15-semitone band holding the most singing, then the
    // weighted median of the notes in it (runs measured an octave or two off cannot move
    // it unless they are most of the song).
    let all: Vec<(f64, f64)> = (0..n).map(|b| (notes[idx[b]].midi, w[b])).collect();
    let mut band = (f64::NEG_INFINITY, 0.0);
    for lo in 24..100 {
        let lo = lo as f64;
        let s: f64 = all.iter().filter(|x| x.0 >= lo - 0.5 && x.0 < lo + 14.5).map(|x| x.1).sum();
        if s > band.0 {
            band = (s, lo);
        }
    }
    let register = median(all.iter().copied().filter(|x| x.0 >= band.1 - 0.5 && x.0 < band.1 + 14.5).collect()).unwrap_or(60.0);
    // Local centres (a sliding window over notes in time order), from the notes within
    // CHOOSE_REGISTER_SPAN of the main register only; too little there: the register.
    let mut centre = vec![register; n];
    let (mut lo, mut hi) = (0, 0);
    for a in 0..n {
        while lo < n && mid[lo] < mid[a] - CHOOSE_WINDOW_S {
            lo += 1;
        }
        while hi < n && mid[hi] <= mid[a] + CHOOSE_WINDOW_S {
            hi += 1;
        }
        let v: Vec<(f64, f64)> = (lo..hi).map(|b| (notes[idx[b]].midi, w[b])).filter(|x| (x.0 - register).abs() <= CHOOSE_REGISTER_SPAN).collect();
        if v.iter().map(|x| x.1).sum::<f64>() >= CHOOSE_LOCAL_MIN_WEIGHT {
            centre[a] = median(v).unwrap_or(register);
        }
    }
    // One Viterbi pass: the shift index (into CHOOSE_SHIFTS) chosen for every note.
    let decode = |spectrum: f64, move_cost: f64| -> Vec<usize> {
        let mut cost = vec![[f64::INFINITY; 5]; n];
        let mut back = vec![[2usize; 5]; n];
        for a in 0..n {
            let i = idx[a];
            for k in 0..5 {
                let shift = CHOOSE_SHIFTS[k];
                let p = notes[i].midi + shift;
                if shift != 0.0 && !CHOOSE_VOICE.contains(&p) {
                    continue;
                }
                let moved = if shift == 0.0 { 0.0 } else if shift.abs() > 12.0 { CHOOSE_TWO_OCTAVES } else { 1.0 };
                let far = ((p - centre[a]).abs() - CHOOSE_FREE_RANGE).max(0.0);
                let emit = w[a] * (move_cost * moved - spectrum * spec[a][k] + CHOOSE_RANGE * far * far);
                if a == 0 {
                    cost[0][k] = emit;
                    continue;
                }
                let j0 = idx[a - 1];
                let gap = notes[i].start_s - notes[j0].end_s;
                let link = if gap <= CHOOSE_LINK_GAP_S {
                    1.0
                } else if gap < CHOOSE_LINK_FADE_S {
                    (CHOOSE_LINK_FADE_S - gap) / (CHOOSE_LINK_FADE_S - CHOOSE_LINK_GAP_S)
                } else {
                    0.0
                };
                let lw = link * w[a].min(w[a - 1]);
                let mut best = f64::INFINITY;
                let mut arg = 2;
                for j in 0..5 {
                    if !cost[a - 1][j].is_finite() {
                        continue;
                    }
                    let d = (p - (notes[j0].midi + CHOOSE_SHIFTS[j])).abs();
                    let over = (d - CHOOSE_FREE_LEAP).max(0.0);
                    let c = cost[a - 1][j] + lw * (over * over + CHOOSE_LEAP_SLOPE * d);
                    if c < best - 1e-9 || (c <= best + 1e-9 && j == 2) {
                        best = c;
                        arg = j;
                    }
                }
                cost[a][k] = emit + best;
                back[a][k] = arg;
            }
        }
        let mut out = vec![2usize; n];
        let mut k = (0..5).min_by(|&p, &q| cost[n - 1][p].total_cmp(&cost[n - 1][q])).unwrap_or(2);
        for a in (0..n).rev() {
            out[a] = k;
            k = back[a][k];
        }
        out
    };
    // Two independent opinions, and a note moves only where they agree: the spectrum (with
    // the melody to settle near-ties) and the melody alone (the same costs as jk-guide
    // singable). Separated vocals with loud accompaniment can make the spectrum point the
    // wrong way ("Seasons": a guitar under the voice pulled a correct Bb4 down to Bb3);
    // clean voices make the melody wrong at real leaps (a tenor's high G4). Where they
    // disagree the note stays as tracked, for the checks below and every reader's
    // melody-only alignment.
    let by_spectrum = decode(spectrum_cost, CHOOSE_MOVE);
    let by_melody = decode(0.0, CHOOSE_MELODY_MOVE);
    let mut moved = 0;
    for a in 0..n {
        if by_spectrum[a] != by_melody[a] {
            continue;
        }
        let shift = CHOOSE_SHIFTS[by_spectrum[a]];
        if shift != 0.0 {
            let i = idx[a];
            notes[i].midi += shift;
            // The curve under the note moves with it.
            let ratio = 2f64.powf(shift / 12.0) as f32;
            let f0 = (notes[i].start_s / curve.hop_s).floor().max(0.0) as usize;
            let f1 = ((notes[i].end_s / curve.hop_s).ceil() as usize).min(curve.f0_hz.len());
            for hz in curve.f0_hz[f0.min(f1)..f1].iter_mut() {
                if *hz > 0.0 {
                    *hz *= ratio;
                }
            }
            moved += 1;
        }
    }
    moved
}

/// Both stages on interleaved audio at any sample rate.
pub fn build(interleaved: &[f32], channels: usize, sample_rate: u32, cfg: &GuideConfig) -> Guide {
    let a = prepare(interleaved, channels, sample_rate);
    let curve = pitch_curve(&a, cfg);
    segment(curve, &a, cfg)
}

/// MIDI pitch (with fractions) of a frequency.
pub fn hz_to_midi(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// Frequency of a MIDI pitch.
pub fn midi_to_hz(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A harmonic tone (8 harmonics, 1/h amplitudes) following `midi_at(t)`, with silence
    /// where it returns None.
    fn tone(seconds: f64, midi_at: impl Fn(f64) -> Option<f64>) -> Vec<f32> {
        let n = (seconds * SR as f64) as usize;
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / SR as f64;
            match midi_at(t) {
                Some(m) => {
                    let f = midi_to_hz(m);
                    phase += f / SR as f64;
                    let mut s = 0.0;
                    for h in 1..=8 {
                        if f * h as f64 > 7000.0 {
                            break;
                        }
                        s += (2.0 * std::f64::consts::PI * phase * h as f64).sin() / h as f64;
                    }
                    out.push((0.25 * s) as f32);
                }
                None => out.push(0.0),
            }
        }
        out
    }

    fn pitched(g: &Guide) -> Vec<&Note> {
        g.pitched().collect()
    }

    #[test]
    fn steady_tones_across_the_range_are_one_accurate_note_each() {
        let cfg = GuideConfig::default();
        for &m in &[40.0, 45.5, 52.0, 60.0, 69.0, 76.25, 84.0] {
            let x = tone(1.2, |t| {
                if (0.2..1.0).contains(&t) {
                    Some(m)
                } else {
                    None
                }
            });
            let g = build(&x, 1, SR, &cfg);
            let p = pitched(&g);
            assert_eq!(p.len(), 1, "midi {m}: {:?}", g.notes);
            let err_cents = (p[0].midi - m) * 100.0;
            assert!(err_cents.abs() < 5.0, "midi {m}: error {err_cents} cents");
            assert!(
                (p[0].start_s - 0.2).abs() < 0.04,
                "midi {m}: start {}",
                p[0].start_s
            );
            assert!(
                (p[0].end_s - 1.0).abs() < 0.05,
                "midi {m}: end {}",
                p[0].end_s
            );
            assert_eq!(p[0].octave, OctaveCheck::Ok, "midi {m}");
        }
    }

    #[test]
    fn vibrato_stays_one_note() {
        let cfg = GuideConfig::default();
        let x = tone(2.5, |t| {
            if (0.2..2.2).contains(&t) {
                Some(64.0 + 0.5 * (2.0 * std::f64::consts::PI * 5.5 * t).sin())
            } else {
                None
            }
        });
        let g = build(&x, 1, SR, &cfg);
        let p = pitched(&g);
        assert_eq!(p.len(), 1, "{:?}", g.notes);
        assert!((p[0].midi - 64.0).abs() < 0.1, "{}", p[0].midi);
    }

    #[test]
    fn a_legato_scale_is_one_note_per_step() {
        let cfg = GuideConfig::default();
        let steps = [60.0, 62.0, 64.0, 65.0, 67.0, 69.0];
        let x = tone(1.6, |t| {
            if (0.1..1.3).contains(&t) {
                Some(steps[(((t - 0.1) / 0.2) as usize).min(5)])
            } else {
                None
            }
        });
        let g = build(&x, 1, SR, &cfg);
        let p = pitched(&g);
        assert_eq!(p.len(), steps.len(), "{:?}", g.notes);
        for (n, s) in p.iter().zip(steps) {
            assert!((n.midi - s).abs() < 0.1, "{} vs {s}", n.midi);
        }
    }

    #[test]
    fn silence_and_noise_give_no_pitched_notes() {
        let cfg = GuideConfig::default();
        let g = build(&vec![0.0; SR as usize], 1, SR, &cfg);
        assert!(g.notes.is_empty());
        // Deterministic white noise.
        let mut s = 0x1234_5678u32;
        let noise: Vec<f32> = (0..SR as usize)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 - 0.5) * 0.2
            })
            .collect();
        let g = build(&noise, 1, SR, &cfg);
        assert_eq!(g.pitched().count(), 0, "{:?}", g.notes);
    }

    #[test]
    fn tuning_offset_is_measured() {
        let cfg = GuideConfig::default();
        // A=436 Hz: every note 15.86 cents flat.
        let off = 12.0 * (436.0f64 / 440.0).log2();
        let steps = [57.0, 60.0, 64.0, 62.0, 65.0, 67.0];
        let x = tone(3.4, |t| {
            let i = (t / 0.5) as usize;
            if i < steps.len() && (t - i as f64 * 0.5) < 0.4 {
                Some(steps[i] + off)
            } else {
                None
            }
        });
        let g = build(&x, 1, SR, &cfg);
        assert!(
            (g.tuning_cents - off * 100.0).abs() < 2.0,
            "{}",
            g.tuning_cents
        );
    }

    #[test]
    fn any_sample_rate_gives_the_same_pitch() {
        let cfg = GuideConfig::default();
        for &sr in &[16_000u32, 22_050, 44_100, 48_000, 96_000] {
            let n = (1.0 * sr as f64) as usize;
            let f = midi_to_hz(57.3);
            let x: Vec<f32> = (0..n)
                .map(|i| {
                    let t = i as f64 / sr as f64;
                    let mut s = 0.0;
                    for h in 1..=6 {
                        s += (2.0 * std::f64::consts::PI * f * h as f64 * t).sin() / h as f64;
                    }
                    if (0.2..0.8).contains(&t) {
                        (0.2 * s) as f32
                    } else {
                        0.0
                    }
                })
                .collect();
            let g = build(&x, 1, sr, &cfg);
            let p = pitched(&g);
            assert_eq!(p.len(), 1, "sr {sr}: {:?}", g.notes);
            assert!((p[0].midi - 57.3).abs() < 0.05, "sr {sr}: {}", p[0].midi);
        }
    }
}

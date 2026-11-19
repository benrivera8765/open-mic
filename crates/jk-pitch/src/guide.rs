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
//!    checks every note's octave on the spectrum by a second, independent method.
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

use serde::Serialize;

/// What made a guide: stored with it so a better method can replace old guides later.
pub const METHOD: &str = "jk-pitch pyin+notes";
/// Bumped whenever the builder's output can change for the same input.
pub const VERSION: &str = "0.1.0";

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
        }
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
pub fn segment(curve: PitchCurve, a: &Analysis, cfg: &GuideConfig) -> Guide {
    let mut notes = segment::notes(&curve, cfg);
    let mut flags = 0;
    let checker = octave::OctaveChecker::new(a.sample_rate);
    for n in notes.iter_mut() {
        if n.kind != NoteKind::Pitched {
            continue;
        }
        n.octave = checker.check(&a.samples, n.start_s + n.glide_in_s, n.end_s, n.midi);
        if matches!(n.octave, OctaveCheck::MaybeHigh | OctaveCheck::MaybeLow) {
            flags += 1;
            n.confidence *= 0.4;
        }
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

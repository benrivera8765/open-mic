// SPDX-License-Identifier: GPL-3.0-or-later
//! Grading a performance against the melody guide (WORK_ORDER-2 4.7 "The grade", 4.6
//! "Octaves" and "Notes already sung", docs/APP_API.md 12.1 and 12.6).
//!
//! The scorer is fed the singer's pitch one tracker value at a time (one every 10 ms), each
//! already placed at the **song time that was playing when it was sung** and tagged with
//! its playback segment ([`SingerSample`]; [`crate::practice`] does that mapping). It
//! compares in cents with the guide's real, unrounded pitch, never snapping the singer to
//! a piano key:
//!
//! ```text
//! target(note, t) = note.midi + key_at(t, segment) + 12 * octave_shift
//! error_cents     = 100 * (singer(t) - target(note, t))      folded into -600..600 unless strict
//! ```
//!
//! # Per-note credit
//!
//! A graded note is split into moments, one per hop (10 ms), after its `glide_in` (a scoop
//! the guide itself has is never punished). Each moment earns a credit from the strictness
//! bands ([`Strictness::bands`]: by default full credit within 50 cents, falling linearly to
//! nothing at 100 cents). The note's credit is the mean over the moments the take covered:
//! the share of its length the singer was on pitch.
//!
//! - **Vibrato and wobble.** The singer's pitch at a moment is the median of their voiced
//!   values within `smooth_s` (80 ms) either side: about one vibrato period at 5.5-6 Hz,
//!   so a vibrato around the note is judged by its centre, not by its peaks. A moment where
//!   the singer is silent earns nothing.
//! - **Timing** (scorer 0.2.0). Being a little early or late, a delay the system could not
//!   measure, a scoop or a fall at the ends of the singer's note and the tracker's own
//!   onset delay cost no pitch points. The singer's *rendition* of a note may start and end
//!   up to the strictness's timing tolerance (150 ms by default) away from the guide's
//!   note, the two ends independently, and be as short as half the note (`min_rendition`);
//!   moment `k` of the note is compared with the singer at the same relative place in the
//!   rendition. The renditions of consecutive pitched notes keep their order and do not
//!   overlap (by more than `overlap_s`, 10 ms), so one stretch of singing is credited to
//!   one note only: a note cannot take its neighbour's singing. Each pitched note's timing
//!   is decided together with the next pitched note's (when their renditions could meet),
//!   choosing what gives the two the most credit, so one note cannot take the start of the
//!   next one's rendition unless that gains more than the next note loses. Ties: the
//!   least stretch, then the timing closest to the previous note's. Per-note timing that
//!   differs by up to the tolerance from note to note (rubato) thus costs nothing, while
//!   a part of a note's middle that was not sung is never forgiven. (Scorer 0.1.0 matched
//!   each note by one shift and, once a "usual timing" was known, only within 80 ms of it,
//!   which made rubato cost points.) The finished-note entry of a note that waits for the
//!   next one comes out with it.
//! - **What the tracker could not resolve** (the excusal rule, 0.2.0). A moment where the
//!   tracker named no pitch while the input was **periodic** sound (tracker clarity at least
//!   `excuse_clarity`, 0.7, level at least `excuse_dbfs`, -50 dBFS, not returned music)
//!   is excused, up to a quarter of the note's moments (`excuse_share`): it neither earns
//!   nor costs. Such frames are a voice the tracker heard but did not commit to (a low or
//!   rough voice, an onset), which is not the singer's fault: on the original vocals of the
//!   development songs 54-64 % of the frames where the guide's own curve is voiced but the
//!   tracker named no pitch have clarity 0.7 or more. Breath, whispers, consonants and noise
//!   stay below it (generated breath, fricatives and speech-shaped noise in the middle of
//!   notes never score more than silence there, docs/benchmarks/practice-score.md), so
//!   they are never excused, nor is silence; a note with no pitch named at all scores 0,
//!   and a note whose middle half was not resolved earns at most about two thirds.
//!   (Scorer 0.1.0 excused any sound of -50 dBFS or more.) Tracker dropouts of up to 40 ms
//!   between voiced values are bridged (`bridge_s`).
//! - **Octaves.** In `auto` and `fixed` a pitch a whole number of octaves from the target is
//!   on the target; `strict` requires the written octave. `auto` also decides, from the
//!   first sung phrases (`settle_s` of voiced singing near the pitch classes of graded
//!   notes), which octave the singer uses, so the lane can be moved there.
//! - **Key changes.** Each moment's target uses the key that was heard at that moment in
//!   that segment ([`KeyTimeline`], the engine's `key_at` history, DECISIONS D-239), so a
//!   note sung before a key change keeps the key it was sung in.
//! - **Seeks.** A note sung again in a later segment (after a seek) keeps its latest
//!   attempt, once that attempt covers at least half of it.
//!
//! # What is not graded
//!
//! Unpitched notes, notes below the guide's `low_confidence`, octave-flagged notes and
//! notes whose midpoint lies in a span the singer excluded (SONG_FORMAT 10.1's rule), and
//! notes during which the input could not be judged for at least half of the covered
//! moments ([`SingerSample::unknown`]: the echo stage had not decided yet, or its removal
//! of returned music was unusable). A moment the system could not judge neither earns nor
//! costs anything, in any note. The result says "graded on X of Y notes".
//!
//! Deterministic: the same samples give the same result on every run and on every
//! system (no hashing, no threads, IEEE arithmetic only and `libm` for logarithms).

use serde::{Deserialize, Serialize};

/// The scorer's name and version, stored with every result (SONG_FORMAT section 11
/// `scorer`), so a later change never makes old and new results look comparable.
pub const SCORER: &str = "jk-pitch score";
pub const SCORER_VERSION: &str = "0.2.0";

/// How close is close enough (APP_API 12.6, owner decision 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strictness {
    Relaxed,
    Standard,
    Strict,
}

/// The bands of one strictness setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bands {
    /// Full credit within this many cents of the target.
    pub full_cents: f64,
    /// No credit from this many cents on (linear in between).
    pub zero_cents: f64,
    /// How far (s) a note may be matched with the singer's pitch from earlier or later.
    pub tolerance_s: f64,
}

impl Strictness {
    /// The bands. `standard` is fixed by WO2 4.7 (50 / 100 cents).
    pub fn bands(self) -> Bands {
        match self {
            Strictness::Relaxed => Bands { full_cents: 75.0, zero_cents: 150.0, tolerance_s: 0.200 },
            Strictness::Standard => Bands { full_cents: 50.0, zero_cents: 100.0, tolerance_s: 0.150 },
            Strictness::Strict => Bands { full_cents: 30.0, zero_cents: 60.0, tolerance_s: 0.100 },
        }
    }

    /// The name used in settings and results.
    pub fn name(self) -> &'static str {
        match self {
            Strictness::Relaxed => "relaxed",
            Strictness::Standard => "standard",
            Strictness::Strict => "strict",
        }
    }

    /// Parses a stored name (`None` for anything else).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "relaxed" => Some(Strictness::Relaxed),
            "standard" => Some(Strictness::Standard),
            "strict" => Some(Strictness::Strict),
            _ => None,
        }
    }
}

impl Bands {
    /// Credit of a moment `cents` away from the target (sign ignored).
    pub fn credit(&self, cents: f64) -> f64 {
        let e = cents.abs();
        if e <= self.full_cents {
            1.0
        } else if e >= self.zero_cents {
            0.0
        } else {
            (self.zero_cents - e) / (self.zero_cents - self.full_cents)
        }
    }

    /// Error (cents) up to which a moment counts as "on the note" for the lit bar: where
    /// the credit is at least one half.
    pub fn lit_cents(&self) -> f64 {
        0.5 * (self.full_cents + self.zero_cents)
    }
}

/// The lane's octave choice (SONG_FORMAT section 11 `octave.mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OctaveMode {
    /// Found from the first sung phrases; octaves forgiven.
    Auto,
    /// The song's saved shift; octaves forgiven.
    Fixed,
    /// The written octave is shown and required.
    Strict,
}

impl OctaveMode {
    pub fn name(self) -> &'static str {
        match self {
            OctaveMode::Auto => "auto",
            OctaveMode::Fixed => "fixed",
            OctaveMode::Strict => "strict",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(OctaveMode::Auto),
            "fixed" => Some(OctaveMode::Fixed),
            "strict" => Some(OctaveMode::Strict),
            _ => None,
        }
    }

    /// Whether a pitch a whole number of octaves from the target counts as on it.
    pub fn folds(self) -> bool {
        self != OctaveMode::Strict
    }
}

/// One note of the guide as the scorer sees it (from SONG_FORMAT 10.1's `notes`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreNote {
    pub start: f64,
    pub end: f64,
    /// Pitch as measured (MIDI with fractions, the recording's tuning included).
    pub midi: f64,
    /// `kind == "pitched"`.
    pub pitched: bool,
    pub confidence: f64,
    /// The guide's octave check flagged it (`maybe_high`, `maybe_low` or unknown values).
    pub octave_flagged: bool,
    /// Seconds of glide into the note at its start (never punished).
    pub glide_in: f64,
    /// The lyrics line of the note's word reference, when it has one.
    pub line: Option<i64>,
}

impl ScoreNote {
    pub fn midpoint(&self) -> f64 {
        0.5 * (self.start + self.end)
    }
}

/// A row of the line-by-line breakdown: a lyrics line, or a phrase of the guide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineSpan {
    pub line: i64,
    pub start: f64,
    pub end: f64,
}

/// What the rows of the breakdown are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineBasis {
    Lyrics,
    Phrases,
}

/// The guide, prepared for grading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreGuide {
    /// Ordered by start, not overlapping.
    pub notes: Vec<ScoreNote>,
    /// The guide's `low_confidence` (0.5 in 1.1).
    pub low_confidence: f64,
    /// Rows of the breakdown, ordered by start.
    pub lines: Vec<LineSpan>,
    pub basis: LineBasis,
    /// The guide is `outdated` (made by an older builder): the result says the guide is
    /// uncertain.
    pub outdated: bool,
    /// Song length (s).
    pub duration_s: f64,
    /// "method version" of the guide (SONG_FORMAT 11 `guide`).
    pub label: String,
}

impl ScoreGuide {
    /// SONG_FORMAT 10.1's rule before exclusions: pitched, confident, octave not flagged.
    pub fn gradable(&self, i: usize) -> bool {
        let n = &self.notes[i];
        n.pitched && n.confidence >= self.low_confidence && !n.octave_flagged
    }

    /// Phrases for the breakdown when the song has no lyrics: the notes cut at gaps of 1 s
    /// or more (SONG_FORMAT 11 `lines.basis` "phrases"), numbered from 0.
    pub fn phrases(notes: &[ScoreNote]) -> Vec<LineSpan> {
        let mut out: Vec<LineSpan> = Vec::new();
        for n in notes {
            match out.last_mut() {
                Some(p) if n.start - p.end < 1.0 => p.end = p.end.max(n.end),
                _ => out.push(LineSpan { line: out.len() as i64, start: n.start, end: n.end }),
            }
        }
        out
    }
}

/// Whether `t` lies inside one of the excluded spans.
fn in_spans(spans: &[(f64, f64)], t: f64) -> bool {
    spans.iter().any(|&(a, b)| t >= a && t <= b)
}

// ---------------------------------------------------------------------------------------
// Key timeline

/// One playback segment's keys: `(song time s, semitones)` heard from each time on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeySeg {
    pub segment: u64,
    pub keys: Vec<(f64, i32)>,
}

/// Which key was heard at each song time, per playback segment: the engine's `key_at`
/// history (DECISIONS D-239: a change counts from the midpoint of its crossfade; in
/// render-ahead mode from the crossover onto the cached key), handed to the scorer as the
/// engine publishes it. Bounded like the engine's (64 segments, 1024 changes each).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyTimeline {
    pub segs: Vec<KeySeg>,
}

const MAX_KEY_SEGMENTS: usize = 64;
const MAX_KEY_MARKS: usize = 1024;

impl KeyTimeline {
    /// One segment (0) in one key throughout.
    pub fn constant(key: i32) -> Self {
        let mut t = Self::default();
        t.start_segment(0, 0.0, key);
        t
    }

    /// A segment starts at `start_s` in `key` (a load, seek, device switch or re-render).
    pub fn start_segment(&mut self, segment: u64, start_s: f64, key: i32) {
        if let Some(s) = self.segs.iter_mut().find(|s| s.segment == segment) {
            s.keys.clear();
            s.keys.push((start_s, key));
            return;
        }
        if self.segs.len() >= MAX_KEY_SEGMENTS {
            self.segs.remove(0);
        }
        self.segs.push(KeySeg { segment, keys: vec![(start_s, key)] });
    }

    /// From `at_s` on, `key` is heard in `segment` (the segment is created if unknown).
    /// Changes normally arrive in time order; one at or before an existing change replaces
    /// the later ones (the engine re-scheduled it).
    pub fn change(&mut self, segment: u64, at_s: f64, key: i32) {
        let Some(s) = self.segs.iter_mut().find(|s| s.segment == segment) else {
            self.start_segment(segment, at_s, key);
            return;
        };
        while s.keys.len() > 1 && s.keys.last().is_some_and(|&(t, _)| t >= at_s) {
            s.keys.pop();
        }
        if s.keys.last().is_some_and(|&(_, k)| k == key) {
            return;
        }
        if s.keys.len() >= MAX_KEY_MARKS {
            s.keys.remove(1);
        }
        s.keys.push((at_s, key));
    }

    /// The key heard at song time `t` in `segment` (the newest segment when `None` or
    /// unknown); 0 when nothing is known.
    pub fn key_at(&self, t: f64, segment: Option<u64>) -> i32 {
        let seg = segment
            .and_then(|id| self.segs.iter().find(|s| s.segment == id))
            .or_else(|| self.segs.last());
        let Some(seg) = seg else { return 0 };
        let mut key = seg.keys.first().map(|&(_, k)| k).unwrap_or(0);
        for &(at, k) in &seg.keys {
            if at <= t + 1e-9 {
                key = k;
            } else {
                break;
            }
        }
        key
    }

    /// The same timeline with every key moved by `by` semitones (WO2 section 9: a guide
    /// "shifted by a different amount").
    pub fn shifted(&self, by: i32) -> Self {
        KeyTimeline {
            segs: self
                .segs
                .iter()
                .map(|s| KeySeg { segment: s.segment, keys: s.keys.iter().map(|&(t, k)| (t, k + by)).collect() })
                .collect(),
        }
    }

    /// Key changes heard in `segment` between `from` and `to` (s).
    pub fn changes_between(&self, segment: u64, from: f64, to: f64) -> u32 {
        self.segs
            .iter()
            .find(|s| s.segment == segment)
            .map(|s| s.keys.iter().skip(1).filter(|&&(t, _)| t > from && t <= to).count() as u32)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------------------
// Note names

/// The name of a piano key (MIDI note number, rounded by the caller) with its octave:
/// `C4` is 60. `flats` spells the black keys as flats (D♭ written `Db`), else sharps; the
/// caller picks the spelling of the key that is heard (APP_API 12.1).
pub fn note_name(note: i32, flats: bool) -> String {
    const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    const FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
    let pc = note.rem_euclid(12) as usize;
    let name = if flats { FLATS[pc] } else { SHARPS[pc] };
    format!("{name}{}", note.div_euclid(12) - 1)
}

/// MIDI note number with fractions for a frequency (A4 = 69 = 440 Hz), with `libm` so it
/// is identical on every system.
pub fn hz_to_midi(hz: f64) -> f64 {
    69.0 + 12.0 * libm::log2(hz / 440.0)
}

/// `cents` folded into -600..600 (the nearest octave of the target).
pub fn fold_cents(cents: f64) -> f64 {
    cents - 1200.0 * (cents / 1200.0).round()
}

// ---------------------------------------------------------------------------------------
// Scorer

/// Scorer settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreConfig {
    pub strictness: Strictness,
    pub octave_mode: OctaveMode,
    /// The octave shift (-2..2): `fixed` uses it; `auto` starts from it (the one found last
    /// time) until it has decided; `strict` ignores it (0).
    pub octave_shift: i32,
    /// Spacing of the moments a note is judged at (the tracker's hop).
    pub hop_s: f64,
    /// Half-width of the median that judges a moment by the centre of a vibrato.
    pub smooth_s: f64,
    /// A tracker dropout inside a sung line is bridged when voiced values lie within this
    /// time before and after it (0: never). Short (40 ms): longer gaps are left to the
    /// excusal rule below, which needs the singer to be making sound.
    pub bridge_s: f64,
    /// The excusal rule (see the module documentation, "What the tracker could not
    /// resolve"): a moment of a note where the tracker named no pitch while the singer made
    /// **periodic** sound (input level at least `excuse_dbfs`, tracker clarity at least
    /// `excuse_clarity`, not returned music) neither earns nor costs, up to `excuse_share`
    /// of the note's judged moments. Silence, breath, whispers and other unvoiced sound
    /// are never excused, a note with no pitch named at all scores 0, and a note half sung
    /// earns at most two thirds. `excuse_share` 0 turns the rule off.
    pub excuse_dbfs: f32,
    pub excuse_share: f64,
    /// Lowest tracker clarity of an excused moment (0 = any sound, scorer 0.1.0's rule).
    pub excuse_clarity: f32,
    /// Voiced singing near graded notes' pitch classes needed before `auto` decides the
    /// octave.
    pub settle_s: f64,
    /// Moments within this many cents of the target (folded) count toward the sharp/flat
    /// tendency (a semitone flat is still "flat"; a different note is not a tendency).
    pub tendency_window_cents: f64,
    /// Tendency needs at least this much singing near the targets (s), else `null`.
    pub tendency_min_s: f64,
    /// Per line: the same.
    pub line_tendency_min_s: f64,
    /// Below this input level (dBFS) during a graded note a moment is "too quiet for a
    /// reliable pitch" (the tracker's level gate to start a note), and from
    /// `sound_dbfs` on there was some sound.
    pub quiet_dbfs: f32,
    pub sound_dbfs: f32,
    /// Keep the per-note outcomes in the result (diagnostics, the harness).
    pub detail: bool,
    /// Timing (see the module documentation, "Timing"): the singer's rendition of a note
    /// may start and end up to the strictness's timing tolerance away from the guide's,
    /// independently, and be as short as this share of the note's judged length; it is
    /// stretched onto the note. Renditions of consecutive graded notes must not overlap
    /// by more than `overlap_s`, so one stretch of singing is credited to one note only.
    pub min_rendition: f64,
    pub overlap_s: f64,
    /// Scorer 0.1.0 only ("usual timing"); ignored since 0.2.0, kept so settings that
    /// name them still build.
    pub jitter_s: f64,
    pub lag_memory_s: f64,
    pub lag_evidence_s: f64,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        Self {
            strictness: Strictness::Standard,
            octave_mode: OctaveMode::Auto,
            octave_shift: 0,
            hop_s: 0.010,
            smooth_s: 0.080,
            bridge_s: 0.040,
            excuse_dbfs: -50.0,
            excuse_share: 0.25,
            excuse_clarity: 0.7,
            settle_s: 3.0,
            tendency_window_cents: 150.0,
            tendency_min_s: 1.0,
            line_tendency_min_s: 0.5,
            quiet_dbfs: -55.0,
            sound_dbfs: -70.0,
            detail: false,
            min_rendition: 0.5,
            overlap_s: 0.010,
            jitter_s: 0.080,
            lag_memory_s: 20.0,
            lag_evidence_s: 0.5,
        }
    }
}

/// One tracker value at the song time it was sung.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingerSample {
    /// Song time (s) that was playing when this was sung.
    pub t: f64,
    /// Playback segment of that song time (a seek starts a new one).
    pub segment: u64,
    /// The pitch sung (MIDI with fractions), `None` for no pitch (silence, breath, noise,
    /// or a pitch the echo stage could not trust).
    pub midi: Option<f64>,
    /// Level of the analysed input (dBFS).
    pub level_db: f32,
    /// The input could not be judged at this moment: the echo stage had not decided yet
    /// ([`crate::echo::Trust::Unknown`]) or its removal of returned music was unusable.
    /// Such a moment neither earns nor costs credit.
    pub unknown: bool,
    /// The echo stage said the input held only returned music here
    /// ([`crate::echo::Trust::Music`]): any pitch was dropped.
    pub music: bool,
    /// The tracker's clarity for this value (MPM NSDF at its best period, 0..1, 1 =
    /// perfectly periodic), given also when it named no pitch
    /// ([`crate::tracker::PitchEvent::clarity`]); 0 when unknown. Added in scorer 0.2.0.
    #[serde(default)]
    pub clarity: f32,
}

const F_UNKNOWN: u8 = 1;
const F_MUSIC: u8 = 2;

/// A stored sample (NaN = no pitch).
#[derive(Clone, Copy, Debug)]
struct Stored {
    t: f64,
    midi: f32,
    level: f32,
    clarity: f32,
    flags: u8,
}

/// A run of samples from one playback segment, in push order.
#[derive(Clone, Copy, Debug)]
struct Run {
    segment: u64,
    begin: usize,
    end: usize,
    t0: f64,
    t1: f64,
}

/// The outcome of one note in the take (its latest attempt).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteOutcome {
    pub note: usize,
    /// Share of the note's length on pitch, 0..1 (computed for every note, used only for
    /// graded ones).
    pub credit: f64,
    /// Graded when finished: SONG_FORMAT 10.1's rule and the input could be judged.
    pub graded: bool,
    /// The input could not be judged for most of the note (not graded).
    pub unknown: bool,
    /// Key heard at the note's midpoint.
    pub key: i32,
    /// Mean folded error of the moments near the target (cents), if any.
    pub mean_err: Option<f64>,
    /// The timing used (s, positive: the singer was late): the mean of `start_shift_s` and
    /// `end_shift_s` since 0.2.0.
    pub delta_s: f64,
    /// Covered length (s) of the judged part.
    pub weight_s: f64,
    /// Segment of the attempt.
    pub segment: u64,
    /// Judged moments (covered by the take and judgeable) and those of them where the
    /// singer had a pitch (diagnostics: how much of the note the tracker heard).
    #[serde(default)]
    pub covered: u32,
    #[serde(default)]
    pub voiced: u32,
    /// Moments excused: no pitch named while the singer was making periodic sound
    /// (`ScoreConfig::excuse_share`).
    #[serde(default)]
    pub excused: u32,
    /// Where the singer's rendition of the note started and ended relative to the guide
    /// (s, positive: later; see "Timing"). `delta_s` is their mean. Added in 0.2.0.
    #[serde(default)]
    pub start_shift_s: f64,
    #[serde(default)]
    pub end_shift_s: f64,
    #[serde(skip)]
    sound: u32,
    #[serde(skip)]
    quiet: u32,
    #[serde(skip)]
    errs: (usize, usize),
}

/// Live output since the last drain (APP_API 12.5 `practice://live`'s `hits` and `done`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreLive {
    /// `[note, from_s, to_s, where]`: 0 on pitch (lit), 1 above, -1 below.
    pub hits: Vec<(usize, f64, f64, i8)>,
    /// `[note, credit (null: not graded), key heard, mean error cents]`.
    pub done: Vec<(usize, Option<f64>, i32, Option<f64>)>,
}

/// The running state of a take (APP_API 12.5 `PracticeState.take`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeProgress {
    /// Share of the song's graded note time covered so far.
    pub covered: f64,
    pub graded: u32,
    pub total: u32,
    /// Running score of the notes finished so far.
    pub overall: Option<f64>,
    pub seeks: u32,
    /// The octave shift in use and whether `auto` has decided.
    pub octave_shift: i32,
    pub octave_settled: bool,
}

/// What the caller adds when the take ends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FinishContext {
    /// The take ran to the end of the song.
    pub completed: bool,
    /// Trust flags known to the caller (SONG_FORMAT 11 `trust`): `music_in_input`,
    /// `removal_active` | `removal_marginal` | `untrusted`, `latency_unmeasured`,
    /// `clipping`, `input_lost`, `dropouts`, `bluetooth_input`. `untrusted` makes
    /// `overall` null.
    pub flags: Vec<String>,
}

/// A graded take: SONG_FORMAT section 11's result without `date` and `app` (the app adds
/// them), plus diagnostics that are not stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoreResult {
    pub key: i32,
    pub key_changes: u32,
    pub seeks: u32,
    pub octave: i32,
    pub octave_mode: OctaveMode,
    pub strictness: Strictness,
    /// 0..100, one decimal; `None` when nothing could be graded or the take is untrusted.
    pub overall: Option<f64>,
    pub grade: Option<String>,
    pub graded: u32,
    pub total: u32,
    /// Whole cents, positive = sharp.
    pub tendency_cents: Option<f64>,
    /// Two decimals.
    pub sung_share: f64,
    pub voiced_share: f64,
    pub completed: bool,
    pub trust: Vec<String>,
    pub guide: String,
    pub scorer: String,
    pub lines: LinesResult,
    // ---- diagnostics (not stored in the song) ----
    /// Notes in the take not graded because the input could not be judged.
    pub unknown_notes: u32,
    /// Per-note outcomes (only with `ScoreConfig::detail`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteOutcome>,
}

/// SONG_FORMAT 11 `lines`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinesResult {
    pub basis: LineBasis,
    /// `[line, start (1 decimal), score (whole, null: nothing graded), graded, tendency (whole)]`.
    pub rows: Vec<LineRow>,
}

/// One row of the breakdown: `[line, start, score, graded, tendency]`.
pub type LineRow = (i64, f64, Option<f64>, u32, Option<f64>);

/// The letter for an overall score (APP_API 12.6, owner decision 3).
pub fn grade_letter(overall: f64) -> &'static str {
    match overall {
        x if x >= 95.0 => "A+",
        x if x >= 90.0 => "A",
        x if x >= 85.0 => "B+",
        x if x >= 80.0 => "B",
        x if x >= 75.0 => "C+",
        x if x >= 70.0 => "C",
        x if x >= 60.0 => "D",
        _ => "F",
    }
}

fn round_to(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Median of a small slice (sorted in place; NaN-free input). Even lengths take the mean
/// of the two middle values.
fn median_in_place(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) }
}

/// Grades one take. See the module documentation.
pub struct Scorer {
    cfg: ScoreConfig,
    guide: ScoreGuide,
    /// SONG_FORMAT 10.1's rule without exclusions, per note.
    gradable: Vec<bool>,
    excluded: Vec<(f64, f64)>,
    /// Index into `guide.lines` per note.
    note_line: Vec<Option<usize>>,
    keys: KeyTimeline,
    samples: Vec<Stored>,
    runs: Vec<Run>,
    outcomes: Vec<Option<NoteOutcome>>,
    /// Folded errors of each note's moments near the target: a fixed slot per note
    /// (`err_slot[i]..err_slot[i + 1]`) in `errs`, overwritten by a later attempt.
    err_slot: Vec<usize>,
    errs: Vec<f64>,
    /// Next note to finish in the newest run.
    cursor: usize,
    /// Note the newest sample fell in (live hits, octave votes).
    live_note: usize,
    live: ScoreLive,
    /// Octave votes for shifts -2..=2 (voiced hops).
    votes: [u32; 5],
    settled: Option<i32>,
    shift: i32,
    strictness: Strictness,
    octave_mode: OctaveMode,
    /// Scratch for one note's judgement (sized at construction for the longest note).
    sc_val: Vec<f64>,
    sc_cov: Vec<u8>,
    sc_med: Vec<f64>,
    /// Target (MIDI, key heard) of each moment of the note being judged.
    sc_tgt: Vec<f64>,
    /// The next note's best credit per earliest start shift (resolution scratch).
    sc_next: Vec<f64>,
    /// Fast table filling: prefix sums of position credits and excusable positions, and
    /// the positions skipped or landed on twice under one stretch.
    sc_csum: Vec<f64>,
    sc_esum: Vec<u32>,
    sc_corr: Vec<(i64, i8)>,
    /// Two credit tables of `tab_size` entries (one per timing `alpha`, `beta` within
    /// `tab_j` steps): the pitched note waiting for its successor, and the note being
    /// judged.
    tab: Vec<f64>,
    tab_size: usize,
    tab_j: usize,
    pending: Option<Pending>,
    /// The monotone alignment (see "Timing"): where the rendition of the last pitched note
    /// finished in run `chain_run` ended (song time; NaN: none yet in that run), and its end
    /// shift (s).
    chain_end: f64,
    chain_shift: f64,
    chain_run: usize,
}

/// A judged note whose timing is decided when the next pitched note has been judged.
#[derive(Clone, Copy, Debug)]
struct Pending {
    note: usize,
    /// Index of the run (attempt) it was judged in.
    run: usize,
    /// Credit table slot (0 or 1).
    slot: usize,
    /// Margin in steps, moments, moment spacing (s), judged start and end (song time).
    j: usize,
    k_n: usize,
    h: f64,
    a: f64,
    end: f64,
    judged: usize,
    covered: usize,
    max_excused: usize,
    /// Judged length (s): the note's weight when two notes' timings are decided together.
    weight: f64,
}

/// `sc_cov` bits.
const C_COVERED: u8 = 1;
const C_VOICED: u8 = 2;
const C_UNKNOWN: u8 = 4;
const C_SOUND: u8 = 8;
const C_QUIET: u8 = 16;
/// The singer was making periodic sound loud enough to be excused when no pitch was named
/// (`ScoreConfig::excuse_dbfs`, `ScoreConfig::excuse_clarity`).
const C_LOUD: u8 = 32;

impl Scorer {
    /// A scorer for one take. Allocates everything it needs for the song here (the sample
    /// store grows by one entry per tracker value; reserve with `expected_s`).
    pub fn new(cfg: ScoreConfig, guide: ScoreGuide, keys: KeyTimeline, excluded: Vec<(f64, f64)>, expected_s: f64) -> Self {
        let n = guide.notes.len();
        let gradable: Vec<bool> = (0..n).map(|i| guide.gradable(i)).collect();
        let note_line = (0..n)
            .map(|i| {
                let note = &guide.notes[i];
                note.line
                    .and_then(|l| guide.lines.iter().position(|s| s.line == l))
                    .or_else(|| {
                        let m = note.midpoint();
                        guide.lines.iter().position(|s| m >= s.start && m <= s.end)
                    })
            })
            .collect();
        let hop = cfg.hop_s;
        let j = (cfg.strictness_max_tolerance() / hop).round() as usize;
        let mut err_slot = Vec::with_capacity(n + 1);
        let mut total = 0usize;
        let mut longest = 1usize;
        for note in &guide.notes {
            err_slot.push(total);
            let k = Self::moments(note, hop);
            total += k;
            longest = longest.max(k);
        }
        err_slot.push(total);
        let scratch = longest + 2 * j + 2;
        let shift = if cfg.octave_mode == OctaveMode::Strict { 0 } else { cfg.octave_shift.clamp(-2, 2) };
        let cap = ((expected_s.max(1.0) / hop) * 1.05) as usize + 64;
        Scorer {
            strictness: cfg.strictness,
            octave_mode: cfg.octave_mode,
            gradable,
            excluded,
            note_line,
            keys,
            samples: Vec::with_capacity(cap),
            runs: Vec::with_capacity(16),
            outcomes: vec![None; n],
            err_slot,
            errs: vec![0.0; total],
            cursor: 0,
            live_note: 0,
            live: ScoreLive { hits: Vec::with_capacity(256), done: Vec::with_capacity(64) },
            votes: [0; 5],
            settled: None,
            shift,
            sc_val: vec![0.0; scratch],
            sc_cov: vec![0; scratch],
            sc_med: vec![0.0; 64],
            sc_tgt: vec![0.0; longest + 1],
            sc_next: vec![0.0; 2 * j + 1],
            sc_csum: vec![0.0; scratch + 1],
            sc_esum: vec![0; scratch + 1],
            sc_corr: vec![(0, 0); scratch + 4 * j + 4],
            tab: vec![0.0; 2 * (2 * j + 1) * (2 * j + 1)],
            tab_size: (2 * j + 1) * (2 * j + 1),
            tab_j: j,
            pending: None,
            chain_end: f64::NAN,
            chain_shift: 0.0,
            chain_run: usize::MAX,
            guide,
            cfg,
        }
    }

    /// Moments a note is judged at: one per hop after its glide (at least one).
    fn moments(n: &ScoreNote, hop: f64) -> usize {
        let a = Self::judged_start(n, hop);
        (((n.end - a) / hop).round() as usize).max(1)
    }

    /// Start of the judged part: after the guide's own glide, keeping at least one hop.
    fn judged_start(n: &ScoreNote, hop: f64) -> f64 {
        let len = (n.end - n.start).max(0.0);
        n.start + n.glide_in.clamp(0.0, (len - hop).max(0.0))
    }

    pub fn guide(&self) -> &ScoreGuide {
        &self.guide
    }

    pub fn config(&self) -> &ScoreConfig {
        &self.cfg
    }

    /// The key timeline in use.
    pub fn keys(&self) -> &KeyTimeline {
        &self.keys
    }

    /// Mutable access to the key timeline, to forward the engine's key marks
    /// ([`KeyTimeline::start_segment`], [`KeyTimeline::change`]) before the samples that
    /// need them.
    pub fn keys_mut(&mut self) -> &mut KeyTimeline {
        &mut self.keys
    }

    /// Changes the strictness for the notes still to finish (APP_API 12.5
    /// `practice_song_set`).
    pub fn set_strictness(&mut self, s: Strictness) {
        self.strictness = s;
    }

    /// Changes the octave choice for the notes still to finish.
    pub fn set_octave(&mut self, mode: OctaveMode, shift: i32) {
        self.octave_mode = mode;
        self.shift = if mode == OctaveMode::Strict { 0 } else { shift.clamp(-2, 2) };
        if mode != OctaveMode::Auto {
            self.settled = None;
        }
    }

    /// Replaces the excluded spans ("the guide is wrong here"); applied to the whole take
    /// when it is graded.
    pub fn set_excluded(&mut self, spans: Vec<(f64, f64)>) {
        self.excluded = spans;
    }

    /// The octave shift in use (what the lane shows) and whether `auto` has decided.
    pub fn octave(&self) -> (i32, bool) {
        (self.shift, self.settled.is_some())
    }

    /// The samples received so far (to grade the same singing again against another key
    /// timeline: WO2 section 9's "guide shifted by a different amount").
    pub fn samples(&self) -> Vec<SingerSample> {
        let mut out = Vec::with_capacity(self.samples.len());
        for r in &self.runs {
            for s in &self.samples[r.begin..r.end] {
                out.push(SingerSample {
                    t: s.t,
                    segment: r.segment,
                    midi: (!s.midi.is_nan()).then_some(f64::from(s.midi)),
                    level_db: s.level,
                    unknown: s.flags & F_UNKNOWN != 0,
                    music: s.flags & F_MUSIC != 0,
                    clarity: s.clarity,
                });
            }
        }
        out
    }

    /// The scorer's target at song time `t` (APP_API 12.1 `practice_target_at`): the note
    /// sounding there, its target pitch with the key heard and the octave shift, the key,
    /// and whether it is graded.
    pub fn target_at(&self, t: f64, segment: Option<u64>) -> Option<(usize, f64, i32, bool)> {
        let i = self.guide.notes.partition_point(|n| n.end <= t);
        let n = self.guide.notes.get(i)?;
        if t < n.start {
            return None;
        }
        let key = self.keys.key_at(t, segment);
        let graded = self.gradable[i] && !in_spans(&self.excluded, n.midpoint());
        Some((i, n.midi + f64::from(key) + 12.0 * f64::from(self.shift), key, graded))
    }

    /// Feeds one tracker value. Values of one segment arrive in time order; a value from a
    /// new segment (a seek) starts a new attempt for the notes it covers.
    pub fn push(&mut self, s: SingerSample) {
        let hop = self.cfg.hop_s;
        let new_run = self.runs.last().is_none_or(|r| r.segment != s.segment);
        if new_run {
            // The segment that ends here (a seek) may have reached notes it could not finish
            // yet (the last note sung before the seek ends less than the timing margin before
            // it): finish them from that segment now, as `finish` does at the end of a take,
            // so a note sung right before a seek is graded and counted.
            self.finish_reached();
            self.flush_pending();
            // Notes reaching into the new segment may be attempted again: start finishing
            // from the first note that ends after the tolerance before this point.
            let tol = self.strictness.bands().tolerance_s;
            self.cursor = self.guide.notes.partition_point(|n| n.end <= s.t - tol);
            self.live_note = self.guide.notes.partition_point(|n| n.end <= s.t);
            let at = self.samples.len();
            self.runs.push(Run { segment: s.segment, begin: at, end: at, t0: s.t, t1: s.t });
        }
        let run = self.runs.last_mut().expect("a run");
        let t = if run.end > run.begin { s.t.max(run.t1) } else { s.t };
        run.t1 = t;
        run.end += 1;
        let mut flags = 0u8;
        if s.unknown {
            flags |= F_UNKNOWN;
        }
        if s.music {
            flags |= F_MUSIC;
        }
        let midi = s.midi.filter(|m| m.is_finite()).map(|m| m as f32).unwrap_or(f32::NAN);
        self.samples.push(Stored { t, midi, level: s.level_db, clarity: s.clarity, flags });

        // Live: the note sounding now, hit or miss, and the octave votes.
        while self.live_note < self.guide.notes.len() && self.guide.notes[self.live_note].end <= t {
            self.live_note += 1;
        }
        if let (Some(m), Some(n)) = (s.midi, self.guide.notes.get(self.live_note)) {
            let i = self.live_note;
            if t >= n.start && self.gradable[i] && !in_spans(&self.excluded, n.midpoint()) && !s.unknown {
                let key = self.keys.key_at(t, Some(s.segment));
                let raw = 100.0 * (m - (n.midi + f64::from(key)));
                if self.octave_mode == OctaveMode::Auto && t >= Self::judged_start(n, hop) {
                    let f = fold_cents(raw);
                    if f.abs() <= 100.0 {
                        let k = ((raw - f) / 1200.0).round().clamp(-2.0, 2.0) as i32;
                        self.votes[(k + 2) as usize] += 1;
                        self.maybe_settle();
                    }
                }
                let e = self.error(raw);
                let bands = self.strictness.bands();
                let w: i8 = if e.abs() <= bands.lit_cents() { 0 } else if e > 0.0 { 1 } else { -1 };
                let (from, to) = (t - 0.5 * hop, t + 0.5 * hop);
                match self.live.hits.last_mut() {
                    Some(h) if h.0 == i && h.3 == w && from - h.2 <= 0.5 * hop => h.2 = to,
                    _ => self.live.hits.push((i, from, to, w)),
                }
            }
        }

        // Finish every note whose judgement can no longer change.
        let margin = self.strictness.bands().tolerance_s + self.cfg.smooth_s + 2.0 * hop;
        while self.cursor < self.guide.notes.len() && self.guide.notes[self.cursor].end + margin <= t {
            let i = self.cursor;
            self.finish_note(i);
            self.cursor += 1;
        }
    }

    /// `auto`: decide the octave once enough has been sung.
    fn maybe_settle(&mut self) {
        let total: u32 = self.votes.iter().sum();
        let need = (self.cfg.settle_s / self.cfg.hop_s).round() as u32;
        if total < need {
            return;
        }
        let best = self.vote_winner();
        if self.settled.is_none() {
            self.settled = Some(best);
            self.shift = best;
        }
    }

    /// The most voted shift (ties: the shift in use, then the smaller shift).
    fn vote_winner(&self) -> i32 {
        let mut best = self.shift;
        let mut best_n = self.votes[(self.shift.clamp(-2, 2) + 2) as usize];
        for k in [0i32, -1, 1, -2, 2] {
            let n = self.votes[(k + 2) as usize];
            if n > best_n {
                best = k;
                best_n = n;
            }
        }
        best
    }

    /// The error in cents for a raw difference by the octave mode in use (the target's
    /// shift is irrelevant when octaves are forgiven).
    fn error(&self, raw_cents: f64) -> f64 {
        if self.octave_mode.folds() { fold_cents(raw_cents) } else { raw_cents - 1200.0 * f64::from(self.shift) }
    }

    /// The newest run (the segment playing now).
    fn newest_run(&self) -> Option<Run> {
        self.runs.last().copied()
    }

    /// Index range of a run's samples within `[a, b]` (s).
    fn range_in(&self, r: &Run, a: f64, b: f64) -> (usize, usize) {
        let s = &self.samples[r.begin..r.end];
        let lo = s.partition_point(|x| x.t < a);
        let hi = s.partition_point(|x| x.t <= b);
        (r.begin + lo, r.begin + hi)
    }

    /// The singer at song time `x` in run `r`: (pitch for judging, coverage bits).
    fn singer_at(&mut self, r: &Run, x: f64) -> (Option<f64>, u8) {
        let hop = self.cfg.hop_s;
        let s = &self.samples[r.begin..r.end];
        if s.is_empty() {
            return (None, 0);
        }
        let i = s.partition_point(|v| v.t < x);
        // Nearest sample.
        let near = match (i.checked_sub(1).map(|k| (k, (x - s[k].t).abs())), s.get(i).map(|v| (i, (v.t - x).abs()))) {
            (Some(a), Some(b)) => if b.1 < a.1 { b } else { a },
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return (None, 0),
        };
        if near.1 > 0.75 * hop {
            return (None, 0);
        }
        let mut v = s[near.0];
        let mut bits = C_COVERED;
        if v.flags & F_UNKNOWN != 0 {
            bits |= C_UNKNOWN;
        }
        // Only the singer's own sound counts toward "too quiet" (not left-over music).
        if v.level >= self.cfg.sound_dbfs && v.flags & (F_MUSIC | F_UNKNOWN) == 0 {
            bits |= C_SOUND;
            if v.level < self.cfg.quiet_dbfs && v.midi.is_nan() {
                bits |= C_QUIET;
            }
            if v.level >= self.cfg.excuse_dbfs && v.clarity >= self.cfg.excuse_clarity {
                bits |= C_LOUD;
            }
        }
        if v.midi.is_nan() {
            // A moment half-way between two values takes the voiced one: the moments and
            // the tracker's hops are not aligned, so the nearest value may be the
            // silence just before or after a note.
            let other = if near.0 + 1 == i { i } else { i.wrapping_sub(1) };
            match s.get(other) {
                Some(o) if !o.midi.is_nan() && (o.t - x).abs() <= 0.75 * hop => v = *o,
                _ => {
                    // A short dropout of the tracker inside a sung line (voiced values within
                    // `bridge_s` before and after, in this segment) is bridged: the singer was
                    // still singing. Silence, breath and noise never have voiced values on
                    // both sides, so they are never bridged.
                    let b = self.cfg.bridge_s;
                    let before = s[..near.0].iter().rev().take_while(|o| x - o.t <= b).find(|o| !o.midi.is_nan());
                    let after = s[near.0 + 1..].iter().take_while(|o| o.t - x <= b).find(|o| !o.midi.is_nan());
                    match (before, after) {
                        (Some(o), Some(_)) if b > 0.0 => v = *o,
                        _ => return (None, bits),
                    }
                }
            }
            bits &= !C_QUIET;
        }
        bits |= C_VOICED;
        // Median of the voiced values within smooth_s either side (vibrato centre).
        let (lo, hi) = self.range_in(r, x - self.cfg.smooth_s, x + self.cfg.smooth_s);
        let mut n = 0usize;
        for k in lo..hi {
            let m = self.samples[k].midi;
            if !m.is_nan() && n < self.sc_med.len() {
                self.sc_med[n] = f64::from(m);
                n += 1;
            }
        }
        let value = if n >= 3 { median_in_place(&mut self.sc_med[..n]) } else { f64::from(v.midi) };
        (Some(value), bits)
    }

    /// Judges note `i` from the newest run's samples and passes it on: an unpitched note is
    /// finished at once; a pitched note waits until the next pitched note has been judged,
    /// and the timing of the one waiting is then decided together with it ("Timing" in the
    /// module documentation). An attempt covering less than half of the note leaves an
    /// earlier attempt in place. Allocates nothing.
    fn finish_note(&mut self, i: usize) {
        let slot = match &self.pending {
            Some(p) => 1 - p.slot,
            None => 0,
        };
        let Some(p) = self.judge(i, slot) else { return };
        if !self.guide.notes[i].pitched {
            self.resolve(p, None);
            return;
        }
        if let Some(q) = self.pending.take() {
            let next = (q.run == p.run).then_some(p);
            self.resolve(q, next);
        }
        // Nothing to wait for when no pitched note follows close enough for the two
        // renditions to meet (both shifted by the tolerance): decide it now.
        let reach = 2.0 * p.j as f64 * p.h + self.cfg.overlap_s.max(0.0);
        let coupled = self.guide.notes[i + 1..]
            .iter()
            .find(|n| n.pitched)
            .is_some_and(|n| Self::judged_start(n, self.cfg.hop_s) < p.end + reach);
        if coupled {
            self.pending = Some(p);
        } else {
            self.resolve(p, None);
        }
    }

    /// Finishes the note still waiting for its successor (the run ended: end of the take or
    /// a seek).
    fn flush_pending(&mut self) {
        if let Some(q) = self.pending.take() {
            self.resolve(q, None);
        }
    }

    /// Fills the judging scratch for note `n` (`j` steps of margin, moment spacing `h` from
    /// the judged start `a`) from run `r`.
    fn fill_scratch(&mut self, r: &Run, n: &ScoreNote, a: f64, h: f64, k_n: usize, j: usize) {
        for m in 0..k_n + 2 * j {
            let x = a + (m as f64 - j as f64 + 0.5) * h;
            let (v, bits) = self.singer_at(r, x);
            self.sc_val[m] = v.unwrap_or(f64::NAN);
            self.sc_cov[m] = bits;
        }
        for k in 0..k_n {
            let tau = a + (k as f64 + 0.5) * h;
            self.sc_tgt[k] = n.midi + f64::from(self.keys.key_at(tau, Some(r.segment)));
        }
    }

    /// Judges note `i` in the newest run: the credit of every timing (`alpha`, `beta`) into
    /// table `slot` (`f64::NEG_INFINITY` where the rendition would be shorter than
    /// `min_rendition`). `None` when the run covers less than half of the note.
    fn judge(&mut self, i: usize, slot: usize) -> Option<Pending> {
        let r = self.newest_run()?;
        let run = self.runs.len() - 1;
        let hop = self.cfg.hop_s;
        let bands = self.strictness.bands();
        let n = self.guide.notes[i].clone();
        let a = Self::judged_start(&n, hop);
        let k_n = Self::moments(&n, hop);
        let h = (n.end - a) / k_n as f64;
        let j = ((bands.tolerance_s / hop).round() as usize).min((self.sc_val.len() - k_n) / 2).min(self.tab_j);
        self.fill_scratch(&r, &n, a, h, k_n, j);
        let covered = (0..k_n).filter(|&k| self.sc_cov[k + j] & C_COVERED != 0).count();
        if covered * 2 < k_n {
            return None;
        }
        // Only covered moments the input could be judged at count.
        let judged = (0..k_n).filter(|&k| self.sc_cov[k + j] & (C_COVERED | C_UNKNOWN) == C_COVERED).count();
        // At most this many moments without a named pitch are excused.
        let max_excused = (self.cfg.excuse_share.clamp(0.0, 1.0) * judged as f64).floor() as usize;
        // The rendition is at least `min_rendition` of the judged length:
        // beta - alpha >= -(1 - min_rendition) k_n.
        let min_len = -(((1.0 - self.cfg.min_rendition.clamp(0.0, 1.0)) * k_n as f64).floor() as i64);
        let ji = j as i64;
        let w = 2 * j + 1;
        let base = slot * self.tab_size;
        let one_target = (1..k_n).all(|k| self.sc_tgt[k] == self.sc_tgt[0]);
        if judged == k_n && one_target {
            self.fill_table_fast(base, k_n, j, min_len, judged, max_excused, bands);
        } else {
            for alpha in -ji..=ji {
                for beta in -ji..=ji {
                    let idx = base + (alpha + ji) as usize * w + (beta + ji) as usize;
                    self.tab[idx] = if beta - alpha < min_len {
                        f64::NEG_INFINITY
                    } else {
                        let (sum, excusable) = self.warped_sum(k_n, j, alpha, beta, bands);
                        let denom = judged - excusable.min(max_excused);
                        if denom > 0 { sum / denom as f64 } else { 0.0 }
                    };
                }
            }
        }
        Some(Pending { note: i, run, slot, j, k_n, h, a, end: n.end, judged, covered, max_excused, weight: judged as f64 * h })
    }

    /// Decides the timing of judged note `q` and finishes it. A pitched note's rendition
    /// starts no earlier than the previous pitched note's rendition of the same run ended
    /// (less `overlap_s`); with `next` (the next pitched note of the run, already judged)
    /// the timing that gives the two notes together the most credit is taken, so a note
    /// cannot take the start of the next note's rendition unless that gains more than the
    /// next note loses. Ties: the least stretch, then the shift closest to the previous
    /// note's.
    fn resolve(&mut self, q: Pending, next: Option<Pending>) {
        let ji = q.j as i64;
        let w = 2 * q.j + 1;
        let pitched = self.guide.notes[q.note].pitched;
        let chained = pitched && q.judged > 0;
        let ov = self.cfg.overlap_s.max(0.0);
        let mut alpha_min = -ji;
        let mut d_prev = 0.0;
        if chained && self.chain_run == q.run && self.chain_end.is_finite() {
            alpha_min = ((((self.chain_end - ov - q.a) / q.h) - 1e-9).ceil() as i64).clamp(-ji, ji);
            d_prev = self.chain_shift;
        }
        // The next note's best credit for each earliest start it may take (suffix maximum
        // over its start shift of the best over its end shift).
        if let Some(p) = next.as_ref() {
            let pj = p.j as i64;
            let pw = 2 * p.j + 1;
            let pb = p.slot * self.tab_size;
            let mut run_max = f64::NEG_INFINITY;
            for alpha in (-pj..=pj).rev() {
                let row = pb + (alpha + pj) as usize * pw;
                let best = self.tab[row..row + pw].iter().copied().fold(f64::NEG_INFINITY, f64::max);
                run_max = run_max.max(best);
                self.sc_next[(alpha + pj) as usize] = run_max;
            }
        }
        let qb = q.slot * self.tab_size;
        let mut best = (f64::NEG_INFINITY, 0i64, 0i64);
        // Ties: the least stretch (a plain shift explains a steady delay), then the shift
        // closest to the previous note's (in steps of this note's spacing).
        let mut best_cost = (i64::MAX, i64::MAX);
        let d = (d_prev / q.h).round() as i64;
        for alpha in alpha_min..=ji {
            for beta in -ji..=ji {
                let c = self.tab[qb + (alpha + ji) as usize * w + (beta + ji) as usize];
                if c == f64::NEG_INFINITY {
                    continue;
                }
                let mut total = q.weight * c;
                if let Some(p) = next.as_ref() {
                    let pj = p.j as i64;
                    let e = q.end + beta as f64 * q.h;
                    let a_next = ((((e - ov - p.a) / p.h) - 1e-9).ceil() as i64).clamp(-pj, pj);
                    let s = self.sc_next[(a_next + pj) as usize];
                    if s.is_finite() {
                        total += p.weight * s;
                    }
                }
                let cost = ((beta - alpha).abs(), (alpha - d).abs() + (beta - d).abs());
                if total > best.0 + 1e-9 || ((total - best.0).abs() <= 1e-9 && cost < best_cost) {
                    best = (total, alpha, beta);
                    best_cost = cost;
                }
            }
        }
        let (alpha, beta) = if best.0 == f64::NEG_INFINITY { (0, 0) } else { (best.1, best.2) };
        if chained {
            self.chain_run = q.run;
            self.chain_end = q.end + beta as f64 * q.h;
            self.chain_shift = beta as f64 * q.h;
        }
        self.finalize(q, alpha, beta);
    }

    /// Records the outcome of note `q` at the chosen timing (credit, tendency errors, the
    /// live "done" entry).
    fn finalize(&mut self, q: Pending, alpha: i64, beta: i64) {
        let i = q.note;
        let Some(r) = self.runs.get(q.run).copied() else { return };
        let n = self.guide.notes[i].clone();
        let (k_n, j, h) = (q.k_n, q.j, q.h);
        self.fill_scratch(&r, &n, q.a, h, k_n, j);
        let bands = self.strictness.bands();
        let (sum, excusable) = self.warped_sum(k_n, j, alpha, beta, bands);
        let excused = excusable.min(q.max_excused);
        let denom = q.judged - excused;
        let credit = if q.judged > 0 && denom > 0 { (sum / denom as f64).clamp(0.0, 1.0) } else { 0.0 };
        // Errors near the target at the chosen timing, for the tendency.
        let (e0, e1) = (self.err_slot[i], self.err_slot[i + 1]);
        let mut ne = 0usize;
        let mut sum_e = 0.0;
        let (mut voiced, mut unknown, mut sound, mut quiet) = (0u32, 0usize, 0u32, 0u32);
        for k in 0..k_n {
            let c = self.sc_cov[k + j];
            if c & C_COVERED == 0 {
                continue;
            }
            if c & C_UNKNOWN != 0 {
                unknown += 1;
                continue;
            }
            if c & C_VOICED != 0 {
                voiced += 1;
            }
            if c & C_SOUND != 0 {
                sound += 1;
            }
            if c & C_QUIET != 0 {
                quiet += 1;
            }
            let v = self.sc_val[Self::warp_pos(k, k_n, j, alpha, beta)];
            if v.is_nan() {
                continue;
            }
            let e = self.error(100.0 * (v - self.sc_tgt[k]));
            if e.abs() <= self.cfg.tendency_window_cents && e0 + ne < e1 {
                self.errs[e0 + ne] = e;
                ne += 1;
                sum_e += e;
            }
        }
        let unknown_note = unknown * 2 >= q.covered;
        let key = self.keys.key_at(n.midpoint(), Some(r.segment));
        let graded = self.gradable[i] && !unknown_note;
        let o = NoteOutcome {
            note: i,
            credit,
            graded,
            unknown: unknown_note,
            key,
            mean_err: (ne > 0).then(|| sum_e / ne as f64),
            delta_s: 0.5 * (alpha + beta) as f64 * h,
            weight_s: q.judged as f64 * h,
            segment: r.segment,
            covered: q.judged as u32,
            voiced,
            excused: excused as u32,
            start_shift_s: alpha as f64 * h,
            end_shift_s: beta as f64 * h,
            sound,
            quiet,
            errs: (e0, e0 + ne),
        };
        let shown = graded && !in_spans(&self.excluded, n.midpoint());
        self.live.done.push((i, shown.then_some(credit), key, o.mean_err));
        self.outcomes[i] = Some(o);
    }

    /// The credit table of a note whose moments are all judged against one target, in
    /// O(positions + stretches x moments) instead of O(timings x moments): the credit of
    /// each scratch position and whether it is excusable are summed into prefix arrays;
    /// for each stretch `d = beta - alpha` the moments land on one contiguous run of
    /// positions, less the positions skipped (stretch) or plus those landed on twice
    /// (compression), the same for every start shift. Gives the sums `warped_sum` gives,
    /// up to the order of the additions.
    #[allow(clippy::too_many_arguments)]
    fn fill_table_fast(&mut self, base: usize, k_n: usize, j: usize, min_len: i64, judged: usize, max_excused: usize, bands: Bands) {
        let ji = j as i64;
        let w = 2 * j + 1;
        let positions = k_n + 2 * j;
        let tgt = self.sc_tgt[0];
        self.sc_csum[0] = 0.0;
        self.sc_esum[0] = 0;
        for m in 0..positions {
            let v = self.sc_val[m];
            let (c, e) = if v.is_nan() {
                (0.0, u32::from(self.sc_cov[m] & (C_COVERED | C_UNKNOWN | C_LOUD) == (C_COVERED | C_LOUD)))
            } else {
                (bands.credit(self.error(100.0 * (v - tgt))), 0)
            };
            self.sc_csum[m + 1] = self.sc_csum[m] + c;
            self.sc_esum[m + 1] = self.sc_esum[m] + e;
        }
        for d in -2 * ji..=2 * ji {
            // Timings with this stretch: alpha in -j..=j with beta = alpha + d in -j..=j.
            let (a_lo, a_hi) = ((-ji).max(-ji - d), ji.min(ji - d));
            if a_lo > a_hi {
                continue;
            }
            if d < min_len {
                for alpha in a_lo..=a_hi {
                    self.tab[base + (alpha + ji) as usize * w + (alpha + d + ji) as usize] = f64::NEG_INFINITY;
                }
                continue;
            }
            // Where the moments land (relative to the start shift) and the corrections.
            let g0 = Self::warp_step(0, k_n, d);
            let g1 = Self::warp_step(k_n - 1, k_n, d);
            let mut nc = 0usize;
            let mut prev = g0;
            let mut ok = true;
            for k in 1..k_n {
                let g = Self::warp_step(k, k_n, d);
                match g - prev {
                    1 => {}
                    0 if nc < self.sc_corr.len() => {
                        self.sc_corr[nc] = (g, 1);
                        nc += 1;
                    }
                    step if step >= 2 && nc + (step as usize - 1) <= self.sc_corr.len() => {
                        for q in prev + 1..g {
                            self.sc_corr[nc] = (q, -1);
                            nc += 1;
                        }
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
                prev = g;
            }
            for alpha in a_lo..=a_hi {
                let beta = alpha + d;
                let idx = base + (alpha + ji) as usize * w + (beta + ji) as usize;
                let off = ji + alpha;
                let (lo, hi) = (off + g0, off + g1);
                if !ok || lo < 0 || hi >= positions as i64 {
                    // Outside the scratch (clamped positions): the plain sum.
                    let (sum, excusable) = self.warped_sum(k_n, j, alpha, beta, bands);
                    let denom = judged - excusable.min(max_excused);
                    self.tab[idx] = if denom > 0 { sum / denom as f64 } else { 0.0 };
                    continue;
                }
                let (lo, hi) = (lo as usize, hi as usize);
                let mut sum = self.sc_csum[hi + 1] - self.sc_csum[lo];
                let mut exc = i64::from(self.sc_esum[hi + 1]) - i64::from(self.sc_esum[lo]);
                for &(q, sign) in &self.sc_corr[..nc] {
                    let m = (off + q) as usize;
                    let v = self.sc_val[m];
                    if v.is_nan() {
                        if self.sc_cov[m] & (C_COVERED | C_UNKNOWN | C_LOUD) == (C_COVERED | C_LOUD) {
                            exc += i64::from(sign);
                        }
                    } else {
                        sum += f64::from(sign) * bands.credit(self.error(100.0 * (v - tgt)));
                    }
                }
                let excusable = exc.max(0) as usize;
                let denom = judged - excusable.min(max_excused);
                self.tab[idx] = if denom > 0 { sum / denom as f64 } else { 0.0 };
            }
        }
    }

    /// Position (index into the judging scratch) of moment `k` of `k_n` under the shifts
    /// `alpha` (start) and `beta` (end), in steps of the moment spacing, with `j` steps of
    /// margin on each side.
    #[inline]
    fn warp_pos(k: usize, k_n: usize, j: usize, alpha: i64, beta: i64) -> usize {
        (j as i64 + alpha + Self::warp_step(k, k_n, beta - alpha)).clamp(0, (k_n + 2 * j - 1) as i64) as usize
    }

    /// Where moment `k` of `k_n` lands, relative to its start shift, under a stretch of
    /// `d` steps: `k + round(d (k + 0.5) / k_n)`.
    #[inline]
    fn warp_step(k: usize, k_n: usize, d: i64) -> i64 {
        k as i64 + (d as f64 * ((k as f64 + 0.5) / k_n as f64)).round() as i64
    }

    /// Sum of the credits of the judged moments of the note being finished under the shifts
    /// `alpha`, `beta`, and how many of them could be excused (no pitch, periodic sound).
    fn warped_sum(&self, k_n: usize, j: usize, alpha: i64, beta: i64, bands: Bands) -> (f64, usize) {
        let mut sum = 0.0;
        let mut excusable = 0usize;
        for k in 0..k_n {
            if self.sc_cov[k + j] & (C_COVERED | C_UNKNOWN) != C_COVERED {
                continue;
            }
            let m = Self::warp_pos(k, k_n, j, alpha, beta);
            let v = self.sc_val[m];
            if v.is_nan() {
                if self.sc_cov[m] & (C_COVERED | C_UNKNOWN | C_LOUD) == (C_COVERED | C_LOUD) {
                    excusable += 1;
                }
                continue;
            }
            sum += bands.credit(self.error(100.0 * (v - self.sc_tgt[k])));
        }
        (sum, excusable)
    }

    /// Takes the live output gathered since the last call (the vectors keep their capacity
    /// when the caller hands them back by `std::mem::swap`).
    pub fn drain_live(&mut self, out: &mut ScoreLive) {
        out.hits.clear();
        out.done.clear();
        std::mem::swap(out, &mut self.live);
    }

    /// Covered song time: the union of the runs, each widened by half a hop.
    fn covered_spans(&self) -> Vec<(f64, f64)> {
        let half = 0.5 * self.cfg.hop_s;
        let mut v: Vec<(f64, f64)> = self.runs.iter().filter(|r| r.end > r.begin).map(|r| (r.t0 - half, r.t1 + half)).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<(f64, f64)> = Vec::with_capacity(v.len());
        for (a, b) in v {
            match out.last_mut() {
                Some(l) if a <= l.1 => l.1 = l.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// Share of the song's graded note time inside the covered spans.
    fn sung_share(&self, spans: &[(f64, f64)]) -> f64 {
        let mut all = 0.0;
        let mut cov = 0.0;
        for (i, n) in self.guide.notes.iter().enumerate() {
            if !self.gradable[i] || in_spans(&self.excluded, n.midpoint()) {
                continue;
            }
            all += n.end - n.start;
            for &(a, b) in spans {
                cov += (n.end.min(b) - n.start.max(a)).max(0.0);
            }
        }
        if all > 0.0 { (cov / all).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// The running state of the take.
    pub fn progress(&self) -> TakeProgress {
        let spans = self.covered_spans();
        let (mut sum, mut w, mut graded, mut total) = (0.0, 0.0, 0u32, 0u32);
        for (i, o) in self.outcomes.iter().enumerate() {
            let Some(o) = o else { continue };
            total += 1;
            if o.graded && !in_spans(&self.excluded, self.guide.notes[i].midpoint()) {
                graded += 1;
                sum += o.credit * o.weight_s;
                w += o.weight_s;
            }
        }
        TakeProgress {
            covered: self.sung_share(&spans),
            graded,
            total,
            overall: (w > 0.0).then(|| round_to(100.0 * sum / w, 1)),
            seeks: self.runs.len().saturating_sub(1) as u32,
            octave_shift: self.shift,
            octave_settled: self.settled.is_some(),
        }
    }

    /// Finishes the notes the newest run reached but could not finish yet (they ended less
    /// than the timing margin before its last value), by what was sung of them: at the end
    /// of a take and when a seek starts a new run. A note covered for less than half keeps
    /// any earlier attempt (`finish_note`). Allocates nothing.
    fn finish_reached(&mut self) {
        if let Some(r) = self.newest_run() {
            while self.cursor < self.guide.notes.len() && self.guide.notes[self.cursor].start < r.t1 {
                let i = self.cursor;
                self.finish_note(i);
                self.cursor += 1;
            }
        }
    }

    /// Ends the take: finishes the notes still open (by what was sung of them) and grades.
    pub fn finish(&mut self, ctx: &FinishContext) -> ScoreResult {
        self.finish_reached();
        self.flush_pending();
        let spans = self.covered_spans();
        let mut total = 0u32;
        let mut graded = 0u32;
        let mut unknown_notes = 0u32;
        let mut pitched_in = 0u32;
        let mut gradable_in = 0u32;
        let (mut sum, mut w) = (0.0, 0.0);
        let (mut cov, mut voiced, mut sound, mut quiet) = (0u64, 0u64, 0u64, 0u64);
        let mut tendency: Vec<f64> = Vec::new();
        let mut key_w: Vec<(i32, f64)> = Vec::new();
        let rows_n = self.guide.lines.len();
        let mut row_sum = vec![0.0f64; rows_n];
        let mut row_w = vec![0.0f64; rows_n];
        let mut row_graded = vec![0u32; rows_n];
        let mut row_errs: Vec<Vec<f64>> = vec![Vec::new(); rows_n];
        let mut row_in = vec![false; rows_n];
        let mut detail = Vec::new();
        for (i, o) in self.outcomes.iter().enumerate() {
            let Some(o) = o else { continue };
            let n = &self.guide.notes[i];
            total += 1;
            if n.pitched {
                pitched_in += 1;
            }
            let excluded = in_spans(&self.excluded, n.midpoint());
            if self.gradable[i] && !excluded {
                gradable_in += 1;
            }
            if let Some(l) = self.note_line[i] {
                row_in[l] = true;
            }
            if self.cfg.detail {
                let mut d = o.clone();
                d.graded = o.graded && !excluded;
                detail.push(d);
            }
            if o.unknown && self.gradable[i] && !excluded {
                unknown_notes += 1;
            }
            if !o.graded || excluded {
                continue;
            }
            graded += 1;
            sum += o.credit * o.weight_s;
            w += o.weight_s;
            cov += u64::from(o.covered);
            voiced += u64::from(o.voiced);
            sound += u64::from(o.sound);
            quiet += u64::from(o.quiet);
            let errs = &self.errs[o.errs.0..o.errs.1];
            tendency.extend_from_slice(errs);
            match key_w.iter_mut().find(|(k, _)| *k == o.key) {
                Some(e) => e.1 += o.weight_s,
                None => key_w.push((o.key, o.weight_s)),
            }
            if let Some(l) = self.note_line[i] {
                row_sum[l] += o.credit * o.weight_s;
                row_w[l] += o.weight_s;
                row_graded[l] += 1;
                row_errs[l].extend_from_slice(errs);
            }
        }
        let mut trust: Vec<String> = Vec::new();
        for f in &ctx.flags {
            if !trust.contains(f) {
                trust.push(f.clone());
            }
        }
        let mut add = |f: &str| {
            if !trust.iter().any(|x| x == f) {
                trust.push(f.to_string());
            }
        };
        if sound > 0 && quiet as f64 > 0.10 * sound as f64 {
            add("low_level");
        }
        if self.guide.outdated || (pitched_in > 0 && (gradable_in as f64) < 0.6 * pitched_in as f64) {
            add("uncertain_guide");
        }
        if !ctx.completed {
            add("stopped_early");
        }
        let untrusted = trust.iter().any(|f| f == "untrusted");
        let overall = if untrusted || w <= 0.0 { None } else { Some(round_to(100.0 * sum / w, 1)) };
        let hop = self.cfg.hop_s;
        let tendency_cents = if tendency.len() as f64 * hop >= self.cfg.tendency_min_s {
            Some(median_in_place(&mut tendency).round() + 0.0)
        } else {
            None
        };
        let mut rows = Vec::new();
        for (l, span) in self.guide.lines.iter().enumerate() {
            let overlaps = spans.iter().any(|&(a, b)| span.end > a && span.start < b);
            if !row_in[l] && !overlaps {
                continue;
            }
            let score = (row_w[l] > 0.0 && !untrusted).then(|| (100.0 * row_sum[l] / row_w[l]).round());
            let tend = if row_errs[l].len() as f64 * hop >= self.cfg.line_tendency_min_s {
                Some(median_in_place(&mut row_errs[l]).round() + 0.0)
            } else {
                None
            };
            rows.push((span.line, round_to(span.start, 1), score, row_graded[l], tend));
        }
        let key = key_w
            .iter()
            .fold(None::<(i32, f64)>, |b, &(k, wt)| match b {
                Some((_, bw)) if bw >= wt => b,
                _ => Some((k, wt)),
            })
            .map(|(k, _)| k)
            .unwrap_or_else(|| self.runs.first().map(|r| self.keys.key_at(r.t0, Some(r.segment))).unwrap_or(0));
        let key_changes = self.runs.iter().map(|r| self.keys.changes_between(r.segment, r.t0, r.t1)).sum();
        let octave = match self.octave_mode {
            OctaveMode::Strict => 0,
            OctaveMode::Fixed => self.shift,
            OctaveMode::Auto => {
                if self.votes.iter().sum::<u32>() > 0 { self.vote_winner() } else { self.shift }
            }
        };
        ScoreResult {
            key,
            key_changes,
            seeks: self.runs.len().saturating_sub(1) as u32,
            octave,
            octave_mode: self.octave_mode,
            strictness: self.strictness,
            grade: overall.map(|x| grade_letter(x).to_string()),
            overall,
            graded,
            total,
            tendency_cents,
            sung_share: round_to(self.sung_share(&spans), 2),
            voiced_share: if cov > 0 { round_to(voiced as f64 / cov as f64, 2) } else { 0.0 },
            completed: ctx.completed,
            trust,
            guide: self.guide.label.clone(),
            scorer: format!("{SCORER} {SCORER_VERSION}"),
            lines: LinesResult { basis: self.guide.basis, rows },
            unknown_notes,
            notes: detail,
        }
    }
}

impl ScoreConfig {
    /// The largest timing tolerance of any strictness (scratch sizing: the strictness can
    /// change during a take).
    fn strictness_max_tolerance(&self) -> f64 {
        [Strictness::Relaxed, Strictness::Standard, Strictness::Strict]
            .iter()
            .map(|s| s.bands().tolerance_s)
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests;

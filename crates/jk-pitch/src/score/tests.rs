// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests of the scorer with scripted pitch values (no audio): each scripted singer
//! of WO2 section 9 lands where a person would expect, and the rules of the module
//! documentation hold. The whole chain with real audio is tested in `practice::tests` and
//! measured in docs/benchmarks/practice-score.md.

use super::*;

const HOP: f64 = 0.01;

/// A melody of `n` notes, 0.4 s each with 0.1 s gaps, alternating pitches around A3/C4
/// with a few repeats, two lines.
fn melody(n: usize) -> ScoreGuide {
    let pitches = [57.0, 60.0, 62.0, 64.0, 62.0, 60.0, 59.0, 57.0, 55.0, 57.0];
    let notes: Vec<ScoreNote> = (0..n)
        .map(|i| {
            let start = 1.0 + 0.5 * i as f64;
            ScoreNote {
                start,
                end: start + 0.4,
                midi: pitches[i % pitches.len()] - 0.15,
                pitched: true,
                confidence: 0.9,
                octave_flagged: false,
                glide_in: 0.0,
                line: Some((i / (n / 2).max(1)) as i64),
            }
        })
        .collect();
    let half = n / 2;
    let lines = vec![
        LineSpan { line: 0, start: notes[0].start, end: notes[half - 1].end },
        LineSpan { line: 1, start: notes[half].start, end: notes[n - 1].end },
    ];
    let duration_s = notes[n - 1].end + 1.0;
    ScoreGuide {
        notes,
        low_confidence: 0.5,
        lines,
        basis: LineBasis::Lyrics,
        outdated: false,
        duration_s,
        label: "test 0".into(),
    }
}

/// The guide's pitch at song time `t` (with `key`), or `None` between notes.
fn guide_at(g: &ScoreGuide, t: f64, key: f64) -> Option<f64> {
    g.notes.iter().find(|n| t >= n.start && t < n.end).map(|n| n.midi + key)
}

/// One tracker value at song time `t`: a singer at -20 dBFS while they have a pitch, the
/// quiet of a room (-90 dBFS) when they have none.
fn at(t: f64, segment: u64, midi: Option<f64>) -> SingerSample {
    SingerSample {
        t,
        segment,
        midi,
        level_db: if midi.is_some() { -20.0 } else { -90.0 },
        unknown: false,
        music: false,
        clarity: if midi.is_some() { 0.95 } else { 0.1 },
    }
}

/// Grades a singer given as a function of song time over the whole song, one segment.
fn grade(g: &ScoreGuide, cfg: ScoreConfig, keys: KeyTimeline, singer: impl Fn(f64) -> Option<f64>) -> ScoreResult {
    let mut s = Scorer::new(cfg, g.clone(), keys, Vec::new(), g.duration_s);
    let n = (g.duration_s / HOP) as usize;
    for i in 0..n {
        let t = i as f64 * HOP;
        s.push(at(t, 0, singer(t)));
    }
    s.finish(&FinishContext { completed: true, flags: vec![] })
}

fn std_cfg() -> ScoreConfig {
    ScoreConfig { detail: true, ..ScoreConfig::default() }
}

#[test]
fn perfect_singer_scores_100_and_silence_scores_0() {
    let g = melody(20);
    let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t, 0.0));
    assert_eq!(r.overall, Some(100.0));
    assert_eq!(r.grade.as_deref(), Some("A+"));
    assert_eq!((r.graded, r.total), (20, 20));
    assert_eq!(r.tendency_cents, Some(0.0));
    assert_eq!(r.sung_share, 1.0);
    assert_eq!(r.voiced_share, 1.0);
    assert!(r.trust.is_empty(), "{:?}", r.trust);
    let s = grade(&g, std_cfg(), KeyTimeline::constant(0), |_| None);
    assert_eq!(s.overall, Some(0.0));
    assert_eq!(s.grade.as_deref(), Some("F"));
    assert_eq!(s.tendency_cents, None);
    assert_eq!(s.voiced_share, 0.0);
}

#[test]
fn bands_and_tendency_for_sharp_and_flat_singers() {
    let g = melody(20);
    // A semitone flat can still meet a neighbouring note a semitone higher within the timing
    // tolerance, hence a few points.
    let cases = [(30.0, 100.0, 100.0), (70.0, 60.0, 60.0), (-100.0, 0.0, 5.0), (-60.0, 80.0, 80.0)];
    for (cents, lo, hi) in cases {
        let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t, cents / 100.0));
        let o = r.overall.unwrap();
        assert!(o >= lo - 0.5 && o <= hi + 0.5, "{cents} cents: overall {o}");
        assert_eq!(r.tendency_cents, Some(cents), "{cents} cents: tendency");
    }
    // Relaxed and strict move the bands.
    let relaxed = grade(&g, ScoreConfig { strictness: Strictness::Relaxed, ..std_cfg() }, KeyTimeline::constant(0), |t| guide_at(&g, t, 0.7));
    assert_eq!(relaxed.overall, Some(100.0));
    let strict = grade(&g, ScoreConfig { strictness: Strictness::Strict, ..std_cfg() }, KeyTimeline::constant(0), |t| guide_at(&g, t, 0.45));
    assert!(strict.overall.unwrap() < 60.0, "strict 45 cents: {:?}", strict.overall);
}

#[test]
fn octaves_are_forgiven_unless_strict_and_auto_finds_the_octave() {
    let g = melody(20);
    let low = |t: f64| guide_at(&g, t, -12.0);
    let auto = grade(&g, std_cfg(), KeyTimeline::constant(0), low);
    assert_eq!(auto.overall, Some(100.0));
    assert_eq!(auto.octave, -1);
    let fixed = grade(&g, ScoreConfig { octave_mode: OctaveMode::Fixed, octave_shift: 1, ..std_cfg() }, KeyTimeline::constant(0), low);
    assert_eq!(fixed.overall, Some(100.0));
    assert_eq!(fixed.octave, 1, "fixed reports the shift in use");
    let strict = grade(&g, ScoreConfig { octave_mode: OctaveMode::Strict, ..std_cfg() }, KeyTimeline::constant(0), low);
    assert_eq!(strict.overall, Some(0.0));
    // Auto settles after settle_s of singing and the lane moves there.
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    let mut settled_at = None;
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        s.push(at(t, 0, guide_at(&g, t, 12.0)));
        if settled_at.is_none() && s.octave().1 {
            settled_at = Some(t);
        }
    }
    assert_eq!(s.octave(), (1, true));
    let at = settled_at.unwrap();
    assert!(at > 3.0 && at < 6.0, "settled at {at}");
}

#[test]
fn timing_tolerance_forgives_small_delays_only() {
    let g = melody(20);
    for (late, min, max) in [(0.1, 100.0, 100.0), (-0.12, 100.0, 100.0), (0.4, 20.0, 50.0)] {
        let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t - late, 0.0));
        let o = r.overall.unwrap();
        assert!(o >= min && o <= max, "late {late}: {o}");
        if late.abs() < 0.15 {
            for n in &r.notes {
                assert!((n.delta_s - late).abs() < 0.011, "note {} offset {}", n.note, n.delta_s);
            }
        }
    }
}

#[test]
fn vibrato_and_scoops_are_not_punished() {
    let g = melody(20);
    // +-80 cents of vibrato at 5.5 Hz around the note: judged by its centre.
    let vib = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        guide_at(&g, t, 0.0).map(|m| m + 0.8 * (2.0 * std::f64::consts::PI * 5.5 * t).sin())
    });
    assert!(vib.overall.unwrap() >= 97.0, "vibrato {:?}", vib.overall);
    // A 90 ms scoop from a semitone below into every note.
    let scoop = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let n = g.notes.iter().find(|n| t >= n.start && t < n.end)?;
        let u = t - n.start;
        Some(if u < 0.09 { n.midi - 1.0 + u / 0.09 } else { n.midi })
    });
    assert!(scoop.overall.unwrap() >= 97.0, "scoop {:?}", scoop.overall);
    // The guide's own glide is not judged at all.
    let mut gg = g.clone();
    for n in &mut gg.notes {
        n.glide_in = 0.2;
    }
    let glide = grade(&gg, std_cfg(), KeyTimeline::constant(0), |t| {
        let n = gg.notes.iter().find(|n| t >= n.start && t < n.end)?;
        Some(if t - n.start < 0.2 { n.midi + 3.0 } else { n.midi })
    });
    assert!(glide.overall.unwrap() >= 99.0, "glide {:?}", glide.overall);
}

#[test]
fn one_note_throughout_and_noise_score_low() {
    let g = melody(20);
    let one = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t, 0.0).map(|_| 59.85));
    assert!(one.overall.unwrap() < 30.0, "one note {:?}", one.overall);
}

#[test]
fn notes_keep_the_key_heard_when_they_were_sung() {
    let g = melody(20);
    // Key 0 until 5.3 s, then -3: the singer follows the key heard.
    let mut keys = KeyTimeline::constant(0);
    keys.change(0, 5.3, -3);
    let follows = |t: f64| guide_at(&g, t, if t < 5.3 { 0.0 } else { -3.0 });
    let r = grade(&g, std_cfg(), keys.clone(), follows);
    // (The one moment at the switch itself, half-way between two values, may miss.)
    assert!(r.overall.unwrap() >= 99.8, "{:?}", r.overall);
    assert_eq!(r.key_changes, 1);
    assert_eq!(r.key, -3, "most of the graded time was at -3");
    // A singer who stays in the old key fails every moment after the change: the score is
    // the share of the graded note time before 5.3 s (the note sung across the change
    // keeps its first part). A little more may leak in where the timing tolerance lets a
    // note borrow the end of its neighbour (here note 1's target at -3 is note 0's pitch).
    let stays = grade(&g, std_cfg(), keys.clone(), |t| guide_at(&g, t, 0.0));
    let total: f64 = g.notes.iter().map(|n| n.end - n.start).sum();
    let before: f64 = g.notes.iter().map(|n| (n.end.min(5.3) - n.start).max(0.0)).sum();
    let o = stays.overall.unwrap();
    let expect = 100.0 * before / total;
    assert!(o >= expect - 0.5 && o <= expect + 2.0, "stays in key 0: {o} (expected {expect:.1})");
    // Each note reports the key heard at its midpoint.
    for n in &r.notes {
        let m = g.notes[n.note].midpoint();
        assert_eq!(n.key, if m < 5.3 { 0 } else { -3 });
    }
    // A wrongly shifted guide (by 2 semitones) fails: lowest band. Six of the ten notes of
    // this melody have a neighbour exactly 2 semitones higher, whose end or start a note
    // may borrow through the timing tolerance (at most 50 ms of a 400 ms note here, as the
    // notes are 100 ms apart), hence a few points.
    let wrong = grade(&g, std_cfg(), keys.shifted(2), follows);
    assert!(wrong.overall.unwrap() < 10.0, "{:?}", wrong.overall);
    // A semitone away does not even get that: no neighbour is 1 semitone higher.
    let wrong1 = grade(&g, std_cfg(), keys.shifted(-1), follows);
    assert!(wrong1.overall.unwrap() < 10.0, "{:?}", wrong1.overall);
}

#[test]
fn ungraded_notes_exclusions_and_unknown_stretches() {
    let mut g = melody(20);
    g.notes[1].confidence = 0.3;
    g.notes[2].pitched = false;
    g.notes[3].octave_flagged = true;
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), vec![(g.notes[4].start, g.notes[4].end)], g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        // Unknown during note 5: the singer is silent there but is not charged for it.
        let unknown = t >= g.notes[5].start - 0.05 && t < g.notes[5].end + 0.05;
        let midi = if unknown { None } else { guide_at(&g, t, 0.0) };
        s.push(SingerSample { unknown, ..at(t, 0, midi) });
    }
    let r = s.finish(&FinishContext { completed: true, flags: vec![] });
    assert_eq!(r.total, 20);
    assert_eq!(r.graded, 15, "4 ungraded by the guide or excluded, 1 unknown");
    assert_eq!(r.unknown_notes, 1);
    assert_eq!(r.overall, Some(100.0));
    // Fewer than 60 % gradable -> uncertain guide.
    let mut u = melody(20);
    for n in u.notes.iter_mut().take(10) {
        n.confidence = 0.2;
    }
    let r = grade(&u, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&u, t, 0.0));
    assert!(r.trust.iter().any(|f| f == "uncertain_guide"), "{:?}", r.trust);
}

#[test]
fn stopping_early_grades_what_was_sung() {
    let g = melody(20);
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    let stop = g.notes[9].end + 0.02;
    for i in 0..(stop / HOP) as usize {
        let t = i as f64 * HOP;
        s.push(at(t, 0, guide_at(&g, t, 0.0)));
    }
    let r = s.finish(&FinishContext { completed: false, flags: vec![] });
    assert_eq!(r.total, 10);
    assert_eq!(r.overall, Some(100.0));
    assert!((r.sung_share - 0.5).abs() <= 0.01, "{}", r.sung_share);
    assert!(r.trust.iter().any(|f| f == "stopped_early"));
    assert_eq!(r.lines.rows.len(), 1, "only the first line was reached");
}

#[test]
fn a_note_sung_again_after_a_seek_keeps_its_latest_attempt() {
    let g = melody(10);
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), 20.0);
    // First pass: silent through the whole song (segment 0).
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        s.push(SingerSample { t, segment: 0, midi: None, level_db: -80.0, unknown: false, music: false, clarity: 0.1 });
    }
    // Seek back to 0 (segment 1) and sing perfectly.
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        s.push(at(t, 1, guide_at(&g, t, 0.0)));
    }
    let r = s.finish(&FinishContext { completed: true, flags: vec![] });
    assert_eq!(r.seeks, 1);
    assert_eq!(r.overall, Some(100.0));
}

#[test]
fn the_note_sung_right_before_a_seek_is_graded() {
    // Sung perfectly to 50 ms after note 9 ends (less than the timing margin), then a seek
    // forward to just before note 15 (segment 1), sung to the end: notes 0-9 and 15-19.
    let g = melody(20);
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    let stop = g.notes[9].end + 0.05;
    let mut i = 0;
    while i as f64 * HOP < stop {
        let t = i as f64 * HOP;
        s.push(at(t, 0, guide_at(&g, t, 0.0)));
        i += 1;
    }
    let resume = g.notes[15].start - 0.2;
    let mut i = 0;
    while resume + i as f64 * HOP < g.duration_s {
        let t = resume + i as f64 * HOP;
        s.push(at(t, 1, guide_at(&g, t, 0.0)));
        i += 1;
    }
    let r = s.finish(&FinishContext { completed: true, flags: vec![] });
    let ids: Vec<usize> = r.notes.iter().map(|n| n.note).collect();
    assert_eq!(ids, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 18, 19]);
    assert_eq!((r.graded, r.total), (15, 15));
    assert_eq!(r.overall, Some(100.0));
}

#[test]
fn untrusted_takes_have_no_grade_and_line_rows_are_rounded() {
    let g = melody(20);
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        s.push(at(t, 0, guide_at(&g, t, 0.7)));
    }
    let p = s.progress();
    assert_eq!((p.graded, p.total), (20, 20));
    let r = s.finish(&FinishContext { completed: true, flags: vec!["music_in_input".into(), "untrusted".into()] });
    assert_eq!(r.overall, None);
    assert_eq!(r.grade, None);
    assert_eq!(r.trust, vec!["music_in_input".to_string(), "untrusted".to_string()]);
    assert_eq!(r.lines.rows.len(), 2);
    for row in &r.lines.rows {
        assert_eq!(row.2, None);
        assert_eq!(row.4, Some(70.0));
        assert_eq!(row.1, (row.1 * 10.0).round() / 10.0);
    }
}

#[test]
fn live_hits_light_the_bar_where_the_singer_is_on_the_note() {
    let g = melody(4);
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    let mut live = ScoreLive::default();
    let mut lit = 0.0;
    let mut above = 0.0;
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        // On the first half of each note, then a semitone sharp.
        let midi = g.notes.iter().find(|n| t >= n.start && t < n.end).map(|n| {
            if t < n.midpoint() { n.midi } else { n.midi + 1.0 }
        });
        s.push(at(t, 0, midi));
        s.drain_live(&mut live);
        for h in &live.hits {
            match h.3 {
                0 => lit += h.2 - h.1,
                1 => above += h.2 - h.1,
                _ => panic!("below"),
            }
        }
    }
    assert!((lit - 0.8).abs() < 0.05, "lit {lit}");
    assert!((above - 0.8).abs() < 0.05, "above {above}");
}

#[test]
fn the_same_input_gives_the_same_result() {
    let g = melody(20);
    let singer = |t: f64| guide_at(&g, t, 0.3 + 0.4 * (7.0 * t).sin());
    let a = grade(&g, std_cfg(), KeyTimeline::constant(2), singer);
    let b = grade(&g, std_cfg(), KeyTimeline::constant(2), singer);
    assert_eq!(a, b);
}

#[test]
fn note_names_follow_the_spelling() {
    assert_eq!(note_name(60, false), "C4");
    assert_eq!(note_name(57, false), "A3");
    assert_eq!(note_name(61, false), "C#4");
    assert_eq!(note_name(61, true), "Db4");
    assert_eq!(note_name(36, true), "C2");
    // A C4 at -3 is an A3 (APP_API 12.1).
    assert_eq!(note_name(60 - 3, true), "A3");
    assert_eq!(fold_cents(1190.0), -10.0);
    assert_eq!(fold_cents(-1230.0), -30.0);
    assert!((hz_to_midi(440.0) - 69.0).abs() < 1e-12);
}

#[test]
fn key_timeline_rules() {
    let mut k = KeyTimeline::constant(0);
    k.change(0, 10.0, -3);
    k.change(0, 20.0, 2);
    assert_eq!(k.key_at(5.0, Some(0)), 0);
    assert_eq!(k.key_at(10.0, Some(0)), -3);
    assert_eq!(k.key_at(25.0, None), 2);
    // A re-scheduled change replaces the later ones.
    k.change(0, 15.0, 5);
    assert_eq!(k.key_at(25.0, Some(0)), 5);
    assert_eq!(k.changes_between(0, 0.0, 30.0), 2);
    // A new segment (seek) has its own history; the old one is kept.
    k.start_segment(1, 3.0, 4);
    assert_eq!(k.key_at(12.0, Some(1)), 4);
    assert_eq!(k.key_at(12.0, Some(0)), -3);
    assert_eq!(k.shifted(1).key_at(12.0, Some(0)), -2);
}

/// Grades with per-moment levels and clarity: `singer` gives (pitch, level dBFS, clarity).
fn grade_lc(g: &ScoreGuide, cfg: ScoreConfig, singer: impl Fn(f64) -> (Option<f64>, f32, f32)) -> ScoreResult {
    let mut s = Scorer::new(cfg, g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        let (midi, level_db, clarity) = singer(t);
        s.push(SingerSample { t, segment: 0, midi, level_db, unknown: false, music: false, clarity });
    }
    s.finish(&FinishContext { completed: true, flags: vec![] })
}

#[test]
fn only_periodic_sound_the_tracker_could_not_resolve_is_excused() {
    let g = melody(20);
    // The pitch of the note sounding at t, and how far into it t is (0..1).
    let note = |t: f64| g.notes.iter().find(|n| t >= n.start && t < n.end).map(|n| (n.midi, (t - n.start) / (n.end - n.start)));
    // The middle 23 % of every note (90 ms): no pitch named. The timing tolerance cannot
    // skip the middle of a note and the 40 ms bridge cannot span it, so only the excusal
    // rule decides.
    let mid = |u: f64| (0.385..0.615).contains(&u);
    let with = |level: f32, clarity: f32| {
        grade_lc(&g, std_cfg(), move |t| match note(t) {
            Some((_, u)) if mid(u) => (None, level, clarity),
            Some((m, _)) => (Some(m), -20.0, 0.95),
            None => (None, -90.0, 0.1),
        })
    };
    // Periodic sound the tracker did not resolve (a low or rough voice): excused.
    let voiced = with(-20.0, 0.85);
    assert_eq!(voiced.overall, Some(100.0), "voiced but unresolved");
    assert!(voiced.voiced_share < 0.85, "the raw voiced share still says what was heard: {}", voiced.voiced_share);
    assert!(voiced.notes.iter().all(|n| n.excused > 0));
    // Breath, whisper or a consonant (sound, but not periodic), and periodic sound too
    // quiet to be the singer: never excused, they score exactly what silence scores (the
    // 70 of 90 ms not reached by the samples either side lost).
    let silence = with(-90.0, 0.1).overall.unwrap();
    assert!((80.0..=85.0).contains(&silence), "silence: {silence}");
    for (what, level, clarity) in [("breath", -20.0, 0.45), ("quiet", -56.0, 0.85)] {
        assert_eq!(with(level, clarity).overall, Some(silence), "{what}");
    }
    // A note with no pitch named at all scores nothing, whatever the sound.
    let unresolved = grade_lc(&g, std_cfg(), |t| if note(t).is_some() { (None, -20.0, 0.9) } else { (None, -90.0, 0.1) });
    assert_eq!(unresolved.overall, Some(0.0));
    // Half of every note's middle unresolved: at most a quarter of the moments excused
    // (20 of the 40 moments sung, 10 excused: 20 / 30, and the moment either side of the
    // gap takes the sample next to it: 22 / 30).
    let half = grade_lc(&g, std_cfg(), |t| match note(t) {
        Some((m, u)) => if (0.25..0.75).contains(&u) { (None, -20.0, 0.9) } else { (Some(m), -20.0, 0.95) },
        None => (None, -90.0, 0.1),
    });
    let o = half.overall.unwrap();
    assert!((65.0..=74.0).contains(&o), "half unresolved: {o}");
    assert!(half.notes.iter().all(|n| n.excused * 4 <= n.covered), "the cap holds");
    // The old rule (any sound, scorer 0.1.0) would have excused the breath.
    let old = ScoreConfig { excuse_clarity: 0.0, ..std_cfg() };
    let breath_old = grade_lc(&g, old, |t| match note(t) {
        Some((_, u)) if mid(u) => (None, -20.0, 0.45),
        Some((m, _)) => (Some(m), -20.0, 0.95),
        None => (None, -90.0, 0.1),
    });
    assert_eq!(breath_old.overall, Some(100.0));
    // Returned music is never the singer's sound.
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        let (midi, music, level_db) = match note(t) {
            Some((_, u)) if mid(u) => (None, true, -20.0),
            Some((m, _)) => (Some(m), false, -20.0),
            None => (None, false, -90.0),
        };
        s.push(SingerSample { t, segment: 0, midi, level_db, unknown: false, music, clarity: 0.9 });
    }
    let r = s.finish(&FinishContext { completed: true, flags: vec![] });
    assert_eq!(r.overall, Some(silence), "music is not excused");
}

#[test]
fn being_early_or_late_up_to_the_tolerance_costs_nothing() {
    let g = melody(20);
    // A singer (or an unmeasured device delay) steadily 140 ms late: full marks, every note
    // matched by a plain shift of 140 ms.
    let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t - 0.14, 0.0));
    assert_eq!(r.overall, Some(100.0));
    for n in &r.notes {
        assert!((n.start_shift_s - 0.14).abs() < 0.011 && (n.end_shift_s - 0.14).abs() < 0.011, "note {}: {} {}", n.note, n.start_shift_s, n.end_shift_s);
    }
    // On time, but one note sung 150 ms late (held until the next note starts): full marks.
    let one_late = 12usize;
    let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let late = if t >= g.notes[one_late].start && t < g.notes[one_late].end + 0.15 { 0.15 } else { 0.0 };
        guide_at(&g, t - late, 0.0)
    });
    assert_eq!(r.overall, Some(100.0), "one note late");
    // Rubato: every note's onset moved by its own offset in +-150 ms, each note held until
    // the next one starts (and kept for at least half its length): full marks.
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut spans: Vec<(f64, f64, f64)> = Vec::new();
    for n in &g.notes {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let o = 0.15 * ((rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0);
        let s0 = spans.last().map_or(f64::NEG_INFINITY, |p| p.0 + 0.5 * 0.4);
        let s = (n.start + o).max(s0);
        if let Some(p) = spans.last_mut() {
            p.1 = p.1.min(s);
        }
        spans.push((s, n.end + o, n.midi));
    }
    let rubato = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| spans.iter().find(|p| t >= p.0 && t < p.1).map(|p| p.2));
    assert_eq!(rubato.overall, Some(100.0), "rubato +-150 ms");
    // A note sung for only part of its length: the tolerance forgives up to 150 ms at its
    // ends, never its middle, and never a rendition shorter than half the note.
    let late_entry = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let n = g.notes.iter().find(|n| t >= n.start + 0.18 && t < n.end)?;
        Some(n.midi)
    });
    // (180 ms late into a 400 ms note: the 30 ms beyond the tolerance are lost, of 250:
    // 88 %, and up to one moment more is gained where the sampling grid meets the onset.)
    let o = late_entry.overall.unwrap();
    assert!((86.0..=93.0).contains(&o), "late entry by 180 ms: {o}");
    let staccato = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let n = g.notes.iter().find(|n| t >= n.start && t < n.start + 0.1)?;
        Some(n.midi)
    });
    // (100 ms of 400: stretched at most to a 250 ms rendition, 100 / 250 = 40 %, and up to
    // one moment more at the sampling grid.)
    let o = staccato.overall.unwrap();
    assert!((38.0..=43.0).contains(&o), "100 ms of each 400 ms note: {o}");
}

#[test]
fn one_stretch_of_singing_is_credited_to_one_note_only() {
    // Two back-to-back notes sung in the wrong order: each note's pitch is sung, but in the
    // other note's place. Matched each on its own, both would take 150 ms of the other's
    // place; renditions must keep their order, so only one of them can.
    let notes = vec![
        ScoreNote { start: 1.0, end: 1.2, midi: 60.0, pitched: true, confidence: 1.0, octave_flagged: false, glide_in: 0.0, line: None },
        ScoreNote { start: 1.2, end: 1.4, midi: 64.0, pitched: true, confidence: 1.0, octave_flagged: false, glide_in: 0.0, line: None },
    ];
    let g = ScoreGuide { lines: ScoreGuide::phrases(&notes), notes, low_confidence: 0.5, basis: LineBasis::Phrases, outdated: false, duration_s: 2.5, label: "t".into() };
    let swapped = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        if (1.0..1.2).contains(&t) {
            Some(64.0)
        } else if (1.2..1.4).contains(&t) {
            Some(60.0)
        } else {
            None
        }
    });
    let credits: Vec<f64> = swapped.notes.iter().map(|n| n.credit).collect();
    assert!(credits.iter().filter(|&&c| c > 0.05).count() <= 1, "both notes borrowed: {credits:?}");
    assert!(swapped.overall.unwrap() < 40.0, "{:?}", swapped.overall);
    // Each rendition starts after the previous one ended.
    let n = &swapped.notes;
    assert!(g.notes[1].start + n[1].start_shift_s >= g.notes[0].end + n[0].end_shift_s - 0.0101, "{n:?}");
}

#[test]
fn the_borrowing_adversary_scores_low() {
    let g = melody(20);
    // Every note sung at the previous note's pitch, 150 ms early (the first note silent).
    let behind_early = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let u = t + 0.15;
        let i = g.notes.iter().position(|n| u >= n.start && u < n.end)?;
        (i > 0).then(|| g.notes[i - 1].midi)
    });
    let o = behind_early.overall.unwrap();
    assert!(o < 60.0, "behind, early: {o}");
    // The same a whole note late, on time and one note ahead.
    for (name, shift, step) in [("behind", 0.0, -1i64), ("ahead, late", -0.15, 1)] {
        let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
            let u = t + shift;
            let i = g.notes.iter().position(|n| u >= n.start && u < n.end)? as i64;
            g.notes.get((i + step) as usize).filter(|_| i + step >= 0).map(|n| n.midi)
        });
        let o = r.overall.unwrap();
        assert!(o < 60.0, "{name}: {o}");
    }
}

#[test]
fn note_names_at_every_key() {
    // The C4 of the original key, at every key -12..12, in both spellings (APP_API 12.1:
    // "a C4 at -3 is named A3").
    let sharps = ["C3", "C#3", "D3", "D#3", "E3", "F3", "F#3", "G3", "G#3", "A3", "A#3", "B3", "C4", "C#4", "D4", "D#4", "E4", "F4", "F#4", "G4", "G#4", "A4", "A#4", "B4", "C5"];
    let flats = ["C3", "Db3", "D3", "Eb3", "E3", "F3", "Gb3", "G3", "Ab3", "A3", "Bb3", "B3", "C4", "Db4", "D4", "Eb4", "E4", "F4", "Gb4", "G4", "Ab4", "A4", "Bb4", "B4", "C5"];
    for key in -12i32..=12 {
        let i = (key + 12) as usize;
        assert_eq!(note_name(60 + key, false), sharps[i], "key {key}");
        assert_eq!(note_name(60 + key, true), flats[i], "key {key}");
    }
    // The scorer's target follows the key heard exactly, unrounded (a note sung at
    // MIDI 64.27 at key -3 is targeted at 61.27, named C#4 / Db4).
    let mut g = melody(2);
    g.notes[0].midi = 64.27;
    let s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(-3), Vec::new(), g.duration_s);
    let (i, target, key, graded) = s.target_at(g.notes[0].start + 0.1, None).unwrap();
    assert_eq!((i, key, graded), (0, -3, true));
    assert!((target - 61.27).abs() < 1e-9);
    assert_eq!(note_name(target.round() as i32, true), "Db4");
}

#[test]
fn the_fast_credit_table_equals_the_plain_sums() {
    // Random singers (voiced on pitch, off pitch, unvoiced periodic, silent) under every
    // timing: the prefix-sum table equals the moment-by-moment sums.
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    for &(len, strictness) in &[(0.05, Strictness::Standard), (0.23, Strictness::Strict), (0.4, Strictness::Standard), (1.37, Strictness::Relaxed), (3.0, Strictness::Standard)] {
        let notes = vec![ScoreNote { start: 1.0, end: 1.0 + len, midi: 57.0, pitched: true, confidence: 1.0, octave_flagged: false, glide_in: 0.0, line: None }];
        let g = ScoreGuide { lines: ScoreGuide::phrases(&notes), notes, low_confidence: 0.5, basis: LineBasis::Phrases, outdated: false, duration_s: len + 3.0, label: "t".into() };
        let mut s = Scorer::new(ScoreConfig { strictness, ..std_cfg() }, g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
        let bands = strictness.bands();
        let k_n = Scorer::moments(&g.notes[0], HOP);
        let j = (bands.tolerance_s / HOP).round() as usize;
        for m in 0..k_n + 2 * j {
            let x = next();
            s.sc_val[m] = if x < 0.6 { 57.0 + 1.5 * (next() - 0.5) } else { f64::NAN };
            s.sc_cov[m] = C_COVERED | if x > 0.8 { C_LOUD } else { 0 };
        }
        s.sc_tgt[..k_n].fill(57.0);
        let max_excused = k_n / 4;
        let min_len = -((0.5 * k_n as f64).floor() as i64);
        s.fill_table_fast(0, k_n, j, min_len, k_n, max_excused, bands);
        let (ji, w) = (j as i64, 2 * j + 1);
        for alpha in -ji..=ji {
            for beta in -ji..=ji {
                let fast = s.tab[(alpha + ji) as usize * w + (beta + ji) as usize];
                let plain = if beta - alpha < min_len {
                    f64::NEG_INFINITY
                } else {
                    let (sum, exc) = s.warped_sum(k_n, j, alpha, beta, bands);
                    sum / (k_n - exc.min(max_excused)) as f64
                };
                assert!(fast == plain || (fast - plain).abs() < 1e-9, "len {len} ({alpha}, {beta}): fast {fast} plain {plain}");
            }
        }
    }
}

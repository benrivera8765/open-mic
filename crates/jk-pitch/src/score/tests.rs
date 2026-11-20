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
    SingerSample { t, segment, midi, level_db: if midi.is_some() { -20.0 } else { -90.0 }, unknown: false, music: false }
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
        s.push(SingerSample { t, segment: 0, midi: None, level_db: -80.0, unknown: false, music: false });
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

/// Grades with per-moment levels: `singer` gives (pitch, level dBFS).
fn grade_lv(g: &ScoreGuide, cfg: ScoreConfig, singer: impl Fn(f64) -> (Option<f64>, f32)) -> ScoreResult {
    let mut s = Scorer::new(cfg, g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        let (midi, level_db) = singer(t);
        s.push(SingerSample { t, segment: 0, midi, level_db, unknown: false, music: false });
    }
    s.finish(&FinishContext { completed: true, flags: vec![] })
}

#[test]
fn moments_the_tracker_could_not_hear_are_excused_only_while_the_singer_makes_sound() {
    let g = melody(20);
    // The pitch of the note sounding at t, and how far into it t is (0..1).
    let note = |t: f64| g.notes.iter().find(|n| t >= n.start && t < n.end).map(|n| (n.midi, (t - n.start) / (n.end - n.start)));
    // The tracker names no pitch for the first fifth of every note (a slow onset), while
    // the singer is singing loudly: excused, full marks.
    let onset = grade_lv(&g, std_cfg(), |t| match note(t) {
        Some((m, u)) => (if u < 0.2 { None } else { Some(m) }, -20.0),
        None => (None, -90.0),
    });
    assert_eq!(onset.overall, Some(100.0), "slow onsets");
    assert!(onset.voiced_share < 0.85, "the raw voiced share still says what was heard: {}", onset.voiced_share);
    assert!(onset.notes.iter().all(|n| n.excused > 0));
    // The same with silence instead of sound: not excused.
    let late_entry = grade_lv(&g, std_cfg(), |t| match note(t) {
        Some((m, u)) if u >= 0.45 => (Some(m), -20.0),
        _ => (None, -90.0),
    });
    let o = late_entry.overall.unwrap();
    assert!(o < 75.0, "silence is never excused: {o}");
    // A note spoken or whispered (sound, never a pitch) scores nothing.
    let spoken = grade_lv(&g, std_cfg(), |t| (None, if note(t).is_some() { -20.0 } else { -90.0 }));
    assert_eq!(spoken.overall, Some(0.0));
    // Half sung on pitch, half spoken: at most two thirds.
    let half = grade_lv(&g, std_cfg(), |t| match note(t) {
        Some((m, u)) => (if u < 0.5 { Some(m) } else { None }, -20.0),
        None => (None, -90.0),
    });
    let o = half.overall.unwrap();
    // (20 of 40 moments sung, 10 excused: 20/30, plus one moment the 10 ms offset gains.)
    assert!((60.0..=70.5).contains(&o), "half sung: {o}");
    // Returned music is never "sound" of the singer.
    let mut s = Scorer::new(std_cfg(), g.clone(), KeyTimeline::constant(0), Vec::new(), g.duration_s);
    for i in 0..(g.duration_s / HOP) as usize {
        let t = i as f64 * HOP;
        let (midi, music, level_db) = match note(t) {
            Some((m, u)) if u >= 0.2 => (Some(m), false, -20.0),
            Some(_) => (None, true, -20.0),
            None => (None, false, -90.0),
        };
        s.push(SingerSample { t, segment: 0, midi, level_db, unknown: false, music });
    }
    let r = s.finish(&FinishContext { completed: true, flags: vec![] });
    assert!(r.overall.unwrap() < 85.0, "music is not excused: {:?}", r.overall);
}

#[test]
fn a_steady_delay_costs_nothing_and_one_note_cannot_move_the_whole_tolerance() {
    let g = melody(20);
    // A singer (or an unmeasured device delay) steadily 140 ms late: full marks, and every
    // note after the first ones is matched at the usual timing.
    let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| guide_at(&g, t - 0.14, 0.0));
    assert_eq!(r.overall, Some(100.0));
    for n in r.notes.iter().skip(2) {
        assert!((n.delta_s - 0.14).abs() < 0.011, "note {} offset {}", n.note, n.delta_s);
    }
    // On time, but one note sung 150 ms late: once the usual timing (0) is known that note
    // is matched within 80 ms of it, and loses the rest.
    let one_late = 12usize;
    let r = grade(&g, std_cfg(), KeyTimeline::constant(0), |t| {
        let late = if t >= g.notes[one_late].start && t < g.notes[one_late].end + 0.2 { 0.15 } else { 0.0 };
        guide_at(&g, t - late, 0.0)
    });
    let n = r.notes.iter().find(|n| n.note == one_late).unwrap();
    assert!((n.delta_s - 0.08).abs() < 0.011, "jitter limit: {}", n.delta_s);
    assert!(n.credit > 0.8 && n.credit < 0.9, "credit {}", n.credit);
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

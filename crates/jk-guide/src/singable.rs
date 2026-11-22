// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody as a singer reads it: the stored guide's notes cleaned up for a practice
//! session (owner, 2026-11-22: "the path to" is all E4 yet the bars were "all jumbled and
//! at different heights", "super unprofessional"; DECISIONS D-251).
//!
//! The stored guide keeps the pitch as measured, in fractions of a semitone, with every
//! scoop, fall and voiceless fragment the analysis found (SONG_FORMAT 10.1; the file is
//! not changed). Drawn as they are, three notes sung on one key sit at three heights and a
//! word shows a cluster of slivers. A session uses this view instead, for the lane **and**
//! the grade, so what is drawn is what is graded:
//!
//! 1. Every note sits on a piano key: its pitch rounded to the nearest semitone of the
//!    recording's own tuning (`round(midi - tuning) + tuning`).
//! 2. Voiceless notes (`unpitched`) that have a pitch become ordinary notes at that key,
//!    never graded (confidence held under the threshold); those without one are dropped.
//!    Spoken stretches shorter than 1 s are detection noise and are dropped; longer ones
//!    (a rap part) keep their notes out of the lane.
//! 3. Within a word, slivers shorter than 120 ms join the touching note (the one on the
//!    same key, else the longer); a scoop or fall of one semitone shorter than 200 ms
//!    joins the note at least twice its length it leads into or out of; touching notes on
//!    the same key merge. A sliver with no word joins any touching note, and a sliver on
//!    the same key as a touching note of the next or previous word joins it.
//! 4. A note that jumps 9 semitones or more away from BOTH its neighbours (the same way)
//!    is a wrong octave of the analysis (owner, 2026-11-22: a note "way too high" in a
//!    chorus, 81 between 62..71, exactly an octave above the line): it moves an octave
//!    toward them when that brings it nearer to both. A real leap is a phrase, not one
//!    note, so it is left alone. Grading folds octaves in Auto anyway; this is what the
//!    singer sees.
//! 5. Short (< 200 ms), unsure notes outside every lyric word, very unsure notes
//!    (confidence < 0.2, < 500 ms) that are not their word's only note, and any note under
//!    60 ms that is not its word's only note, are dropped.
//!
//! The result keeps time order and never overlaps. Word references are kept (a merged
//! note keeps its first part's).

use jk_format::guide::{GuideDoc, GuideNote, Span};

/// Slivers shorter than this join a touching note of the same word.
const SLIVER_S: f64 = 0.12;
/// A one-semitone scoop or fall shorter than this joins the note it leads into or out of.
const SCOOP_S: f64 = 0.20;
/// Notes this close (s) count as touching.
const TOUCH_S: f64 = 0.06;
/// Same-key notes this close merge.
const MERGE_GAP_S: f64 = 0.08;
/// Spoken spans shorter than this are dropped.
const MIN_SPOKEN_S: f64 = 1.0;
/// The longest slide kept as `glide_in` when a scoop joins the note after it.
const MAX_GLIDE_S: f64 = 0.15;
/// A note this many semitones from both neighbours (same direction) is an octave spike.
const SPIKE_ST: f64 = 9.0;
/// Neighbours further apart than this (s) do not count for the spike rule.
const SPIKE_GAP_S: f64 = 1.5;
/// Very unsure: below this confidence a short note that is not its word's only note goes.
const JUNK_CONFIDENCE: f64 = 0.2;
const JUNK_S: f64 = 0.5;

fn dur(n: &GuideNote) -> f64 {
    n.end - n.start
}

fn word_of(n: &GuideNote) -> Option<(usize, usize)> {
    n.word.as_ref().map(|w| (w.0, w.1))
}

/// `a` (a sliver or scoop) may join neighbour `b`: same word, or `a` has no word.
fn may_join(a: &GuideNote, b: &GuideNote) -> bool {
    match (word_of(a), word_of(b)) {
        (None, _) => true,
        (Some(x), Some(y)) => x == y,
        (Some(_), None) => false,
    }
}

fn same_key(a: &GuideNote, b: &GuideNote) -> bool {
    (a.midi - b.midi).abs() < 0.01
}

/// Joins note `from` into note `into` (adjacent indices); `into` takes the union of times.
fn absorb(notes: &mut Vec<GuideNote>, from: usize, into: usize) {
    let f = notes[from].clone();
    let n = &mut notes[into];
    let old_start = n.start;
    if f.start < n.start {
        // A scoop into the note: the slide stays forgiven (glide_in).
        n.start = f.start;
        if !same_key(&f, n) {
            n.glide_in = (n.glide_in + (old_start - f.start)).min(MAX_GLIDE_S);
        } else {
            n.glide_in = f.glide_in;
        }
        if n.word.is_none() {
            n.word = f.word.clone();
        }
    }
    if f.end > n.end {
        n.end = f.end;
    }
    notes.remove(from);
}

/// One pass of step 3; true when something changed.
fn tidy_once(notes: &mut Vec<GuideNote>) -> bool {
    // Same-key merges first (they only ever lengthen notes).
    for i in 0..notes.len().saturating_sub(1) {
        let (a, b) = (&notes[i], &notes[i + 1]);
        if same_key(a, b) && b.start - a.end <= MERGE_GAP_S && word_of(a) == word_of(b) {
            let (da, db) = (dur(a), dur(b));
            let conf = (a.confidence * da + b.confidence * db) / (da + db).max(1e-9);
            let octave = if db > da { b.octave.clone() } else { a.octave.clone() };
            let end = b.end;
            let n = &mut notes[i];
            n.end = end;
            n.confidence = conf;
            n.octave = octave;
            notes.remove(i + 1);
            return true;
        }
    }
    // Slivers, shortest first so a cluster folds into its main note.
    let mut order: Vec<usize> = (0..notes.len()).filter(|&i| dur(&notes[i]) < SLIVER_S).collect();
    order.sort_by(|&a, &b| dur(&notes[a]).total_cmp(&dur(&notes[b])));
    for i in order {
        let n = &notes[i];
        let prev = (i > 0).then(|| i - 1).filter(|&p| n.start - notes[p].end <= TOUCH_S && (may_join(n, &notes[p]) || same_key(n, &notes[p])));
        let next = (i + 1 < notes.len()).then_some(i + 1).filter(|&q| notes[q].start - n.end <= TOUCH_S && (may_join(n, &notes[q]) || same_key(n, &notes[q])));
        let pick = match (prev, next) {
            (Some(p), Some(q)) => {
                let (sp, sq) = (same_key(n, &notes[p]), same_key(n, &notes[q]));
                Some(if sp && !sq {
                    p
                } else if sq && !sp {
                    q
                } else if dur(&notes[q]) > dur(&notes[p]) {
                    q
                } else {
                    p
                })
            }
            (p, q) => p.or(q),
        };
        if let Some(t) = pick {
            absorb(notes, i, t);
            return true;
        }
    }
    // One-semitone scoops and falls.
    for i in 0..notes.len() {
        let n = &notes[i];
        if dur(n) >= SCOOP_S {
            continue;
        }
        let fits = |j: usize| {
            let m = &notes[j];
            (m.midi - n.midi).abs().round() == 1.0 && dur(m) >= 2.0 * dur(n) && may_join(n, m) && word_of(n) == word_of(m)
        };
        if i + 1 < notes.len() && notes[i + 1].start - n.end <= TOUCH_S && fits(i + 1) {
            absorb(notes, i, i + 1);
            return true;
        }
        if i > 0 && n.start - notes[i - 1].end <= TOUCH_S && fits(i - 1) {
            absorb(notes, i, i - 1);
            return true;
        }
    }
    false
}

/// Step 4: moves single notes that sit an octave away from both neighbours; true when
/// something moved.
pub(crate) fn fix_octave_spikes(notes: &mut [GuideNote]) -> bool {
    let mut moved = false;
    for i in 0..notes.len() {
        let n = &notes[i];
        let prev = (i > 0).then(|| i - 1).filter(|&p| n.start - notes[p].end <= SPIKE_GAP_S);
        let next = (i + 1 < notes.len()).then_some(i + 1).filter(|&q| notes[q].start - n.end <= SPIKE_GAP_S);
        let (Some(p), Some(q)) = (prev, next) else { continue };
        let (d1, d2) = (n.midi - notes[p].midi, n.midi - notes[q].midi);
        if d1.signum() != d2.signum() || d1.abs().min(d2.abs()) < SPIKE_ST {
            continue;
        }
        let shift = -12.0 * d1.signum();
        if (d1 + shift).abs() < d1.abs() && (d2 + shift).abs() < d2.abs() {
            notes[i].midi += shift;
            moved = true;
        }
    }
    moved
}

/// Rewrites `doc`'s notes and spans as the singer's melody (module docs).
pub fn singable(doc: &mut GuideDoc) {
    let tuning = doc.tuning.use_cents / 100.0;
    let low = doc.low_confidence;
    doc.spans.retain(|s: &Span| s.kind != "spoken" || s.end - s.start >= MIN_SPOKEN_S);
    let spoken: Vec<(f64, f64)> = doc.spans.iter().filter(|s| s.kind == "spoken").map(|s| (s.start, s.end)).collect();
    let in_spoken = |t: f64| spoken.iter().any(|&(a, b)| t >= a && t <= b);

    let mut notes: Vec<GuideNote> = Vec::with_capacity(doc.notes.len());
    for n in doc.notes.drain(..) {
        let mut n = n;
        if !(n.midi.is_finite() && n.midi > 0.0) || n.end <= n.start {
            continue;
        }
        if !n.is_pitched() {
            if in_spoken(n.midpoint()) {
                continue;
            }
            n.kind = "pitched".into();
            n.octave = "unchecked".into();
            n.confidence = n.confidence.min(low - 1e-6);
        }
        n.midi = (n.midi - tuning).round() + tuning;
        notes.push(n);
    }

    let mut guard = notes.len() * 4 + 16;
    while guard > 0 && tidy_once(&mut notes) {
        guard -= 1;
    }

    let has_words = notes.iter().any(|n| n.word.is_some());
    let mut keep = vec![true; notes.len()];
    for (i, n) in notes.iter().enumerate() {
        if has_words && n.word.is_none() && dur(n) < SCOOP_S && n.confidence < low {
            keep[i] = false;
            continue;
        }
        if dur(n) < TOUCH_S || (n.confidence < JUNK_CONFIDENCE && dur(n) < JUNK_S) {
            let w = word_of(n);
            let alone = w.is_some() && notes.iter().filter(|m| word_of(m) == w).count() == 1;
            if !alone {
                keep[i] = false;
            }
        }
    }
    let mut k = keep.into_iter();
    notes.retain(|_| k.next().unwrap_or(true));
    // Step 4 on what is left (dropped notes no longer count as neighbours); a moved note
    // may now sit on its neighbour's key, and moving one can reveal the next.
    for _ in 0..4 {
        if !fix_octave_spikes(&mut notes) {
            break;
        }
        let mut guard = notes.len() * 4 + 16;
        while guard > 0 && tidy_once(&mut notes) {
            guard -= 1;
        }
    }
    // Round as the stored guide does (1 ms, 0.1 cent).
    for n in &mut notes {
        n.midi = (n.midi * 1000.0).round() / 1000.0;
        n.glide_in = n.glide_in.min(0.5 * dur(n)).max(0.0);
    }
    doc.notes = notes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use jk_format::guide::WordRef;
    use serde_json::Map;

    fn n(a: f64, b: f64, midi: f64, kind: &str, conf: f64, word: Option<(usize, usize)>) -> GuideNote {
        GuideNote {
            start: a,
            end: b,
            midi,
            confidence: conf,
            kind: kind.into(),
            octave: "ok".into(),
            glide_in: 0.0,
            word: word.map(|(l, w)| WordRef(l, w, a)),
            extra: Map::new(),
        }
    }

    fn doc(notes: Vec<GuideNote>, cents: f64) -> GuideDoc {
        let mut d: GuideDoc = serde_json::from_value(serde_json::json!({
            "guide_version": 1, "hop_s": 0.01, "low_confidence": 0.5,
            "tuning": {"cents": cents, "strength": 0.8, "use_cents": cents},
            "notes": [], "spans": [],
            "provenance": {"method": "test", "version": "0"}
        }))
        .expect("doc");
        d.notes = notes;
        d
    }

    #[test]
    fn notes_on_one_key_line_up_and_voiceless_words_become_plain_ungraded_bars() {
        // "the path to": measured 64.17, 63.94, 63.57 (unpitched) -> all on E4.
        let mut d = doc(
            vec![
                n(0.0, 0.23, 64.17, "pitched", 0.42, Some((0, 0))),
                n(0.34, 0.57, 63.94, "pitched", 0.54, Some((0, 1))),
                n(0.57, 0.74, 63.57, "unpitched", 0.48, Some((0, 2))),
            ],
            -2.1,
        );
        singable(&mut d);
        assert_eq!(d.notes.len(), 3);
        for x in &d.notes {
            assert!((x.midi - 63.979).abs() < 1e-9, "{}", x.midi);
            assert!(x.is_pitched());
        }
        assert!(!d.notes[2].graded(d.low_confidence));
    }

    #[test]
    fn slivers_scoops_and_same_key_parts_of_a_word_fold_into_one_bar() {
        let w = Some((3, 1));
        let mut d = doc(
            vec![
                n(1.0, 1.18, 65.03, "pitched", 0.64, w), // a fall leading in
                n(1.18, 1.70, 63.96, "pitched", 0.83, w),
                n(1.70, 1.74, 64.2, "unpitched", 0.85, w), // sliver
                n(1.74, 2.00, 64.1, "pitched", 0.7, w),
            ],
            0.0,
        );
        singable(&mut d);
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes.iter().map(|x| (x.start, x.end, x.midi)).collect::<Vec<_>>());
        let x = &d.notes[0];
        assert_eq!((x.start, x.end, x.midi), (1.0, 2.0, 64.0));
        assert!(x.glide_in > 0.1);
    }

    #[test]
    fn a_two_syllable_word_keeps_both_notes_and_words_never_merge() {
        let mut d = doc(
            vec![
                n(0.0, 0.19, 66.82, "pitched", 0.7, Some((0, 0))),
                n(0.19, 0.36, 65.79, "pitched", 0.7, Some((0, 0))),
                n(0.36, 0.60, 66.0, "pitched", 0.7, Some((0, 1))),
            ],
            0.0,
        );
        singable(&mut d);
        let got: Vec<_> = d.notes.iter().map(|x| (x.midi, word_of(x))).collect();
        assert_eq!(got, vec![(67.0, Some((0, 0))), (66.0, Some((0, 0))), (66.0, Some((0, 1)))]);
    }

    #[test]
    fn noise_outside_the_lyrics_and_short_spoken_spans_are_dropped() {
        let mut d = doc(
            vec![
                n(0.0, 0.5, 60.0, "pitched", 0.9, Some((0, 0))),
                n(2.0, 2.1, 71.0, "pitched", 0.3, None),
                n(3.0, 3.1, 62.0, "unpitched", 0.9, Some((0, 1))),
                n(5.0, 5.3, 62.0, "unpitched", 0.9, Some((1, 0))),
            ],
            0.0,
        );
        d.spans = vec![
            Span { start: 3.0, end: 3.2, kind: "spoken".into(), extra: Map::new() },
            Span { start: 4.5, end: 6.0, kind: "spoken".into(), extra: Map::new() },
        ];
        singable(&mut d);
        let got: Vec<_> = d.notes.iter().map(|x| (x.start, x.midi)).collect();
        assert_eq!(got, vec![(0.0, 60.0), (3.0, 62.0)]);
        assert_eq!(d.spans.len(), 1);
    }

    #[test]
    fn a_single_note_an_octave_above_its_neighbours_comes_down_but_a_sung_leap_stays() {
        // The owner's chorus: 69, 71, then 81 (A5, exactly an octave above A4), then 70.
        let mut d = doc(
            vec![
                n(0.0, 0.13, 69.0, "pitched", 0.8, Some((0, 1))),
                n(0.13, 0.39, 71.0, "pitched", 0.6, Some((0, 1))),
                n(0.56, 0.84, 81.0, "pitched", 0.97, Some((0, 2))),
                n(0.89, 1.2, 70.0, "pitched", 0.6, Some((0, 3))),
            ],
            0.0,
        );
        singable(&mut d);
        let got: Vec<f64> = d.notes.iter().map(|x| x.midi).collect();
        assert_eq!(got, vec![69.0, 71.0, 69.0, 70.0]);
        // A leap of a whole phrase (two notes up there) is the melody: kept.
        let mut d = doc(
            vec![
                n(0.0, 0.3, 60.0, "pitched", 0.9, Some((0, 0))),
                n(0.3, 0.6, 72.0, "pitched", 0.9, Some((0, 1))),
                n(0.6, 0.9, 72.0, "pitched", 0.9, Some((0, 2))),
                n(0.9, 1.2, 60.0, "pitched", 0.9, Some((0, 3))),
            ],
            0.0,
        );
        singable(&mut d);
        assert_eq!(d.notes.iter().map(|x| x.midi).collect::<Vec<_>>(), vec![60.0, 72.0, 72.0, 60.0]);
    }

    #[test]
    fn order_and_no_overlap_hold_on_a_dense_random_guide() {
        let mut notes = Vec::new();
        let mut t = 0.0;
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        for i in 0..400 {
            let d = 0.03 + rnd() * 0.4;
            let kind = if rnd() < 0.1 { "unpitched" } else { "pitched" };
            notes.push(n(t, t + d, 55.0 + rnd() * 14.0, kind, rnd(), Some((i / 8, (i / 2) % 4))));
            t += d + if rnd() < 0.5 { 0.0 } else { rnd() * 0.2 };
        }
        let mut d = doc(notes, 13.0);
        singable(&mut d);
        assert!(!d.notes.is_empty());
        for w in d.notes.windows(2) {
            assert!(w[0].end <= w[1].start + 1e-9, "{} > {}", w[0].end, w[1].start);
        }
        for x in &d.notes {
            let frac = x.midi - 0.13;
            assert!((frac - frac.round()).abs() < 1e-6, "{}", x.midi);
            assert!(x.end > x.start);
        }
    }
}

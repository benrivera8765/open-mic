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
//! 4a. Every note is put in the octave that fits the melody round it (`align_octaves`): the
//!    analysis sometimes measures a whole run of notes one or two octaves off (owner,
//!    2026-11-23, "Seasons": "sunlight" at F#2 under an E4 line). Pitch classes never
//!    change; sung leaps and falsetto choruses stay (see the function).
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

use jk_format::Lyrics;
use jk_format::guide::{GuideDoc, GuideNote, Span};

/// How long a bar may run on after the end of its last word (s): the voice's own release.
pub const WORD_TAIL_S: f64 = 0.15;
/// A note this close to a word (s) is still on it.
const WORD_PAD_S: f64 = 0.1;
/// A bar trimmed shorter than this is dropped.
const TRIMMED_MIN_S: f64 = 0.06;

/// Bars follow the words (owner, 2026-11-25: words "shown as being held for longer than
/// they actually are" and "a weird one off bar where there is no word said"; DECISIONS
/// D-280). The separated lead vocal keeps the singer's reverb and echo, and the analysis
/// follows its pitch for seconds after the voice has stopped ("Hearts Like Ours": "inside"
/// sung to 38.1 s, its echo 15-43 dB down until 40.5 s, drawn as bars to 40.0 s); it also
/// keeps ad-libs and backing vocals that are not in the lyrics. Measured over the owner's
/// 36 songs, 89% of the bar time past a word's end (+0.15 s) was echo more than 12 dB below
/// that word's loudest moment. So, when the song has timed lyrics: a note that touches no
/// word (within 0.1 s) is not drawn or graded, and a note runs at most `WORD_TAIL_S` past
/// the end of the last word it touches. The stored guide is unchanged (the pitch analysis
/// stays independent of the lyrics, WO2 trap 9); every reader applies this after
/// [`singable`], so songs made before it are fixed too. Without lyrics nothing changes.
///
/// Backing vocals are not the lead's words (owner, 2026-11-27: "words in parentheses show
/// as words that you're supposed to sing"; DECISIONS D-282): words in parentheses (and
/// lines marked `"kind": "backing"`, [`Lyrics::backing_words`]) do not count here, so a
/// note under backing words only is not drawn or graded either. A note that overlaps a
/// backing word must overlap one of the singer's words itself (being within 0.1 s of one is
/// not enough: "myself) I", the backing "myself)" sung up to the lead's "I"), and it starts
/// no earlier than 0.1 s before the first of the singer's words it overlaps. Notes that touch
/// no backing word are treated exactly as before.
pub fn fit_to_words(doc: &mut GuideDoc, lyrics: Option<&Lyrics>, offset_ms: i64) {
    let Some(l) = lyrics else { return };
    let off = offset_ms as f64 / 1000.0;
    let backing = l.backing_words();
    let timed = |lead: bool| -> Vec<(f64, f64)> {
        let mut v: Vec<(f64, f64)> = l
            .lines
            .iter()
            .zip(&backing)
            .flat_map(|(line, b)| line.words.iter().zip(b).filter(move |(_, bk)| **bk != lead).map(move |(w, _)| (w.start + off, w.end + off)))
            .filter(|(s, e)| s.is_finite() && e.is_finite() && e > s)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    };
    let words = timed(true);
    let back = timed(false);
    if words.is_empty() {
        return;
    }
    // The longest word bounds how far back a word that still touches a note can start.
    let longest = words.iter().map(|w| w.1 - w.0).fold(0.0, f64::max);
    doc.notes.retain_mut(|n| {
        let upto = words.partition_point(|w| w.0 < n.end + WORD_PAD_S);
        let mut last_end: Option<f64> = None;
        for w in words[..upto].iter().rev() {
            if w.0 + longest + WORD_PAD_S < n.start {
                break;
            }
            if w.1 + WORD_PAD_S > n.start {
                last_end = Some(last_end.map_or(w.1, |e: f64| e.max(w.1)));
            }
        }
        let Some(last_end) = last_end else { return false };
        if n.end > last_end + WORD_TAIL_S {
            n.end = last_end + WORD_TAIL_S;
            if n.end - n.start < TRIMMED_MIN_S {
                return false;
            }
            n.glide_in = n.glide_in.min(0.5 * dur(n)).max(0.0);
        }
        // Over a backing word: only the part on the singer's own words stays (D-282).
        let overlaps = |w: &(f64, f64)| w.0 < n.end && w.1 > n.start;
        if back.iter().any(overlaps) {
            let Some(first) = words.iter().filter(|w| overlaps(w)).map(|w| w.0).reduce(f64::min) else { return false };
            if n.start < first - WORD_PAD_S {
                n.start = first - WORD_PAD_S;
                if n.end - n.start < TRIMMED_MIN_S {
                    return false;
                }
                n.glide_in = n.glide_in.min(0.5 * dur(n)).max(0.0);
            }
        }
        true
    });
}

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

/// Octaves the alignment may move a note by (semitones).
const OCTAVE_SHIFTS: [f64; 5] = [-24.0, -12.0, 0.0, 12.0, 24.0];
/// Leaps up to this many semitones (a fifth) cost only a little in the alignment
/// (LEAP_SLOPE per semitone, so a tie goes to the octave nearest its neighbours); wider
/// ones cost the square of the excess on top.
const FREE_LEAP_ST: f64 = 7.0;
const LEAP_SLOPE: f64 = 0.6;
/// Notes up to this far (semitones) from the local centre of the melody cost nothing.
const FREE_RANGE_ST: f64 = 13.0;
/// The local centre is the weighted median of the notes within this many seconds.
const RANGE_WINDOW_S: f64 = 10.0;
/// Cost of moving a note at all, per unit of weight (keeps sung leaps sung). Two octaves
/// cost only a little more than one: the analysis slips by two as readily as by one, and
/// a dearer double move would leave a run one octave short.
const SHIFT_COST: f64 = 40.0;
const SHIFT_TWO_OCTAVES: f64 = 1.15;
/// Cost per squared semitone beyond FREE_RANGE_ST.
const RANGE_COST: f64 = 2.0;
/// Consecutive notes closer than this are fully linked; the link fades out by LINK_FADE_S.
const LINK_GAP_S: f64 = 0.6;
const LINK_FADE_S: f64 = 2.5;
/// Nothing is ever moved outside a human voice (C2 .. C7).
const VOICE_LOW: f64 = 36.0;
const VOICE_HIGH: f64 = 96.0;

/// How much a note counts in the alignment: longer and surer notes count more.
fn align_weight(n: &GuideNote) -> f64 {
    (dur(n) / 0.25).clamp(0.25, 2.0) * (0.4 + 0.6 * n.confidence.clamp(0.0, 1.0))
}

/// Weighted median of `(value, weight)` pairs (sorted in place).
fn weighted_median(v: &mut [(f64, f64)]) -> f64 {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for &(x, w) in v.iter() {
        acc += w;
        if acc >= total / 2.0 {
            return x;
        }
    }
    v.last().map(|x| x.0).unwrap_or(60.0)
}

/// The song's main register: the 15-semitone band holding the most singing (duration x
/// confidence), then the weighted median of the notes in it. Runs measured an octave or
/// two off cannot move it unless they are most of the song.
fn main_register(notes: &[(f64, f64)]) -> Option<f64> {
    let total: f64 = notes.iter().map(|x| x.1).sum();
    if notes.is_empty() || total <= 0.0 {
        return None;
    }
    let mut best = (f64::NEG_INFINITY, 0.0);
    for lo in 24..100 {
        let lo = lo as f64;
        let s: f64 = notes.iter().filter(|x| x.0 >= lo - 0.5 && x.0 < lo + 14.5).map(|x| x.1).sum();
        if s > best.0 {
            best = (s, lo);
        }
    }
    let mut v: Vec<(f64, f64)> = notes.iter().copied().filter(|x| x.0 >= best.1 - 0.5 && x.0 < best.1 + 14.5).collect();
    Some(weighted_median(&mut v))
}

/// Notes further than this (semitones) from the main register do not count towards a
/// local centre (a run two octaves off must not drag it), and a window with less than
/// LOCAL_MIN_WEIGHT of singing left uses the main register.
const REGISTER_SPAN_ST: f64 = 15.0;
const LOCAL_MIN_WEIGHT: f64 = 1.0;

/// Step 4a (owner, 2026-11-23, "Seasons": "sunlight" drawn at F#2, two octaves under the
/// line, and a slide from Bb2 up to E4): puts every note in the octave that fits the
/// melody round it. The analysis sometimes locks onto a multiple of the voice's period
/// for a whole run of notes (in "Seasons" 80 notes, 10 % of the singing, sit exactly
/// 12 or 24 semitones under where they were sung, many with high confidence), which the
/// single-note spike rule cannot see.
///
/// A Viterbi pass over the notes chooses, for each, a shift of 0, +-12 or +-24 semitones
/// minimising: a cost for moving a note at all (more for long, sure notes,
/// less for notes the builder's octave check already doubted), a cost for leaps wider
/// than a fifth between linked notes (fading out across rests), and a cost for sitting
/// more than 13 semitones from the local centre of the melody (the weighted median of
/// the notes within 10 s that lie within 15 semitones of the song's main register). A run displaced by two octaves pays hundreds for its leaps and
/// its range and a few dozen to move, so it moves; a sung octave leap of a phrase, or a
/// falsetto chorus (its own notes set the local centre), pays less to stay. Pitch classes
/// never change. Returns how many notes moved.
pub(crate) fn align_octaves(notes: &mut [GuideNote]) -> usize {
    let n = notes.len();
    if n < 3 {
        return 0;
    }
    let w: Vec<f64> = notes.iter().map(align_weight).collect();
    let mid: Vec<f64> = notes.iter().map(|x| 0.5 * (x.start + x.end)).collect();
    let pairs: Vec<(f64, f64)> = notes.iter().zip(&w).map(|(x, &wt)| (x.midi, wt)).collect();
    let register = main_register(&pairs).unwrap_or(60.0);
    // Local centres (notes are in time order: a sliding window), from the notes near the
    // main register only.
    let mut centre = vec![register; n];
    let mut lo = 0;
    let mut hi = 0;
    let mut buf: Vec<(f64, f64)> = Vec::new();
    for i in 0..n {
        while lo < n && mid[lo] < mid[i] - RANGE_WINDOW_S {
            lo += 1;
        }
        while hi < n && mid[hi] <= mid[i] + RANGE_WINDOW_S {
            hi += 1;
        }
        buf.clear();
        buf.extend((lo..hi).filter(|&j| (notes[j].midi - register).abs() <= REGISTER_SPAN_ST).map(|j| (notes[j].midi, w[j])));
        if buf.iter().map(|x| x.1).sum::<f64>() >= LOCAL_MIN_WEIGHT {
            centre[i] = weighted_median(&mut buf);
        }
    }
    let k_n = OCTAVE_SHIFTS.len();
    let mut cost = vec![[f64::INFINITY; 5]; n];
    let mut back = vec![[2usize; 5]; n];
    for i in 0..n {
        let doubted = notes[i].octave.starts_with("maybe");
        for k in 0..k_n {
            let shift = OCTAVE_SHIFTS[k];
            let p = notes[i].midi + shift;
            if shift != 0.0 && !(VOICE_LOW..=VOICE_HIGH).contains(&p) {
                continue;
            }
            // A doubted note costs less to move the way its flag points.
            let toward_flag = (notes[i].octave == "maybe_high" && shift < 0.0) || (notes[i].octave == "maybe_low" && shift > 0.0);
            let octaves = if shift == 0.0 { 0.0 } else if shift.abs() > 12.0 { SHIFT_TWO_OCTAVES } else { 1.0 };
            let move_cost = SHIFT_COST * octaves * w[i] * if doubted && toward_flag { 0.35 } else { 1.0 };
            let far = ((p - centre[i]).abs() - FREE_RANGE_ST).max(0.0);
            let emit = move_cost + RANGE_COST * far * far * w[i];
            if i == 0 {
                cost[0][k] = emit;
                continue;
            }
            let gap = notes[i].start - notes[i - 1].end;
            let link = if gap <= LINK_GAP_S {
                1.0
            } else if gap < LINK_FADE_S {
                (LINK_FADE_S - gap) / (LINK_FADE_S - LINK_GAP_S)
            } else {
                0.0
            };
            let lw = link * w[i].min(w[i - 1]);
            let mut best = f64::INFINITY;
            let mut arg = 2;
            for j in 0..k_n {
                if !cost[i - 1][j].is_finite() {
                    continue;
                }
                let d = (p - (notes[i - 1].midi + OCTAVE_SHIFTS[j])).abs();
                let over = (d - FREE_LEAP_ST).max(0.0);
                let c = cost[i - 1][j] + lw * (over * over + LEAP_SLOPE * d);
                // Ties keep the unmoved state.
                if c < best - 1e-9 || (c <= best + 1e-9 && j == 2) {
                    best = c;
                    arg = j;
                }
            }
            cost[i][k] = emit + best;
            back[i][k] = arg;
        }
    }
    let mut k = (0..k_n).min_by(|&a, &b| cost[n - 1][a].total_cmp(&cost[n - 1][b])).unwrap_or(2);
    let mut moved = 0;
    for i in (0..n).rev() {
        let shift = OCTAVE_SHIFTS[k];
        if shift != 0.0 {
            let x = &mut notes[i];
            let resolved = (x.octave == "maybe_high" && shift < 0.0) || (x.octave == "maybe_low" && shift > 0.0);
            x.midi += shift;
            if resolved {
                x.octave = "ok".into();
            }
            moved += 1;
        }
        k = back[i][k];
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
    // Step 4a first, so slivers and scoops merge into notes in the right octave.
    align_octaves(&mut notes);

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
    // Step 4a again on what is left (the dropped junk no longer pulls), then step 4 on
    // single spikes (dropped notes no longer count as neighbours); a moved note may now
    // sit on its neighbour's key, and moving one can reveal the next.
    if align_octaves(&mut notes) > 0 {
        let mut guard = notes.len() * 4 + 16;
        while guard > 0 && tidy_once(&mut notes) {
            guard -= 1;
        }
    }
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

    /// Lyrics with one line of words at these times (no characters needed here).
    fn lyrics(words: &[(f64, f64)]) -> Lyrics {
        let ws: Vec<serde_json::Value> = words.iter().map(|&(s, e)| serde_json::json!({"start": s, "end": e, "chars": []})).collect();
        let (s, e) = (words.first().map_or(0.0, |w| w.0), words.last().map_or(0.0, |w| w.1));
        serde_json::from_value(serde_json::json!({"lines": [{"start": s, "end": e, "words": ws}]})).expect("lyrics")
    }

    #[test]
    fn an_echo_after_the_last_word_is_not_drawn_as_singing() {
        // "Hearts Like Ours": "inside" 36.98-38.14 s; the analysis held it to 38.925 and then
        // found 38.925-39.995 in the echo, under no word.
        let mut d = doc(
            vec![
                n(37.255, 38.925, 69.9, "pitched", 0.82, Some((0, 0))),
                n(38.925, 39.995, 69.9, "pitched", 0.49, None),
                n(40.615, 40.955, 68.1, "pitched", 0.66, Some((0, 1))),
            ],
            0.0,
        );
        fit_to_words(&mut d, Some(&lyrics(&[(36.98, 38.14), (40.6, 41.0)])), 0);
        assert_eq!(d.notes.len(), 2, "{:?}", d.notes);
        assert!((d.notes[0].end - (38.14 + WORD_TAIL_S)).abs() < 1e-9, "{}", d.notes[0].end);
        assert!((d.notes[1].start - 40.615).abs() < 1e-9 && (d.notes[1].end - 40.955).abs() < 1e-9);
    }

    #[test]
    fn backing_vocals_in_parentheses_get_no_bars() {
        // Owner, 2026-11-27: "(Can't help myself)" sung by the backing singers was drawn and
        // graded as the lead's; "I get" after it is the lead's.
        let ws = [("(Can't", 10.0, 10.4), ("help", 10.5, 10.9), ("myself)", 11.0, 11.6), ("I", 12.0, 12.2), ("get", 12.3, 12.7)];
        let words: Vec<serde_json::Value> =
            ws.iter().map(|&(t, s, e)| serde_json::json!({"start": s, "end": e, "chars": [[t, s, e]]})).collect();
        let l: Lyrics = serde_json::from_value(serde_json::json!({"lines": [{"start": 10.0, "end": 12.7, "words": words}]})).expect("lyrics");
        let mut d = doc(
            vec![
                n(10.05, 10.4, 62.0, "pitched", 0.8, Some((0, 0))),
                n(10.5, 10.9, 64.0, "pitched", 0.8, Some((0, 1))),
                n(11.0, 11.6, 62.0, "pitched", 0.8, Some((0, 2))),
                n(12.0, 12.2, 67.0, "pitched", 0.8, Some((0, 3))),
                n(12.3, 12.7, 65.0, "pitched", 0.8, Some((0, 4))),
            ],
            0.0,
        );
        fit_to_words(&mut d, Some(&l), 0);
        let got: Vec<f64> = d.notes.iter().map(|x| x.start).collect();
        assert_eq!(got, vec![12.0, 12.3], "{:?}", d.notes);

        // Sung right up to the singer's word (within the 0.1 s pad): still no bar for
        // "myself)"; a note running from "myself)" into "I" keeps only its part on "I".
        let ws = [("(Can't", 10.0, 10.4), ("help", 10.5, 10.9), ("myself)", 11.0, 11.6), ("I", 11.65, 11.85), ("get", 11.9, 12.3)];
        let words: Vec<serde_json::Value> =
            ws.iter().map(|&(t, s, e)| serde_json::json!({"start": s, "end": e, "chars": [[t, s, e]]})).collect();
        let l: Lyrics = serde_json::from_value(serde_json::json!({"lines": [{"start": 10.0, "end": 12.3, "words": words}]})).expect("lyrics");
        let mut d = doc(vec![n(11.0, 11.6, 62.0, "pitched", 0.8, Some((0, 2))), n(11.9, 12.3, 65.0, "pitched", 0.8, Some((0, 4)))], 0.0);
        fit_to_words(&mut d, Some(&l), 0);
        assert_eq!(d.notes.iter().map(|x| x.start).collect::<Vec<_>>(), vec![11.9], "{:?}", d.notes);
        let mut d = doc(vec![n(11.1, 11.85, 62.0, "pitched", 0.8, Some((0, 2)))], 0.0);
        fit_to_words(&mut d, Some(&l), 0);
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert!((d.notes[0].start - 11.55).abs() < 1e-9 && (d.notes[0].end - 11.85).abs() < 1e-9, "{:?}", d.notes);
    }

    #[test]
    fn notes_inside_their_words_and_songs_without_lyrics_are_left_alone() {
        let notes = vec![
            n(1.0, 1.4, 60.0, "pitched", 0.9, Some((0, 0))),
            n(1.4, 2.1, 62.0, "pitched", 0.9, Some((0, 1))),
            // A legato note across two words ends with the later one.
            n(2.1, 3.05, 64.0, "pitched", 0.9, Some((0, 2))),
        ];
        let words = [(0.95, 1.4), (1.4, 2.0), (2.05, 2.5), (2.5, 3.0)];
        let mut d = doc(notes.clone(), 0.0);
        fit_to_words(&mut d, Some(&lyrics(&words)), 0);
        let ends: Vec<f64> = d.notes.iter().map(|x| x.end).collect();
        assert_eq!(ends, vec![1.4, 2.1, 3.05]);
        let mut d = doc(notes, 0.0);
        fit_to_words(&mut d, None, 0);
        assert_eq!(d.notes.len(), 3);
    }

    #[test]
    fn the_lyrics_offset_moves_the_words_the_bars_follow() {
        // The song's lyrics offset (+500 ms) puts the word at 1.5-2.0 s.
        let mut d = doc(vec![n(1.5, 3.0, 60.0, "pitched", 0.9, Some((0, 0))), n(0.2, 0.6, 60.0, "pitched", 0.9, None)], 0.0);
        fit_to_words(&mut d, Some(&lyrics(&[(1.0, 1.5)])), 500);
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert!((d.notes[0].end - (2.0 + WORD_TAIL_S)).abs() < 1e-9);
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
    fn a_run_measured_two_octaves_low_is_lifted_back_into_the_line() {
        // "Seasons" at 3:09: E4/F4 line, then a run measured at F#2 / C#2 / B1 (exactly two
        // octaves under F#4 / C#4 / B3, some of it with high confidence), then F#4 again.
        let mut d = doc(
            vec![
                n(0.0, 0.8, 65.0, "pitched", 0.8, Some((0, 0))),
                n(0.9, 1.3, 63.0, "pitched", 0.8, Some((0, 1))),
                n(1.4, 1.8, 42.0, "pitched", 0.93, Some((0, 2))),
                n(1.9, 2.2, 37.0, "pitched", 0.74, Some((0, 3))),
                n(2.3, 2.6, 35.0, "pitched", 0.81, Some((0, 3))),
                n(2.7, 3.0, 37.0, "pitched", 0.69, Some((0, 4))),
                n(3.1, 4.2, 66.0, "pitched", 0.71, Some((0, 5))),
                n(4.3, 5.0, 65.0, "pitched", 0.8, Some((0, 6))),
            ],
            0.0,
        );
        singable(&mut d);
        let got: Vec<f64> = d.notes.iter().map(|x| x.midi).collect();
        assert_eq!(got, vec![65.0, 63.0, 66.0, 61.0, 59.0, 61.0, 66.0, 65.0]);
    }

    #[test]
    fn a_verse_and_a_chorus_an_octave_apart_and_a_falsetto_line_keep_their_octaves() {
        // Low verse (around C3), a rest, then a chorus around C4: two phrases, both sung.
        let mut v = Vec::new();
        for i in 0..8 {
            v.push(n(i as f64 * 0.5, i as f64 * 0.5 + 0.4, [48.0, 50.0, 52.0, 50.0][i % 4], "pitched", 0.85, Some((0, i))));
        }
        for i in 0..8 {
            let t = 6.0 + i as f64 * 0.5;
            v.push(n(t, t + 0.4, [60.0, 62.0, 64.0, 62.0][i % 4], "pitched", 0.85, Some((1, i))));
        }
        let mut d = doc(v, 0.0);
        singable(&mut d);
        let got: Vec<f64> = d.notes.iter().map(|x| x.midi).collect();
        assert_eq!(&got[..4], &[48.0, 50.0, 52.0, 50.0]);
        assert_eq!(&got[8..12], &[60.0, 62.0, 64.0, 62.0]);

        // A verse around A3 that climbs into a falsetto line around B4-D5 step by step.
        let mut v = Vec::new();
        let line = [57.0, 59.0, 60.0, 62.0, 64.0, 67.0, 69.0, 71.0, 72.0, 74.0, 72.0, 71.0, 74.0, 72.0];
        for (i, m) in line.iter().enumerate() {
            v.push(n(i as f64 * 0.45, i as f64 * 0.45 + 0.4, *m, "pitched", 0.8, Some((0, i))));
        }
        let mut d = doc(v, 0.0);
        singable(&mut d);
        assert_eq!(d.notes.iter().map(|x| x.midi).collect::<Vec<_>>(), line.to_vec());
    }

    #[test]
    fn a_doubted_note_moves_the_way_its_flag_points_and_is_then_trusted() {
        let mut v = vec![
            n(0.0, 0.5, 64.0, "pitched", 0.8, Some((0, 0))),
            n(0.6, 1.0, 76.0, "pitched", 0.35, Some((0, 1))),
            n(1.1, 1.5, 77.0, "pitched", 0.35, Some((0, 2))),
            n(1.6, 2.1, 64.0, "pitched", 0.8, Some((0, 3))),
        ];
        v[1].octave = "maybe_high".into();
        v[2].octave = "maybe_high".into();
        let mut d = doc(v, 0.0);
        singable(&mut d);
        let got: Vec<(f64, String)> = d.notes.iter().map(|x| (x.midi, x.octave.clone())).collect();
        assert_eq!(got[1], (64.0, "ok".to_string()));
        assert_eq!(got[2], (65.0, "ok".to_string()));
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

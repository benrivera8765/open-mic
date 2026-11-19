// SPDX-License-Identifier: GPL-3.0-or-later
//! Pitch curve -> notes (the half of the guide builder that every tracker shares).
//!
//! Designed after reading how UltraSinger (one note per word or syllable, the most
//! common pitch inside it), SOME (a learned note segmenter) and pYIN's note mode (an HMM
//! over semitone notes with attack/stable states) do it. UltraSinger's approach hangs
//! notes on lyric timings, which WO2 forbids (trap 9); SOME needs a model; the note HMM
//! quantises to semitones and needs the tuning first. This module keeps the pitch as
//! measured and cuts only where the audio says so:
//!
//! 1. Frames below the vocal's loud level by more than `floor_db` are not singing.
//!    Dropouts of up to 30 ms inside a steady pitch are bridged; islands under the
//!    shortest note are dropped.
//! 2. Each voiced stretch is cut at level dips of `dip_db` (a syllable boundary sung
//!    legato through a voiced consonant).
//! 3. Inside each piece: a 3-frame median, then vibrato flattening (alternating extrema
//!    3-12 Hz with a steady centre are replaced by that centre), then an optimal
//!    piecewise-constant fit (least squares plus `note_penalty` per extra note, notes at
//!    least `min_note_s` long).
//! 4. Fit segments that are short or steep are glides (scoops, slides, fall-offs): they
//!    join the note they lead into, or the note they leave at the end of a piece, so a
//!    slide stays one gesture whose shape the pitch curve shows. Neighbours less than
//!    50 cents apart are one note.
//! 5. A note's pitch is the median of its steady part (glides excluded, vibrato centred).
//! 6. Pieces whose pitch is rarely steady (speech-like contours) are unpitched; phrases
//!    that are mostly unpitched become unpitched as a whole (spoken or rapped lines).

use super::{GuideConfig, Note, NoteKind, OctaveCheck, PitchCurve};

fn percentile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i]
}

fn median(v: &[f64]) -> f64 {
    let mut w: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    percentile(&mut w, 0.5)
}

/// A note while it is being built, in frame indices of the curve.
#[derive(Clone, Debug)]
struct Draft {
    start: usize,
    core_start: usize,
    end: usize, // exclusive
    core_end: usize,
    cents: f64,
    kind: NoteKind,
    /// First frame of the piece (voiced stretch between gaps or dips) it came from.
    piece: usize,
}

pub fn notes(curve: &PitchCurve, cfg: &GuideConfig) -> Vec<Note> {
    let n = curve.len();
    if n == 0 {
        return Vec::new();
    }
    let hop = curve.hop_s;
    let min_len = ((cfg.min_note_s / hop).round() as usize).max(2);

    // 1. Cents (MIDI * 100), NaN where unvoiced; loudness gate.
    let mut cents: Vec<f64> = curve
        .f0_hz
        .iter()
        .map(|&f| {
            if f > 0.0 {
                100.0 * super::hz_to_midi(f as f64)
            } else {
                f64::NAN
            }
        })
        .collect();
    let mut voiced_levels: Vec<f64> = (0..n)
        .filter(|&k| cents[k].is_finite())
        .map(|k| curve.level_db[k] as f64)
        .collect();
    let loud = percentile(&mut voiced_levels, 0.95);
    if !loud.is_finite() {
        return Vec::new();
    }
    for k in 0..n {
        if (curve.level_db[k] as f64) < loud - cfg.floor_db {
            cents[k] = f64::NAN;
        }
    }
    bridge_gaps(&mut cents, 3, 150.0);
    drop_islands(&mut cents, min_len);

    // 2. Voiced runs, cut at level dips.
    let level = smooth_level(&curve.level_db, 5);
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < n {
        if !cents[k].is_finite() {
            k += 1;
            continue;
        }
        let a = k;
        while k < n && cents[k].is_finite() {
            k += 1;
        }
        let b = k;
        let mut s = a;
        for c in dip_cuts(&level, a, b, cfg.dip_db, min_len) {
            pieces.push((s, c));
            s = c;
        }
        pieces.push((s, b));
    }

    // 3-6. Notes per piece.
    let mut drafts: Vec<Draft> = Vec::new();
    for &(a, b) in &pieces {
        piece_notes(&cents[a..b], a, cfg, min_len, hop, &mut drafts);
    }
    spoken_phrases(&mut drafts, hop, cfg.spoken_phrase_share);

    drafts
        .into_iter()
        .map(|d| {
            let core: Vec<f64> = cents[d.core_start..d.core_end].to_vec();
            let conf = confidence(curve, &cents, &d, &core, loud, hop);
            Note {
                start_s: d.start as f64 * hop - hop / 2.0,
                end_s: d.end as f64 * hop - hop / 2.0,
                midi: d.cents / 100.0,
                confidence: conf,
                kind: d.kind,
                octave: OctaveCheck::Unchecked,
                glide_in_s: (d.core_start - d.start) as f64 * hop,
            }
        })
        .collect()
}

/// Fills unvoiced gaps of up to `max_gap` frames whose two sides are within `max_jump`
/// cents, linearly.
fn bridge_gaps(c: &mut [f64], max_gap: usize, max_jump: f64) {
    let n = c.len();
    let mut k = 1;
    while k < n {
        if c[k].is_finite() || !c[k - 1].is_finite() {
            k += 1;
            continue;
        }
        let a = k - 1;
        let mut b = k;
        while b < n && !c[b].is_finite() {
            b += 1;
        }
        if b < n && b - a - 1 <= max_gap && (c[b] - c[a]).abs() < max_jump {
            for j in a + 1..b {
                let w = (j - a) as f64 / (b - a) as f64;
                c[j] = c[a] * (1.0 - w) + c[b] * w;
            }
        }
        k = b + 1;
    }
}

fn drop_islands(c: &mut [f64], min_len: usize) {
    let n = c.len();
    let mut k = 0;
    while k < n {
        if !c[k].is_finite() {
            k += 1;
            continue;
        }
        let a = k;
        while k < n && c[k].is_finite() {
            k += 1;
        }
        if k - a < min_len {
            c[a..k].iter_mut().for_each(|v| *v = f64::NAN);
        }
    }
}

/// Level in dB smoothed by a centred moving average of power over `w` frames.
fn smooth_level(level_db: &[f32], w: usize) -> Vec<f64> {
    let p: Vec<f64> = level_db
        .iter()
        .map(|&d| 10f64.powf(d as f64 / 10.0))
        .collect();
    let n = p.len();
    let h = w / 2;
    (0..n)
        .map(|k| {
            let lo = k.saturating_sub(h);
            let hi = (k + h + 1).min(n);
            let m = p[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
            10.0 * (m + 1e-12).log10()
        })
        .collect()
}

/// Frames inside `[a, b)` where the level dips at least `dip` dB below the loudest frame
/// within 150 ms on both sides.
fn dip_cuts(level: &[f64], a: usize, b: usize, dip: f64, min_len: usize) -> Vec<usize> {
    let reach = 15;
    let mut cuts = Vec::new();
    let mut last = a;
    if b - a < 2 * min_len {
        return cuts;
    }
    for k in a + min_len..b - min_len {
        if !(level[k] <= level[k - 1] && level[k] < level[k + 1]) {
            continue;
        }
        let left = level[k.saturating_sub(reach).max(a)..k]
            .iter()
            .copied()
            .fold(f64::MIN, f64::max);
        let right = level[k + 1..(k + 1 + reach).min(b)]
            .iter()
            .copied()
            .fold(f64::MIN, f64::max);
        if level[k] <= left - dip && level[k] <= right - dip && k - last >= min_len {
            cuts.push(k);
            last = k;
        }
    }
    cuts
}

fn median3(p: &[f64]) -> Vec<f64> {
    let n = p.len();
    (0..n)
        .map(|k| {
            if k == 0 || k + 1 == n {
                p[k]
            } else {
                let (mut a, mut b, c) = (p[k - 1], p[k], p[k + 1]);
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                b.min(c).max(a)
            }
        })
        .collect()
}

/// Replaces stretches of vibrato (alternating extrema 60-120 ms apart, i.e. 4.2-8.3 Hz,
/// swings of 15-250 cents, a centre that moves less than 40 cents per half-cycle, at
/// least 3 half-cycles)
/// by their centre line. Monotone runs and slides have no alternating extrema, so they
/// are left alone.
fn flatten_vibrato(p: &[f64]) -> Vec<f64> {
    let n = p.len();
    let mut q = p.to_vec();
    if n < 16 {
        return q;
    }
    let h = 12.0;
    let mut ext: Vec<(usize, f64)> = Vec::new();
    let (mut hi, mut lo) = ((0usize, p[0]), (0usize, p[0]));
    let mut dir = 0i8;
    for (k, &v) in p.iter().enumerate().skip(1) {
        match dir {
            0 => {
                if v > hi.1 {
                    hi = (k, v);
                }
                if v < lo.1 {
                    lo = (k, v);
                }
                if v - lo.1 > h {
                    ext.push(lo);
                    dir = 1;
                    hi = (k, v);
                } else if hi.1 - v > h {
                    ext.push(hi);
                    dir = -1;
                    lo = (k, v);
                }
            }
            1 => {
                if v > hi.1 {
                    hi = (k, v);
                } else if hi.1 - v > h {
                    ext.push(hi);
                    dir = -1;
                    lo = (k, v);
                }
            }
            _ => {
                if v < lo.1 {
                    lo = (k, v);
                } else if v - lo.1 > h {
                    ext.push(lo);
                    dir = 1;
                    hi = (k, v);
                }
            }
        }
    }
    if ext.len() < 4 {
        return q;
    }
    let valid = |i: usize| -> bool {
        let dt = ext[i + 1].0 - ext[i].0;
        let sw = (ext[i + 1].1 - ext[i].1).abs();
        (6..=12).contains(&dt) && (15.0..=250.0).contains(&sw)
    };
    let centre = |i: usize| 0.5 * (ext[i].1 + ext[i + 1].1);
    let mut i = 0;
    while i + 1 < ext.len() {
        if !valid(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 2 < ext.len() && valid(j + 1) && (centre(j + 1) - centre(j)).abs() < 40.0 && {
            let r = (ext[j + 2].1 - ext[j + 1].1).abs() / (ext[j + 1].1 - ext[j].1).abs().max(1e-9);
            (0.33..=3.0).contains(&r)
        } {
            j += 1;
        }
        // Half-cycles i..=j.
        if j + 1 - i >= 3 {
            let mids: Vec<(f64, f64)> = (i..=j)
                .map(|m| (0.5 * (ext[m].0 + ext[m + 1].0) as f64, centre(m)))
                .collect();
            for (k, v) in q
                .iter_mut()
                .enumerate()
                .take(ext[j + 1].0 + 1)
                .skip(ext[i].0)
            {
                let x = k as f64;
                *v = if x <= mids[0].0 {
                    mids[0].1
                } else if x >= mids[mids.len() - 1].0 {
                    mids[mids.len() - 1].1
                } else {
                    let s = mids
                        .windows(2)
                        .find(|w| x <= w[1].0)
                        .expect("x inside mids");
                    let w = (x - s[0].0) / (s[1].0 - s[0].0);
                    s[0].1 * (1.0 - w) + s[1].1 * w
                };
            }
        }
        i = j + 1;
    }
    q
}

/// Optimal piecewise-constant fit: least squares + `penalty` per segment, segments at
/// least `min_len` long. Returns `[start, end)` ranges covering `q`.
fn dp_segments(q: &[f64], penalty: f64, min_len: usize) -> Vec<(usize, usize)> {
    let n = q.len();
    if n < 2 * min_len {
        return vec![(0, n)];
    }
    let mut s1 = vec![0.0f64; n + 1];
    let mut s2 = vec![0.0f64; n + 1];
    for k in 0..n {
        s1[k + 1] = s1[k] + q[k];
        s2[k + 1] = s2[k] + q[k] * q[k];
    }
    let cost = |i: usize, j: usize| {
        let m = (j - i) as f64;
        let a = s1[j] - s1[i];
        (s2[j] - s2[i]) - a * a / m
    };
    let mut best = vec![f64::INFINITY; n + 1];
    let mut from = vec![0usize; n + 1];
    best[0] = 0.0;
    for j in min_len..=n {
        // i = 0, or i >= min_len with j - i >= min_len.
        let mut b = cost(0, j) + penalty;
        let mut f = 0;
        if j >= 2 * min_len {
            for (i, bi) in best.iter().enumerate().take(j - min_len + 1).skip(min_len) {
                if bi.is_finite() {
                    let c = bi + cost(i, j) + penalty;
                    if c < b {
                        b = c;
                        f = i;
                    }
                }
            }
        }
        best[j] = b;
        from[j] = f;
    }
    let mut segs = Vec::new();
    let mut j = n;
    while j > 0 {
        let i = from[j];
        segs.push((i, j));
        j = i;
    }
    segs.reverse();
    segs
}

/// Theil-Sen slope (median of pairwise slopes over pairs at least a quarter of the length
/// apart, at most about 2000 pairs): robust to a transition at either end of a note.
fn robust_slope(v: &[f64]) -> f64 {
    let n = v.len();
    if n < 3 {
        return 0.0;
    }
    let gap = (n / 4).max(2);
    let pairs = (n - gap) * (n - gap + 1) / 2;
    let stride = (pairs / 2000).max(1);
    let mut s = Vec::new();
    let mut c = 0usize;
    for i in 0..n {
        for j in i + gap..n {
            if c.is_multiple_of(stride) {
                s.push((v[j] - v[i]) / (j - i) as f64);
            }
            c += 1;
        }
    }
    median(&s)
}

/// Fraction of frames within `tol` cents of `centre`.
fn concentration(v: &[f64], centre: f64, tol: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().filter(|x| (*x - centre).abs() < tol).count() as f64 / v.len() as f64
}

/// Notes of one piece (a voiced stretch between gaps or level dips). `p` are the cents
/// of the piece, which starts at curve frame `offset`.
fn piece_notes(
    p: &[f64],
    offset: usize,
    cfg: &GuideConfig,
    min_len: usize,
    hop: f64,
    out: &mut Vec<Draft>,
) {
    let n = p.len();
    if n < min_len {
        return;
    }
    let p3 = median3(p);
    let q = flatten_vibrato(&p3);
    let segs = dp_segments(&q, cfg.note_penalty, min_len);

    // Classify: glide or note.
    struct Seg {
        a: usize,
        b: usize,
        glide: bool,
    }
    let mut segs: Vec<Seg> = segs
        .into_iter()
        .map(|(a, b)| {
            // Glides: too short for a note, or moving most of the time (a scoop, a slide,
            // a fall-off): most frames away from the segment's median. A fast-run note
            // with a transition at one end still has most frames on its pitch.
            let len = b - a;
            let glide =
                len < 6 || (len <= 30 && concentration(&q[a..b], median(&q[a..b]), 30.0) < 0.5);
            Seg { a, b, glide }
        })
        .collect();
    // A scoop that starts flat (a short first note under 100 ms, sliding into a note at
    // least twice as long and within 3 semitones) and a fall-off at the end of a piece
    // are glides too, not notes of their own.
    let notes_idx: Vec<usize> = (0..segs.len()).filter(|&i| !segs[i].glide).collect();
    let short_into = |i: usize, j: usize, segs: &[Seg]| {
        let (li, lj) = (segs[i].b - segs[i].a, segs[j].b - segs[j].a);
        li < 10
            && lj >= 2 * li
            && (median(&q[segs[i].a..segs[i].b]) - median(&q[segs[j].a..segs[j].b])).abs() < 300.0
    };
    if notes_idx.len() >= 2
        && short_into(notes_idx[0], notes_idx[1], &segs)
        && notes_idx[1] == notes_idx[0] + 1
    {
        segs[notes_idx[0]].glide = true;
    }
    let k = notes_idx.len();
    if k >= 2
        && short_into(notes_idx[k - 1], notes_idx[k - 2], &segs)
        && notes_idx[k - 1] == notes_idx[k - 2] + 1
    {
        segs[notes_idx[k - 1]].glide = true;
    }

    // Notes with their glides attached: leading glides join the next note, trailing ones
    // the last note.
    let mut drafts: Vec<Draft> = Vec::new();
    let mut pending: Option<usize> = None; // start of glides waiting for a note
    for s in &segs {
        if s.glide {
            pending.get_or_insert(s.a);
            continue;
        }
        let start = pending.take().unwrap_or(s.a);
        drafts.push(Draft {
            start,
            core_start: s.a,
            end: s.b,
            core_end: s.b,
            cents: median(&q[s.a..s.b]),
            kind: NoteKind::Pitched,
            piece: offset,
        });
    }
    if let (Some(_), Some(last)) = (pending, drafts.last_mut()) {
        last.end = n;
    }
    // A slide starts where the pitch leaves the previous note (more than 15 cents away),
    // which is often before the fit put the boundary: the start of a raised-cosine slide
    // hardly moves.
    for i in 1..drafts.len() {
        if drafts[i].start == drafts[i].core_start || drafts[i - 1].end != drafts[i].start {
            continue;
        }
        let prev = drafts[i - 1].clone();
        let mut b = drafts[i].start;
        while b > prev.core_start + min_len && (q[b - 1] - prev.cents).abs() > 15.0 {
            b -= 1;
        }
        drafts[i - 1].end = b;
        drafts[i - 1].core_end = drafts[i - 1].core_end.min(b);
        drafts[i].start = b;
    }
    // Neighbours within 50 cents are one note.
    let mut merged: Vec<Draft> = Vec::new();
    for d in drafts {
        if let Some(prev) = merged.last_mut()
            && (prev.cents - d.cents).abs() < 50.0
            && d.start == prev.end
        {
            prev.end = d.end;
            prev.core_end = d.core_end;
            prev.cents = median(&q[prev.core_start..prev.core_end]);
            continue;
        }
        merged.push(d);
    }

    // Speech-like piece: little of it is in notes whose pitch holds still (a robust slope
    // ignores a note's quick transitions).
    let speech_slope = cfg.speech_cents_per_s * hop;
    let steady_frames: usize = merged
        .iter()
        .filter(|d| robust_slope(&q[d.core_start..d.core_end]).abs() < speech_slope)
        .map(|d| d.core_end - d.core_start)
        .sum();
    if merged.is_empty() || (steady_frames as f64) < cfg.speech_steady_share * n as f64 {
        out.push(Draft {
            start: offset,
            core_start: offset,
            end: offset + n,
            core_end: offset + n,
            cents: median(&q),
            kind: NoteKind::Unpitched,
            piece: offset,
        });
        return;
    }
    for mut d in merged {
        d.start += offset;
        d.core_start += offset;
        d.end += offset;
        d.core_end += offset;
        out.push(d);
    }
}

/// Phrases (notes separated by less than 0.3 s) that are mostly unpitched become
/// unpitched as a whole: a spoken or rapped line. So do phrases of at least five short
/// syllables (pieces with a median length under 0.25 s) of which a quarter is already
/// unpitched: rap is fast, and its syllables are too short to show much contour.
fn spoken_phrases(d: &mut [Draft], hop: f64, share: f64) {
    let gap = (0.3 / hop).round() as usize;
    let mut i = 0;
    while i < d.len() {
        let mut j = i + 1;
        while j < d.len() && d[j].start <= d[j - 1].end + gap {
            j += 1;
        }
        let total: usize = d[i..j].iter().map(|x| x.end - x.start).sum();
        let unp: usize = d[i..j]
            .iter()
            .filter(|x| x.kind == NoteKind::Unpitched)
            .map(|x| x.end - x.start)
            .sum();
        // Syllables: the pieces the notes came from, with their lengths.
        let mut pieces: Vec<(usize, usize, usize)> = Vec::new(); // (piece, start, end)
        for x in &d[i..j] {
            match pieces.last_mut() {
                Some(p) if p.0 == x.piece => p.2 = x.end,
                _ => pieces.push((x.piece, x.start, x.end)),
            }
        }
        let mut lens: Vec<f64> = pieces.iter().map(|p| (p.2 - p.1) as f64 * hop).collect();
        let short = pieces.len() >= 5 && percentile(&mut lens, 0.5) < 0.25;
        let unp_share = unp as f64 / total.max(1) as f64;
        if (j - i >= 3 && unp_share >= share) || (short && unp_share >= 0.25) {
            d[i..j]
                .iter_mut()
                .for_each(|x| x.kind = NoteKind::Unpitched);
        }
        i = j;
    }
}

fn confidence(
    curve: &PitchCurve,
    cents: &[f64],
    d: &Draft,
    core: &[f64],
    loud: f64,
    hop: f64,
) -> f32 {
    let frames = d.end - d.start;
    let vp = curve.conf[d.start..d.end]
        .iter()
        .map(|&c| c as f64)
        .sum::<f64>()
        / frames.max(1) as f64;
    if d.kind == NoteKind::Unpitched {
        return vp.clamp(0.0, 1.0) as f32;
    }
    let q = flatten_vibrato(&median3(&cents[d.core_start..d.core_end]));
    let stab = concentration(&q, d.cents, 50.0);
    let dur = ((core.len() as f64 * hop) / 0.1).min(1.0);
    let lvl = curve.level_db[d.core_start..d.core_end]
        .iter()
        .map(|&l| l as f64)
        .sum::<f64>()
        / core.len().max(1) as f64;
    let level = if lvl < loud - 25.0 { 0.6 } else { 1.0 };
    (vp.clamp(0.0, 1.0).sqrt() * (0.4 + 0.6 * stab) * dur * level) as f32
}

/// The recording's tuning against A=440: the circular mean of the confident notes'
/// offsets from the nearest semitone, weighted by duration. Returns (cents, strength).
pub fn tuning(notes: &[Note], cfg: &GuideConfig) -> (f64, f64) {
    let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
    for n in notes {
        let dur = n.end_s - n.start_s - n.glide_in_s;
        if n.kind != NoteKind::Pitched || n.confidence < cfg.low_confidence || dur < 0.15 {
            continue;
        }
        let ang = 2.0 * std::f64::consts::PI * (n.midi * 100.0).rem_euclid(100.0) / 100.0;
        sx += dur * ang.cos();
        sy += dur * ang.sin();
        sw += dur;
    }
    if sw == 0.0 {
        return (0.0, 0.0);
    }
    let cents = sy.atan2(sx) * 100.0 / (2.0 * std::f64::consts::PI);
    (
        (cents * 100.0).round() / 100.0,
        ((sx * sx + sy * sy).sqrt() / sw * 1000.0).round() / 1000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dp_finds_steps_and_ignores_noise() {
        let mut q = vec![6000.0; 20];
        q.extend(vec![6200.0; 20]);
        q.extend(vec![6100.0; 10]);
        let segs = dp_segments(&q, 12_000.0, 4);
        assert_eq!(segs, vec![(0, 20), (20, 40), (40, 50)]);
    }

    #[test]
    fn vibrato_is_flattened_but_a_run_is_not() {
        let vib: Vec<f64> = (0..120)
            .map(|k| 6400.0 + 50.0 * (2.0 * std::f64::consts::PI * 5.5 * k as f64 * 0.01).sin())
            .collect();
        let q = flatten_vibrato(&vib);
        let dev = q[20..100]
            .iter()
            .map(|v| (v - 6400.0).abs())
            .fold(0.0, f64::max);
        assert!(dev < 15.0, "{dev}");
        let run: Vec<f64> = (0..80).map(|k| 6000.0 + 200.0 * (k / 10) as f64).collect();
        assert_eq!(flatten_vibrato(&run), run);
    }
}

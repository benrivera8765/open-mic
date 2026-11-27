// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody guide as a practice session uses it: one object read once per session,
//! from which both the lane (`practice_guide`, docs/APP_API.md 12.5 `PracticeGuide`) and
//! the scorer (`jk_pitch::score::ScoreGuide`) are made, so drawing and grading cannot
//! disagree (ARCHITECTURE 12.2). Note indices are the guide's own, in both.
//!
//! Word references are checked against the effective lyrics and recomputed when they do
//! not match (SONG_FORMAT 10.4: they are a cache). Never prints or logs lyrics.

use jk_format::guide::{Curve, GuideDoc};
use jk_pitch::score::{LineBasis, LineSpan, ScoreGuide, ScoreNote};
use serde::Serialize;

use crate::guide::GuideStatus;
use crate::keys::Key;

/// The guide of the song being practised.
#[derive(Clone, Debug)]
pub struct SessionGuide {
    pub song_id: String,
    pub doc: GuideDoc,
    pub curve: Option<Curve>,
    pub status: GuideStatus,
    /// The scorer's view (no exclusions: those are passed separately).
    pub score: ScoreGuide,
    pub song_key: Option<Key>,
    pub duration_s: f64,
}

/// `PracticeGuide.range`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RangeView {
    pub low: f64,
    pub high: f64,
}

/// One note of `PracticeGuide.notes`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoteView {
    pub start: f64,
    pub end: f64,
    pub midi: f64,
    pub kind: &'static str,
    pub confidence: f64,
    pub dim: bool,
    pub graded: bool,
    pub glide_in: f64,
    pub word: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpanView {
    pub start: f64,
    pub end: f64,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CurveView {
    pub hop_s: f64,
    pub t0_s: f64,
    /// MIDI with two decimals; 0 = unvoiced.
    pub midi: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LineView {
    pub line: i64,
    pub start: f64,
    pub end: f64,
}

/// docs/APP_API.md 12.5 `PracticeGuide`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PracticeGuideView {
    pub song_id: String,
    pub status: GuideStatus,
    pub tuning_cents: f64,
    pub range: Option<RangeView>,
    pub notes: Vec<NoteView>,
    pub spans: Vec<SpanView>,
    pub curve: Option<CurveView>,
    pub lines: Vec<LineView>,
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// True when `t` lies in one of the spans.
pub fn in_spans(spans: &[(f64, f64)], t: f64) -> bool {
    spans.iter().any(|&(a, b)| t >= a && t <= b)
}

impl SessionGuide {
    /// Builds the session's guide from a song file (for its lyrics, offset and key) and a
    /// guide document and curve (from the file, or built in memory).
    pub fn new(song_id: &str, song: &jk_format::SongFile, mut doc: GuideDoc, curve: Option<Curve>, status: GuideStatus) -> SessionGuide {
        let lyrics = song.effective_lyrics().ok().flatten();
        let offset_ms = song.effective_settings().lyrics_offset_ms;
        let is_override = song.user_state().lyrics_override.is_some();
        // SONG_FORMAT 10.4: references that do not match the effective lyrics are recomputed.
        if !doc.word_refs_valid(lyrics.as_ref(), offset_ms) {
            jk_guide::set_word_refs(&mut doc, lyrics.as_ref(), is_override, offset_ms);
        }
        // The melody as the singer reads it: notes on piano keys, slivers folded in (D-251).
        // Lane and grade both read this, so what is drawn is what is graded.
        jk_guide::singable::singable(&mut doc);
        // Bars follow the words: none where nothing is sung, none held into the echo (D-280),
        // none under backing vocals (words in parentheses, D-282).
        jk_guide::singable::fit_to_words(&mut doc, lyrics.as_ref(), offset_ms);
        let offset_s = offset_ms as f64 / 1000.0;
        let notes: Vec<ScoreNote> = doc
            .notes
            .iter()
            .map(|n| ScoreNote {
                start: n.start,
                end: n.end,
                midi: n.midi,
                pitched: n.is_pitched(),
                confidence: n.confidence,
                octave_flagged: n.octave_flagged(),
                glide_in: n.glide_in,
                line: n.word.as_ref().map(|w| w.0 as i64),
            })
            .collect();
        let (lines, basis) = match &lyrics {
            // A line of backing vocals only is not the singer's line: no result row (D-282).
            Some(l) if !l.lines.is_empty() => (
                l.lines
                    .iter()
                    .enumerate()
                    .zip(l.backing_words())
                    .filter(|(_, b)| !(!b.is_empty() && b.iter().all(|&x| x)))
                    .map(|((i, line), _)| LineSpan { line: i as i64, start: line.start + offset_s, end: line.end + offset_s })
                    .collect(),
                LineBasis::Lyrics,
            ),
            _ => (ScoreGuide::phrases(&notes), LineBasis::Phrases),
        };
        let duration_s = song.manifest().duration();
        let score = ScoreGuide {
            notes,
            low_confidence: doc.low_confidence,
            lines,
            basis,
            outdated: status.state == "outdated",
            duration_s,
            label: format!("{} {}", doc.provenance.method, doc.provenance.version),
        };
        SessionGuide {
            song_id: song_id.to_string(),
            song_key: crate::library::song_key(song),
            doc,
            curve,
            status,
            score,
            duration_s,
        }
    }

    /// The guide's tuning offset readers use (cents).
    pub fn tuning_cents(&self) -> f64 {
        self.doc.tuning.use_cents
    }

    /// SONG_FORMAT 10.1's "graded" rule with the singer's exclusions.
    pub fn graded(&self, i: usize, excluded: &[(f64, f64)]) -> bool {
        self.doc.notes.get(i).is_some_and(|n| n.graded(self.doc.low_confidence) && !in_spans(excluded, n.midpoint()))
    }

    /// The note sounding at song time `t`, if any.
    pub fn note_at(&self, t: f64) -> Option<usize> {
        let notes = &self.doc.notes;
        // Notes are in time order and do not overlap (SONG_FORMAT 10.1).
        let i = notes.partition_point(|n| n.end <= t);
        notes.get(i).filter(|n| t >= n.start).map(|_| i)
    }

    /// `practice_guide` (APP_API 12.5), with the exclusions applied.
    pub fn view(&self, excluded: &[(f64, f64)]) -> PracticeGuideView {
        let low = self.doc.low_confidence;
        let notes = self
            .doc
            .notes
            .iter()
            .enumerate()
            .map(|(i, n)| NoteView {
                start: r3(n.start),
                end: r3(n.end),
                midi: (n.midi * 1000.0).round() / 1000.0,
                kind: if n.is_pitched() { "pitched" } else { "unpitched" },
                confidence: (n.confidence * 100.0).round() / 100.0,
                dim: n.confidence < low || n.octave_flagged(),
                graded: self.graded(i, excluded),
                glide_in: r3(n.glide_in),
                word: n.word.as_ref().map(|w| (w.0, w.1)),
            })
            .collect();
        let curve = self.curve.as_ref().and_then(|c| {
            let midi = c.midi()?;
            Some(CurveView {
                hop_s: c.hop_s(),
                t0_s: f64::from(c.t0_us) * 1e-6,
                midi: midi
                    .iter()
                    .map(|&m| if m.is_finite() && m > 0.0 { (f64::from(m) * 100.0).round() / 100.0 } else { 0.0 })
                    .collect(),
            })
        });
        PracticeGuideView {
            song_id: self.song_id.clone(),
            status: self.status.clone(),
            tuning_cents: self.doc.tuning.use_cents,
            range: self.doc.compute_range().map(|r| RangeView { low: r.low, high: r.high }),
            notes,
            spans: self.doc.spans.iter().map(|s| SpanView { start: r3(s.start), end: r3(s.end), kind: s.kind.clone() }).collect(),
            curve,
            lines: self.score.lines.iter().map(|l| LineView { line: l.line, start: r3(l.start), end: r3(l.end) }).collect(),
        }
    }
}

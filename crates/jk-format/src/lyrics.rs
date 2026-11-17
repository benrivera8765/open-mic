//! Lyrics with line, word and letter timings (SONG_FORMAT.md section 5).
//!
//! A "letter" is a Unicode extended grapheme cluster, stored explicitly as a string so
//! implementations never have to agree on segmentation.

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

/// One timed grapheme cluster: JSON `[grapheme, start, end]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Grapheme {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// Array elements after the third, from a newer writer; kept for rewrites.
    pub extra: Vec<Value>,
}

impl Grapheme {
    pub fn new(text: impl Into<String>, start: f64, end: f64) -> Self {
        Grapheme { text: text.into(), start, end, extra: Vec::new() }
    }
}

impl Serialize for Grapheme {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(3 + self.extra.len()))?;
        seq.serialize_element(&self.text)?;
        seq.serialize_element(&self.start)?;
        seq.serialize_element(&self.end)?;
        for v in &self.extra {
            seq.serialize_element(v)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Grapheme {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Grapheme;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an array [grapheme, start, end]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Grapheme, A::Error> {
                let text: String = a.next_element()?.ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let start: f64 = a.next_element()?.ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let end: f64 = a.next_element()?.ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let mut extra = Vec::new();
                while let Some(v) = a.next_element::<Value>()? {
                    extra.push(v);
                }
                Ok(Grapheme { text, start, end, extra })
            }
        }
        d.deserialize_seq(V)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub chars: Vec<Grapheme>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Word {
    /// The word's text: its graphemes concatenated.
    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
    /// Unknown keys (for example a later `"kind": "backing"`) are kept here.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Line {
    /// The line's text: its words joined with single spaces.
    pub fn text(&self) -> String {
        self.words.iter().map(Word::text).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    #[serde(default = "one_u32")]
    pub lyrics_version: u32,
    #[serde(default)]
    pub language: Option<String>,
    /// `user_text`, `lrc`, `embedded`, `online:<service>` or `transcribed`.
    #[serde(default)]
    pub source: Option<String>,
    /// False until a person has reviewed the text.
    #[serde(default)]
    pub checked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Value>,
    pub lines: Vec<Line>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one_u32() -> u32 {
    1
}

/// Tolerance for time comparisons, in seconds. JSON numbers written by different
/// languages round-trip exactly, so this only absorbs aligner arithmetic noise.
const EPS: f64 = 1e-9;

/// `start <= end` within tolerance; false when either is NaN (so NaN is reported).
fn ordered(start: f64, end: f64) -> bool {
    start <= end + EPS
}

impl Lyrics {
    /// Checks the timing rules of section 5 and the validation checklist (section 9):
    /// `start <= end` everywhere; chars within a word, words within a line, and lines in
    /// order and not overlapping; every time within `0..=duration` (plus 1 ms for
    /// rounding); every word has at least one char and no char is empty.
    ///
    /// Returns a list of problems (empty when valid). `where_` prefixes each message.
    pub fn validate(&self, duration: f64, where_: &str) -> Vec<String> {
        let mut errs = Vec::new();
        let max_t = duration + 0.001;
        let mut push = |m: String| {
            if errs.len() < 50 {
                errs.push(format!("{where_}: {m}"));
            }
        };
        let in_song = |t: f64| t.is_finite() && t >= -EPS && t <= max_t;
        let mut prev_line_end = f64::NEG_INFINITY;
        for (li, line) in self.lines.iter().enumerate() {
            if !ordered(line.start, line.end) {
                push(format!("line {li} ends before it starts"));
            }
            if !in_song(line.start) || !in_song(line.end) {
                push(format!("line {li} lies outside the song (0..{duration:.3} s)"));
            }
            if line.start + EPS < prev_line_end {
                push(format!("line {li} overlaps the previous line"));
            }
            prev_line_end = line.end;
            let mut prev_word_end = f64::NEG_INFINITY;
            for (wi, w) in line.words.iter().enumerate() {
                if w.chars.is_empty() {
                    push(format!("line {li} word {wi} has no chars"));
                }
                if !ordered(w.start, w.end) {
                    push(format!("line {li} word {wi} ends before it starts"));
                }
                if !in_song(w.start) || !in_song(w.end) {
                    push(format!("line {li} word {wi} lies outside the song"));
                }
                if w.start + EPS < prev_word_end {
                    push(format!("line {li} word {wi} overlaps the previous word"));
                }
                prev_word_end = w.end;
                if let Some(c) = w.confidence {
                    if !(0.0..=1.0).contains(&c) {
                        push(format!("line {li} word {wi} confidence {c} is outside 0..1"));
                    }
                }
                let mut prev_char_end = f64::NEG_INFINITY;
                for (ci, c) in w.chars.iter().enumerate() {
                    if c.text.is_empty() {
                        push(format!("line {li} word {wi} char {ci} is an empty string"));
                    }
                    if !ordered(c.start, c.end) {
                        push(format!("line {li} word {wi} char {ci} ends before it starts"));
                    }
                    if !in_song(c.start) || !in_song(c.end) {
                        push(format!("line {li} word {wi} char {ci} lies outside the song"));
                    }
                    if c.start + EPS < prev_char_end {
                        push(format!("line {li} word {wi} char {ci} overlaps the previous char"));
                    }
                    prev_char_end = c.end;
                }
            }
        }
        errs
    }

    /// Soft checks that are not errors in the spec: chars that are more (or less) than
    /// one grapheme cluster by UAX #29 as implemented by `unicode-segmentation`, and
    /// words or chars that stick out of their line or word.
    pub fn lint(&self, where_: &str) -> Vec<String> {
        use unicode_segmentation::UnicodeSegmentation;
        let mut out = Vec::new();
        for (li, line) in self.lines.iter().enumerate() {
            for (wi, w) in line.words.iter().enumerate() {
                if w.start + EPS < line.start || w.end > line.end + EPS {
                    out.push(format!("{where_}: line {li} word {wi} extends outside its line"));
                }
                for (ci, c) in w.chars.iter().enumerate() {
                    let n = c.text.graphemes(true).count();
                    if n != 1 && !c.text.is_empty() {
                        out.push(format!(
                            "{where_}: line {li} word {wi} char {ci} {:?} is {n} grapheme clusters, expected 1",
                            c.text
                        ));
                    }
                    if c.start + EPS < w.start || c.end > w.end + EPS {
                        out.push(format!("{where_}: line {li} word {wi} char {ci} extends outside its word"));
                    }
                }
            }
        }
        out.truncate(50);
        out
    }

    /// Total number of timed graphemes.
    pub fn grapheme_count(&self) -> usize {
        self.lines.iter().flat_map(|l| &l.words).map(|w| w.chars.len()).sum()
    }
}

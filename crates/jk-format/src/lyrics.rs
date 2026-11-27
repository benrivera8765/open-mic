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

// ---------------------------------------------------------------------------
// Backing vocals (owner, 2026-11-27: "words in parentheses show as words that you're
// supposed to sing"; DECISIONS D-282)
// ---------------------------------------------------------------------------

fn opens(c: char) -> bool {
    c == '(' || c == '\u{FF08}'
}

fn closes(c: char) -> bool {
    c == ')' || c == '\u{FF09}'
}

/// A letter or digit (`char::is_alphanumeric`; the app's UI uses the same test).
fn counts(c: char) -> bool {
    c.is_alphanumeric()
}

/// True when the line has a `)` with no `(` before it on the line.
fn closes_unopened(line: &Line) -> bool {
    let mut depth = 0u32;
    for c in line.words.iter().flat_map(|w| w.chars.iter()).flat_map(|g| g.text.chars()) {
        if opens(c) {
            depth += 1;
        } else if closes(c) {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
    }
    false
}

impl Lyrics {
    /// Which words are backing vocals, not the lead's to sing: one flag per word, per line.
    ///
    /// Lyrics put what the backing singers sing in parentheses: "(Can't help myself) I get
    /// so lonely". A word is backing when every letter or digit of it lies inside
    /// parentheses (`(` `)` or the full-width `（` `）`), or when its line carries
    /// `"kind": "backing"` (section 5). A word with no letter or digit is backing when it
    /// starts inside parentheses or holds one. Parentheses opened on one line and closed
    /// on the next carry over to it (a line with no text in between is passed over, so
    /// readers that drop such lines get the same flags, whatever kind they carry); one never
    /// closed ends with its line. When every word with a letter or digit would be backing,
    /// none is (a lyrics source that brackets everything says nothing).
    ///
    /// Backing words are shown dimmed and are not the lead's: the practice lane draws and
    /// grades no bar for them. The same rule is written in the app's UI
    /// (`app/ui/src/lib/lyrics.ts` `backingWords`); change both together.
    pub fn backing_words(&self) -> Vec<Vec<bool>> {
        let mut out: Vec<Vec<bool>> = Vec::with_capacity(self.lines.len());
        let mut carry = false;
        // Some word with a letter or digit, and some such word that is the singer's.
        let (mut any_letters, mut lead_letters) = (false, false);
        let has_letters = |w: &Word| w.chars.iter().flat_map(|g| g.text.chars()).any(counts);
        for line in &self.lines {
            if line.words.iter().all(|w| w.chars.is_empty()) {
                out.push(vec![false; line.words.len()]);
                continue;
            }
            if line.extra.get("kind").and_then(Value::as_str) == Some("backing") {
                out.push(vec![true; line.words.len()]);
                any_letters |= line.words.iter().any(has_letters);
                carry = false;
                continue;
            }
            let mut depth: u32 = u32::from(carry && closes_unopened(line));
            let mut flags = Vec::with_capacity(line.words.len());
            for w in &line.words {
                let start_depth = depth;
                let (mut letters, mut inside, mut paren) = (0usize, 0usize, false);
                for c in w.chars.iter().flat_map(|g| g.text.chars()) {
                    if opens(c) {
                        depth += 1;
                        paren = true;
                    } else if closes(c) {
                        depth = depth.saturating_sub(1);
                        paren = true;
                    } else if counts(c) {
                        letters += 1;
                        if depth > 0 {
                            inside += 1;
                        }
                    }
                }
                let backing = if letters > 0 { inside == letters } else { start_depth > 0 || paren };
                if letters > 0 {
                    any_letters = true;
                    lead_letters |= !backing;
                }
                flags.push(backing);
            }
            carry = depth > 0;
            out.push(flags);
        }
        if any_letters && !lead_letters {
            for f in out.iter_mut() {
                f.fill(false);
            }
        }
        out
    }
}

#[cfg(test)]
mod backing_tests {
    use super::*;

    fn word(text: &str, t: f64) -> Word {
        Word { start: t, end: t + 0.3, confidence: None, chars: vec![Grapheme::new(text, t, t + 0.3)], extra: Map::new() }
    }

    fn line(texts: &[&str], t: f64) -> Line {
        let words: Vec<Word> = texts.iter().enumerate().map(|(i, s)| word(s, t + i as f64 * 0.4)).collect();
        Line { start: t, end: t + texts.len() as f64 * 0.4, words, extra: Map::new() }
    }

    fn lyrics(lines: Vec<Line>) -> Lyrics {
        Lyrics { lyrics_version: 1, language: None, source: None, checked: false, alignment: None, lines, extra: Map::new() }
    }

    #[test]
    fn words_in_parentheses_are_backing() {
        let l = lyrics(vec![line(&["(Can't", "help", "myself)", "I", "get", "so"], 0.0), line(&["lonely", "(ooh)"], 3.0)]);
        assert_eq!(l.backing_words(), vec![vec![true, true, true, false, false, false], vec![false, true]]);
    }

    #[test]
    fn a_word_half_in_parentheses_is_sung() {
        let l = lyrics(vec![line(&["love(d)", "(oh)yeah", "-", "(", "ooh", ")", "you"], 0.0)]);
        assert_eq!(l.backing_words(), vec![vec![false, false, false, true, true, true, false]]);
    }

    #[test]
    fn parentheses_carry_to_the_line_that_closes_them() {
        let l = lyrics(vec![
            line(&["sing", "(can't", "help"], 0.0),
            line(&["myself)", "now"], 2.0),
            line(&["(never", "closed"], 4.0),
            line(&["next", "line"], 6.0),
        ]);
        assert_eq!(
            l.backing_words(),
            vec![vec![false, true, true], vec![true, false], vec![true, true], vec![false, false]]
        );
    }

    #[test]
    fn a_line_with_no_text_is_passed_over() {
        let empty = Line { start: 1.5, end: 1.5, words: vec![], extra: Map::new() };
        let mut blank = line(&["x"], 1.6);
        blank.words[0].chars.clear();
        let l = lyrics(vec![line(&["sing", "(can't"], 0.0), empty, blank, line(&["help)", "now"], 2.0)]);
        assert_eq!(l.backing_words(), vec![vec![false, true], vec![], vec![false], vec![true, false]]);
        let mut blank = line(&["x"], 1.6);
        blank.words[0].chars.clear();
        let all = lyrics(vec![line(&["(all", "of", "it)"], 0.0), blank]);
        assert_eq!(all.backing_words(), vec![vec![false, false, false], vec![false]]);
        // An empty line marked as backing is passed over too (readers drop it).
        let mut empty_backing = Line { start: 1.5, end: 1.5, words: vec![], extra: Map::new() };
        empty_backing.extra.insert("kind".into(), Value::from("backing"));
        let l = lyrics(vec![line(&["sing", "(can't"], 0.0), empty_backing, line(&["help)", "now"], 2.0)]);
        assert_eq!(l.backing_words(), vec![vec![false, true], vec![], vec![true, false]]);
        // Words without letters ("-", a music sign) do not keep the safeguard from applying.
        let l = lyrics(vec![line(&["(all", "of", "it)", "-"], 0.0), line(&["♪"], 2.0)]);
        assert_eq!(l.backing_words(), vec![vec![false, false, false, false], vec![false]]);
    }

    #[test]
    fn a_backing_line_and_full_width_parentheses() {
        let mut b = line(&["la", "la"], 0.0);
        b.extra.insert("kind".into(), Value::from("backing"));
        let l = lyrics(vec![b, line(&["\u{FF08}ah\u{FF09}", "oh"], 2.0)]);
        assert_eq!(l.backing_words(), vec![vec![true, true], vec![true, false]]);
    }

    #[test]
    fn everything_in_parentheses_means_nothing_is_backing() {
        let l = lyrics(vec![line(&["(all", "of", "it)"], 0.0)]);
        assert_eq!(l.backing_words(), vec![vec![false, false, false]]);
    }
}

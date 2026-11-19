// SPDX-License-Identifier: GPL-3.0-or-later
//! Word references of the guide's notes (SONG_FORMAT.md section 10.4).
//!
//! A display aid only: **it never moves, splits or deletes a note** (the aligner can
//! place a word seconds from where it is sung, WO2 trap 9). The rule, applied identically
//! by every implementation (the Python twin is `jksong.map_words`): for each note, the
//! word with the largest overlap between `[note.start, note.end]` and
//! `[word.start + offset, word.end + offset]` (the earlier word, in lyric order, on a
//! tie), kept only when that overlap is positive and at least 30 % of the note's length
//! or at least 50 % of the word's length; otherwise the note has no word.
//!
//! The arithmetic is spelled out (`min(ne, we) - max(ns, ws)`, `0.3 * (ne - ns)`,
//! `0.5 * (we - ws)`) so that the Rust and Python implementations, both IEEE doubles,
//! give the same answer bit for bit.

/// A lyric word's time span, already shifted by the song's lyrics offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordSpan {
    /// Index of the line in the effective lyrics.
    pub line: usize,
    /// Index of the word in its line.
    pub word: usize,
    /// `word.start + offset_ms / 1000`.
    pub start: f64,
    /// `word.end + offset_ms / 1000`.
    pub end: f64,
}

/// Share of the note's length the overlap must reach (or [`WORD_SHARE`] of the word's).
pub const NOTE_SHARE: f64 = 0.3;
/// Share of the word's length the overlap must reach (or [`NOTE_SHARE`] of the note's).
pub const WORD_SHARE: f64 = 0.5;

/// For each note `(start, end)`, the index into `words` of its word, or `None`.
/// `words` is in lyric order (lines, then words), which is time order for valid lyrics;
/// the result does not depend on it being sorted.
pub fn map_words(notes: &[(f64, f64)], words: &[WordSpan]) -> Vec<Option<usize>> {
    notes
        .iter()
        .map(|&(ns, ne)| {
            let mut best: Option<(usize, f64)> = None;
            for (i, w) in words.iter().enumerate() {
                let overlap = ne.min(w.end) - ns.max(w.start);
                if overlap <= 0.0 {
                    continue;
                }
                if best.is_none_or(|(_, b)| overlap > b) {
                    best = Some((i, overlap));
                }
            }
            let (i, overlap) = best?;
            let w = &words[i];
            (overlap >= NOTE_SHARE * (ne - ns) || overlap >= WORD_SHARE * (w.end - w.start)).then_some(i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(line: usize, word: usize, start: f64, end: f64) -> WordSpan {
        WordSpan { line, word, start, end }
    }

    #[test]
    fn largest_overlap_wins_and_the_thresholds_apply() {
        let words = [w(0, 0, 1.0, 2.0), w(0, 1, 2.0, 2.5), w(1, 0, 5.0, 6.0)];
        let notes = [
            (1.1, 1.9), // inside word 0
            (1.75, 2.25), // 0.25 s of word 0, 0.25 s of word 1 (exact): tie -> the earlier word
            (2.05, 2.15), // inside word 1
            (3.0, 4.0), // between words: none
            (4.9, 5.05), // 0.05 of a 0.15 s note on word 2: 33 % of the note -> kept
            (5.95, 7.0), // 0.05 of a 1.05 s note (5 %), 5 % of the word: dropped
        ];
        let m = map_words(&notes, &words);
        assert_eq!(m, vec![Some(0), Some(0), Some(1), None, Some(2), None]);
    }

    #[test]
    fn several_notes_share_one_word_and_a_word_may_have_none() {
        let words = [w(0, 0, 0.0, 3.0), w(0, 1, 3.0, 3.1)];
        let notes = [(0.0, 1.0), (1.0, 2.0), (2.0, 2.98)];
        assert_eq!(map_words(&notes, &words), vec![Some(0), Some(0), Some(0)]);
    }

    #[test]
    fn a_zero_length_word_never_takes_a_note() {
        let words = [w(0, 0, 1.5, 1.5)];
        assert_eq!(map_words(&[(1.0, 2.0)], &words), vec![None]);
    }

    #[test]
    fn a_short_word_inside_a_long_note_is_kept_by_the_word_share() {
        // 0.1 s word inside a 2 s note: 5 % of the note but 100 % of the word.
        let words = [w(0, 0, 1.0, 1.1)];
        assert_eq!(map_words(&[(0.5, 2.5)], &words), vec![Some(0)]);
    }
}

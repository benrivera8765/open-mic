// SPDX-License-Identifier: GPL-3.0-or-later
//! What the backing-vocal rule (D-282: words in parentheses are not the singer's) changes in
//! the melody a practice session draws and grades, per song: words in parentheses, lines of
//! backing vocals only, bars before and after, bars left under backing words only, and how
//! much of the singer's own words still has a bar. Never prints lyrics.
//!
//! cargo run -p jk-guide --release --example backing_stats -- <song.jksong>...
use jk_format::guide::GuideDoc;
use jk_format::{Lyrics, SongFile};

/// The same lyrics with every parenthesis removed: as the app read them before D-282.
fn without_parentheses(l: &Lyrics) -> Lyrics {
    let mut c = l.clone();
    for line in &mut c.lines {
        line.extra.remove("kind");
        for w in &mut line.words {
            for g in &mut w.chars {
                g.text = g.text.chars().filter(|&ch| !matches!(ch, '(' | ')' | '\u{FF08}' | '\u{FF09}')).collect();
            }
        }
    }
    c
}

fn covered(d: &GuideDoc, words: &[(f64, f64)]) -> f64 {
    let total: f64 = words.iter().map(|w| w.1 - w.0).sum();
    let mut c = 0.0;
    for w in words {
        for n in &d.notes {
            c += (n.end.min(w.1) - n.start.max(w.0)).max(0.0);
        }
    }
    100.0 * c / total.max(1e-9)
}

fn main() {
    let (mut songs, mut with, mut words_n, mut backing_n, mut lines_n, mut before_n, mut after_n, mut left_n) = (0, 0, 0, 0, 0, 0, 0, 0);
    for path in std::env::args().skip(1) {
        let Ok(song) = SongFile::open(std::path::Path::new(&path)) else { continue };
        let (_, doc) = jk_guide::file_state(&song);
        let Some(mut d) = doc else { continue };
        let Some(lyrics) = song.effective_lyrics().ok().flatten() else { continue };
        let offset_ms = song.effective_settings().lyrics_offset_ms;
        let is_override = song.user_state().lyrics_override.is_some();
        if !d.word_refs_valid(Some(&lyrics), offset_ms) {
            jk_guide::set_word_refs(&mut d, Some(&lyrics), is_override, offset_ms);
        }
        jk_guide::singable::singable(&mut d);
        let off = offset_ms as f64 / 1000.0;
        let flags = lyrics.backing_words();
        let mut lead: Vec<(f64, f64)> = Vec::new();
        let mut back: Vec<(f64, f64)> = Vec::new();
        for (line, f) in lyrics.lines.iter().zip(&flags) {
            for (w, &b) in line.words.iter().zip(f) {
                if w.end > w.start {
                    if b { back.push((w.start + off, w.end + off)) } else { lead.push((w.start + off, w.end + off)) }
                }
            }
        }
        let n_back = flags.iter().flatten().filter(|&&b| b).count();
        let n_lines = flags.iter().filter(|f| !f.is_empty() && f.iter().all(|&b| b)).count();
        let mut old = d.clone();
        jk_guide::singable::fit_to_words(&mut old, Some(&without_parentheses(&lyrics)), offset_ms);
        let mut new = d.clone();
        jk_guide::singable::fit_to_words(&mut new, Some(&lyrics), offset_ms);
        // On a backing word and on none of the singer's words (no padding).
        let overlaps = |n: &jk_format::guide::GuideNote, ws: &[(f64, f64)]| ws.iter().any(|w| w.0 < n.end && w.1 > n.start);
        let left = new.notes.iter().filter(|n| overlaps(n, &back) && !overlaps(n, &lead)).count();
        songs += 1;
        words_n += flags.iter().map(Vec::len).sum::<usize>();
        backing_n += n_back;
        lines_n += n_lines;
        before_n += old.notes.len();
        after_n += new.notes.len();
        left_n += left;
        if n_back == 0 {
            continue;
        }
        with += 1;
        let name: String = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().chars().take(40).collect()).unwrap_or_default();
        println!(
            "{name:40} backing words {n_back:3}  backing-only lines {n_lines:2}  bars {:4} -> {:4}  left under backing only {left}  backing words under a bar {:5.1}% -> {:5.1}%  singer's words {:5.1}% -> {:5.1}%",
            old.notes.len(),
            new.notes.len(),
            covered(&old, &back),
            covered(&new, &back),
            covered(&old, &lead),
            covered(&new, &lead)
        );
    }
    println!("{songs} songs with a guide and lyrics, {with} with words in parentheses: {backing_n} of {words_n} words, {lines_n} lines; bars {before_n} -> {after_n}; left under backing words only: {left_n}");
}

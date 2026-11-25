// SPDX-License-Identifier: GPL-3.0-or-later
//! What `fit_to_words` changes in the melody a practice session draws and grades (D-280),
//! for each song: bars under no word, bars running past their word's end, and how much of
//! the sung words' time still has a bar. Never prints lyrics.
//!
//! cargo run -p jk-guide --release --example word_fit_stats -- <song.jksong>...
use jk_format::SongFile;
use jk_format::guide::GuideDoc;

#[derive(Default, Clone, Copy)]
struct S {
    notes: usize,
    no_word: usize,
    past_end: usize,
    word_s: f64,
    covered_s: f64,
}

fn measure(d: &GuideDoc, words: &[(f64, f64)]) -> S {
    let mut s = S { notes: d.notes.len(), ..S::default() };
    for n in &d.notes {
        let touching: Vec<&(f64, f64)> = words.iter().filter(|w| w.0 - 0.1 < n.end && w.1 + 0.1 > n.start).collect();
        match touching.iter().map(|w| w.1).fold(None, |m: Option<f64>, e| Some(m.map_or(e, |m| m.max(e)))) {
            None => s.no_word += 1,
            Some(last) if n.end > last + 0.5 => s.past_end += 1,
            Some(_) => {}
        }
    }
    for w in words {
        s.word_s += w.1 - w.0;
        for n in &d.notes {
            s.covered_s += (n.end.min(w.1) - n.start.max(w.0)).max(0.0);
        }
    }
    s
}

fn main() {
    let mut tot = [S::default(); 2];
    for path in std::env::args().skip(1) {
        let song = match SongFile::open(std::path::Path::new(&path)) {
            Ok(s) => s,
            Err(e) => {
                println!("SKIP {path}: {e}");
                continue;
            }
        };
        let (_, doc) = jk_guide::file_state(&song);
        let Some(mut d) = doc else { continue };
        let lyrics = song.effective_lyrics().ok().flatten();
        let offset_ms = song.effective_settings().lyrics_offset_ms;
        let is_override = song.user_state().lyrics_override.is_some();
        if !d.word_refs_valid(lyrics.as_ref(), offset_ms) {
            jk_guide::set_word_refs(&mut d, lyrics.as_ref(), is_override, offset_ms);
        }
        let off = offset_ms as f64 / 1000.0;
        let words: Vec<(f64, f64)> = lyrics
            .as_ref()
            .map(|l| l.lines.iter().flat_map(|ln| ln.words.iter().map(|w| (w.start + off, w.end + off))).filter(|w| w.1 > w.0).collect())
            .unwrap_or_default();
        // As the app did before D-280, then as it does now.
        jk_guide::singable::singable(&mut d);
        let before = measure(&d, &words);
        jk_guide::singable::fit_to_words(&mut d, lyrics.as_ref(), offset_ms);
        let after = measure(&d, &words);
        let name: String = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().chars().take(44).collect()).unwrap_or_default();
        let pct = |s: &S| 100.0 * s.covered_s / s.word_s.max(1e-9);
        println!(
            "{name:44} bars {:4} -> {:4}  no-word {:3} -> {:3}  held >0.5 s past word {:3} -> {:3}  words covered {:5.1}% -> {:5.1}%",
            before.notes, after.notes, before.no_word, after.no_word, before.past_end, after.past_end, pct(&before), pct(&after)
        );
        for (t, s) in tot.iter_mut().zip([before, after]) {
            t.notes += s.notes;
            t.no_word += s.no_word;
            t.past_end += s.past_end;
            t.word_s += s.word_s;
            t.covered_s += s.covered_s;
        }
    }
    let pct = |s: &S| 100.0 * s.covered_s / s.word_s.max(1e-9);
    println!(
        "TOTAL bars {} -> {}  no-word {} -> {}  held >0.5 s past word {} -> {}  words covered {:.1}% -> {:.1}%",
        tot[0].notes, tot[1].notes, tot[0].no_word, tot[1].no_word, tot[0].past_end, tot[1].past_end, pct(&tot[0]), pct(&tot[1])
    );
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Prints how `singable` changes a song's guide (counts, short notes, notes per word,
//! and the notes of one lyric line). Never prints lyrics.
//!
//! cargo run -p jk-guide --example singable_stats -- <song.jksong> [line]
use jk_format::SongFile;

fn stats(label: &str, d: &jk_format::guide::GuideDoc) {
    let n = d.notes.len();
    let short = d.notes.iter().filter(|x| x.end - x.start < 0.12).count();
    let unp = d.notes.iter().filter(|x| !x.is_pitched()).count();
    let graded = d.notes.iter().filter(|x| x.graded(d.low_confidence)).count();
    let dim = d.notes.iter().filter(|x| x.confidence < d.low_confidence).count();
    let mut words = std::collections::HashMap::new();
    for x in &d.notes {
        if let Some(w) = &x.word {
            *words.entry((w.0, w.1)).or_insert(0usize) += 1;
        }
    }
    let multi = words.values().filter(|&&c| c > 1).count();
    println!("{label}: notes {n}, <120ms {short}, unpitched {unp}, graded {graded}, unsure {dim}, words with notes {}, words with >1 note {multi}, spans {}", words.len(), d.spans.len());
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let song = SongFile::open(std::path::Path::new(&a[0])).expect("open");
    let (_, doc) = jk_guide::file_state(&song);
    let mut d = doc.expect("guide");
    let line: Option<usize> = a.get(1).and_then(|s| s.parse().ok());
    let show = |d: &jk_format::guide::GuideDoc| {
        if let Some(l) = line {
            for x in d.notes.iter().filter(|x| x.word.as_ref().is_some_and(|w| w.0 == l)) {
                let w = x.word.as_ref().unwrap();
                println!("  {:.3}-{:.3} midi {:.2} {} conf {:.2} word {}", x.start, x.end, x.midi, &x.kind[..1], x.confidence, w.1);
            }
        }
    };
    stats("stored ", &d);
    show(&d);
    jk_guide::singable::singable(&mut d);
    stats("singable", &d);
    show(&d);
}

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
    // Notes 9+ semitones from both neighbours (within 1.5 s), the same way.
    let spikes = d
        .notes
        .windows(3)
        .filter(|w| {
            let (a, b, c) = (&w[0], &w[1], &w[2]);
            let (d1, d2) = (b.midi - a.midi, b.midi - c.midi);
            b.start - a.end <= 1.5 && c.start - b.end <= 1.5 && d1.signum() == d2.signum() && d1.abs().min(d2.abs()) >= 9.0
        })
        .count();
    // Octave outliers: notes more than 12 / 18 semitones from the weighted median of the
    // notes within 10 s, and leaps over an octave between notes less than 0.6 s apart.
    let mut far12 = 0;
    let mut far18 = 0;
    for x in &d.notes {
        let m = 0.5 * (x.start + x.end);
        let mut v: Vec<(f64, f64)> = d
            .notes
            .iter()
            .filter(|y| (0.5 * (y.start + y.end) - m).abs() <= 10.0)
            .map(|y| (y.midi, (y.end - y.start).clamp(0.05, 0.5) * (0.4 + 0.6 * y.confidence)))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        let tot: f64 = v.iter().map(|p| p.1).sum();
        let mut acc = 0.0;
        let mut c = x.midi;
        for (p, w) in &v {
            acc += w;
            if acc >= tot / 2.0 {
                c = *p;
                break;
            }
        }
        let off = (x.midi - c).abs();
        far12 += usize::from(off > 12.0);
        far18 += usize::from(off > 18.0);
    }
    let big_leaps = d.notes.windows(2).filter(|w| w[1].start - w[0].end < 0.6 && (w[1].midi - w[0].midi).abs() > 12.0).count();
    println!("{label}: notes {n}, <120ms {short}, unpitched {unp}, graded {graded}, unsure {dim}, words with notes {}, words with >1 note {multi}, spans {}, octave spikes {spikes}, far>12 {far12}, far>18 {far18}, leaps>12 {big_leaps}", words.len(), d.spans.len());
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let song = SongFile::open(std::path::Path::new(&a[0])).expect("open");
    let (_, doc) = jk_guide::file_state(&song);
    let mut d = doc.expect("guide");
    let line: Option<usize> = a.get(1).and_then(|s| s.parse().ok());
    // "t0:t1" instead of a line: the notes between two song times (s).
    let window: Option<(f64, f64)> = a.get(1).and_then(|s| s.split_once(':')).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
    let show = |d: &jk_format::guide::GuideDoc| {
        if let Some((t0, t1)) = window {
            for x in d.notes.iter().filter(|x| x.end > t0 && x.start < t1) {
                let w = x.word.as_ref().map(|w| format!("{}:{}", w.0, w.1)).unwrap_or_else(|| "-".into());
                println!("  {:.3}-{:.3} midi {:.2} {} conf {:.2} oct {} glide {:.2} word {w}", x.start, x.end, x.midi, &x.kind[..1], x.confidence, x.octave, x.glide_in);
            }
        }
        if let Some(l) = line {
            for x in d.notes.iter().filter(|x| x.word.as_ref().is_some_and(|w| w.0 == l)) {
                let w = x.word.as_ref().unwrap();
                println!("  {:.3}-{:.3} midi {:.2} {} conf {:.2} word {}", x.start, x.end, x.midi, &x.kind[..1], x.confidence, w.1);
            }
        }
    };
    stats("stored ", &d);
    show(&d);
    let stored = d.notes.clone();
    jk_guide::singable::singable(&mut d);
    stats("singable", &d);
    show(&d);
    // "moved": every note now sung an octave or two away from the stored note under it.
    if a.iter().any(|x| x == "moved") {
        for x in &d.notes {
            let best = stored
                .iter()
                .filter(|y| y.end > x.start && y.start < x.end)
                .max_by(|p, q| (p.end.min(x.end) - p.start.max(x.start)).total_cmp(&(q.end.min(x.end) - q.start.max(x.start))));
            if let Some(y) = best {
                let diff = x.midi - y.midi;
                if diff.abs() >= 11.0 {
                    println!("  moved {:8.3}-{:8.3}  {:6.2} -> {:6.2} ({:+.0})  conf {:.2} oct {}", x.start, x.end, y.midi, x.midi, diff, y.confidence, y.octave);
                }
            }
        }
    }
}

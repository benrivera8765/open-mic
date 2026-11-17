// SPDX-License-Identifier: GPL-3.0-or-later
//! The demo song: valid, deterministic, small, and its lyrics are the constructed ground
//! truth the ticker test relies on (letters inside their syllable's note, held vowels
//! long, an instrumental break of about 8 s), matching the committed fixture.

use std::path::PathBuf;

use jk_engine::demo::{demo_lyrics, make_demo_song};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/generated/demo-song.jksong")
}

#[test]
fn demo_song_is_valid_small_and_reproducible() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-engine-demo");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("demo.jksong");
    let made = make_demo_song(&p).unwrap();
    assert!(made.bytes < 3_000_000, "{} bytes", made.bytes);
    let rep = jk_format::verify_file(&p, &Default::default());
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    // The committed copy is byte-identical to a fresh generation.
    let a = std::fs::read(&p).unwrap();
    let b = std::fs::read(fixture()).expect("tests/fixtures/generated/demo-song.jksong");
    assert!(a == b, "the committed demo song differs from `jukeoke-cli make-demo-song` output; regenerate it");
}

#[test]
fn demo_lyrics_are_the_constructed_ground_truth() {
    let l = demo_lyrics();
    assert!(l.validate(40.0, "demo").is_empty(), "{:?}", l.validate(40.0, "demo"));
    assert!(l.lint("demo").is_empty(), "{:?}", l.lint("demo"));
    assert_eq!(l.lines.len(), 6);
    let texts: Vec<String> = l.lines.iter().map(|x| x.text()).collect();
    assert_eq!(texts[0], "Twinkle, twinkle, little star,");
    assert_eq!(texts[1], "How I wonder what you are!");
    assert_eq!(texts[2], "Up above the world so high,");
    assert_eq!(texts[3], "Like a diamond in the sky.");
    assert_eq!(texts[4], texts[0]);
    assert_eq!(texts[5], texts[1]);
    // Lines start on the beat grid (120 BPM): 2, 6, 10, 14, then 26, 30 s.
    let starts: Vec<f64> = l.lines.iter().map(|x| x.start).collect();
    assert_eq!(starts, vec![2.0, 6.0, 10.0, 14.0, 26.0, 30.0]);
    // The instrumental break: no letter between the end of line 4 and line 5 for > 8 s.
    assert!(l.lines[4].start - l.lines[3].end > 8.0);
    // Every letter is inside a note: notes start on half-second beats; a syllable's
    // letters are contiguous and the vowel nucleus holds longest.
    let mut n = 0;
    for line in &l.lines {
        for w in &line.words {
            for c in &w.chars {
                assert!(c.end >= c.start);
                n += 1;
            }
            let alpha: Vec<_> = w.chars.iter().filter(|c| c.text.chars().all(char::is_alphabetic)).collect();
            let longest = alpha.iter().map(|c| c.end - c.start).fold(0.0, f64::max);
            let vowel_longest = alpha
                .iter()
                .filter(|c| "aeiouyAEIOUY".contains(c.text.as_str()))
                .map(|c| c.end - c.start)
                .fold(0.0, f64::max);
            assert_eq!(longest, vowel_longest, "a vowel holds longest in {:?}", w.text());
        }
    }
    assert_eq!(n, 139);
    // Spot check by construction: "Twin" = T, w (60 ms each), i held, n (60 ms) over
    // 2.0 .. 2.45 s.
    let tw = &l.lines[0].words[0].chars;
    let t = |i: usize| (tw[i].text.as_str(), tw[i].start, tw[i].end);
    assert_eq!(t(0), ("T", 2.0, 2.06));
    assert_eq!(t(1), ("w", 2.06, 2.12));
    assert_eq!(t(2), ("i", 2.12, 2.39));
    assert_eq!(t(3), ("n", 2.39, 2.45));
    // The file carries exactly these lyrics.
    let f = jk_format::SongFile::open(fixture()).unwrap();
    assert_eq!(f.lyrics().unwrap().unwrap(), l);
}

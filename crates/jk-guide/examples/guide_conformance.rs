// SPDX-License-Identifier: GPL-3.0-or-later
//! The Rust side of the 1.1 conformance steps (tests/conformance/run.sh, steps 8-10):
//! the Python and Rust implementations of the melody guide checked against each other.
//!
//! ```text
//! guide_conformance fingerprint SONG OUT.json   SHA-256 of every chunk (with its offset) and of both slot regions
//! guide_conformance add SONG                    the real builder + the raw-copy rewrite (jk_guide::add_to_song, forced)
//! guide_conformance check SONG BEFORE.json      verify (strict, deep); the word references equal the Rust rule's;
//!                                               every chunk but the guide's and both slots equal BEFORE
//! guide_conformance interrupt SONG DIR          a copy per interruption point, each interrupted; DIR/<point>.sha256
//!                                               holds the copy's SHA-256 before the attempt
//! guide_conformance check-interrupted DIR       each copy is byte-identical to its .sha256 and verifies
//! ```
//!
//! Prints no lyrics (word references are compared as indices).

use std::path::{Path, PathBuf};

use jk_format::guide::{GuideDoc, read_guide};
use jk_format::guide_write::{FailPoint, hex};
use jk_format::{RewriteControl, SongFile, VerifyOptions, verify_file};
use serde_json::{Value, json};
use sha2::Digest;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn sha(b: &[u8]) -> String {
    hex(&sha2::Sha256::digest(b))
}

fn fingerprint(song: &Path) -> Res<Value> {
    let bytes = std::fs::read(song)?;
    let s = SongFile::open(song)?;
    let h = s.header().clone();
    let chunks: Vec<Value> = s
        .toc()
        .chunks
        .iter()
        .map(|c| json!([c.id, c.offset, sha(&bytes[c.offset as usize..(c.offset + c.length) as usize])]))
        .collect();
    let region = |o: u64| sha(&bytes[o as usize..(o + h.slot_size as u64) as usize]);
    Ok(json!({"chunks": chunks, "slot_a": region(h.slot_a_offset), "slot_b": region(h.slot_b_offset)}))
}

fn without_guide(v: &Value) -> Vec<Value> {
    v["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c[0] != "guide" && c[0] != "guide.curve")
        .cloned()
        .collect()
}

fn ok(cond: bool, what: &str, fails: &mut usize) {
    println!("{}{what}", if cond { "  ok    " } else { "  FAIL  " });
    if !cond {
        *fails += 1;
    }
}

const POINTS: [(&str, FailPoint); 8] = [
    ("after_placeholder", FailPoint::AfterPlaceholder),
    ("after_slots", FailPoint::AfterSlots),
    ("mid_chunks", FailPoint::MidChunks),
    ("before_toc", FailPoint::BeforeToc),
    ("before_header", FailPoint::BeforeHeader),
    ("before_second_slot_copy", FailPoint::BeforeSecondSlotCopy),
    ("before_rename", FailPoint::BeforeRename),
    ("truncated_temp", FailPoint::TruncatedTemp),
];

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut fails = 0usize;
    match args.first().map(String::as_str) {
        Some("fingerprint") => {
            std::fs::write(&args[2], serde_json::to_vec_pretty(&fingerprint(Path::new(&args[1]))?)?)?;
        }
        Some("add") => {
            let app = jk_guide::app_stamp("guide_conformance", env!("CARGO_PKG_VERSION"));
            let o = jk_guide::add_to_song(Path::new(&args[1]), &Default::default(), &app, true)?;
            let b = o.built.expect("forced");
            println!("  Rust added a guide: {} notes, {:.2} s", b.doc.notes.len(), o.total_seconds);
        }
        Some("check") => {
            let song = Path::new(&args[1]);
            let before: Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
            let rep = verify_file(song, &VerifyOptions { deep: true, ..Default::default() });
            ok(rep.ok() && rep.warnings.is_empty(), &format!("Rust verify --strict --deep ({} errors, {} warnings: {:?} {:?})", rep.errors.len(), rep.warnings.len(), rep.errors, rep.warnings), &mut fails);
            let s = SongFile::open(song)?;
            let (doc, curve) = read_guide(&s)?.ok_or("no guide")?;
            ok(curve.is_some(), "the curve decodes in Rust", &mut fails);
            // The word references Python stored equal the Rust rule's.
            let lyrics = s.effective_lyrics()?;
            let offset = s.effective_settings().lyrics_offset_ms;
            ok(doc.word_refs_valid(lyrics.as_ref(), offset), "stored word references are valid by the cache rule", &mut fails);
            let mut mine: GuideDoc = doc.clone();
            jk_guide::set_word_refs(&mut mine, lyrics.as_ref(), s.user_state().lyrics_override.is_some(), offset);
            let a: Vec<_> = doc.notes.iter().map(|n| n.word.map(|w| (w.0, w.1, w.2))).collect();
            let b: Vec<_> = mine.notes.iter().map(|n| n.word.map(|w| (w.0, w.1, w.2))).collect();
            let with = a.iter().filter(|x| x.is_some()).count();
            ok(a == b && with > 0, &format!("word references: Rust's rule gives the same {with} of {} notes", a.len()), &mut fails);
            ok(doc.compute_counts() == doc.counts.clone().unwrap_or_default(), "counts agree with the notes", &mut fails);
            let after = fingerprint(song)?;
            ok(without_guide(&after) == without_guide(&before), "every other chunk: same SHA-256 at the same offset", &mut fails);
            ok(after["slot_a"] == before["slot_a"] && after["slot_b"] == before["slot_b"], "both slot regions: same SHA-256", &mut fails);
        }
        Some("interrupt") => {
            let song = Path::new(&args[1]);
            let dir = PathBuf::from(&args[2]);
            std::fs::create_dir_all(&dir)?;
            let app = jk_guide::app_stamp("guide_conformance", env!("CARGO_PKG_VERSION"));
            let built = jk_guide::build_for_song(song, &Default::default(), &app, &mut Default::default())?;
            for (name, point) in POINTS {
                let copy = dir.join(format!("{name}.jksong"));
                std::fs::copy(song, &copy)?;
                std::fs::write(dir.join(format!("{name}.sha256")), sha(&std::fs::read(&copy)?))?;
                let mut ctl = RewriteControl { fail_at: Some(point), ..Default::default() };
                let r = jk_format::add_guide(&copy, &built.chunks(), &mut ctl);
                ok(r.is_err_and(|e| e.to_string().contains("simulated")), &format!("Rust interrupted at {name}"), &mut fails);
            }
        }
        Some("check-interrupted") => {
            let dir = PathBuf::from(&args[1]);
            let mut n = 0;
            for e in std::fs::read_dir(&dir)? {
                let p = e?.path();
                if p.extension().is_some_and(|x| x == "sha256") {
                    let song = p.with_extension("jksong");
                    let want = std::fs::read_to_string(&p)?;
                    let same = sha(&std::fs::read(&song)?) == want.trim();
                    let rep = verify_file(&song, &VerifyOptions { decode_stems: false, ..Default::default() });
                    ok(same && rep.ok(), &format!("Rust: {} is the old file, byte for byte, and verifies", song.file_name().unwrap().to_string_lossy()), &mut fails);
                    n += 1;
                }
            }
            let listed = jk_format::list_song_files(&dir, false)?;
            ok(listed.len() == n && n == POINTS.len(), &format!("Rust lists {} songs and no temporary file", listed.len()), &mut fails);
        }
        _ => {
            eprintln!("usage: guide_conformance fingerprint|add|check|interrupt|check-interrupted ...");
            std::process::exit(2);
        }
    }
    if fails > 0 {
        println!("guide_conformance: FAIL ({fails})");
        std::process::exit(1);
    }
    Ok(())
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Version 1.1: the melody guide in the file (SONG_FORMAT.md sections 7.1, 9, 10) and the
//! practice data in the slot (section 11).
//!
//! - Adding a guide by the raw-copy rewrite keeps every other chunk and both slot
//!   regions byte-identical (SHA-256), at the same offsets, even an invalid slot.
//! - Replacing a guide keeps its place in the chunk order.
//! - A simulated interruption at every point of the rewrite leaves the original file
//!   byte-identical and valid (and a later rewrite succeeds); cancelling leaves nothing.
//! - A settings save made between writing the temporary file and the swap is kept.
//! - The original changing meanwhile (a whole-file rewrite) stops the swap.
//! - A damaged chunk stops the rewrite and the original stays.
//! - verify: the guide checks, a stale guide, a missing curve, `--deep`.
//! - Practice data: rounding, and the size rule instead of a whole-file rewrite.

#[path = "../examples/support/flac_verbatim.rs"]
mod flac_verbatim;

use std::fs;
use std::path::{Path, PathBuf};

use jk_format::guide::*;
use jk_format::guide_write::{FailPoint, hex, sha256_of_range};
use jk_format::*;
use serde_json::{Map, Value, json};
use sha2::Digest;

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "jkformat-guide-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const RATE: u32 = 44_100;
const FRAMES: usize = 44_100 + 1234;
const ROLES: [&str; 4] = ["lead_vocal", "backing_vocals", "drums", "other"];

fn stem_samples(k: usize) -> Vec<i32> {
    let mut v = Vec::with_capacity(FRAMES * 2);
    for i in 0..FRAMES {
        for c in 0..2 {
            let f = 110.0 * (k + 1) as f64 + 37.0 * c as f64;
            let x = (2.0 * std::f64::consts::PI * f * i as f64 / RATE as f64).sin() * 0.25;
            v.push((x * 8_388_608.0).round() as i32);
        }
    }
    v
}

fn duration() -> f64 {
    FRAMES as f64 / RATE as f64
}

fn write_song(path: &Path) {
    let stems: Vec<Value> = ROLES
        .iter()
        .map(|r| {
            json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24, "gain": 1.0, "peak": 0.25,
                   "in_karaoke_mix": *r != "lead_vocal", "pitched": *r != "drums"})
        })
        .collect();
    let manifest: Manifest = serde_json::from_value(json!({
        "format": "jksong", "format_version": "1.0", "title": "Guide test", "artist": "Generator", "album": null,
        "sample_rate": RATE, "channels": 2, "duration_frames": FRAMES, "duration_seconds": duration(),
        "cover": null, "lyrics": "lyrics", "analysis": {"key": null, "tempo_bpm": null},
        "preset": "full", "stems_sum_to_source": true, "stems": stems, "provenance": {}
    }))
    .unwrap();
    let mut w = SongWriter::new(manifest);
    w.add_json_chunk(
        "lyrics",
        &json!({"lyrics_version": 1, "language": "en", "source": "user_text", "checked": true, "lines": [
            {"start": 0.1, "end": 0.9, "words": [
                {"start": 0.1, "end": 0.4, "chars": [["l", 0.1, 0.2], ["a", 0.2, 0.4]]},
                {"start": 0.5, "end": 0.9, "chars": [["l", 0.5, 0.6], ["a", 0.6, 0.9]]}]}]}),
    )
    .unwrap();
    for (k, r) in ROLES.iter().enumerate() {
        let flac = flac_verbatim::encode(&stem_samples(k), 2, RATE, RATE as u64);
        w.add_chunk(format!("stem.{r}"), "audio/flac", ChunkData::Bytes(flac)).unwrap();
    }
    w.add_chunk("x.future", "application/octet-stream", ChunkData::Bytes(b"opaque \xff bytes".to_vec())).unwrap();
    w.write_atomic(path).unwrap();
    // Two saves: both slots valid, B active.
    update_user_state(path, |s| s.settings.semitones = -2).unwrap();
    update_user_state(path, |s| {
        s.settings.semitones = -3;
        s.extra.insert("x_unknown_state".into(), json!({"kept": true}));
    })
    .unwrap();
}

fn guide_for(song: &Path, notes: &[(f64, f64, f64)]) -> GuideChunks {
    let s = SongFile::open(song).unwrap();
    let lead = lead_entry(&s).unwrap().clone();
    let sha = sha256_of_range(song, lead.offset, lead.length).unwrap();
    let mut doc = GuideDoc {
        guide_version: 1,
        hop_s: 0.01,
        low_confidence: 0.5,
        tuning: Tuning { cents: 0.0, strength: 1.0, use_cents: 0.0, extra: Map::new() },
        range: None,
        counts: None,
        curve: Some(CURVE_CHUNK.into()),
        notes: notes
            .iter()
            .map(|&(a, b, m)| GuideNote {
                start: a,
                end: b,
                midi: m,
                confidence: 0.95,
                kind: "pitched".into(),
                octave: "ok".into(),
                glide_in: 0.0,
                word: None,
                extra: Map::new(),
            })
            .collect(),
        spans: vec![],
        words: Some(WordsRef { lyrics: "chunk".into(), offset_ms: 0, word_count: 2, extra: Map::new() }),
        provenance: GuideProvenance {
            method: "test".into(),
            version: "0.1.0".into(),
            parameters: Map::new(),
            source: Some(GuideSource {
                role: "lead_vocal".into(),
                chunk: "stem.lead_vocal".into(),
                length: lead.length,
                crc32: lead.crc32,
                sha256: sha,
                sample_rate: RATE,
                channels: 2,
                extra: Map::new(),
            }),
            analysis_rate_hz: Some(14_700.0),
            app: None,
            created_utc: Some("2026-11-19T00:00:00Z".into()),
            seconds: Some(0.01),
            extra: Map::new(),
        },
        extra: Map::new(),
    };
    doc.counts = Some(doc.compute_counts());
    doc.range = doc.compute_range();
    let n = Curve::frames_for(duration(), 10_000);
    let curve = Curve::new(10_000, vec![45.0; n], vec![0.9; n], vec![-12.0; n]);
    GuideChunks { guide_json: doc.to_json(), curve: curve.encode() }
}

fn sha(b: &[u8]) -> String {
    hex(&sha2::Sha256::digest(b))
}

/// SHA-256 of every chunk (by id, with its offset) and of both slot regions.
fn fingerprints(p: &Path) -> (Vec<(String, u64, String)>, String, String) {
    let bytes = fs::read(p).unwrap();
    let s = SongFile::open(p).unwrap();
    let h = s.header().clone();
    let chunks = s
        .toc()
        .chunks
        .iter()
        .map(|c| (c.id.clone(), c.offset, sha(&bytes[c.offset as usize..(c.offset + c.length) as usize])))
        .collect();
    let a = sha(&bytes[h.slot_a_offset as usize..(h.slot_a_offset + h.slot_size as u64) as usize]);
    let b = sha(&bytes[h.slot_b_offset as usize..(h.slot_b_offset + h.slot_size as u64) as usize]);
    (chunks, a, b)
}

fn no_guide(v: &[(String, u64, String)]) -> Vec<(String, u64, String)> {
    v.iter().filter(|c| c.0 != GUIDE_CHUNK && c.0 != CURVE_CHUNK).cloned().collect()
}

fn temp_files(d: &TempDir) -> Vec<PathBuf> {
    fs::read_dir(&d.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".tmp-"))
        .collect()
}

#[test]
fn adding_a_guide_keeps_every_other_byte() {
    let d = TempDir::new("add");
    let p = d.path("song.jksong");
    write_song(&p);
    let before = fingerprints(&p);
    let g = guide_for(&p, &[(0.1, 0.4, 45.0), (0.5, 0.9, 47.0)]);
    let w = add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
    assert_eq!(w.header.minor, 1, "1.1 writers write minor 1");
    let after = fingerprints(&p);
    // Every other chunk: same bytes at the same offsets, in the same order.
    assert_eq!(no_guide(&after.0), before.0);
    // Both slot regions byte-identical.
    assert_eq!((after.1.clone(), after.2.clone()), (before.1, before.2));
    // The guide is appended after the last chunk, curve after it.
    let ids: Vec<&str> = after.0.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(&ids[ids.len() - 2..], &[GUIDE_CHUNK, CURVE_CHUNK]);
    let s = SongFile::open(&p).unwrap();
    assert_eq!(s.effective_settings().semitones, -3);
    assert_eq!(s.user_state().extra.get("x_unknown_state"), Some(&json!({"kept": true})));
    let (doc, curve) = read_guide(&s).unwrap().unwrap();
    assert_eq!(doc.to_json(), g.guide_json);
    assert_eq!(curve.unwrap().encode(), g.curve);
    assert_eq!(guide_state(&s, "test", "0.1.0").0, GuideFileState::Ready);
    assert_eq!(guide_state(&s, "test", "0.2.0").0, GuideFileState::Outdated);
    assert_eq!(guide_state(&s, "other", "0.1.0").0, GuideFileState::Outdated);
    let rep = verify_file(&p, &VerifyOptions { decode_stems: false, deep: true, ..Default::default() });
    assert!(rep.ok(), "{:?}", rep.errors);
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    assert!(rep.guide.as_deref().unwrap().contains("2 notes"), "{:?}", rep.guide);
    assert!(temp_files(&d).is_empty());
}

#[test]
fn replacing_a_guide_keeps_its_place_and_the_rest() {
    let d = TempDir::new("replace");
    let p = d.path("song.jksong");
    write_song(&p);
    add_guide(&p, &guide_for(&p, &[(0.1, 0.4, 45.0)]), &mut RewriteControl::default()).unwrap();
    let first = fingerprints(&p);
    let g2 = guide_for(&p, &[(0.1, 0.3, 50.0), (0.3, 0.9, 52.0)]);
    add_guide(&p, &g2, &mut RewriteControl::default()).unwrap();
    let second = fingerprints(&p);
    let pos = |v: &[(String, u64, String)], id: &str| v.iter().position(|c| c.0 == id).unwrap();
    assert_eq!(pos(&first.0, GUIDE_CHUNK), pos(&second.0, GUIDE_CHUNK));
    assert_eq!(no_guide(&second.0), no_guide(&first.0));
    assert_eq!((&second.1, &second.2), (&first.1, &first.2));
    let s = SongFile::open(&p).unwrap();
    assert_eq!(read_guide(&s).unwrap().unwrap().0.notes.len(), 2);
}

#[test]
fn an_invalid_slot_is_copied_raw_too() {
    let d = TempDir::new("badslot");
    let p = d.path("song.jksong");
    write_song(&p);
    // Garbage in slot A (the inactive one after two saves is A): an interrupted save.
    let h = SongFile::open(&p).unwrap().header().clone();
    let mut bytes = fs::read(&p).unwrap();
    for b in &mut bytes[h.slot_a_offset as usize + 40..h.slot_a_offset as usize + 90] {
        *b ^= 0x5A;
    }
    fs::write(&p, &bytes).unwrap();
    let before = fingerprints(&p);
    add_guide(&p, &guide_for(&p, &[(0.1, 0.4, 45.0)]), &mut RewriteControl::default()).unwrap();
    let after = fingerprints(&p);
    assert_eq!((after.1, after.2), (before.1, before.2));
}

#[test]
fn an_interruption_at_any_point_leaves_the_old_file_or_the_new_one() {
    let points = [
        FailPoint::AfterPlaceholder,
        FailPoint::AfterSlots,
        FailPoint::MidChunks,
        FailPoint::BeforeToc,
        FailPoint::BeforeHeader,
        FailPoint::BeforeSecondSlotCopy,
        FailPoint::BeforeRename,
        FailPoint::TruncatedTemp,
    ];
    for at in points {
        let d = TempDir::new("interrupt");
        let p = d.path("song.jksong");
        write_song(&p);
        let original = fs::read(&p).unwrap();
        let g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
        let mut ctl = RewriteControl { fail_at: Some(at), ..Default::default() };
        let e = add_guide(&p, &g, &mut ctl).unwrap_err();
        assert!(e.to_string().contains("simulated interruption"), "{at:?}: {e}");
        // The original is untouched and valid; the leftover is not listed as a song.
        assert_eq!(fs::read(&p).unwrap(), original, "{at:?}: the original changed");
        assert!(verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() }).ok(), "{at:?}");
        assert_eq!(temp_files(&d).len(), 1, "{at:?}: a crash leaves its temporary file");
        assert_eq!(list_song_files(&d.0, false).unwrap(), vec![p.clone()], "{at:?}");
        // The leftover never opens as a valid song unless it was complete.
        let tmp = &temp_files(&d)[0];
        let opened = SongFile::open(tmp);
        if matches!(at, FailPoint::BeforeRename | FailPoint::BeforeSecondSlotCopy) {
            assert!(opened.is_ok(), "{at:?}: a complete temporary file");
        } else {
            assert!(opened.is_err(), "{at:?}: an incomplete temporary file must not open");
        }
        // Done again, it works; the new file is complete.
        add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
        let rep = verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() });
        assert!(rep.ok() && rep.guide.as_deref() != Some("none"), "{at:?}: {:?}", rep.errors);
    }
}

#[test]
fn cancelling_leaves_the_song_and_no_temporary_file() {
    let d = TempDir::new("cancel");
    let p = d.path("song.jksong");
    write_song(&p);
    let original = fs::read(&p).unwrap();
    let flag = std::sync::atomic::AtomicBool::new(true);
    let mut ctl = RewriteControl { cancel: Some(&flag), ..Default::default() };
    let e = add_guide(&p, &guide_for(&p, &[(0.1, 0.4, 45.0)]), &mut ctl).unwrap_err();
    assert!(jk_format::guide_write::is_cancelled(&e), "{e}");
    assert_eq!(fs::read(&p).unwrap(), original);
    assert!(temp_files(&d).is_empty());
}

#[test]
fn a_settings_save_during_the_build_is_kept() {
    let d = TempDir::new("save-during");
    let p = d.path("song.jksong");
    write_song(&p);
    let g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
    let prep = prepare_guide_rewrite(&p, &g, &mut RewriteControl::default()).unwrap();
    // The singer changes key while the temporary file waits (a deferred swap).
    update_user_state(&p, |s| s.settings.semitones = 5).unwrap();
    let before = fingerprints(&p);
    prep.finish(&mut RewriteControl::default()).unwrap();
    let after = fingerprints(&p);
    assert_eq!((after.1, after.2), (before.1, before.2), "slots copied again before the swap");
    assert_eq!(SongFile::open(&p).unwrap().effective_settings().semitones, 5);
}

#[test]
fn the_original_changing_meanwhile_stops_the_swap() {
    let d = TempDir::new("changed");
    let p = d.path("song.jksong");
    write_song(&p);
    let g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
    let prep = prepare_guide_rewrite(&p, &g, &mut RewriteControl::default()).unwrap();
    let tmp = prep.tmp.clone();
    // A lyrics_override that outgrew its slot rewrites the whole file with larger slots.
    rewrite_song(&p, Rewrite { slot_size: Some(2 * DEFAULT_SLOT_SIZE), ..Default::default() }).unwrap();
    let rewritten = fs::read(&p).unwrap();
    let e = prep.finish(&mut RewriteControl::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ChangedOnDisk, "{e}");
    assert_eq!(fs::read(&p).unwrap(), rewritten, "the original stays");
    assert!(!tmp.exists(), "the temporary file is deleted");
}

#[test]
fn a_damaged_chunk_stops_the_rewrite() {
    let d = TempDir::new("damaged");
    let p = d.path("song.jksong");
    write_song(&p);
    let g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
    let s = SongFile::open(&p).unwrap();
    let c = s.chunk("stem.drums").unwrap().clone();
    drop(s);
    let mut bytes = fs::read(&p).unwrap();
    bytes[(c.offset + c.length / 2) as usize] ^= 0xFF;
    fs::write(&p, &bytes).unwrap();
    let e = add_guide(&p, &g, &mut RewriteControl::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Damaged, "{e}");
    assert_eq!(fs::read(&p).unwrap(), bytes, "the damaged original is left alone");
    assert!(temp_files(&d).is_empty());
}

#[test]
fn verify_reports_stale_guides_missing_curves_and_bad_notes() {
    let d = TempDir::new("verify");
    let p = d.path("song.jksong");
    write_song(&p);
    // A guide made from another vocal: stale (warning), still valid.
    let mut g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
    let mut doc = GuideDoc::from_json(&g.guide_json).unwrap();
    doc.provenance.source.as_mut().unwrap().crc32 ^= 1;
    doc.provenance.source.as_mut().unwrap().sha256 = "00".repeat(32);
    g.guide_json = doc.to_json();
    add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
    let rep = verify_file(&p, &VerifyOptions { decode_stems: false, deep: true, ..Default::default() });
    assert!(rep.ok(), "{:?}", rep.errors);
    assert_eq!(rep.warnings.iter().filter(|w| w.contains("stale")).count(), 2, "{:?}", rep.warnings);
    let s = SongFile::open(&p).unwrap();
    assert!(matches!(guide_state(&s, "test", "0.1.0").0, GuideFileState::Stale(_)));
    drop(s);
    // Overlapping notes and a curve that is named but empty: errors.
    let mut g = guide_for(&p, &[(0.1, 0.4, 45.0), (0.3, 0.5, 47.0)]);
    g.curve.truncate(10);
    add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
    let rep = verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() });
    assert!(rep.errors.iter().any(|e| e.contains("overlaps")), "{:?}", rep.errors);
    assert!(rep.errors.iter().any(|e| e.contains("guide.curve")), "{:?}", rep.errors);
    // A newer guide_version: a warning, not checked further, state Newer.
    let mut v: Value = serde_json::from_slice(&guide_for(&p, &[]).guide_json).unwrap();
    v["guide_version"] = json!(2);
    v["notes"] = json!("a new shape");
    let g = GuideChunks { guide_json: serde_json::to_vec(&v).unwrap(), curve: guide_for(&p, &[]).curve };
    add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
    let rep = verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() });
    assert!(rep.ok(), "{:?}", rep.errors);
    assert!(rep.warnings.iter().any(|w| w.contains("newer")), "{:?}", rep.warnings);
    assert_eq!(guide_state(&SongFile::open(&p).unwrap(), "test", "0.1.0").0, GuideFileState::Newer);
}

#[test]
fn a_whole_file_rewrite_by_a_1_0_style_writer_keeps_the_guide() {
    let d = TempDir::new("keep");
    let p = d.path("song.jksong");
    write_song(&p);
    let g = guide_for(&p, &[(0.1, 0.4, 45.0)]);
    add_guide(&p, &g, &mut RewriteControl::default()).unwrap();
    rewrite_song(&p, Rewrite { slot_size: Some(2 * DEFAULT_SLOT_SIZE), ..Default::default() }).unwrap();
    let s = SongFile::open(&p).unwrap();
    assert_eq!(s.read_chunk(GUIDE_CHUNK).unwrap(), g.guide_json);
    assert_eq!(s.read_chunk(CURVE_CHUNK).unwrap(), g.curve);
}

#[test]
fn practice_data_shrinks_instead_of_rewriting_the_file() {
    use jk_format::practice::*;
    let d = TempDir::new("practice");
    let p = d.path("song.jksong");
    write_song(&p);
    let len_before = fs::metadata(&p).unwrap().len();
    // Twenty results with 3000 rows each do not fit 256 KiB even compressed.
    let big = |i: usize| -> PracticeResult {
        let rows: Vec<Value> = (0..3000)
            .map(|k| json!([k, (k as f64) * 0.731 + i as f64, (k * 37 + i) % 101, k % 9, ((k * 13 + i) % 200) as i64 - 100]))
            .collect();
        serde_json::from_value(json!({
            "date": format!("2026-11-20T19:{:02}:00Z", i), "key": 0, "octave": 0, "octave_mode": "auto",
            "strictness": "standard", "overall": 50.0 + i as f64 * 0.37, "grade": "C", "graded": 10, "total": 12,
            "tendency_cents": 3, "sung_share": 0.95, "voiced_share": 0.9, "completed": true, "trust": [],
            "guide": "g", "scorer": "s", "app": "a", "lines": {"basis": "lyrics", "rows": rows}
        }))
        .unwrap()
    };
    for i in 0..20 {
        save_practice(&p, |pr| pr.add_result(big(i))).unwrap();
    }
    assert_eq!(fs::metadata(&p).unwrap().len(), len_before, "never a whole-file rewrite for practice data");
    let s = SongFile::open(&p).unwrap();
    let pr = s.user_state().practice().unwrap();
    assert!(pr.best.is_some());
    assert_eq!(pr.recent[0].date, "2026-11-20T19:19:00Z", "the newest result is kept");
    let with_rows = pr.recent.iter().filter(|r| r.lines.as_ref().is_some_and(|l| !l.rows.is_empty())).count();
    assert!(with_rows < pr.recent.len(), "the oldest rows were dropped");
    // Newest results keep their rows; the dropped ones are the oldest.
    assert!(pr.recent[0].lines.as_ref().unwrap().rows.len() == 3000);
    let rep = verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() });
    assert!(rep.ok(), "{:?}", rep.errors);
    // The other settings survived every save.
    assert_eq!(s.effective_settings().semitones, -3);
}

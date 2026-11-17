//! Integration tests for jk-format: round trip, every corruption case, slot A/B
//! alternation, torn slot writes, preservation of unknown chunks and keys, version
//! handling, library scan, stem streaming and decoding.

#[path = "../examples/support/flac_verbatim.rs"]
mod flac_verbatim;

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use jk_format::*;
use serde_json::{Value, json};

// ---------------------------------------------------------------- helpers

/// A fresh empty directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "jkformat-test-{}-{}-{}",
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
const FRAMES: usize = 44_100 / 2 + 1234; // not a multiple of the FLAC block size

/// Deterministic 24-bit stereo test signal for a stem: a tone per channel.
fn stem_samples(k: usize) -> Vec<i32> {
    let mut v = Vec::with_capacity(FRAMES * 2);
    for i in 0..FRAMES {
        for c in 0..2 {
            let f = 110.0 * (k + 1) as f64 + 37.0 * c as f64;
            let x = (2.0 * std::f64::consts::PI * f * i as f64 / RATE as f64).sin() * 0.25 * (c + 1) as f64;
            v.push((x * 8_388_608.0).round() as i32);
        }
    }
    v
}

const ROLES: [&str; 4] = ["lead_vocal", "backing_vocals", "drums", "other"];

fn test_manifest() -> Manifest {
    let stems: Vec<Value> = ROLES
        .iter()
        .map(|r| {
            json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24,
                   "gain": if *r == "other" { 1.25 } else { 1.0 }, "peak": 0.5,
                   "in_karaoke_mix": *r != "lead_vocal", "pitched": *r != "drums"})
        })
        .collect();
    serde_json::from_value(json!({
        "format": "jksong", "format_version": "1.0",
        "title": "Test tones", "artist": "Generator", "album": null,
        "sample_rate": RATE, "channels": 2, "duration_frames": FRAMES,
        "duration_seconds": FRAMES as f64 / RATE as f64,
        "cover": "cover", "lyrics": "lyrics",
        "analysis": {"key": {"tonic": "A", "mode": "minor", "name": "A minor", "confidence": 0.5}, "tempo_bpm": 120.0},
        "preset": "full", "stems_sum_to_source": true, "stems": stems,
        "provenance": {"app": {"name": "test", "version": "0"}},
        "x_future_manifest_key": {"nested": [1, 2, 3]}
    }))
    .unwrap()
}

fn test_lyrics() -> Value {
    json!({
        "lyrics_version": 1, "language": "en", "source": "user_text", "checked": true,
        "lines": [{"start": 0.05, "end": 0.40, "x_line_key": "kept", "words": [
            {"start": 0.05, "end": 0.20, "chars": [["c", 0.05, 0.08], ["a", 0.08, 0.11], ["f", 0.11, 0.14], ["\u{e9}", 0.14, 0.20]]},
            {"start": 0.25, "end": 0.40, "confidence": 0.9, "chars": [["n", 0.25, 0.30], ["a", 0.30, 0.33], ["i\u{308}", 0.33, 0.36], ["f", 0.36, 0.40]]}
        ]}]
    })
}

/// 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

const UNKNOWN_CHUNK: &[u8] = b"\x00\x01\x02 opaque bytes from a newer writer \xff\xfe";

fn write_test_song(path: &Path) -> WrittenSong {
    let mut w = SongWriter::new(test_manifest());
    w.add_json_chunk("lyrics", &test_lyrics()).unwrap();
    w.add_chunk("cover", "image/png", ChunkData::Bytes(PNG.to_vec())).unwrap();
    for (k, r) in ROLES.iter().enumerate() {
        let flac = flac_verbatim::encode(&stem_samples(k), 2, RATE, RATE as u64);
        w.add_chunk(format!("stem.{r}"), "audio/flac", ChunkData::Bytes(flac)).unwrap();
    }
    w.add_chunk("x.future", "application/octet-stream", ChunkData::Bytes(UNKNOWN_CHUNK.to_vec())).unwrap();
    w.write_atomic(path).unwrap()
}

fn read_all(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap()
}

/// Recomputes and stores the header CRC (for crafting files with altered fields).
fn fix_header_crc(bytes: &mut [u8]) {
    let crc = crc32fast::hash(&bytes[..124]);
    bytes[124..128].copy_from_slice(&crc.to_le_bytes());
}

/// Replaces the TOC with `toc` (re-appended at the old offset), fixing lengths and CRCs.
fn replace_toc(bytes: &mut Vec<u8>, toc: &Value) {
    let h = Header::parse(&bytes[..4096], bytes.len() as u64).unwrap();
    let t = serde_json::to_vec(toc).unwrap();
    bytes.truncate(h.toc_offset as usize);
    bytes.extend_from_slice(&t);
    bytes[24..32].copy_from_slice(&(t.len() as u64).to_le_bytes());
    bytes[32..36].copy_from_slice(&crc32fast::hash(&t).to_le_bytes());
    let len = bytes.len() as u64;
    bytes[56..64].copy_from_slice(&len.to_le_bytes());
    fix_header_crc(bytes);
}

fn toc_value(bytes: &[u8]) -> Value {
    let h = Header::parse(&bytes[..4096], bytes.len() as u64).unwrap();
    serde_json::from_slice(&bytes[h.toc_offset as usize..(h.toc_offset + h.toc_length) as usize]).unwrap()
}

// ---------------------------------------------------------------- round trip

#[test]
fn round_trip_everything() {
    let d = TempDir::new("rt");
    let p = d.path("song.jksong");
    let written = write_test_song(&p);
    assert_eq!(fs::metadata(&p).unwrap().len(), written.header.file_length);

    let song = SongFile::open(&p).unwrap();
    assert_eq!(song.header().major, 1);
    assert_eq!(song.header().minor, 0);
    assert_eq!(song.header().slot_size, DEFAULT_SLOT_SIZE);
    assert_eq!(song.manifest(), &test_manifest());
    assert_eq!(song.manifest().extra["x_future_manifest_key"], json!({"nested": [1, 2, 3]}));
    // Layout rules: manifest first, everything 4096-aligned, TOC last.
    assert_eq!(song.toc().chunks[0].id, "manifest");
    for c in &song.toc().chunks {
        assert_eq!(c.offset % 4096, 0, "{} not aligned", c.id);
        assert!(c.offset + c.length <= song.header().toc_offset);
    }
    assert_eq!(song.header().toc_offset % 4096, 0);
    assert_eq!(song.header().toc_offset + song.header().toc_length, song.header().file_length);

    // Lyrics, including grapheme strings and unknown line keys.
    let lyr = song.lyrics().unwrap().unwrap();
    assert_eq!(lyr.lines[0].text(), "caf\u{e9} nai\u{308}f");
    assert_eq!(lyr.lines[0].words[1].chars[2].text, "i\u{308}");
    assert_eq!(lyr.lines[0].extra["x_line_key"], "kept");
    assert_eq!(serde_json::to_value(&lyr).unwrap(), test_lyrics());
    assert_eq!(song.cover().unwrap().unwrap(), ("image/png".to_string(), PNG.to_vec()));
    assert_eq!(song.read_chunk("x.future").unwrap(), UNKNOWN_CHUNK);

    // Initial user state: slot A sequence 1, slot B empty, defaults.
    assert!(matches!(song.slots().a, SlotState::Valid { sequence: 1, .. }));
    assert_eq!(song.slots().b, SlotState::Empty);
    assert_eq!(song.slots().active, Some(0));
    assert_eq!(song.effective_settings(), &Settings::default());
    assert_eq!(song.effective_metadata().title, "Test tones");

    // Stems: raw bytes through the ChunkReader equal what was written; decoded samples
    // equal the input exactly.
    for (k, r) in ROLES.iter().enumerate() {
        let (stem, mut rd) = song.stem_reader(r).unwrap();
        let mut got = Vec::new();
        rd.read_to_end(&mut got).unwrap();
        assert_eq!(got, flac_verbatim::encode(&stem_samples(k), 2, RATE, RATE as u64));
        assert_eq!(stem.gain, if *r == "other" { 1.25 } else { 1.0 });
        let planes = jk_format::decode::decode_stem_f32(&song, r).unwrap();
        let want = stem_samples(k);
        for (i, w) in want.iter().enumerate() {
            let expect = (*w as f64 / 8_388_608.0 * stem.gain) as f32;
            assert_eq!(planes[i % 2][i / 2], expect, "stem {r} sample {i}");
        }
    }

    let rep = verify_file(&p, &VerifyOptions::default());
    assert!(rep.ok(), "verify errors: {:?}", rep.errors);
    assert!(rep.warnings.is_empty(), "verify warnings: {:?}", rep.warnings);
    for s in &rep.stems {
        assert_eq!(s.decoded_frames, Some(FRAMES as u64));
        assert_eq!(s.md5_ok, Some(true));
        assert_eq!(s.seeks_ok.map(|(ok, n)| ok == n && n > 0), Some(true));
    }
}

#[test]
fn chunk_reader_seek_and_read() {
    let d = TempDir::new("cr");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let song = SongFile::open(&p).unwrap();
    let mut r = song.chunk_reader("x.future").unwrap();
    assert_eq!(r.len(), UNKNOWN_CHUNK.len() as u64);
    let mut b = [0u8; 4];
    r.seek(SeekFrom::Start(3)).unwrap();
    r.read_exact(&mut b).unwrap();
    assert_eq!(&b, &UNKNOWN_CHUNK[3..7]);
    r.seek(SeekFrom::End(-2)).unwrap();
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, &UNKNOWN_CHUNK[UNKNOWN_CHUNK.len() - 2..]);
    // Past the end: reads return 0, like a file.
    r.seek(SeekFrom::Start(10_000)).unwrap();
    assert_eq!(r.read(&mut b).unwrap(), 0);
    assert!(r.seek(SeekFrom::Current(-20_000)).is_err());
    // Two readers on the same song do not disturb each other.
    let mut a = song.chunk_reader("x.future").unwrap();
    let mut c = song.chunk_reader("cover").unwrap();
    let (mut x, mut y) = ([0u8; 3], [0u8; 3]);
    a.read_exact(&mut x).unwrap();
    c.read_exact(&mut y).unwrap();
    a.read_exact(&mut x).unwrap();
    assert_eq!(&x, &UNKNOWN_CHUNK[3..6]);
    assert_eq!(&y, &PNG[..3]);
}

// ---------------------------------------------------------------- corruption

fn open_bytes(d: &TempDir, name: &str, bytes: &[u8]) -> Result<SongFile> {
    let p = d.path(name);
    fs::write(&p, bytes).unwrap();
    SongFile::open(&p)
}

#[test]
fn bad_magic_is_not_a_song_file() {
    let d = TempDir::new("magic");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let good = read_all(&p);
    for i in 0..8 {
        let mut b = good.clone();
        b[i] ^= 0x20;
        let e = open_bytes(&d, "bad.jksong", &b).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotASongFile, "byte {i}");
    }
    // Typical text-mode damage: CRLF -> LF on bytes 4..6.
    let mut b = good.clone();
    b.remove(4);
    assert_eq!(open_bytes(&d, "crlf.jksong", &b).unwrap_err().kind(), ErrorKind::NotASongFile);
    // A ZIP file, a text file.
    assert_eq!(open_bytes(&d, "zip.jksong", b"PK\x03\x04....").unwrap_err().kind(), ErrorKind::NotASongFile);
    assert_eq!(open_bytes(&d, "txt.jksong", b"hello world").unwrap_err().kind(), ErrorKind::NotASongFile);
}

#[test]
fn flipped_header_bytes_are_damage() {
    let d = TempDir::new("flip");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let good = read_all(&p);
    for i in 8..128 {
        let mut b = good.clone();
        b[i] ^= 0x04;
        let e = open_bytes(&d, "bad.jksong", &b).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Damaged, "flip at header byte {i}: {e}");
        assert!(e.user_message().contains("damaged"));
    }
}

#[test]
fn truncated_files_are_reported_as_incomplete() {
    let d = TempDir::new("trunc");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let good = read_all(&p);
    for cut in [0usize, 1, 7, 8, 64, 127, 128, 4095, 4096, 300_000, good.len() / 2, good.len() - 1] {
        let e = open_bytes(&d, "cut.jksong", &good[..cut]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Truncated, "cut at {cut}: {e}");
        assert!(e.user_message().contains("incomplete"));
    }
}

#[test]
fn bad_toc_crc_is_damage() {
    let d = TempDir::new("toc");
    let p = d.path("ok.jksong");
    let w = write_test_song(&p);
    let mut b = read_all(&p);
    let i = w.header.toc_offset as usize + 5;
    b[i] ^= 0x01;
    let e = open_bytes(&d, "bad.jksong", &b).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Damaged);
    assert!(e.to_string().contains("table of contents checksum"), "{e}");
}

#[test]
fn chunk_out_of_bounds_is_damage() {
    let d = TempDir::new("oob");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let good = read_all(&p);
    let file_len = good.len() as u64;
    // Past the end; overlapping slot A; overlapping the TOC; offset overflow.
    for (k, bad_offset, bad_len) in [
        (0, file_len - 10, 100u64),
        (1, 4096 + 8, 16),
        (2, file_len - 4, 4),
        (3, u64::MAX - 2, 10),
    ] {
        let mut b = good.clone();
        let mut toc = toc_value(&b);
        let stem = toc["chunks"].as_array_mut().unwrap().iter_mut().find(|c| c["id"] == "stem.drums").unwrap();
        stem["offset"] = json!(bad_offset);
        stem["length"] = json!(bad_len);
        replace_toc(&mut b, &toc);
        let e = open_bytes(&d, "bad.jksong", &b).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Damaged, "case {k}: {e}");
    }
    // Damaged chunk bytes: open succeeds (stems are not checked on open), verify catches.
    let mut b = good.clone();
    let toc = toc_value(&b);
    let drums = toc["chunks"].as_array().unwrap().iter().find(|c| c["id"] == "stem.drums").unwrap();
    let off = drums["offset"].as_u64().unwrap() as usize + 5000;
    b[off] ^= 0xff;
    let p2 = d.path("bitrot.jksong");
    fs::write(&p2, &b).unwrap();
    assert!(SongFile::open(&p2).is_ok());
    let rep = verify_file(&p2, &VerifyOptions::default());
    assert!(rep.errors.iter().any(|e| e.contains("stem.drums") && e.contains("CRC")), "{:?}", rep.errors);
    // A damaged manifest chunk is caught on open.
    let mut b = good.clone();
    let m = toc["chunks"].as_array().unwrap().iter().find(|c| c["id"] == "manifest").unwrap();
    b[m["offset"].as_u64().unwrap() as usize + 3] ^= 0x01;
    assert_eq!(open_bytes(&d, "badman.jksong", &b).unwrap_err().kind(), ErrorKind::Damaged);
}

#[test]
fn missing_referenced_chunk_is_reported() {
    let d = TempDir::new("miss");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let mut b = read_all(&p);
    let mut toc = toc_value(&b);
    toc["chunks"].as_array_mut().unwrap().retain(|c| c["id"] != "stem.other");
    replace_toc(&mut b, &toc);
    let e = open_bytes(&d, "bad.jksong", &b).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::MissingChunk);
}

#[test]
fn newer_major_is_refused_newer_minor_opens() {
    let d = TempDir::new("ver");
    let p = d.path("ok.jksong");
    write_test_song(&p);
    let good = read_all(&p);

    let mut b = good.clone();
    b[8..10].copy_from_slice(&2u16.to_le_bytes());
    fix_header_crc(&mut b);
    let e = open_bytes(&d, "v2.jksong", &b).unwrap_err();
    assert!(matches!(e, FormatError::NewerVersion { major: 2, minor: 0 }), "{e}");
    assert!(e.user_message().contains("newer version"));

    // 1.7: higher minor, non-zero reserved header bytes, unknown TOC keys, an unknown
    // chunk type, unknown manifest keys. All must be ignored.
    let mut b = good.clone();
    b[10..12].copy_from_slice(&7u16.to_le_bytes());
    b[64..72].copy_from_slice(b"FUTURE!!");
    fix_header_crc(&mut b);
    let mut toc = toc_value(&b);
    toc["x_toc_key"] = json!("future");
    toc["chunks"][1]["x_entry_key"] = json!(42);
    replace_toc(&mut b, &toc);
    let song = open_bytes(&d, "v17.jksong", &b).unwrap();
    assert_eq!((song.header().major, song.header().minor), (1, 7));
    assert_eq!(song.toc().extra["x_toc_key"], "future");
    assert_eq!(song.toc().chunks[1].extra["x_entry_key"], 42);
}

// ---------------------------------------------------------------- slots

#[test]
fn slots_alternate_a_b_with_increasing_sequence() {
    let d = TempDir::new("ab");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let mut song = SongFile::open(&p).unwrap();
    // Slot A holds sequence 1 from the writer, so the first save goes to B.
    for (i, want_slot) in [1usize, 0, 1, 0, 1].into_iter().enumerate() {
        let semis = i as i32 - 2;
        let w = song.save_user_state(|s| s.settings.semitones = semis).unwrap();
        assert_eq!(w.slot, want_slot, "save {i}");
        assert_eq!(w.sequence, i as u64 + 2);
        let fresh = SongFile::open(&p).unwrap();
        assert_eq!(fresh.effective_settings().semitones, semis);
        assert_eq!(fresh.slots().active, Some(want_slot));
        assert!(fresh.user_state().updated_utc.as_deref().is_some_and(|t| t.ends_with('Z')));
    }
    // The by-path API does the same.
    let w = update_user_state(&p, |s| s.settings.lyrics_offset_ms = -120).unwrap();
    assert_eq!((w.slot, w.sequence), (0, 7));
    let fresh = SongFile::open(&p).unwrap();
    assert_eq!(fresh.effective_settings().lyrics_offset_ms, -120);
    assert_eq!(fresh.effective_settings().semitones, 2); // merged, not replaced
    // The file length never changed: slots are updated in place.
    assert_eq!(fs::metadata(&p).unwrap().len(), fresh.header().file_length);
}

/// Writes `bytes` at absolute `offset` of the file at `p` (simulating a partial write).
fn poke(p: &Path, offset: u64, bytes: &[u8]) {
    let mut f = OpenOptions::new().write(true).open(p).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn torn_slot_write_keeps_previous_settings() {
    let d = TempDir::new("torn");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let mut song = SongFile::open(&p).unwrap();
    let w = song.save_user_state(|s| s.settings.semitones = 3).unwrap();
    assert_eq!((w.slot, w.sequence), (1, 2)); // active is now B
    let h = song.header().clone();
    drop(song);

    // The image the next save would write into slot A (sequence 3, semitones -5).
    let mut next = UserState::default();
    next.settings.semitones = -5;
    let image = jk_format::slot::encode_slot(3, &next.to_json(), h.slot_size).unwrap();
    let pristine = read_all(&p);
    let payload_len = u32::from_le_bytes(image[16..20].try_into().unwrap()) as usize;
    assert!(payload_len > 20);

    // Interruptions at many points of the slot write, plus garbage.
    type Damage<'a> = Box<dyn Fn() + 'a>;
    let cases: Vec<(&str, Damage)> = vec![
        ("header only, payload missing", Box::new(|| poke(&p, h.slot_a_offset, &image[..32]))),
        ("half the payload", Box::new(|| poke(&p, h.slot_a_offset, &image[..32 + payload_len / 2]))),
        ("all but the last payload byte", Box::new(|| poke(&p, h.slot_a_offset, &image[..32 + payload_len - 1]))),
        ("payload without the header", Box::new(|| poke(&p, h.slot_a_offset + 32, &image[32..400]))),
        ("first 20 bytes only", Box::new(|| poke(&p, h.slot_a_offset, &image[..20]))),
        ("garbage", Box::new(|| poke(&p, h.slot_a_offset, &[0xA5; 5000]))),
        ("zeroed header", Box::new(|| poke(&p, h.slot_a_offset, &[0u8; 32]))),
    ];
    for (name, damage) in cases {
        fs::write(&p, &pristine).unwrap();
        // Start from a slot A that holds older valid data (sequence 1), like a real file.
        damage();
        let song = SongFile::open(&p).unwrap();
        assert_eq!(song.effective_settings().semitones, 3, "case '{name}'");
        assert_eq!(song.slots().active, Some(1), "case '{name}'");
        // The next save goes into the damaged slot A and repairs it.
        let mut song = song;
        let w = song.save_user_state(|s| s.settings.guide_vocal = 0.5).unwrap();
        assert_eq!(w.slot, 0, "case '{name}'");
        assert!(w.sequence >= 3);
        let fresh = SongFile::open(&p).unwrap();
        assert_eq!(fresh.effective_settings().semitones, 3);
        assert_eq!(fresh.effective_settings().guide_vocal, 0.5);
        assert!(verify_file(&p, &VerifyOptions { decode_stems: false, ..Default::default() }).ok());
    }

    // Torn write into the *active* slot cannot happen through the API, but if both slots
    // are damaged the song still opens, with default settings.
    fs::write(&p, &pristine).unwrap();
    poke(&p, h.slot_a_offset, &[0xFF; 64]);
    poke(&p, h.slot_b_offset, &[0xFF; 64]);
    let song = SongFile::open(&p).unwrap();
    assert_eq!(song.effective_settings(), &Settings::default());
    assert_eq!(song.warnings().len(), 2);
}

#[test]
fn unknown_keys_and_chunks_survive_slot_update_and_rewrite() {
    let d = TempDir::new("keep");
    let p = d.path("s.jksong");
    write_test_song(&p);

    // A newer app wrote these keys into the user state.
    let newer = json!({
        "user_state_version": 1,
        "settings": {"semitones": 1, "x_new_setting": {"deep": true}, "levels": {"drums": 0.5, "bass": 0.7}},
        "metadata_override": {"title": "Better title", "x_meta": 1},
        "lyrics_override": null,
        "x_top": [1, "two"]
    });
    let h = SongFile::open(&p).unwrap().header().clone();
    let image = jk_format::slot::encode_slot(2, &serde_json::to_vec(&newer).unwrap(), h.slot_size).unwrap();
    poke(&p, h.slot_b_offset, &image);

    // This version changes one setting.
    let mut song = SongFile::open(&p).unwrap();
    assert_eq!(song.effective_metadata().title, "Better title");
    song.save_user_state(|s| s.settings.semitones = -4).unwrap();
    let after = |p: &Path| {
        let s = SongFile::open(p).unwrap();
        let v: Value = serde_json::from_slice(s.slots().active_json().unwrap()).unwrap();
        (s, v)
    };
    let (_, v) = after(&p);
    assert_eq!(v["settings"]["semitones"], -4);
    assert_eq!(v["settings"]["x_new_setting"], json!({"deep": true}));
    assert_eq!(v["settings"]["levels"]["bass"], 0.7);
    assert_eq!(v["metadata_override"]["x_meta"], 1);
    assert_eq!(v["x_top"], json!([1, "two"]));

    // Full rewrite (for example to store a big lyrics override): unknown chunk, TOC
    // entry keys, manifest keys and user-state keys all survive.
    let mut b = read_all(&p);
    let mut toc = toc_value(&b);
    toc["x_toc_key"] = json!("kept");
    for c in toc["chunks"].as_array_mut().unwrap() {
        if c["id"] == "x.future" {
            c["x_entry_key"] = json!("kept too");
        }
    }
    replace_toc(&mut b, &toc);
    fs::write(&p, &b).unwrap();
    let before_stems: Vec<Vec<u8>> = {
        let s = SongFile::open(&p).unwrap();
        ROLES.iter().map(|r| s.read_chunk(&format!("stem.{r}")).unwrap()).collect()
    };
    let mut big = SongFile::open(&p).unwrap().user_state().clone();
    big.lyrics_override = Some(serde_json::from_value(test_lyrics()).unwrap());
    rewrite_song(&p, Rewrite { user_state: Some(big), ..Default::default() }).unwrap();
    let (s, v) = after(&p);
    assert_eq!(s.read_chunk("x.future").unwrap(), UNKNOWN_CHUNK);
    assert_eq!(s.toc().extra["x_toc_key"], "kept");
    assert_eq!(s.chunk("x.future").unwrap().extra["x_entry_key"], "kept too");
    assert_eq!(s.manifest().extra["x_future_manifest_key"], json!({"nested": [1, 2, 3]}));
    assert_eq!(v["x_top"], json!([1, "two"]));
    assert_eq!(v["settings"]["semitones"], -4);
    assert_eq!(s.effective_lyrics().unwrap().unwrap().lines[0].extra["x_line_key"], "kept");
    for (r, before) in ROLES.iter().zip(&before_stems) {
        assert_eq!(&s.read_chunk(&format!("stem.{r}")).unwrap(), before);
    }
    // Chunk order is kept.
    let ids: Vec<&str> = s.toc().chunks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["manifest", "lyrics", "cover", "stem.lead_vocal", "stem.backing_vocals", "stem.drums", "stem.other", "x.future"]);
    assert!(verify_file(&p, &VerifyOptions::default()).ok());
    // No temporary files left behind.
    assert_eq!(stale_temp_files(&d.0).unwrap().len(), 0);
}

#[test]
fn oversized_settings_fall_back_to_rewrite_with_bigger_slots() {
    let d = TempDir::new("big");
    let p = d.path("s.jksong");
    {
        let mut w = SongWriter::new(test_manifest()).slot_size(4096);
        w.add_json_chunk("lyrics", &test_lyrics()).unwrap();
        w.add_chunk("cover", "image/png", ChunkData::Bytes(PNG.to_vec())).unwrap();
        for (k, r) in ROLES.iter().enumerate() {
            w.add_chunk(format!("stem.{r}"), "audio/flac", ChunkData::Bytes(flac_verbatim::encode(&stem_samples(k), 2, RATE, 0)))
                .unwrap();
        }
        w.write_atomic(&p).unwrap();
    }
    // An incompressible blob in an unknown key, bigger than a 4096-byte slot.
    let mut x: u64 = 88172645463325252;
    let blob: String = (0..20_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'a' + (x % 26) as u8)
        })
        .collect();
    let mut song = SongFile::open(&p).unwrap();
    let before = read_all(&p);
    let e = song.save_user_state(|s| {
        s.extra.insert("x_blob".into(), json!(blob.clone()));
    });
    match e {
        Err(FormatError::SlotPayloadTooLarge { needed, capacity }) => assert!(needed > capacity),
        other => panic!("expected SlotPayloadTooLarge, got {other:?}"),
    }
    assert_eq!(read_all(&p), before, "a refused save must not touch the file");
    drop(song);

    let mut state = SongFile::open(&p).unwrap().user_state().clone();
    state.extra.insert("x_blob".into(), json!(blob.clone()));
    let w = rewrite_song(&p, Rewrite { user_state: Some(state), ..Default::default() }).unwrap();
    assert!(w.header.slot_size >= DEFAULT_SLOT_SIZE);
    let mut song = SongFile::open(&p).unwrap();
    assert_eq!(song.user_state().extra["x_blob"], json!(blob));
    // And now quick saves work again.
    song.save_user_state(|s| s.settings.semitones = 5).unwrap();
    assert_eq!(SongFile::open(&p).unwrap().effective_settings().semitones, 5);
}

#[test]
fn effective_values_override_wins() {
    let d = TempDir::new("eff");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let mut song = SongFile::open(&p).unwrap();
    assert_eq!(song.effective_metadata(), EffectiveMetadata { title: "Test tones".into(), artist: "Generator".into(), album: None });
    let mut corrected: Lyrics = serde_json::from_value(test_lyrics()).unwrap();
    corrected.lines[0].words[0].start = 0.06;
    song.save_user_state(|s| {
        s.metadata_override.artist = Some("Corrected".into());
        s.metadata_override.album = Some(Some("An album".into()));
        s.lyrics_override = Some(corrected.clone());
    })
    .unwrap();
    let song = SongFile::open(&p).unwrap();
    let m = song.effective_metadata();
    assert_eq!((m.title.as_str(), m.artist.as_str(), m.album.as_deref()), ("Test tones", "Corrected", Some("An album")));
    assert_eq!(song.effective_lyrics().unwrap().unwrap(), corrected);
    assert_eq!(song.lyrics().unwrap().unwrap().lines[0].words[0].start, 0.05); // chunk untouched
}

#[test]
fn lenient_user_state_keeps_good_fields() {
    let v = json!({"settings": {"semitones": 2.0, "guide_vocal": "loud", "backing_on": 0, "levels": {"drums": 0.5, "x": "no"}},
                   "metadata_override": null, "lyrics_override": {"not": "lyrics"}, "updated_utc": 5});
    let s = UserState::from_value(v).unwrap();
    assert_eq!(s.settings.semitones, 2);
    assert_eq!(s.settings.guide_vocal, 0.0);
    assert!(s.settings.backing_on);
    assert_eq!(s.settings.level("drums"), 0.5);
    assert_eq!(s.settings.level("x"), 1.0);
    assert_eq!(s.metadata_override, MetadataOverride::default());
    assert!(s.lyrics_override.is_none());
    assert!(UserState::from_json(b"[1,2]").is_err());
}

// ---------------------------------------------------------------- writer details

#[test]
fn writer_rejects_bad_input_and_leaves_no_temp_file() {
    let d = TempDir::new("wbad");
    let p = d.path("s.jksong");
    // Manifest refers to stems that were not added.
    let w = SongWriter::new(test_manifest());
    assert_eq!(w.write_atomic(&p).unwrap_err().kind(), ErrorKind::InvalidInput);
    let mut w = SongWriter::new(test_manifest());
    assert!(w.add_chunk("manifest", "application/json", ChunkData::Bytes(vec![])).is_err());
    w.add_chunk("a", "x", ChunkData::Bytes(vec![])).unwrap();
    assert!(w.add_chunk("a", "x", ChunkData::Bytes(vec![])).is_err());
    assert!(!p.exists());
    assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
}

#[test]
fn rewrite_refuses_to_copy_a_damaged_chunk() {
    let d = TempDir::new("rot");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let b = read_all(&p);
    let toc = toc_value(&b);
    let other = toc["chunks"].as_array().unwrap().iter().find(|c| c["id"] == "stem.other").unwrap();
    poke(&p, other["offset"].as_u64().unwrap() + 999, &[0x5A]);
    let damaged = read_all(&p);
    let e = rewrite_song(&p, Rewrite::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Damaged);
    assert_eq!(read_all(&p), damaged, "the original must be left as it was");
    assert_eq!(stale_temp_files(&d.0).unwrap().len(), 0);
}

#[test]
fn open_reader_survives_atomic_replace() {
    // The engine may be streaming a stem while the app rewrites the file. On NTFS (and
    // on Unix) the old file stays readable through the open handle; the path now names
    // the new file.
    let d = TempDir::new("live");
    let p = d.path("s.jksong");
    write_test_song(&p);
    let song = SongFile::open(&p).unwrap();
    let mut r = song.chunk_reader("x.future").unwrap();
    rewrite_song(&p, Rewrite { remove: vec!["x.future".into()], ..Default::default() }).unwrap();
    let mut got = Vec::new();
    r.read_to_end(&mut got).unwrap();
    assert_eq!(got, UNKNOWN_CHUNK);
    assert!(SongFile::open(&p).unwrap().chunk("x.future").is_none());
    // Saving settings through the stale SongFile is refused instead of corrupting the
    // new file.
    let mut song = song;
    assert_eq!(song.save_user_state(|s| s.settings.semitones = 1).unwrap_err().kind(), ErrorKind::ChangedOnDisk);
}

// ---------------------------------------------------------------- library scan

#[test]
fn library_scan_skips_junk() {
    let d = TempDir::new("lib");
    for n in ["b.jksong", "A.JKSONG", "._b.jksong", ".hidden.jksong", "c.jksong.tmp-0123456789abcdef", "notes.txt", "x.tmp-1.jksong"] {
        fs::write(d.path(n), b"x").unwrap();
    }
    // Non-ASCII names next to the songs must neither panic the scan nor be listed.
    for n in ["naïve.txt", "日本の歌.flac"] {
        fs::write(d.path(n), b"x").unwrap();
    }
    fs::create_dir(d.path("folder.jksong")).unwrap();
    fs::create_dir(d.path("Sub")).unwrap();
    fs::write(d.path("Sub").join("d.jksong"), b"x").unwrap();
    fs::create_dir(d.path(".Trashes")).unwrap();
    fs::write(d.path(".Trashes").join("e.jksong"), b"x").unwrap();
    let names = |v: Vec<PathBuf>| -> Vec<String> {
        v.iter().map(|p| p.strip_prefix(&d.0).unwrap().to_string_lossy().replace('\\', "/")).collect()
    };
    assert_eq!(names(list_song_files(&d.0, false).unwrap()), ["A.JKSONG", "b.jksong"]);
    assert_eq!(names(list_song_files(&d.0, true).unwrap()), ["A.JKSONG", "b.jksong", "Sub/d.jksong"]);
    assert_eq!(names(stale_temp_files(&d.0).unwrap()), ["c.jksong.tmp-0123456789abcdef"]);
}

#[test]
fn json_numbers_round_trip_exactly() {
    // Values the Python writer produced in the conformance run; serde_json's default
    // fast float parser returned 0.119999997317791 and 1.2100000381469729 for them.
    for x in [0.11999999731779099f64, 1.2100000381469727, 1.222222260754518, 12.345, 1e-7, 0.1 + 0.2] {
        let text = serde_json::to_string(&json!({"gain": x})).unwrap();
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["gain"].as_f64().unwrap().to_bits(), x.to_bits(), "{text}");
        let py_text = format!("{{\"gain\":{x:?}}}"); // Rust's {:?} is shortest round-trip, like Python's repr
        let back: Value = serde_json::from_str(&py_text).unwrap();
        assert_eq!(back["gain"].as_f64().unwrap().to_bits(), x.to_bits(), "{py_text}");
    }
    // Through the flattened manifest structs too.
    let mut m = test_manifest();
    m.stems[1].peak = Some(0.11999999731779099);
    let back: Manifest = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(back.stems[1].peak.unwrap().to_bits(), 0.11999999731779099f64.to_bits());
}

//! The Rust -> Python direction of the conformance test.
//!
//! ```text
//! cargo run -p jk-format --example jksong-write-test -- <out.jksong>
//!     Writes a song from scratch with the Rust writer (stems: integer triangle waves,
//!     uncompressed "verbatim" FLAC from examples/support/flac_verbatim.rs), lyrics
//!     with non-ASCII graphemes, a cover, an unknown chunk and unknown keys; then saves
//!     settings through a slot update. Writes <out>.expect.json describing exactly what
//!     a reader must find (tests/conformance/read_rust_song.py checks it).
//!
//! cargo run -p jk-format --example jksong-write-test -- --update <file.jksong>
//!     Slot update on an existing file (for example one the Python writer made):
//!     semitones +4, lyrics_offset_ms 250, a new unknown key `x_rust_touch`. Prints the
//!     slot and sequence used.
//!
//! cargo run -p jk-format --example jksong-write-test -- --rewrite <file.jksong>
//!     Full atomic rewrite (`rewrite_song`) with a changed title and an added manifest
//!     key; all other chunks, unknown ones included, are copied unchanged.
//! ```
//!
//! The stem generator is integer-only so Python can regenerate it bit for bit:
//! `tri(i, period, amp) = (v * 2 * amp) / half - amp` with `half = period / 2`,
//! `p = i % period`, `v = p if p < half else period - p` (all non-negative integer
//! division), and for stem k, channel c: `period = 100 + 37k + 11c`,
//! `amp = 2^20 + 12345k + 999c`.

#[path = "support/flac_verbatim.rs"]
mod flac_verbatim;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jk_format::*;
use serde_json::{Value, json};

const RATE: u32 = 48_000;
const FRAMES: u64 = 120_001; // 2.5 s + 1 frame: not a multiple of the 4096 block
const ROLES: [&str; 4] = ["lead_vocal", "backing_vocals", "drums", "other"];
const GAINS: [f64; 4] = [1.0, 1.0, 1.0, 1.5];

fn tri(i: i64, period: i64, amp: i64) -> i64 {
    let p = i % period;
    let half = period / 2;
    let v = if p < half { p } else { period - p };
    (v * 2 * amp) / half - amp
}

fn stem(k: usize) -> Vec<i32> {
    let mut v = Vec::with_capacity(FRAMES as usize * 2);
    for i in 0..FRAMES as i64 {
        for c in 0..2i64 {
            let period = 100 + 37 * k as i64 + 11 * c;
            let amp = (1 << 20) + 12345 * k as i64 + 999 * c;
            v.push(tri(i, period, amp) as i32);
        }
    }
    v
}

/// 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];
const UNKNOWN: &[u8] = b"rust-written opaque chunk \x00\x01\x02\xfe\xff";

/// Graphemes per word: precomposed "caf\u{e9}", combining "nai\u{308}ve", a ZWJ emoji
/// sequence as one cluster, and a word with trailing punctuation.
fn words() -> Vec<Vec<&'static str>> {
    vec![
        vec!["c", "a", "f", "\u{e9}"],
        vec!["n", "a", "i\u{308}", "v", "e"],
        vec!["\u{1F469}\u{200D}\u{1F3A4}"],
        vec!["s", "i", "n", "g", "s", "!"],
    ]
}

fn lyrics() -> Value {
    // Line 0: words 0-1, line 1: words 2-3. 60 ms per grapheme, 100 ms between words,
    // a held final grapheme on the last word.
    let mut t = 0.2f64;
    let mut lines = Vec::new();
    for group in [&words()[0..2], &words()[2..4]] {
        let start = t;
        let mut ws = Vec::new();
        for (wi, w) in group.iter().enumerate() {
            let ws_start = t;
            let mut chars = Vec::new();
            for (ci, g) in w.iter().enumerate() {
                let held = wi == group.len() - 1 && ci == w.len() - 1;
                let d = if held { 0.4 } else { 0.06 };
                let (a, b) = (round_ms(t), round_ms(t + d));
                chars.push(json!([g, a, b]));
                t += d;
            }
            ws.push(json!({"start": round_ms(ws_start), "end": round_ms(t), "confidence": 0.5, "chars": chars}));
            t += 0.1;
        }
        lines.push(json!({"start": round_ms(start), "end": round_ms(t - 0.1), "words": ws, "x_line_note": "rust"}));
        t += 0.3;
    }
    json!({"lyrics_version": 1, "language": "en", "source": "user_text", "checked": true,
           "alignment": {"method": "synthetic", "model": null, "anchored_lines": false},
           "lines": lines, "x_lyrics_note": "from rust"})
}

fn round_ms(t: f64) -> f64 {
    (t * 1000.0).round() / 1000.0
}

fn write_new(out: &Path) -> Result<()> {
    let stems: Vec<Value> = ROLES
        .iter()
        .zip(GAINS)
        .map(|(r, g)| {
            json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24, "gain": g, "peak": 0.25 * g,
                   "in_karaoke_mix": *r != "lead_vocal", "pitched": *r != "drums", "x_stem_note": r})
        })
        .collect();
    let manifest: Manifest = serde_json::from_value(json!({
        "format": "jksong", "format_version": FORMAT_VERSION,
        "title": "Rust-written tones", "artist": "jk-format", "album": "Conformance",
        "sample_rate": RATE, "channels": 2, "duration_frames": FRAMES,
        "duration_seconds": FRAMES as f64 / RATE as f64,
        "cover": "cover", "lyrics": "lyrics",
        "analysis": {"key": {"tonic": "F#", "mode": "minor", "name": "F# minor", "confidence": 0.42}, "tempo_bpm": 96.5},
        "preset": "full", "stems_sum_to_source": false, "stems": stems,
        "provenance": {"app": {"name": "jk-format example", "version": env!("CARGO_PKG_VERSION")},
                       "created_utc": jk_format::time::utc_now_iso8601(), "resampling": [], "models": []},
        "x_manifest_note": {"written_by": "rust"}
    }))
    .map_err(|e| FormatError::InvalidInput(e.to_string()))?;
    let mut w = SongWriter::new(manifest);
    w.add_json_chunk("lyrics", &lyrics())?;
    w.add_chunk("cover", "image/png", ChunkData::Bytes(PNG.to_vec()))?;
    for (k, r) in ROLES.iter().enumerate() {
        let flac = flac_verbatim::encode(&stem(k), 2, RATE, RATE as u64);
        w.add_chunk(format!("stem.{r}"), "audio/flac", ChunkData::Bytes(flac))?;
    }
    w.add_chunk("x.rust-extra", "application/octet-stream", ChunkData::Bytes(UNKNOWN.to_vec()))?;
    w.write_atomic(out)?;

    // A settings save through the A/B slots (goes to slot B, sequence 2).
    let wr = update_user_state(out, |s| {
        s.settings.semitones = -3;
        s.settings.guide_vocal = 0.25;
        s.settings.levels.insert("drums".into(), 0.8);
        s.metadata_override.title = Some("Rust-corrected title".into());
        s.extra.insert("x_rust_key".into(), json!({"nested": ["\u{e9}", 1]}));
    })?;

    let expect = json!({
        "sample_rate": RATE, "channels": 2, "frames": FRAMES,
        "roles": ROLES, "gains": GAINS,
        "generator": "tri(i, 100 + 37k + 11c, 2^20 + 12345k + 999c)",
        "title": "Rust-corrected title", "manifest_title": "Rust-written tones", "artist": "jk-format", "album": "Conformance",
        "settings": {"semitones": -3, "guide_vocal": 0.25, "levels": {"drums": 0.8}},
        "slot": {"active": wr.slot, "sequence": wr.sequence},
        "user_state_keys": {"x_rust_key": {"nested": ["\u{e9}", 1]}},
        "lyrics_graphemes": words(),
        "lyrics_lines": ["caf\u{e9} nai\u{308}ve", "\u{1F469}\u{200D}\u{1F3A4} sings!"],
        "unknown_chunk": {"id": "x.rust-extra", "hex": hex(UNKNOWN)},
        "manifest_extra": {"x_manifest_note": {"written_by": "rust"}},
        "cover_hex": hex(PNG),
    });
    let mut p = out.as_os_str().to_owned();
    p.push(".expect.json");
    std::fs::write(PathBuf::from(p), serde_json::to_vec_pretty(&expect).unwrap())?;
    println!("wrote {} ({} bytes); settings saved to slot {} sequence {}", out.display(), std::fs::metadata(out)?.len(), wr.slot, wr.sequence);
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn update_existing(path: &Path) -> Result<()> {
    let before = SongFile::open(path)?;
    let unknown_before: Vec<String> = before.user_state().extra.keys().cloned().collect();
    let settings_unknown: Vec<String> = before.user_state().settings.extra.keys().cloned().collect();
    drop(before);
    let w = update_user_state(path, |s| {
        s.settings.semitones = 4;
        s.settings.lyrics_offset_ms = 250;
        s.extra.insert("x_rust_touch".into(), json!(true));
    })?;
    println!(
        "updated {}: slot {} sequence {}; unknown top-level keys kept: {:?}; unknown settings keys kept: {:?}",
        path.display(),
        w.slot,
        w.sequence,
        unknown_before,
        settings_unknown
    );
    Ok(())
}

/// Full rewrite of an existing file through `rewrite_song`: title changed, a manifest
/// key added; every other chunk (including unknown ones) must come through unchanged.
fn rewrite_existing(path: &Path) -> Result<()> {
    let mut m = SongFile::open(path)?.manifest().clone();
    m.title = format!("{} (rewritten by Rust)", m.title);
    m.extra.insert("x_rust_rewrite".into(), json!(true));
    let w = rewrite_song(path, Rewrite { manifest: Some(m), ..Default::default() })?;
    println!(
        "rewrote {}: {} chunks, {} bytes, slot size {}",
        path.display(),
        w.toc.chunks.len(),
        w.header.file_length,
        w.header.slot_size
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.as_slice() {
        [flag, file] if flag == "--update" => update_existing(Path::new(file)),
        [flag, file] if flag == "--rewrite" => rewrite_existing(Path::new(file)),
        [file] if !file.starts_with("--") => write_new(Path::new(file)),
        _ => {
            eprintln!("usage: jksong-write-test <out.jksong> | --update <file.jksong> | --rewrite <file.jksong>");
            return ExitCode::from(2);
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

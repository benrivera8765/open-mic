//! The validation checklist of SONG_FORMAT.md section 9 (`jukeoke-cli verify`).
//!
//! Magic, header CRC, file length, TOC CRC, every chunk CRC and bounds, manifest
//! schema, every referenced chunk present, every stem a valid FLAC stream with the
//! declared rate, channels, bit depth and length, lyrics times ordered and within the
//! song, each word's chars non-empty, slots valid or empty. Version 1.1 adds the guide
//! checks (`guide`, `guide.curve`, a stale guide, `--deep` source hash) and the slot's
//! `practice` value.
//!
//! Errors are violations of the specification. Warnings are things a conforming file
//! may contain but that deserve a look (a damaged inactive slot left by an interrupted
//! save, a missing seek table, a char that is not exactly one grapheme cluster).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::flac;
use crate::header::ranges_overlap;
use crate::lyrics::Lyrics;
use crate::manifest::TONICS;
use crate::slot::SlotState;
use crate::song::SongFile;
use crate::user_state::UserState;
use crate::{ALIGN, HEADER_LENGTH};

/// Maximum cover size (section 4.1: "at most 2 MB"; read as 2 MiB).
pub const MAX_COVER_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    /// Compute every chunk's CRC (streams through all stems).
    pub check_chunk_crcs: bool,
    /// Decode every stem completely (feature `symphonia`): frame count and MD5.
    pub decode_stems: bool,
    /// Number of random-access seeks per stem compared against the linear decode.
    pub seek_checks: usize,
    /// 1.1 `--deep`: also compare the guide's `provenance.source.sha256` with the SHA-256
    /// of the lead vocal chunk's bytes.
    pub deep: bool,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        VerifyOptions { check_chunk_crcs: true, decode_stems: true, seek_checks: 8, deep: false }
    }
}

/// Per-stem findings.
#[derive(Clone, Debug, Default)]
pub struct StemReport {
    pub role: String,
    pub chunk: String,
    pub bytes: u64,
    pub gain: f64,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
    pub md5_present: bool,
    /// Number of seek points, `None` without a SEEKTABLE.
    pub seek_points: Option<usize>,
    pub max_seek_gap_seconds: Option<f64>,
    /// Frames produced by a full decode.
    pub decoded_frames: Option<u64>,
    /// MD5 of the decoded audio matches STREAMINFO.
    pub md5_ok: Option<bool>,
    /// (seeks that landed on the right samples, seeks tried).
    pub seeks_ok: Option<(usize, usize)>,
    /// Frames compared with the linear decode over all seeks (one second after each
    /// seek target, less only where the stream ends sooner).
    pub seek_frames_compared: Option<u64>,
    /// Largest |sample| after applying the gain.
    pub decoded_peak: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub path: PathBuf,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub version: Option<(u16, u16)>,
    pub stems: Vec<StemReport>,
    pub lyrics_graphemes: Option<usize>,
    /// Description of each slot ("empty", "valid seq 3", "invalid: ...").
    pub slots: Vec<String>,
    /// 1.1: a one-line description of the melody guide ("none", or its counts and
    /// provenance), never the lyrics.
    pub guide: Option<String>,
}

impl VerifyReport {
    /// No specification violations.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Runs the whole checklist. Never panics on a bad file; I/O problems become errors in
/// the report.
pub fn verify_file(path: impl AsRef<Path>, opts: &VerifyOptions) -> VerifyReport {
    let path = path.as_ref();
    let mut r = VerifyReport { path: path.to_path_buf(), ..Default::default() };
    let song = match SongFile::open(path) {
        Ok(s) => s,
        Err(e) => {
            r.errors.push(format!("{e} [{:?}] - {}", e.kind(), e.user_message()));
            return r;
        }
    };
    let h = song.header().clone();
    r.version = Some((h.major, h.minor));
    for w in song.warnings() {
        r.warnings.push(w.clone());
    }

    // Header padding (bytes 128..4096 are zero).
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut b = vec![0u8; ALIGN as usize];
        if f.read_exact(&mut b).is_ok() && b[HEADER_LENGTH.max(h.header_length as usize)..].iter().any(|&x| x != 0) {
            r.warnings.push("header block bytes after the header are not zero".into());
        }
        if b[64..124].iter().any(|&x| x != 0) && h.header_length as usize == HEADER_LENGTH {
            r.warnings.push("reserved header bytes 64..124 are not zero (a newer minor version?)".into());
        }
    }
    if h.slot_a_offset != ALIGN || h.slot_b_offset != ALIGN + h.slot_size as u64 {
        r.errors.push("slot offsets are not 4096 and 4096 + slot_size".into());
    }
    let actual_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if actual_len > h.file_length {
        r.warnings.push(format!("{} bytes after the declared end of the file", actual_len - h.file_length));
    }

    // Chunk layout.
    let chunks = &song.toc().chunks;
    for (i, c) in chunks.iter().enumerate() {
        if c.offset % ALIGN != 0 {
            r.errors.push(format!("chunk '{}' does not start on a 4096-byte boundary", c.id));
        }
        for d in &chunks[i + 1..] {
            if ranges_overlap(c.offset, c.length, d.offset, d.length) {
                r.errors.push(format!("chunks '{}' and '{}' overlap", c.id, d.id));
            }
        }
        if c.offset >= h.toc_offset + h.toc_length {
            r.warnings.push(format!("chunk '{}' comes after the table of contents", c.id));
        }
    }
    if opts.check_chunk_crcs {
        match song.check_all_chunk_crcs() {
            Ok(bad) => {
                for id in bad {
                    r.errors.push(format!("chunk '{id}' CRC-32 mismatch"));
                }
            }
            Err(e) => r.errors.push(format!("reading chunks: {e}")),
        }
    }

    // Manifest schema, on the raw JSON for precise messages.
    match song.read_chunk(crate::MANIFEST_CHUNK) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => check_manifest_schema(&v, &song, &mut r),
            Err(e) => r.errors.push(format!("manifest JSON: {e}")),
        },
        Err(e) => r.errors.push(format!("manifest: {e}")),
    }
    if let Some(c) = song.chunk(crate::MANIFEST_CHUNK) {
        if c.mime != "application/json" {
            r.errors.push(format!("manifest chunk type is '{}', expected application/json", c.mime));
        }
    }

    let m = song.manifest().clone();
    let duration = m.duration();

    // Stems.
    for stem in &m.stems {
        let mut sr = StemReport { role: stem.role.clone(), chunk: stem.chunk.clone(), gain: stem.gain, ..Default::default() };
        let Some(entry) = song.chunk(&stem.chunk) else { continue }; // reported by open
        sr.bytes = entry.length;
        if entry.mime != "audio/flac" {
            r.errors.push(format!("stem '{}': chunk type '{}', expected audio/flac", stem.role, entry.mime));
        }
        let meta = song.chunk_reader(&stem.chunk).map_err(|e| e.to_string()).and_then(|rd| flac::read_metadata(rd).map_err(|e| e.to_string()));
        match meta {
            Ok(fm) => {
                let si = &fm.stream_info;
                sr.sample_rate = si.sample_rate;
                sr.channels = si.channels;
                sr.bits_per_sample = si.bits_per_sample;
                sr.total_samples = si.total_samples;
                sr.md5_present = si.md5 != [0u8; 16];
                sr.seek_points = fm.seek_points.as_ref().map(|p| p.len());
                sr.max_seek_gap_seconds = fm.max_seek_gap().map(|g| g as f64 / si.sample_rate.max(1) as f64);
                let who = format!("stem '{}'", stem.role);
                if si.sample_rate != m.sample_rate {
                    r.errors.push(format!("{who}: FLAC sample rate {} != manifest {}", si.sample_rate, m.sample_rate));
                }
                if si.channels as u16 != m.channels {
                    r.errors.push(format!("{who}: FLAC channels {} != manifest {}", si.channels, m.channels));
                }
                if si.bits_per_sample as u32 != stem.bits || si.bits_per_sample != 24 {
                    r.errors.push(format!("{who}: FLAC bits per sample {} (manifest says {}, v1.0 requires 24)", si.bits_per_sample, stem.bits));
                }
                if si.total_samples != m.duration_frames {
                    r.errors.push(format!("{who}: STREAMINFO total samples {} != duration_frames {}", si.total_samples, m.duration_frames));
                }
                if !sr.md5_present {
                    r.errors.push(format!("{who}: STREAMINFO has no MD5"));
                }
                match sr.max_seek_gap_seconds {
                    None => r.warnings.push(format!("{who}: no SEEKTABLE (recommended: a point at least every 10 s)")),
                    Some(g) if g > 10.0 + 1e-9 => r.warnings.push(format!("{who}: seek points up to {g:.1} s apart (recommended <= 10 s)")),
                    _ => {}
                }
            }
            Err(e) => r.errors.push(format!("stem '{}' is not a valid FLAC stream: {e}", stem.role)),
        }
        #[cfg(feature = "symphonia")]
        if opts.decode_stems {
            decode_check(&song, stem, m.duration_frames, opts.seek_checks, &mut sr, &mut r);
        }
        r.stems.push(sr);
    }

    // Lyrics chunk.
    if m.lyrics.is_some() {
        match song.lyrics() {
            Ok(Some(l)) => check_lyrics(&l, duration, "lyrics", &mut r),
            Ok(None) => {}
            Err(e) => r.errors.push(format!("lyrics: {e}")),
        }
    }

    // Melody guide (1.1, section 9).
    check_guide(&song, duration, opts, &mut r);

    // Slots.
    for (i, s) in [(0usize, &song.slots().a), (1, &song.slots().b)] {
        let name = if i == 0 { "slot A" } else { "slot B" };
        match s {
            SlotState::Empty => r.slots.push(format!("{name}: empty")),
            SlotState::Invalid { reason } => {
                r.slots.push(format!("{name}: invalid ({reason})"));
                // Already in song.warnings(): an interrupted save is recoverable by design.
            }
            SlotState::Valid { sequence, encoding, json } => {
                let active = song.slots().active == Some(i);
                r.slots.push(format!(
                    "{name}: valid, sequence {sequence}, {} bytes {}{}",
                    json.len(),
                    if *encoding == 1 { "zlib" } else { "json" },
                    if active { ", active" } else { "" }
                ));
                match UserState::from_json(json) {
                    Ok(st) => {
                        let sem = st.settings.semitones;
                        if !(crate::user_state::MIN_SEMITONES..=crate::user_state::MAX_SEMITONES).contains(&sem) {
                            r.errors.push(format!("{name}: semitones {sem} outside -12..12"));
                        }
                        if let Some(lo) = &st.lyrics_override {
                            check_lyrics(lo, duration, &format!("{name} lyrics_override"), &mut r);
                        }
                        // 1.1: the practice data, when the active slot holds it.
                        if active && let Some(p) = st.extra.get(crate::practice::PRACTICE_KEY) {
                            for e in crate::practice::validate_practice(p) {
                                r.errors.push(format!("{name} {e}"));
                            }
                        }
                    }
                    Err(e) => r.errors.push(format!("{name}: payload is not a JSON object: {e}")),
                }
            }
        }
    }
    r
}

/// The guide checks of section 9 (1.1). An absent guide is not a warning.
fn check_guide(song: &SongFile, duration: f64, opts: &VerifyOptions, r: &mut VerifyReport) {
    use crate::guide::{CURVE_CHUNK, Curve, CurveError, GUIDE_CHUNK, GUIDE_MIME, GUIDE_VERSION, GuideDoc};
    let Some(entry) = song.chunk(GUIDE_CHUNK) else {
        if song.chunk(CURVE_CHUNK).is_some() {
            r.warnings.push("guide.curve present without a guide chunk".into());
        }
        r.guide = Some("none".into());
        return;
    };
    if entry.mime != GUIDE_MIME {
        r.errors.push(format!("guide: chunk type '{}', expected application/json", entry.mime));
    }
    let bytes = match song.read_chunk(GUIDE_CHUNK) {
        Ok(b) => b,
        Err(e) => {
            r.errors.push(format!("guide: {e}"));
            return;
        }
    };
    let v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            r.errors.push(format!("guide: not valid JSON: {e}"));
            return;
        }
    };
    match v.get("guide_version").and_then(Value::as_u64) {
        Some(g) if g > GUIDE_VERSION as u64 => {
            r.warnings.push(format!("guide: guide_version {g} was made by a newer version (not checked further)"));
            r.guide = Some(format!("guide_version {g} (newer)"));
            return;
        }
        Some(1) => {}
        _ => {
            r.errors.push("guide: guide_version must be 1".into());
            return;
        }
    }
    // A word reference of the wrong shape is read as "no word" (a cache entry, section
    // 10.4); say so, as the Python checker does.
    if let Some(notes) = v.get("notes").and_then(Value::as_array) {
        for (i, n) in notes.iter().enumerate() {
            if n.get("word").is_some_and(crate::guide::malformed_word_ref) {
                r.warnings.push(format!("guide: note {i}: word reference is not [line, word, word_start] (ignored)"));
            }
        }
    }
    let doc: GuideDoc = match serde_json::from_value(v) {
        Ok(d) => d,
        Err(e) => {
            r.errors.push(format!("guide: does not follow section 10.1: {e}"));
            return;
        }
    };
    let f = crate::guide::validate_doc(&doc, duration);
    r.errors.extend(f.errors);
    r.warnings.extend(f.warnings);
    // The curve: an error when it is named but missing or damaged.
    let mut curve_desc = "no curve".to_string();
    if let Some(cid) = &doc.curve {
        match song.chunk(cid) {
            None => r.errors.push(format!("guide: names the curve '{cid}', which is missing")),
            Some(c) => match song.read_chunk(cid) {
                Err(e) => r.errors.push(format!("guide.curve: {e}")),
                Ok(b) => match Curve::decode(&b) {
                    Ok(curve) => {
                        if c.mime != crate::guide::CURVE_MIME {
                            r.warnings.push(format!("guide.curve: chunk type '{}', expected application/octet-stream", c.mime));
                        }
                        let f = crate::guide::validate_curve(&curve, duration);
                        r.errors.extend(f.errors);
                        r.warnings.extend(f.warnings);
                        curve_desc = format!("curve {} frames", curve.frames());
                    }
                    Err(CurveError::NewerVersion(v)) => {
                        r.warnings.push(format!("guide.curve: version {v} is newer (treated as absent)"));
                    }
                    Err(e) => r.errors.push(format!("guide.curve: {e}")),
                },
            },
        }
    }
    // Stale: the lead vocal changed after the guide was made.
    if let Some(reason) = crate::guide::stale_reason(song, &doc) {
        r.warnings.push(format!("guide: stale ({reason})"));
    }
    if opts.deep
        && let (Some(src), Some(lead)) = (&doc.provenance.source, crate::guide::lead_entry(song))
    {
        match crate::guide_write::sha256_of_range(song.path(), lead.offset, lead.length) {
            Ok(h) if h.eq_ignore_ascii_case(&src.sha256) => {}
            Ok(h) => r.warnings.push(format!(
                "guide: stale (--deep: the lead vocal's SHA-256 is {h}, the guide was made from {})",
                if src.sha256.is_empty() { "an unrecorded hash" } else { src.sha256.as_str() }
            )),
            Err(e) => r.errors.push(format!("guide: --deep: reading the lead vocal: {e}")),
        }
    }
    let c = doc.compute_counts();
    r.guide = Some(format!(
        "{} {}, {} notes ({} pitched, {} unpitched, {} low confidence, {} octave flags, {} graded), {} spans, tuning {:+.1} c (strength {:.2}, used {:+.1} c), {curve_desc}",
        doc.provenance.method,
        doc.provenance.version,
        c.notes,
        c.pitched,
        c.unpitched,
        c.low_confidence,
        c.octave_flags,
        c.graded,
        doc.spans.len(),
        doc.tuning.cents,
        doc.tuning.strength,
        doc.tuning.use_cents
    ));
}

fn check_lyrics(l: &Lyrics, duration: f64, where_: &str, r: &mut VerifyReport) {
    r.errors.extend(l.validate(duration, where_));
    r.warnings.extend(l.lint(where_));
    if where_ == "lyrics" {
        r.lyrics_graphemes = Some(l.grapheme_count());
    }
    if l.lyrics_version > 1 {
        r.warnings.push(format!("{where_}: lyrics_version {} is newer than 1", l.lyrics_version));
    }
}

fn check_manifest_schema(v: &Value, song: &SongFile, r: &mut VerifyReport) {
    let mut err = |m: String| r.errors.push(format!("manifest: {m}"));
    let Some(o) = v.as_object() else {
        err("not a JSON object".into());
        return;
    };
    let is_str = |k: &str| o.get(k).is_some_and(Value::is_string);
    let is_str_or_null = |k: &str| o.get(k).is_some_and(|x| x.is_string() || x.is_null());
    if o.get("format").and_then(Value::as_str) != Some("jksong") {
        err("format must be \"jksong\"".into());
    }
    match o.get("format_version").and_then(Value::as_str) {
        Some(s) if s.split('.').count() == 2 && s.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) => {}
        _ => err("format_version must be a string like \"1.0\"".into()),
    }
    for k in ["title", "artist"] {
        if !is_str(k) {
            err(format!("{k} must be a string"));
        }
    }
    for k in ["album", "cover", "lyrics"] {
        if !is_str_or_null(k) {
            err(format!("{k} must be a string or null"));
        }
    }
    let rate = o.get("sample_rate").and_then(Value::as_u64).unwrap_or(0);
    if rate == 0 || rate > 1_536_000 {
        err("sample_rate must be a positive integer".into());
    }
    if !matches!(o.get("channels").and_then(Value::as_u64), Some(1 | 2)) {
        err("channels must be 1 or 2".into());
    }
    let frames = o.get("duration_frames").and_then(Value::as_u64);
    match frames {
        None => err("duration_frames must be a non-negative integer".into()),
        Some(0) => r.warnings.push("manifest: duration_frames is 0".into()),
        _ => {}
    }
    match (o.get("duration_seconds").and_then(Value::as_f64), frames) {
        (Some(s), Some(f)) if rate > 0 => {
            if (s - f as f64 / rate as f64).abs() > 1e-3 {
                r.warnings.push(format!("manifest: duration_seconds {s} disagrees with duration_frames / sample_rate"));
            }
        }
        (None, _) => r.warnings.push("manifest: duration_seconds missing or not a number".into()),
        _ => {}
    }
    let mut err = |m: String| r.errors.push(format!("manifest: {m}"));
    if let Some(cid) = o.get("cover").and_then(Value::as_str) {
        if let Some(c) = song.chunk(cid) {
            if c.mime != "image/jpeg" && c.mime != "image/png" {
                err(format!("cover chunk type '{}' is not image/jpeg or image/png", c.mime));
            }
            if c.length > MAX_COVER_BYTES {
                err(format!("cover is {} bytes, more than 2 MB", c.length));
            }
            if let Ok(b) = song.read_chunk(cid) {
                let png = b.starts_with(b"\x89PNG\r\n\x1a\n");
                let jpg = b.starts_with(&[0xFF, 0xD8, 0xFF]);
                if (c.mime == "image/png" && !png) || (c.mime == "image/jpeg" && !jpg) {
                    err(format!("cover bytes do not match its type '{}'", c.mime));
                }
            }
        }
    }
    if let Some(lid) = o.get("lyrics").and_then(Value::as_str) {
        if let Some(c) = song.chunk(lid) {
            if c.mime != "application/json" {
                err(format!("lyrics chunk type '{}' is not application/json", c.mime));
            }
        }
    }
    match o.get("analysis") {
        Some(Value::Object(a)) => {
            match a.get("key") {
                None | Some(Value::Null) => {}
                Some(Value::Object(k)) => {
                    if !k.get("tonic").and_then(Value::as_str).is_some_and(|t| TONICS.contains(&t)) {
                        err("analysis.key.tonic must be one of C C# D D# E F F# G G# A A# B".into());
                    }
                    if !matches!(k.get("mode").and_then(Value::as_str), Some("major" | "minor")) {
                        err("analysis.key.mode must be major or minor".into());
                    }
                    if let Some(c) = k.get("confidence") {
                        if !c.as_f64().is_some_and(|c| (0.0..=1.0).contains(&c)) {
                            err("analysis.key.confidence must be a number 0..1".into());
                        }
                    }
                }
                Some(_) => err("analysis.key must be an object or null".into()),
            }
            match a.get("tempo_bpm") {
                None | Some(Value::Null) => {}
                Some(t) if t.as_f64().is_some_and(|t| t > 0.0 && t.is_finite()) => {}
                Some(_) => err("analysis.tempo_bpm must be a positive number or null".into()),
            }
        }
        None | Some(Value::Null) => r.warnings.push("manifest: no analysis".into()),
        Some(_) => r.errors.push("manifest: analysis must be an object".into()),
    }
    let mut err = |m: String| r.errors.push(format!("manifest: {m}"));
    match o.get("preset") {
        // The Studio's presets (studio/jukeoke_studio/presets.py, SONG_FORMAT 4.1).
        // "permissive" (MIT / Apache-2.0 models only) was added by the Studio pipeline
        // stream; without it every permissive song failed `verify --strict` on this
        // warning (found in the end-to-end integration).
        Some(Value::String(p)) if matches!(p.as_str(), "full" | "clean" | "permissive") => {}
        Some(Value::String(p)) => r.warnings.push(format!("manifest: unknown preset '{p}'")),
        _ => err("preset must be a string".into()),
    }
    let mut err = |m: String| r.errors.push(format!("manifest: {m}"));
    if !o.get("stems_sum_to_source").is_some_and(Value::is_boolean) {
        err("stems_sum_to_source must be true or false".into());
    }
    match o.get("stems").and_then(Value::as_array) {
        Some(stems) if !stems.is_empty() => {
            let mut roles = std::collections::HashSet::new();
            let mut chunks = std::collections::HashSet::new();
            for (i, s) in stems.iter().enumerate() {
                let Some(s) = s.as_object() else {
                    err(format!("stems[{i}] is not an object"));
                    continue;
                };
                let role = s.get("role").and_then(Value::as_str);
                match role {
                    Some(role) => {
                        if !roles.insert(role.to_string()) {
                            err(format!("stem role '{role}' appears twice"));
                        }
                    }
                    None => err(format!("stems[{i}].role must be a string")),
                }
                match s.get("chunk").and_then(Value::as_str) {
                    Some(c) => {
                        if !chunks.insert(c.to_string()) {
                            err(format!("stem chunk '{c}' is used twice"));
                        }
                    }
                    None => err(format!("stems[{i}].chunk must be a string")),
                }
                if s.get("codec").and_then(Value::as_str) != Some("flac") {
                    err(format!("stems[{i}].codec must be \"flac\""));
                }
                if s.get("bits").and_then(Value::as_u64) != Some(24) {
                    err(format!("stems[{i}].bits must be 24"));
                }
                if !s.get("gain").and_then(Value::as_f64).is_some_and(|g| g.is_finite() && g > 0.0) {
                    err(format!("stems[{i}].gain must be a positive number"));
                }
                if let Some(p) = s.get("peak") {
                    if !p.as_f64().is_some_and(|p| p.is_finite() && p >= 0.0) {
                        err(format!("stems[{i}].peak must be a non-negative number"));
                    }
                }
                for k in ["in_karaoke_mix", "pitched"] {
                    if !s.get(k).is_some_and(Value::is_boolean) {
                        err(format!("stems[{i}].{k} must be true or false"));
                    }
                }
            }
        }
        _ => err("stems must be a non-empty array".into()),
    }
    if !o.get("provenance").is_some_and(Value::is_object) {
        r.warnings.push("manifest: provenance missing".into());
    }
}

/// Full decode of one stem: frame count, MD5, and random-access seeks compared with
/// the linear decode (proves seeking is sample-accurate with this file's seek table).
///
/// Each seek is compared over one second of audio, many FLAC frames: a decoder can get
/// the first frame after a seek right and the later ones wrong (symphonia-bundle-flac
/// 0.6.1 did, see `decode::StemDecoder::seek`), so a few frames prove nothing. The seeks
/// reuse the decoder that has just read the whole stream and alternate between the two
/// ends of the stream, so each one starts from a different decoder state (the first goes
/// back to frame 0 from the end).
#[cfg(feature = "symphonia")]
fn decode_check(
    song: &SongFile,
    stem: &crate::manifest::StemInfo,
    expected_frames: u64,
    seeks: usize,
    sr: &mut StemReport,
    r: &mut VerifyReport,
) {
    use crate::decode::StemDecoder;
    let who = format!("stem '{}'", stem.role);
    let open = || song.chunk_reader(&stem.chunk).and_then(|rd| StemDecoder::new(rd, true));
    let mut dec = match open() {
        Ok(d) => d,
        Err(e) => {
            r.errors.push(format!("{who}: decoder: {e}"));
            return;
        }
    };
    let ch = dec.channels;
    // Frames compared at each seek target: one second.
    let probe = (dec.sample_rate as usize).max(1);
    // Seek targets spread over the stream, at odd offsets so they fall inside blocks.
    // The last target leaves a second to compare, or half the stream when it is shorter
    // than two seconds (otherwise every target of a stream up to 1 s long collapses to
    // frame 0 and only one seek is checked); targets near the end compare to the end.
    let span = expected_frames.saturating_sub((probe as u64).min(expected_frames / 2));
    let mut targets: Vec<u64> = (0..seeks as u64)
        .map(|i| if seeks == 1 { 0 } else { span * i / (seeks as u64 - 1) })
        .map(|t| if t > 0 && t % 2 == 0 { t - 1 } else { t })
        .collect();
    targets.dedup();
    // Visit order: first, last, second, second to last, ... (backward and forward jumps).
    let order: Vec<usize> = (0..targets.len())
        .map(|k| if k % 2 == 0 { k / 2 } else { targets.len() - 1 - k / 2 })
        .collect();
    let mut captured: Vec<Vec<i32>> = vec![Vec::new(); targets.len()];
    let mut block = Vec::new();
    let mut frames = 0u64;
    let mut peak = 0i64;
    let mut expected_ts = 0u64;
    loop {
        match dec.next_block(&mut block) {
            Ok(Some((ts, n))) => {
                if ts != expected_ts {
                    r.errors.push(format!("{who}: packet at frame {ts}, expected {expected_ts}"));
                }
                expected_ts = ts + n as u64;
                for &s in &block {
                    peak = peak.max((s as i64).abs());
                }
                for (t, cap) in targets.iter().zip(captured.iter_mut()) {
                    let want = probe.min((expected_frames - t.min(&expected_frames)) as usize);
                    let have = cap.len() / ch;
                    if have >= want {
                        continue;
                    }
                    let next = t + have as u64; // next frame we need
                    if next >= ts && next < ts + n as u64 {
                        let from = (next - ts) as usize;
                        let take = (want - have).min(n - from);
                        cap.extend_from_slice(&block[from * ch..(from + take) * ch]);
                    }
                }
                frames += n as u64;
            }
            Ok(None) => break,
            Err(e) => {
                r.errors.push(format!("{who}: decode error after {frames} frames: {e}"));
                return;
            }
        }
    }
    sr.decoded_frames = Some(frames);
    sr.md5_ok = dec.finish();
    sr.decoded_peak = Some(peak as f64 / 2f64.powi(dec.bits_per_sample as i32 - 1) * stem.gain);
    if frames != expected_frames {
        r.errors.push(format!("{who}: decoded {frames} frames, manifest says {expected_frames}"));
    }
    if sr.md5_ok != Some(true) {
        r.errors.push(format!("{who}: decoded audio MD5 check {:?} (expected Some(true))", sr.md5_ok));
    }
    if dec.channels as u16 != song.manifest().channels || dec.sample_rate != song.manifest().sample_rate {
        r.errors.push(format!("{who}: decoder reports {} ch {} Hz", dec.channels, dec.sample_rate));
    }

    // Seeks, each from a different position, compared frame for frame.
    let mut ok = 0usize;
    let mut tried = 0usize;
    let mut compared = 0u64;
    for &k in &order {
        let (t, cap) = (&targets[k], &captured[k]);
        if cap.is_empty() {
            continue;
        }
        tried += 1;
        compared += (cap.len() / ch) as u64;
        let result = (|| -> crate::Result<Vec<i32>> {
            let landed = dec.seek(*t)?;
            if landed > *t {
                return Err(crate::FormatError::damaged(format!("seek to {t} landed after it, at {landed}")));
            }
            let mut got = Vec::new();
            while got.len() < cap.len() {
                let Some((ts, n)) = dec.next_block(&mut block)? else { break };
                let next = t + (got.len() / ch) as u64;
                if next >= ts && next < ts + n as u64 {
                    let from = (next - ts) as usize;
                    let take = (cap.len() / ch - got.len() / ch).min(n - from);
                    got.extend_from_slice(&block[from * ch..(from + take) * ch]);
                }
            }
            Ok(got)
        })();
        match result {
            Ok(got) if got == *cap => ok += 1,
            Ok(_) => r.errors.push(format!("{who}: seek to frame {t} returned different samples than a linear decode")),
            Err(e) => r.errors.push(format!("{who}: seek to frame {t} failed: {e}")),
        }
    }
    sr.seeks_ok = Some((ok, tried));
    sr.seek_frames_compared = Some(compared);
}

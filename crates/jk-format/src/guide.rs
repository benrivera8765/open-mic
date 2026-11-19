// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody guide (SONG_FORMAT.md section 10, version 1.1): the `guide` chunk (JSON)
//! and the `guide.curve` chunk (binary pitch curve).
//!
//! This module holds the data types, the curve codec, the validation of section 9,
//! the "which notes are graded" rule, the word-reference cache check of section 10.4 and
//! the guide's state for a song (none / ready / outdated / stale / newer / damaged).
//! Writing a guide into an existing file is [`crate::guide_write`] (section 7.1).
//!
//! The guide is found by its chunk id in the TOC, never through the manifest, so adding
//! one leaves the manifest byte for byte as it was.
//!
//! Number precision (section 10.1): times and pitches are written to 3 decimals
//! ([`round3`]); the builder rounds before encoding so the JSON stays small.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::lyrics::Lyrics;
use crate::song::SongFile;
use crate::toc::ChunkEntry;

/// Chunk id of the guide JSON.
pub const GUIDE_CHUNK: &str = "guide";
/// Chunk id of the binary pitch curve.
pub const CURVE_CHUNK: &str = "guide.curve";
/// Media type of the guide chunk.
pub const GUIDE_MIME: &str = "application/json";
/// Media type of the curve chunk.
pub const CURVE_MIME: &str = "application/octet-stream";
/// The `guide_version` this crate writes and fully understands.
pub const GUIDE_VERSION: u32 = 1;
/// Magic of the curve chunk.
pub const CURVE_MAGIC: [u8; 4] = *b"JKPC";
/// Curve version this crate writes and reads.
pub const CURVE_VERSION: u16 = 1;
/// Length of the curve's fixed header.
pub const CURVE_HEADER_LEN: usize = 32;
/// Format code of a 32-bit IEEE float field.
pub const CURVE_F32: u32 = 1;
/// Field tags defined in 1.1, in the order 1.1 writers write them.
pub const TAG_MIDI: [u8; 4] = *b"MIDI";
pub const TAG_VOIC: [u8; 4] = *b"VOIC";
pub const TAG_LVDB: [u8; 4] = *b"LVDB";
/// Notes may end this far past the end of the song (section 9).
pub const END_TOLERANCE_S: f64 = 0.05;
/// Notes may overlap by at most this much (section 9).
pub const OVERLAP_TOLERANCE_S: f64 = 0.001;
/// A stored word reference is used when its start agrees within this (section 10.4).
pub const WORD_START_TOLERANCE_S: f64 = 0.0015;
/// Default tuning strength below which `use_cents` is 0 (builder 0.1.0).
pub const DEFAULT_TUNING_MIN_STRENGTH: f64 = 0.3;

/// Rounds to 3 decimals, the precision of guide times and pitches (section 10.1).
/// Half away from zero, the same rule as the Python writer's `_round3`.
pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Rounds to `d` decimals (half away from zero).
pub fn round_to(x: f64, d: i32) -> f64 {
    let p = 10f64.powi(d);
    (x * p).round() / p
}

fn default_hop() -> f64 {
    0.01
}
fn default_low_confidence() -> f64 {
    0.5
}

/// The whole `guide` chunk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuideDoc {
    pub guide_version: u32,
    #[serde(default = "default_hop")]
    pub hop_s: f64,
    /// Notes whose confidence is below this are drawn dim and never graded.
    #[serde(default = "default_low_confidence")]
    pub low_confidence: f64,
    pub tuning: Tuning,
    /// Pitch range of the pitched notes (informational; `null` when there are none).
    #[serde(default)]
    pub range: Option<PitchRange>,
    /// Informational counts (readers recompute them when they need them exactly).
    #[serde(default)]
    pub counts: Option<Counts>,
    /// Chunk id of the pitch curve, normally `guide.curve`.
    #[serde(default)]
    pub curve: Option<String>,
    #[serde(default)]
    pub notes: Vec<GuideNote>,
    #[serde(default)]
    pub spans: Vec<Span>,
    /// What the word references were made against (`null`: no lyrics).
    #[serde(default)]
    pub words: Option<WordsRef>,
    pub provenance: GuideProvenance,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The recording's tuning (section 10.1 "Tuning").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tuning {
    /// Offset from A = 440 Hz in cents, -50..50.
    pub cents: f64,
    /// How consistently the notes agree on it, 0..1.
    pub strength: f64,
    /// The offset readers use: `cents` when `strength >= tuning_min_strength`, else 0.
    pub use_cents: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Lowest and highest pitched note (MIDI with fractions, unshifted).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PitchRange {
    pub low: f64,
    pub high: f64,
}

/// Informational counts (section 10.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub notes: usize,
    pub pitched: usize,
    pub unpitched: usize,
    pub low_confidence: usize,
    pub octave_flags: usize,
    /// Notes graded by the rule of section 10.1, before any exclusion by the singer.
    pub graded: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `word`: `[line, word, word_start]` (section 10.4), a JSON array.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WordRef(pub usize, pub usize, pub f64);

/// `word` as stored, or `None` when it does not have the `[line, word, word_start]` shape.
fn lenient_word<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<WordRef>, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).ok())
}

/// True when a note's raw `word` value is present but not `[line, word, word_start]`
/// (non-negative integers and a finite number); `verify` warns about these.
pub fn malformed_word_ref(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Array(a) => !(a.len() == 3 && a[0].is_u64() && a[1].is_u64() && a[2].as_f64().is_some_and(f64::is_finite)),
        _ => true,
    }
}

/// One note (section 10.1 "Notes").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuideNote {
    pub start: f64,
    pub end: f64,
    /// Pitch as measured (MIDI with fractions, tuning included).
    pub midi: f64,
    pub confidence: f64,
    /// `pitched` or `unpitched`; a reader treats any other value as `unpitched`. A missing
    /// `kind` reads as "" (unknown, so unpitched), as the Python reader does (verifier:
    /// Rust refused the whole guide where Python only warned).
    #[serde(default)]
    pub kind: String,
    /// `ok`, `maybe_high`, `maybe_low` or `unchecked`; any other value counts as flagged.
    /// A missing `octave` reads as "" (unknown, so flagged).
    #[serde(default)]
    pub octave: String,
    #[serde(default)]
    pub glide_in: f64,
    /// A reference that is not `[line, word, word_start]` reads as no word: the
    /// references are a cache (section 10.4), so one bad entry must not make the whole
    /// guide unreadable. `verify` warns about it.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "lenient_word")]
    pub word: Option<WordRef>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GuideNote {
    /// `kind == "pitched"` (an unknown kind is unpitched).
    pub fn is_pitched(&self) -> bool {
        self.kind == "pitched"
    }

    /// The octave check agreed or did not apply (an unknown value counts as flagged).
    pub fn octave_ok(&self) -> bool {
        matches!(self.octave.as_str(), "ok" | "unchecked")
    }

    /// The octave check flagged the note (`maybe_high`, `maybe_low` or unknown).
    pub fn octave_flagged(&self) -> bool {
        !self.octave_ok()
    }

    /// Section 10.1 "Which notes are graded", before exclusions: pitched, confident
    /// enough, octave not flagged.
    pub fn graded(&self, low_confidence: f64) -> bool {
        self.is_pitched() && self.confidence >= low_confidence && self.octave_ok()
    }

    /// Midpoint of the note (s), used for exclusions.
    pub fn midpoint(&self) -> f64 {
        0.5 * (self.start + self.end)
    }
}

/// A stretch drawn differently and never graded on pitch (`spoken`, `breath`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    pub kind: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What the stored word references were made against (section 10.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WordsRef {
    /// `chunk` (the `lyrics` chunk) or `override` (the slot's `lyrics_override`).
    pub lyrics: String,
    pub offset_ms: i64,
    pub word_count: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The stem a guide was made from (section 10.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuideSource {
    pub role: String,
    pub chunk: String,
    /// The chunk's length and CRC-32 as in the TOC when the guide was made.
    pub length: u64,
    pub crc32: u32,
    /// SHA-256 of the chunk's bytes as stored (lowercase hex).
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub channels: u16,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The app that made a guide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuideApp {
    pub name: String,
    pub version: String,
    /// `windows` or `macos`.
    pub platform: String,
    pub arch: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What made the guide (section 10.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuideProvenance {
    pub method: String,
    pub version: String,
    #[serde(default)]
    pub parameters: Map<String, Value>,
    #[serde(default)]
    pub source: Option<GuideSource>,
    #[serde(default)]
    pub analysis_rate_hz: Option<f64>,
    #[serde(default)]
    pub app: Option<GuideApp>,
    #[serde(default)]
    pub created_utc: Option<String>,
    /// Build time in seconds.
    #[serde(default)]
    pub seconds: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GuideProvenance {
    /// `parameters.tuning_min_strength`, or 0.3 when absent.
    pub fn tuning_min_strength(&self) -> f64 {
        self.parameters
            .get("tuning_min_strength")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_TUNING_MIN_STRENGTH)
    }
}

impl GuideDoc {
    /// Parses the guide chunk's bytes.
    pub fn from_json(bytes: &[u8]) -> Result<GuideDoc, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Compact JSON, as written into the file.
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a guide always serialises")
    }

    /// Recomputes `counts` from the notes (section 10.1).
    pub fn compute_counts(&self) -> Counts {
        let low = self.low_confidence;
        Counts {
            notes: self.notes.len(),
            pitched: self.notes.iter().filter(|n| n.is_pitched()).count(),
            unpitched: self.notes.iter().filter(|n| !n.is_pitched()).count(),
            // Pitched notes below `low_confidence` (the contract's example: 397 pitched - 64
            // low confidence = 333 graded; unpitched notes are never graded anyway).
            low_confidence: self.notes.iter().filter(|n| n.is_pitched() && n.confidence < low).count(),
            octave_flags: self.notes.iter().filter(|n| n.is_pitched() && n.octave_flagged()).count(),
            graded: self.notes.iter().filter(|n| n.graded(low)).count(),
            extra: Map::new(),
        }
    }

    /// Recomputes `range` over the pitched notes.
    pub fn compute_range(&self) -> Option<PitchRange> {
        let mut it = self.notes.iter().filter(|n| n.is_pitched() && n.midi > 0.0).map(|n| n.midi);
        let first = it.next()?;
        let (lo, hi) = it.fold((first, first), |(a, b), m| (a.min(m), b.max(m)));
        Some(PitchRange { low: round3(lo), high: round3(hi) })
    }

    /// The notes graded by the rule of section 10.1, minus those whose midpoint lies in
    /// one of `excluded` (start, end) spans.
    pub fn graded_notes<'a>(&'a self, excluded: &'a [(f64, f64)]) -> impl Iterator<Item = &'a GuideNote> + 'a {
        let low = self.low_confidence;
        self.notes.iter().filter(move |n| {
            n.graded(low) && !excluded.iter().any(|&(a, b)| (a..=b).contains(&n.midpoint()))
        })
    }

    /// Section 10.4: the stored word references may be used when they were made against
    /// lyrics with the same word count and the same offset, and every reference names an
    /// existing word whose start (plus the offset) equals `word_start` within 1.5 ms.
    pub fn word_refs_valid(&self, lyrics: Option<&Lyrics>, offset_ms: i64) -> bool {
        let Some(l) = lyrics else {
            // No lyrics: valid only when nothing refers to a word.
            return self.notes.iter().all(|n| n.word.is_none());
        };
        let Some(w) = &self.words else { return false };
        let count: usize = l.lines.iter().map(|x| x.words.len()).sum();
        if w.word_count != count || w.offset_ms != offset_ms {
            return false;
        }
        let off = offset_ms as f64 / 1000.0;
        self.notes.iter().all(|n| match n.word {
            None => true,
            Some(WordRef(li, wi, ws)) => l
                .lines
                .get(li)
                .and_then(|line| line.words.get(wi))
                .is_some_and(|word| (word.start + off - ws).abs() <= WORD_START_TOLERANCE_S),
        })
    }
}

// ------------------------------------------------------------------------- the curve

/// One field of the curve: a 4-byte tag, its format code and `N` values.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveField {
    pub tag: [u8; 4],
    /// `1` = 32-bit IEEE float. Fields with other codes are kept as raw 32-bit words
    /// (stored here through `f32::from_bits`) so a rewrite does not lose them.
    pub format: u32,
    pub values: Vec<f32>,
}

/// The `guide.curve` chunk (section 10.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Frame step in microseconds (10000).
    pub hop_us: u32,
    /// Song time of frame 0 in microseconds (0).
    pub t0_us: i32,
    pub fields: Vec<CurveField>,
}

/// Why a curve could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CurveError {
    /// Bad magic, field count 0, or a length that does not match the declared sizes.
    Damaged(String),
    /// A curve version this reader does not know (treated as absent).
    NewerVersion(u16),
}

impl std::fmt::Display for CurveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurveError::Damaged(s) => write!(f, "pitch curve damaged: {s}"),
            CurveError::NewerVersion(v) => write!(f, "pitch curve version {v} is newer than 1"),
        }
    }
}

impl Curve {
    /// A 1.1 curve with the three defined fields, in the defined order.
    pub fn new(hop_us: u32, midi: Vec<f32>, voic: Vec<f32>, lvdb: Vec<f32>) -> Curve {
        Curve {
            hop_us,
            t0_us: 0,
            fields: vec![
                CurveField { tag: TAG_MIDI, format: CURVE_F32, values: midi },
                CurveField { tag: TAG_VOIC, format: CURVE_F32, values: voic },
                CurveField { tag: TAG_LVDB, format: CURVE_F32, values: lvdb },
            ],
        }
    }

    /// Number of frames (`N`).
    pub fn frames(&self) -> usize {
        self.fields.first().map_or(0, |f| f.values.len())
    }

    /// Frame step in seconds.
    pub fn hop_s(&self) -> f64 {
        self.hop_us as f64 * 1e-6
    }

    /// The values of the float field with this tag.
    pub fn field(&self, tag: [u8; 4]) -> Option<&[f32]> {
        self.fields.iter().find(|f| f.tag == tag && f.format == CURVE_F32).map(|f| f.values.as_slice())
    }

    /// `MIDI`: pitch with fractions, 0 = unvoiced. A curve without it is treated as absent.
    pub fn midi(&self) -> Option<&[f32]> {
        self.field(TAG_MIDI)
    }

    /// The exact number of frames a song of `duration_s` has: `floor(duration / hop) + 1`.
    pub fn frames_for(duration_s: f64, hop_us: u32) -> usize {
        let hop = hop_us as f64 * 1e-6;
        // A tiny epsilon so that 40.0 s / 0.01 s is 4000, not 3999.9999.
        (duration_s / hop + 1e-9).floor() as usize + 1
    }

    /// Encodes the chunk bytes: header, field table, planar values.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.frames();
        let f = self.fields.len();
        let mut out = Vec::with_capacity(CURVE_HEADER_LEN + 8 * f + 4 * n * f);
        out.extend_from_slice(&CURVE_MAGIC);
        out.extend_from_slice(&CURVE_VERSION.to_le_bytes());
        out.extend_from_slice(&(f as u16).to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&self.hop_us.to_le_bytes());
        out.extend_from_slice(&self.t0_us.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        for fld in &self.fields {
            out.extend_from_slice(&fld.tag);
            out.extend_from_slice(&fld.format.to_le_bytes());
        }
        for fld in &self.fields {
            debug_assert_eq!(fld.values.len(), n, "every curve field has N values");
            for v in &fld.values {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Decodes the chunk bytes. The length must be exactly `32 + 8F + 4NF`.
    pub fn decode(b: &[u8]) -> Result<Curve, CurveError> {
        let dmg = |s: &str| CurveError::Damaged(s.to_string());
        if b.len() < CURVE_HEADER_LEN {
            return Err(dmg("shorter than its header"));
        }
        if b[0..4] != CURVE_MAGIC {
            return Err(dmg("bad magic"));
        }
        let version = u16::from_le_bytes([b[4], b[5]]);
        if version != CURVE_VERSION {
            return Err(CurveError::NewerVersion(version));
        }
        let f = u16::from_le_bytes([b[6], b[7]]) as usize;
        let n = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        let hop_us = u32::from_le_bytes(b[12..16].try_into().unwrap());
        let t0_us = i32::from_le_bytes(b[16..20].try_into().unwrap());
        if f == 0 {
            return Err(dmg("field count is 0"));
        }
        if hop_us == 0 {
            return Err(dmg("frame step is 0"));
        }
        let want = (CURVE_HEADER_LEN as u64) + 8 * f as u64 + 4 * n as u64 * f as u64;
        if b.len() as u64 != want {
            return Err(CurveError::Damaged(format!("length {} bytes, expected {want}", b.len())));
        }
        let mut fields = Vec::with_capacity(f);
        let table = &b[CURVE_HEADER_LEN..CURVE_HEADER_LEN + 8 * f];
        let data = &b[CURVE_HEADER_LEN + 8 * f..];
        for i in 0..f {
            let tag: [u8; 4] = table[8 * i..8 * i + 4].try_into().unwrap();
            let format = u32::from_le_bytes(table[8 * i + 4..8 * i + 8].try_into().unwrap());
            let vals = &data[4 * n * i..4 * n * (i + 1)];
            let values = vals.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            fields.push(CurveField { tag, format, values });
        }
        Ok(Curve { hop_us, t0_us, fields })
    }
}

// ------------------------------------------------------------------------- validation

/// Findings of the guide checks of section 9.
#[derive(Clone, Debug, Default)]
pub struct GuideFindings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn in01(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

/// Checks a guide document against section 9 (`duration_s`: the song's length).
///
/// Errors are violations a 1.1 writer must never produce; warnings are values a
/// conforming reader tolerates (an unknown `kind` / `octave` / span kind, counts that
/// disagree with the notes).
pub fn validate_doc(doc: &GuideDoc, duration_s: f64) -> GuideFindings {
    let mut r = GuideFindings::default();
    let e = &mut r.errors;
    if doc.guide_version != GUIDE_VERSION {
        e.push(format!("guide: guide_version {} (expected 1)", doc.guide_version));
        return r;
    }
    if !(doc.hop_s.is_finite() && doc.hop_s > 0.0) {
        e.push("guide: hop_s must be a positive number".into());
    }
    if !in01(doc.low_confidence) {
        e.push("guide: low_confidence must be within 0..1".into());
    }
    let tmax = duration_s + END_TOLERANCE_S;
    let mut prev: Option<&GuideNote> = None;
    for (i, n) in doc.notes.iter().enumerate() {
        let who = format!("guide: note {i}");
        if !(n.start.is_finite() && n.end.is_finite() && n.start >= 0.0 && n.start < n.end && n.end <= tmax) {
            e.push(format!("{who}: start {} / end {} not within 0 <= start < end <= song length + 0.05", n.start, n.end));
        }
        if let Some(p) = prev {
            if n.start < p.start {
                e.push(format!("{who}: notes are not ordered by start"));
            } else if p.end - n.start > OVERLAP_TOLERANCE_S + 1e-9 {
                e.push(format!("{who}: overlaps the previous note by {:.4} s (more than 1 ms)", p.end - n.start));
            }
        }
        if !in01(n.confidence) {
            e.push(format!("{who}: confidence {} not within 0..1", n.confidence));
        }
        let len = n.end - n.start;
        // Both are rounded to 1 ms independently, so allow one rounding step.
        if !(n.glide_in.is_finite() && n.glide_in >= 0.0 && n.glide_in <= len + 0.0015) {
            e.push(format!("{who}: glide_in {} not within 0..(end - start)", n.glide_in));
        }
        let midi_ok = n.midi.is_finite() && (12.0..=120.0).contains(&n.midi);
        match n.kind.as_str() {
            "pitched" => {
                if !midi_ok {
                    e.push(format!("{who}: midi {} not within 12..120", n.midi));
                }
            }
            "unpitched" => {
                if !(midi_ok || n.midi == 0.0) {
                    e.push(format!("{who}: unpitched midi {} is neither 0 nor within 12..120", n.midi));
                }
            }
            k => r.warnings.push(format!("{who}: unknown kind '{k}' (read as unpitched)")),
        }
        if !matches!(n.octave.as_str(), "ok" | "maybe_high" | "maybe_low" | "unchecked") {
            r.warnings.push(format!("{who}: unknown octave value '{}' (read as flagged)", n.octave));
        }
        prev = Some(n);
    }
    let e = &mut r.errors;
    let mut prev_start = f64::NEG_INFINITY;
    for (i, s) in doc.spans.iter().enumerate() {
        if !(s.start.is_finite() && s.end.is_finite() && s.start >= 0.0 && s.start <= s.end && s.end <= tmax) {
            e.push(format!("guide: span {i} ({} .. {}) is not inside the song", s.start, s.end));
        }
        if s.start < prev_start {
            e.push(format!("guide: span {i} is not ordered by start"));
        }
        prev_start = s.start;
        if !matches!(s.kind.as_str(), "spoken" | "breath") {
            r.warnings.push(format!("guide: span {i}: unknown kind '{}' (drawn as a gap)", s.kind));
        }
    }
    let e = &mut r.errors;
    let t = &doc.tuning;
    if !(t.cents.is_finite() && (-50.0..=50.0).contains(&t.cents)) {
        e.push(format!("guide: tuning.cents {} not within -50..50", t.cents));
    }
    if !in01(t.strength) {
        e.push(format!("guide: tuning.strength {} not within 0..1", t.strength));
    }
    if !(t.use_cents.is_finite() && (-50.0..=50.0).contains(&t.use_cents)) {
        e.push(format!("guide: tuning.use_cents {} not within -50..50", t.use_cents));
    }
    let min_strength = doc.provenance.tuning_min_strength();
    let want_use = if t.strength >= min_strength { t.cents } else { 0.0 };
    if (t.use_cents - want_use).abs() > 0.05 {
        r.warnings.push(format!(
            "guide: tuning.use_cents {} disagrees with the rule (cents {} when strength {} >= {min_strength}, else 0)",
            t.use_cents, t.cents, t.strength
        ));
    }
    let e = &mut r.errors;
    if doc.provenance.method.trim().is_empty() {
        e.push("guide: provenance.method missing".into());
    }
    if doc.provenance.version.trim().is_empty() {
        e.push("guide: provenance.version missing".into());
    }
    if let Some(c) = &doc.counts {
        let want = doc.compute_counts();
        if (c.notes, c.pitched, c.unpitched, c.low_confidence, c.octave_flags, c.graded)
            != (want.notes, want.pitched, want.unpitched, want.low_confidence, want.octave_flags, want.graded)
        {
            r.warnings.push("guide: counts disagree with the notes (informational only)".into());
        }
    }
    if let Some(w) = &doc.words {
        if !matches!(w.lyrics.as_str(), "chunk" | "override") {
            r.warnings.push(format!("guide: words.lyrics '{}' is neither chunk nor override", w.lyrics));
        }
    }
    r
}

/// Checks a decoded curve against section 9: every value finite, `MIDI` 0 or within
/// 12..120, `VOIC` within 0..1, and the frames cover the song.
pub fn validate_curve(c: &Curve, duration_s: f64) -> GuideFindings {
    let mut r = GuideFindings::default();
    if c.midi().is_none() {
        r.errors.push("guide.curve: no MIDI field (the curve is treated as absent)".into());
    }
    for f in &c.fields {
        if f.format != CURVE_F32 {
            r.warnings.push(format!(
                "guide.curve: field '{}' has unknown format code {} (skipped)",
                String::from_utf8_lossy(&f.tag),
                f.format
            ));
            continue;
        }
        let tag = String::from_utf8_lossy(&f.tag).into_owned();
        if let Some(i) = f.values.iter().position(|v| !v.is_finite()) {
            r.errors.push(format!("guide.curve: {tag} frame {i} is not a finite number"));
            continue;
        }
        match f.tag {
            TAG_MIDI => {
                if let Some(i) = f.values.iter().position(|&v| v != 0.0 && !(12.0..=120.0).contains(&v)) {
                    r.errors.push(format!("guide.curve: MIDI frame {i} = {} (must be 0 or within 12..120)", f.values[i]));
                }
            }
            TAG_VOIC => {
                if let Some(i) = f.values.iter().position(|&v| !(0.0..=1.0).contains(&v)) {
                    r.errors.push(format!("guide.curve: VOIC frame {i} = {} (must be within 0..1)", f.values[i]));
                }
            }
            _ => {}
        }
    }
    let n = c.frames();
    let hop = c.hop_s();
    let covered = (n.max(1) - 1) as f64 * hop + c.t0_us as f64 * 1e-6;
    if (covered - duration_s).abs() > hop + 1e-6 {
        r.errors.push(format!(
            "guide.curve: {n} frames of {:.0} ms cover {covered:.3} s, the song is {duration_s:.3} s",
            hop * 1000.0
        ));
    }
    r
}

// ------------------------------------------------------------------------- state

/// The guide's state in a file, compared with the app's own builder (section 10.2).
#[derive(Clone, Debug, PartialEq)]
pub enum GuideFileState {
    /// No `guide` chunk.
    None,
    /// Usable and made by this builder (or a newer version of it with `guide_version` 1:
    /// kept, never downgraded).
    Ready,
    /// Made by an older builder version or another method: used as it is, rebuilt on request.
    Outdated,
    /// The `lead_vocal` stem changed after the guide was made (or the guide cannot be
    /// read): not used, rebuilt like `None`. The string says why, in plain words.
    Stale(String),
    /// `guide_version` above 1: kept, never overwritten.
    Newer,
}

/// Compares dotted version strings numerically (`0.10.0` > `0.9.1`). Missing parts are 0;
/// a non-numeric part compares as 0.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect() };
    let (x, y) = (parse(a), parse(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        match p.cmp(&q) {
            std::cmp::Ordering::Equal => continue,
            o => return o,
        }
    }
    std::cmp::Ordering::Equal
}

/// The song's guide (when it can be read) and its state for a builder `method` /
/// `version`. Never fails: a guide that cannot be read is `Stale` with the reason. The
/// named curve is read and decoded too (see [`guide_state_opts`]).
pub fn guide_state(song: &SongFile, method: &str, version: &str) -> (GuideFileState, Option<GuideDoc>) {
    guide_state_opts(song, method, version, true)
}

/// [`guide_state`]; with `check_curve` false the curve is only looked up in the TOC, not
/// read (a library scan of many songs; the curve is decoded when practice reads it).
pub fn guide_state_opts(song: &SongFile, method: &str, version: &str, check_curve: bool) -> (GuideFileState, Option<GuideDoc>) {
    let Some(_) = song.chunk(GUIDE_CHUNK) else { return (GuideFileState::None, None) };
    let bytes = match song.read_chunk(GUIDE_CHUNK) {
        Ok(b) => b,
        Err(e) => {
            return (GuideFileState::Stale(format!("The practice data in this song is damaged ({e}).")), None);
        }
    };
    // A newer guide_version may have another shape: look at the number first.
    let v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return (GuideFileState::Stale("The practice data in this song cannot be read.".into()), None),
    };
    if v.get("guide_version").and_then(Value::as_u64).is_some_and(|g| g > GUIDE_VERSION as u64) {
        return (GuideFileState::Newer, None);
    }
    let doc: GuideDoc = match serde_json::from_value(v) {
        Ok(d) => d,
        Err(_) => return (GuideFileState::Stale("The practice data in this song cannot be read.".into()), None),
    };
    if let Some(reason) = stale_reason(song, &doc) {
        return (GuideFileState::Stale(reason), Some(doc));
    }
    // The named curve must be there and readable (section 9: an error otherwise).
    if let Some(cid) = &doc.curve
        && !check_curve
    {
        if song.chunk(cid).is_none() {
            return (GuideFileState::Stale("The practice data's pitch curve is missing.".into()), Some(doc));
        }
    } else if let Some(cid) = &doc.curve {
        match song.read_chunk(cid) {
            Ok(b) => match Curve::decode(&b) {
                Ok(_) | Err(CurveError::NewerVersion(_)) => {}
                Err(e) => return (GuideFileState::Stale(format!("The practice data's pitch curve is damaged ({e}).")), Some(doc)),
            },
            Err(_) => return (GuideFileState::Stale("The practice data's pitch curve is missing or damaged.".into()), Some(doc)),
        }
    }
    let p = &doc.provenance;
    let state = if p.method != method || compare_versions(&p.version, version) == std::cmp::Ordering::Less {
        GuideFileState::Outdated
    } else {
        GuideFileState::Ready
    };
    (state, Some(doc))
}

/// Why a guide no longer matches its song's lead vocal, if it does not (section 10.2:
/// `source.length` or `crc32` differs from the `lead_vocal` stem's TOC entry).
pub fn stale_reason(song: &SongFile, doc: &GuideDoc) -> Option<String> {
    let Some(lead) = lead_entry(song) else {
        return Some("This song has no separate lead vocal, so its practice data cannot be checked.".into());
    };
    match &doc.provenance.source {
        Some(s) if s.length == lead.length && s.crc32 == lead.crc32 => None,
        Some(_) => Some("The song's lead vocal changed after its practice data was made.".into()),
        None => Some("The practice data does not say which vocal it was made from.".into()),
    }
}

/// The TOC entry of the `lead_vocal` stem, if the song has one.
pub fn lead_entry(song: &SongFile) -> Option<&ChunkEntry> {
    let stem = song.manifest().stem("lead_vocal")?;
    song.chunk(&stem.chunk)
}

/// Reads the song's guide and curve (no state checks). `Ok(None)` when there is no guide.
pub fn read_guide(song: &SongFile) -> crate::Result<Option<(GuideDoc, Option<Curve>)>> {
    if song.chunk(GUIDE_CHUNK).is_none() {
        return Ok(None);
    }
    let bytes = song.read_chunk(GUIDE_CHUNK)?;
    let doc = GuideDoc::from_json(&bytes)
        .map_err(|e| crate::FormatError::Damaged { detail: format!("guide chunk is not valid: {e}") })?;
    let curve = match &doc.curve {
        Some(cid) if song.chunk(cid).is_some() => Curve::decode(&song.read_chunk(cid)?).ok(),
        _ => None,
    };
    Ok(Some((doc, curve)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, midi: f64) -> GuideNote {
        GuideNote {
            start,
            end,
            midi,
            confidence: 0.9,
            kind: "pitched".into(),
            octave: "ok".into(),
            glide_in: 0.0,
            word: None,
            extra: Map::new(),
        }
    }

    pub(crate) fn sample_doc() -> GuideDoc {
        let mut d = GuideDoc {
            guide_version: 1,
            hop_s: 0.01,
            low_confidence: 0.5,
            tuning: Tuning { cents: -15.8, strength: 0.91, use_cents: -15.8, extra: Map::new() },
            range: None,
            counts: None,
            curve: Some(CURVE_CHUNK.into()),
            notes: vec![note(1.0, 1.5, 64.273), note(1.5, 2.0, 66.118)],
            spans: vec![],
            words: None,
            provenance: GuideProvenance {
                method: "jk-pitch pyin+notes".into(),
                version: "0.1.0".into(),
                parameters: Map::new(),
                source: None,
                analysis_rate_hz: Some(16000.0),
                app: None,
                created_utc: None,
                seconds: None,
                extra: Map::new(),
            },
            extra: Map::new(),
        };
        d.notes[1].confidence = 0.4;
        d.notes[1].octave = "maybe_high".into();
        d.counts = Some(d.compute_counts());
        d.range = d.compute_range();
        d
    }

    #[test]
    fn json_round_trip_keeps_unknown_keys_and_word_arrays() {
        let mut d = sample_doc();
        d.notes[0].word = Some(WordRef(3, 0, 12.34));
        d.extra.insert("future".into(), Value::from(7));
        d.notes[0].extra.insert("x".into(), Value::from("y"));
        let j = d.to_json();
        let s = String::from_utf8(j.clone()).unwrap();
        assert!(s.contains("\"word\":[3,0,12.34]"), "{s}");
        assert!(s.starts_with("{\"guide_version\":1,"), "{s}");
        let back = GuideDoc::from_json(&j).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn graded_rule_and_counts() {
        let d = sample_doc();
        let c = d.compute_counts();
        assert_eq!((c.notes, c.pitched, c.low_confidence, c.octave_flags, c.graded), (2, 2, 1, 1, 1));
        assert_eq!(d.graded_notes(&[]).count(), 1);
        assert_eq!(d.graded_notes(&[(1.0, 1.3)]).count(), 0, "midpoint 1.25 is excluded");
        let mut u = note(0.0, 1.0, 0.0);
        u.kind = "shouted".into();
        assert!(!u.is_pitched() && !u.graded(0.5), "an unknown kind is unpitched");
        let mut o = note(0.0, 1.0, 60.0);
        o.octave = "maybe_sideways".into();
        assert!(o.octave_flagged() && !o.graded(0.5), "an unknown octave value is flagged");
    }

    #[test]
    fn validation_catches_each_rule() {
        let d = sample_doc();
        let f = validate_doc(&d, 3.0);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        let check = |mutate: &dyn Fn(&mut GuideDoc), needle: &str| {
            let mut x = sample_doc();
            mutate(&mut x);
            let f = validate_doc(&x, 3.0);
            assert!(f.errors.iter().any(|e| e.contains(needle)), "{needle}: {:?}", f.errors);
        };
        check(&|x| x.notes[1].start = 0.5, "not ordered");
        check(&|x| x.notes[1].start = 1.4, "overlaps");
        check(&|x| x.notes[1].end = 3.06, "start < end");
        check(&|x| x.notes[0].confidence = 1.2, "confidence");
        check(&|x| x.notes[0].glide_in = 0.7, "glide_in");
        check(&|x| x.notes[0].midi = 130.0, "midi");
        check(&|x| x.tuning.cents = 60.0, "tuning.cents");
        check(&|x| x.tuning.strength = 1.5, "tuning.strength");
        check(&|x| x.provenance.method.clear(), "provenance.method");
        // One millisecond of overlap is allowed (rounding).
        let mut x = sample_doc();
        x.notes[1].start = 1.499;
        assert!(validate_doc(&x, 3.0).errors.is_empty());
    }

    #[test]
    fn curve_round_trip_and_damage() {
        let n = Curve::frames_for(40.0, 10_000);
        assert_eq!(n, 4001);
        let c = Curve::new(10_000, vec![60.5; n], vec![0.9; n], vec![-20.0; n]);
        let b = c.encode();
        assert_eq!(b.len(), 32 + 24 + 12 * n);
        assert_eq!(Curve::decode(&b).unwrap(), c);
        assert!(validate_curve(&c, 40.0).errors.is_empty());
        assert!(!validate_curve(&c, 41.0).errors.is_empty(), "does not cover the song");
        assert!(matches!(Curve::decode(&b[..b.len() - 1]), Err(CurveError::Damaged(_))));
        let mut v2 = b.clone();
        v2[4] = 2;
        assert_eq!(Curve::decode(&v2), Err(CurveError::NewerVersion(2)));
        let mut bad = c.clone();
        bad.fields[0].values[5] = 5.0;
        bad.fields[1].values[6] = f32::NAN;
        let f = validate_curve(&bad, 40.0);
        assert_eq!(f.errors.len(), 2, "{:?}", f.errors);
    }

    #[test]
    fn missing_kind_or_octave_and_a_bad_word_reference_do_not_lose_the_guide() {
        // Verifier: these made the whole guide unreadable in Rust while Python only warned.
        let mut v = serde_json::to_value(sample_doc()).unwrap();
        let notes = v["notes"].as_array_mut().unwrap();
        notes[0].as_object_mut().unwrap().remove("octave");
        notes[0]["word"] = serde_json::json!([0, 1]);
        notes[1].as_object_mut().unwrap().remove("kind");
        let d: GuideDoc = serde_json::from_value(v).unwrap();
        assert!(d.notes[0].octave_flagged() && d.notes[0].word.is_none());
        assert!(!d.notes[1].is_pitched());
        let f = validate_doc(&d, 3.0);
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.iter().any(|w| w.contains("unknown kind")), "{:?}", f.warnings);
        assert!(f.warnings.iter().any(|w| w.contains("unknown octave")), "{:?}", f.warnings);
        assert!(malformed_word_ref(&serde_json::json!([0, 1])));
        assert!(malformed_word_ref(&serde_json::json!([-1, 0, 1.0])));
        assert!(malformed_word_ref(&serde_json::json!("x")));
        assert!(!malformed_word_ref(&serde_json::json!([3, 0, 12.34])));
        assert!(!malformed_word_ref(&Value::Null));
    }

    #[test]
    fn version_compare() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Equal);
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Less);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Greater);
        assert_eq!(compare_versions("1", "1.0.0"), Equal);
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(round3(12.3456), 12.346);
        assert_eq!(round3(-0.0005), -0.001);
        assert_eq!(round_to(87.45, 1), 87.5);
    }
}

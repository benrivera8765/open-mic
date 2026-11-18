//! The `manifest` chunk (SONG_FORMAT.md section 4).
//!
//! Every struct keeps the keys it does not know in `extra`, so a tool that rewrites a
//! file written by a newer version does not drop them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The stem roles defined in version 1.0. Newer files may carry others.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StemRole {
    LeadVocal,
    BackingVocals,
    Drums,
    Other,
    /// A role this version does not know (for example `bass` from a later version).
    Unknown(String),
}

impl StemRole {
    pub fn parse(s: &str) -> StemRole {
        match s {
            "lead_vocal" => StemRole::LeadVocal,
            "backing_vocals" => StemRole::BackingVocals,
            "drums" => StemRole::Drums,
            "other" => StemRole::Other,
            x => StemRole::Unknown(x.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            StemRole::LeadVocal => "lead_vocal",
            StemRole::BackingVocals => "backing_vocals",
            StemRole::Drums => "drums",
            StemRole::Other => "other",
            StemRole::Unknown(s) => s,
        }
    }

    /// Default `in_karaoke_mix` for a role (the lead vocal is the only one left out).
    pub fn default_in_karaoke_mix(&self) -> bool {
        !matches!(self, StemRole::LeadVocal)
    }

    /// Default `pitched` for a role (drums are not pitch-shifted).
    pub fn default_pitched(&self) -> bool {
        !matches!(self, StemRole::Drums)
    }
}

/// One stem entry of the manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StemInfo {
    pub role: String,
    /// Chunk id of the FLAC stream.
    pub chunk: String,
    pub codec: String,
    pub bits: u32,
    /// The true float sample is `decoded_integer / 8388608 * gain`.
    #[serde(default = "one")]
    pub gain: f64,
    /// Float peak before scaling (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f64>,
    /// Whether the stem is in the default karaoke mix. Missing: the role's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_karaoke_mix: Option<bool>,
    /// Whether the stem goes through the key change by default. Missing: the role's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitched: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> f64 {
    1.0
}

impl StemInfo {
    pub fn role(&self) -> StemRole {
        StemRole::parse(&self.role)
    }

    /// `in_karaoke_mix`, falling back to the role's default when the key is missing.
    /// For an unknown role the forward-compatibility rule (section 4.2) applies: play it
    /// when `in_karaoke_mix` is true.
    pub fn in_karaoke_mix(&self) -> bool {
        self.in_karaoke_mix.unwrap_or_else(|| self.role().default_in_karaoke_mix())
    }

    /// `pitched`, falling back to the role's default when the key is missing.
    pub fn pitched(&self) -> bool {
        self.pitched.unwrap_or_else(|| self.role().default_pitched())
    }
}

/// Detected musical key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyInfo {
    /// One of `C C# D D# E F F# G G# A A# B` (sharps only).
    pub tonic: String,
    /// `major` or `minor`.
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The tonic names allowed in `analysis.key.tonic`, in semitone order from C.
pub const TONICS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

impl KeyInfo {
    /// Semitones above C (0..11), or `None` for an unknown tonic name.
    pub fn tonic_index(&self) -> Option<usize> {
        TONICS.iter().position(|t| *t == self.tonic)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    #[serde(default)]
    pub key: Option<KeyInfo>,
    #[serde(default)]
    pub tempo_bpm: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The manifest. Required: `format`, `format_version`, `sample_rate`, `channels`,
/// `duration_frames`, `stems`. Everything else has a default when missing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always `"jksong"`.
    pub format: String,
    /// `"1.0"` for this version; `"1.x"` files are readable.
    pub format_version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_frames: u64,
    /// Informational: `duration_frames / sample_rate`.
    #[serde(default)]
    pub duration_seconds: f64,
    /// Chunk id of the cover image, or null.
    #[serde(default)]
    pub cover: Option<String>,
    /// Chunk id of the lyrics, or null for an instrumental.
    #[serde(default)]
    pub lyrics: Option<String>,
    #[serde(default)]
    pub analysis: Option<Analysis>,
    /// Studio quality preset that made the stems: `full`, `clean` or `permissive`
    /// (any other string is kept; `verify` warns about it).
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub stems_sum_to_source: bool,
    pub stems: Vec<StemInfo>,
    /// Kept as raw JSON: informational, and its shape will grow over time.
    #[serde(default)]
    pub provenance: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Manifest {
    /// Duration in seconds computed from the frame count (exact source of truth).
    pub fn duration(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.duration_frames as f64 / self.sample_rate as f64 }
    }

    /// The stem with this role, if present.
    pub fn stem(&self, role: &str) -> Option<&StemInfo> {
        self.stems.iter().find(|s| s.role == role)
    }

    /// Every chunk id the manifest refers to (all must be present in the file).
    pub fn referenced_chunks(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.stems.iter().map(|s| s.chunk.as_str()).collect();
        if let Some(c) = &self.cover {
            v.push(c);
        }
        if let Some(l) = &self.lyrics {
            v.push(l);
        }
        v
    }

    /// The major part of `format_version` ("1.3" -> 1), if it parses.
    pub fn format_major(&self) -> Option<u16> {
        self.format_version.split('.').next()?.trim().parse().ok()
    }
}

//! The singer's saved state, stored in the settings slots (SONG_FORMAT.md section 6).
//!
//! Reading is lenient (a wrongly typed field falls back to its default, see
//! [`crate::serde_util`]) and preserving (unknown keys at every level are kept in
//! `extra` and written back), because this payload is read-merged-written by every
//! version of the app that ever opens the song.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::lyrics::Lyrics;
use crate::manifest::Manifest;
use crate::serde_util::*;

/// Key shift limits (section 6: `semitones` is -12..+12).
pub const MIN_SEMITONES: i32 = -12;
pub const MAX_SEMITONES: i32 = 12;

fn true_() -> bool {
    true
}
fn one() -> f64 {
    1.0
}
fn one_u32() -> u32 {
    1
}

/// Per-song playback settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Key shift in semitones, -12..+12.
    #[serde(default, deserialize_with = "lenient_i32")]
    pub semitones: i32,
    /// Whether the drums go through the pitch shifter too (default false: they do not).
    #[serde(default, deserialize_with = "lenient")]
    pub shift_drums: bool,
    /// Guide (original lead) vocal level 0..1; default 0 (off).
    #[serde(default, deserialize_with = "lenient_f64")]
    pub guide_vocal: f64,
    #[serde(default = "true_", deserialize_with = "lenient_bool_true")]
    pub backing_on: bool,
    #[serde(default = "one", deserialize_with = "lenient_f64_one")]
    pub backing_level: f64,
    /// Level per role (`drums`, `other`, ...). A missing role means 1.0.
    #[serde(default, deserialize_with = "lenient_levels")]
    pub levels: BTreeMap<String, f64>,
    /// Per-song lyrics timing offset; positive = lyrics later.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub lyrics_offset_ms: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            semitones: 0,
            shift_drums: false,
            guide_vocal: 0.0,
            backing_on: true,
            backing_level: 1.0,
            levels: BTreeMap::new(),
            lyrics_offset_ms: 0,
            extra: Map::new(),
        }
    }
}

impl Settings {
    /// `semitones` clamped to the allowed range (a file from a buggy writer cannot ask
    /// the engine for +40).
    pub fn semitones_clamped(&self) -> i32 {
        self.semitones.clamp(MIN_SEMITONES, MAX_SEMITONES)
    }

    /// Level for a role; 1.0 when not set.
    pub fn level(&self, role: &str) -> f64 {
        self.levels.get(role).copied().unwrap_or(1.0)
    }
}

/// Corrected metadata; each present key wins over the manifest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetadataOverride {
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// `None` = not overridden; `Some(None)` = overridden to "no album".
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub album: Option<Option<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The whole slot payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserState {
    #[serde(default = "one_u32", deserialize_with = "lenient_u32_one")]
    pub user_state_version: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub updated_utc: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub settings: Settings,
    /// `null` in the file reads as an empty override.
    #[serde(default, deserialize_with = "lenient")]
    pub metadata_override: MetadataOverride,
    /// Lyrics corrected in the sync editor; win over the `lyrics` chunk. A malformed
    /// override reads as `None` (the chunk is used).
    #[serde(default, deserialize_with = "lenient")]
    pub lyrics_override: Option<Lyrics>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn lenient_u32_one<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(Value::deserialize(d)?.as_u64().map(|x| x.min(u32::MAX as u64) as u32).unwrap_or(1))
}

impl Default for UserState {
    fn default() -> Self {
        UserState {
            user_state_version: 1,
            updated_utc: None,
            settings: Settings::default(),
            metadata_override: MetadataOverride::default(),
            lyrics_override: None,
            extra: Map::new(),
        }
    }
}

impl UserState {
    /// Parses a payload. Only fails when the payload is not a JSON object at all.
    pub fn from_json(bytes: &[u8]) -> Result<UserState, serde_json::Error> {
        let v: Value = serde_json::from_slice(bytes)?;
        Self::from_value(v)
    }

    pub fn from_value(v: Value) -> Result<UserState, serde_json::Error> {
        if !v.is_object() {
            return Err(serde::de::Error::custom("user state is not a JSON object"));
        }
        serde_json::from_value(v)
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("user state always serialises")
    }
}

/// Title / artist / album after applying the override.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveMetadata {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

/// Applies `metadata_override` to the manifest's metadata: each present key wins.
pub fn effective_metadata(manifest: &Manifest, state: &UserState) -> EffectiveMetadata {
    let o = &state.metadata_override;
    EffectiveMetadata {
        title: o.title.clone().unwrap_or_else(|| manifest.title.clone()),
        artist: o.artist.clone().unwrap_or_else(|| manifest.artist.clone()),
        album: match &o.album {
            Some(a) => a.clone(),
            None => manifest.album.clone(),
        },
    }
}

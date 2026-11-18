// SPDX-License-Identifier: GPL-3.0-or-later
//! App settings: `<root>/Settings/settings.json` (docs/APP_API.md section 5).
//!
//! - `settings_set` deep-merges a partial object into the current settings, checks
//!   the result, clamps numbers into their ranges, and saves.
//! - Saving is atomic (src/fs_atomic.rs) and debounced by 300 ms, so a volume slider
//!   does not cause dozens of fsyncs on a USB stick; it is flushed on exit.
//! - Loading is forgiving: a value of the wrong type falls back to its default (and
//!   the rest is kept); a file that is not JSON at all is kept as
//!   `settings.json.bad` and defaults are used. Unknown keys, at every level, are kept
//!   and written back, so a newer version's settings survive an older version.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::debounce::Debouncer;
use crate::error::{ApiError, ApiResult};

/// File name inside the Settings folder.
pub const FILE_NAME: &str = "settings.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    System,
    Reduce,
    Full,
}

/// How the lyrics ticker moves (docs/APP_API.md section 5; DECISIONS.md D-216).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsMotion {
    /// Smooth, paced by word: each word reaches the marker as it is sung (within
    /// 40 ms) and glides past at its own pace. The owner's choice of 18 Nov 2026.
    #[default]
    Word,
    /// Exact, letter by letter: every letter is under the marker while it is sung.
    Letter,
}

/// How the lyrics are shown on the Stage (docs/APP_API.md section 5; DECISIONS.md D-218).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsDisplay {
    /// Static lines with a colour wipe: the current line stands still and the sung
    /// colour sweeps through it; the next line is previewed underneath. The owner's
    /// choice of 18 Nov 2026.
    #[default]
    Page,
    /// The scrolling ticker: one line moving under a fixed marker (D-04).
    Scroll,
}

/// Lyrics colours of every version before D-218 (hot pink sung, cream upcoming). A
/// settings file written before `ui.lyrics_display` existed that still holds one of these
/// exact values never chose it: it gets the new default instead (see `migrate_colors`).
pub const PREVIOUS_DEFAULT_SUNG: &str = "#ff3d8b";
pub const PREVIOUS_DEFAULT_UPCOMING: &str = "#f6efd9";
/// Defaults since D-218: warm gold sung, white upcoming (the owner's reference video).
pub const DEFAULT_SUNG: &str = "#ffc21a";
pub const DEFAULT_UPCOMING: &str = "#f4f4f4";

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibrarySort {
    Title,
    Artist,
    Recent,
    Key,
}

/// How key changes are made (WORK_ORDER 4.5; docs/APP_API.md section 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyModeSetting {
    /// Measure Rubber Band on this computer once (per sample rate) and render ahead
    /// when it runs below 3x real time.
    #[default]
    Auto,
    /// Always shift in real time (the normal path).
    Realtime,
    /// Always render ahead and cache.
    Prerender,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Colors {
    pub sung: String,
    pub upcoming: String,
    pub marker: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiSettings {
    pub skin: bool,
    pub bubbles: bool,
    pub motion: Motion,
    /// CSS px at 1440x900, scaled with the window.
    pub lyrics_text_size: f64,
    /// 0..1 from the left.
    pub marker_position: f64,
    /// Pace of the lyrics (the wipe or the ticker): "word" (default) or "letter".
    #[serde(default)]
    pub lyrics_motion: LyricsMotion,
    /// "page" (static lines with a colour wipe, default) or "scroll" (the ticker).
    #[serde(default)]
    pub lyrics_display: LyricsDisplay,
    /// Page display: preview the next line under the current one.
    #[serde(default = "default_true")]
    pub show_next_line: bool,
    pub colors: Colors,
    pub library_sort: LibrarySort,
    pub fullscreen_stage: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// `None` = the system default output.
    pub device_id: Option<String>,
    /// WASAPI exclusive mode (Windows only).
    pub exclusive: bool,
    /// 0..1, linear. 1.0 = unity gain (bit-transparent; no hidden processing).
    pub master_volume: f64,
    /// Bluetooth and other output latency, per device id, in milliseconds.
    pub device_offsets_ms: BTreeMap<String, f64>,
    /// Key change: real time, render ahead and cache, or chosen per computer.
    pub key_mode: KeyModeSetting,
    /// Size cap of the key cache (`<root>/Settings/cache/keys`), in MB.
    pub key_cache_mb: f64,
    /// Rubber Band real-time factor measured by "auto", per computer and sample rate
    /// (`"<computer>@<rate>"`; the computer part is a hash, see src/keymode.rs).
    pub key_benchmarks: BTreeMap<String, f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub ui: UiSettings,
    pub audio: AudioSettings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppSettings {
    /// Defaults (docs/APP_API.md section 5, colours from docs/DESIGN.md "Stage").
    fn default() -> Self {
        AppSettings {
            ui: UiSettings {
                skin: true,
                bubbles: true,
                motion: Motion::System,
                // 56 px at 1440x900 (the UI's default, used for its ticker tests; at
                // 64 px the 2-second lookahead rule shrinks fast songs more often).
                lyrics_text_size: 56.0,
                marker_position: 0.33,
                lyrics_motion: LyricsMotion::Word,
                lyrics_display: LyricsDisplay::Page,
                show_next_line: true,
                colors: Colors {
                    sung: DEFAULT_SUNG.into(),
                    upcoming: DEFAULT_UPCOMING.into(),
                    // Mint, not the sung colour: the scrolling ticker's marker must
                    // stand out against the letters it has just passed (same as the UI
                    // default). The page display has no marker.
                    marker: "#7dffd8".into(),
                    extra: Map::new(),
                },
                library_sort: LibrarySort::Title,
                fullscreen_stage: false,
                extra: Map::new(),
            },
            audio: AudioSettings {
                device_id: None,
                exclusive: false,
                master_volume: 1.0,
                device_offsets_ms: BTreeMap::new(),
                key_mode: KeyModeSetting::Auto,
                key_cache_mb: 1024.0,
                key_benchmarks: BTreeMap::new(),
                extra: Map::new(),
            },
            extra: Map::new(),
        }
    }
}

impl AppSettings {
    /// Clamps numbers into their ranges and replaces non-finite values. Never fails:
    /// a settings file edited by hand cannot make the app misbehave.
    pub fn sanitize(&mut self) {
        let ui = &mut self.ui;
        ui.lyrics_text_size = finite_or(ui.lyrics_text_size, 56.0).clamp(12.0, 400.0);
        ui.marker_position = finite_or(ui.marker_position, 0.33).clamp(0.05, 0.95);
        for c in [&mut ui.colors.sung, &mut ui.colors.upcoming, &mut ui.colors.marker] {
            if c.len() > 64 {
                c.truncate(64);
            }
        }
        let a = &mut self.audio;
        a.master_volume = finite_or(a.master_volume, 1.0).clamp(0.0, 1.0);
        a.device_offsets_ms.retain(|k, _| !k.is_empty());
        for v in a.device_offsets_ms.values_mut() {
            *v = finite_or(*v, 0.0).clamp(-5000.0, 5000.0);
        }
        if a.device_id.as_deref().is_some_and(str::is_empty) {
            a.device_id = None;
        }
        a.key_cache_mb = finite_or(a.key_cache_mb, 1024.0).clamp(64.0, 65536.0);
        a.key_benchmarks.retain(|k, v| !k.is_empty() && k.len() <= 64 && v.is_finite() && *v > 0.0);
        while a.key_benchmarks.len() > 32 {
            let first = a.key_benchmarks.keys().next().cloned();
            if let Some(k) = first {
                a.key_benchmarks.remove(&k);
            }
        }
    }

    /// Reads settings leniently from JSON text (see the module docs). Returns the
    /// settings and warnings for the log.
    pub fn from_json_lenient(text: &str) -> (AppSettings, Vec<String>) {
        let mut warnings = Vec::new();
        let file: Value = match serde_json::from_str(text) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                warnings.push("settings file is not a JSON object; using defaults".into());
                return (AppSettings::default(), warnings);
            }
            Err(e) => {
                warnings.push(format!("settings file is not valid JSON ({e}); using defaults"));
                return (AppSettings::default(), warnings);
            }
        };
        let mut merged = serde_json::to_value(AppSettings::default()).expect("defaults serialise");
        merge_typed(&mut merged, &file, "", &mut warnings);
        let mut s = match serde_json::from_value::<AppSettings>(merged) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!("settings could not be read ({e}); using defaults"));
                AppSettings::default()
            }
        };
        // A file written before the page display existed (no `ui.lyrics_display`) holds
        // every colour it ever had, chosen or not: every earlier version wrote the whole
        // settings object on its first save.
        if file.get("ui").and_then(|u| u.get("lyrics_display")).is_none() {
            s.migrate_colors();
        }
        s.sanitize();
        (s, warnings)
    }

    /// D-218: the sung and upcoming colours of every earlier version (hot pink, cream)
    /// were the defaults, so a file written before `ui.lyrics_display` existed that still
    /// holds exactly one of them never chose it; it gets the new default (gold, white).
    /// A colour the owner did choose is kept. After the next save the file carries
    /// `ui.lyrics_display`, so this runs once: an old colour chosen again later stays.
    pub fn migrate_colors(&mut self) {
        let c = &mut self.ui.colors;
        if c.sung.trim().eq_ignore_ascii_case(PREVIOUS_DEFAULT_SUNG) {
            c.sung = DEFAULT_SUNG.into();
        }
        if c.upcoming.trim().eq_ignore_ascii_case(PREVIOUS_DEFAULT_UPCOMING) {
            c.upcoming = DEFAULT_UPCOMING.into();
        }
    }
}

fn finite_or(x: f64, fallback: f64) -> f64 {
    if x.is_finite() { x } else { fallback }
}

/// Same JSON "shape" for the lenient merge: numbers with numbers, strings with
/// strings, and `null` interchangeable with strings (optional ids).
fn same_kind(default: &Value, file: &Value) -> bool {
    matches!(
        (default, file),
        (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Null, Value::Null)
            | (Value::Null, Value::String(_))
            | (Value::String(_), Value::Null)
            | (Value::Array(_), Value::Array(_))
    )
}

/// Values whose JSON kind can be right while the typed settings still refuse them:
/// enum names (a newer version may add one, and both apps on a stick share one
/// settings file) and the numbers in `device_offsets_ms`. Checked one by one, so a
/// single such value falls back to its default instead of making the whole file
/// unreadable (which reset every setting, device and offsets included).
fn typed_ok(path: &str, v: &Value) -> bool {
    match path {
        "ui.motion" => serde_json::from_value::<Motion>(v.clone()).is_ok(),
        "ui.library_sort" => serde_json::from_value::<LibrarySort>(v.clone()).is_ok(),
        "ui.lyrics_motion" => serde_json::from_value::<LyricsMotion>(v.clone()).is_ok(),
        "ui.lyrics_display" => serde_json::from_value::<LyricsDisplay>(v.clone()).is_ok(),
        "audio.key_mode" => serde_json::from_value::<KeyModeSetting>(v.clone()).is_ok(),
        _ if path.starts_with("audio.device_offsets_ms.") => v.is_number(),
        _ if path.starts_with("audio.key_benchmarks.") => v.is_number(),
        _ => true,
    }
}

/// Merges `file` into `defaults`, keeping a file value only when it has the kind of
/// the default. Keys the defaults do not have (unknown keys from a newer version, or
/// the entries of a map such as `device_offsets_ms`) are taken as they are.
fn merge_typed(defaults: &mut Value, file: &Value, path: &str, warnings: &mut Vec<String>) {
    let (Value::Object(d), Value::Object(f)) = (defaults, file) else {
        return;
    };
    for (k, fv) in f {
        let here = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
        if !typed_ok(&here, fv) {
            warnings.push(format!("settings: '{here}' has a value this version does not know; using the default"));
            continue;
        }
        match d.get_mut(k) {
            None => {
                d.insert(k.clone(), fv.clone());
            }
            Some(dv @ Value::Object(_)) => {
                if fv.is_object() {
                    merge_typed(dv, fv, &here, warnings);
                } else {
                    warnings.push(format!("settings: '{here}' is not an object; using the default"));
                }
            }
            Some(dv) => {
                if same_kind(dv, fv) {
                    *dv = fv.clone();
                } else {
                    warnings.push(format!("settings: '{here}' has the wrong type; using the default"));
                }
            }
        }
    }
}

/// Deep merge for `settings_set`: objects merge key by key, anything else replaces.
pub fn deep_merge(target: &mut Value, changes: &Value) {
    match (target, changes) {
        (Value::Object(t), Value::Object(c)) => {
            for (k, cv) in c {
                match t.get_mut(k) {
                    Some(tv) if tv.is_object() && cv.is_object() => deep_merge(tv, cv),
                    _ => {
                        t.insert(k.clone(), cv.clone());
                    }
                }
            }
        }
        (t, c) => *t = c.clone(),
    }
}

/// Reads the settings file once (or defaults), without a store. Used at start-up
/// for the engine's device and volume before the store exists.
pub fn read_file(settings_dir: &Path) -> AppSettings {
    match std::fs::read_to_string(settings_dir.join(FILE_NAME)) {
        Ok(text) => AppSettings::from_json_lenient(&text).0,
        Err(_) => AppSettings::default(),
    }
}

struct Inner {
    path: PathBuf,
    current: Mutex<AppSettings>,
}

impl Inner {
    fn write_now(&self) {
        let text = {
            let s = self.current.lock().unwrap_or_else(|e| e.into_inner());
            serde_json::to_string_pretty(&*s).expect("settings serialise")
        };
        if let Err(e) = crate::fs_atomic::write_atomic(&self.path, text.as_bytes()) {
            crate::applog::error(&format!("saving {} failed: {e}", self.path.display()));
        }
    }
}

/// The settings file, loaded once and kept in memory.
pub struct SettingsStore {
    inner: Arc<Inner>,
    saver: Debouncer,
}

impl SettingsStore {
    /// Loads `<settings_dir>/settings.json` (or defaults when it does not exist).
    pub fn load(settings_dir: &Path) -> SettingsStore {
        Self::load_with_delay(settings_dir, Duration::from_millis(300))
    }

    pub fn load_with_delay(settings_dir: &Path, delay: Duration) -> SettingsStore {
        let path = settings_dir.join(FILE_NAME);
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (s, warnings) = AppSettings::from_json_lenient(&text);
                for w in &warnings {
                    crate::applog::warn(w);
                }
                if warnings.iter().any(|w| w.contains("using defaults")) {
                    // Keep the unreadable file for the owner instead of overwriting it
                    // silently on the next save.
                    let _ = std::fs::copy(&path, path.with_extension("json.bad"));
                }
                s
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppSettings::default(),
            Err(e) => {
                crate::applog::error(&format!("cannot read {}: {e}", path.display()));
                AppSettings::default()
            }
        };
        let inner = Arc::new(Inner {
            path,
            current: Mutex::new(settings),
        });
        let i = Arc::clone(&inner);
        let saver = Debouncer::new("settings", delay, move || i.write_now());
        SettingsStore { inner, saver }
    }

    /// A copy of the current settings.
    pub fn get(&self) -> AppSettings {
        self.inner
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// `settings_set`: deep-merges `changes` (a partial `AppSettings` object), checks
    /// and clamps the result, schedules the save, and returns the new settings.
    pub fn set(&self, changes: &Value) -> ApiResult<AppSettings> {
        if !changes.is_object() {
            return Err(ApiError::bad_request("Settings changes must be an object."));
        }
        let mut cur = self.inner.current.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = serde_json::to_value(&*cur).map_err(ApiError::internal)?;
        deep_merge(&mut v, changes);
        let mut next: AppSettings = serde_json::from_value(v).map_err(|e| {
            ApiError::bad_request(format!("These settings could not be applied: {e}"))
        })?;
        next.sanitize();
        *cur = next.clone();
        drop(cur);
        self.saver.touch();
        Ok(next)
    }

    /// Changes the settings from Rust (device switch, master volume) and schedules the
    /// save.
    pub fn update(&self, edit: impl FnOnce(&mut AppSettings)) -> AppSettings {
        let mut cur = self.inner.current.lock().unwrap_or_else(|e| e.into_inner());
        edit(&mut cur);
        cur.sanitize();
        let out = cur.clone();
        drop(cur);
        self.saver.touch();
        out
    }

    /// Writes any pending change now (on exit).
    pub fn flush(&self) {
        self.saver.flush();
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "jukeoke-settings-test-{}-{name}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_match_the_contract() {
        let v = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(v["ui"]["marker_position"], json!(0.33));
        assert_eq!(v["ui"]["motion"], json!("system"));
        assert_eq!(v["ui"]["lyrics_motion"], json!("word"));
        assert_eq!(v["ui"]["library_sort"], json!("title"));
        assert_eq!(v["audio"]["device_id"], Value::Null);
        assert_eq!(v["audio"]["master_volume"], json!(1.0));
        assert_eq!(v["audio"]["device_offsets_ms"], json!({}));
        for k in ["skin", "bubbles", "lyrics_text_size", "colors", "fullscreen_stage"] {
            assert!(v["ui"].get(k).is_some(), "ui.{k} missing");
        }
        assert!(v["audio"].get("exclusive").is_some());
        assert_eq!(v["audio"]["key_mode"], json!("auto"));
        assert_eq!(v["audio"]["key_cache_mb"], json!(1024.0));
        assert_eq!(v["audio"]["key_benchmarks"], json!({}));
    }

    #[test]
    fn key_mode_settings_are_checked_and_clamped() {
        let text = r#"{"audio": {"key_mode": "turbo", "key_cache_mb": 1,
                                 "key_benchmarks": {"pc@48000": 9.5, "bad": "x", "neg@44100": -1}}}"#;
        let (s, warnings) = AppSettings::from_json_lenient(text);
        assert_eq!(s.audio.key_mode, KeyModeSetting::Auto, "unknown mode falls back alone");
        assert_eq!(s.audio.key_cache_mb, 64.0);
        assert_eq!(s.audio.key_benchmarks.len(), 1);
        assert_eq!(s.audio.key_benchmarks["pc@48000"], 9.5);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let (s, _) = AppSettings::from_json_lenient(r#"{"audio": {"key_mode": "prerender"}}"#);
        assert_eq!(s.audio.key_mode, KeyModeSetting::Prerender);
    }

    #[test]
    fn lyrics_motion_defaults_to_word_and_unknown_values_reset_alone() {
        // A settings file from before the setting existed: the new default.
        let (s, warnings) = AppSettings::from_json_lenient(r#"{"ui": {"marker_position": 0.4}}"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Word);
        assert_eq!(s.ui.marker_position, 0.4);
        // The letter pace is kept.
        let (s, _) = AppSettings::from_json_lenient(r#"{"ui": {"lyrics_motion": "letter"}}"#);
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Letter);
        // A value this version does not know (or of the wrong type) falls back alone.
        let (s, warnings) = AppSettings::from_json_lenient(
            r#"{"ui": {"lyrics_motion": "syllable", "bubbles": false}}"#,
        );
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Word);
        assert!(!s.ui.bubbles);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (s, warnings) = AppSettings::from_json_lenient(r#"{"ui": {"lyrics_motion": 3}}"#);
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Word);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // settings_set takes "letter" and refuses an unknown name without changing anything.
        let dir = scratch("lyrics-motion");
        let store = SettingsStore::load_with_delay(&dir, Duration::from_millis(50));
        let s = store
            .set(&json!({"ui": {"lyrics_motion": "letter"}}))
            .unwrap();
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Letter);
        assert!(
            store
                .set(&json!({"ui": {"lyrics_motion": "fast"}}))
                .is_err()
        );
        assert_eq!(store.get().ui.lyrics_motion, LyricsMotion::Letter);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lyrics_display_defaults_to_page_and_unknown_values_reset_alone() {
        let v = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(v["ui"]["lyrics_display"], json!("page"));
        assert_eq!(v["ui"]["show_next_line"], json!(true));
        assert_eq!(v["ui"]["colors"]["sung"], json!("#ffc21a"));
        assert_eq!(v["ui"]["colors"]["upcoming"], json!("#f4f4f4"));
        assert_eq!(v["ui"]["colors"]["marker"], json!("#7dffd8"));
        // A file from before the setting existed: the new default, no warning.
        let (s, warnings) = AppSettings::from_json_lenient(r#"{"ui": {"bubbles": false}}"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.ui.lyrics_display, LyricsDisplay::Page);
        assert!(s.ui.show_next_line);
        // The scrolling ticker and a hidden next line are kept.
        let (s, _) = AppSettings::from_json_lenient(
            r#"{"ui": {"lyrics_display": "scroll", "show_next_line": false}}"#,
        );
        assert_eq!(s.ui.lyrics_display, LyricsDisplay::Scroll);
        assert!(!s.ui.show_next_line);
        // A value this version does not know (or of the wrong type) falls back alone.
        let (s, warnings) = AppSettings::from_json_lenient(
            r#"{"ui": {"lyrics_display": "karaoke3d", "show_next_line": "no", "bubbles": false}}"#,
        );
        assert_eq!(s.ui.lyrics_display, LyricsDisplay::Page);
        assert!(s.ui.show_next_line);
        assert!(!s.ui.bubbles);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        // settings_set takes "scroll" and refuses an unknown name without changing anything.
        let dir = scratch("lyrics-display");
        let store = SettingsStore::load_with_delay(&dir, Duration::from_millis(50));
        let s = store
            .set(&json!({"ui": {"lyrics_display": "scroll", "show_next_line": false}}))
            .unwrap();
        assert_eq!(s.ui.lyrics_display, LyricsDisplay::Scroll);
        assert!(store.set(&json!({"ui": {"lyrics_display": "ticker"}})).is_err());
        assert_eq!(store.get().ui.lyrics_display, LyricsDisplay::Scroll);
        assert!(!store.get().ui.show_next_line);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn previous_default_colours_migrate_once_and_chosen_colours_stay() {
        // The owner's settings.json as every earlier version wrote it (dist/Settings,
        // 18 Nov 2026): the previous default colours, no lyrics_display, no lyrics_motion.
        let owner = r##"{"ui": {"skin": true, "bubbles": true, "motion": "system",
            "lyrics_text_size": 56.0, "marker_position": 0.33,
            "colors": {"sung": "#ff3d8b", "upcoming": "#f6efd9", "marker": "#7dffd8"},
            "library_sort": "title", "fullscreen_stage": false},
            "audio": {"device_id": null, "exclusive": false, "master_volume": 1.0,
            "device_offsets_ms": {}, "key_mode": "auto", "key_cache_mb": 1024.0,
            "key_benchmarks": {"windows-x86_64-38c7d06324e5@44100": 9.06,
                               "windows-x86_64-38c7d06324e5@48000": 8.27}}}"##;
        let (s, warnings) = AppSettings::from_json_lenient(owner);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(s.ui.colors.sung, DEFAULT_SUNG, "previous default -> new default");
        assert_eq!(s.ui.colors.upcoming, DEFAULT_UPCOMING);
        assert_eq!(s.ui.colors.marker, "#7dffd8", "marker default unchanged");
        assert_eq!(s.ui.lyrics_display, LyricsDisplay::Page);
        assert_eq!(s.ui.lyrics_motion, LyricsMotion::Word);
        assert_eq!(s.audio.key_benchmarks.len(), 2, "nothing else lost");
        assert_eq!(s.audio.key_benchmarks["windows-x86_64-38c7d06324e5@48000"], 8.27);
        assert_eq!(s.ui.lyrics_text_size, 56.0);
        // Same check ignoring case (the colour picker writes lower case; a hand edit may not).
        let (s, _) = AppSettings::from_json_lenient(
            r##"{"ui": {"colors": {"sung": "#FF3D8B", "upcoming": "#F6EFD9"}}}"##,
        );
        assert_eq!((s.ui.colors.sung.as_str(), s.ui.colors.upcoming.as_str()), (DEFAULT_SUNG, DEFAULT_UPCOMING));
        // A colour the owner chose is kept; only the other one migrates.
        let (s, _) = AppSettings::from_json_lenient(
            r##"{"ui": {"colors": {"sung": "#00ff00", "upcoming": "#f6efd9", "marker": "#ffffff"}}}"##,
        );
        assert_eq!(s.ui.colors.sung, "#00ff00");
        assert_eq!(s.ui.colors.upcoming, DEFAULT_UPCOMING);
        assert_eq!(s.ui.colors.marker, "#ffffff");
        // Written by this version (it has lyrics_display): the old pink was chosen again
        // on purpose and stays.
        let (s, _) = AppSettings::from_json_lenient(
            r##"{"ui": {"lyrics_display": "scroll", "colors": {"sung": "#ff3d8b", "upcoming": "#f6efd9"}}}"##,
        );
        assert_eq!(s.ui.colors.sung, PREVIOUS_DEFAULT_SUNG);
        assert_eq!(s.ui.colors.upcoming, PREVIOUS_DEFAULT_UPCOMING);
        // Through the store: the owner's file loads with the new look, the first save
        // writes lyrics_display and the new colours, and everything else survives.
        let dir = scratch("colour-migration");
        std::fs::write(dir.join(FILE_NAME), owner).unwrap();
        let store = SettingsStore::load_with_delay(&dir, Duration::from_millis(50));
        assert_eq!(store.get().ui.colors.sung, DEFAULT_SUNG);
        store.set(&json!({"ui": {"bubbles": false}})).unwrap();
        store.flush();
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(on_disk["ui"]["lyrics_display"], json!("page"));
        assert_eq!(on_disk["ui"]["colors"]["sung"], json!(DEFAULT_SUNG));
        assert_eq!(on_disk["ui"]["bubbles"], json!(false));
        assert_eq!(on_disk["audio"]["key_benchmarks"]["windows-x86_64-38c7d06324e5@44100"], json!(9.06));
        // The owner then picks the old pink on purpose: kept after a reload.
        store.set(&json!({"ui": {"colors": {"sung": "#ff3d8b"}}})).unwrap();
        drop(store);
        let again = SettingsStore::load(&dir);
        assert_eq!(again.get().ui.colors.sung, PREVIOUS_DEFAULT_SUNG);
        assert_eq!(again.get().ui.colors.upcoming, DEFAULT_UPCOMING);
        drop(again);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn set_deep_merges_clamps_and_saves_atomically() {
        let dir = scratch("set");
        let store = SettingsStore::load_with_delay(&dir, Duration::from_millis(50));
        let s = store
            .set(&json!({"ui": {"colors": {"sung": "#123456"}, "marker_position": 7.0},
                          "audio": {"master_volume": 0.5, "device_offsets_ms": {"bt": 180}}}))
            .unwrap();
        assert_eq!(s.ui.colors.sung, "#123456");
        assert_eq!(s.ui.colors.upcoming, DEFAULT_UPCOMING, "sibling keys kept");
        assert_eq!(s.ui.marker_position, 0.95, "clamped");
        assert_eq!(s.audio.device_offsets_ms["bt"], 180.0);
        store.flush();
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(on_disk["audio"]["master_volume"], json!(0.5));
        // A wrong type is refused and changes nothing.
        let err = store.set(&json!({"ui": {"skin": "yes"}})).unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert!(store.get().ui.skin);
        // Reload sees the same values.
        drop(store);
        let again = SettingsStore::load(&dir);
        assert_eq!(again.get().ui.colors.sung, "#123456");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lenient_load_keeps_good_values_and_unknown_keys() {
        let text = r#"{"ui": {"bubbles": false, "skin": "not a bool", "future_knob": 3},
                       "audio": {"master_volume": "loud", "device_id": "dev-1"},
                       "newer_section": {"x": 1}}"#;
        let (s, warnings) = AppSettings::from_json_lenient(text);
        assert!(!s.ui.bubbles);
        assert!(s.ui.skin, "bad type falls back to the default");
        assert_eq!(s.audio.master_volume, 1.0);
        assert_eq!(s.audio.device_id.as_deref(), Some("dev-1"));
        assert_eq!(s.ui.extra["future_knob"], json!(3));
        assert_eq!(s.extra["newer_section"], json!({"x": 1}));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["newer_section"]["x"], json!(1), "unknown keys written back");
    }

    #[test]
    fn an_unknown_enum_value_or_bad_offset_resets_only_itself() {
        // E.g. a newer app on the same stick wrote a sort order this version lacks.
        let text = r##"{"ui": {"motion": "auto", "library_sort": "duration", "bubbles": false,
                                "colors": {"sung": "#010203"}},
                        "audio": {"device_id": "dev-1", "master_volume": 0.4,
                                  "device_offsets_ms": {"bt": 180, "bad": "x"}}}"##;
        let (s, warnings) = AppSettings::from_json_lenient(text);
        assert!(warnings.iter().all(|w| !w.contains("using defaults")), "{warnings:?}");
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(s.ui.motion, Motion::System);
        assert_eq!(s.ui.library_sort, LibrarySort::Title);
        assert!(!s.ui.bubbles, "the rest is kept");
        assert_eq!(s.ui.colors.sung, "#010203");
        assert_eq!(s.audio.device_id.as_deref(), Some("dev-1"));
        assert_eq!(s.audio.master_volume, 0.4);
        assert_eq!(s.audio.device_offsets_ms.get("bt"), Some(&180.0));
        assert!(!s.audio.device_offsets_ms.contains_key("bad"));
    }

    #[test]
    fn garbage_file_is_kept_aside_and_defaults_used() {
        let dir = scratch("garbage");
        std::fs::write(dir.join(FILE_NAME), b"{not json").unwrap();
        let store = SettingsStore::load(&dir);
        assert_eq!(store.get(), AppSettings::default());
        assert!(dir.join("settings.json.bad").is_file());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

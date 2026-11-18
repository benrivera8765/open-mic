// SPDX-License-Identifier: GPL-3.0-or-later
//! The song library: `<root>/Library` (docs/APP_API.md section 2).
//!
//! - Scans with jk_format's helpers (they skip dotfiles, macOS `._*` files and
//!   unfinished `*.tmp-*` writes), recursively, sorted by file name.
//! - A song is identified by `song_id`: its path relative to the Library folder with
//!   `/` separators, the same on both systems and on every drive letter.
//! - Summaries use the *effective* metadata and settings (the song's settings-slot
//!   overrides win over the manifest), and are cached by (size, modification time) so
//!   listing a big library twice does not reopen every file.
//! - Cover thumbnails are decoded (JPEG / PNG), downscaled and cached in memory.
//! - A file watcher (notify) emits `library://changed`, debounced by 400 ms.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, names};
use crate::keys::{Key, KeyView};

/// One song in the list (docs/APP_API.md `SongSummary`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SongSummary {
    pub song_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_s: f64,
    pub key: Option<KeyView>,
    pub tempo_bpm: Option<f64>,
    pub has_lyrics: bool,
    pub lyrics_checked: bool,
    /// Saved key shift.
    pub semitones: i32,
    pub file_bytes: u64,
    pub modified_ms: u64,
    pub preset: String,
    /// Non-null: listed but unplayable (damaged, newer version...).
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryList {
    pub songs: Vec<SongSummary>,
    pub free_bytes: u64,
    pub total_bytes: u64,
    /// Addition (2026-11-18): songs in Deleted songs and the space they still take on
    /// the drive (src/deleted.rs; filled in by `Core::library_list`).
    pub deleted_count: usize,
    pub deleted_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Cover {
    pub data_url: Option<String>,
}

/// (file size, modification time in ms): a cheap "has it changed" stamp.
type Stamp = (u64, u64);

/// (song_id, max_px) of a cached thumbnail.
type CoverKey = (String, u32);

fn stamp(meta: &std::fs::Metadata) -> Stamp {
    let ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    (meta.len(), ms)
}

pub struct Library {
    dir: PathBuf,
    summaries: Mutex<HashMap<PathBuf, (Stamp, SongSummary)>>,
    covers: Mutex<HashMap<CoverKey, (Stamp, Option<String>)>>,
}

/// Most cover thumbnails kept in memory (a 160 px JPEG is about 8 KB).
const MAX_CACHED_COVERS: usize = 512;

impl Library {
    pub fn new(dir: PathBuf) -> Library {
        Library {
            dir,
            summaries: Mutex::new(HashMap::new()),
            covers: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `song_id` of a path inside the library.
    pub fn song_id_of(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.dir).ok()?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Option<_>>()?;
        (!parts.is_empty()).then(|| parts.join("/"))
    }

    /// The file of a `song_id`. Refuses ids that would leave the Library folder
    /// (`..`, absolute paths, drive letters) or that do not name a song file.
    pub fn resolve(&self, song_id: &str) -> ApiResult<PathBuf> {
        let bad = || ApiError::new("not_found", "This song is not in the library.");
        if song_id.is_empty() || song_id.contains('\0') || song_id.contains('\\') {
            return Err(bad());
        }
        let mut path = self.dir.clone();
        for part in song_id.split('/') {
            if part.is_empty() || part == "." || part == ".." || part.contains(':') {
                return Err(bad());
            }
            path.push(part);
        }
        let name = path.file_name().ok_or_else(bad)?;
        if !jk_format::is_song_file_name(name) {
            return Err(bad());
        }
        if !path.is_file() {
            return Err(ApiError::new(
                "not_found",
                "This song file is no longer in the library (it was moved or deleted).",
            ));
        }
        Ok(path)
    }

    /// `library_list`: every song with its effective metadata, plus free space.
    pub fn list(&self) -> ApiResult<LibraryList> {
        let files = match jk_format::list_song_files(&self.dir, true) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ApiError::io("the Library folder", &e)),
        };
        let mut songs = Vec::with_capacity(files.len());
        let mut fresh = HashMap::with_capacity(files.len());
        {
            let cache = self.summaries.lock().unwrap_or_else(|e| e.into_inner());
            for path in &files {
                let Ok(meta) = std::fs::metadata(path) else { continue };
                let st = stamp(&meta);
                let summary = match cache.get(path) {
                    Some((s, summary)) if *s == st => summary.clone(),
                    _ => self.summarise(path, st),
                };
                fresh.insert(path.clone(), (st, summary.clone()));
                songs.push(summary);
            }
        }
        *self.summaries.lock().unwrap_or_else(|e| e.into_inner()) = fresh;
        let (free_bytes, total_bytes) = crate::sysinfo::disk_space(&self.dir).unwrap_or((0, 0));
        Ok(LibraryList {
            songs,
            free_bytes,
            total_bytes,
            deleted_count: 0,
            deleted_bytes: 0,
        })
    }

    /// The summary of one song (fresh from disk).
    pub fn summary(&self, path: &Path) -> SongSummary {
        let st = std::fs::metadata(path).map(|m| stamp(&m)).unwrap_or((0, 0));
        self.summarise(path, st)
    }

    fn summarise(&self, path: &Path, st: Stamp) -> SongSummary {
        let song_id = self.song_id_of(path).unwrap_or_default();
        match jk_format::SongFile::open(path) {
            Ok(song) => summary_of(&song, song_id, st),
            Err(e) => {
                crate::applog::warn(&format!("library: {} cannot be opened: {e}", path.display()));
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // "Artist - Title" is the Studio's naming; split it back for the list.
                let (artist, title) = match stem.split_once(" - ") {
                    Some((a, t)) => (a.to_string(), t.to_string()),
                    None => (String::new(), stem),
                };
                SongSummary {
                    song_id,
                    title,
                    artist,
                    album: None,
                    duration_s: 0.0,
                    key: None,
                    tempo_bpm: None,
                    has_lyrics: false,
                    lyrics_checked: false,
                    semitones: 0,
                    file_bytes: st.0,
                    modified_ms: st.1,
                    preset: String::new(),
                    error: Some(e.user_message()),
                }
            }
        }
    }

    /// `song_cover`: a JPEG / PNG thumbnail whose longer side is at most `max_px`.
    pub fn cover(&self, song_id: &str, max_px: u32) -> ApiResult<Cover> {
        let path = self.resolve(song_id)?;
        let max_px = max_px.clamp(16, 2048);
        let st = std::fs::metadata(&path).map(|m| stamp(&m)).unwrap_or((0, 0));
        let key = (song_id.to_string(), max_px);
        if let Some((s, url)) = self.covers.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
            && *s == st
        {
            return Ok(Cover {
                data_url: url.clone(),
            });
        }
        let song = jk_format::SongFile::open(&path)?;
        let url = match song.cover() {
            Ok(Some((mime, bytes))) => match thumbnail_data_url(&mime, &bytes, max_px) {
                Ok(url) => Some(url),
                Err(e) => {
                    crate::applog::warn(&format!("cover of {song_id} cannot be decoded: {e}"));
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                crate::applog::warn(&format!("cover of {song_id} cannot be read: {e}"));
                None
            }
        };
        let mut covers = self.covers.lock().unwrap_or_else(|e| e.into_inner());
        if covers.len() >= MAX_CACHED_COVERS {
            covers.clear();
        }
        covers.insert(key, (st, url.clone()));
        Ok(Cover { data_url: url })
    }

    /// Deletes `*.jksong.tmp-*` leftovers of interrupted writes in the Library folder
    /// that are older than an hour (a younger one may belong to a Studio job writing
    /// right now). Returns how many were removed.
    pub fn clean_stale_temp_files(&self) -> usize {
        let Ok(files) = jk_format::stale_temp_files(&self.dir) else { return 0 };
        let mut n = 0;
        for f in files {
            let old = std::fs::metadata(&f)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(3600));
            if old && std::fs::remove_file(&f).is_ok() {
                crate::applog::info(&format!("removed unfinished write {}", f.display()));
                n += 1;
            }
        }
        n
    }
}

/// Builds the summary of an open song file (effective metadata and settings).
pub fn summary_of(song: &jk_format::SongFile, song_id: String, st: (u64, u64)) -> SongSummary {
    let m = song.manifest();
    let meta = song.effective_metadata();
    let state = song.user_state();
    let key = m
        .analysis
        .as_ref()
        .and_then(|a| a.key.as_ref())
        .and_then(|k| Key::parse(&k.tonic, &k.mode));
    let tempo_bpm = m
        .analysis
        .as_ref()
        .and_then(|a| a.tempo_bpm)
        .filter(|t| t.is_finite() && *t > 0.0);
    let (has_lyrics, lyrics_checked) = match &state.lyrics_override {
        Some(l) => (!l.lines.is_empty(), l.checked),
        None => match song.lyrics() {
            Ok(Some(l)) => (!l.lines.is_empty(), l.checked),
            _ => (false, false),
        },
    };
    SongSummary {
        song_id,
        title: meta.title,
        artist: meta.artist,
        album: meta.album,
        duration_s: m.duration(),
        key: key.map(Key::view),
        tempo_bpm,
        has_lyrics,
        lyrics_checked,
        semitones: state.settings.semitones_clamped(),
        file_bytes: st.0,
        modified_ms: st.1,
        preset: m.preset.clone().unwrap_or_else(|| "full".into()),
        error: None,
    }
}

/// The detected key of a song, if any.
pub fn song_key(song: &jk_format::SongFile) -> Option<Key> {
    song.manifest()
        .analysis
        .as_ref()
        .and_then(|a| a.key.as_ref())
        .and_then(|k| Key::parse(&k.tonic, &k.mode))
}

/// Decodes a cover, downscales it so the longer side is at most `max_px`, and returns
/// a `data:` URL. Small JPEG / PNG covers are passed through unchanged (no needless
/// re-encoding).
pub fn thumbnail_data_url(mime: &str, bytes: &[u8], max_px: u32) -> Result<String, String> {
    use base64::Engine as _;
    use image::ImageFormat;
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    let out_mime = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        other => return Err(format!("unsupported cover format {other:?} (declared {mime})")),
    };
    let img = image::load_from_memory_with_format(bytes, format).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD;
    if img.width() <= max_px && img.height() <= max_px {
        return Ok(format!("data:{out_mime};base64,{}", b64.encode(bytes)));
    }
    // Lanczos3 for a sharp downscale; the result is at most max_px on its longer side.
    let thumb = img.resize(max_px, max_px, image::imageops::FilterType::Lanczos3);
    let mut out = std::io::Cursor::new(Vec::new());
    let (mime, result) = if thumb.color().has_alpha() {
        ("image/png", thumb.write_to(&mut out, ImageFormat::Png))
    } else {
        let rgb = image::DynamicImage::ImageRgb8(thumb.to_rgb8());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
        ("image/jpeg", rgb.write_with_encoder(enc))
    };
    result.map_err(|e| e.to_string())?;
    Ok(format!("data:{mime};base64,{}", b64.encode(out.into_inner())))
}

/// Watches the Library folder and emits `library://changed` (no payload), at most
/// every 400 ms. Falls back to polling every 5 s when the OS watcher cannot start
/// (some network or FAT drivers).
pub struct Watcher {
    _watcher: Box<dyn notify::Watcher + Send>,
}

impl Watcher {
    pub fn start(dir: &Path, sink: Arc<dyn EventSink>) -> Option<Watcher> {
        use notify::RecursiveMode;
        let _ = std::fs::create_dir_all(dir);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handler = move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                // Ignore pure accesses and our own temp files coming and going.
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                // A name that was removed or renamed away can no longer be checked with
                // is_dir(): a folder of songs moved to the Recycle Bin arrives as one
                // event for the folder alone. So any removed / renamed name counts,
                // except our own unfinished writes (`*.tmp-*`).
                let gone = matches!(
                    ev.kind,
                    notify::EventKind::Remove(_) | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                let relevant = ev.paths.iter().any(|p| {
                    p.file_name()
                        .map(|n| {
                            jk_format::is_song_file_name(n)
                                || p.is_dir()
                                || (gone && !n.to_string_lossy().contains(".tmp-"))
                        })
                        .unwrap_or(true)
                }) || ev.paths.is_empty();
                if relevant {
                    let _ = tx.send(());
                }
            }
        };
        let mut watcher: Box<dyn notify::Watcher + Send> =
            match notify::recommended_watcher(handler.clone()) {
                Ok(w) => Box::new(w),
                Err(e) => {
                    crate::applog::warn(&format!("library watcher: {e}; polling every 5 s instead"));
                    let cfg = notify::Config::default().with_poll_interval(Duration::from_secs(5));
                    Box::new(notify::PollWatcher::new(handler, cfg).ok()?)
                }
            };
        if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
            crate::applog::warn(&format!("library watcher on {}: {e}", dir.display()));
            return None;
        }
        std::thread::Builder::new()
            .name("library-watch".into())
            .spawn(move || {
                // Coalesce bursts (a Studio write is create + several modifies + rename).
                while rx.recv().is_ok() {
                    let until = Instant::now() + Duration::from_millis(400);
                    while let Some(left) = until.checked_duration_since(Instant::now()) {
                        if rx.recv_timeout(left).is_err() {
                            break;
                        }
                    }
                    sink.emit_value(names::LIBRARY_CHANGED, serde_json::Value::Null);
                }
            })
            .ok()?;
        Some(Watcher { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "jukeoke-library-test-{}-{name}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn song_ids_are_relative_with_forward_slashes_and_cannot_escape() {
        let dir = scratch("ids");
        std::fs::create_dir_all(dir.join("Pop")).unwrap();
        std::fs::write(dir.join("Pop").join("A - B.jksong"), b"x").unwrap();
        let lib = Library::new(dir.clone());
        assert_eq!(
            lib.song_id_of(&dir.join("Pop").join("A - B.jksong")).as_deref(),
            Some("Pop/A - B.jksong")
        );
        assert!(lib.resolve("Pop/A - B.jksong").is_ok());
        for bad in [
            "../x.jksong",
            "Pop/../../x.jksong",
            "C:/x.jksong",
            "/etc/x.jksong",
            "Pop\\A - B.jksong",
            "Pop/A - B.txt",
            "",
            "Pop//A - B.jksong",
        ] {
            assert_eq!(lib.resolve(bad).unwrap_err().code, "not_found", "{bad}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn thumbnails_downscale_and_pass_small_covers_through() {
        // A 300x200 opaque PNG -> JPEG thumbnail at most 64 px wide.
        let img = image::RgbImage::from_fn(300, 200, |x, y| image::Rgb([x as u8, y as u8, 128]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let url = thumbnail_data_url("image/png", &png, 64).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"), "{}", &url[..30]);
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(url.split_once(",").unwrap().1)
            .unwrap();
        let thumb = image::load_from_memory(&bytes).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (64, 43));
        // Small enough already: the original bytes, unchanged.
        let url = thumbnail_data_url("image/png", &png, 512).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(thumbnail_data_url("image/gif", b"GIF89a....", 64).is_err());
    }
}

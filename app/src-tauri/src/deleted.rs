// SPDX-License-Identifier: GPL-3.0-or-later
//! Deleted songs: the app's own recycle bin, `<root>/Deleted/` (the owner's request of
//! 18 Nov 2026, DECISIONS D-230; docs/APP_API.md section 2).
//!
//! Files deleted from a USB stick bypass the Windows Recycle Bin, so "delete" in the
//! Library MOVES the song's `.jksong` file (one self-contained file) out of `Library/`
//! into `Deleted/`, from where it can be restored or deleted for good.
//!
//! - `Deleted/index.json` (written atomically, src/fs_atomic.rs) remembers each song's
//!   place in the Library (its `song_id`), title, artist, size and deletion time.
//!   Unknown keys are kept, so a newer app sharing the stick loses nothing.
//! - Names never clash: `Title.jksong`, `Title (2).jksong`... compared without letter
//!   case (exFAT, NTFS and APFS ignore it by default); a restore whose place is taken
//!   gets a free name next to it the same way.
//! - A move is a rename on the same volume ([`crate::fs_atomic::rename_new`], never
//!   replacing a file). When the folders are on different volumes the file is copied
//!   to a temporary name, flushed, read back and compared byte for byte, renamed into
//!   place, and only then is the original removed; if the original cannot be removed
//!   the copy is taken out again, so a song is never in two places or in none.
//! - Crash safety: an entry is written to the index, marked with the operation in
//!   progress, before a file moves, and settled after. [`DeletedSongs::list`] (run at
//!   start-up and before every operation) repairs whatever an interruption left:
//!   an entry whose move never happened is dropped (the song is still in the Library),
//!   a finished move is settled, an interrupted copy is finished (of two identical
//!   copies the source is removed, the destination kept: removing the destination
//!   could race with another app instance removing the source; verifier finding),
//!   a song file without an entry is adopted, and a
//!   leftover temporary copy is removed. No path removes the only copy of a song.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::ApiError;
use crate::library::SongSummary;

/// The index file inside `Deleted/`.
pub const INDEX_FILE: &str = "index.json";
const INDEX_FORMAT: &str = "jukeoke-deleted-songs";
const INDEX_VERSION: u32 = 1;
/// Temporary copies older than this are leftovers of an interrupted copy (a younger
/// one may belong to another app instance copying right now).
const STALE_TEMP: Duration = Duration::from_secs(3600);

/// An operation in progress when the index was written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Pending {
    /// The file is moving from the Library (`original`) into `Deleted/`.
    Deleting,
    /// The file is moving back into the Library, to the song id `to`.
    Restoring { to: String },
}

/// One deleted song in the index.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    /// File name inside `Deleted/`; also the song's `deleted_id`.
    file: String,
    /// Where it was: its `song_id` (path relative to the Library, `/` separators).
    original: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration_s: f64,
    #[serde(default)]
    bytes: u64,
    /// When it was deleted, milliseconds since 1970 (UTC).
    #[serde(default)]
    deleted_ms: u64,
    /// The same instant as text, for people reading the file on the stick.
    #[serde(default)]
    deleted_utc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Pending>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    format: String,
    version: u32,
    songs: Vec<Entry>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl Default for Index {
    fn default() -> Self {
        Index {
            format: INDEX_FORMAT.into(),
            version: INDEX_VERSION,
            songs: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// A deleted song as the UI sees it (docs/APP_API.md `DeletedSong`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeletedSong {
    /// Its file name inside `Deleted/`: what restore and purge take.
    pub deleted_id: String,
    /// Its `song_id` before it was deleted.
    pub original_song_id: String,
    /// The `song_id` a restore gives it now: the original, or a free name next to it
    /// when that place is taken.
    pub restore_to: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_s: f64,
    pub file_bytes: u64,
    pub deleted_ms: u64,
}

/// `library_deleted_list()`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeletedList {
    /// Newest first.
    pub songs: Vec<DeletedSong>,
    pub count: usize,
    pub total_bytes: u64,
    /// The folder, for information.
    pub folder: String,
}

/// One song an operation could not handle, with the reason in plain words.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Failure {
    pub id: String,
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(id: &str, e: &ApiError) -> Failure {
        Failure {
            id: id.to_string(),
            code: e.code.clone(),
            message: e.message.clone(),
        }
    }
}

/// A song going to `Deleted/`.
pub struct ToDelete {
    pub song_id: String,
    pub path: PathBuf,
    pub summary: SongSummary,
}

/// A restored song: its `deleted_id` and the `song_id` it has now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Restored {
    pub deleted_id: String,
    pub song_id: String,
}

/// The result of a purge ("delete for good" / "Empty Deleted songs").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Purged {
    pub purged: Vec<String>,
    pub freed_bytes: u64,
    pub failed: Vec<Failure>,
}

pub struct DeletedSongs {
    dir: PathBuf,
    library: PathBuf,
    /// Serialises every operation (the index is read, changed and written whole).
    lock: Mutex<()>,
    /// Tests: behave as if `Deleted/` were on another volume than the Library.
    force_copy: AtomicBool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// A song id's path under `library`, or None when the id could leave the folder or
/// does not name a song file (the same rules as `Library::resolve`, without requiring
/// the file to exist).
fn library_path(library: &Path, song_id: &str) -> Option<PathBuf> {
    if song_id.is_empty() || song_id.contains('\0') || song_id.contains('\\') {
        return None;
    }
    let mut path = library.to_path_buf();
    for part in song_id.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return None;
        }
        path.push(part);
    }
    let name = path.file_name()?;
    jk_format::is_song_file_name(name).then_some(path)
}

/// `stem (n).ext` for n = 2, 3... ; the first one `free` accepts.
fn numbered(name: &str, mut free: impl FnMut(&str) -> bool) -> String {
    if free(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 2u32.. {
        let candidate = format!("{stem} ({n}){ext}");
        if free(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// True when the two files have the same length and the same bytes.
pub fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    let mut ba = vec![0u8; 1 << 20];
    let mut bb = vec![0u8; 1 << 20];
    loop {
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Windows cannot always move or remove a file that is open (it can when every
/// handle allows it, as Rust's do, but not when a virus scanner or another program
/// holds it). Waits until nobody has `path` open, by opening it with no sharing at all,
/// for at most `timeout`. True when it is free (or on other systems).
pub fn wait_until_closed(path: &Path, timeout: Duration) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let end = Instant::now() + timeout;
        loop {
            match fs::OpenOptions::new().read(true).share_mode(0).open(path) {
                Ok(f) => {
                    drop(f);
                    return true;
                }
                // 32 = ERROR_SHARING_VIOLATION: someone still has it open.
                Err(e) if e.raw_os_error() == Some(32) && Instant::now() < end => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(_) => return false,
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, timeout, Instant::now());
        true
    }
}

/// Removes a file; on Windows waits up to about 1 s for a scanner holding it.
fn remove_file_patiently(path: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if cfg!(windows) && matches!(e.raw_os_error(), Some(5) | Some(32)) && attempt < 50 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Copies `from` to `to` (which must not exist) on another volume: temporary name,
/// flush, same modification time, read back and compared, renamed into place; then the
/// original is removed. If the original cannot be removed, the copy is removed again
/// and the error returned: the song stays where it was.
fn copy_verify_move(from: &Path, to: &Path) -> io::Result<()> {
    let tmp = crate::fs_atomic::temp_path_for(to)?;
    let copied = (|| {
        let mut src = File::open(from)?;
        let modified = src.metadata()?.modified().ok();
        let mut dst = File::create_new(&tmp)?;
        io::copy(&mut src, &mut dst)?;
        dst.flush()?;
        if let Some(t) = modified {
            // Keeps the Library's "recently added" order after a restore.
            let _ = dst.set_modified(t);
        }
        dst.sync_all()?;
        drop(dst);
        drop(src);
        if !same_content(from, &tmp)? {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the copy does not match the original"));
        }
        crate::fs_atomic::rename_new(&tmp, to)
    })();
    if let Err(e) = copied {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = remove_file_patiently(from) {
        crate::applog::warn(&format!(
            "deleted songs: {} cannot be removed after copying it ({e}); taking the copy out again",
            from.display()
        ));
        let _ = fs::remove_file(to);
        return Err(e);
    }
    crate::fs_atomic::sync_dir(from.parent().unwrap_or(Path::new(".")));
    Ok(())
}

fn move_error(what: &str, e: &io::Error) -> ApiError {
    crate::applog::warn(&format!("deleted songs: {what}: {e}"));
    let in_use = cfg!(windows) && matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33));
    let message = if in_use {
        "The song file is open in another program (or a virus scanner is checking it). Close that program and try again.".to_string()
    } else {
        match e.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                "Jukeoke is not allowed to change the song file (the drive may be read-only: check the USB stick's lock switch).".to_string()
            }
            io::ErrorKind::StorageFull => "The drive is full, so the song could not be moved.".to_string(),
            io::ErrorKind::NotFound => "The song file is no longer there (it was moved or deleted).".to_string(),
            _ => format!("The song file could not be moved: {e}"),
        }
    };
    let code = if e.kind() == io::ErrorKind::NotFound { "not_found" } else if in_use { "in_use" } else { "io" };
    ApiError::new(code, message)
}

impl DeletedSongs {
    /// `dir` is `<root>/Deleted`, `library` is `<root>/Library`. Nothing is created
    /// until the first song is deleted.
    pub fn new(dir: PathBuf, library: PathBuf) -> DeletedSongs {
        DeletedSongs {
            dir,
            library,
            lock: Mutex::new(()),
            force_copy: AtomicBool::new(false),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Tests: makes every move copy, verify and remove, as across two volumes.
    #[doc(hidden)]
    pub fn simulate_cross_volume(&self, on: bool) {
        self.force_copy.store(on, Ordering::SeqCst);
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join(INDEX_FILE)
    }

    /// Moves a file to a name that must be free: a rename, or across volumes a
    /// verified copy followed by removing the original.
    fn move_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target name is taken"));
        }
        if !self.force_copy.load(Ordering::SeqCst) {
            match crate::fs_atomic::rename_new(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if crate::fs_atomic::is_cross_volume(&e) => {
                    crate::applog::info("deleted songs: the Library and Deleted are on different volumes; copying");
                }
                Err(e) => return Err(e),
            }
        }
        copy_verify_move(from, to)
    }

    fn read_index(&self) -> Index {
        let path = self.index_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Index::default(),
            Err(e) => {
                crate::applog::warn(&format!("deleted songs: cannot read {}: {e}", path.display()));
                return Index::default();
            }
        };
        match serde_json::from_slice::<Index>(&bytes) {
            Ok(i) => i,
            Err(e) => {
                // Keep it for a person to look at; the songs themselves are adopted
                // again from the folder (their original places are lost, not the songs).
                let bad = self.dir.join(format!("{INDEX_FILE}.bad"));
                crate::applog::warn(&format!("deleted songs: {} is unreadable ({e}); kept as {}", path.display(), bad.display()));
                let _ = fs::remove_file(&bad);
                let _ = fs::rename(&path, &bad);
                Index::default()
            }
        }
    }

    fn write_index(&self, idx: &Index) -> io::Result<()> {
        let mut json = serde_json::to_vec_pretty(idx).map_err(io::Error::other)?;
        json.push(b'\n');
        crate::fs_atomic::write_atomic(&self.index_path(), &json)
    }

    /// Reads the index and repairs what an interruption left (module docs). Writes it
    /// back only when something changed.
    fn load(&self) -> Index {
        let mut idx = self.read_index();
        if !self.dir.is_dir() {
            return idx;
        }
        let mut changed = false;
        let mut keep: Vec<Entry> = Vec::with_capacity(idx.songs.len());
        let mut names: HashSet<String> = HashSet::new();
        for mut e in std::mem::take(&mut idx.songs) {
            let ok_name = !e.file.is_empty()
                && !e.file.contains(['/', '\\', ':', '\0'])
                && jk_format::is_song_file_name(std::ffi::OsStr::new(&e.file));
            if !ok_name || names.contains(&lower(&e.file)) {
                changed = true;
                continue;
            }
            let here = self.dir.join(&e.file);
            let in_deleted = here.is_file();
            match e.pending.clone() {
                None => {
                    if !in_deleted {
                        crate::applog::info(&format!("deleted songs: {} is no longer in the folder; forgotten", e.file));
                        changed = true;
                        continue;
                    }
                }
                Some(p) => {
                    changed = true;
                    let other = match &p {
                        Pending::Deleting => library_path(&self.library, &e.original),
                        Pending::Restoring { to } => library_path(&self.library, to),
                    };
                    let other_exists = other.as_deref().is_some_and(Path::is_file);
                    if !in_deleted {
                        // Deleting: the move never happened (the song is still in the
                        // Library). Restoring: it did (the song is back).
                        crate::applog::info(&format!("deleted songs: interrupted {p:?} of {}: settled", e.file));
                        continue;
                    }
                    if other_exists {
                        // Both copies exist: a copy across volumes was cut off before
                        // the source was removed (or another app instance on the same
                        // folder is between its rename and that removal right now).
                        // Identical: finish the move by removing the SOURCE (the Library
                        // copy of a delete, the Deleted copy of a restore), never the
                        // destination: the other instance's own removal of the source
                        // then finds nothing to remove, and one copy always remains.
                        let other = other.unwrap_or_default();
                        match same_content(&here, &other) {
                            Ok(true) => {
                                let source = match p {
                                    Pending::Deleting => &other,
                                    Pending::Restoring { .. } => &here,
                                };
                                if fs::remove_file(source).is_ok() {
                                    crate::applog::info(&format!(
                                        "deleted songs: interrupted {p:?} of {}: finished (removed {})",
                                        e.file,
                                        source.display()
                                    ));
                                    if matches!(p, Pending::Restoring { .. }) {
                                        continue;
                                    }
                                    e.pending = None;
                                    names.insert(lower(&e.file));
                                    keep.push(e);
                                    continue;
                                }
                            }
                            _ => {
                                crate::applog::warn(&format!(
                                    "deleted songs: {} and {} differ; both kept",
                                    here.display(),
                                    other.display()
                                ));
                            }
                        }
                    }
                    e.pending = None;
                }
            }
            names.insert(lower(&e.file));
            keep.push(e);
        }
        // Song files nobody listed (a crash between the move and the index write of an
        // older version, or a file put there by hand): adopted.
        if let Ok(files) = jk_format::list_song_files(&self.dir, false) {
            for f in files {
                let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
                if names.contains(&lower(&name)) {
                    continue;
                }
                let s = crate::library::Library::new(self.dir.clone()).summary(&f);
                let now = now_ms();
                crate::applog::info(&format!("deleted songs: adopted {name}"));
                names.insert(lower(&name));
                keep.push(Entry {
                    file: name.clone(),
                    original: name,
                    title: s.title,
                    artist: s.artist,
                    album: s.album,
                    duration_s: s.duration_s,
                    bytes: s.file_bytes,
                    deleted_ms: now,
                    deleted_utc: jk_format::time::format_unix_utc((now / 1000) as i64),
                    pending: None,
                    extra: Map::new(),
                });
                changed = true;
            }
        }
        // Temporary copies of an interrupted copy (the original was never removed).
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.contains(".tmp-") && !name.starts_with('.') {
                    let old = ent
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_TEMP);
                    if old && ent.file_type().is_ok_and(|t| t.is_file()) && fs::remove_file(ent.path()).is_ok() {
                        crate::applog::info(&format!("deleted songs: removed the unfinished copy {name}"));
                    }
                }
            }
        }
        idx.songs = keep;
        if changed && let Err(e) = self.write_index(&idx) {
            crate::applog::warn(&format!("deleted songs: cannot write the index: {e}"));
        }
        idx
    }

    /// The song id a restore of `e` gives it now (`taken`: ids already promised in
    /// the same batch, lower case).
    fn restore_target(&self, e: &Entry, taken: &HashSet<String>) -> String {
        let original = if library_path(&self.library, &e.original).is_some() { e.original.clone() } else { e.file.clone() };
        let (folder, name) = match original.rsplit_once('/') {
            Some((f, n)) => (Some(f.to_string()), n.to_string()),
            None => (None, original.clone()),
        };
        let id_of = |n: &str| match &folder {
            Some(f) => format!("{f}/{n}"),
            None => n.to_string(),
        };
        let free_name = numbered(&name, |n| {
            let id = id_of(n);
            !taken.contains(&lower(&id))
                && library_path(&self.library, &id).is_some_and(|p| fs::symlink_metadata(p).is_err())
        });
        id_of(&free_name)
    }

    fn view(&self, e: &Entry) -> DeletedSong {
        DeletedSong {
            deleted_id: e.file.clone(),
            original_song_id: e.original.clone(),
            restore_to: self.restore_target(e, &HashSet::new()),
            title: e.title.clone(),
            artist: e.artist.clone(),
            album: e.album.clone(),
            duration_s: e.duration_s,
            file_bytes: e.bytes,
            deleted_ms: e.deleted_ms,
        }
    }

    fn list_of(&self, idx: &Index) -> DeletedList {
        let mut songs: Vec<DeletedSong> = idx.songs.iter().map(|e| self.view(e)).collect();
        songs.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms).then_with(|| a.deleted_id.cmp(&b.deleted_id)));
        DeletedList {
            count: songs.len(),
            total_bytes: songs.iter().map(|s| s.file_bytes).sum(),
            songs,
            folder: self.dir.display().to_string(),
        }
    }

    /// `library_deleted_list()`, after repairing anything an interruption left.
    pub fn list(&self) -> DeletedList {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let idx = self.load();
        self.list_of(&idx)
    }

    /// Moves songs into `Deleted/`. The caller has already stopped and closed the
    /// loaded song when it is one of them.
    pub fn delete(&self, songs: Vec<ToDelete>) -> (Vec<DeletedSong>, Vec<Failure>) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut failed = Vec::new();
        if songs.is_empty() {
            return (Vec::new(), failed);
        }
        if let Err(e) = fs::create_dir_all(&self.dir) {
            let err = ApiError::io("the Deleted songs folder", &e);
            return (Vec::new(), songs.iter().map(|s| Failure::new(&s.song_id, &err)).collect());
        }
        let mut idx = self.load();
        let mut taken: HashSet<String> = idx.songs.iter().map(|e| lower(&e.file)).collect();
        let now = now_ms();
        let mut plan: Vec<(usize, PathBuf)> = Vec::new();
        for s in &songs {
            let base = s
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Song.jksong".into());
            let name = numbered(&base, |n| !taken.contains(&lower(n)) && fs::symlink_metadata(self.dir.join(n)).is_err());
            taken.insert(lower(&name));
            let bytes = fs::metadata(&s.path).map(|m| m.len()).unwrap_or(s.summary.file_bytes);
            idx.songs.push(Entry {
                file: name,
                original: s.song_id.clone(),
                title: s.summary.title.clone(),
                artist: s.summary.artist.clone(),
                album: s.summary.album.clone(),
                duration_s: s.summary.duration_s,
                bytes,
                deleted_ms: now,
                deleted_utc: jk_format::time::format_unix_utc((now / 1000) as i64),
                pending: Some(Pending::Deleting),
                extra: Map::new(),
            });
            plan.push((idx.songs.len() - 1, s.path.clone()));
        }
        // The index first: if anything stops half-way, the next start knows where each
        // file came from.
        if let Err(e) = self.write_index(&idx) {
            let err = ApiError::io("the list of deleted songs (Deleted/index.json)", &e);
            return (Vec::new(), songs.iter().map(|s| Failure::new(&s.song_id, &err)).collect());
        }
        let mut moved = vec![false; idx.songs.len()];
        for ((i, path), s) in plan.iter().zip(&songs) {
            let to = self.dir.join(&idx.songs[*i].file);
            if !wait_until_closed(path, Duration::from_secs(2)) {
                crate::applog::warn(&format!("deleted songs: {} is still open somewhere; moving anyway", path.display()));
            }
            match self.move_file(path, &to) {
                Ok(()) => {
                    crate::applog::info(&format!("deleted songs: {} -> {}", s.song_id, to.display()));
                    moved[*i] = true;
                }
                Err(e) => failed.push(Failure::new(&s.song_id, &move_error(&s.song_id, &e))),
            }
        }
        let mut deleted = Vec::new();
        let mut kept = Vec::with_capacity(idx.songs.len());
        for (i, mut e) in std::mem::take(&mut idx.songs).into_iter().enumerate() {
            if plan.iter().any(|(j, _)| *j == i) {
                if !moved[i] {
                    // Not moved: the song is still in the Library; no entry.
                    continue;
                }
                e.pending = None;
                deleted.push(self.view(&e));
            }
            kept.push(e);
        }
        idx.songs = kept;
        if let Err(e) = self.write_index(&idx) {
            // The next `load` settles the entries from the files on disk.
            crate::applog::warn(&format!("deleted songs: cannot update the index after moving: {e}"));
        }
        (deleted, failed)
    }

    /// Moves songs back into the Library: to their original place, or a free name
    /// next to it when that is taken (missing folders are made again).
    pub fn restore(&self, ids: &[String]) -> (Vec<Restored>, Vec<Failure>) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut idx = self.load();
        let mut failed = Vec::new();
        let mut taken: HashSet<String> = HashSet::new();
        let mut plan: Vec<(usize, String)> = Vec::new();
        for id in ids {
            let Some(i) = idx.songs.iter().position(|e| &e.file == id || lower(&e.file) == lower(id)) else {
                failed.push(Failure::new(id, &ApiError::new("not_found", "This song is no longer in Deleted songs.")));
                continue;
            };
            if plan.iter().any(|(j, _)| *j == i) {
                continue;
            }
            let to = self.restore_target(&idx.songs[i], &taken);
            taken.insert(lower(&to));
            idx.songs[i].pending = Some(Pending::Restoring { to: to.clone() });
            plan.push((i, to));
        }
        if plan.is_empty() {
            return (Vec::new(), failed);
        }
        if let Err(e) = self.write_index(&idx) {
            let err = ApiError::io("the list of deleted songs (Deleted/index.json)", &e);
            failed.extend(plan.iter().map(|(i, _)| Failure::new(&idx.songs[*i].file, &err)));
            return (Vec::new(), failed);
        }
        let mut restored = Vec::new();
        let mut done = vec![false; idx.songs.len()];
        for (i, to) in &plan {
            let e = &idx.songs[*i];
            let from = self.dir.join(&e.file);
            let Some(dest) = library_path(&self.library, to) else { continue };
            let result = dest
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| self.move_file(&from, &dest));
            match result {
                Ok(()) => {
                    crate::applog::info(&format!("deleted songs: restored {} -> {to}", e.file));
                    done[*i] = true;
                    restored.push(Restored {
                        deleted_id: e.file.clone(),
                        song_id: to.clone(),
                    });
                }
                Err(err) => failed.push(Failure::new(&e.file, &move_error(&e.file, &err))),
            }
        }
        let mut kept = Vec::with_capacity(idx.songs.len());
        for (i, mut e) in std::mem::take(&mut idx.songs).into_iter().enumerate() {
            if done[i] {
                continue;
            }
            if matches!(e.pending, Some(Pending::Restoring { .. })) {
                e.pending = None;
            }
            kept.push(e);
        }
        idx.songs = kept;
        if let Err(e) = self.write_index(&idx) {
            crate::applog::warn(&format!("deleted songs: cannot update the index after restoring: {e}"));
        }
        (restored, failed)
    }

    /// Deletes songs for good: `Some(ids)`, or every song in `Deleted/` with `None`.
    pub fn purge(&self, ids: Option<&[String]>) -> Purged {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut idx = self.load();
        let mut failed = Vec::new();
        let mut chosen = vec![false; idx.songs.len()];
        match ids {
            None => chosen.iter_mut().for_each(|c| *c = true),
            Some(ids) => {
                for id in ids {
                    match idx.songs.iter().position(|e| &e.file == id || lower(&e.file) == lower(id)) {
                        Some(i) => chosen[i] = true,
                        None => failed.push(Failure::new(id, &ApiError::new("not_found", "This song is no longer in Deleted songs."))),
                    }
                }
            }
        }
        let mut purged = Vec::new();
        let mut freed = 0u64;
        let mut kept = Vec::with_capacity(idx.songs.len());
        for (i, e) in std::mem::take(&mut idx.songs).into_iter().enumerate() {
            if !chosen[i] {
                kept.push(e);
                continue;
            }
            let path = self.dir.join(&e.file);
            let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            match remove_file_patiently(&path) {
                Ok(()) => {
                    crate::applog::info(&format!("deleted songs: deleted {} for good", e.file));
                    freed += bytes;
                    purged.push(e.file);
                }
                Err(err) => {
                    failed.push(Failure::new(&e.file, &move_error(&e.file, &err)));
                    kept.push(e);
                }
            }
        }
        idx.songs = kept;
        if (!purged.is_empty() || ids.is_none())
            && self.dir.is_dir()
            && let Err(e) = self.write_index(&idx)
        {
            crate::applog::warn(&format!("deleted songs: cannot update the index after deleting for good: {e}"));
        }
        Purged {
            purged,
            freed_bytes: freed,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use crate::testsong::{self, ToneSong};

    struct Rig {
        root: PathBuf,
        lib: Library,
        bin: DeletedSongs,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn rig(name: &str) -> Rig {
        let root = std::env::temp_dir().join(format!(
            "jukeoke-deleted-test-{}-{name}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Library/Pop")).unwrap();
        let short = ToneSong { seconds: 1.0, ..Default::default() };
        testsong::write(&root.join("Library/Artist - One.jksong"), short).unwrap();
        testsong::write(&root.join("Library/Pop/Artist - Two.jksong"), short).unwrap();
        testsong::write(&root.join("Library/Pop/Artist - One.jksong"), ToneSong { seconds: 2.0, ..Default::default() }).unwrap();
        let lib = Library::new(root.join("Library"));
        let bin = DeletedSongs::new(root.join("Deleted"), root.join("Library"));
        Rig { root, lib, bin }
    }

    fn del(r: &Rig, ids: &[&str]) -> (Vec<DeletedSong>, Vec<Failure>) {
        let items = ids
            .iter()
            .map(|id| {
                let path = r.lib.resolve(id).unwrap();
                ToDelete {
                    song_id: id.to_string(),
                    summary: r.lib.summary(&path),
                    path,
                }
            })
            .collect();
        r.bin.delete(items)
    }

    fn library_ids(r: &Rig) -> Vec<String> {
        r.lib.list().unwrap().songs.into_iter().map(|s| s.song_id).collect()
    }

    fn deleted_files(r: &Rig) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(r.root.join("Deleted"))
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    fn index_json(r: &Rig) -> Value {
        serde_json::from_slice(&fs::read(r.root.join("Deleted/index.json")).unwrap()).unwrap()
    }

    #[test]
    fn delete_restore_and_purge_round_trip() {
        let r = rig("roundtrip");
        let before = fs::read(r.root.join("Library/Pop/Artist - Two.jksong")).unwrap();
        let (d, f) = del(&r, &["Pop/Artist - Two.jksong"]);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].deleted_id, "Artist - Two.jksong");
        assert_eq!(d[0].original_song_id, "Pop/Artist - Two.jksong");
        assert_eq!(d[0].restore_to, "Pop/Artist - Two.jksong");
        assert_eq!((d[0].title.as_str(), d[0].artist.as_str()), ("Test Tone", "Jukeoke"));
        assert_eq!(d[0].file_bytes, before.len() as u64);
        assert!(d[0].deleted_ms > 0);
        // Moved, not copied; the scan no longer lists it; the index says where it was.
        assert!(!r.root.join("Library/Pop/Artist - Two.jksong").exists());
        assert_eq!(fs::read(r.root.join("Deleted/Artist - Two.jksong")).unwrap(), before);
        assert!(!library_ids(&r).contains(&"Pop/Artist - Two.jksong".to_string()));
        let j = index_json(&r);
        assert_eq!(j["format"], "jukeoke-deleted-songs");
        assert_eq!(j["songs"][0]["original"], "Pop/Artist - Two.jksong");
        assert!(j["songs"][0].get("pending").is_none(), "settled: {j}");
        assert!(j["songs"][0]["deleted_utc"].as_str().unwrap().ends_with('Z'));
        let l = r.bin.list();
        assert_eq!((l.count, l.total_bytes), (1, before.len() as u64));

        // Restore: back to its place, byte for byte, gone from the index.
        let (back, f) = r.bin.restore(&["Artist - Two.jksong".to_string()]);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(back, [Restored { deleted_id: "Artist - Two.jksong".into(), song_id: "Pop/Artist - Two.jksong".into() }]);
        assert_eq!(fs::read(r.root.join("Library/Pop/Artist - Two.jksong")).unwrap(), before);
        assert_eq!(r.bin.list().count, 0);
        assert_eq!(deleted_files(&r), ["index.json"]);

        // Purge one, then empty the rest.
        del(&r, &["Pop/Artist - Two.jksong", "Artist - One.jksong"]);
        assert_eq!(deleted_files(&r), ["Artist - One.jksong", "Artist - Two.jksong", "index.json"]);
        let p = r.bin.purge(Some(&["Artist - Two.jksong".to_string()]));
        assert_eq!(p.purged, ["Artist - Two.jksong"]);
        assert_eq!(p.freed_bytes, before.len() as u64);
        assert_eq!(deleted_files(&r), ["Artist - One.jksong", "index.json"]);
        let p = r.bin.purge(Some(&["nope.jksong".to_string()]));
        assert_eq!(p.failed[0].code, "not_found");
        let p = r.bin.purge(None);
        assert_eq!(p.purged, ["Artist - One.jksong"]);
        assert_eq!(deleted_files(&r), ["index.json"]);
        assert_eq!(r.bin.list().count, 0);
        assert_eq!(library_ids(&r), ["Pop/Artist - One.jksong"]);
    }

    #[test]
    fn name_clashes_get_numbered_names_both_ways() {
        let r = rig("clash");
        // Two songs with the same file name from different folders.
        let (d, _) = del(&r, &["Artist - One.jksong", "Pop/Artist - One.jksong"]);
        let names: Vec<&str> = d.iter().map(|s| s.deleted_id.as_str()).collect();
        assert_eq!(names, ["Artist - One.jksong", "Artist - One (2).jksong"]);
        // Letter case does not make a name free (exFAT, NTFS, APFS).
        testsong::write(&r.root.join("Library/artist - one.jksong"), ToneSong { seconds: 1.0, ..Default::default() }).unwrap();
        let (d, _) = del(&r, &["artist - one.jksong"]);
        assert_eq!(d[0].deleted_id, "artist - one (3).jksong");
        // A place taken again since: restore beside it; a vanished folder is made again.
        testsong::write(&r.root.join("Library/Artist - One.jksong"), ToneSong { seconds: 1.0, ..Default::default() }).unwrap();
        fs::remove_dir_all(r.root.join("Library/Pop")).unwrap();
        let l = r.bin.list();
        let first = l.songs.iter().find(|s| s.deleted_id == "Artist - One.jksong").unwrap();
        assert_eq!(first.restore_to, "Artist - One (2).jksong");
        let ids: Vec<String> = ["Artist - One.jksong", "Artist - One (2).jksong", "artist - one (3).jksong"].map(String::from).to_vec();
        let (back, f) = r.bin.restore(&ids);
        assert!(f.is_empty(), "{f:?}");
        let to: Vec<&str> = back.iter().map(|b| b.song_id.as_str()).collect();
        // "artist - one.jksong" is taken by "Artist - One.jksong" where letter case is
        // ignored (Windows, macOS, exFAT), and " (2)" by the first restore.
        let ignores_case = r.root.join("Library/ARTIST - ONE.jksong").exists();
        let third = if ignores_case { "artist - one (3).jksong" } else { "artist - one.jksong" };
        assert_eq!(to, ["Artist - One (2).jksong", "Pop/Artist - One.jksong", third]);
        let mut ids = library_ids(&r);
        ids.sort();
        assert_eq!(ids, ["Artist - One (2).jksong", "Artist - One.jksong", "Pop/Artist - One.jksong", third]);
    }

    #[test]
    fn interrupted_operations_are_repaired_without_losing_a_file() {
        let r = rig("crash");
        let dir = r.root.join("Deleted");
        fs::create_dir_all(&dir).unwrap();
        let one = r.root.join("Library/Artist - One.jksong");
        let two = r.root.join("Library/Pop/Artist - Two.jksong");
        let bytes_one = fs::read(&one).unwrap();
        // (1) Crash after the index was written, before the move: entry dropped, the
        //     song is still in the Library.
        // (2) Crash after a copy across volumes was renamed in, before the original was
        //     removed: the move is finished (the identical source in the Library goes,
        //     the copy in Deleted/ stays), never the other way round: removing the copy
        //     could race with another instance removing the source.
        fs::copy(&two, dir.join("Artist - Two.jksong")).unwrap();
        let bytes_two = fs::read(&two).unwrap();
        // (7) The same for a restore: the identical source in Deleted/ goes, the
        //     Library keeps the song.
        let three = r.root.join("Library/Pop/Artist - One.jksong");
        let bytes_three = fs::read(&three).unwrap();
        fs::write(dir.join("Back - Twice.jksong"), &bytes_three).unwrap();
        // (3) A finished move whose index update was lost: settled.
        testsong::write(&dir.join("Moved - Away.jksong"), ToneSong { seconds: 1.0, ..Default::default() }).unwrap();
        // (4) A song file nobody listed: adopted, with its title from the file.
        testsong::write(&dir.join("Stray - Song.jksong"), ToneSong { seconds: 1.0, ..Default::default() }).unwrap();
        // (5) An interrupted restore whose move finished: entry dropped.
        // (6) An old unfinished copy: removed. A young one is left (another instance).
        let old_tmp = dir.join("X.jksong.tmp-1-2-00000000");
        fs::write(&old_tmp, b"partial").unwrap();
        File::options().write(true).open(&old_tmp).unwrap().set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        let young_tmp = dir.join("Y.jksong.tmp-1-3-00000000");
        fs::write(&young_tmp, b"partial").unwrap();
        let entry = |file: &str, original: &str, pending: Value| {
            let mut e = serde_json::json!({"file": file, "original": original, "title": "T", "artist": "A", "bytes": 1, "deleted_ms": 5});
            if !pending.is_null() {
                e["pending"] = pending;
            }
            e
        };
        let idx = serde_json::json!({
            "format": "jukeoke-deleted-songs", "version": 1, "future_key": [1, 2],
            "songs": [
                entry("Artist - One.jksong", "Artist - One.jksong", serde_json::json!({"op": "deleting"})),
                entry("Artist - Two.jksong", "Pop/Artist - Two.jksong", serde_json::json!({"op": "deleting"})),
                entry("Moved - Away.jksong", "Pop/Moved - Away.jksong", serde_json::json!({"op": "deleting"})),
                entry("Back - Home.jksong", "Back - Home.jksong", serde_json::json!({"op": "restoring", "to": "Back - Home.jksong"})),
                entry("Gone - By Hand.jksong", "Gone - By Hand.jksong", Value::Null),
                entry("Back - Twice.jksong", "Pop/Artist - One.jksong", serde_json::json!({"op": "restoring", "to": "Pop/Artist - One.jksong"})),
            ]
        });
        fs::write(dir.join("index.json"), serde_json::to_vec(&idx).unwrap()).unwrap();

        let l = r.bin.list();
        let ids: Vec<&str> = l.songs.iter().map(|s| s.deleted_id.as_str()).collect();
        assert_eq!(l.count, 3, "{ids:?}");
        assert!(ids.contains(&"Moved - Away.jksong") && ids.contains(&"Stray - Song.jksong"), "{ids:?}");
        assert_eq!(fs::read(&one).unwrap(), bytes_one, "(1) untouched");
        assert!(ids.contains(&"Artist - Two.jksong"), "(2) listed as deleted: {ids:?}");
        assert!(!two.exists(), "(2) the source in the Library is gone");
        assert_eq!(fs::read(dir.join("Artist - Two.jksong")).unwrap(), bytes_two, "(2) the copy in Deleted/ stays");
        let two_entry = l.songs.iter().find(|s| s.deleted_id == "Artist - Two.jksong").unwrap();
        assert_eq!(two_entry.original_song_id, "Pop/Artist - Two.jksong");
        assert_eq!(fs::read(&three).unwrap(), bytes_three, "(7) the Library keeps the restored song");
        assert!(!dir.join("Back - Twice.jksong").exists(), "(7) the source in Deleted/ is gone");
        assert!(!ids.contains(&"Back - Twice.jksong"), "{ids:?}");
        let stray = l.songs.iter().find(|s| s.deleted_id == "Stray - Song.jksong").unwrap();
        assert_eq!((stray.title.as_str(), stray.restore_to.as_str()), ("Test Tone", "Stray - Song.jksong"));
        assert!(!old_tmp.exists(), "(6) old temp removed");
        assert!(young_tmp.exists(), "(6) young temp kept");
        let j = index_json(&r);
        assert_eq!(j["future_key"], serde_json::json!([1, 2]), "unknown keys kept");
        assert!(j["songs"].as_array().unwrap().iter().all(|e| e.get("pending").is_none()), "{j}");
        let moved = j["songs"].as_array().unwrap().iter().find(|e| e["file"] == "Moved - Away.jksong").unwrap();
        assert_eq!(moved["original"], "Pop/Moved - Away.jksong", "(3) the original place is remembered");

        // A damaged index is kept aside; the songs are adopted again.
        fs::write(dir.join("index.json"), b"{ not json").unwrap();
        let l = r.bin.list();
        assert_eq!(l.count, 3);
        assert!(dir.join("index.json.bad").is_file());
    }

    #[test]
    fn across_volumes_the_copy_is_verified_before_the_original_goes() {
        let r = rig("xvol");
        r.bin.simulate_cross_volume(true);
        let src = r.root.join("Library/Pop/Artist - Two.jksong");
        let before = fs::read(&src).unwrap();
        let mtime = fs::metadata(&src).unwrap().modified().unwrap();
        let (d, f) = del(&r, &["Pop/Artist - Two.jksong"]);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(d.len(), 1);
        assert!(!src.exists());
        let moved = r.root.join("Deleted/Artist - Two.jksong");
        assert_eq!(fs::read(&moved).unwrap(), before);
        assert_eq!(fs::metadata(&moved).unwrap().modified().unwrap(), mtime, "modification time kept");
        assert_eq!(deleted_files(&r), ["Artist - Two.jksong", "index.json"], "no temporary copy left");
        let (back, f) = r.bin.restore(&["Artist - Two.jksong".to_string()]);
        assert!(f.is_empty() && back.len() == 1, "{f:?}");
        assert_eq!(fs::read(&src).unwrap(), before);
        assert_eq!(deleted_files(&r), ["index.json"]);
    }

    /// Windows: a file another program holds (here: this test, without delete
    /// sharing) can be copied but not removed; the copy is taken out again and the
    /// song stays in the Library. Held with no sharing at all, nothing moves.
    #[cfg(windows)]
    #[test]
    fn a_move_that_fails_half_way_leaves_the_song_where_it_was() {
        use std::os::windows::fs::OpenOptionsExt;
        let r = rig("halfway");
        r.bin.simulate_cross_volume(true);
        let src = r.root.join("Library/Pop/Artist - Two.jksong");
        let before = fs::read(&src).unwrap();
        // FILE_SHARE_READ only: our copy can read it, nobody can remove it.
        let hold = fs::OpenOptions::new().read(true).share_mode(1).open(&src).unwrap();
        let (d, f) = del(&r, &["Pop/Artist - Two.jksong"]);
        assert!(d.is_empty());
        assert_eq!(f[0].code, "in_use", "{f:?}");
        drop(hold);
        assert_eq!(fs::read(&src).unwrap(), before);
        assert_eq!(deleted_files(&r), ["index.json"], "the copy was taken out again");
        assert_eq!(r.bin.list().count, 0);
        // No sharing at all, on the same volume: the rename is refused, nothing moves.
        r.bin.simulate_cross_volume(false);
        let hold = fs::OpenOptions::new().read(true).share_mode(0).open(&src).unwrap();
        let t0 = Instant::now();
        let (d, f) = del(&r, &["Pop/Artist - Two.jksong"]);
        drop(hold);
        assert!(d.is_empty() && f.len() == 1, "{f:?}");
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert_eq!(fs::read(&src).unwrap(), before);
        assert_eq!(r.bin.list().count, 0);
        // Released: it goes.
        let (d, f) = del(&r, &["Pop/Artist - Two.jksong"]);
        assert!(f.is_empty() && d.len() == 1, "{f:?}");
    }

    #[test]
    fn the_library_scan_never_lists_deleted_songs() {
        let r = rig("scan");
        del(&r, &["Artist - One.jksong"]);
        let ids = library_ids(&r);
        assert_eq!(ids, ["Pop/Artist - One.jksong", "Pop/Artist - Two.jksong"]);
        assert!(r.lib.resolve("../Deleted/Artist - One.jksong").is_err());
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("A.jksong", |_| true), "A.jksong");
        let taken = ["A.jksong", "A (2).jksong"];
        assert_eq!(numbered("A.jksong", |n| !taken.contains(&n)), "A (3).jksong");
        assert_eq!(numbered("noext", |n| n != "noext"), "noext (2)");
    }
}

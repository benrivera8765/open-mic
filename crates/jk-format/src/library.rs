//! Finding song files in a library folder.
//!
//! Skips dotfiles (which covers macOS `._*` AppleDouble files that macOS sprinkles on
//! exFAT sticks) and `*.tmp-*` files (unfinished atomic writes, section 7).

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::SONG_EXTENSION;

/// True for a name a library scan should list: ends in `.jksong` (any case), is not a
/// dotfile / AppleDouble file, and is not an unfinished temporary file.
pub fn is_song_file_name(name: &OsStr) -> bool {
    // Lossy: a name that is not valid Unicode (possible on Windows) still lists when it
    // ends in ".jksong"; only the ASCII suffix and prefix matter here.
    let name = name.to_string_lossy();
    if name.starts_with('.') || is_temp_file_name(&name) {
        return false;
    }
    // Compare bytes, not a `str` slice: `name.len() - ext.len()` can fall inside a
    // multi-byte character ("naïve.txt", "日本の歌.flac"), and slicing a `str` there panics.
    let (n, ext) = (name.as_bytes(), format!(".{SONG_EXTENSION}"));
    let ext = ext.as_bytes();
    n.len() > ext.len() && n[n.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// True for names produced by the atomic writer before the rename (`*.tmp-*`).
pub fn is_temp_file_name(name: &str) -> bool {
    name.contains(".tmp-")
}

/// Lists the song files in `dir`, sorted by file name (case-insensitively). Only
/// regular files are listed. With `recursive`, sub-folders are searched too, except
/// hidden ones (names starting with `.`).
pub fn list_song_files(dir: impl AsRef<Path>, recursive: bool) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(dir.as_ref(), recursive, &mut out)?;
    out.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    Ok(out)
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        // file_type() does not follow symlinks; metadata() does. exFAT has no symlinks,
        // but an internal drive might.
        let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
        if meta.is_file() && is_song_file_name(&name) {
            out.push(entry.path());
        } else if recursive && meta.is_dir() && !name.to_string_lossy().starts_with('.') {
            // An unreadable sub-folder should not hide the rest of the library.
            let _ = walk(&entry.path(), true, out);
        }
    }
    Ok(())
}

/// Leftover temporary files from interrupted writes in `dir` (not recursive). The app
/// may delete them when no write is in progress.
pub fn stale_temp_files(dir: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
    let ext = format!(".{SONG_EXTENSION}.tmp-");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('.') && name.to_lowercase().contains(&ext) && entry.file_type()?.is_file() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let ok = |s: &str| is_song_file_name(OsStr::new(s));
        assert!(ok("Artist - Title.jksong"));
        assert!(ok("LOUD.JKSONG"));
        assert!(ok("café – naïve.jksong"));
        assert!(!ok("._Artist - Title.jksong"));
        assert!(!ok(".hidden.jksong"));
        assert!(!ok("Artist - Title.jksong.tmp-0123abcd"));
        assert!(!ok("x.tmp-1.jksong"));
        assert!(!ok("song.flac"));
        assert!(!ok(".jksong"));
        assert!(!ok("jksong"));
        // Non-ASCII names whose last 7 bytes start inside a multi-byte character used
        // to panic the whole library scan.
        assert!(!ok("naïve.txt"));
        assert!(!ok("日本の歌.flac"));
        assert!(!ok("ééééé"));
        assert!(ok("日本の歌.jksong"));
        assert!(ok("Ünïcödé – 歌.JKSong"));
    }
}

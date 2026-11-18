// SPDX-License-Identifier: GPL-3.0-or-later
//! Atomic file writes (WORK_ORDER section 7, trap 13: USB sticks get pulled out).
//!
//! 1. The data goes to a temporary file in the same folder, with a name nobody else
//!    can be using at the same moment (`<name>.tmp-<pid>-<counter>-<random>`), so two
//!    threads or two app instances never write into the same temp file.
//! 2. The temp file is flushed to the device (`fsync` / `FlushFileBuffers`).
//! 3. It is renamed over the real name. On Windows the rename is a `MoveFileExW` with
//!    `MOVEFILE_WRITE_THROUGH`, which returns only after the rename itself has been
//!    flushed to disk (Windows has no directory fsync; this is its equivalent). On
//!    Unix the parent directory is fsynced after the rename, so the new directory
//!    entry survives a power cut or a pulled stick.
//!
//! A reader therefore sees either the old file or the complete new one, never half of
//! each. `*.tmp-*` names are ignored by library scans (jk_format::library).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path` atomically (unique temp file + fsync + rename + directory
/// flush where the platform supports it).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent folder"))?;
    if !dir.as_os_str().is_empty() {
        fs::create_dir_all(dir)?;
    }
    let tmp = temp_path_for(path)?;

    let result = (|| {
        let mut file = File::create_new(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_durable(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A temporary name next to `path` that no other writer can be using right now:
/// process id, a per-process counter and 32 bits from the OS-seeded hasher.
pub fn temp_path_for(path: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut tmp = name.to_os_string();
    tmp.push(format!(
        ".tmp-{}-{}-{:08x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        random_u32()
    ));
    Ok(path.with_file_name(tmp))
}

/// 32 bits that differ between processes and calls (std's `RandomState` is seeded
/// from the OS random number generator). Not for cryptography.
pub fn random_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    h.finish() as u32
}

/// Renames `from` over `to` (replacing it) and makes the rename durable.
pub fn rename_durable(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        use windows::core::PCWSTR;
        let wide = |p: &Path| -> Vec<u16> {
            p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
        };
        let (a, b) = (wide(from), wide(to));
        // A virus scanner, the search indexer or a concurrent reader can hold the
        // target for a moment (ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION). Retry
        // briefly instead of failing a settings save over it.
        let mut attempt = 0;
        loop {
            // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive the call.
            let r = unsafe {
                MoveFileExW(
                    PCWSTR(a.as_ptr()),
                    PCWSTR(b.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            match r {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let code = e.code().0 & 0xFFFF;
                    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION
                    if (code == 5 || code == 32) && attempt < 50 {
                        attempt += 1;
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    return Err(io::Error::from_raw_os_error(code));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)?;
        sync_dir(to.parent().unwrap_or(Path::new(".")));
        Ok(())
    }
}

/// Moves `from` to `to`, a name that must not exist yet (never replaces a file), and
/// makes the move durable. Only within one volume: across volumes it fails with
/// [`is_cross_volume`] true (Windows `ERROR_NOT_SAME_DEVICE`, Unix `EXDEV`), and the
/// caller copies instead (src/deleted.rs). On Windows a virus scanner or the search
/// indexer holding the file for a moment is waited for (up to about 2 s).
pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        use windows::core::PCWSTR;
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
        let (a, b) = (wide(from), wide(to));
        let mut attempt = 0;
        loop {
            // No MOVEFILE_REPLACE_EXISTING (an existing `to` is an error, 80 / 183) and no
            // MOVEFILE_COPY_ALLOWED (another volume is an error, 17), so this is a rename.
            // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive the call.
            let r = unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVEFILE_WRITE_THROUGH) };
            match r {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let code = e.code().0 & 0xFFFF;
                    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION
                    if (code == 5 || code == 32) && attempt < 100 {
                        attempt += 1;
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        continue;
                    }
                    return Err(io::Error::from_raw_os_error(code));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        // std's rename replaces silently; the callers hold their own lock and pick
        // free names, so this check only guards against a programming error.
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target name is taken"));
        }
        fs::rename(from, to)?;
        sync_dir(to.parent().unwrap_or(Path::new(".")));
        if from.parent() != to.parent() {
            sync_dir(from.parent().unwrap_or(Path::new(".")));
        }
        Ok(())
    }
}

/// True for the error a rename gives when the two names are on different volumes.
pub fn is_cross_volume(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::CrossesDevices {
        return true;
    }
    // ERROR_NOT_SAME_DEVICE (Windows) / EXDEV (Linux, macOS).
    if cfg!(windows) { e.raw_os_error() == Some(17) } else { e.raw_os_error() == Some(18) }
}

/// Flushes a directory's entries to disk (Unix). Errors are ignored: some file
/// systems (exFAT through some drivers) do not support fsync on a directory, and the
/// file data itself was already flushed.
#[cfg(not(windows))]
pub fn sync_dir(dir: &Path) {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

/// No-op on Windows: [`rename_durable`] uses `MOVEFILE_WRITE_THROUGH` instead.
#[cfg(windows)]
pub fn sync_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jukeoke-atomic-test-{}-{name}-{:08x}",
            std::process::id(),
            random_u32()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn replaces_existing_file_and_leaves_no_temp() {
        let dir = scratch("replace");
        let path = dir.join("settings.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "temp file left behind: {names:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn temp_names_are_unique_and_ignored_by_library_scans() {
        let p = Path::new("x/Song.jksong");
        let a = temp_path_for(p).unwrap();
        let b = temp_path_for(p).unwrap();
        assert_ne!(a, b);
        assert!(!jk_format::is_song_file_name(a.file_name().unwrap()));
    }

    #[test]
    fn concurrent_writers_never_collide() {
        // Eight threads hammer the same file; every write must succeed (a shared temp
        // name would make some of them fail or rename someone else's half file).
        let dir = scratch("concurrent");
        let path = dir.join("settings.json");
        std::thread::scope(|s| {
            for t in 0..8u8 {
                let path = path.clone();
                s.spawn(move || {
                    for i in 0..25u8 {
                        let body = vec![t; 100 + i as usize];
                        write_atomic(&path, &body).unwrap();
                    }
                });
            }
        });
        let body = fs::read(&path).unwrap();
        assert!(body.iter().all(|&b| b == body[0]), "mixed content");
        let left = fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, 1, "temp files left behind");
        fs::remove_dir_all(dir).unwrap();
    }
}

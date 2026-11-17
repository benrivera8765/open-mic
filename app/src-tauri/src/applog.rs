// SPDX-License-Identifier: GPL-3.0-or-later
//! A tiny append-only log: `<Settings>/logs/app.log`, one line per event with a UTC
//! timestamp. Plain text so the owner can read it, small (rotated at 1 MiB, one old
//! copy kept) so it never fills a stick.
//!
//! Never called from the audio callback (that lives in jk-engine and does not log).
//! Before [`init`] runs (or in unit tests) lines go to stderr only.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Rotate when the log grows past this.
const MAX_BYTES: u64 = 1024 * 1024;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
/// Serialises writers so lines never interleave.
static LOCK: Mutex<()> = Mutex::new(());

/// Directs the log to `<logs_dir>/app.log`. Later calls are ignored.
pub fn init(logs_dir: &Path) {
    let _ = std::fs::create_dir_all(logs_dir);
    let _ = LOG_PATH.set(logs_dir.join("app.log"));
}

/// Where the log goes, once initialised.
pub fn path() -> Option<&'static Path> {
    LOG_PATH.get().map(PathBuf::as_path)
}

fn write(level: &str, msg: &str) {
    let line = format!("{} {level} {msg}\n", jk_format::time::utc_now_iso8601());
    if cfg!(debug_assertions) || LOG_PATH.get().is_none() {
        eprint!("{line}");
    }
    let Some(path) = LOG_PATH.get() else { return };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

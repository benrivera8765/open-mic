// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared helpers for the app crate's integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A fresh, empty temporary folder.
pub fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "jukeoke-it-{}-{name}-{:08x}",
        std::process::id(),
        jukeoke_lib::fs_atomic::random_u32()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The repository root.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// tests/fixtures/generated/tiny.jksong (4 s, cover, lyrics, gain > 1 stem).
pub fn tiny_song() -> PathBuf {
    repo().join("tests/fixtures/generated/tiny.jksong")
}

/// Polls `f` until it returns Some or `timeout` passes.
pub fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A Python 3 interpreter for the fake Studio: `$JK_TEST_PYTHON`, the dev Studio
/// venv, then `python3` / `python` on PATH. `None` when there is none (the Studio
/// tests then say so and pass vacuously - CI images all have Python).
pub fn python() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("JK_TEST_PYTHON") {
        candidates.push(p.into());
    }
    if let Some(b) = std::env::var_os("JK_BUILD") {
        let b = PathBuf::from(b);
        candidates.push(b.join("studio-venv/Scripts/python.exe"));
        candidates.push(b.join("studio-venv/bin/python"));
    }
    candidates.push("python3".into());
    candidates.push("python".into());
    candidates.into_iter().find(|c| {
        std::process::Command::new(c)
            .args(["-c", "import sys; assert sys.version_info >= (3, 9)"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

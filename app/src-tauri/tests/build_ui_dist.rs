// SPDX-License-Identifier: GPL-3.0-or-later
//! The build script's app/ui/dist rule (build_support/ui_dist.rs, the same file
//! build.rs uses): release builds fail without a real UI, debug builds get a stub.

mod common;

#[path = "../build_support/ui_dist.rs"]
mod ui_dist;

use ui_dist::{UiDist, ensure_ui_dist};

#[test]
fn release_fails_without_a_real_ui_and_debug_gets_a_stub() {
    let root = common::scratch("uidist");
    let dist = root.join("dist");

    // Release, nothing there: refused, and nothing is written.
    let err = ensure_ui_dist(&dist, true, false).unwrap_err();
    assert!(err.contains("is missing"), "{err}");
    assert!(!dist.exists());

    // Debug: the stub is written...
    assert_eq!(ensure_ui_dist(&dist, false, false).unwrap(), UiDist::StubWritten);
    assert!(std::fs::read_to_string(dist.join("index.html")).unwrap().contains(ui_dist::STUB_MARKER));
    // ...and a later release build still refuses it.
    let err = ensure_ui_dist(&dist, true, false).unwrap_err();
    assert!(err.contains("holds only the stub page"), "{err}");
    // Unless explicitly allowed.
    assert_eq!(ensure_ui_dist(&dist, true, true).unwrap(), UiDist::StubKept);

    // A real UI passes everywhere.
    std::fs::write(dist.join("index.html"), "<!doctype html><title>Jukeoke</title>").unwrap();
    assert_eq!(ensure_ui_dist(&dist, true, false).unwrap(), UiDist::Built);
    assert_eq!(ensure_ui_dist(&dist, false, false).unwrap(), UiDist::Built);
    let _ = std::fs::remove_dir_all(root);
}

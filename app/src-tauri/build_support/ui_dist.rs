// SPDX-License-Identifier: GPL-3.0-or-later
//! The `app/ui/dist` rule of build.rs, in its own file so the tests can include it
//! (tests/build_ui_dist.rs) - build scripts cannot be unit-tested otherwise.
//!
//! `tauri::generate_context!()` embeds `app/ui/dist` and refuses to compile when it is
//! missing. Debug and test builds get a one-page stub in that case (with a warning),
//! so `cargo build --workspace` works before the UI has been built. Release builds
//! fail instead, also when `dist` holds only that stub, so a shipped app can never
//! contain the stub page. `JK_ALLOW_UI_STUB=1` lets a release build that will never be
//! shipped (e.g. release-mode tests of the Rust code) use the stub.

use std::fs;
use std::path::Path;

/// Marker in the stub page, so a release build can tell it from a real UI.
pub const STUB_MARKER: &str = "<title>UI not built</title>";

/// What [`ensure_ui_dist`] found or did.
#[derive(Debug, PartialEq, Eq)]
pub enum UiDist {
    /// A built UI is there.
    Built,
    /// Only the stub is there, and this build may use it.
    StubKept,
    /// Nothing was there; the stub was written (debug / test builds, or allowed).
    StubWritten,
}

/// Checks `dist` (app/ui/dist) for a `release` build. `Err` is the message to fail
/// the build with.
pub fn ensure_ui_dist(dist: &Path, release: bool, stub_allowed: bool) -> Result<UiDist, String> {
    let index = dist.join("index.html");
    let existing = fs::read_to_string(&index).ok();
    let is_stub = existing.as_deref().is_some_and(|t| t.contains(STUB_MARKER));
    if release && !stub_allowed && (existing.is_none() || is_stub) {
        return Err(format!(
            "app/ui/dist {} - refusing to build a release app without its user interface. \
             Run `npm ci && npm run build` in app/ui first (app/scripts/build-windows.sh does), \
             or set JK_ALLOW_UI_STUB=1 for a release build that will never be shipped.",
            if is_stub { "holds only the stub page" } else { "is missing" }
        ));
    }
    match existing {
        Some(_) if is_stub => Ok(UiDist::StubKept),
        Some(_) => Ok(UiDist::Built),
        None => {
            fs::create_dir_all(dist).map_err(|e| format!("cannot create {}: {e}", dist.display()))?;
            let page = format!(
                "<!doctype html><meta charset=\"utf-8\">{STUB_MARKER}\
                 <p>The web UI was not built when this binary was compiled. \
                 Run <code>npm install</code> and <code>npm run build</code> in <code>app/ui</code>, then rebuild.</p>\n"
            );
            fs::write(&index, page).map_err(|e| format!("cannot write {}: {e}", index.display()))?;
            Ok(UiDist::StubWritten)
        }
    }
}

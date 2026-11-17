// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri commands called by the web UI (app/ui/src/api.ts mirrors these types).
//!
//! Commands are the only way the UI talks to the Rust core. They never carry audio
//! samples (docs/ARCHITECTURE.md section 1).

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::AppState;
use crate::portable::DataMode;
use crate::smoke::{self, UiReport};

/// Answer of `app_info` (app/ui/src/api.ts `AppInfo`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    name: String,
    version: String,
    identifier: String,
    mode: DataMode,
    portable_root: Option<String>,
    data_root: String,
    webview_data_dir: Option<String>,
    mode_reason: String,
    translocated: bool,
    os: &'static str,
    arch: &'static str,
    webview_version: String,
    smoke_test: bool,
}

/// Who and where the app is. The product name comes from Tauri's package info
/// (tauri.conf.json `productName`, checked against branding.json at build time),
/// not from a literal in the code.
#[tauri::command]
pub fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppInfo {
    state.smoke.mark("app_info_called");
    let pkg = app.package_info();
    let layout = &state.layout;
    AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        identifier: app.config().identifier.clone(),
        mode: layout.mode,
        portable_root: layout
            .portable_root
            .as_ref()
            .map(|p| p.display().to_string()),
        data_root: layout.data_root.display().to_string(),
        webview_data_dir: layout
            .webview_data_dir
            .as_ref()
            .map(|p| p.display().to_string()),
        mode_reason: layout.reason.clone(),
        translocated: layout.translocated,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        webview_version: tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})")),
        smoke_test: state.smoke.enabled(),
    }
}

/// Answer of `ping`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingReply {
    echo: String,
    uptime_ms: u64,
}

/// Echoes the text back; used by the placeholder page to show the IPC round trip.
#[tauri::command]
pub fn ping(state: State<'_, AppState>, text: String) -> PingReply {
    PingReply {
        echo: text,
        uptime_ms: state.smoke.elapsed_ms() as u64,
    }
}

/// The UI's half of `--smoke-test` (see src/smoke.rs). Writes the report and exits.
#[tauri::command]
pub fn smoke_report(
    app: AppHandle,
    state: State<'_, AppState>,
    report: UiReport,
) -> Result<(), String> {
    if !state.smoke.enabled() {
        return Err("the app was not started with --smoke-test".into());
    }
    smoke::finish_with_ui_report(&app, report);
    Ok(())
}

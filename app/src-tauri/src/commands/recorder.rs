// SPDX-License-Identifier: GPL-3.0-or-later
//! Recorder commands (APP_API section 8).

use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::error::ApiResult;
use crate::recorder::{Recording, RecorderStatusView};

/// `recorder_status() -> RecorderStatus`.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_status(state: State<'_, AppState>) -> ApiResult<RecorderStatusView> {
    Ok(state.core.recorder.status())
}

/// `recorder_start()` (returns the status once the OS stream runs; up to 20 s).
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_start(state: State<'_, AppState>) -> ApiResult<RecorderStatusView> {
    let core = state.core.clone();
    blocking(move || core.recorder.start()).await
}

/// `recorder_stop() -> Recording`.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_stop(state: State<'_, AppState>) -> ApiResult<Recording> {
    let core = state.core.clone();
    blocking(move || core.recorder.stop()).await
}

#[derive(Serialize)]
pub struct Permission {
    pub permission: &'static str,
}

/// `recorder_request_permission()`: macOS shows its "Screen Recording" prompt (the UI
/// shows `capabilities.recorder.permission_explanation` first). Returns the state.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_request_permission(state: State<'_, AppState>) -> ApiResult<Permission> {
    let core = state.core.clone();
    blocking(move || {
        Ok(Permission {
            permission: core.recorder.request_permission(),
        })
    })
    .await
}

#[derive(Serialize)]
pub struct RecordingList {
    pub recordings: Vec<Recording>,
}

/// `recordings_list() -> { recordings }` (newest first).
#[tauri::command(rename_all = "snake_case")]
pub async fn recordings_list(state: State<'_, AppState>) -> ApiResult<RecordingList> {
    let core = state.core.clone();
    blocking(move || core.recorder.list().map(|recordings| RecordingList { recordings })).await
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Melody guide commands (Work Order 2; docs/APP_API.md section 12.3).

use serde_json::Value;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::error::ApiResult;
use crate::guide::{BatchQueued, GuideQueue, GuideStatus};

/// `guide_status(song_id) -> GuideStatus`.
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_status(state: State<'_, AppState>, song_id: String) -> ApiResult<GuideStatus> {
    let core = state.core.clone();
    blocking(move || core.guide_status(&song_id)).await
}

/// `guide_build(song_id, force?) -> GuideStatus`: queues a build and returns at once;
/// progress comes as `guide://status`.
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_build(state: State<'_, AppState>, song_id: String, force: Option<bool>) -> ApiResult<GuideStatus> {
    let core = state.core.clone();
    blocking(move || core.guide_build(&song_id, force.unwrap_or(false))).await
}

/// `guide_build_all(options?: { include_outdated?: boolean }) -> { queued, skipped }`
/// ("Add practice data to all songs"). `include_outdated` is accepted at top level too.
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_build_all(
    state: State<'_, AppState>,
    options: Option<Value>,
    include_outdated: Option<bool>,
) -> ApiResult<BatchQueued> {
    let outdated = include_outdated
        .or_else(|| options.as_ref().and_then(|o| o.get("include_outdated")).and_then(Value::as_bool))
        .unwrap_or(false);
    let core = state.core.clone();
    blocking(move || core.guide_build_all(outdated)).await
}

/// `guide_cancel(song_id?)`: one song's build, or every one.
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_cancel(state: State<'_, AppState>, song_id: Option<String>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.guide_cancel(song_id.as_deref())).await
}

/// `guide_queue() -> { items, running }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_queue(state: State<'_, AppState>) -> ApiResult<GuideQueue> {
    Ok(state.core.guide_queue())
}

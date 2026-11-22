// SPDX-License-Identifier: GPL-3.0-or-later
//! Recorder commands (APP_API section 8), including the media-player link and
//! "Record songs separately" (additions of 2026-11-18, the owner's request).

use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::error::ApiResult;
use crate::recorder::{Recording, RecorderStatusView};
use crate::recsession::{MediaPlayersView, SessionStartOptions, SessionView};

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

/// `recorder_stop(pause_player?) -> Recording`. `pause_player` (addition): after
/// "Record & play", also pause that player.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_stop(state: State<'_, AppState>, pause_player: Option<bool>) -> ApiResult<Recording> {
    let core = state.core.clone();
    blocking(move || core.recorder.stop_and_pause(pause_player.unwrap_or(false))).await
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

/// `recorder_media_sessions() -> { available, reason, players }` (addition): the
/// media players Windows knows about and what each is playing. Read only.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_media_sessions(state: State<'_, AppState>) -> ApiResult<MediaPlayersView> {
    let core = state.core.clone();
    blocking(move || Ok(core.recorder.media_players())).await
}

/// `recorder_record_and_play(player_id) -> RecorderStatus` (addition).
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_record_and_play(state: State<'_, AppState>, player_id: String) -> ApiResult<RecorderStatusView> {
    let core = state.core.clone();
    blocking(move || core.recorder.record_and_play(&player_id)).await
}

/// `recorder_session_start(options) -> RecorderStatus` (addition). `options` is
/// `{ player_id, auto_stop_idle_s?, max_songs?, start_player?, pause_player_at_end? }`;
/// a bare `player_id` argument is accepted too.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_session_start(
    state: State<'_, AppState>,
    options: Option<SessionStartOptions>,
    player_id: Option<String>,
) -> ApiResult<RecorderStatusView> {
    let mut opts = options.unwrap_or_default();
    if opts.player_id.is_empty()
        && let Some(id) = player_id
    {
        opts.player_id = id;
    }
    let core = state.core.clone();
    blocking(move || core.recorder.session_start(&opts)).await
}

/// `recorder_session_stop(pause_player?) -> RecordingSession` (addition). The split
/// runs in the background (`recorder://session` events).
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_session_stop(state: State<'_, AppState>, pause_player: Option<bool>) -> ApiResult<SessionView> {
    let core = state.core.clone();
    blocking(move || core.recorder.session_stop(pause_player.unwrap_or(true))).await
}

#[derive(Serialize)]
pub struct SessionList {
    pub sessions: Vec<SessionView>,
}

/// `recorder_sessions_list() -> { sessions }` (addition), newest first.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_sessions_list(state: State<'_, AppState>) -> ApiResult<SessionList> {
    let core = state.core.clone();
    blocking(move || core.recorder.sessions_list().map(|sessions| SessionList { sessions })).await
}

/// `recorder_session_split(folder) -> RecordingSession` (addition): split again.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_session_split(state: State<'_, AppState>, folder: String) -> ApiResult<SessionView> {
    let core = state.core.clone();
    blocking(move || core.recorder.session_split(&folder)).await
}

/// `recorder_session_delete(folder)` (addition): deletes a finished session for good.
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_session_delete(state: State<'_, AppState>, folder: String) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.recorder.session_delete(&folder)).await
}

/// `recorder_preview(path, start_s?, max_s?)` (addition): plays part of a recording
/// natively (the web view never plays audio).
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_preview(
    state: State<'_, AppState>,
    path: String,
    start_s: Option<f64>,
    max_s: Option<f64>,
) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.recorder.preview(&path, start_s.unwrap_or(0.0), max_s.unwrap_or(30.0))).await
}

/// `recorder_preview_stop()` (addition).
#[tauri::command(rename_all = "snake_case")]
pub async fn recorder_preview_stop(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || {
        core.recorder.preview_stop();
        Ok(())
    })
    .await
}

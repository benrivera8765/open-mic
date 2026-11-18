// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading, playback and output device commands (APP_API sections 3 and 4).

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::engine::DeviceInfo;
use crate::error::{ApiError, ApiResult};
use crate::playback::{PlayerState, SongDetail, SongSettings};

/// `song_load(song_id) -> SongDetail` (stops any current song; paused at 0).
#[tauri::command(rename_all = "snake_case")]
pub async fn song_load(state: State<'_, AppState>, song_id: String) -> ApiResult<SongDetail> {
    let core = state.core.clone();
    blocking(move || core.player.load(&song_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn player_play(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.player.play()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn player_pause(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.player.pause()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn player_toggle(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.player.toggle()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn player_stop(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.player.stop()).await
}

/// `player_seek(position_s)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn player_seek(state: State<'_, AppState>, position_s: f64) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.player.seek(position_s)).await
}

/// `player_set(changes: Partial<SongSettings>) -> SongSettings` (live; slot save
/// debounced about 1 s).
#[tauri::command(rename_all = "snake_case")]
pub async fn player_set(state: State<'_, AppState>, changes: Value) -> ApiResult<SongSettings> {
    let core = state.core.clone();
    blocking(move || core.player.set(&changes)).await
}

/// `player_set_master(volume)` (0..1; an app setting, not saved in the song).
#[tauri::command(rename_all = "snake_case")]
pub async fn player_set_master(state: State<'_, AppState>, volume: f64) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.set_master(volume)).await
}

/// `player_state() -> PlayerState`.
#[tauri::command(rename_all = "snake_case")]
pub async fn player_state(state: State<'_, AppState>) -> ApiResult<PlayerState> {
    let core = state.core.clone();
    blocking(move || Ok(core.player.state())).await
}

/// `player_diagnostics() -> Diagnostics | null`: the engine's counters and timings
/// (underruns, device_starved, min_lead_ms, max_callback_us, worker_max_block_ms...), for
/// soak tests and support reports. Null when no engine is running.
#[tauri::command(rename_all = "snake_case")]
pub async fn player_diagnostics(state: State<'_, AppState>) -> ApiResult<Option<jk_engine::engine::Diagnostics>> {
    let core = state.core.clone();
    blocking(move || Ok(core.player.engine().diagnostics())).await
}

#[derive(Serialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceInfo>,
}

/// `devices_list() -> { devices }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn devices_list(state: State<'_, AppState>) -> ApiResult<DeviceList> {
    let core = state.core.clone();
    blocking(move || {
        core.player
            .engine()
            .list_devices()
            .map(|devices| DeviceList { devices })
            .map_err(|e| ApiError::new(e.code.clone(), e.message.clone()))
    })
    .await
}

/// `device_set(id | null, exclusive)`: pauses, switches, resumes; saved in settings.
#[tauri::command(rename_all = "snake_case")]
pub async fn device_set(state: State<'_, AppState>, id: Option<String>, exclusive: Option<bool>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.device_set(id, exclusive.unwrap_or(false))).await
}

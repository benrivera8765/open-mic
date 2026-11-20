// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice mode commands (Work Order 2; docs/APP_API.md sections 3 `player_key_at`, 10.1
//! `selfcheck_lane`, 10.2 `practice_rig_check`, 12.1, 12.4-12.6). Thin wrappers: the work
//! is in src/practice/.

use serde_json::Value;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::practice::guide_view::PracticeGuideView;
use crate::practice::mic::MicPermissionView;
use crate::practice::rigcheck::{RigCheckOptions, RigCheckReport};
use crate::practice::{ExcludedSpan, HistoryView, MeterResult, MicList, PracticeStateView, TargetView, result_payload, start_options};
use crate::settings::InputChoice;

/// `player_key_at(position_s, segment?) -> { semitones }` (APP_API 3, WO2).
#[tauri::command(rename_all = "snake_case")]
pub async fn player_key_at(state: State<'_, AppState>, position_s: f64, segment: Option<u64>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || Ok(serde_json::json!({ "semitones": core.player.key_at(position_s, segment)? }))).await
}

// ---------------------------------------------------------------- microphone (12.4)

#[tauri::command(rename_all = "snake_case")]
pub async fn mic_permission(state: State<'_, AppState>) -> ApiResult<MicPermissionView> {
    let core = state.core.clone();
    blocking(move || Ok(core.practice.mic_permission())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mic_request_permission(state: State<'_, AppState>) -> ApiResult<MicPermissionView> {
    let core = state.core.clone();
    blocking(move || Ok(core.practice.mic_request_permission())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mic_open_settings(state: State<'_, AppState>) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.practice.mic_open_settings()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mic_list(state: State<'_, AppState>) -> ApiResult<MicList> {
    let core = state.core.clone();
    blocking(move || Ok(core.practice.mic_list())).await
}

/// `mic_choose(choice: InputChoice) -> InputChoice` (the flat form is accepted too).
#[tauri::command(rename_all = "snake_case")]
pub async fn mic_choose(
    state: State<'_, AppState>,
    choice: Option<InputChoice>,
    device_id: Option<String>,
    device_name: Option<String>,
    channel: Option<u32>,
) -> ApiResult<InputChoice> {
    let c = choice.unwrap_or(InputChoice { device_id, device_name, channel });
    let core = state.core.clone();
    blocking(move || core.practice.mic_choose(c)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mic_meter(state: State<'_, AppState>, device_id: Option<String>) -> ApiResult<MeterResult> {
    let core = state.core.clone();
    blocking(move || core.practice.mic_meter(device_id)).await
}

// ---------------------------------------------------------------- session (12.5)

/// `practice_start(options?: { song_id?, watch_only? }) -> PracticeState`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_start(state: State<'_, AppState>, options: Option<Value>) -> ApiResult<PracticeStateView> {
    let (song_id, watch) = start_options(&options.unwrap_or(Value::Null));
    let core = state.core.clone();
    blocking(move || core.practice.start(song_id, watch)).await
}

/// `practice_stop(options?: { keep_result? }) -> { result }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_stop(state: State<'_, AppState>, options: Option<Value>) -> ApiResult<Value> {
    let keep = options.as_ref().and_then(|o| o.get("keep_result")).and_then(Value::as_bool).unwrap_or(true);
    let core = state.core.clone();
    blocking(move || core.practice.stop(keep).map(result_payload)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_state(state: State<'_, AppState>) -> ApiResult<PracticeStateView> {
    let core = state.core.clone();
    blocking(move || Ok(core.practice.state())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_guide(state: State<'_, AppState>, song_id: Option<String>) -> ApiResult<PracticeGuideView> {
    let core = state.core.clone();
    blocking(move || core.practice.guide_view(song_id.as_deref())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_suspend_input(state: State<'_, AppState>, suspended: bool) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.practice.suspend_input(suspended)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_finish(state: State<'_, AppState>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.practice.finish().map(result_payload)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_again(state: State<'_, AppState>) -> ApiResult<PracticeStateView> {
    let core = state.core.clone();
    blocking(move || core.practice.again()).await
}

/// `practice_song_set(song_id, changes: { octave?, strictness? })`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_song_set(state: State<'_, AppState>, song_id: String, changes: Value) -> ApiResult<PracticeStateView> {
    let core = state.core.clone();
    blocking(move || core.practice.song_set(&song_id, &changes)).await
}

/// `practice_exclude(song_id, span, excluded) -> { excluded }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_exclude(state: State<'_, AppState>, song_id: String, span: ExcludedSpan, excluded: bool) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.practice.exclude(&song_id, &span, excluded).map(|l| serde_json::json!({ "excluded": l }))).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn practice_history(state: State<'_, AppState>, song_id: String) -> ApiResult<HistoryView> {
    let core = state.core.clone();
    blocking(move || core.practice.history(&song_id)).await
}

/// `practice_latency_set(manual_ms, device_id?) -> { manual_ms }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_latency_set(state: State<'_, AppState>, manual_ms: f64, device_id: Option<String>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.practice.latency_set(manual_ms, device_id).map(|ms| serde_json::json!({ "manual_ms": ms }))).await
}

/// Internal (screen checks): `practice_target_at(position_s, segment?)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_target_at(state: State<'_, AppState>, position_s: f64, segment: Option<u64>) -> ApiResult<TargetView> {
    let core = state.core.clone();
    blocking(move || core.practice.target_at(position_s, segment)).await
}

/// Internal, test modes only (`--smoke-test`, `--practice-test`): the next
/// `practice_start` listens to generated sound instead of the microphone (APP_API 12.1).
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_test_input(state: State<'_, AppState>, source: Option<Value>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.practice.test_input(&source.unwrap_or(Value::Null)).map(|_| serde_json::json!({ "ok": true }))).await
}

/// `practice_rig_check(options) -> RigCheckReport`: AUDIBLE, started by hand only
/// (APP_API 10.2); refused without `audible_confirmed`.
#[tauri::command(rename_all = "snake_case")]
pub async fn practice_rig_check(state: State<'_, AppState>, options: Option<RigCheckOptions>) -> ApiResult<RigCheckReport> {
    let opts = options.ok_or_else(|| ApiError::new("audible_not_confirmed", "The rig check needs its options (audible_confirmed)."))?;
    let core = state.core.clone();
    blocking(move || core.practice.rig_check(&opts)).await
}

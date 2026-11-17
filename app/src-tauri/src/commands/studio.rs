// SPDX-License-Identifier: GPL-3.0-or-later
//! Studio commands (APP_API sections 6 and 9).

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use super::blocking;
use crate::AppState;
use crate::error::ApiResult;
use crate::studio::StudioStatus;

/// `studio_status()`.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_status(state: State<'_, AppState>) -> ApiResult<StudioStatus> {
    Ok(state.core.studio.status())
}

/// `studio_inspect(path) -> InspectResult` (STUDIO_PROTOCOL section 5).
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_inspect(state: State<'_, AppState>, path: String) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio.inspect(&path)).await
}

/// `studio_lyrics_lookup({ artist, title, album, duration_s }) -> { results }`. The
/// object may be passed as `query` or its fields directly.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_lyrics_lookup(
    state: State<'_, AppState>,
    query: Option<Value>,
    artist: Option<Value>,
    title: Option<Value>,
    album: Option<Value>,
    duration_s: Option<Value>,
) -> ApiResult<Value> {
    let q = query.unwrap_or_else(|| json!({"artist": artist, "title": title, "album": album, "duration_s": duration_s}));
    let core = state.core.clone();
    blocking(move || core.studio.lyrics_lookup(&q)).await
}

#[derive(Serialize)]
pub struct Submitted {
    pub job_id: String,
}

/// `studio_submit({ input, preset, language, metadata, lyrics }) -> { job_id }`. The
/// object may be passed as `job` or its fields directly.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub async fn studio_submit(
    state: State<'_, AppState>,
    job: Option<Value>,
    input: Option<String>,
    preset: Option<String>,
    language: Option<String>,
    metadata: Option<Value>,
    lyrics: Option<Value>,
) -> ApiResult<Submitted> {
    let spec = job.unwrap_or_else(|| {
        json!({"input": input, "preset": preset.unwrap_or_else(|| "full".into()),
               "language": language.unwrap_or_else(|| "en".into()),
               "metadata": metadata.unwrap_or_else(|| json!({})),
               "lyrics": lyrics.unwrap_or_else(|| json!({"mode": "auto", "text": null, "lrc": null}))})
    });
    let core = state.core.clone();
    blocking(move || core.studio.submit(&spec).map(|job_id| Submitted { job_id })).await
}

/// `studio_cancel(job_id)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_cancel(state: State<'_, AppState>, job_id: String) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.studio.cancel(&job_id)).await
}

/// `studio_clear_finished()` (addition): removes done / failed / cancelled jobs.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_clear_finished(state: State<'_, AppState>) -> ApiResult<()> {
    state.core.studio.clear_finished();
    Ok(())
}

/// `studio_models_status(preset?)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_models_status(state: State<'_, AppState>, preset: Option<String>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio.models_status(preset.as_deref().unwrap_or("full"))).await
}

/// `studio_models_download(preset?)`: progress as `studio://models` events.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_models_download(state: State<'_, AppState>, preset: Option<String>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio.models_download(preset.as_deref().unwrap_or("full"))).await
}

/// `studio_realign(song_id, text, anchors) -> Lyrics` (also stored as the song's
/// `lyrics_override`).
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_realign(
    state: State<'_, AppState>,
    song_id: String,
    text: String,
    anchors: Option<Value>,
) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio_realign(&song_id, &text, &anchors.unwrap_or(Value::Null))).await
}

/// File types the Studio accepts (WORK_ORDER 4.1).
pub const MEDIA_EXTENSIONS: [&str; 13] = [
    "mp3", "m4a", "flac", "wav", "ogg", "opus", "aiff", "aif", "wma", "mp4", "mkv", "mov", "webm",
];

/// `pick_files() -> string[]`: the native open dialog for audio and video files.
#[tauri::command(rename_all = "snake_case")]
pub async fn pick_files(app: AppHandle) -> ApiResult<Vec<String>> {
    blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let mut exts: Vec<&str> = MEDIA_EXTENSIONS.to_vec();
        exts.push("avi");
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Choose songs to make karaoke songs from")
            .add_filter("Audio and video", &exts);
        // Modal to the app window, so it cannot end up behind it.
        if let Some(w) = crate::main_window(&app) {
            dialog = dialog.set_parent(&w);
        }
        let picked = dialog.blocking_pick_files();
        Ok(picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .map(|p| p.display().to_string())
            .collect())
    })
    .await
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Library, song file edits and export commands (APP_API sections 2, 6, 7).

use serde_json::Value;
use tauri::State;

use super::blocking;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::export::ExportStarted;
use crate::library::{Cover, LibraryList};

/// `library_list() -> { songs, free_bytes, total_bytes }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn library_list(state: State<'_, AppState>) -> ApiResult<LibraryList> {
    let core = state.core.clone();
    blocking(move || core.library.list()).await
}

/// `song_cover(song_id, max_px) -> { data_url }`.
#[tauri::command(rename_all = "snake_case")]
pub async fn song_cover(state: State<'_, AppState>, song_id: String, max_px: Option<u32>) -> ApiResult<Cover> {
    let core = state.core.clone();
    blocking(move || core.library.cover(&song_id, max_px.unwrap_or(256))).await
}

/// `lyrics_save(song_id, lyrics)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn lyrics_save(state: State<'_, AppState>, song_id: String, lyrics: Value) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.lyrics_save(&song_id, lyrics)).await
}

/// `lyrics_reset(song_id)`.
#[tauri::command(rename_all = "snake_case")]
pub async fn lyrics_reset(state: State<'_, AppState>, song_id: String) -> ApiResult<()> {
    let core = state.core.clone();
    blocking(move || core.lyrics_reset(&song_id)).await
}

/// `metadata_save(song_id, { title?, artist?, album? })`. The object may be passed
/// as `metadata` or `changes`, or its fields directly.
#[tauri::command(rename_all = "snake_case")]
pub async fn metadata_save(
    state: State<'_, AppState>,
    song_id: String,
    metadata: Option<Value>,
    changes: Option<Value>,
    title: Option<Value>,
    artist: Option<Value>,
    album: Option<Value>,
) -> ApiResult<()> {
    let obj = match metadata.or(changes) {
        Some(v) => v,
        None => {
            let mut m = serde_json::Map::new();
            for (k, v) in [("title", title), ("artist", artist), ("album", album)] {
                if let Some(v) = v {
                    m.insert(k.into(), v);
                }
            }
            Value::Object(m)
        }
    };
    let core = state.core.clone();
    blocking(move || core.metadata_save(&song_id, &obj)).await
}

/// `export_backing(song_id, { format }) -> { job_id }`. The format may be passed as
/// `format` or inside `options`.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_backing(
    state: State<'_, AppState>,
    song_id: String,
    format: Option<String>,
    options: Option<Value>,
) -> ApiResult<ExportStarted> {
    let format = format
        .or_else(|| {
            options
                .as_ref()
                .and_then(|o| o.get("format"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| ApiError::bad_request("Choose an export format (flac24 or wav32f)."))?;
    let core = state.core.clone();
    blocking(move || core.export_backing(&song_id, &format)).await
}

/// `export_cancel(job_id)` (addition to APP_API v1).
#[tauri::command(rename_all = "snake_case")]
pub async fn export_cancel(state: State<'_, AppState>, job_id: String) -> ApiResult<()> {
    state.core.exports.cancel(&job_id)
}

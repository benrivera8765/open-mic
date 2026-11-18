// SPDX-License-Identifier: GPL-3.0-or-later
//! Studio commands (APP_API sections 6 and 9).

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, State};

use super::blocking;
use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::library::Cover;
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

/// `studio_models_status(preset?)`: the UI passes the preset chosen in the Studio form;
/// absent means the default preset.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_models_status(state: State<'_, AppState>, preset: Option<String>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio.models_status(preset.as_deref().unwrap_or(crate::studio::DEFAULT_PRESET))).await
}

/// `studio_models_download(preset?)`: progress as `studio://models` events.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_models_download(state: State<'_, AppState>, preset: Option<String>) -> ApiResult<Value> {
    let core = state.core.clone();
    blocking(move || core.studio.models_download(preset.as_deref().unwrap_or(crate::studio::DEFAULT_PRESET))).await
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

/// Picture files a cover can be chosen from (SONG_FORMAT 4.1 allows JPEG and PNG).
pub const COVER_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// Largest picture file read for a preview; the Studio refuses larger user covers too
/// (studio/jukeoke_studio/media.py `MAX_USER_COVER_FILE`).
const MAX_COVER_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// `pick_cover_image() -> string | null` (addition): the native open dialog for one
/// JPEG or PNG picture, used by the Studio form to replace a song's cover. The path is
/// passed on as `metadata.cover_path` (STUDIO_PROTOCOL section 7).
#[tauri::command(rename_all = "snake_case")]
pub async fn pick_cover_image(app: AppHandle) -> ApiResult<Option<String>> {
    blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Choose a cover picture")
            .add_filter("JPEG or PNG picture", &COVER_EXTENSIONS);
        if let Some(w) = crate::main_window(&app) {
            dialog = dialog.set_parent(&w);
        }
        Ok(dialog
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string()))
    })
    .await
}

/// `studio_cover_preview(path, max_px) -> { data_url }` (addition): a JPEG or PNG
/// picture file (the cover `studio_inspect` extracted, or one the user picked) as a
/// thumbnail `data:` URL for the Studio form. Anything that is not a JPEG or PNG
/// picture is refused, so this cannot be used to read other files.
#[tauri::command(rename_all = "snake_case")]
pub async fn studio_cover_preview(path: String, max_px: Option<u32>) -> ApiResult<Cover> {
    blocking(move || cover_preview(std::path::Path::new(&path), max_px.unwrap_or(256))).await
}

/// See [`studio_cover_preview`].
pub fn cover_preview(path: &std::path::Path, max_px: u32) -> ApiResult<Cover> {
    let not_found = || ApiError::new("not_found", "That picture could not be found.");
    let meta = std::fs::metadata(path).map_err(|_| not_found())?;
    if !meta.is_file() {
        return Err(not_found());
    }
    if meta.len() > MAX_COVER_FILE_BYTES {
        return Err(ApiError::bad_request("That picture is larger than 64 MB."));
    }
    let bytes = std::fs::read(path).map_err(|_| not_found())?;
    let url = crate::library::thumbnail_data_url("", &bytes, max_px.clamp(16, 1024))
        .map_err(|_| ApiError::bad_request("The cover picture must be a JPEG or PNG image."))?;
    Ok(Cover { data_url: Some(url) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 90]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn cover_preview_shows_jpeg_and_png_and_refuses_anything_else() {
        let dir = std::env::temp_dir().join(format!("jk-cover-preview-{}", crate::fs_atomic::random_u32()));
        std::fs::create_dir_all(&dir).unwrap();
        // A small PNG comes back unchanged; a big one as a thumbnail of at most max_px.
        let small = dir.join("small.png");
        std::fs::write(&small, png(40, 30)).unwrap();
        let url = cover_preview(&small, 256).unwrap().data_url.unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let big = dir.join("big.png");
        std::fs::write(&big, png(600, 300)).unwrap();
        let url = cover_preview(&big, 128).unwrap().data_url.unwrap();
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(url.split_once(',').unwrap().1)
            .unwrap();
        let thumb = image::load_from_memory(&bytes).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (128, 64));
        // Not a picture, a folder, a missing file: refused with a plain message.
        let gif = dir.join("x.gif");
        std::fs::write(&gif, b"GIF89a................").unwrap();
        let text = dir.join("notes.txt");
        std::fs::write(&text, b"secret text").unwrap();
        for p in [&gif, &text] {
            let e = cover_preview(p, 128).unwrap_err();
            assert_eq!(e.code, "bad_request", "{}", e.message);
        }
        assert_eq!(cover_preview(&dir, 128).unwrap_err().code, "not_found");
        assert_eq!(cover_preview(&dir.join("missing.jpg"), 128).unwrap_err().code, "not_found");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

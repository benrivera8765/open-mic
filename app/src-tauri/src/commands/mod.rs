// SPDX-License-Identifier: GPL-3.0-or-later
//! Tauri commands: the only way the web UI talks to the Rust core
//! (docs/APP_API.md; TypeScript types in app/ui/src/api/types.ts).
//!
//! - Every command takes and returns `snake_case` JSON (`rename_all = "snake_case"`).
//! - Every command is `async` and runs its work on Tauri's blocking pool: plain
//!   (non-async) Tauri commands run on the main thread, where a slow USB stick or a
//!   Studio start would freeze the window.
//! - Errors reject with `{ code, message }` (src/error.rs).
//!
//! The commands are thin: the work is in [`crate::core::Core`] and the modules it
//! owns, which are tested without a window.

mod app;
mod library;
mod player;
mod recorder;
mod studio;

pub use app::*;
pub use library::*;
pub use player::*;
pub use recorder::*;
pub use studio::*;

use crate::error::{ApiError, ApiResult};

/// Runs blocking work off the main thread and the async runtime's worker threads.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(ApiError::internal)?
}

/// Every command, for `tauri::generate_handler!` (src/lib.rs).
#[macro_export]
macro_rules! all_commands {
    () => {
        tauri::generate_handler![
            // app
            $crate::commands::app_info,
            $crate::commands::ping,
            $crate::commands::smoke_report,
            $crate::commands::settings_get,
            $crate::commands::settings_set,
            $crate::commands::selfcheck_run,
            $crate::commands::selfcheck_user_agent,
            $crate::commands::root_choose,
            // library and song file edits
            $crate::commands::library_list,
            $crate::commands::song_cover,
            $crate::commands::library_delete,
            $crate::commands::library_deleted_list,
            $crate::commands::library_restore,
            $crate::commands::library_purge,
            $crate::commands::lyrics_save,
            $crate::commands::lyrics_reset,
            $crate::commands::metadata_save,
            $crate::commands::export_backing,
            $crate::commands::export_cancel,
            // playback and devices
            $crate::commands::song_load,
            $crate::commands::player_play,
            $crate::commands::player_pause,
            $crate::commands::player_toggle,
            $crate::commands::player_stop,
            $crate::commands::player_seek,
            $crate::commands::player_set,
            $crate::commands::player_set_master,
            $crate::commands::player_state,
            $crate::commands::player_diagnostics,
            $crate::commands::devices_list,
            $crate::commands::device_set,
            // recorder
            $crate::commands::recorder_status,
            $crate::commands::recorder_start,
            $crate::commands::recorder_stop,
            $crate::commands::recorder_request_permission,
            $crate::commands::recordings_list,
            $crate::commands::recorder_media_sessions,
            $crate::commands::recorder_record_and_play,
            $crate::commands::recorder_session_start,
            $crate::commands::recorder_session_stop,
            $crate::commands::recorder_sessions_list,
            $crate::commands::recorder_session_split,
            $crate::commands::recorder_preview,
            $crate::commands::recorder_preview_stop,
            // studio
            $crate::commands::studio_status,
            $crate::commands::studio_inspect,
            $crate::commands::studio_lyrics_lookup,
            $crate::commands::studio_submit,
            $crate::commands::studio_cancel,
            $crate::commands::studio_clear_finished,
            $crate::commands::studio_models_status,
            $crate::commands::studio_models_download,
            $crate::commands::studio_realign,
            $crate::commands::pick_files,
            $crate::commands::pick_cover_image,
            $crate::commands::studio_cover_preview,
        ]
    };
}

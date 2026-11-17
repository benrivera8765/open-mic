// SPDX-License-Identifier: GPL-3.0-or-later
//! App, settings, self-check and smoke-test commands (APP_API sections 1, 5, 10).

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use super::blocking;
use crate::AppState;
use crate::core::AppInfo;
use crate::error::{ApiError, ApiResult};
use crate::selfcheck::{self, SelfcheckResult};
use crate::settings::AppSettings;
use crate::smoke::{self, UiReport};

/// `app_info() -> AppInfo`.
#[tauri::command(rename_all = "snake_case")]
pub async fn app_info(state: State<'_, AppState>) -> ApiResult<AppInfo> {
    state.smoke.mark("app_info_called");
    let core = state.core.clone();
    blocking(move || Ok(core.app_info(webview_version()))).await
}

/// WebView2 runtime version (Windows) or WebKit version (macOS).
pub fn webview_version() -> String {
    if cfg!(target_os = "macos") {
        selfcheck::webkit_version().unwrap_or_else(|| "unknown".into())
    } else {
        tauri::webview_version().unwrap_or_else(|e| format!("unknown ({e})"))
    }
}

/// `settings_get() -> AppSettings`.
#[tauri::command(rename_all = "snake_case")]
pub async fn settings_get(state: State<'_, AppState>) -> ApiResult<AppSettings> {
    Ok(state.core.settings.get())
}

/// `settings_set(changes) -> AppSettings` (deep merge, saved atomically).
#[tauri::command(rename_all = "snake_case")]
pub async fn settings_set(state: State<'_, AppState>, changes: Value) -> ApiResult<AppSettings> {
    let core = state.core.clone();
    blocking(move || core.settings_set(&changes)).await
}

/// `selfcheck_run({ user_agent?, silent? })` - `silent` (an addition) plays digital
/// silence instead of the -60 dBFS tone.
#[tauri::command(rename_all = "snake_case")]
pub async fn selfcheck_run(
    state: State<'_, AppState>,
    user_agent: Option<String>,
    silent: Option<bool>,
) -> ApiResult<SelfcheckResult> {
    let core = state.core.clone();
    if let Some(ua) = &user_agent {
        *core.user_agent.lock().unwrap_or_else(|e| e.into_inner()) = Some(ua.clone());
    }
    let ua = user_agent.or_else(|| core.user_agent.lock().unwrap_or_else(|e| e.into_inner()).clone());
    blocking(move || {
        Ok(selfcheck::run(
            &core,
            &selfcheck::Options {
                user_agent: ua,
                silent: silent.unwrap_or(false),
            },
        ))
    })
    .await
}

/// Internal: the page reports `navigator.userAgent` in `--selfcheck` runs (injected
/// by src/lib.rs, not called by the UI code).
#[tauri::command(rename_all = "snake_case")]
pub async fn selfcheck_user_agent(app: AppHandle, user_agent: String) -> ApiResult<()> {
    crate::cli_selfcheck_user_agent(&app, user_agent);
    Ok(())
}

#[derive(Serialize)]
pub struct RootChoice {
    pub ok: bool,
    pub root: Option<String>,
    pub message: String,
}

/// Addition to APP_API v1 (macOS translocation, WORK_ORDER trap 2): shows a folder
/// picker for the stick's main folder, remembers it in
/// `~/Library/Application Support/<bundle id>/root.txt` and restarts the app.
#[tauri::command(rename_all = "snake_case")]
pub async fn root_choose(app: AppHandle, state: State<'_, AppState>) -> ApiResult<RootChoice> {
    if !state.core.layout.translocated {
        return Err(ApiError::new(
            "not_needed",
            "Jukeoke can already see its folder; there is nothing to choose.",
        ));
    }
    let handle = app.clone();
    blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let mut dialog = handle
            .dialog()
            .file()
            .set_title("Choose the Jukeoke stick's main folder (the one with Library and Mac in it)");
        if let Some(w) = crate::main_window(&handle) {
            dialog = dialog.set_parent(&w);
        }
        let picked = dialog.blocking_pick_folder();
        let Some(picked) = picked.and_then(|p| p.into_path().ok()) else {
            return Ok(RootChoice {
                ok: false,
                root: None,
                message: "No folder was chosen.".into(),
            });
        };
        let Some(root) = crate::portable::root_from_pick(&picked, None) else {
            return Ok(RootChoice {
                ok: false,
                root: None,
                message: format!(
                    "{} does not look like the Jukeoke stick: it has no Library folder. Choose the stick's main folder.",
                    picked.display()
                ),
            });
        };
        let user_dir = handle
            .path()
            .app_local_data_dir()
            .map_err(|e| ApiError::internal(format!("app_local_data_dir: {e}")))?;
        crate::portable::write_root_choice(&user_dir, &root)
            .map_err(|e| ApiError::io("the remembered folder choice", &e))?;
        crate::applog::info(&format!("translocated: root chosen {}", root.display()));
        // Restart so every path is set up from the chosen root.
        let h = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            h.request_restart();
        });
        Ok(RootChoice {
            ok: true,
            root: Some(root.display().to_string()),
            message: "Jukeoke will now restart using the chosen folder.".into(),
        })
    })
    .await
}

/// Answer of `ping` (Stage 0 IPC round-trip check).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingReply {
    echo: String,
    uptime_ms: u64,
}

/// Echoes the text back.
#[tauri::command]
pub async fn ping(state: State<'_, AppState>, text: String) -> ApiResult<PingReply> {
    Ok(PingReply {
        echo: text,
        uptime_ms: state.smoke.elapsed_ms() as u64,
    })
}

/// The UI's half of `--smoke-test` (see src/smoke.rs). Writes the report and exits.
/// Its payload keeps the Stage 0 camelCase shape (app/ui `SmokeReport`).
#[tauri::command]
pub async fn smoke_report(
    app: AppHandle,
    state: State<'_, AppState>,
    report: UiReport,
) -> ApiResult<()> {
    if !state.smoke.enabled() {
        return Err(ApiError::bad_request("the app was not started with --smoke-test"));
    }
    smoke::finish_with_ui_report(&app, report);
    Ok(())
}

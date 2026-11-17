// SPDX-License-Identifier: GPL-3.0-or-later
//! The Jukeoke desktop app: a Tauri 2 shell around the Rust core.
//!
//! Stage 0 scope: open the window, serve the web UI, answer a few commands, keep
//! every byte of state in the portable folder, and prove it with `--smoke-test`.
//! The audio engine, library and Studio supervisor plug in here later
//! (docs/ARCHITECTURE.md section 1).

mod commands;
pub mod fs_atomic;
pub mod portable;
pub mod screenshot;
pub mod smoke;
pub mod webview2_loader;

use std::path::PathBuf;
use std::time::Instant;

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use portable::DataLayout;
use smoke::Smoke;

/// Label of the one main window (also used in capabilities/default.json).
pub const MAIN_WINDOW: &str = "main";

/// State shared with every command.
pub struct AppState {
    pub layout: DataLayout,
    pub smoke: Smoke,
    /// Path of the running executable, when the OS could tell us.
    pub exe: Option<PathBuf>,
}

/// Runs the app and returns the process exit code.
pub fn run() -> i32 {
    let started = Instant::now();
    let smoke_enabled = std::env::args_os().skip(1).any(|a| a == smoke::FLAG);
    // Deliberately not canonicalised: on Windows that resolves a subst or mapped
    // drive letter to the underlying path, and the app must work from whatever path
    // it was actually started from.
    let exe = std::env::current_exe().ok();

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::ping,
            commands::smoke_report
        ])
        .setup(move |app| {
            let smoke = Smoke::new(smoke_enabled, started);
            smoke.mark("setup_started");

            // Where does our data live? Portable root, dev folder or user profile.
            let exe_for_layout = exe.clone().unwrap_or_default();
            let dev_env = std::env::var_os(portable::DEV_ENV_VAR).map(PathBuf::from);
            let layout = DataLayout::detect(&exe_for_layout, dev_env, || {
                app.path().app_local_data_dir().ok()
            })?;
            layout.create_dirs()?;

            app.manage(AppState { layout, smoke, exe });
            let handle = app.handle().clone();
            create_main_window(&handle)?;
            handle.state::<AppState>().smoke.mark("window_built");

            if smoke_enabled {
                smoke::start_watchdog(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    match app {
        Ok(app) => app.run_return(|_app, _event| {}),
        Err(err) => {
            // TODO(stage 7): show a native message box; release builds have no console.
            eprintln!("Jukeoke could not start: {err}");
            1
        }
    }
}

/// Creates the main window. The web view profile goes where `DataLayout` says:
/// on Windows a WebView2 user-data folder inside `Settings/`; on macOS a
/// non-persistent (incognito) WKWebView data store, so nothing is kept at all.
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let title = app.package_info().name.clone();

    let builder = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 600.0)
        .center()
        // Do not steal focus from whatever the developer is doing during a smoke test.
        .focused(!state.smoke.enabled())
        .on_page_load(|webview, payload| {
            let state = webview.state::<AppState>();
            match payload.event() {
                PageLoadEvent::Started => state.smoke.mark("page_load_started"),
                PageLoadEvent::Finished => state.smoke.mark("page_load_finished"),
            }
        });

    #[cfg(target_os = "macos")]
    let builder = builder.incognito(true);

    #[cfg(not(target_os = "macos"))]
    let builder = match &state.layout.webview_data_dir {
        Some(dir) => builder.data_directory(dir.clone()),
        None => builder,
    };

    builder.build()?;
    Ok(())
}

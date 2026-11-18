// SPDX-License-Identifier: GPL-3.0-or-later
//! The Jukeoke desktop app: a Tauri 2 shell around the Rust core.
//!
//! - [`core`]: everything the UI can ask for (docs/APP_API.md), testable without a
//!   window: library, playback on the audio engine, per-song settings saving, export,
//!   recorder, Studio supervisor, app settings, self-check.
//! - [`commands`]: the Tauri commands, thin wrappers around the core.
//! - This file: start-up (data layout, engine, window), command-line modes
//!   (`--smoke-test`, `--selfcheck [--silent]`), drag-and-drop, shut-down. Start-up
//!   failures end in a native error dialog and `Settings/logs/startup-error.txt`
//!   (src/startup.rs), never in a silent exit.

pub mod applog;
pub mod commands;
pub mod core;
pub mod debounce;
pub mod engine;
pub mod error;
pub mod events;
pub mod export;
pub mod fs_atomic;
pub mod keymode;
pub mod keys;
pub mod library;
pub mod medialink;
pub mod playback;
pub mod portable;
pub mod recorder;
pub mod recsession;
pub mod screenshot;
pub mod selfcheck;
pub mod settings;
pub mod smoke;
pub mod startup;
pub mod studio;
pub mod sysinfo;
pub mod testsong;
pub mod webview2_loader;

// The library's unit-test executable needs our application manifest (Common Controls
// v6: Tauri's menu crate imports TaskDialogIndirect), but Cargo has no build-script
// directive that reaches only that executable (`-tests` covers integration tests,
// the plain form would give the app's exe a second manifest). So the resource object
// tauri-build compiled into OUT_DIR is linked here, in unit-test builds only. See
// build.rs.
#[cfg(all(test, windows, target_env = "gnu"))]
#[link(name = "resource", kind = "static", modifiers = "+whole-archive,-bundle")]
unsafe extern "C" {}

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

use crate::core::{Core, CoreOptions};
use crate::engine::{AudioEngine, EngineRenderer, NoEngine};
use crate::portable::DataLayout;
use crate::smoke::Smoke;

/// Label of the main window (also used in capabilities/default.json).
pub const MAIN_WINDOW: &str = "main";
/// Label used if the window had to be created a second time with a fresh web view
/// profile (see [`portable::discard_webview_profile`]).
pub const MAIN_WINDOW_RETRY: &str = "main-retry";
/// Command-line flag: run the self-check, write the report, exit 0 (passed) or 1.
pub const SELFCHECK_FLAG: &str = "--selfcheck";
/// With `--selfcheck`: play digital silence instead of the -60 dBFS test tone.
pub const SILENT_FLAG: &str = "--silent";

/// State shared with every command.
pub struct AppState {
    pub layout: DataLayout,
    pub smoke: Smoke,
    /// Path of the running executable, when the OS could tell us.
    pub exe: Option<PathBuf>,
    pub core: Arc<Core>,
    /// Set in `--selfcheck` runs.
    pub selfcheck_cli: Option<selfcheck::Options>,
}

/// A start-up failure: what to tell the user, and the technical detail.
struct StartupError {
    message: String,
    detail: String,
}

impl StartupError {
    fn new(message: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        StartupError {
            message: message.into(),
            detail: detail.to_string(),
        }
    }
}

/// Adds plugins to the builder (main.rs registers the dialog plugin, see there).
pub type BuilderHook = fn(tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry>;

/// Runs the app and returns the process exit code. `plugins` adds the plugins the
/// executable registers (the dialog plugin, used by `pick_files` and `root_choose`).
pub fn run(plugins: BuilderHook) -> i32 {
    let started = Instant::now();
    // First, before any other thread exists: WEBVIEW2_USER_DATA_FOLDER would move the
    // web view profile off the stick; the app always chooses the folder itself.
    let webview_env_ignored = portable::neutralise_webview2_env()
        .map(|v| v.to_string_lossy().into_owned());
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let smoke_enabled = args.iter().any(|a| a == smoke::FLAG);
    let selfcheck_cli = args.iter().any(|a| a == SELFCHECK_FLAG).then(|| selfcheck::Options {
        user_agent: None,
        silent: args.iter().any(|a| a == SILENT_FLAG),
    });
    startup::set_unattended(smoke_enabled || selfcheck_cli.is_some());
    // Deliberately not canonicalised: on Windows that resolves a subst or mapped
    // drive letter to the underlying path, and the app must work from whatever path
    // it was actually started from.
    let exe = std::env::current_exe().ok();
    // A first guess of where start-up reports go, before the layout is known.
    if let Some(root) = exe
        .as_deref()
        .and_then(|e| portable::find_portable_root(e, portable::Platform::current()))
    {
        startup::set_logs_dir(&root.root.join(portable::SETTINGS_DIR).join(portable::LOGS_DIR));
    } else if let Some(dev) = std::env::var_os(portable::DEV_ENV_VAR) {
        startup::set_logs_dir(
            &PathBuf::from(dev)
                .join(portable::DEV_DATA_DIR)
                .join(portable::SETTINGS_DIR)
                .join(portable::LOGS_DIR),
        );
    }
    startup::install_panic_hook();

    let exe_for_setup = exe.clone();
    let selfcheck_for_setup = selfcheck_cli.clone();
    let built = plugins(tauri::Builder::default())
        .invoke_handler(all_commands!())
        .on_window_event(|window, event| {
            // Files dropped onto the window go to the Studio queue UI.
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                let _ = window
                    .app_handle()
                    .emit(events::names::STUDIO_DROPPED, serde_json::json!({ "paths": paths }));
            }
        })
        .setup(move |app| {
            // Never return an error from here: Tauri would turn it into a panic inside
            // the event loop. Report it ourselves and exit.
            if let Err(e) = setup(
                app,
                started,
                smoke_enabled,
                exe_for_setup.clone(),
                selfcheck_for_setup.clone(),
                webview_env_ignored.clone(),
            ) {
                startup::report(&e.message, &e.detail);
                std::process::exit(1);
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    match built {
        Ok(app) => app.run_return(|handle, event| {
            if let RunEvent::Exit = event
                && let Some(state) = handle.try_state::<AppState>()
            {
                state.core.shutdown();
            }
        }),
        Err(err) => {
            startup::report(
                &startup::plain_message_for_panic(&err.to_string()),
                &format!("tauri::Builder::build failed: {err}"),
            );
            1
        }
    }
}

fn setup(
    app: &mut tauri::App,
    started: Instant,
    smoke_enabled: bool,
    exe: Option<PathBuf>,
    selfcheck_cli: Option<selfcheck::Options>,
    webview_env_ignored: Option<String>,
) -> Result<(), StartupError> {
    let smoke = Smoke::new(smoke_enabled, started);
    smoke.mark("setup_started");

    // Where does our data live? Portable root, dev folder, user profile, or (macOS
    // translocation) a temporary folder until the user picks the stick.
    let exe_for_layout = exe.clone().unwrap_or_default();
    let dev_env = std::env::var_os(portable::DEV_ENV_VAR).map(PathBuf::from);
    let layout = DataLayout::detect(&exe_for_layout, dev_env, || app.path().app_local_data_dir().ok())
        .map_err(|e| StartupError::new("Jukeoke could not work out where to keep its data.", e))?;
    layout.create_dirs().map_err(|e| {
        StartupError::new(
            format!(
                "Jukeoke cannot write to its Settings folder ({}). The drive may be read-only (check the USB stick's lock switch) or full.",
                layout.settings_dir.display()
            ),
            e,
        )
    })?;
    let paths = layout.paths();
    startup::set_logs_dir(&paths.logs);
    applog::init(&paths.logs);
    applog::info(&format!(
        "start: {} {} on {}; mode {:?} ({}); data {}",
        app.package_info().name,
        app.package_info().version,
        sysinfo::os_version(),
        layout.mode,
        layout.reason,
        layout.data_root.display()
    ));
    if let Some(v) = &webview_env_ignored {
        applog::warn(&format!("ignored {}={v}", portable::WEBVIEW2_USER_DATA_ENV));
    }

    // The audio engine, on the saved device. A missing device is not an error (the
    // engine reports it on play); a failure to start at all leaves the rest of the app
    // usable.
    let saved = settings::read_file(&paths.settings);
    let engine: Arc<dyn AudioEngine> = match jk_engine::Engine::new(jk_engine::EngineConfig {
        device_id: saved.audio.device_id.clone(),
        exclusive: saved.audio.exclusive,
        master: saved.audio.master_volume as f32,
    }) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            applog::error(&format!("audio engine failed to start: {e}"));
            Arc::new(NoEngine::new(e))
        }
    };

    let version = app.package_info().version.to_string();
    let exe_dir = exe.as_deref().and_then(Path::parent);
    let root = layout.portable_root.clone().unwrap_or_else(|| layout.data_root.clone());
    let studio_cfg = studio::StudioConfig::locate(
        exe_dir,
        layout.mode == portable::DataMode::Dev,
        &paths,
        &root,
        &version,
    );
    let core = Core::new(CoreOptions {
        layout: layout.clone(),
        exe: exe.clone(),
        name: app.package_info().name.clone(),
        version,
        identifier: app.config().identifier.clone(),
        sink: Arc::new(events::TauriSink(app.handle().clone())),
        engine,
        renderer: Arc::new(EngineRenderer),
        studio: studio_cfg,
        demo_song: find_demo_song(app.handle()),
        slot_save_delay: Duration::from_millis(1000),
        webview_env_ignored,
    });

    let selfcheck_mode = selfcheck_cli.is_some();
    app.manage(AppState {
        layout: layout.clone(),
        smoke,
        exe,
        core: Arc::clone(&core),
        selfcheck_cli,
    });
    let handle = app.handle().clone();

    // The window. If the web view cannot be created, the profile may be damaged or
    // left by a newer WebView2 on another PC: discard it and try once more.
    if let Err(first) = create_main_window(&handle, MAIN_WINDOW, selfcheck_mode) {
        applog::error(&format!("main window failed: {first}"));
        let retried = layout
            .webview_data_dir
            .as_deref()
            .is_some_and(portable::discard_webview_profile);
        if !retried {
            return Err(StartupError::new(startup::plain_message_for_panic(&first.to_string()), first));
        }
        applog::warn("discarded the web view profile; retrying with a fresh one");
        let _ = std::fs::create_dir_all(layout.webview_data_dir.as_deref().unwrap_or(&layout.settings_dir));
        create_main_window(&handle, MAIN_WINDOW_RETRY, selfcheck_mode).map_err(|e| {
            StartupError::new(
                startup::plain_message_for_panic(&e.to_string()),
                format!("first attempt: {first}; with a fresh profile: {e}"),
            )
        })?;
    }
    handle.state::<AppState>().smoke.mark("window_built");
    startup::finished();

    if selfcheck_mode {
        start_cli_selfcheck_fallback(handle.clone());
    } else {
        core.start_background();
    }
    if smoke_enabled {
        smoke::start_watchdog(handle);
    }
    Ok(())
}

/// The bundled demo song: a Tauri resource (`resources/demo-song.jksong`), or the
/// generated fixture in the source tree on a developer machine.
fn find_demo_song(app: &AppHandle) -> Option<PathBuf> {
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("resources").join("demo-song.jksong"));
    let source_tree = crate::portable::SOURCE_DIR
        .map(|d| Path::new(d).join("../../tests/fixtures/generated/demo-song.jksong"));
    bundled.into_iter().chain(source_tree).find(|p| p.is_file())
}

/// Creates the main window. The web view profile goes where `DataLayout` says:
/// on Windows a WebView2 user-data folder inside `Settings/`; on macOS a
/// non-persistent (incognito) WKWebView data store, so nothing is kept at all.
fn create_main_window(app: &AppHandle, label: &str, hidden: bool) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let title = app.package_info().name.clone();
    let selfcheck = state.selfcheck_cli.is_some();

    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 600.0)
        .center()
        .visible(!hidden)
        // Do not steal focus from whatever the developer is doing during a smoke test.
        .focused(!state.smoke.enabled() && !hidden)
        .on_page_load(move |webview, payload| {
            let state = webview.state::<AppState>();
            match payload.event() {
                PageLoadEvent::Started => state.smoke.mark("page_load_started"),
                PageLoadEvent::Finished => {
                    state.smoke.mark("page_load_finished");
                    if selfcheck {
                        // --selfcheck: the report records the real web engine. The page
                        // reports navigator.userAgent through the normal IPC.
                        let _ = webview.eval(
                            "window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke('selfcheck_user_agent', { user_agent: navigator.userAgent })",
                        );
                    }
                }
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

/// The main window (whichever label it got).
pub fn main_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window(MAIN_WINDOW)
        .or_else(|| app.get_webview_window(MAIN_WINDOW_RETRY))
}

static CLI_SELFCHECK_STARTED: AtomicBool = AtomicBool::new(false);

/// `--selfcheck`: the page reported its user agent; run now.
pub(crate) fn cli_selfcheck_user_agent(app: &AppHandle, user_agent: String) {
    let state = app.state::<AppState>();
    *state.core.user_agent.lock().unwrap_or_else(|e| e.into_inner()) = Some(user_agent);
    if state.selfcheck_cli.is_some() {
        start_cli_selfcheck(app.clone());
    }
}

/// `--selfcheck`: run even if the page never reports (after 20 s).
fn start_cli_selfcheck_fallback(app: AppHandle) {
    let _ = std::thread::Builder::new()
        .name("selfcheck-fallback".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            start_cli_selfcheck(app);
        });
}

fn start_cli_selfcheck(app: AppHandle) {
    if CLI_SELFCHECK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("selfcheck".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            let mut opts = state.selfcheck_cli.clone().unwrap_or_default();
            opts.user_agent = state.core.user_agent.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let result = selfcheck::run(&state.core, &opts);
            {
                use std::io::Write;
                let mut out = std::io::stdout();
                for i in &result.items {
                    let _ = writeln!(out, "[{}] {}: {}", if i.ok { " OK " } else { "FAIL" }, i.name, i.detail);
                }
                let _ = writeln!(
                    out,
                    "selfcheck: {} report={}",
                    if result.passed { "passed" } else { "FAILED" },
                    result.report_path
                );
            }
            app.exit(if result.passed { 0 } else { 1 });
        });
}

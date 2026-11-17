// SPDX-License-Identifier: GPL-3.0-or-later
//! `--smoke-test`: an end-to-end self-test of the app shell.
//!
//! Started as `Jukeoke.exe --smoke-test`, the app opens its window as usual. The
//! web UI asks Rust for `app_info`, renders it, waits for the browser to paint, then
//! calls `smoke_report` with its own measurements. Rust then saves what the web
//! view rendered as `Settings/smoke-test.png` (Windows), adds its timings, the web
//! view version and the location of the web view profile, writes
//! `<data root>/Settings/smoke-test.json` atomically and exits:
//!
//! - exit code 0: every check passed;
//! - exit code 1: the UI reported, but a check failed (details in the JSON);
//! - exit code 2: the UI never reported within the timeout (JS did not run, the
//!   page did not load, or the web view did not start), or the screenshot and
//!   report step after it did not finish within the timeout;
//! - exit code 3: the report was written but the app did not exit (forced exit).
//!
//! This is the proof for WORK_ORDER section 9 "Portability" on Windows: run it from
//! a portable layout on another drive letter and inspect the report.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::AppState;
use crate::portable::DataLayout;

/// Command-line flag that turns the smoke test on.
pub const FLAG: &str = "--smoke-test";
/// Report file name, inside the Settings folder.
pub const REPORT_FILE: &str = "smoke-test.json";
/// Screenshot of the rendered page, inside the Settings folder (Windows only for now).
pub const SCREENSHOT_FILE: &str = "smoke-test.png";
/// How long the UI gets to report before the test fails.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// Overrides [`TIMEOUT`] in milliseconds; used to test the failure path.
pub const TIMEOUT_ENV: &str = "JK_SMOKE_TIMEOUT_MS";

fn timeout() -> Duration {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(TIMEOUT, Duration::from_millis)
}

/// What the web UI measured (app/ui/src/api.ts `SmokeReport`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiReport {
    pub script_start_ms: f64,
    pub first_paint_ms: Option<f64>,
    pub first_contentful_paint_ms: Option<f64>,
    pub report_sent_ms: f64,
    pub app_info_round_trip_ms: f64,
    pub rendered_title: String,
    pub user_agent: String,
    pub viewport: Viewport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
}

/// A named moment, in milliseconds since `main()` started.
#[derive(Debug, Clone, Serialize)]
struct Mark {
    name: &'static str,
    ms: f64,
}

#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    pass: bool,
    detail: String,
}

/// Smoke-test state shared by the setup hook, the page-load hook, commands and the
/// watchdog thread.
pub struct Smoke {
    enabled: bool,
    started: Instant,
    marks: Mutex<Vec<Mark>>,
    /// Someone (the UI report or the watchdog) has taken charge of finishing.
    finished: AtomicBool,
    /// The report file has been written.
    reported: AtomicBool,
}

impl Smoke {
    pub fn new(enabled: bool, started: Instant) -> Self {
        Smoke {
            enabled,
            started,
            marks: Mutex::new(Vec::new()),
            finished: AtomicBool::new(false),
            reported: AtomicBool::new(false),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Milliseconds since `main()` started.
    pub fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// Records the first occurrence of a named moment (later repeats are ignored,
    /// e.g. a second `app_info` call).
    pub fn mark(&self, name: &'static str) {
        if !self.enabled {
            return;
        }
        let ms = self.elapsed_ms();
        if let Ok(mut marks) = self.marks.lock()
            && !marks.iter().any(|m| m.name == name)
        {
            marks.push(Mark { name, ms });
        }
    }

    /// True for exactly one caller: the one allowed to write the report and exit.
    fn claim_finish(&self) -> bool {
        self.enabled && !self.finished.swap(true, Ordering::SeqCst)
    }
}

/// Called by the `smoke_report` command: takes the screenshot, then writes the
/// report and asks the app to exit.
pub fn finish_with_ui_report(app: &AppHandle, ui: UiReport) {
    let state = app.state::<AppState>();
    state.smoke.mark("ui_report_received");
    if !state.smoke.claim_finish() {
        return;
    }
    let png = state.layout.settings_dir.join(SCREENSHOT_FILE);
    let handle = app.clone();
    let done: crate::screenshot::Done = Box::new(move |shot| {
        let state = handle.state::<AppState>();
        state.smoke.mark("screenshot_done");
        let code = write_report(&handle, &state, Some(ui), None, Some(shot));
        handle.exit(code);
    });
    let window = app
        .get_webview_window(crate::MAIN_WINDOW)
        .or_else(|| app.get_webview_window(crate::MAIN_WINDOW_RETRY));
    match window {
        Some(window) => crate::screenshot::capture_png(&window, png, done),
        None => done(Err("the main window is gone".into())),
    }
}

/// Starts the watchdog: if the UI has not reported after [`TIMEOUT`] (or
/// `$JK_SMOKE_TIMEOUT_MS`), write a
/// failure report and exit with code 2 (and force the exit 5 s later if the event
/// loop is stuck).
pub fn start_watchdog(app: AppHandle) {
    std::thread::Builder::new()
        .name("smoke-test-watchdog".into())
        .spawn(move || {
            let limit = timeout();
            std::thread::sleep(limit);
            let state = app.state::<AppState>();
            let secs = limit.as_secs_f64();
            let reason = if state.smoke.claim_finish() {
                Some(format!("the UI did not report within {secs} s"))
            } else if !state.smoke.reported.load(Ordering::SeqCst) {
                Some(format!("the UI reported, but the screenshot and report step did not finish within {secs} s"))
            } else {
                None // report written; only the exit itself is stuck
            };
            let code = match reason {
                Some(reason) => {
                    write_report(&app, &state, None, Some(reason), None);
                    app.exit(2);
                    2
                }
                None => 3,
            };
            // The normal path exits long before this; still alive means a hang.
            std::thread::sleep(Duration::from_secs(5));
            std::process::exit(code);
        })
        .expect("spawn smoke-test watchdog");
}

fn path_string(p: &Path) -> String {
    p.display().to_string()
}

/// Lists the names directly inside `dir` (sorted), or an empty list.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn checks(
    app: &AppHandle,
    layout: &DataLayout,
    ui: Option<&UiReport>,
    webview_version: &Result<String, String>,
    screenshot: Option<&Result<u64, String>>,
) -> Vec<Check> {
    let mut out = Vec::new();
    let name = app.package_info().name.clone();
    out.push(match ui {
        Some(ui) => Check {
            name: "ui_rendered_app_name",
            pass: ui.rendered_title == name,
            detail: format!(
                "<h1> read back from the DOM: {:?}, expected {:?}",
                ui.rendered_title, name
            ),
        },
        None => Check {
            name: "ui_reported",
            pass: false,
            detail: "the UI never called smoke_report".into(),
        },
    });
    out.push(Check {
        name: "webview_version_known",
        pass: webview_version.is_ok(),
        detail: webview_version.clone().unwrap_or_else(|e| e),
    });
    if cfg!(all(windows, target_env = "gnu")) {
        let loader = crate::webview2_loader::info();
        out.push(Check {
            name: "webview2_builtin_loader",
            pass: loader.route == "built-in",
            detail: format!(
                "route: {}; runtime: {}",
                loader.route,
                loader.runtime_dll.as_deref().unwrap_or("-")
            ),
        });
    }
    if cfg!(windows) {
        out.push(Check {
            name: "screenshot_saved",
            pass: matches!(screenshot, Some(Ok(bytes)) if *bytes > 0),
            detail: match screenshot {
                Some(Ok(bytes)) => format!("{SCREENSHOT_FILE}: {bytes} bytes"),
                Some(Err(e)) => e.clone(),
                None => "not taken".into(),
            },
        });
    }
    if let Some(dir) = &layout.webview_data_dir {
        let inside = dir.starts_with(&layout.settings_dir);
        let names = entries(dir);
        out.push(Check {
            name: "webview_profile_inside_settings",
            pass: inside && !names.is_empty(),
            detail: format!("{} contains {:?}", path_string(dir), names),
        });
    }
    out
}

fn write_report(
    app: &AppHandle,
    state: &AppState,
    ui: Option<UiReport>,
    failure: Option<String>,
    screenshot: Option<Result<u64, String>>,
) -> i32 {
    let layout = &state.layout;
    let webview_version = tauri::webview_version().map_err(|e| format!("unknown: {e}"));
    let checks = checks(
        app,
        layout,
        ui.as_ref(),
        &webview_version,
        screenshot.as_ref(),
    );
    let ok = failure.is_none() && checks.iter().all(|c| c.pass);
    let marks = state
        .smoke
        .marks
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();
    let written_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let pkg = app.package_info();

    let report = serde_json::json!({
        "schema": "jukeoke-smoke-test/1",
        "ok": ok,
        "failure": failure,
        "checks": checks,
        "app": {
            "name": pkg.name,
            "version": pkg.version.to_string(),
            "identifier": app.config().identifier,
            "exe": state.exe.as_deref().map(path_string),
        },
        "system": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "webview": {
            "version": webview_version.as_ref().ok(),
            "error": webview_version.as_ref().err(),
            "loader": crate::webview2_loader::info(),
            "user_data_dir": layout.webview_data_dir.as_deref().map(path_string),
            "user_data_dir_entries": layout.webview_data_dir.as_deref().map(entries),
        },
        "layout": {
            "mode": layout.mode,
            "reason": layout.reason,
            "portable_root": layout.portable_root.as_deref().map(path_string),
            "data_root": path_string(&layout.data_root),
            "settings_dir": path_string(&layout.settings_dir),
            "translocated": layout.translocated,
        },
        "screenshot": match &screenshot {
            Some(Ok(bytes)) => serde_json::json!({ "file": SCREENSHOT_FILE, "bytes": bytes }),
            Some(Err(e)) => serde_json::json!({ "error": e }),
            None => serde_json::Value::Null,
        },
        "timings_ms_since_main": marks,
        "total_ms_since_main": state.smoke.elapsed_ms(),
        "ui": ui,
        "written_at_unix_ms": written_at_unix_ms,
    });

    let path: PathBuf = layout.settings_dir.join(REPORT_FILE);
    let text = serde_json::to_string_pretty(&report)
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"failure\":\"serialise: {e}\"}}"));
    let written = crate::fs_atomic::write_atomic(&path, text.as_bytes());
    state.smoke.reported.store(true, Ordering::SeqCst);
    let code = match written {
        Ok(()) if ok => 0,
        Ok(()) if ui.is_none() => 2,
        Ok(()) => 1,
        Err(_) => 2,
    };
    // Release builds have no console; when started from a terminal with redirected
    // output this still reaches it. Errors are ignored on purpose.
    {
        use std::io::Write;
        let _ = writeln!(
            std::io::stdout(),
            "smoke-test: ok={ok} exit={code} report={}",
            path.display()
        );
    }
    code
}

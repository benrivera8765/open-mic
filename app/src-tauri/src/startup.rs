// SPDX-License-Identifier: GPL-3.0-or-later
//! Start-up failures are shown, not swallowed (Stage 0 verifier finding).
//!
//! A release build has no console, and Tauri turns an error returned from the setup
//! hook into a panic inside the event loop. So every start-up failure, whether an
//! error we return or a panic, ends here:
//!
//! 1. the details go to `<Settings>/logs/startup-error.txt` (plain text, for the
//!    owner to bring back), next to the app's log;
//! 2. a native error dialog says in plain words what happened and where the details
//!    are (Windows `MessageBoxW`, macOS `NSAlert` through rfd);
//! 3. the process exits with code 1.
//!
//! In unattended runs (`--smoke-test`, `--selfcheck`) no dialog is shown: nobody is
//! there to click it. After start-up, panics in background threads are written to
//! `app.log` and do not show a dialog.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// File name of the start-up error report.
pub const REPORT_FILE: &str = "startup-error.txt";

static STARTING: AtomicBool = AtomicBool::new(true);
static UNATTENDED: AtomicBool = AtomicBool::new(false);
static LOGS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where reports go. Called as soon as anything is known (first a guess from the
/// portable layout, then the real Settings folder).
pub fn set_logs_dir(dir: &Path) {
    *LOGS_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
}

/// No dialogs in `--smoke-test` / `--selfcheck` runs.
pub fn set_unattended(on: bool) {
    UNATTENDED.store(on, Ordering::SeqCst);
}

/// The window is up: from now on panics are logged, not shown.
pub fn finished() {
    STARTING.store(false, Ordering::SeqCst);
}

pub fn is_starting() -> bool {
    STARTING.load(Ordering::SeqCst)
}

fn logs_dir() -> PathBuf {
    LOGS_DIR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(std::env::temp_dir)
}

/// Writes the report and shows the dialog. Returns the report path.
pub fn report(message: &str, detail: &str) -> PathBuf {
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(REPORT_FILE);
    let text = format!(
        "Open Mic could not start\r\n\
         =======================\r\n\
         time (UTC): {}\r\n\
         app version: {}\r\n\
         system: {} ({} {})\r\n\
         executable: {}\r\n\r\n\
         What happened:\r\n{message}\r\n\r\n\
         Details:\r\n{detail}\r\n",
        jk_format::time::utc_now_iso8601(),
        env!("CARGO_PKG_VERSION"),
        crate::sysinfo::os_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into()),
    );
    let written = crate::fs_atomic::write_atomic(&path, text.as_bytes()).is_ok();
    crate::applog::error(&format!("start-up failed: {message} | {detail}"));
    eprintln!("Open Mic could not start: {message}\n{detail}");
    if !UNATTENDED.load(Ordering::SeqCst) {
        let where_ = if written {
            format!("\n\nThe details were saved in:\n{}", path.display())
        } else {
            String::new()
        };
        show_error_dialog("Open Mic could not start", &format!("{message}{where_}"));
    }
    path
}

/// A native, modal error message box. Safe to call before any window exists.
pub fn show_error_dialog(title: &str, text: &str) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TASKMODAL, MessageBoxW,
        };
        use windows::core::HSTRING;
        // SAFETY: plain call with owned, NUL-terminated wide strings; no owner window.
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(text),
                &HSTRING::from(title),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TASKMODAL,
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (title, text);
    }
}

/// Installs the panic hook: during start-up a panic becomes a report + dialog;
/// afterwards it is logged. The previous hook (which prints to stderr) still runs.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed").to_string();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let detail = format!("panic in thread '{name}' at {location}: {msg}\r\n\r\n{backtrace}");
        if is_starting() {
            report(&plain_message_for_panic(&msg), &detail);
        } else {
            crate::applog::error(&detail);
        }
        previous(info);
    }));
}

/// Turns a panic text from deep inside Tauri / WebView2 into something a person can
/// act on.
pub fn plain_message_for_panic(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("webview2") || lower.contains("webview") || lower.contains("0x80070002") {
        "The app's window could not be created because the Microsoft Edge WebView2 Runtime is missing or damaged. \
         It is part of Windows 11; on Windows 10 install it from Microsoft (search for \"WebView2 Runtime\"), then start Open Mic again."
            .into()
    } else if lower.contains("access is denied") || lower.contains("permission denied") || lower.contains("read-only") {
        "Open Mic cannot write to its folder (the drive may be read-only or full). Check the USB stick's lock switch and free space, then start Open Mic again."
            .into()
    } else {
        "Open Mic hit an unexpected error while starting. Please send the details file to whoever maintains your copy of Open Mic."
            .into()
    }
}

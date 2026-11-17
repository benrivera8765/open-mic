// SPDX-License-Identifier: GPL-3.0-or-later
//! Saves what the web view has actually rendered as a PNG file.
//!
//! Windows: WebView2's `CapturePreview` renders the page into a memory stream.
//! Used by `--smoke-test` as visual proof that the UI rendered (and later for the
//! screenshots WORK_ORDER section 9 asks for). Not implemented on macOS yet.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::WebviewWindow;

/// Called exactly once with the PNG size in bytes, or an error text.
pub type Done = Box<dyn FnOnce(Result<u64, String>) + Send + 'static>;

/// Makes `done` callable from whichever path finishes first (success or error).
fn once(done: Done) -> impl Fn(Result<u64, String>) + Clone + Send + 'static {
    let slot = Arc::new(Mutex::new(Some(done)));
    move |result| {
        if let Some(done) = slot.lock().ok().and_then(|mut s| s.take()) {
            done(result);
        }
    }
}

#[cfg(windows)]
pub fn capture_png(window: &WebviewWindow, path: PathBuf, done: Done) {
    let finish = once(done);
    let finish_on_error = finish.clone();
    let posted = window.with_webview(move |platform| {
        // Runs on the UI thread, which owns the WebView2 objects.
        let started = start_capture(platform, path, finish.clone());
        if let Err(err) = started {
            finish(Err(err));
        }
    });
    if let Err(err) = posted {
        finish_on_error(Err(format!("could not reach the web view: {err}")));
    }
}

#[cfg(windows)]
fn start_capture(
    platform: tauri::webview::PlatformWebview,
    path: PathBuf,
    finish: impl Fn(Result<u64, String>) + Clone + Send + 'static,
) -> Result<(), String> {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::UI::Shell::SHCreateMemStream;

    // SAFETY: plain COM calls on the UI thread with valid interface pointers.
    unsafe {
        let core = platform
            .controller()
            .CoreWebView2()
            .map_err(|e| format!("CoreWebView2: {e}"))?;
        let stream = SHCreateMemStream(None).ok_or("SHCreateMemStream failed")?;
        let stream_for_handler = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            let saved = result
                .map_err(|e| format!("CapturePreview: {e}"))
                .and_then(|()| save_stream(&stream_for_handler, &path));
            finish(saved);
            Ok(())
        }));
        core.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
        .map_err(|e| format!("CapturePreview: {e}"))
    }
}

/// Copies the whole memory stream to `path` (atomically) and returns its size.
#[cfg(windows)]
unsafe fn save_stream(
    stream: &windows::Win32::System::Com::IStream,
    path: &std::path::Path,
) -> Result<u64, String> {
    use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};

    // SAFETY: `stream` is a valid IStream; the buffer is sized from Stat().
    unsafe {
        let mut stat = STATSTG::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .map_err(|e| format!("IStream::Stat: {e}"))?;
        let len = usize::try_from(stat.cbSize).map_err(|_| "image too large")?;
        let mut bytes = vec![0u8; len];
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(|e| format!("IStream::Seek: {e}"))?;
        let mut read = 0u32;
        stream
            .Read(bytes.as_mut_ptr().cast(), len as u32, Some(&mut read))
            .ok()
            .map_err(|e| format!("IStream::Read: {e}"))?;
        bytes.truncate(read as usize);
        crate::fs_atomic::write_atomic(path, &bytes)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(bytes.len() as u64)
    }
}

#[cfg(not(windows))]
pub fn capture_png(_window: &WebviewWindow, _path: PathBuf, done: Done) {
    let finish = once(done);
    finish(Err(
        "screenshots are not implemented on this platform yet".into()
    ));
}

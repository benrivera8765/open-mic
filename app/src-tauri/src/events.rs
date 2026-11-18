// SPDX-License-Identifier: GPL-3.0-or-later
//! Events from the Rust core to the web UI (docs/APP_API.md), behind a small trait so
//! the core can be tested without a window: the app uses [`TauriSink`], tests use
//! [`RecordingSink`].

use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;

/// Event names (docs/APP_API.md). Additions beyond the contract are marked.
pub mod names {
    pub const LIBRARY_CHANGED: &str = "library://changed";
    /// Addition (owner's request, 2026-11-18): the full `library_deleted_list()` after
    /// songs were deleted, restored or deleted for good (src/deleted.rs).
    pub const LIBRARY_DELETED: &str = "library://deleted";
    pub const PLAYBACK_CLOCK: &str = "playback://clock";
    pub const PLAYBACK_STATE: &str = "playback://state";
    pub const PLAYBACK_ERROR: &str = "playback://error";
    pub const EXPORT_PROGRESS: &str = "export://progress";
    pub const EXPORT_DONE: &str = "export://done";
    pub const EXPORT_FAILED: &str = "export://failed";
    pub const RECORDER_LEVEL: &str = "recorder://level";
    /// Addition (owner's request, 2026-11-18): a `RecordingSession` whenever a
    /// "record songs separately" session starts, stops, splits or finishes splitting.
    pub const RECORDER_SESSION: &str = "recorder://session";
    /// Addition: `{ path, playing }` when a recording preview starts or ends.
    pub const RECORDER_PREVIEW: &str = "recorder://preview";
    pub const STUDIO_JOB: &str = "studio://job";
    pub const STUDIO_MODELS: &str = "studio://models";
    pub const STUDIO_DROPPED: &str = "studio://dropped";
    /// Addition to APP_API v1: a full `studio_status()` payload whenever the Studio's
    /// availability or running state changes (the capability probe finishes in the
    /// background after start-up).
    pub const STUDIO_STATUS: &str = "studio://status";
}

/// Somewhere events go.
pub trait EventSink: Send + Sync + 'static {
    fn emit_value(&self, event: &str, payload: Value);
}

/// Serialises and emits; serialisation of our own types cannot fail in practice, and
/// a failure is logged rather than panicking a worker thread.
pub fn emit<T: Serialize>(sink: &dyn EventSink, event: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(v) => sink.emit_value(event, v),
        Err(e) => crate::applog::error(&format!("cannot serialise event {event}: {e}")),
    }
}

/// Emits to every window of the app.
pub struct TauriSink(pub tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit_value(&self, event: &str, payload: Value) {
        use tauri::Emitter;
        if let Err(e) = self.0.emit(event, payload) {
            crate::applog::warn(&format!("emit {event} failed: {e}"));
        }
    }
}

/// Drops everything (headless runs such as `--selfcheck` before a window exists).
pub struct NullSink;

impl EventSink for NullSink {
    fn emit_value(&self, _event: &str, _payload: Value) {}
}

/// Keeps every event, for tests.
#[derive(Default)]
pub struct RecordingSink {
    pub events: Mutex<Vec<(String, Value)>>,
}

impl RecordingSink {
    /// Payloads of all events with this name so far.
    pub fn of(&self, name: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl EventSink for RecordingSink {
    fn emit_value(&self, event: &str, payload: Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

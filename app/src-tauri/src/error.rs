// SPDX-License-Identifier: GPL-3.0-or-later
//! The error every command returns (docs/APP_API.md "Conventions"): the promise in the
//! UI rejects with `{ code, message }`, where `message` is plain language for the user
//! and `code` is a stable machine-readable word. Technical detail goes to the log
//! (src/applog.rs), never into `message`.

use serde::Serialize;

/// `{ code, message }` as the UI receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

/// Result type of every command handler.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ApiError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A bad argument from the UI (a programming error on the UI side, but still
    /// explained in words).
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new("bad_request", message)
    }

    /// Something unexpected; `detail` is logged, the user sees a generic sentence.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        crate::applog::error(&format!("internal error: {detail}"));
        Self::new(
            "internal",
            "Something went wrong inside Jukeoke. The details are in Settings/logs/app.log.",
        )
    }

    /// A file-system error while reading or writing `what` (a short noun phrase such as
    /// "the settings file").
    pub fn io(what: &str, err: &std::io::Error) -> Self {
        crate::applog::error(&format!("I/O error on {what}: {err}"));
        let message = match err.kind() {
            std::io::ErrorKind::NotFound => format!("Jukeoke could not find {what}."),
            std::io::ErrorKind::PermissionDenied => format!(
                "Jukeoke is not allowed to write {what} (permission denied, or the drive is read-only)."
            ),
            _ => format!("Jukeoke could not read or write {what}: {err}"),
        };
        Self::new("io", message)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<jk_format::FormatError> for ApiError {
    fn from(e: jk_format::FormatError) -> Self {
        use jk_format::ErrorKind as K;
        crate::applog::warn(&format!("song file error: {e}"));
        let code = match e.kind() {
            K::NotASongFile => "not_a_song",
            K::NewerVersion => "newer_version",
            K::Truncated => "truncated",
            K::Damaged | K::MissingChunk => "damaged",
            K::SlotPayloadTooLarge => "slot_too_large",
            K::ChangedOnDisk => "changed_on_disk",
            K::InvalidInput => "internal",
            K::Io => "io",
        };
        ApiError::new(code, e.user_message())
    }
}

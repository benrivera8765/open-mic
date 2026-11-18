// SPDX-License-Identifier: GPL-3.0-or-later
//! Error type for the recorder. Every variant carries enough text to act on
//! without a second round trip (WORK_ORDER section 4.10: "make each failure
//! message specific enough to act on").

use std::fmt;

/// Everything that can go wrong while starting, running or stopping a recording.
#[derive(Debug)]
pub enum RecorderError {
    /// A backend feature that exists in the API but has not been written yet.
    /// (No current backend returns it; kept so the API stays stable.)
    NotImplemented(&'static str),
    /// This platform or OS version cannot capture system audio at all.
    Unsupported(String),
    /// The user has not granted (or has revoked) the permission the OS requires.
    /// On macOS this is "Screen Recording" (System Settings > Privacy & Security).
    PermissionDenied(String),
    /// An OS API returned an error. `domain`/`code` are the raw values (for macOS
    /// an `NSError` domain and code, for Windows an HRESULT) so a report can be
    /// matched against Apple's or Microsoft's documentation.
    Backend {
        /// Where the error came from, e.g. `SCStreamErrorDomain`.
        domain: String,
        /// The raw numeric code.
        code: i64,
        /// Human-readable description, including what we were doing.
        message: String,
    },
    /// The OS did not answer within the time we allow.
    Timeout(&'static str),
    /// The OS delivered audio in a layout we do not handle (we never convert
    /// silently; see `CaptureFormat`).
    Format(String),
    /// A file-system error while writing the recording.
    Io(std::io::Error),
    /// `stop()` was called on a recorder whose worker already ended.
    AlreadyStopped,
    /// The media player is not open, refused a command, or cannot be followed
    /// (see [`crate::media`]). The text says what to do, in plain words.
    Player(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecorderError::NotImplemented(what) => write!(f, "not implemented yet: {what}"),
            RecorderError::Unsupported(why) => {
                write!(f, "system audio capture is not supported here: {why}")
            }
            RecorderError::PermissionDenied(why) => write!(f, "permission denied: {why}"),
            RecorderError::Backend {
                domain,
                code,
                message,
            } => {
                write!(f, "{message} (error domain {domain}, code {code})")
            }
            RecorderError::Timeout(what) => write!(f, "timed out while {what}"),
            RecorderError::Format(why) => write!(f, "unsupported capture format: {why}"),
            RecorderError::Io(e) => write!(f, "file error while recording: {e}"),
            RecorderError::AlreadyStopped => write!(f, "the recorder has already stopped"),
            RecorderError::Player(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for RecorderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RecorderError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RecorderError {
    fn from(e: std::io::Error) -> Self {
        RecorderError::Io(e)
    }
}

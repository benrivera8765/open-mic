// SPDX-License-Identifier: GPL-3.0-or-later
//! Platforms without a media-player link (macOS has no public API that lets one app
//! see or control another app's playback; Linux would use MPRIS, not written).

use super::{MediaObservation, MediaSessionInfo, Thumbnail};
use crate::error::RecorderError;

const WHY: &str = "Following and controlling the music player works on Windows only for now. \
Start the song in your player yourself, then record.";

fn unsupported() -> RecorderError {
    RecorderError::Unsupported(WHY.to_string())
}

pub(super) fn available() -> Result<(), String> {
    Err(WHY.to_string())
}

pub(super) fn sessions() -> Result<Vec<MediaSessionInfo>, RecorderError> {
    Err(unsupported())
}

pub(super) fn play(_id: &str) -> Result<(), RecorderError> {
    Err(unsupported())
}

pub(super) fn pause(_id: &str) -> Result<(), RecorderError> {
    Err(unsupported())
}

pub(super) fn thumbnail(_id: &str) -> Result<Option<Thumbnail>, RecorderError> {
    Err(unsupported())
}

pub(super) struct Watch;

impl Watch {
    pub(super) fn start(
        _id: &str,
        _on_change: Box<dyn Fn(MediaObservation) + Send + 'static>,
    ) -> Result<Watch, RecorderError> {
        Err(unsupported())
    }

    pub(super) fn alive(&self) -> bool {
        false
    }
}

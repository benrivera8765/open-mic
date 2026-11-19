// SPDX-License-Identifier: GPL-3.0-or-later
//! Platforms without a microphone backend (Linux and others). Everything
//! compiles; listing returns nothing and opening a device says why. Generated
//! and file sources still work.

use super::InputError;
use super::ring::{CaptureProducer, FailureKind};
use super::{InputDeviceInfo, InputFormat, MicPermission};

pub(super) struct Capture;

impl Capture {
    pub(super) fn new(_id: String) -> Self {
        Capture
    }
    pub(super) fn open(&mut self) -> Result<InputFormat, InputError> {
        Err(InputError::new(
            FailureKind::NoDevice,
            "microphone input is built for Windows and macOS only",
        ))
    }
    pub(super) fn start(&mut self, _producer: CaptureProducer) -> Result<(), InputError> {
        Err(InputError::new(
            FailureKind::NoDevice,
            "microphone input is built for Windows and macOS only",
        ))
    }
    pub(super) fn stop(&mut self) {}
}

pub(super) fn list_input_devices() -> Result<Vec<InputDeviceInfo>, InputError> {
    Ok(Vec::new())
}

pub(super) fn mic_permission() -> MicPermission {
    MicPermission::Granted
}

pub(super) fn request_mic_permission() -> MicPermission {
    MicPermission::Granted
}

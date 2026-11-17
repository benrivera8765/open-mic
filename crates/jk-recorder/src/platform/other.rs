// SPDX-License-Identifier: GPL-3.0-or-later
//! Platforms without a backend (Linux and others; WORK_ORDER section 12 says
//! "do not block one, do not make one"). Everything compiles; recording reports
//! that it is unsupported.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Instant;

use crate::error::RecorderError;
use crate::format::{CaptureDevice, CaptureSetup, OutputDeviceInfo};
use crate::pipeline::CapturePipeline;
use crate::{Backend, PermissionState, RecorderConfig};

pub(crate) const BACKEND: Backend = Backend::None;

pub(crate) fn permission_preflight() -> PermissionState {
    PermissionState::NotRequired
}

pub(crate) fn permission_request() -> PermissionState {
    PermissionState::NotRequired
}

pub(crate) fn output_device() -> Option<OutputDeviceInfo> {
    None
}

pub(crate) fn capture_devices() -> Result<Vec<CaptureDevice>, RecorderError> {
    Ok(Vec::new())
}

pub(crate) fn os_version() -> String {
    std::env::consts::OS.to_string()
}

pub(crate) fn run_session(
    _config: &RecorderConfig,
    _pipeline: Arc<CapturePipeline>,
    ready: SyncSender<Result<CaptureSetup, RecorderError>>,
    _stop: Receiver<Instant>,
    _requested: Instant,
) -> Result<Vec<String>, RecorderError> {
    let _ = ready.send(Err(RecorderError::Unsupported(format!(
        "no system-audio capture backend for {}",
        std::env::consts::OS
    ))));
    Ok(Vec::new())
}

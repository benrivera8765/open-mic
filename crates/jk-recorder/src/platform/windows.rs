// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows backend: WASAPI loopback on the default render endpoint.
//!
//! NOT IMPLEMENTED YET. This stub keeps the public API identical on every
//! platform so the app can be written against it now. The Windows workstream
//! replaces the body of `run_session` (and `output_device`). The contract:
//!
//! 1. Open the default render endpoint in shared mode with
//!    `AUDCLNT_STREAMFLAGS_LOOPBACK`, using the endpoint's own mix format
//!    (`GetMixFormat`): the device's own rate, no resampling.
//! 2. Call `pipeline.set_format(...)` with that format (usually 32-bit float).
//! 3. For each packet from `IAudioCaptureClient::GetBuffer`: convert QPC
//!    position to `MediaTime { value: qpc_100ns as i64, timescale: 10_000_000 }`;
//!    call `pipeline.push_silence(...)` if `AUDCLNT_BUFFERFLAGS_SILENT` is set,
//!    else `pipeline.push(...)`. The pipeline fills the holes loopback leaves
//!    while the system is silent (WORK_ORDER section 7, trap 5).
//! 4. Send `Ok(CaptureSetup { backend: "WASAPI loopback", .. })` on `ready`
//!    once the client is started; on `stop`, stop the client and return.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};

use crate::error::RecorderError;
use crate::format::{CaptureSetup, OutputDeviceInfo};
use crate::pipeline::CapturePipeline;
use crate::{Backend, PermissionState, RecorderConfig};

pub(crate) const BACKEND: Backend = Backend::WasapiLoopback;

/// Loopback capture needs no permission on Windows.
pub(crate) fn permission_preflight() -> PermissionState {
    PermissionState::NotRequired
}

pub(crate) fn permission_request() -> PermissionState {
    PermissionState::NotRequired
}

pub(crate) fn output_device() -> Option<OutputDeviceInfo> {
    None
}

pub(crate) fn os_version() -> String {
    "Windows".to_string()
}

pub(crate) fn run_session(
    _config: &RecorderConfig,
    _pipeline: Arc<CapturePipeline>,
    ready: SyncSender<Result<CaptureSetup, RecorderError>>,
    _stop: Receiver<()>,
) -> Result<Vec<String>, RecorderError> {
    let _ = ready.send(Err(RecorderError::NotImplemented(
        "Windows system-audio capture (WASAPI loopback) is not written yet; \
         see crates/jk-recorder/src/platform/windows.rs for the contract",
    )));
    Ok(Vec::new())
}

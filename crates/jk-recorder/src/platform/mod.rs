// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform dispatch. Each backend module provides the same free functions:
//!
//! * `BACKEND`, `permission_preflight()`, `permission_request()`,
//!   `output_device()`, `capture_devices()`, `os_version()`;
//! * `run_session(config, pipeline, ready, stop, requested) -> Result<Vec<String>, _>`:
//!   runs on the recorder's private worker thread. It must send exactly one
//!   message on `ready` (the setup, or the reason it could not start), then,
//!   if it started, block until `stop` yields (the instant `stop()` was
//!   called, or a disconnect), stop the OS stream and return notes for the
//!   final report. `requested` is the instant `Recorder::start` was called:
//!   the backend declares it and the stop instant to the pipeline on the OS
//!   clock (`mark_start_since` / `mark_stop`) so the recording spans exactly
//!   Start..Stop. The OS objects never leave this thread, which is what makes `Recorder` `Send + Sync`.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(any(target_os = "macos", windows)))]
mod other;
#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) use other::*;

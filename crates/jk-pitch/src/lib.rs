// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice mode for Jukeoke (WORK_ORDER-2-practice-mode.md).
//!
//! Pure functions only: no threads, no I/O, no OS calls, so the same code runs on
//! Windows and macOS and can be tested offline with generated input.
//!
//! - [`tracker`]: streaming pitch tracker for the singer's microphone.
//! - [`guide`]: the melody guide builder (pitch curve of the isolated lead vocal -> notes).
//! - [`score`]: grading a performance against the guide.
//! - [`echo`]: detecting and removing the app's own playback when a mixer returns it in the input.

pub mod echo;
pub mod guide;
pub mod score;
pub mod tracker;

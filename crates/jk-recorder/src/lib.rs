// SPDX-License-Identifier: GPL-3.0-or-later
//! # jk-recorder: lossless capture of everything the computer is playing
//!
//! WORK_ORDER sections 4.6 and 4.10. One button records the system mix:
//!
//! * **macOS 13+:** ScreenCaptureKit system audio (`SCStreamConfiguration.capturesAudio`,
//!   macOS 13.0). The newer Core Audio process taps need macOS 14.2 and are not
//!   used, because the owner's MacBook runs 13.6.9. Every API this crate calls
//!   exists on macOS 13.0; see `docs/MAC.md` for the list with sources.
//! * **Windows:** WASAPI loopback of the default output device (or a chosen one,
//!   see [`capture_devices`]), at the device mix format, with a silent
//!   keep-alive render stream so packets keep coming while nothing plays
//!   (`src/platform/windows.rs`).
//!
//! What every backend guarantees:
//!
//! * capture at the output device's own rate when the OS allows it, and report
//!   the **real** format of what arrived ([`CaptureFormat`]), never the request;
//! * no processing: no resampling, no dither, no gain, no limiting. Samples are
//!   stored bit-for-bit as 32-bit float WAV, promoted to RF64 above 4 GiB;
//! * the file is written progressively and is valid on disk about once a
//!   second, so a pulled USB stick costs at most the last second;
//! * the recording's length equals the time from Start to Stop: holes in the OS
//!   timestamps, and silent stretches before the first or after the last
//!   buffer, are filled with exact digital silence (see [`pipeline`]);
//! * a mid-recording format change or a lost device stops the recording
//!   visibly ([`RecorderState::Failed`] with a plain message) and the file is
//!   closed, complete and valid, at that point;
//! * a lock-free level meter, elapsed time and a clip counter for the UI
//!   ([`Recorder::status`]).
//!
//! ```no_run
//! use jk_recorder::{Recorder, RecorderConfig};
//! let rec = Recorder::start(RecorderConfig::new("Recordings/take-1.wav"))?;
//! std::thread::sleep(std::time::Duration::from_secs(3));
//! let status = rec.status();            // levels, elapsed, clips, format
//! let done = rec.stop()?;               // final file + statistics
//! println!("{}", done.report());
//! # Ok::<(), jk_recorder::RecorderError>(())
//! ```

#![warn(missing_docs)]

pub mod error;
pub mod format;
pub mod media;
pub mod meter;
pub mod pipeline;
pub mod preview;
pub mod session;
pub mod wav;

mod platform;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use error::RecorderError;
pub use format::{CaptureDevice, CaptureFormat, CaptureSetup, OutputDeviceInfo, SourceEncoding};
pub use meter::{CLIP_LEVEL, Levels, to_dbfs};
pub use pipeline::{CapturePipeline, MediaTime, PipelineOptions, PipelineState, RecordingStats};
pub use wav::WavSummary;

/// Which system-audio capture technology this build uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// macOS 13+: ScreenCaptureKit.
    ScreenCaptureKit,
    /// Windows: WASAPI loopback on the default render device.
    WasapiLoopback,
    /// No system-audio capture on this platform.
    None,
}

impl Backend {
    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Backend::ScreenCaptureKit => "ScreenCaptureKit",
            Backend::WasapiLoopback => "WASAPI loopback",
            Backend::None => "none",
        }
    }
}

/// The capture backend compiled into this build.
pub fn backend() -> Backend {
    platform::BACKEND
}

/// Whether the OS will let this app capture system audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    /// Capture is allowed.
    Granted,
    /// Not granted yet (never asked, denied, or granted but the app has not
    /// been restarted since: macOS applies Screen Recording on relaunch).
    NotGranted,
    /// This platform needs no permission (Windows loopback).
    NotRequired,
}

/// Check the capture permission **without** showing anything to the user.
/// macOS: `CGPreflightScreenCaptureAccess()` (macOS 10.15+).
pub fn permission_preflight() -> PermissionState {
    platform::permission_preflight()
}

/// Ask for the capture permission. macOS: `CGRequestScreenCaptureAccess()`
/// (10.15+). macOS shows its prompt only the first time; after that this just
/// returns the current state, and the user has to use System Settings (open
/// [`MACOS_SCREEN_RECORDING_SETTINGS_URL`]). Call this only after the UI has
/// shown [`PERMISSION_EXPLANATION`], so "Screen Recording" does not surprise anyone.
pub fn permission_request() -> PermissionState {
    platform::permission_request()
}

/// Opens System Settings > Privacy & Security > Screen Recording on macOS 13
/// (pass it to `open` / `NSWorkspace`).
pub const MACOS_SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// Plain-language explanation for the UI to show **before** macOS asks
/// (WORK_ORDER section 4.10: "explain the permission wording in the app").
pub const PERMISSION_EXPLANATION: &str = "To record the sound your Mac is playing, macOS needs the \
\"Screen Recording\" permission. On macOS 13 that is the only way any app can capture system audio. \
Jukeoke records sound only: it asks macOS for the smallest possible picture (2 by 2 pixels, \
one frame per second) and throws every picture away unseen. macOS will say the app wants to \
\"record this computer's screen and audio\". Choose \"Open System Settings\", switch Jukeoke on \
under Privacy & Security > Screen Recording, then quit and reopen Jukeoke (macOS usually offers \
a button for that). The permission only takes effect after the app restarts.";

/// The default output device (name, nominal rate, physical bit depth), if the
/// OS reports it. macOS: Core Audio HAL. Windows: the default render endpoint
/// (mix rate, and the hardware format from `PKEY_AudioEngine_DeviceFormat`).
pub fn output_device() -> Option<OutputDeviceInfo> {
    platform::output_device()
}

/// Output devices whose sound can be recorded. Windows: every active render
/// endpoint (pass a `CaptureDevice::id` as [`RecorderConfig::device_id`]).
/// macOS: empty, because ScreenCaptureKit records the system mix rather than a
/// chosen device.
pub fn capture_devices() -> Result<Vec<CaptureDevice>, RecorderError> {
    platform::capture_devices()
}

/// The OS name and version, for reports (macOS: "macOS Version 13.6.9 (Build
/// ...)", Windows: "Windows 11 (build 26200.7171, 25H2)").
pub fn os_version() -> String {
    platform::os_version()
}

/// How to record.
#[derive(Clone, Debug)]
pub struct RecorderConfig {
    /// The file to create (it must not exist yet; recordings never overwrite).
    /// The app puts it in `<root>/Recordings/`.
    pub path: PathBuf,
    /// Channels to ask for (ScreenCaptureKit offers 1 or 2). Default 2.
    pub channels: u16,
    /// Force a sample rate instead of following the output device. `None`
    /// (default) asks for the device's nominal rate when the OS can deliver
    /// it, else the OS default (48 kHz on macOS).
    pub sample_rate: Option<u32>,
    /// Leave this app's own sound out of the recording. Default `false`
    /// (record everything, including a backing track Jukeoke is playing).
    pub exclude_own_audio: bool,
    /// If the permission is missing, ask the OS (may show the system prompt).
    /// Default `false`: fail with [`RecorderError::PermissionDenied`] instead,
    /// so the UI can explain first.
    pub request_permission: bool,
    /// How long to wait for the OS to start the stream. Default 20 s.
    pub start_timeout: Duration,
    /// Windows: the output device to record (a [`CaptureDevice::id`] from
    /// [`capture_devices`]). `None` (default) records the default output
    /// device. Ignored on macOS, which records the system mix.
    pub device_id: Option<String>,
    /// Windows: while recording, play digital silence (buffers flagged
    /// silent) on the recorded device, so loopback keeps delivering packets
    /// while nothing else plays (WORK_ORDER section 7, trap 5). It adds
    /// exactly 0.0 to the mix and changes nothing audible. Default `true`.
    /// With `false`, silent stretches are still filled from the timestamps.
    pub silent_keepalive: bool,
    /// Timeline and writer settings.
    pub pipeline: PipelineOptions,
}

impl RecorderConfig {
    /// Defaults for a recording at `path`.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            channels: 2,
            sample_rate: None,
            exclude_own_audio: false,
            request_permission: false,
            start_timeout: Duration::from_secs(20),
            device_id: None,
            silent_keepalive: true,
            pipeline: PipelineOptions::default(),
        }
    }
}

/// Coarse state for the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderState {
    /// The stream is running but no audio buffer has arrived yet.
    WaitingForAudio,
    /// Audio is being written.
    Recording,
    /// Something stopped the capture (the message says what, in plain words:
    /// device removed, format changed, disk full...). The file up to that
    /// point is valid (after a device loss or format change it is already
    /// closed); call `stop()` to collect the final statistics.
    Failed(String),
}

/// What the UI polls (about 30 times a second is fine).
#[derive(Clone, Debug, PartialEq)]
pub struct RecorderStatus {
    /// Coarse state.
    pub state: RecorderState,
    /// What the backend decided at start (device, requested rate, notes).
    pub setup: CaptureSetup,
    /// Counters: real format, elapsed time, frames, clips, gaps, drops.
    pub stats: RecordingStats,
    /// Meter values since the previous poll.
    pub levels: Levels,
}

/// A recording that has been stopped and closed.
#[derive(Clone, Debug, PartialEq)]
pub struct FinishedRecording {
    /// What the backend decided at start, plus notes from shutdown.
    pub setup: CaptureSetup,
    /// Final counters.
    pub stats: RecordingStats,
    /// Final meter values (whole-recording peaks in `peak_total`).
    pub levels: Levels,
    /// The file, or `None` if no audio ever arrived (then no file exists).
    pub file: Option<WavSummary>,
}

impl FinishedRecording {
    /// A plain-text report for logs and the self-check (one fact per line).
    pub fn report(&self) -> String {
        let s = &self.stats;
        let mut out = String::new();
        let mut line = |k: &str, v: String| {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(&v);
            out.push('\n');
        };
        line("backend", self.setup.backend.to_string());
        line("os", os_version());
        match &self.setup.device {
            Some(d) => line(
                "output device",
                format!(
                    "{} | nominal {} Hz | physical {} bit, {} ch",
                    d.name,
                    d.nominal_sample_rate,
                    d.physical_bits.map_or("?".to_string(), |b| b.to_string()),
                    d.physical_channels
                        .map_or("?".to_string(), |c| c.to_string())
                ),
            ),
            None => line("output device", "unknown".into()),
        }
        line(
            "requested",
            format!(
                "{} Hz, {} ch",
                self.setup
                    .requested_sample_rate
                    .map_or("device default".to_string(), |r| r.to_string()),
                self.setup.requested_channels
            ),
        );
        line(
            "capture format",
            s.format
                .as_ref()
                .map_or("no audio arrived".to_string(), |f| f.to_string()),
        );
        line("file", s.path.display().to_string());
        if let Some(f) = &self.file {
            line("file frames", f.frames.to_string());
            line("file bytes", f.file_bytes.to_string());
            line(
                "container",
                if f.rf64 {
                    "RF64".into()
                } else {
                    "RIFF/WAVE".into()
                },
            );
        }
        line(
            "wall-clock elapsed s",
            format!("{:.3}", s.wall_elapsed.as_secs_f64()),
        );
        line("recording length s", format!("{:.6}", s.media_elapsed_secs));
        line(
            "start to stop s (OS clock)",
            s.start_to_stop_secs
                .map_or("n/a".to_string(), |t| format!("{t:.6}")),
        );
        line(
            "length / start-to-stop",
            s.length_vs_start_stop_ratio()
                .map_or("n/a".to_string(), |r| format!("{r:.6}")),
        );
        line(
            "OS clock span s",
            s.os_clock_elapsed_secs
                .map_or("n/a".to_string(), |t| format!("{t:.6}")),
        );
        line(
            "length / OS clock",
            s.frames_per_os_second_ratio()
                .map_or("n/a".to_string(), |r| format!("{r:.6}")),
        );
        line("blocks", s.blocks.to_string());
        line("frames received from the OS", s.received_frames.to_string());
        line(
            "silence before the first block (frames)",
            s.head_fill_frames.to_string(),
        );
        line(
            "gaps filled",
            format!("{} ({} frames)", s.gap_events, s.gap_frames),
        );
        line(
            "silence after the last block (frames)",
            s.tail_fill_frames.to_string(),
        );
        line(
            "start/stop extensions refused (clock mismatch)",
            s.edge_fill_refused.to_string(),
        );
        line(
            "overlaps",
            format!("{} ({} frames)", s.overlap_events, s.overlap_frames),
        );
        line("timestamp resets", s.timestamp_resets.to_string());
        line(
            "max timestamp jitter frames",
            s.max_jitter_frames.to_string(),
        );
        line("dropped frames (audio lost)", s.dropped_frames.to_string());
        line(
            "format changes (each stops the recording)",
            s.format_mismatches.to_string(),
        );
        line(
            "peak dBFS per channel",
            self.levels
                .peak_total_dbfs()
                .iter()
                .map(|d| format!("{d:.2}"))
                .collect::<Vec<_>>()
                .join(" / "),
        );
        line(
            "clipped samples (>= full scale)",
            s.clipped_samples.to_string(),
        );
        line("non-finite samples", s.non_finite_samples.to_string());
        line("error", s.error.clone().unwrap_or_else(|| "none".into()));
        for n in &self.setup.notes {
            line("note", n.clone());
        }
        out
    }
}

/// A running recording. `Send + Sync`: the OS objects live on a private
/// worker thread, and everything else is shared through atomics.
#[derive(Debug)]
pub struct Recorder {
    pipeline: Arc<CapturePipeline>,
    setup: CaptureSetup,
    /// Carries the instant `stop()` was called.
    stop_tx: Option<mpsc::Sender<Instant>>,
    worker: Option<JoinHandle<Result<Vec<String>, RecorderError>>>,
}

impl Recorder {
    /// Start capturing into `config.path`. Returns once the OS has started the
    /// stream (or failed to). The file is created in the real format the OS
    /// delivers: on Windows at once (the endpoint's mix format is fixed), on
    /// macOS when the first audio buffer arrives.
    pub fn start(config: RecorderConfig) -> Result<Self, RecorderError> {
        // "Start" is this call: the recording covers the time from here, even
        // while the OS stream is still being set up (filled with silence).
        let requested = Instant::now();
        let pipeline = CapturePipeline::new(&config.path, config.pipeline.clone())?;
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<CaptureSetup, RecorderError>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<Instant>();
        let timeout = config.start_timeout;
        let worker = {
            let pipeline = Arc::clone(&pipeline);
            std::thread::Builder::new()
                .name("jk-recorder-session".into())
                .spawn(move || {
                    platform::run_session(&config, pipeline, ready_tx, stop_rx, requested)
                })?
        };
        match ready_rx.recv_timeout(timeout) {
            Ok(Ok(setup)) => Ok(Self {
                pipeline,
                setup,
                stop_tx: Some(stop_tx),
                worker: Some(worker),
            }),
            Ok(Err(e)) => {
                drop(stop_tx);
                let _ = worker.join();
                discard_empty(&pipeline);
                Err(e)
            }
            Err(_) => {
                // Dropping stop_tx tells the worker to shut down whatever it
                // managed to start.
                drop(stop_tx);
                discard_empty(&pipeline);
                Err(RecorderError::Timeout(
                    "waiting for the OS to start the capture stream",
                ))
            }
        }
    }

    /// Current state, counters and meter values. Resets the meter's "recent"
    /// peaks, so poll it from one place (the UI).
    pub fn status(&self) -> RecorderStatus {
        let state = match self.pipeline.state() {
            PipelineState::WaitingForAudio => RecorderState::WaitingForAudio,
            PipelineState::Recording | PipelineState::Finished => RecorderState::Recording,
            PipelineState::Failed(e) => RecorderState::Failed(e),
        };
        RecorderStatus {
            state,
            setup: self.setup.clone(),
            stats: self.pipeline.stats(),
            levels: self.pipeline.take_levels(),
        }
    }

    /// The real capture format, once the first buffer has arrived.
    pub fn format(&self) -> Option<CaptureFormat> {
        self.pipeline.format().cloned()
    }

    /// True once the OS has delivered at least one buffer (audio or flagged
    /// silence): from then on, sound that starts playing is in the recording.
    /// "Record & play" waits for this before it tells the player to play.
    pub fn has_audio(&self) -> bool {
        self.pipeline.stats().blocks > 0
    }

    /// The position in the recording (frame index) of what was playing at the
    /// wall-clock instant `at`; see [`CapturePipeline::timeline_frame_at`].
    pub fn timeline_frame_at(&self, at: Instant) -> Option<u64> {
        self.pipeline.timeline_frame_at(at)
    }

    /// The file being written.
    pub fn path(&self) -> &Path {
        self.pipeline.path()
    }

    /// Stop the OS stream, drain the queue, finalise the file.
    pub fn stop(mut self) -> Result<FinishedRecording, RecorderError> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<FinishedRecording, RecorderError> {
        let Some(worker) = self.worker.take() else {
            return Err(RecorderError::AlreadyStopped);
        };
        if let Some(tx) = self.stop_tx.take() {
            // "Stop" is this call: the backend ends the recording at this instant.
            let _ = tx.send(Instant::now());
        }
        let mut setup = self.setup.clone();
        match worker.join() {
            Ok(Ok(notes)) => setup.notes.extend(notes),
            Ok(Err(e)) => setup
                .notes
                .push(format!("stopping the OS stream reported: {e}")),
            Err(_) => setup
                .notes
                .push("the capture worker thread panicked".to_string()),
        }
        let summary = self.pipeline.finish()?;
        Ok(FinishedRecording {
            setup,
            stats: summary.stats,
            levels: self.pipeline.peek_levels(),
            file: summary.file,
        })
    }
}

/// After a failed start: close the pipeline and delete the file if it holds no
/// audio (a backend that knows its format up front creates the file before the
/// OS stream starts). The pipeline creates its file with `create_new`, so this
/// only ever removes a file this recorder made.
fn discard_empty(pipeline: &CapturePipeline) {
    if let Ok(summary) = pipeline.finish()
        && summary.file.is_some_and(|f| f.frames == 0)
    {
        let _ = std::fs::remove_file(pipeline.path());
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // A recorder dropped without stop() still closes a valid file.
        let _ = self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Recorder>();
        assert_send_sync::<CapturePipeline>();
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn unsupported_platforms_fail_cleanly_without_creating_a_file() {
        let mut p = std::env::temp_dir();
        p.push(format!("jk-recorder-lib-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let err = Recorder::start(RecorderConfig::new(&p)).unwrap_err();
        assert!(matches!(err, RecorderError::Unsupported(_)), "{err}");
        assert!(!p.exists());
        assert_eq!(permission_preflight(), PermissionState::NotRequired);
    }

    /// A real half-second loopback capture of the default output device (the
    /// keep-alive stream plays only digital silence; nothing is audible). On a
    /// machine without an output device (some CI runners) it checks the clean
    /// error instead.
    #[cfg(windows)]
    #[test]
    fn windows_loopback_records_start_to_stop() {
        let mut p = std::env::temp_dir();
        p.push(format!("jk-recorder-lib-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(permission_preflight(), PermissionState::NotRequired);
        let rec = match Recorder::start(RecorderConfig::new(&p)) {
            Ok(r) => r,
            Err(e) => {
                assert!(
                    output_device().is_none(),
                    "a device exists but start failed: {e}"
                );
                assert!(!p.exists());
                return;
            }
        };
        let st = rec.status();
        assert_eq!(st.state, RecorderState::Recording, "{st:?}");
        let fmt = rec.format().expect("Windows declares the format at start");
        assert_eq!(
            Some(f64::from(fmt.sample_rate)),
            output_device().map(|d| d.nominal_sample_rate)
        );
        std::thread::sleep(Duration::from_millis(500));
        let done = rec.stop().unwrap();
        let s = &done.stats;
        assert!(s.error.is_none(), "{:?}", s.error);
        let file = done.file.expect("file");
        assert_eq!(file.frames, s.frames);
        assert_eq!(s.dropped_frames, 0);
        // Length equals Start..Stop within one 10 ms engine period.
        let span = s.start_to_stop_secs.expect("start and stop marked");
        let diff = (s.media_elapsed_secs - span).abs();
        assert!(
            diff < 0.011,
            "length {} s vs start-stop {span} s
{}",
            s.media_elapsed_secs,
            done.report()
        );
        std::fs::remove_file(&p).unwrap();
    }
}

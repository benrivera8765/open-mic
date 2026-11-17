// SPDX-License-Identifier: GPL-3.0-or-later
//! The playback clock and real-time counters, shared lock-free between the device
//! callback (writer) and everyone else (readers).
//!
//! Song time at the DAC = `start_song_frame / song_rate`
//!                      + `(frames_delivered - latency_frames) / device_rate`,
//! where `frames_delivered` counts device frames handed to the device since the stream
//! started (device-stream frame 0 is song frame `start_song_frame`; the pipeline removes
//! the graph and resampler delays before the first frame) and `latency_frames` is the
//! device's reported output latency at the last callback (frames queued in the endpoint
//! buffer + the stream latency). The UI interpolates between clock reads with its own
//! timer but re-anchors on every read; it never runs a free timer (WORK_ORDER trap 7).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Instant;

/// Monotonic nanoseconds since the first use of the engine (for timestamps).
pub fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[derive(Default)]
pub struct Clock {
    pub device_rate: AtomicU32,
    pub song_rate: AtomicU32,
    pub start_song_frame: AtomicU64,
    /// Device frames handed to the device since the stream started.
    pub frames_delivered: AtomicU64,
    /// Reported output latency at the last callback, in device frames.
    pub latency_frames: AtomicU32,
    /// Callbacks that could not be filled completely from the output ring.
    pub underruns: AtomicU64,
    /// Frames of silence written because of underruns.
    pub underrun_frames: AtomicU64,
    /// WASAPI shared mode only: device events at which the endpoint buffer was found
    /// completely empty (padding 0) after the stream started. The device thread woke too
    /// late and the audio engine most likely played a gap. `underruns` cannot see this
    /// (it counts an empty output ring, not a late device thread). Always 0 where it
    /// cannot be measured (exclusive event mode, Core Audio).
    pub device_starved: AtomicU64,
    /// Device callbacks so far.
    pub callbacks: AtomicU64,
    /// `now_ns()` at the last callback.
    pub last_callback_ns: AtomicU64,
    /// Longest time spent inside one device callback (ns).
    pub max_callback_ns: AtomicU64,
    /// Device position from the OS clock (Windows: IAudioClock), first and latest sample,
    /// in device frames, with matching OS timestamps in 100 ns units. Used to measure the
    /// device clock against the OS performance counter (drift in ppm).
    pub os_pos_first: AtomicU64,
    pub os_qpc_first: AtomicU64,
    pub os_pos_last: AtomicU64,
    pub os_qpc_last: AtomicU64,
    /// Set by the worker once the last song frame is in the output ring: running dry
    /// after that is the end of the song, not an underrun.
    pub finished: AtomicBool,
    /// Set by the device thread when the stream stops (song end or error).
    pub stopped: AtomicBool,
}

impl Clock {
    /// Song position (seconds) of the sample leaving the DAC now.
    pub fn song_time_at_dac(&self) -> f64 {
        let dr = self.device_rate.load(Relaxed) as f64;
        let sr = self.song_rate.load(Relaxed) as f64;
        if dr == 0.0 || sr == 0.0 {
            return 0.0;
        }
        let delivered = self.frames_delivered.load(Relaxed) as f64;
        let lat = self.latency_frames.load(Relaxed) as f64;
        let played = (delivered - lat).max(0.0);
        self.start_song_frame.load(Relaxed) as f64 / sr + played / dr
    }

    /// [`song_time_at_dac`](Self::song_time_at_dac) advanced by the time since the last
    /// device callback (capped at 50 ms), i.e. what a UI does between clock events.
    pub fn song_time_interpolated(&self, now_ns: u64) -> f64 {
        let last = self.last_callback_ns.load(Relaxed);
        let dt = (now_ns.saturating_sub(last) as f64 * 1e-9).min(0.05);
        self.song_time_at_dac() + if last == 0 { 0.0 } else { dt }
    }

    /// Device clock vs OS performance counter, in ppm (None until two samples exist).
    pub fn device_drift_ppm(&self) -> Option<f64> {
        let dr = self.device_rate.load(Relaxed) as f64;
        let p0 = self.os_pos_first.load(Relaxed);
        let p1 = self.os_pos_last.load(Relaxed);
        let q0 = self.os_qpc_first.load(Relaxed);
        let q1 = self.os_qpc_last.load(Relaxed);
        if dr == 0.0 || q1 <= q0 + 10_000_000 {
            return None;
        }
        let dev_s = (p1 - p0) as f64 / dr;
        let os_s = (q1 - q0) as f64 * 1e-7;
        Some((dev_s / os_s - 1.0) * 1e6)
    }
}

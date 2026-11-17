// SPDX-License-Identifier: GPL-3.0-or-later
//! Device output. One thread (or OS callback) per stream that only copies from the output
//! ring, applies the master volume and advances the clock: no allocation, no locks, no
//! logging, no syscalls besides the audio API's own buffer calls.
//!
//! * Windows: WASAPI via the `wasapi` crate. Shared mode at the device mix rate (default),
//!   or exclusive mode (option) at the song rate when the device accepts it.
//! * macOS: Core Audio via `cpal`, at the device's nominal rate (type-checked on Windows
//!   only; built and run in CI / on the Mac).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::Arc;

use crate::clock::{now_ns, Clock};

#[cfg(windows)]
pub mod wasapi_out;
#[cfg(target_os = "macos")]
pub mod coreaudio_out;

/// What the caller asks for.
#[derive(Clone, Debug)]
pub struct OutputRequest {
    /// WASAPI exclusive mode (Windows only).
    pub exclusive: bool,
    /// Rate to try first in exclusive mode (normally the song rate, so no resampling).
    pub preferred_rate: Option<u32>,
    /// Device buffer length to ask for (shared mode), in milliseconds.
    pub buffer_ms: u32,
}

impl Default for OutputRequest {
    fn default() -> Self {
        OutputRequest { exclusive: false, preferred_rate: None, buffer_ms: 20 }
    }
}

/// What we got.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub backend: &'static str,
    pub share_mode: &'static str,
    pub sample_rate: u32,
    pub channels: usize,
    /// e.g. "f32", "i32 (24 valid bits)", "i16".
    pub sample_format: String,
    /// Endpoint buffer size in frames.
    pub buffer_frames: u32,
    /// The stream latency the OS reports (frames), excluding queued buffer contents.
    pub stream_latency_frames: u32,
}

/// Everything the device thread needs once the pipeline exists.
pub struct DeviceFeed {
    pub consumer: rtrb::Consumer<f32>,
    pub clock: Arc<Clock>,
    /// Master volume as `f32::to_bits`.
    pub master_gain: Arc<AtomicU32>,
    /// Start the stream once the ring is primed.
    pub go: Arc<AtomicBool>,
    /// Cleared to stop.
    pub running: Arc<AtomicBool>,
}

/// Copies from the ring into the device buffer, applies a smoothed master gain and keeps
/// the counters. Shared by all backends. Real-time safe.
pub(crate) struct Filler {
    consumer: rtrb::Consumer<f32>,
    clock: Arc<Clock>,
    master: Arc<AtomicU32>,
    gain: f32,
    channels: usize,
}

impl Filler {
    pub fn new(feed: &mut Option<DeviceFeed>, channels: usize) -> Option<Self> {
        let f = feed.take()?;
        let gain = f32::from_bits(f.master_gain.load(Relaxed));
        Some(Filler { consumer: f.consumer, clock: f.clock, master: f.master_gain, gain, channels })
    }

    /// Fill `out` (interleaved f32, whole frames). `latency_frames` is the latency to
    /// publish for this callback.
    #[inline]
    pub fn fill(&mut self, out: &mut [f32], latency_frames: u32) {
        let t0 = now_ns();
        let ch = self.channels;
        let frames = out.len() / ch;
        let want = frames * ch;
        let avail = self.consumer.slots().min(want) / ch * ch;
        if avail > 0 {
            if let Ok(chunk) = self.consumer.read_chunk(avail) {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
            }
        }
        if avail < want {
            out[avail..want].iter_mut().for_each(|v| *v = 0.0);
            if !self.clock.finished.load(Relaxed) {
                self.clock.underruns.fetch_add(1, Relaxed);
                self.clock.underrun_frames.fetch_add(((want - avail) / ch) as u64, Relaxed);
            }
        }
        // Master volume: linear ramp from the previous value to the target over this buffer.
        let target = f32::from_bits(self.master.load(Relaxed));
        if target == self.gain {
            if target != 1.0 {
                out.iter_mut().for_each(|v| *v *= target);
            }
        } else {
            let step = (target - self.gain) / frames.max(1) as f32;
            let mut g = self.gain;
            for f in out.chunks_exact_mut(ch) {
                g += step;
                f.iter_mut().for_each(|v| *v *= g);
            }
            self.gain = target;
        }
        let c = &self.clock;
        c.frames_delivered.fetch_add(frames as u64, Relaxed);
        c.latency_frames.store(latency_frames, Relaxed);
        c.callbacks.fetch_add(1, Relaxed);
        let t1 = now_ns();
        c.last_callback_ns.store(t1, Relaxed);
        c.max_callback_ns.fetch_max(t1 - t0, Relaxed);
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }
}

/// Raise the calling thread's priority for audio work (MMCSS "Pro Audio" on Windows,
/// QoS user-interactive on macOS). Returns a description of what happened.
pub fn promote_audio_thread() -> String {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let mut task = 0u32;
        // SAFETY: plain Win32 call with a static string and a valid out pointer.
        match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) } {
            Ok(_) => "MMCSS Pro Audio".to_string(),
            Err(e) => format!("MMCSS failed: {e}"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio_out::promote_current_thread()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "default priority".to_string()
    }
}

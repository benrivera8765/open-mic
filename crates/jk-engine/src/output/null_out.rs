// SPDX-License-Identifier: GPL-3.0-or-later
//! The null device: plays nothing, consumes audio in 10 ms buffers at the real-time
//! rate (paced by the system clock), so the whole pipeline runs exactly as with a sound
//! card. Used by tests, by a headless self-check, and on computers without any output.
//!
//! Test mode ("fast", only through `Engine::new_with_capture`): buffers are consumed as
//! fast as the worker produces them and every buffer is appended to a capture vector.
//! That path takes a mutex per buffer, which is fine for a device that is not a real
//! audio callback.

use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use super::{Capture, DeviceFeed, Filler, OpenOutput, StreamInfo};

/// `"null"` (48 kHz) or `"null:<rate>"`.
pub fn parse_null_id(id: &str) -> Option<u32> {
    if id == "null" {
        return Some(48_000);
    }
    let rate: u32 = id.strip_prefix("null:")?.parse().ok()?;
    (8_000..=384_000).contains(&rate).then_some(rate)
}

pub fn open(rate: u32, capture: Option<(Capture, bool)>) -> Result<OpenOutput, String> {
    let period = (rate / 100).max(1) as usize;
    let info = StreamInfo {
        id: format!("null:{rate}"),
        name: "No audio output (null device)".into(),
        backend: "null",
        share_mode: "shared",
        exclusive: false,
        sample_rate: rate,
        channels: 2,
        sample_format: "f32".into(),
        buffer_frames: period as u32,
        stream_latency_frames: 0,
    };
    let (feed_tx, feed_rx) = mpsc::channel::<DeviceFeed>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("jk-null-out".into())
        .spawn(move || run(rate, period, feed_rx, stop2, capture))
        .map_err(|e| e.to_string())?;
    Ok(OpenOutput { info, feed_tx, thread, stop })
}

fn run(
    rate: u32,
    period: usize,
    feed_rx: mpsc::Receiver<DeviceFeed>,
    stop: Arc<AtomicBool>,
    capture: Option<(Capture, bool)>,
) -> Result<(), String> {
    let feed = loop {
        if stop.load(Acquire) {
            return Ok(());
        }
        match feed_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(f) => break f,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Ok(()),
        }
    };
    let mut filler = Filler::new(feed, 2, rate);
    let mut buf = vec![0.0f32; period * 2];
    let fast = capture.as_ref().is_some_and(|c| c.1);
    let dt = Duration::from_secs_f64(period as f64 / rate as f64);
    let mut next = Instant::now();
    while !stop.load(Acquire) {
        if filler.shared().simulate_device_loss.load(Acquire) {
            filler.shared().device_failed.store(true, Release);
            return Err("device lost (simulated)".into());
        }
        if fast {
            if !filler.can_fill(period) {
                std::thread::sleep(Duration::from_micros(200));
                continue;
            }
        } else {
            next += dt;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > Duration::from_millis(200) {
                next = now; // the computer slept; do not catch up in a burst
            }
        }
        filler.fill(&mut buf, period as u32);
        if let Some((cap, _)) = &capture
            && let Ok(mut v) = cap.lock()
        {
            v.extend_from_slice(&buf);
        }
    }
    Ok(())
}

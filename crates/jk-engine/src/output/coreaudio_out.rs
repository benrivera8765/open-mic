// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS output via cpal (Core Audio AUHAL), at the device's nominal sample rate, f32.
//!
//! cpal is pinned to 0.16: from 0.17 on, cpal's Core Audio backend links the process-tap
//! loopback API (`AudioHardwareCreateProcessTap`, `CATapDescription`, macOS 14.2+) and
//! its README states macOS 14.2 as the minimum for that backend. The owner's Mac runs
//! macOS 13.6.9 (WORK_ORDER section 4.10). cpal 0.16 uses only long-standing Core Audio
//! properties (checked in its source). Do not upgrade without re-checking this.
//!
//! STATUS: type-checked on Windows with `cargo check --target aarch64-apple-darwin`; it has
//! not been compiled with the Apple SDK or run yet (that happens in the macOS CI job and on
//! the owner's MacBook).
//!
//! The stream lives on its own thread because cpal streams are not `Send` on every
//! backend. Core Audio's I/O thread calls the data callback, which only runs [`Filler`].
//! Device ids on macOS are the device names (cpal 0.16 has no stable id).
//!
//! Device loss: cpal reports `StreamError::DeviceNotAvailable` through the error
//! callback; it sets `device_failed`, the thread stops the stream and returns an error.

use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{DeviceFeed, Filler, OpenOutput, OutputRequest, StreamInfo};

/// The output device named `id`, or the default for `None`.
pub(crate) fn find_device(host: &cpal::Host, id: Option<&str>) -> Result<cpal::Device, String> {
    match id {
        None => host.default_output_device().ok_or_else(|| "no default output device".to_string()),
        Some(id) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == id).unwrap_or(false))
            .ok_or_else(|| format!("device {id} not found")),
    }
}

pub fn open(req: OutputRequest) -> Result<OpenOutput, String> {
    let (info_tx, info_rx) = mpsc::channel::<Result<StreamInfo, String>>();
    let (feed_tx, feed_rx) = mpsc::channel::<DeviceFeed>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("jk-coreaudio-out".into())
        .spawn(move || device_thread(req, info_tx, feed_rx, stop2))
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "device thread died".to_string())??;
    Ok(OpenOutput { info, feed_tx, thread, stop })
}

fn device_thread(
    req: OutputRequest,
    info_tx: mpsc::Sender<Result<StreamInfo, String>>,
    feed_rx: mpsc::Receiver<DeviceFeed>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let setup = (|| -> Result<_, String> {
        let device = find_device(&host, req.device_id.as_deref())?;
        let supported = device.default_output_config().map_err(|e| format!("default output config: {e}"))?;
        Ok((device, supported))
    })();
    let (device, supported) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = info_tx.send(Err(e.clone()));
            return Err(e);
        }
    };
    // The device's nominal rate: Core Audio would resample anything else.
    let rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config = cpal::StreamConfig {
        channels: supported.channels(),
        sample_rate: supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };
    let name = device.name().unwrap_or_else(|_| "default output".to_string());
    let info = StreamInfo {
        id: name.clone(),
        name,
        backend: "CoreAudio (cpal)",
        share_mode: "shared",
        exclusive: false,
        sample_rate: rate,
        channels,
        sample_format: "f32".into(),
        buffer_frames: 0,
        stream_latency_frames: 0,
    };
    let _ = info_tx.send(Ok(info));

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
    let shared = feed.shared.clone();
    let shared_err = feed.shared.clone();
    let mut filler = Filler::new(feed, channels, rate);
    let stream = device
        .build_output_stream::<f32, _, _>(
            &config,
            move |data: &mut [f32], cb: &cpal::OutputCallbackInfo| {
                // cpal: `playback` is when the FIRST frame of `data` reaches the DAC.
                // The filler subtracts the latency we pass from the write head (the end
                // of `data`), so it must also cover the frames of this buffer.
                let ts = cb.timestamp();
                let lat = ts.playback.duration_since(&ts.callback).unwrap_or_default();
                let lat_frames = (lat.as_secs_f64() * rate as f64).round() as u32;
                let buf_frames = (data.len() / channels.max(1)) as u32;
                filler.fill(data, lat_frames + buf_frames);
            },
            move |e| {
                // Not real-time: cpal calls this from its own thread.
                if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                    shared_err.device_failed.store(true, Release);
                }
                eprintln!("core audio stream error: {e}");
            },
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    let mut result = Ok(());
    while !stop.load(Acquire) {
        if shared.simulate_device_loss.load(Acquire) {
            shared.device_failed.store(true, Release);
        }
        if shared.device_failed.load(Acquire) {
            result = Err("device lost".to_string());
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(stream);
    result
}

/// QoS user-interactive for the render worker (Core Audio's own I/O thread is already
/// real-time).
pub fn promote_current_thread() -> String {
    // SAFETY: plain libc call on the current thread.
    let r = unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
    if r == 0 { "QoS user-interactive".into() } else { format!("QoS failed ({r})") }
}

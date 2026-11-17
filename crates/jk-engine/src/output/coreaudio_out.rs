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

use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{DeviceFeed, DeviceInfo, Filler, OutputRequest};

pub struct CoreAudioOutput {
    pub info: DeviceInfo,
    pub feed_tx: mpsc::Sender<DeviceFeed>,
    pub thread: JoinHandle<Result<(), String>>,
}

pub fn open(req: OutputRequest) -> Result<CoreAudioOutput, String> {
    let (info_tx, info_rx) = mpsc::channel::<Result<DeviceInfo, String>>();
    let (feed_tx, feed_rx) = mpsc::channel::<DeviceFeed>();
    let thread = std::thread::Builder::new()
        .name("jk-coreaudio-out".into())
        .spawn(move || device_thread(req, info_tx, feed_rx))
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "device thread died".to_string())??;
    Ok(CoreAudioOutput { info, feed_tx, thread })
}

fn device_thread(
    _req: OutputRequest,
    info_tx: mpsc::Sender<Result<DeviceInfo, String>>,
    feed_rx: mpsc::Receiver<DeviceFeed>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => {
            let e = "no default output device".to_string();
            let _ = info_tx.send(Err(e.clone()));
            return Err(e);
        }
    };
    let supported = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            let e = format!("default output config: {e}");
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
    let info = DeviceInfo {
        name,
        backend: "CoreAudio (cpal)",
        share_mode: "shared",
        sample_rate: rate,
        channels,
        sample_format: "f32".into(),
        buffer_frames: 0,
        stream_latency_frames: 0,
    };
    let _ = info_tx.send(Ok(info));

    let mut feed = Some(feed_rx.recv().map_err(|_| "no pipeline".to_string())?);
    let go = feed.as_ref().unwrap().go.clone();
    let running = feed.as_ref().unwrap().running.clone();
    let mut filler = Filler::new(&mut feed, channels).ok_or("no feed")?;
    let go_cb = go.clone();
    let stream = device
        .build_output_stream::<f32, _, _>(
            &config,
            move |data: &mut [f32], cb: &cpal::OutputCallbackInfo| {
                if !go_cb.load(Relaxed) {
                    data.iter_mut().for_each(|v| *v = 0.0);
                    return;
                }
                // cpal: `playback` is when the FIRST frame of `data` reaches the DAC.
                // `Filler::fill` adds all of `data` to `frames_delivered` and the clock
                // subtracts the latency we pass, so the latency must also cover the
                // frames of this buffer (as on WASAPI, where it is the queued padding
                // including the frames just written). Without them the macOS clock would
                // run one device buffer (about 10 ms) ahead of the audio.
                let ts = cb.timestamp();
                let lat = ts.playback.duration_since(&ts.callback).unwrap_or_default();
                let lat_frames = (lat.as_secs_f64() * rate as f64).round() as u32;
                let buf_frames = (data.len() / channels.max(1)) as u32;
                filler.fill(data, lat_frames + buf_frames);
            },
            |e| eprintln!("core audio stream error: {e}"),
            None,
        )
        .map_err(|e| format!("build_output_stream: {e}"))?;
    while !go.load(Relaxed) {
        if !running.load(Relaxed) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    stream.play().map_err(|e| format!("play: {e}"))?;
    while running.load(Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(stream);
    Ok(())
}

/// QoS user-interactive for the render worker (Core Audio's own I/O thread is already
/// real-time).
pub fn promote_current_thread() -> String {
    // SAFETY: plain libc call on the current thread.
    let r = unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
    if r == 0 { "QoS user-interactive".into() } else { format!("QoS failed ({r})") }
}

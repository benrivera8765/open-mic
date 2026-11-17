// SPDX-License-Identifier: GPL-3.0-or-later
//! WASAPI output (event driven): shared mode at the device mix format/rate, or exclusive
//! mode as an option.
//!
//! All COM work happens on the device thread. The thread reports the negotiated format
//! back, waits for the pipeline (`DeviceFeed`), pre-fills the endpoint buffer (silence
//! until the first segment is ready) and starts the stream. The stream runs until the
//! engine closes it; pause and seek are handled by the filler, not by stopping the stream.
//!
//! Device loss (unplugged, disabled, driver reset, format change by another exclusive
//! user): WASAPI calls start failing (AUDCLNT_E_DEVICE_INVALIDATED) or events stop. The
//! thread then sets `device_failed`, stops and returns the error; the engine pauses and
//! reports `DeviceLost`. Nothing panics or hangs: every wait has a timeout.

use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use wasapi::{initialize_mta, Device, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use super::{promote_audio_thread, DeviceFeed, Filler, OpenOutput, OutputRequest, StreamInfo};

#[derive(Clone, Copy, Debug)]
enum DevFmt {
    F32,
    I32,
    I24Packed,
    I16,
}

fn hns_to_frames(hns: i64, rate: u32) -> u32 {
    ((hns.max(0) as u128 * rate as u128 + 5_000_000) / 10_000_000) as u32
}

/// Open the requested render device (or the default). Returns once the format is known.
pub fn open(req: OutputRequest) -> Result<OpenOutput, String> {
    let (info_tx, info_rx) = mpsc::channel::<Result<StreamInfo, String>>();
    let (feed_tx, feed_rx) = mpsc::channel::<DeviceFeed>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("jk-wasapi-out".into())
        .spawn(move || device_thread(req, info_tx, feed_rx, stop2))
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "device thread died".to_string())??;
    Ok(OpenOutput { info, feed_tx, thread, stop })
}

/// The device with this endpoint id, or the default render device for `None`.
pub(crate) fn find_device(enumerator: &DeviceEnumerator, id: Option<&str>) -> Result<Device, String> {
    match id {
        Some(id) => enumerator.get_device(id).map_err(|e| format!("device {id} not found: {e}")),
        None => enumerator.get_default_device(Direction::Render).map_err(|e| format!("no default output device: {e}")),
    }
}

/// Exclusive-mode formats to try, best first, for each rate.
fn exclusive_candidates() -> [(usize, usize, SampleType, DevFmt); 4] {
    [
        (32, 32, SampleType::Float, DevFmt::F32),
        (32, 24, SampleType::Int, DevFmt::I32),
        (24, 24, SampleType::Int, DevFmt::I24Packed),
        (16, 16, SampleType::Int, DevFmt::I16),
    ]
}

/// Does the device accept any exclusive-mode format at its mix rate? (Device picker.)
pub(crate) fn exclusive_capable(client: &wasapi::AudioClient, mix: &WaveFormat) -> bool {
    let ch = mix.get_nchannels() as usize;
    let rate = mix.get_samplespersec() as usize;
    exclusive_candidates()
        .iter()
        .any(|&(bits, valid, st, _)| client.is_supported_exclusive_with_quirks(&WaveFormat::new(bits, valid, st, rate, ch, None)).is_ok())
}

fn device_thread(
    req: OutputRequest,
    info_tx: mpsc::Sender<Result<StreamInfo, String>>,
    feed_rx: mpsc::Receiver<DeviceFeed>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let setup = (|| -> Result<_, String> {
        // This thread is ours, so it can join the multithreaded apartment.
        initialize_mta().ok().map_err(|e| format!("COM init: {e}"))?;
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let device = find_device(&enumerator, req.device_id.as_deref())?;
        let id = device.get_id().unwrap_or_default();
        let name = device.get_friendlyname().unwrap_or_else(|_| "default device".into());
        let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
        let mix = client.get_mixformat().map_err(|e| e.to_string())?;
        let (def_period, _min_period) = client.get_device_period().map_err(|e| e.to_string())?;

        let (format, fmt, mode, share) = if req.exclusive {
            // Exclusive: try the preferred (song) rate first, then the mix rate; float,
            // then 32-bit int (24 valid), packed 24, 16.
            let mut rates = vec![];
            if let Some(r) = req.preferred_rate {
                rates.push(r);
            }
            rates.push(mix.get_samplespersec());
            let ch = mix.get_nchannels() as usize;
            let mut found = None;
            'outer: for &rate in &rates {
                for (bits, valid, st, f) in exclusive_candidates() {
                    let want = WaveFormat::new(bits, valid, st, rate as usize, ch, None);
                    if let Ok(ok) = client.is_supported_exclusive_with_quirks(&want) {
                        found = Some((ok, f));
                        break 'outer;
                    }
                }
            }
            let (format, f) = found.ok_or("no exclusive-mode format accepted by the device")?;
            let period = client.calculate_aligned_period_near(def_period, Some(128), &format).unwrap_or(def_period);
            (format, f, StreamMode::EventsExclusive { period_hns: period }, "exclusive")
        } else {
            let st = mix.get_subformat().map_err(|e| e.to_string())?;
            let f = match (st, mix.get_bitspersample()) {
                (SampleType::Float, 32) => DevFmt::F32,
                (SampleType::Int, 32) => DevFmt::I32,
                (SampleType::Int, 24) => DevFmt::I24Packed,
                (SampleType::Int, 16) => DevFmt::I16,
                _ => return Err("unsupported shared-mode mix format".into()),
            };
            let buf_hns = (req.buffer_ms as i64 * 10_000).max(def_period);
            (mix.clone(), f, StreamMode::EventsShared { autoconvert: false, buffer_duration_hns: buf_hns }, "shared")
        };
        client
            .initialize_client(&format, Direction::Render, mode, None)
            .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
        let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
        let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
        let buffer_frames = client.get_buffer_size().map_err(|e| e.to_string())?;
        let rate = format.get_samplespersec();
        let stream_latency = client.get_stream_latency().map(|h| hns_to_frames(h, rate)).unwrap_or(0);
        let audio_clock = client.get_audioclock().ok();
        let info = StreamInfo {
            id,
            name,
            backend: "WASAPI",
            share_mode: share,
            exclusive: share == "exclusive",
            sample_rate: rate,
            channels: format.get_nchannels() as usize,
            sample_format: match fmt {
                DevFmt::F32 => "f32".into(),
                DevFmt::I32 => format!("i32 ({} valid bits)", format.get_validbitspersample()),
                DevFmt::I24Packed => "i24 packed".into(),
                DevFmt::I16 => "i16".into(),
            },
            buffer_frames,
            stream_latency_frames: stream_latency,
        };
        Ok((client, event, render, info, fmt, audio_clock))
    })();
    let (client, event, render, info, fmt, audio_clock) = match setup {
        Ok(v) => {
            let _ = info_tx.send(Ok(v.3.clone()));
            v
        }
        Err(e) => {
            let _ = info_tx.send(Err(e.clone()));
            return Err(e);
        }
    };

    // Wait for the pipeline.
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
    let channels = info.channels;
    let mut filler = Filler::new(feed, channels, info.sample_rate);
    let _prio = promote_audio_thread();
    let freq = audio_clock.as_ref().and_then(|c| c.get_frequency().ok()).unwrap_or(0);

    // Pre-allocated buffers (the loop below never allocates).
    let max_frames = info.buffer_frames as usize;
    let mut fbuf = vec![0.0f32; max_frames * channels];
    let bytes_per_sample = match fmt {
        DevFmt::F32 | DevFmt::I32 => 4,
        DevFmt::I24Packed => 3,
        DevFmt::I16 => 2,
    };
    let mut bbuf = vec![0u8; max_frames * channels * bytes_per_sample];

    let mut write = |filler: &mut Filler, frames: usize, latency: u32| -> Result<(), String> {
        let n = frames * channels;
        filler.fill(&mut fbuf[..n], latency);
        let bytes = n * bytes_per_sample;
        convert(fmt, &fbuf[..n], &mut bbuf[..bytes]);
        render.write_to_device(&bbuf[..bytes], None).map_err(|e| e.to_string())
    };

    let fail = |filler: &Filler, e: String| -> Result<(), String> {
        filler.shared().device_failed.store(true, Release);
        Err(e)
    };

    // Pre-fill the whole endpoint buffer, then start.
    let avail = match client.get_available_space_in_frames() {
        Ok(a) => a as usize,
        Err(e) => return fail(&filler, format!("device lost: {e}")),
    };
    if let Err(e) = write(&mut filler, avail, avail as u32 + info.stream_latency_frames) {
        return fail(&filler, e);
    }
    if let Err(e) = client.start_stream() {
        return fail(&filler, format!("start: {e}"));
    }
    let shared_mode = !info.exclusive;
    let mut have_first = false;
    let mut result = Ok(());
    while !stop.load(Acquire) {
        if filler.shared().simulate_device_loss.load(Acquire) {
            result = fail(&filler, "device lost (simulated)".into());
            break;
        }
        if event.wait_for_event(500).is_err() {
            result = fail(&filler, "the device stopped delivering events (lost?)".into());
            break;
        }
        let avail = match client.get_available_space_in_frames() {
            Ok(a) => a as usize,
            Err(e) => {
                result = fail(&filler, format!("device lost: {e}"));
                break;
            }
        };
        if avail == 0 {
            continue;
        }
        // Shared mode: free space == whole buffer means padding 0, i.e. the endpoint buffer
        // ran dry before this thread got to write (a device-side gap the ring-based
        // underrun counter cannot see). In exclusive event mode `avail` is always the
        // whole buffer, so this check is shared-mode only.
        if shared_mode && avail as u32 >= info.buffer_frames {
            filler.shared().device_starved.fetch_add(1, Relaxed);
        }
        // Frames queued in the endpoint buffer once this write is done, + stream latency.
        let padding_after = info.buffer_frames.saturating_sub(avail as u32) + avail as u32;
        if let Err(e) = write(&mut filler, avail, padding_after + info.stream_latency_frames) {
            result = fail(&filler, format!("device lost: {e}"));
            break;
        }
        if let (Some(ac), true) = (audio_clock.as_ref(), freq > 0)
            && let Ok((pos, qpc)) = ac.get_position()
        {
            // pos is in units of `freq` per second; convert to device frames.
            let frames = (pos as u128 * info.sample_rate as u128 / freq as u128) as u64;
            let s = filler.shared();
            if !have_first && frames > 0 {
                s.os_pos_first.store(frames, Relaxed);
                s.os_qpc_first.store(qpc, Relaxed);
                have_first = true;
            }
            s.os_pos_last.store(frames, Relaxed);
            s.os_qpc_last.store(qpc, Relaxed);
        }
    }
    let _ = client.stop_stream();
    result
}

/// f32 -> device sample format (little endian). Integer formats are clipped, not wrapped.
fn convert(fmt: DevFmt, src: &[f32], dst: &mut [u8]) {
    match fmt {
        DevFmt::F32 => {
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                d.copy_from_slice(&s.to_le_bytes());
            }
        }
        DevFmt::I32 => {
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 2147483647.0).round() as i32;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
        DevFmt::I24Packed => {
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<3>().0) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 8388607.0).round() as i32;
                d.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        DevFmt::I16 => {
            for (s, d) in src.iter().zip(dst.as_chunks_mut::<2>().0) {
                let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

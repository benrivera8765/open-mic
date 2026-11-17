// SPDX-License-Identifier: GPL-3.0-or-later
//! WASAPI output (event driven), shared mode at the device mix format/rate, or exclusive
//! mode as an option.
//!
//! All COM work happens on the device thread. The thread reports the negotiated format
//! back, waits for the pipeline (`DeviceFeed`), waits for `go`, pre-fills the endpoint
//! buffer and starts the stream.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::thread::JoinHandle;

use wasapi::{
    initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
};

use super::{promote_audio_thread, DeviceFeed, DeviceInfo, Filler, OutputRequest};

#[derive(Clone, Copy, Debug)]
enum DevFmt {
    F32,
    I32,
    I24Packed,
    I16,
}

pub struct WasapiOutput {
    pub info: DeviceInfo,
    pub feed_tx: mpsc::Sender<DeviceFeed>,
    pub thread: JoinHandle<Result<(), String>>,
}

fn hns_to_frames(hns: i64, rate: u32) -> u32 {
    ((hns.max(0) as u128 * rate as u128 + 5_000_000) / 10_000_000) as u32
}

/// Open the default render device. Returns once the format is known.
pub fn open(req: OutputRequest) -> Result<WasapiOutput, String> {
    let (info_tx, info_rx) = mpsc::channel::<Result<DeviceInfo, String>>();
    let (feed_tx, feed_rx) = mpsc::channel::<DeviceFeed>();
    let thread = std::thread::Builder::new()
        .name("jk-wasapi-out".into())
        .spawn(move || device_thread(req, info_tx, feed_rx))
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "device thread died".to_string())??;
    Ok(WasapiOutput { info, feed_tx, thread })
}

fn device_thread(
    req: OutputRequest,
    info_tx: mpsc::Sender<Result<DeviceInfo, String>>,
    feed_rx: mpsc::Receiver<DeviceFeed>,
) -> Result<(), String> {
    let setup = (|| -> Result<_, String> {
        initialize_mta().ok().map_err(|e| format!("COM init: {e}"))?;
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let device = enumerator.get_default_device(Direction::Render).map_err(|e| e.to_string())?;
        let name = device.get_friendlyname().unwrap_or_else(|_| "default device".into());
        let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
        let mix = client.get_mixformat().map_err(|e| e.to_string())?;
        let (def_period, min_period) = client.get_device_period().map_err(|e| e.to_string())?;

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
                for (bits, valid, st, f) in [
                    (32, 32, SampleType::Float, DevFmt::F32),
                    (32, 24, SampleType::Int, DevFmt::I32),
                    (24, 24, SampleType::Int, DevFmt::I24Packed),
                    (16, 16, SampleType::Int, DevFmt::I16),
                ] {
                    let want = WaveFormat::new(bits, valid, st, rate as usize, ch, None);
                    if let Ok(ok) = client.is_supported_exclusive_with_quirks(&want) {
                        found = Some((ok, f));
                        break 'outer;
                    }
                }
            }
            let (format, f) = found.ok_or("no exclusive-mode format accepted by the device")?;
            let period = client
                .calculate_aligned_period_near(def_period, Some(128), &format)
                .unwrap_or(def_period);
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
            let _ = min_period;
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
        let info = DeviceInfo {
            name,
            backend: "WASAPI",
            share_mode: share,
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

    let mut feed = Some(feed_rx.recv().map_err(|_| "no pipeline".to_string())?);
    let go = feed.as_ref().unwrap().go.clone();
    let running = feed.as_ref().unwrap().running.clone();
    let channels = info.channels;
    let mut filler = Filler::new(&mut feed, channels).ok_or("no feed")?;
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

    while !go.load(Relaxed) {
        if !running.load(Relaxed) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let mut write = |filler: &mut Filler, frames: usize, padding_after: u32| -> Result<(), String> {
        let n = frames * channels;
        filler.fill(&mut fbuf[..n], padding_after + info.stream_latency_frames);
        let bytes = n * bytes_per_sample;
        convert(fmt, &fbuf[..n], &mut bbuf[..bytes]);
        render.write_to_device(&bbuf[..bytes], None).map_err(|e| e.to_string())
    };

    // Pre-fill the whole endpoint buffer, then start.
    let avail = client.get_available_space_in_frames().map_err(|e| e.to_string())? as usize;
    write(&mut filler, avail, avail as u32)?;
    client.start_stream().map_err(|e| e.to_string())?;
    let shared = info.share_mode == "shared";
    let mut have_first = false;
    let mut result = Ok(());
    while running.load(Relaxed) {
        if event.wait_for_event(1000).is_err() {
            result = Err("device stopped delivering events".to_string());
            break;
        }
        let avail = match client.get_available_space_in_frames() {
            Ok(a) => a as usize,
            Err(e) => {
                result = Err(format!("device lost: {e}"));
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
        if shared && avail as u32 >= info.buffer_frames {
            filler.clock().device_starved.fetch_add(1, Relaxed);
        }
        // Frames queued in the endpoint buffer once this write is done.
        let padding_after = info.buffer_frames.saturating_sub(avail as u32) + avail as u32;
        if let Err(e) = write(&mut filler, avail, padding_after) {
            result = Err(e);
            break;
        }
        if let (Some(ac), true) = (audio_clock.as_ref(), freq > 0) {
            if let Ok((pos, qpc)) = ac.get_position() {
                // pos is in units of `freq` per second; convert to device frames.
                let frames = (pos as u128 * info.sample_rate as u128 / freq as u128) as u64;
                let c = filler.clock();
                if !have_first && frames > 0 {
                    c.os_pos_first.store(frames, Relaxed);
                    c.os_qpc_first.store(qpc, Relaxed);
                    have_first = true;
                }
                c.os_pos_last.store(frames, Relaxed);
                c.os_qpc_last.store(qpc, Relaxed);
            }
        }
    }
    let _ = client.stop_stream();
    filler.clock().stopped.store(true, Relaxed);
    result
}

/// f32 -> device sample format (little endian). Integer formats are clipped, not wrapped.
fn convert(fmt: DevFmt, src: &[f32], dst: &mut [u8]) {
    match fmt {
        DevFmt::F32 => {
            for (s, d) in src.iter().zip(dst.chunks_exact_mut(4)) {
                d.copy_from_slice(&s.to_le_bytes());
            }
        }
        DevFmt::I32 => {
            for (s, d) in src.iter().zip(dst.chunks_exact_mut(4)) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 2147483647.0).round() as i32;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
        DevFmt::I24Packed => {
            for (s, d) in src.iter().zip(dst.chunks_exact_mut(3)) {
                let v = (s.clamp(-1.0, 1.0) as f64 * 8388607.0).round() as i32;
                d.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        DevFmt::I16 => {
            for (s, d) in src.iter().zip(dst.chunks_exact_mut(2)) {
                let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                d.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

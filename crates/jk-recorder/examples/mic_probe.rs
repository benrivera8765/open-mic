// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice-mode microphone probe (WO2 sections 4.4, 4.8, 9).
//!
//! ```text
//! mic_probe --list                      input devices (names, bus, format, flags) and the
//!                                       automatic choice; opens nothing
//! mic_probe --capture 3 [--device ID]   open a device for N seconds IN MEMORY and report its
//!                                       format, time stamps, per-channel level and noise floor
//!           [--loopback]                (Windows test path: the default output's loopback
//!                                       through the same capture code, with a silent keep-alive)
//! mic_probe --signals                   generated "voice on one channel of two" at 16/44.1/48/96
//!                                       kHz through the same input path: meters, channel choice,
//!                                       bit-exact channel extraction, pitch preserved
//! mic_probe --wav FILE                  a 32-bit float WAV fed through the same input path as a
//!                                       microphone (format, per-channel levels, suggested channel)
//! --json FILE                           also write the results as JSON
//! ```
//!
//! Microphone sound is analysed in memory and dropped: this tool never writes
//! captured audio anywhere and plays nothing. Without `--device`, `--capture`
//! uses the automatic choice, which never opens a Bluetooth device.

use std::time::{Duration, Instant};

use jk_recorder::input::signal::{OtherChannel, VoiceTone, check_f0_hz, place_voice};
use jk_recorder::input::{
    self, BlockStamp, InputState, InputStream, Pacing, SignalSource, clock, extract_channel,
    list_input_devices, preferred_input, suggest_voice_channel,
};
use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let mut report = serde_json::Map::new();
    report.insert("os".into(), json!(jk_recorder::os_version()));
    report.insert(
        "mic_permission".into(),
        json!(format!("{:?}", input::mic_permission())),
    );
    println!("OS: {}", jk_recorder::os_version());
    println!("microphone permission: {:?}", input::mic_permission());

    let mut ok = true;
    if flag("--list") || args.is_empty() {
        report.insert("devices".into(), list());
    }
    if let Some(secs) = value("--capture") {
        let secs: f64 = secs.parse().unwrap_or(3.0);
        let id = if flag("--loopback") {
            input::loopback_test_device_id()
        } else {
            value("--device")
        };
        let r = capture(id, secs);
        ok &= r.get("error").is_none();
        report.insert("capture".into(), r);
    }
    if flag("--signals") {
        let (r, pass) = signals();
        ok &= pass;
        report.insert("signals".into(), r);
    }
    if let Some(path) = value("--wav") {
        let r = wav(&path);
        ok &= r.get("error").is_none();
        report.insert("wav".into(), r);
    }
    if let Some(path) = value("--json") {
        let text = serde_json::to_string_pretty(&Value::Object(report)).unwrap();
        std::fs::write(&path, text).expect("writing the JSON report");
        println!("\nJSON report: {path}");
    }
    if !ok {
        std::process::exit(1);
    }
}

fn list() -> Value {
    println!("\n== Input devices (listing opens no stream)");
    let devices = match list_input_devices() {
        Ok(d) => d,
        Err(e) => {
            println!("  listing failed: {e}");
            return json!({ "error": e.to_string() });
        }
    };
    if devices.is_empty() {
        println!("  none: no microphone or audio input is active on this computer");
    }
    for d in &devices {
        println!(
            "  - \"{}\"{}\n      bus: {} ({}), form factor: {}, adapter: {}\n      format: {} ch, {} Hz, {}; auto-eligible: {}{}",
            d.name,
            if d.is_default {
                " [system default]"
            } else {
                ""
            },
            d.transport.label(),
            d.bus.as_deref().unwrap_or("?"),
            d.form_factor.as_deref().unwrap_or("?"),
            d.adapter.as_deref().unwrap_or("?"),
            d.channels,
            d.sample_rate,
            d.sample_format.as_deref().unwrap_or("?"),
            d.auto_eligible(),
            if d.hands_free {
                "  ** Bluetooth hands-free: never opened automatically (trap 2) **"
            } else {
                ""
            }
        );
    }
    let choice = preferred_input(&devices);
    match choice {
        Some(d) => println!("  automatic choice: \"{}\"", d.name),
        None => println!("  automatic choice: none (no device may be opened automatically)"),
    }
    let inactive = input::list_inactive_inputs();
    if !inactive.is_empty() {
        println!("  known to the OS but not usable now:");
        for (name, state) in &inactive {
            println!("    - \"{name}\": {state}");
        }
    }
    json!({
        "devices": devices,
        "automatic_choice": choice.map(|d| d.name.clone()),
        "inactive": inactive,
    })
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Read a stream for `secs` seconds; returns the stamps and arrival times.
fn drain(stream: &mut InputStream, secs: f64) -> (Vec<BlockStamp>, Vec<i64>, Vec<f32>) {
    let t = Instant::now();
    let mut buf = Vec::new();
    let mut stamps = Vec::new();
    let mut arrivals = Vec::new();
    let mut last_block = Vec::new();
    while t.elapsed() < Duration::from_secs_f64(secs) {
        let mut got = false;
        while let Some(s) = stream.read(&mut buf) {
            arrivals.push(clock::host_time_ns());
            stamps.push(s);
            got = true;
            last_block.clone_from(&buf);
        }
        if !got {
            if matches!(
                stream.state(),
                InputState::Failed { .. } | InputState::Ended
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    (stamps, arrivals, last_block)
}

fn capture(id: Option<String>, secs: f64) -> Value {
    println!("\n== Capture for {secs} s in memory (nothing is saved)");
    let opened = match &id {
        Some(id) => InputStream::open(Box::new(input::DeviceSource::new(id.clone()))),
        None => InputStream::open_device(None),
    };
    let mut stream = match opened {
        Ok(s) => s,
        Err(e) => {
            println!("  not opened: {e}");
            return json!({ "error": e.to_string() });
        }
    };
    let f = stream.format().clone();
    println!(
        "  device: \"{}\"\n  source: {}\n  format: {} ch, {} Hz, {} (handed on as f32), max block {} frames, declared latency {}",
        f.device_name,
        f.source,
        f.channels,
        f.sample_rate,
        f.sample_format,
        f.max_block_frames,
        f.declared_latency_ns
            .map_or("not declared".into(), |n| format!(
                "{:.1} ms",
                n as f64 / 1e6
            ))
    );
    for n in &f.notes {
        println!("  note: {n}");
    }
    let (stamps, arrivals, _) = drain(&mut stream, secs);
    let state = stream.state();
    let levels = stream.levels();
    let health = stream.health();
    stream.stop();
    let rate = f64::from(f.sample_rate);
    // Time stamps: cadence against the frame count, and how long after its
    // last frame each block reached the reader.
    let mut step_err_ms = Vec::new();
    let mut lag_ms = Vec::new();
    for w in stamps.windows(2) {
        let expect = f64::from(w[0].frames) / rate * 1e3;
        step_err_ms.push((w[1].host_ns - w[0].host_ns) as f64 / 1e6 - expect);
    }
    for (s, a) in stamps.iter().zip(&arrivals) {
        let end = s.host_ns as f64 + f64::from(s.frames) / rate * 1e9;
        lag_ms.push((*a as f64 - end) / 1e6);
    }
    let frames: u64 = stamps.iter().map(|s| u64::from(s.frames)).sum();
    let drift_ppm = match (stamps.first(), stamps.last()) {
        (Some(a), Some(b)) if b.host_ns > a.host_ns => {
            let by_clock = (b.host_ns - a.host_ns) as f64 / 1e9;
            let by_frames = (frames - u64::from(b.frames)) as f64 / rate;
            (by_frames / by_clock - 1.0) * 1e6
        }
        _ => f64::NAN,
    };
    let mut sizes: Vec<f64> = stamps.iter().map(|s| f64::from(s.frames)).collect();
    let block_median = percentile(&mut sizes, 0.5);
    let max_step_err = step_err_ms.iter().fold(0.0f64, |m, &x| m.max(x.abs()));
    let lag_median = percentile(&mut lag_ms.clone(), 0.5);
    let lag_max = percentile(&mut lag_ms, 1.0);
    println!(
        "  blocks: {} ({} frames, median block {} frames); dropped {} blocks; discontinuities {}; silent-flagged {}",
        stamps.len(),
        frames,
        block_median,
        health.dropped_blocks,
        stamps.iter().filter(|s| s.discontinuity()).count(),
        stamps
            .iter()
            .filter(|s| s.flags & input::ring::FLAG_SILENT != 0)
            .count(),
    );
    println!(
        "  time stamps: largest step error {max_step_err:.3} ms; device clock vs host clock {drift_ppm:+.0} ppm; block reached the reader {lag_median:.2} ms (median) / {lag_max:.2} ms (max) after its last frame's time stamp"
    );
    for (ch, l) in levels.channels.iter().enumerate() {
        println!(
            "  channel {}: peak {:.1} dBFS, RMS (300 ms) {:.1} dBFS, peak 3 s {:.1} dBFS, noise floor {}, clipped {}, advice {:?}",
            ch + 1,
            l.peak_dbfs,
            l.rms_dbfs,
            l.peak_3s_dbfs,
            l.noise_floor_dbfs
                .map_or("n/a".into(), |v| format!("{v:.1} dBFS")),
            l.clipped_samples,
            l.advice
        );
    }
    println!("  state at the end: {state:?}");
    json!({
        "format": f,
        "blocks": stamps.len(),
        "frames": frames,
        "median_block_frames": block_median,
        "health": health,
        "max_stamp_step_error_ms": max_step_err,
        "drift_ppm": drift_ppm,
        "reader_lag_ms_median": lag_median,
        "reader_lag_ms_max": lag_max,
        "levels": levels,
        "state": state,
    })
}

/// A WAV file through the input path, as fast as it is read.
fn wav(path: &str) -> Value {
    println!("\n== WAV file through the input path");
    let src = match SignalSource::from_wav(std::path::Path::new(path)) {
        Ok(s) => s.pacing(Pacing::AsFastAsPossible),
        Err(e) => {
            println!("  not opened: {e}");
            return json!({ "error": e.to_string() });
        }
    };
    let mut s = match InputStream::open(Box::new(src)) {
        Ok(s) => s,
        Err(e) => {
            println!("  not opened: {e}");
            return json!({ "error": e.to_string() });
        }
    };
    let f = s.format().clone();
    let (stamps, _, _) = drain(&mut s, 600.0);
    let levels = s.levels();
    let frames: u64 = stamps.iter().map(|st| u64::from(st.frames)).sum();
    let suggestion = suggest_voice_channel(&levels);
    println!(
        "  {} ch, {} Hz; {} frames in {} blocks; state {:?}; suggested voice channel {}",
        f.channels,
        f.sample_rate,
        frames,
        stamps.len(),
        s.state(),
        suggestion.map_or("none".into(), |c| (c + 1).to_string())
    );
    for (ch, l) in levels.channels.iter().enumerate() {
        println!(
            "  channel {}: peak 3 s {:.1} dBFS, RMS (300 ms) {:.1} dBFS, noise floor {}, active {}, advice {:?}",
            ch + 1,
            l.peak_3s_dbfs,
            l.rms_dbfs,
            l.noise_floor_dbfs
                .map_or("n/a".into(), |v| format!("{v:.1} dBFS")),
            l.active_rms_dbfs
                .map_or("n/a".into(), |v| format!("{v:.1} dBFS")),
            l.advice
        );
    }
    json!({
        "format": f,
        "frames": frames,
        "blocks": stamps.len(),
        "levels": levels,
        "suggested_channel": suggestion.map(|c| c + 1),
        "state": s.state(),
    })
}

/// Generated voice on one channel of two, through the real input path.
fn signals() -> (Value, bool) {
    println!("\n== Generated signals through the input path (voice on one channel of two)");
    let mut rows = Vec::new();
    let mut all_pass = true;
    let tone = VoiceTone {
        vibrato_cents: 0.0,
        ..VoiceTone::default_a3()
    };
    let others = [
        ("silence", OtherChannel::Silence),
        (
            "noise -70 dBFS RMS",
            OtherChannel::Noise { rms_dbfs: -70.0 },
        ),
        (
            "tone 1 kHz -30 dBFS peak",
            OtherChannel::Tone {
                hz: 1000.0,
                peak_dbfs: -30.0,
            },
        ),
    ];
    for rate in [16_000u32, 44_100, 48_000, 96_000] {
        let voice = tone.render(rate, 3.0);
        let voice_rms =
            (voice.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / voice.len() as f64).sqrt();
        for voice_ch in [0u16, 1] {
            for (other_name, other) in others {
                let x = place_voice(&voice, rate, 2, voice_ch, other);
                let src =
                    SignalSource::new("generated", rate, 2, x).pacing(Pacing::AsFastAsPossible);
                let mut s = InputStream::open(Box::new(src)).expect("open");
                let mut buf = Vec::new();
                let mut all = Vec::new();
                let t = Instant::now();
                while t.elapsed() < Duration::from_secs(20) {
                    match s.read(&mut buf) {
                        Some(_) => all.extend_from_slice(&buf),
                        None if s.state() == InputState::Ended => break,
                        None => std::thread::sleep(Duration::from_millis(1)),
                    }
                }
                let levels = s.levels();
                let vc = usize::from(voice_ch);
                let oc = 1 - vc;
                let mut picked = Vec::new();
                extract_channel(&all, 2, vc, &mut picked);
                let bit_exact = picked == voice;
                let f0 = check_f0_hz(&picked[picked.len() / 2..], rate);
                let cents = f0.map(|f| 1200.0 * (f / tone.f0_hz).log2());
                let suggestion = suggest_voice_channel(&levels);
                let lv = &levels.channels[vc];
                let lo = &levels.channels[oc];
                let expect_rms = 20.0 * voice_rms.log10();
                // Over the last 300 ms the RMS of a steady tone equals its overall RMS.
                let pass = bit_exact
                    && (lv.peak_3s_dbfs + 12.0).abs() < 0.1
                    && (f64::from(lv.rms_dbfs) - expect_rms).abs() < 0.2
                    && suggestion == Some(vc)
                    && cents.is_some_and(|c| c.abs() < 5.0)
                    && match other {
                        OtherChannel::Silence => lo.peak_dbfs <= -159.0,
                        OtherChannel::Noise { .. } => (lo.rms_dbfs + 70.0).abs() < 1.0,
                        OtherChannel::Tone { .. } => (lo.peak_3s_dbfs + 30.0).abs() < 0.1,
                    };
                all_pass &= pass;
                println!(
                    "  {:>6} Hz, voice on ch {} | other: {:<24} | voice peak {:6.2} rms {:6.2} (expect {:6.2}) | other peak {:7.2} rms {:7.2} | suggested ch {} | bit-exact {} | pitch {:+.2} cents | {}",
                    rate,
                    vc + 1,
                    other_name,
                    lv.peak_3s_dbfs,
                    lv.rms_dbfs,
                    expect_rms,
                    lo.peak_3s_dbfs,
                    lo.rms_dbfs,
                    suggestion.map_or("none".into(), |c| (c + 1).to_string()),
                    bit_exact,
                    cents.unwrap_or(f64::NAN),
                    if pass { "PASS" } else { "FAIL" }
                );
                rows.push(json!({
                    "rate": rate, "voice_channel": vc + 1, "other": other_name,
                    "voice_peak_dbfs": lv.peak_3s_dbfs, "voice_rms_dbfs": lv.rms_dbfs,
                    "expected_voice_rms_dbfs": expect_rms,
                    "other_peak_dbfs": lo.peak_3s_dbfs, "other_rms_dbfs": lo.rms_dbfs,
                    "other_noise_floor_dbfs": lo.noise_floor_dbfs,
                    "suggested_channel": suggestion.map(|c| c + 1),
                    "bit_exact": bit_exact, "pitch_error_cents": cents, "pass": pass,
                }));
            }
        }
    }

    // One run in real time at 48 kHz: time stamps against the host clock.
    let rate = 48_000;
    let x = place_voice(&tone.render(rate, 3.0), rate, 2, 1, OtherChannel::Silence);
    let src = SignalSource::new("generated, real time", rate, 2, x);
    let mut s = InputStream::open(Box::new(src)).expect("open");
    let fmt = s.format().clone();
    let (stamps, arrivals, _) = drain(&mut s, 4.0);
    let mut lag: Vec<f64> = stamps
        .iter()
        .zip(&arrivals)
        .map(|(st, a)| {
            (*a - st.host_ns) as f64 / 1e6 - f64::from(st.frames) / f64::from(rate) * 1e3
        })
        .collect();
    let lag_median = percentile(&mut lag.clone(), 0.5);
    let lag_max = percentile(&mut lag, 1.0);
    println!(
        "  real-time pacing, 48 kHz, {} blocks of {} frames: block reached the reader {:.2} ms (median) / {:.2} ms (max) after its last frame",
        stamps.len(),
        fmt.max_block_frames,
        lag_median,
        lag_max
    );
    println!(
        "  all generated-signal checks: {}",
        if all_pass { "PASS" } else { "FAIL" }
    );
    (
        json!({
            "rows": rows,
            "realtime_48k": { "blocks": stamps.len(), "reader_lag_ms_median": lag_median, "reader_lag_ms_max": lag_max },
            "pass": all_pass,
        }),
        all_pass,
    )
}

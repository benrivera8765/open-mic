// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice mode's output tap, end to end on this computer's real output device (Windows):
//! the engine plays a generated song at master 0.001 (-60 dB: WO2 allows nothing louder),
//! the output tap is read on one thread with its DAC stamps, and the recorder's capture
//! code (`jk_recorder::input`, the microphone path) records the same output device's
//! loopback with its own host-clock stamps. The two streams are lined up once by their
//! stamps (the reference handed 50 ms early, as docs/ARCHITECTURE.md section 12.3 says),
//! then fed continuously to the echo canceller, which must find the "returned" playback
//! and measure how late it comes relative to the stamps.
//!
//! What it shows: the tap is readable by an analysis thread next to a capture stream, the
//! two clocks meet (engine `now_ns` and the recorder's QPC clock, offset measured once),
//! and the residual delay the canceller measures is what the stamps leave unexplained
//! (for a loopback: where Windows takes the copy versus the DAC). An independent brute-force
//! cross-correlation checks the canceller's delay. Only numbers are printed; nothing is
//! written; no microphone is opened (the loopback test device is the output itself).
//!
//! ```text
//! cargo run --release -p jk-engine --example tap_loopback -- [--secs 20]
//! ```

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};

    use jk_engine::{Engine, EngineConfig, MixParams, PlaybackState};
    use jk_pitch::echo::{EchoCanceller, EchoConfig, Presence};
    use jk_recorder::input::{clock::host_time_ns, loopback_test_device_id, DeviceSource, InputStream};

    let args: Vec<String> = std::env::args().collect();
    let secs: f64 = args
        .iter()
        .position(|a| a == "--secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let lead_s = 0.050;

    let dir = std::env::temp_dir().join("jk-tap-loopback");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let song = dir.join("tap.jksong");
    jk_engine::demo::make_synth_song(&song, secs + 10.0, 44_100, 7).expect("song");

    let engine = Engine::new(EngineConfig { device_id: None, exclusive: false, master: 0.001 }).expect("engine");
    engine.load(&song, MixParams::default()).expect("load");
    let id = loopback_test_device_id().expect("default output device id");
    let mut input = InputStream::open(Box::new(DeviceSource::new(id))).expect("loopback capture");
    let fmt = input.format().clone();
    let mut tap = engine.output_tap();
    engine.play();
    let t0 = Instant::now();
    while engine.snapshot().state != PlaybackState::Playing && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let dev = engine.snapshot().device;
    println!("output: {dev:?}");
    println!("capture: {} / {} Hz, {} ch, declared latency {:?} ns", fmt.source, fmt.sample_rate, fmt.channels, fmt.declared_latency_ns);

    // The two host clocks: the engine's (Instant from a process epoch) and the recorder's
    // (QPC). Offset from the tightest of 200 back-to-back readings.
    let mut best = (i64::MAX, 0i64);
    for _ in 0..200 {
        let a = jk_engine::now_ns() as i64;
        let h = host_time_ns();
        let b = jk_engine::now_ns() as i64;
        if b - a < best.0 {
            best = (b - a, h - (a + b) / 2);
        }
    }
    let off = best.1;
    println!("clock offset recorder - engine: {off} ns (bracket {} ns)", best.0);

    // Collect both streams (the analysis thread's job; here one loop does both).
    let mut tap_buf = vec![0.0f32; 2 * 2048];
    let mut tap_s: Vec<f32> = Vec::new();
    let mut tap_anchor: Vec<(u64, i64, u32)> = Vec::new(); // (index in tap_s frames, host ns, rate)
    let mut cap_block = Vec::new();
    let mut cap_s: Vec<f32> = Vec::new();
    let mut cap_anchor: Vec<(u64, i64)> = Vec::new();
    let mut tap_lost = 0u64;
    let mut cap_gaps = 0u32;
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        while let Some(p) = tap.read(&mut tap_buf) {
            tap_lost += p.lost_before;
            tap_anchor.push(((tap_s.len() / 2) as u64, p.dac_ns as i64 + off, p.rate));
            tap_s.extend_from_slice(&tap_buf[..2 * p.frames]);
        }
        while let Some(st) = input.read(&mut cap_block) {
            if st.discontinuity() {
                cap_gaps += 1;
            }
            let ch = fmt.channels as usize;
            cap_anchor.push(((cap_s.len()) as u64, st.host_ns));
            for f in cap_block.chunks_exact(ch) {
                cap_s.push(f.iter().sum::<f32>() / ch as f32);
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    drop(engine);
    let rate_t = tap_anchor.first().map_or(0, |a| a.2);
    println!(
        "tap: {} frames at {} Hz, {} lost; capture: {} frames at {} Hz, {} discontinuities",
        tap_s.len() / 2,
        rate_t,
        tap_lost,
        cap_s.len(),
        fmt.sample_rate,
        cap_gaps
    );
    assert_eq!(rate_t, fmt.sample_rate, "this check needs the tap and the loopback at one rate");
    let fs = f64::from(fmt.sample_rate);

    // Line up once: the tap frame that left the DAC `lead_s` before the first captured
    // frame (piecewise from the tap's stamps), then both streams continuously.
    let host_of_tap = |k: f64| -> f64 {
        let i = tap_anchor.partition_point(|a| (a.0 as f64) <= k).saturating_sub(1);
        let a = tap_anchor[i];
        a.1 as f64 + (k - a.0 as f64) / fs * 1e9
    };
    // The first captured frame at least half a second into the tap (the capture opened
    // before the reader attached), by the capture's own stamps.
    let host_of_cap = |j: usize| -> f64 {
        let i = cap_anchor.partition_point(|a| (a.0 as usize) <= j).saturating_sub(1);
        let a = cap_anchor[i];
        a.1 as f64 + (j as f64 - a.0 as f64) / fs * 1e9
    };
    let tap_start = host_of_tap(0.0);
    let c0 = (0..cap_s.len()).find(|&j| host_of_cap(j) - lead_s * 1e9 >= tap_start + 0.5e9).expect("capture overlaps the tap");
    let target = host_of_cap(c0) - lead_s * 1e9;
    // Bisection for the tap index whose host time is `target`.
    let (mut lo, mut hi) = (0.0f64, (tap_s.len() / 2) as f64 - 1.0);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if host_of_tap(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let k0 = lo.round() as i64;
    println!("alignment: capture frame {c0} <-> tap frame {k0} (reference handed {:.0} ms early)", lead_s * 1e3);
    if k0 < 0 {
        println!("the tap starts after the capture: run longer");
        return;
    }
    let k0 = k0 as usize;
    let n = (cap_s.len() - c0).min(tap_s.len() / 2 - k0);
    let cfg = EchoConfig { music_floor_dbfs: -140.0, ..EchoConfig::default() };
    let mut ec = EchoCanceller::with_config(fmt.sample_rate, cfg);
    let mut out = vec![0.0f32; 480];
    let mut i = 0;
    let mut first_present = None;
    while i + 480 <= n {
        ec.process(&tap_s[2 * (k0 + i)..2 * (k0 + i + 480)], &cap_s[c0 + i..c0 + i + 480], &mut out);
        i += 480;
        let r = ec.report();
        if r.presence == Presence::Present && first_present.is_none() {
            first_present = Some(i as f64 / fs);
        }
        if i % (fmt.sample_rate as usize * 2) < 480 {
            println!(
                "  t {:5.1} s: {:?}, delay {:?} ms, level {:?} dB, drift {:?} ppm, {:?}, suppression {:?} dB",
                i as f64 / fs,
                r.presence,
                r.delay_ms.map(|d| (d * 1000.0).round() / 1000.0),
                r.level_db.map(|d| (d * 10.0).round() / 10.0),
                r.drift_ppm.map(|d| (d * 10.0).round() / 10.0),
                r.stability,
                r.suppression_db.map(|d| (d * 10.0).round() / 10.0)
            );
        }
    }
    let r = ec.report();
    // Independent check: brute-force cross-correlation of the last 2 s, lags 0..120 ms.
    let w = (2.0 * fs) as usize;
    let s0 = n.saturating_sub(w);
    let (mut best_lag, mut best_c) = (0usize, f64::MIN);
    let max_lag = (0.12 * fs) as usize;
    for lag in 0..max_lag.min(s0) {
        let mut acc = 0.0f64;
        for j in s0..n {
            let x = 0.5 * (tap_s[2 * (k0 + j - lag)] + tap_s[2 * (k0 + j - lag) + 1]);
            acc += f64::from(x) * f64::from(cap_s[c0 + j]);
        }
        if acc > best_c {
            best_c = acc;
            best_lag = lag;
        }
    }
    let xcorr_ms = best_lag as f64 / fs * 1e3;
    println!(
        "result: {:?} after {:?} s; canceller delay {:?} ms, cross-correlation {:.3} ms; residual delay (measured minus the {:.0} ms lead): {:?} ms; drift {:?} ppm; {:?}; suppression {:?} dB",
        r.presence,
        first_present,
        r.delay_ms.map(|d| (d * 1000.0).round() / 1000.0),
        xcorr_ms,
        lead_s * 1e3,
        r.delay_ms.map(|d| ((d - lead_s * 1e3) * 1000.0).round() / 1000.0),
        r.drift_ppm.map(|d| (d * 10.0).round() / 10.0),
        r.stability,
        r.suppression_db.map(|d| (d * 10.0).round() / 10.0)
    );
}

#[cfg(not(windows))]
fn main() {
    println!("tap_loopback needs WASAPI loopback (Windows)");
}

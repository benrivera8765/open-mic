// SPDX-License-Identifier: GPL-3.0-or-later
//! The output tap (src/tap.rs, docs/ARCHITECTURE.md section 12): a lock-free copy of what
//! the device plays, with DAC time stamps, read on its own thread.
//!
//! - `tap_is_exactly_what_the_device_played`: on the null device with capture, the tap read
//!   by a separate "analysis" thread equals the captured device buffers bit for bit (graph,
//!   44.1 -> 48 kHz resampler, a master-volume change, a seek with its fades, a pause), with
//!   no frame lost; its stamps advance with the frames and agree with the playback clock.
//! - `no_reader_no_copy_and_playback_unchanged`: with no reader the filler copies nothing,
//!   and the device output is bit-identical with and without a reader.
//!
//! With `JK_TAP_REAL_DEVICE=1` the first test's stamp checks also run on the default output
//! device at master 0.001 (-60 dB; WO2: nothing played above -60 dBFS).
//!
//! The no-allocation check of the callback with a reader attached is in tests/rt_alloc.rs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jk_engine::{Engine, EngineConfig, MixParams, PlaybackState, TapRead};

fn tmp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-output-tap").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_for(secs: f64, what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed().as_secs_f64() < secs, "timed out after {secs} s waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// One tests at a time: they play in real time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the analysis thread got: the samples and every piece's stamp.
struct Taken {
    samples: Vec<f32>,
    pieces: Vec<TapRead>,
}

/// Read the tap on its own thread (as practice mode's analysis thread will) until `stop`.
fn spawn_reader(engine: &Engine, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Taken> {
    let mut r = engine.output_tap();
    std::thread::Builder::new()
        .name("jk-practice-test".into())
        .spawn(move || {
            let mut buf = vec![0.0f32; 2 * 1024];
            let mut t = Taken { samples: Vec::new(), pieces: Vec::new() };
            loop {
                let last = stop.load(Acquire);
                while let Some(p) = r.read(&mut buf) {
                    t.samples.extend_from_slice(&buf[..2 * p.frames]);
                    t.pieces.push(p);
                }
                if last {
                    break;
                }
                std::thread::sleep(Duration::from_millis(3));
            }
            t
        })
        .unwrap()
}

/// Snapshots of the playback clock taken while the reader runs.
fn clock_samples(engine: &Engine, ms: u64, out: &mut Vec<(f64, u64)>) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        let s = engine.snapshot();
        if s.state == PlaybackState::Playing && s.position_ns > 0 {
            out.push((s.position_s, s.position_ns));
        }
        sleep_ms(7);
    }
}

/// Checks the stamps: frames contiguous (nothing lost), DAC times advancing with the
/// frames, and the song position the tap reports at a DAC time against the playback
/// clock's. Returns (median, 99th percentile) clock disagreement in ms (the worst few are
/// the moments of the seek and the pause, where the clock reports the held or target
/// position of a fade).
fn check_stamps(label: &str, t: &Taken, clock: &[(f64, u64)], window: std::ops::RangeInclusive<f64>) -> (f64, f64) {
    assert!(!t.pieces.is_empty(), "{label}: nothing read");
    let mut next = t.pieces[0].first_frame;
    let mut steps = Vec::new();
    for w in t.pieces.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        assert_eq!(b.lost_before, 0, "{label}: frames lost");
        assert_eq!(b.first_frame, a.first_frame + a.frames as u64, "{label}: frames not contiguous");
        assert!(b.dac_ns >= a.dac_ns, "{label}: DAC time went backwards");
        // Inside one callback block the stamps are exact; across blocks they carry the
        // callback timing jitter.
        steps.push((b.dac_ns as f64 - a.dac_ns as f64) / 1e6 - a.frames as f64 * 1e3 / a.rate as f64);
    }
    for p in &t.pieces {
        assert_eq!(p.first_frame, next);
        next += p.frames as u64;
    }
    steps.sort_by(f64::total_cmp);
    let p50 = steps[steps.len() / 2];
    let p99 = steps[steps.len() * 99 / 100];
    // Song position at a DAC time: the tap's piece nearest each clock sample.
    let mut err = Vec::new();
    for &(pos, ns) in clock {
        let i = t.pieces.partition_point(|p| p.dac_ns < ns);
        for p in t.pieces[i.saturating_sub(1)..(i + 1).min(t.pieces.len())].iter() {
            let Some(sp) = p.song_pos else { continue };
            // `window`: which pieces to compare with the clock sample (seconds from the
            // piece's DAC stamp). On the null device only the piece of the callback the
            // snapshot came from: across callbacks the comparison would measure the callback
            // timing jitter, which reaches tens of ms on a loaded computer (the null device
            // paces itself with sleeps). A real device's callbacks follow its clock.
            let dt = (ns as f64 - p.dac_ns as f64) / 1e9;
            if !window.contains(&dt) {
                continue;
            }
            let e = (sp + dt - pos) * 1e3;
            // Across a seek or pause the two refer to different stretches: not comparable.
            if e.abs() < 100.0 {
                err.push(e.abs());
            }
        }
    }
    assert!(err.len() > 20, "{label}: too few clock comparisons ({})", err.len());
    err.sort_by(f64::total_cmp);
    let med = err[err.len() / 2];
    let e99 = err[err.len() * 99 / 100];
    let worst = err[err.len() - 1];
    println!(
        "{label}: {} pieces, {} frames; DAC stamp step minus frame duration: median {p50:.3} ms, 99th percentile {p99:.3} ms; tap song position vs playback clock at the same DAC time: median {med:.3} ms, 99th percentile {e99:.3} ms, worst {worst:.3} ms ({} comparisons)",
        t.pieces.len(),
        next - t.pieces[0].first_frame,
        err.len()
    );
    (med, e99)
}

#[test]
fn tap_is_exactly_what_the_device_played() {
    let _g = serial();
    let dir = tmp_dir("exact");
    let song = dir.join("a.jksong");
    jk_engine::demo::make_synth_song(&song, 20.0, 44_100, 7).unwrap();
    let (engine, cap) = Engine::new_with_capture(48_000, false, 0.5).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.play();
    wait_for(10.0, "playing", || engine.snapshot().position_s > 1.0);

    let stop = Arc::new(AtomicBool::new(false));
    let reader = spawn_reader(&engine, stop.clone());
    let mut clock = Vec::new();
    clock_samples(&engine, 1000, &mut clock);
    engine.set_master(0.2);
    clock_samples(&engine, 800, &mut clock);
    engine.seek(9.0);
    clock_samples(&engine, 800, &mut clock);
    engine.pause();
    sleep_ms(300);
    engine.play();
    clock_samples(&engine, 800, &mut clock);
    stop.store(true, Release);
    let taken = reader.join().unwrap();
    let captured = cap.lock().unwrap().clone();
    drop(engine);

    // Where the tap starts in the capture: the first 512 frames match exactly once.
    let n0 = 512 * 2;
    assert!(taken.samples.len() > 3 * 48_000 * 2, "tap too short: {} samples", taken.samples.len());
    let head = &taken.samples[..n0];
    assert!(head.iter().any(|v| *v != 0.0), "the tap started on silence");
    let starts: Vec<usize> = (0..captured.len().saturating_sub(n0))
        .step_by(2)
        .filter(|&i| captured[i] == head[0] && captured[i..i + n0] == *head)
        .collect();
    assert_eq!(starts.len(), 1, "tap head found {} times in the capture", starts.len());
    let s0 = starts[0];
    let n = taken.samples.len().min(captured.len() - s0);
    assert!(n + 2 * 48_000 / 100 >= taken.samples.len(), "capture ends early");
    let mismatch = (0..n).find(|&i| captured[s0 + i].to_bits() != taken.samples[i].to_bits());
    assert_eq!(mismatch, None, "tap differs from the device output");
    // The master change, the seek fades and the pause silence are all in there.
    let silent = taken.samples.chunks(960).filter(|c| c.iter().all(|v| *v == 0.0)).count();
    assert!(silent >= 20, "the pause should show as silence in the tap ({silent} silent 10 ms blocks)");
    println!("tap = device output, bit for bit: {} frames compared (capture offset {} frames)", n / 2, s0 / 2);

    let (med, e99) = check_stamps("null:48000", &taken, &clock, 0.0..=0.002);
    // Same callback, same clock: within the microseconds between the two reads of the
    // clock in the callback.
    assert!(med < 0.1 && e99 < 0.5, "stamps disagree with the playback clock: median {med} ms, 99th percentile {e99} ms");

    if std::env::var_os("JK_TAP_REAL_DEVICE").is_some() {
        let engine = Engine::new(EngineConfig { device_id: None, exclusive: false, master: 0.001 }).unwrap();
        engine.load(&song, MixParams::default()).unwrap();
        engine.play();
        wait_for(10.0, "playing", || engine.snapshot().position_s > 1.0);
        let stop = Arc::new(AtomicBool::new(false));
        let reader = spawn_reader(&engine, stop.clone());
        let mut clock = Vec::new();
        clock_samples(&engine, 3000, &mut clock);
        stop.store(true, Release);
        let taken = reader.join().unwrap();
        let dev = engine.snapshot().device;
        println!("real device: {dev:?}");
        let (med, e99) = check_stamps("default device", &taken, &clock, -0.02..=0.02);
        // The tap stamps the callback's start (when the device's queue was measured), the
        // clock its end: they differ by the callback's own duration, about 0.1 ms here.
        assert!(med < 0.25 && e99 < 0.75, "real device: stamps disagree with the playback clock: median {med} ms, 99th percentile {e99} ms");
    }
}

#[test]
fn no_reader_no_copy_and_playback_unchanged() {
    let _g = serial();
    let dir = tmp_dir("unchanged");
    let song = dir.join("b.jksong");
    jk_engine::demo::make_synth_song(&song, 6.0, 44_100, 11).unwrap();

    // Fast capture (consumed as fast as produced), with and without a reader.
    let render = |with_reader: bool| -> (Vec<f32>, u64) {
        let (engine, cap) = Engine::new_with_capture(48_000, true, 0.7).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let reader = with_reader.then(|| spawn_reader(&engine, stop.clone()));
        engine.load(&song, MixParams::default()).unwrap();
        engine.play();
        wait_for(30.0, "end", || engine.snapshot().state == PlaybackState::Ended);
        stop.store(true, Release);
        let got = reader.map(|h| h.join().unwrap().samples.len() as u64 / 2).unwrap_or(0);
        let mut v = cap.lock().unwrap().clone();
        // Leading silence depends on when the device thread started: not part of the song.
        let first = v.iter().position(|x| *x != 0.0).unwrap_or(v.len()) & !1;
        v.drain(..first);
        let last = v.iter().rposition(|x| *x != 0.0).map_or(0, |i| (i | 1) + 1);
        v.truncate(last);
        (v, got)
    };
    let (plain, _) = render(false);
    let (tapped, got) = render(true);
    assert!(plain.len() > 5 * 48_000 * 2);
    assert!(got as usize * 2 >= plain.len(), "the reader saw {got} frames of {}", plain.len() / 2);
    assert_eq!(plain.len(), tapped.len(), "output length changed with a reader attached");
    let diff = plain.iter().zip(&tapped).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
    assert_eq!(diff, 0, "device output differs with a reader attached");
    println!("device output with and without a reader: identical over {} frames", plain.len() / 2);

    // No reader: nothing is written.
    let engine = Engine::new(EngineConfig { device_id: Some("null:48000".into()), exclusive: false, master: 0.0 }).unwrap();
    engine.load(&song, MixParams::default()).unwrap();
    engine.play();
    wait_for(10.0, "playing", || engine.snapshot().position_s > 0.3);
    let r = engine.output_tap();
    sleep_ms(200);
    let a = r.total_written();
    assert!(a > 0, "a reader is attached: the filler should write");
    drop(r);
    // At most one block that was being written when the reader left.
    sleep_ms(30);
    let b = engine.output_tap().total_written();
    sleep_ms(500);
    let r2 = engine.output_tap();
    let c = r2.total_written();
    println!("tap frames written: {a} with a reader, {} more in the 30 ms after it left, then the next reader found {}", b - a, c);
    assert!(b - a <= 480 * 4, "the filler kept copying after the reader left ({} frames)", b - a);
    // `b` was read through a reader that existed for an instant: allow its few blocks.
    assert!(c - b <= 480 * 2, "the filler copied {} frames with no reader attached", c - b);
}

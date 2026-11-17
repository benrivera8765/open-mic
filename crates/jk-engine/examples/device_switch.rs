// SPDX-License-Identifier: GPL-3.0-or-later
//! Device handling on the real output device, at digital silence (master 0):
//! shared -> exclusive -> shared -> null -> default while playing, then a simulated
//! device loss (the WASAPI thread fails through the same path as an invalidated device)
//! and `play()` reopening it. Prints the position and counters after each step.
//!
//! cargo run --release -p jk-engine --example device_switch -- <song.jksong>

use std::time::{Duration, Instant};

use jk_engine::{Engine, EngineConfig, EngineEvent, MixParams};

fn main() {
    let song = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: device_switch <song.jksong>"));
    let engine = Engine::new(EngineConfig { device_id: None, exclusive: false, master: 0.0 }).unwrap();
    let events = engine.events();
    engine.load(&song, MixParams { semitones: 2, ..MixParams::default() }).unwrap();
    engine.seek(5.0);
    engine.play();
    let show = |what: &str, e: &Engine| {
        let s = e.snapshot();
        let d = e.diagnostics();
        println!(
            "{what:<38} state {:?}, pos {:7.3} s, device {:?}, underruns {}, render-thread RB builds {}",
            s.state,
            s.position_s,
            s.device.as_ref().map(|d| format!("{} {} Hz excl {} latency {:.1} ms", d.name, d.sample_rate, d.exclusive, d.latency_ms)),
            d.underruns,
            d.inline_constructions
        );
    };
    std::thread::sleep(Duration::from_millis(1000));
    show("shared, default device", &engine);
    for (id, excl, label) in [
        (None, true, "-> exclusive (default device)"),
        (None, false, "-> shared again"),
        (Some("null"), false, "-> null device"),
        (None, false, "-> default device"),
    ] {
        let p0 = engine.snapshot().position_s;
        let t = Instant::now();
        let r = engine.set_device(id, excl);
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        std::thread::sleep(Duration::from_millis(1000));
        let p1 = engine.snapshot().position_s;
        println!("  set_device {label}: {:?} in {dt:.0} ms; position {p0:.3} -> {p1:.3} s after 1 s", r.as_ref().map_err(|e| e.code.clone()));
        show(label, &engine);
    }
    let p0 = engine.snapshot().position_s;
    let t = Instant::now();
    engine.debug_simulate_device_loss();
    while engine.snapshot().state == jk_engine::PlaybackState::Playing && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("  simulated device loss: paused after {:.0} ms at {:.3} s (was {p0:.3})", t.elapsed().as_secs_f64() * 1000.0, engine.snapshot().position_s);
    let evs: Vec<EngineEvent> = events.try_iter().collect();
    println!("  DeviceLost event: {}", evs.contains(&EngineEvent::DeviceLost));
    show("after loss", &engine);
    engine.play();
    std::thread::sleep(Duration::from_millis(1000));
    show("play() reopened", &engine);
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The key in effect (Work Order 2: 4.6 "Changing key during a song", trap 1, and the
//! section 9 row "Key change during a song"), end to end on the null device in both key
//! modes:
//!
//! * the key published in `Snapshot::key` (`heard`, `pending`) changes exactly where the
//!   pitch of the played audio changes: a steady tone in the lead stem is played, keys are
//!   changed during playback, and the moment the tone's pitch really changes in the
//!   captured device output is measured and compared with the published switch;
//! * every change is published (with its song position) before it reaches the DAC;
//! * render-ahead mode keeps the old key as "heard" with the new one pending (no position)
//!   until the engine crosses over to the rendered key;
//! * seeks (also during a crossfade, and in render-ahead mode before the new key is
//!   rendered), paused key changes and `Engine::key_at` for the recent past.
//!
//! The measured offsets are printed (`--nocapture`) for docs/benchmarks/practice-key.md.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use jk_dsp::analysis::tone_switch_index;
use jk_engine::{Engine, KeyMode, KeyStatus, MixParams, PlaybackState};

const RATE: u32 = 44_100;
const F0: f64 = 440.0;
/// The section 9 bar: published switch vs the pitch change in the produced audio.
const BAR_MS: f64 = 50.0;

fn tmp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-key-in-effect").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tone_song(name: &str, seconds: f64) -> PathBuf {
    let p = tmp_dir(&format!("song-{name}")).join(format!("{name}.jksong"));
    jk_engine::demo::make_tone_song(&p, seconds, RATE, F0).unwrap();
    p
}

/// Key `k` with the lead stem (the tone) audible.
fn params(k: i32) -> MixParams {
    MixParams { semitones: k, guide_vocal: 1.0, ..MixParams::default() }
}

fn tone(k: i32) -> f64 {
    F0 * (k as f64 / 12.0).exp2()
}

fn wait_for(secs: f64, what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed().as_secs_f64() < secs, "timed out after {secs} s waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The tests play in real time and two use the render-ahead renderer, which other tests
/// would starve of CPU: one at a time (as in tests/keycache.rs).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// One snapshot taken while playing.
struct Sample {
    /// Seconds since play.
    t: f64,
    state: PlaybackState,
    pos: f64,
    key: KeyStatus,
}

/// Seek to `start_s` (key 0, paused), clear the capture, play, change key at the given
/// wall-clock times (seconds after play) and take a snapshot every ~2 ms for `secs`.
/// Returns the snapshots and, per change, the position at the request.
fn play_plan(engine: &Engine, cap: &jk_engine::output::Capture, start_s: f64, plan: &[(f64, i32)], secs: f64) -> (Vec<Sample>, Vec<f64>) {
    engine.seek(start_s);
    wait_for(5.0, "the paused seek", || engine.snapshot().key.segment > 0 && engine.key_at(start_s, Some(engine.snapshot().key.segment)).is_some());
    cap.lock().unwrap().clear();
    engine.play();
    let t0 = Instant::now();
    let mut samples = Vec::new();
    let mut requested_at = Vec::new();
    let mut next = 0;
    while t0.elapsed().as_secs_f64() < secs {
        let t = t0.elapsed().as_secs_f64();
        if next < plan.len() && t >= plan[next].0 {
            requested_at.push(engine.snapshot().position_s);
            engine.set_params(params(plan[next].1));
            next += 1;
        }
        let s = engine.snapshot();
        samples.push(Sample { t, state: s.state, pos: s.position_s, key: s.key });
        std::thread::sleep(Duration::from_millis(2));
    }
    engine.pause();
    (samples, requested_at)
}

/// Check the published key against the audio and against itself; returns the offsets
/// (ms, measured pitch change minus published switch) of every change. `dev_rate`: the
/// null device's rate (the capture's rate; the song is at `RATE`).
#[allow(clippy::too_many_arguments)]
fn check(label: &str, engine: &Engine, cap: &jk_engine::output::Capture, start_s: f64, dev_rate: u32, samples: &[Sample], requested_at: &[f64], plan: &[(f64, i32)]) -> Vec<f64> {
    let d = engine.diagnostics();
    assert_eq!(d.underruns, 0, "{label}: an underrun breaks the capture's timeline");
    assert_eq!(d.key_log_lost, 0);
    // Capture index of song frame `start`: the first non-zero frame (the segment fades
    // in from its first frame; everything before is digital silence).
    let left: Vec<f32> = cap.lock().unwrap().chunks(2).map(|f| f[0]).collect();
    let k0 = left.iter().position(|&v| v != 0.0).expect("audio in the capture");
    // Song time of the segment's first frame, and capture index of a song time (device
    // frames; the output resampler's delay is compensated by the engine).
    let start_song_s = (start_s * RATE as f64).round() / RATE as f64;
    let idx_of = |song_s: f64| (k0 as i64 + ((song_s - start_song_s) * dev_rate as f64).round() as i64) as usize;

    let segment = samples.last().unwrap().key.segment;
    let timeline = engine.key_timeline();
    let seg = timeline.iter().find(|s| s.segment == segment).expect("the played segment");
    let switches: Vec<(f64, i32)> = seg.keys[1..].to_vec();
    assert_eq!(seg.keys[0].1, 0, "{label}: starts in key 0");
    assert_eq!(switches.iter().map(|s| s.1).collect::<Vec<_>>(), plan.iter().map(|p| p.1).collect::<Vec<_>>(), "{label}: one switch per change, in order");

    // Every snapshot agrees with the history: the key heard at its position, and the
    // next change ahead of it (with the position it ended up having).
    for s in samples.iter().filter(|s| s.state == PlaybackState::Playing && s.key.segment == segment) {
        let want = engine.key_at(s.pos, Some(segment)).unwrap();
        assert_eq!(s.key.heard, want, "{label}: heard {} at {:.4} s, history says {want}", s.key.heard, s.pos);
        if let Some(p) = s.key.pending
            && let Some(at) = p.at_s
        {
            assert!(at > s.pos, "{label}: a pending switch lies ahead of the DAC");
            assert!(switches.iter().any(|w| (w.0 - at).abs() < 1e-9 && w.1 == p.semitones), "{label}: pending {p:?} is a real switch");
        }
    }

    let mut offsets = Vec::new();
    let mut prev = 0;
    for (i, &(at, key)) in switches.iter().enumerate() {
        let around = idx_of(at);
        let exact = k0 as f64 + (at - start_song_s) * dev_rate as f64;
        let measured = tone_switch_index(&left, dev_rate, tone(prev), tone(key), around, dev_rate as usize / 10, dev_rate as usize / 2000);
        let measured = measured.unwrap_or_else(|| panic!("{label}: no pitch change found near {at:.3} s"));
        let off_ms = (measured - exact) * 1e3 / dev_rate as f64;
        // When the change was first published with its position, and first heard.
        let published = samples.iter().find(|s| s.key.pending.is_some_and(|p| p.at_s.is_some_and(|a| (a - at).abs() < 1e-9)));
        let heard = samples.iter().find(|s| s.key.segment == segment && s.pos >= at && s.key.heard == key);
        let notice_ms = match (published, heard) {
            (Some(p), Some(h)) => (h.t - p.t) * 1e3,
            _ => f64::NAN,
        };
        let lag_ms = (at - requested_at[i]) * 1e3;
        println!(
            "{label}: {prev:+} -> {key:+}: published switch at {at:.4} s ({lag_ms:.0} ms of song after the request), published {notice_ms:.0} ms before the DAC got there; pitch change measured in the played audio {off_ms:+.2} ms from the published switch"
        );
        assert!(heard.is_some(), "{label}: the change to {key:+} was never reported as heard");
        offsets.push(off_ms);
        prev = key;
    }
    let worst = offsets.iter().fold(0.0f64, |m, o| m.max(o.abs()));
    println!("{label}: SUMMARY {} changes, worst |measured - published| {worst:.2} ms (bar {BAR_MS} ms)", offsets.len());
    assert!(worst <= BAR_MS, "{label}: published switch {worst:.1} ms away from the pitch change");
    offsets
}

#[test]
fn realtime_key_in_effect_switches_where_the_pitch_changes() {
    let _serial = serial();
    let song = tone_song("rt", 14.0);
    let (engine, cap) = Engine::new_with_capture(RATE, false, 1.0).unwrap();
    engine.load(&song, params(0)).unwrap();
    assert_eq!(engine.snapshot().key.requested, 0);
    // Steps of 3 to 24 semitones, both ends of the range.
    let plan = [(1.0, 3), (2.5, -4), (4.0, 7), (5.5, -9), (7.0, 12), (8.5, -12), (10.0, 0)];
    let (samples, req) = play_plan(&engine, &cap, 0.5, &plan, 11.5);
    // Straight after each request the key heard is still the old one.
    for (i, &(t, k)) in plan.iter().enumerate() {
        let s = samples.iter().find(|s| s.t >= t && s.key.requested == k).unwrap();
        let old = if i == 0 { 0 } else { plan[i - 1].1 };
        assert_eq!(s.key.heard, old, "real time: the old key is heard right after the request");
        assert_eq!(s.key.pending.map(|p| p.semitones), Some(k));
    }
    check("realtime", &engine, &cap, 0.5, RATE, &samples, &req, &plan);
}

#[test]
fn prerender_key_in_effect_is_the_crossover_to_the_rendered_key() {
    let _serial = serial();
    let song = tone_song("pre", 20.0);
    let cache = tmp_dir("cache-pre");
    let (engine, cap) = Engine::new_with_capture(RATE, false, 1.0).unwrap();
    engine.set_key_cache(Some(&cache), 1 << 30).unwrap();
    engine.set_key_mode(KeyMode::Prerender);
    engine.load(&song, params(0)).unwrap();
    let plan = [(0.5, 3), (3.5, -4), (6.5, 7), (9.5, 0)];
    let (samples, req) = play_plan(&engine, &cap, 0.5, &plan, 12.5);
    assert_eq!(samples.last().unwrap().key.mode, KeyMode::Prerender);
    // While a new key is being rendered the old one is heard and the new one is pending
    // without a position.
    for (i, &(t, k)) in plan.iter().enumerate() {
        let s = samples.iter().find(|s| s.t >= t && s.key.requested == k).unwrap();
        let old = if i == 0 { 0 } else { plan[i - 1].1 };
        assert_eq!(s.key.heard, old);
        if k != 0 {
            assert_eq!(s.key.pending.map(|p| (p.semitones, p.at_s)), Some((k, None)), "render ahead: pending, no position yet");
        }
        let waited = samples.iter().filter(|x| x.t >= t && x.key.requested == k && x.key.pending.is_some_and(|p| p.at_s.is_none())).count();
        println!("prerender: {old:+} -> {k:+}: pending without a position for {} snapshots (about {} ms) while the key was rendered", waited, waited * 2);
    }
    check("prerender", &engine, &cap, 0.5, RATE, &samples, &req, &plan);
}

/// The usual case on a real computer: a 44.1 kHz song on a 48 kHz output (Windows
/// shared mode, the M1's built-in output). The output resampler sits between the graph
/// and the DAC; the published switch must still be where the pitch changes, in both
/// key modes.
#[test]
fn key_in_effect_on_a_resampling_device_in_both_modes() {
    let _serial = serial();
    const DEV: u32 = 48_000;
    let song = tone_song("rs", 12.0);
    let (engine, cap) = Engine::new_with_capture(DEV, false, 1.0).unwrap();
    engine.load(&song, params(0)).unwrap();
    let plan = [(1.0, 5), (2.5, -7), (4.0, 12), (5.5, 0)];
    let (samples, req) = play_plan(&engine, &cap, 0.5, &plan, 7.0);
    check("realtime, 44.1 kHz song on a 48 kHz device", &engine, &cap, 0.5, DEV, &samples, &req, &plan);
    drop(engine);

    let cache = tmp_dir("cache-rs");
    let (engine, cap) = Engine::new_with_capture(DEV, false, 1.0).unwrap();
    engine.set_key_cache(Some(&cache), 1 << 30).unwrap();
    engine.set_key_mode(KeyMode::Prerender);
    engine.load(&song, params(0)).unwrap();
    let plan = [(0.5, -5), (4.0, 0)];
    let (samples, req) = play_plan(&engine, &cap, 0.5, &plan, 6.0);
    assert_eq!(samples.last().unwrap().key.mode, KeyMode::Prerender);
    check("prerender, 44.1 kHz song on a 48 kHz device", &engine, &cap, 0.5, DEV, &samples, &req, &plan);
}

#[test]
fn seeks_crossfades_and_paused_changes_report_the_key_heard() {
    let _serial = serial();
    let song = tone_song("seek", 16.0);
    let (engine, _cap) = Engine::new_with_capture(RATE, false, 1.0).unwrap();
    engine.load(&song, params(0)).unwrap();
    engine.seek(1.0);
    engine.play();
    wait_for(5.0, "playing past 2 s", || engine.snapshot().position_s > 2.0);
    let seg_a = engine.snapshot().key.segment;

    // A key change, then a seek before it is heard (its crossfade is scheduled but not
    // at the DAC): the new segment starts in the new key at once, the old segment keeps
    // its history.
    engine.set_params(params(5));
    wait_for(1.0, "the switch to be scheduled", || engine.snapshot().key.pending.is_some_and(|p| p.at_s.is_some()));
    let s = engine.snapshot();
    let at = s.key.pending.unwrap().at_s.unwrap();
    assert_eq!((s.key.heard, s.key.requested), (0, 5));
    assert!(at > s.position_s);
    let before_seek = s.position_s;
    engine.seek(8.0);
    wait_for(2.0, "the new segment", || {
        let s = engine.snapshot();
        s.key.segment != seg_a && s.position_s >= 8.0 && s.key.heard == 5 && engine.key_at(8.0, Some(s.key.segment)).is_some()
    });
    let s = engine.snapshot();
    assert_eq!(s.key.pending, None);
    let seg_b = s.key.segment;
    assert_eq!(engine.key_at(8.05, Some(seg_b)), Some(5));
    assert_eq!(engine.key_at(before_seek - 0.5, Some(seg_a)), Some(0), "the old segment's history is kept");
    assert_eq!(engine.key_at(before_seek - 0.5, None), Some(0), "the newest segment that played 1.5 s");
    wait_for(3.0, "playing past 9 s", || engine.snapshot().position_s > 9.0);
    assert_eq!(engine.key_at(8.5, None), Some(5));

    // During a crossfade: the key heard switches exactly at the published position.
    engine.set_params(params(-7));
    wait_for(1.0, "the switch to be scheduled", || engine.snapshot().key.pending.is_some_and(|p| p.at_s.is_some()));
    let at = engine.snapshot().key.pending.unwrap().at_s.unwrap();
    let mut seen = (0, 0);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(800) {
        let s = engine.snapshot();
        assert_eq!(s.key.heard, if s.position_s >= at { -7 } else { 5 }, "heard at {:.4} s, switch at {at:.4} s", s.position_s);
        if (s.position_s - at).abs() < 0.03 {
            seen.0 += (s.position_s < at) as u32;
            seen.1 += (s.position_s >= at) as u32;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(seen.0 > 0 && seen.1 > 0, "snapshots on both sides of the 40 ms crossfade ({seen:?})");

    // Paused: a key change re-renders from the paused position, which then plays in the
    // new key; there is nothing to wait for.
    engine.pause();
    wait_for(1.0, "paused", || engine.snapshot().state == PlaybackState::Paused);
    std::thread::sleep(Duration::from_millis(50));
    let paused_at = engine.snapshot().position_s;
    engine.set_params(params(-2));
    wait_for(1.0, "the paused re-render in the new key", || {
        let s = engine.snapshot();
        s.key.heard == -2 && s.key.pending.is_none()
    });
    let s = engine.snapshot();
    assert!((s.position_s - paused_at).abs() < 0.03, "still at the paused position");
    engine.play();
    std::thread::sleep(Duration::from_millis(300));
    let s = engine.snapshot();
    assert_eq!((s.state, s.key.heard, s.key.pending), (PlaybackState::Playing, -2, None));
    println!("seeks: a seek during a scheduled change starts the new segment in the new key; 40 ms crossfade sampled on both sides of the switch {seen:?}; paused change heard at once");
}

#[test]
fn prerender_seek_before_the_new_key_is_rendered_keeps_the_old_key() {
    let _serial = serial();
    let song = tone_song("preseek", 20.0);
    let cache = tmp_dir("cache-preseek");
    let (engine, _cap) = Engine::new_with_capture(RATE, false, 1.0).unwrap();
    engine.set_key_cache(Some(&cache), 1 << 30).unwrap();
    engine.set_key_mode(KeyMode::Prerender);
    engine.load(&song, params(0)).unwrap();
    engine.play();
    wait_for(5.0, "playing", || engine.snapshot().position_s > 0.5);
    // A slow computer: the renderer runs well below real time, so the new key is not
    // ready for a long time (the bypass route at key 0 does no Rubber Band work).
    let rtf = jk_engine::benchmark_realtime_factor(RATE).unwrap();
    engine.debug_set_slowdown(rtf / 0.5);
    engine.set_params(params(4));
    let s = engine.snapshot();
    assert_eq!((s.key.heard, s.key.requested, s.key.pending.map(|p| (p.semitones, p.at_s))), (0, 4, Some((4, None))));
    std::thread::sleep(Duration::from_millis(1000));
    let s = engine.snapshot();
    assert!(s.key.preparing.is_some(), "still rendering");
    assert_eq!((s.key.heard, s.key.pending.map(|p| p.at_s)), (0, Some(None)), "not rendered: the old key plays on");
    let seg_a = s.key.segment;
    engine.seek(10.0);
    wait_for(2.0, "the new segment", || {
        let s = engine.snapshot();
        s.key.segment != seg_a && s.position_s >= 10.0 && engine.key_timeline().last().is_some_and(|x| x.segment == s.key.segment)
    });
    let s = engine.snapshot();
    assert_eq!((s.key.heard, s.key.pending.map(|p| (p.semitones, p.at_s))), (0, Some((4, None))), "after the seek: still the old key, the new one pending");
    // Back to full speed: the key is rendered, the engine crosses over, and the switch is
    // published before it is heard.
    engine.debug_set_slowdown(1.0);
    wait_for(30.0, "the crossover to be scheduled", || engine.snapshot().key.pending.is_some_and(|p| p.at_s.is_some()));
    let s = engine.snapshot();
    let at = s.key.pending.unwrap().at_s.unwrap();
    let ahead_ms = (at - s.position_s) * 1e3;
    assert!(ahead_ms > 0.0 && s.key.heard == 0);
    wait_for(2.0, "the new key heard", || engine.snapshot().key.heard == 4);
    let s = engine.snapshot();
    assert!(s.position_s >= at);
    assert_eq!(engine.key_at(at - 0.01, Some(s.key.segment)), Some(0));
    assert_eq!(engine.key_at(at, Some(s.key.segment)), Some(4));
    println!("prerender seek: the unrendered key stayed pending without a position through a seek; the crossover was published {ahead_ms:.0} ms of song ahead of the DAC");
}

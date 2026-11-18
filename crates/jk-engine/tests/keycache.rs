// SPDX-License-Identifier: GPL-3.0-or-later
//! Key change by render-ahead-and-cache (WORK_ORDER 4.5, last bullet), end to end on the
//! null device: the cached output equals the real-time path bit for bit, seeks work
//! (also into parts not rendered yet), the previous key plays until the new one is ready,
//! a cached key is reused by a later session, and a computer too slow to shift in real
//! time (simulated) plays without a single underrun in prerender mode while the
//! real-time path underruns.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jk_dsp::{render, RenderOptions, StemsRef};
use jk_engine::{Engine, KeyMode, MixParams, PlaybackState};

fn tmp_dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-keycache-tests").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn song(name: &str, seconds: f64) -> PathBuf {
    let p = tmp_dir(&format!("song-{name}")).join(format!("{name}.jksong"));
    jk_engine::demo::make_synth_song(&p, seconds, 44_100, 23).unwrap();
    p
}

/// Offline render (= the real-time path's output, tests/engine.rs), interleaved.
fn reference_render(song: &Path, p: &MixParams) -> Vec<f32> {
    let f = jk_format::SongFile::open(song).unwrap();
    let m = f.manifest().clone();
    let frames = m.duration_frames as usize;
    let mut stems: Vec<[Vec<f32>; 2]> = Vec::new();
    for role in ["lead_vocal", "backing_vocals", "drums", "other"] {
        if m.stem(role).is_some() {
            let planes = jk_format::decode::decode_stem_f32(&f, role).unwrap();
            let r = planes.last().unwrap().clone();
            stems.push([planes[0].clone(), r]);
        } else {
            stems.push([vec![0.0; frames], vec![0.0; frames]]);
        }
    }
    let sref = StemsRef { sample_rate: m.sample_rate, frames, stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]) };
    let out = render(sref, p.to_dsp(), &[], &RenderOptions::native(m.sample_rate)).unwrap();
    (0..frames).flat_map(|i| [out.channels[0][i], out.channels[1][i]]).collect()
}

fn wait_for(secs: f64, what: &str, mut cond: impl FnMut() -> bool) {
    let t = Instant::now();
    while !cond() {
        assert!(t.elapsed().as_secs_f64() < secs, "timed out after {secs} s waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Frame offset of `want` (interleaved) inside `got`, bit-exact.
fn find(got: &[f32], want: &[f32]) -> Option<usize> {
    (0..(got.len() / 2).saturating_sub(want.len() / 2)).find(|&d| got[d * 2] == want[0] && got[d * 2 + 1] == want[1] && got[d * 2..d * 2 + want.len()] == *want)
}

fn prerender_engine(fast: bool, cache: &Path) -> (Engine, jk_engine::output::Capture) {
    let (e, cap) = Engine::new_with_capture(44_100, fast, 1.0).unwrap();
    e.set_key_cache(Some(cache), 1 << 30).unwrap();
    e.set_key_mode(KeyMode::Prerender);
    (e, cap)
}

const FADE: usize = 441; // the filler's 10 ms fade-in at 44.1 kHz

/// The tests in this file run one at a time. Two of them play on the null device in
/// "fast" mode (as fast as the engine can render), where the worker can outrun a cache
/// feed that another test's renderer starves of CPU; that is a miss the real-time device
/// pacing (a 2 s feed ring against 150 ms of render-ahead) does not produce. Run in
/// parallel by `cargo test` they failed in 4 of 5 runs while other builds loaded the
/// CPU (final evidence pass, 2026-11-18); serially they pass.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn cached_output_is_bit_identical_to_the_real_time_path_and_seeks_work() {
    let _serial = serial();
    let song = song("bitident", 20.0);
    let cache = tmp_dir("cache-bitident");
    let p = MixParams { semitones: 3, guide_vocal: 0.4, drums: 0.7, ..MixParams::default() };
    let want = reference_render(&song, &p);
    let n = want.len() / 2;
    let (engine, cap) = prerender_engine(true, &cache);
    engine.load(&song, p).unwrap();
    wait_for(2.0, "the preparing status", || engine.snapshot().key.preparing.is_some());
    assert_eq!(engine.snapshot().key.mode, KeyMode::Prerender);
    let prep = engine.snapshot().key.preparing.expect("a new key is prepared first");
    assert_eq!((prep.semitones, prep.reason.as_str()), (3, "key"));
    wait_for(60.0, "the whole song to be rendered", || engine.diagnostics().key_target_progress == Some(1.0));
    let t0 = Instant::now();
    wait_for(5.0, "the paused song to restart on the cached key", || engine.diagnostics().key_route == 2);
    assert!(engine.snapshot().key.preparing.is_none());
    println!("prerender +3: 20 s song rendered; switched while paused after {:.0} ms", t0.elapsed().as_secs_f64() * 1e3);
    engine.seek(0.0);
    cap.lock().unwrap().clear();
    engine.play();
    wait_for(60.0, "end of song", || engine.snapshot().state == PlaybackState::Ended);
    let got = cap.lock().unwrap().clone();
    let first = got.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
    let first_want = want.chunks(2).position(|f| f[0] != 0.0 || f[1] != 0.0).unwrap();
    let start = first - first_want;
    let g = &got[start * 2..(start + n) * 2];
    let mismatches = (FADE * 2..n * 2).filter(|&i| g[i].to_bits() != want[i].to_bits()).count();
    assert_eq!(mismatches, 0, "prerendered playback differs from the real-time path");
    let d = engine.diagnostics();
    assert_eq!((d.underruns, d.cache_misses, d.audible_semitones, d.key_route), (0, 0, 3, 2));
    println!("prerender +3: {} frames after the fade-in bit-identical to the real-time path (offline render)", n - FADE);

    // Seeks while playing land bit-exactly in the cached audio.
    for pos in [12.34, 3.21, 17.0] {
        engine.seek(pos);
        engine.play();
        let frame = (pos * 44_100.0f64).round() as usize;
        let len = 44_100 / 2;
        let w = &want[(frame + FADE) * 2..(frame + FADE + len) * 2];
        let found = (0..200).find_map(|_| {
            std::thread::sleep(Duration::from_millis(20));
            find(&cap.lock().unwrap(), w)
        });
        assert!(found.is_some(), "seek to {pos} s: cached audio not found bit-exact");
        println!("prerender seek to {pos} s: 0.5 s after the fade-in found bit-exact");
    }
    assert_eq!(engine.diagnostics().underruns, 0);
    drop(engine);

    // A later session finds the finished entry in the cache: the key is there at once.
    let (engine, _cap) = prerender_engine(true, &cache);
    engine.load(&song, p).unwrap();
    assert!(engine.snapshot().key.preparing.is_none(), "cached key reused at load");
    engine.play();
    wait_for(5.0, "cached route", || engine.diagnostics().key_route == 2);
    assert_eq!(engine.diagnostics().key_cells_rendered, 0, "nothing rendered again");
    println!("prerender: second session plays +3 from the cache without rendering");
}

#[test]
fn key_change_while_playing_keeps_the_old_key_until_ready_and_seeks_into_unrendered_parts() {
    let _serial = serial();
    let song = song("live", 40.0);
    let cache = tmp_dir("cache-live");
    let (engine, _cap) = prerender_engine(false, &cache);
    engine.load(&song, MixParams::default()).unwrap();
    engine.play();
    std::thread::sleep(Duration::from_millis(1500));
    let p = MixParams { semitones: -4, ..MixParams::default() };
    let t = Instant::now();
    engine.set_params(p);
    // The original key keeps playing while the new one is rendered.
    wait_for(2.0, "preparing status", || engine.snapshot().key.preparing.is_some());
    assert_eq!(engine.diagnostics().audible_semitones, 0);
    wait_for(20.0, "the new key to be heard", || engine.diagnostics().audible_semitones == -4);
    println!("prerender: key -4 heard {:.2} s after the change (render speed {:.1}x)", t.elapsed().as_secs_f64(), engine.diagnostics().key_render_speed.unwrap_or(0.0));
    assert!(engine.snapshot().key.preparing.is_none());
    assert_eq!(engine.diagnostics().key_route, 2);

    // A mix change of a pitched stem: the old mix (same key) plays until the new one is ready.
    engine.set_params(MixParams { guide_vocal: 0.8, ..p });
    wait_for(2.0, "preparing the mix", || engine.snapshot().key.preparing.as_ref().is_some_and(|x| x.reason == "mix"));
    wait_for(20.0, "the new mix", || engine.snapshot().key.preparing.is_none());
    // The drum level is live (no new render).
    let cells = engine.diagnostics().key_cells_rendered;
    engine.set_params(MixParams { guide_vocal: 0.8, drums: 0.5, ..p });
    std::thread::sleep(Duration::from_millis(100));
    assert!(engine.snapshot().key.preparing.is_none(), "drum level needs no render");

    // Seek back before the point where the key changed: not rendered yet; playback goes
    // on (original key) and switches when the renderer has caught up there.
    let _ = cells;
    engine.seek(0.5);
    wait_for(20.0, "the key after a seek into an unrendered part", || {
        let d = engine.diagnostics();
        d.audible_semitones == -4 && d.key_route == 2 && engine.snapshot().position_s > 0.5
    });
    // Seek far ahead.
    engine.seek(36.0);
    wait_for(20.0, "the key after a seek ahead", || engine.diagnostics().audible_semitones == -4 && engine.snapshot().position_s > 36.0);
    // Back to key 0: immediate (no render needed).
    engine.set_params(MixParams { guide_vocal: 0.8, drums: 0.5, ..MixParams::default() });
    wait_for(2.0, "key 0", || engine.diagnostics().audible_semitones == 0 && engine.diagnostics().key_route == 0);
    let d = engine.diagnostics();
    println!(
        "prerender: seeks done; cells rendered {}, seams {}, interrupted runs {}, underruns {}, cache misses {}",
        d.key_cells_rendered, d.key_seams, d.key_interrupts, d.underruns, d.cache_misses
    );
    assert_eq!(d.underruns, 0);
    assert_eq!(d.cache_misses, 0);
    assert_eq!(d.inline_constructions, 0);

    // Switching to real time mid-song: the graph leaves the cached route by crossfade.
    engine.set_params(MixParams { semitones: 2, ..MixParams::default() });
    wait_for(20.0, "+2 cached", || engine.diagnostics().audible_semitones == 2);
    engine.set_key_mode(KeyMode::Realtime);
    wait_for(3.0, "real-time route", || engine.diagnostics().key_route == 1);
    assert_eq!(engine.snapshot().key.mode, KeyMode::Realtime);
    assert_eq!(engine.diagnostics().audible_semitones, 2);
    assert_eq!(engine.diagnostics().underruns, 0);
}

#[test]
fn a_computer_too_slow_for_real_time_plays_without_underruns_in_prerender_mode() {
    let _serial = serial();
    let song = song("slow", 8.0);
    let rtf = jk_engine::benchmark_realtime_factor(44_100).unwrap();
    // Make Rubber Band run at about 0.75x real time: slower than the song plays.
    let factor = rtf / 0.75;
    let p = MixParams { semitones: 5, ..MixParams::default() };

    // Real-time path: underruns.
    let (rt, _cap) = Engine::new_with_capture(44_100, false, 1.0).unwrap();
    rt.debug_set_slowdown(factor);
    rt.load(&song, p).unwrap();
    rt.play();
    std::thread::sleep(Duration::from_millis(3000));
    let rt_underruns = rt.diagnostics().underruns;
    drop(rt);
    assert!(rt_underruns > 10, "the simulated slow computer should underrun in real time ({rt_underruns})");

    // Prerender: plays at once (original key) while rendering, then the whole song in
    // the new key; not one underrun.
    let cache = tmp_dir("cache-slow");
    let (pre, cap) = prerender_engine(false, &cache);
    pre.debug_set_slowdown(factor);
    pre.load(&song, p).unwrap();
    pre.play();
    wait_for(30.0, "end of the first play", || pre.snapshot().state == PlaybackState::Ended);
    wait_for(60.0, "the render to finish", || pre.diagnostics().key_target_progress == Some(1.0));
    let speed = pre.diagnostics().key_render_speed.unwrap_or(0.0);
    let mark = cap.lock().unwrap().len();
    pre.seek(0.0);
    pre.play();
    wait_for(30.0, "end of the second play", || pre.snapshot().state == PlaybackState::Ended);
    let d = pre.diagnostics();
    assert_eq!(d.underruns, 0, "prerender mode must never underrun");
    assert_eq!(d.cache_misses, 0);
    assert_eq!(d.audible_semitones, 5);
    // And the second play is the real-time path's output, bit for bit.
    let want = reference_render(&song, &p);
    let got = cap.lock().unwrap()[mark..].to_vec();
    let n = want.len() / 2;
    let w = &want[FADE * 2..n * 2];
    assert!(find(&got, w).is_some(), "slow-computer prerender output differs from the real-time path");
    println!(
        "slow computer (Rubber Band {rtf:.1}x slowed {factor:.1}x, renderer measured {speed:.2}x): real-time path {rt_underruns} underruns in 3 s; prerender 0 underruns over two plays, second play bit-identical"
    );
}

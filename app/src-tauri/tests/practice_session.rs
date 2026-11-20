// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice mode in the app core (Work Order 2; docs/APP_API.md section 12) on the REAL
//! audio engine with its silent "null" output device. Every input is generated sound fed
//! through the real capture-side path (`practice_test_input`'s sources -> input rings and
//! meters -> the analysis thread -> `jk_pitch::practice::Chain`); no microphone is opened
//! and nothing is played aloud. Never prints lyrics.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::EngineRenderer;
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::portable::DataLayout;
use jukeoke_lib::practice::guide_view::SessionGuide;
use jukeoke_lib::practice::{rigcheck, synth};
use serde_json::{Value, json};

const SONG: &str = "Jukeoke - Demo.jksong";

/// The tests run one at a time: each plays in real time on its own engine, and timing
/// measurements (the rig check's delay, the live batch rate) are cleaner without four other
/// engines and analysis threads competing for the CPU.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct App {
    root: PathBuf,
    core: Arc<Core>,
    sink: Arc<RecordingSink>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A portable root with the generated demo song (40 s, with its melody guide added) and a
/// core on the null output device.
fn app(name: &str, with_guide: bool) -> App {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("Library")).unwrap();
    let song = root.join("Library").join(SONG);
    jk_engine::demo::make_demo_song(&song).expect("demo song");
    if with_guide {
        jk_guide::add_to_song(&song, &jk_pitch::guide::GuideConfig::default(), &jk_guide::app_stamp("test", "0"), false).expect("guide");
    }
    let exe = if cfg!(target_os = "macos") { root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke") } else { root.join("Windows/Jukeoke.exe") };
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    let engine = jk_engine::Engine::new(jk_engine::EngineConfig { device_id: Some("null:48000".into()), exclusive: false, master: 1.0 })
        .expect("engine on the null device");
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(CoreOptions {
        layout,
        exe: Some(exe),
        name: "Jukeoke".into(),
        version: "0.1.0".into(),
        identifier: "org.jukeoke.app".into(),
        sink: sink.clone(),
        engine: Arc::new(engine),
        renderer: Arc::new(EngineRenderer),
        studio: Err("No Studio in this test.".into()),
        demo_song: None,
        slot_save_delay: Duration::from_millis(200),
        webview_env_ignored: None,
    });
    core.player.start_clock();
    core.guide.start();
    core.practice.allow_test_input(true);
    App { root, core, sink }
}

/// The session guide of the song, as practice reads it.
fn guide_of(path: &Path) -> SessionGuide {
    let song = jk_format::SongFile::open(path).unwrap();
    let (doc, curve) = jk_format::guide::read_guide(&song).unwrap().expect("guide");
    let st = jukeoke_lib::guide::GuideStatus::from_file(&song);
    SessionGuide::new("x", &song, doc, curve, st)
}

/// A generated singer WAV (48 kHz float) for the whole song, `cents` off (None: silent).
fn singer_wav(dir: &Path, name: &str, g: &SessionGuide, cents: Option<f64>, octave: i32, keys: &jk_pitch::score::KeyTimeline) -> PathBuf {
    let x = synth::singer(&g.score, keys, 48_000, cents, octave);
    let p = dir.join(name);
    let mut w = jk_recorder::wav::WavWriter::create(&p, jk_recorder::wav::WavSpec::new(48_000, 1)).unwrap();
    w.write_interleaved(&x).unwrap();
    w.finalize().unwrap();
    p
}

fn wait<T>(secs: f64, what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    wait_for(Duration::from_secs_f64(secs), &mut f).unwrap_or_else(|| panic!("timed out waiting for {what}"))
}

/// Plays from `from_s` for `secs` of song time with practice listening, then "Finish".
fn sing_and_finish(a: &App, from_s: f64, secs: f64) -> Value {
    let c = &a.core;
    c.player.seek(from_s).unwrap();
    c.player.play().unwrap();
    wait(10.0, "playing", || (c.player.state().state == "playing").then_some(()));
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        std::thread::sleep(Duration::from_millis(50));
    }
    let r = c.practice.finish().unwrap();
    r.expect("a graded take")
}

#[test]
fn generated_singers_through_the_real_input_path() {
    let _g = serial();
    let a = app("practice-sing", true);
    let song = a.root.join("Library").join(SONG);
    let g = guide_of(&song);
    let keys = jk_pitch::score::KeyTimeline::constant(0);
    let perfect = singer_wav(&a.root, "perfect.wav", &g, Some(0.0), 0, &keys);
    let silent = singer_wav(&a.root, "silent.wav", &g, None, 0, &keys);
    let c = &a.core;
    c.song_load(SONG).unwrap();

    // The player reports the key heard (APP_API 3, WO2).
    let ps = serde_json::to_value(c.player.state()).unwrap();
    for k in ["key_heard", "key_requested", "key_pending", "key_segment", "key_heard_label", "key_spelling", "key_pending_spelling"] {
        assert!(ps.get(k).is_some(), "PlayerState.{k} missing: {ps}");
    }

    // Perfect singer: marker values arrive, notes light, a high grade, saved in the slot.
    c.practice.test_input(&json!({"kind": "wav", "path": perfect})).unwrap();
    let st = c.practice.start(None, false).unwrap();
    assert_eq!(st.phase, "listening", "{st:?}");
    assert!(st.input.as_ref().unwrap().device.id.starts_with("test:"));
    a.sink.clear();
    let r = sing_and_finish(&a, 0.0, 15.0);
    eprintln!("measured: perfect singer, first 15 s: {} ({}), graded {} of {}, trust {}", r["overall"], r["grade"], r["graded"], r["total"], r["trust"]);
    assert!(r["overall"].as_f64().unwrap() >= 85.0, "{r}");
    assert_eq!(r["saved"], json!(true), "{r}");
    let live = a.sink.of("practice://live");
    let voiced: usize = live.iter().map(|b| b["pitch"].as_array().unwrap().iter().filter(|p| !p[1].is_null()).count()).sum();
    let lit: usize = live.iter().map(|b| b["hits"].as_array().unwrap().iter().filter(|h| h[3] == json!(0)).count()).sum();
    eprintln!("measured: {} live batches, {voiced} voiced values, {lit} lit stretches", live.len());
    assert!(live.len() > 300 && voiced > 500 && lit > 10);
    // Marker heights: the voiced values lie on the guide's notes (within 50 cents) most of the time.
    let mut on = 0;
    let mut all = 0;
    for b in &live {
        for p in b["pitch"].as_array().unwrap() {
            let (t, m) = (p[0].as_f64().unwrap(), p[1].as_f64());
            if let (Some(m), Some(i)) = (m, g.note_at(t)) {
                let n = &g.doc.notes[i];
                if n.is_pitched() && t > n.start + 0.1 {
                    all += 1;
                    if (m - n.midi).abs() * 100.0 < 50.0 {
                        on += 1;
                    }
                }
            }
        }
    }
    eprintln!("measured: marker on the note {on} of {all} values inside notes");
    assert!(on as f64 > 0.8 * all as f64);
    // The history (best + recent) is in the song's slot.
    let h = c.practice.history(SONG).unwrap();
    assert_eq!(h.recent.len(), 1);
    assert_eq!(h.recent[0]["overall"], r["overall"]);
    let slot = jk_format::SongFile::open(&song).unwrap().user_state().practice().unwrap();
    assert_eq!(slot.recent.len(), 1);
    assert!(slot.recent[0].lines.as_ref().is_some_and(|l| !l.rows.is_empty()));
    // The phase is "result"; "Sing again" goes back to listening.
    assert_eq!(c.practice.state().phase, "result");
    c.practice.stop(true).unwrap();

    // Silent singer: no marker, the lowest grade.
    c.practice.test_input(&json!({"kind": "wav", "path": silent})).unwrap();
    c.practice.start(None, false).unwrap();
    a.sink.clear();
    let r = sing_and_finish(&a, 0.0, 12.0);
    eprintln!("measured: silent singer: {} ({})", r["overall"], r["grade"]);
    assert!(r["overall"].as_f64().is_none_or(|v| v <= 1.0), "{r}");
    let voiced: usize =
        a.sink.of("practice://live").iter().map(|b| b["pitch"].as_array().unwrap().iter().filter(|p| !p[1].is_null()).count()).sum();
    assert_eq!(voiced, 0, "silence never shows a marker");
    let stopped = c.practice.stop(true).unwrap();
    assert!(stopped.is_none());
    assert!(!c.practice.state().active);
    // Practice data and results only: nothing else was written next to the song.
    let files: Vec<String> = std::fs::read_dir(a.root.join("Library")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files, vec![SONG.to_string()]);
}

#[test]
fn the_key_heard_moves_the_target_not_the_request() {
    let _g = serial();
    let a = app("practice-key", true);
    let song = a.root.join("Library").join(SONG);
    let g = guide_of(&song);
    let c = &a.core;
    // The real-time key path on purpose: with the default "auto", a busy computer (Rubber
    // Band measured below its real-time threshold) picks render-ahead, where the change has
    // no scheduled position until the crossover (that path is proven in the real app,
    // docs/benchmarks/practice-app.md section 4).
    c.settings_set(&json!({"audio": {"key_mode": "realtime"}})).unwrap();
    c.song_load(SONG).unwrap();
    let wav = singer_wav(&a.root, "steady.wav", &g, Some(0.0), 0, &jk_pitch::score::KeyTimeline::constant(0));
    c.practice.test_input(&json!({"kind": "wav", "path": wav})).unwrap();
    c.practice.start(None, false).unwrap();
    c.player.play().unwrap();
    // Past the first notes (the echo stage needs about 3 s of music before it trusts the
    // input; notes in that time are not graded), so notes in key 0 are graded first.
    wait(15.0, "the song to reach 8 s", || (c.player.state().position_s > 8.0).then_some(()));
    // A key change asked now is heard later (about 0.28 s): the target follows the heard key.
    let asked_at = c.player.state().position_s;
    c.player.set(&json!({"semitones": -3})).unwrap();
    let pending = wait(2.0, "the switch to be scheduled", || c.player.state().key_pending.and_then(|p| p.at_s));
    eprintln!("measured: key -3 asked at song time {asked_at:.3} s, heard from {pending:.3} s ({:.0} ms later)", (pending - asked_at) * 1000.0);
    assert!(pending > asked_at + 0.1, "heard {pending} asked {asked_at}");
    wait(3.0, "the key to be heard", || (c.player.state().key_heard == -3).then_some(()));
    let before = c.practice.target_at(pending - 0.05, None).unwrap();
    let after = c.practice.target_at(pending + 0.05, None).unwrap();
    eprintln!("measured: key at {:.3} s = {:?}, at {:.3} s = {:?}", pending - 0.05, before.key, pending + 0.05, after.key);
    assert_eq!(before.key, Some(0));
    assert_eq!(after.key, Some(-3));
    // A note's name in the new key: every pitched note's target is 3 semitones lower.
    if let (Some(i), Some(m)) = (after.note, after.midi) {
        assert!((m - (g.doc.notes[i].midi - 3.0)).abs() < 1e-6);
    }
    std::thread::sleep(Duration::from_millis(3000));
    let r = c.practice.finish().unwrap().expect("graded");
    // Notes sung before the change kept key 0; the singer stayed in the old key, so the notes
    // after it are off by 3 semitones.
    let done: Vec<Value> = a.sink.of("practice://live").iter().flat_map(|b| b["done"].as_array().cloned().unwrap_or_default()).collect();
    let k0 = done.iter().filter(|d| d[2] == json!(0) && d[1].as_f64().is_some_and(|c| c > 0.8)).count();
    let k3 = done.iter().filter(|d| d[2] == json!(-3)).count();
    eprintln!("measured: notes done [note, credit, key, mean error]: {done:?}");
    eprintln!("measured: notes finished in key 0 with high credit {k0}, notes finished in key -3 {k3}; take {} key changes {}", r["overall"], r["key_changes"]);
    assert!(k0 > 0 && k3 > 0, "{done:?}");
    assert!(r["key_changes"].as_u64().unwrap() >= 1);
    c.practice.stop(true).unwrap();
}

#[test]
fn missing_things_explain_themselves() {
    let _g = serial();
    let a = app("practice-missing", false);
    let c = &a.core;
    c.song_load(SONG).unwrap();
    // No guide, no Studio: built on the spot ("preparing"), then practice goes on.
    c.practice.test_input(&json!({"kind": "no_device"})).unwrap();
    let st = c.practice.start(None, false).unwrap();
    assert_eq!(st.phase, "preparing", "{st:?}");
    let st = wait(30.0, "the guide to be made", || {
        let s = c.practice.state();
        (s.phase != "preparing").then_some(s)
    });
    // No input device: watch only, in plain words.
    assert_eq!(st.phase, "watch_only", "{st:?}");
    let w = st.watch_only.unwrap();
    assert_eq!(w.code, "no_device");
    eprintln!("measured: no input -> {}", w.message);
    assert!(w.message.contains("No microphone"));
    assert!(st.guide.state == "ready" || st.guide.in_memory, "{:?}", st.guide);
    let g = c.practice.guide_view(None).unwrap();
    assert!(!g.notes.is_empty());
    c.practice.stop(true).unwrap();

    // Permission refused (simulated): watch only with the exact steps.
    c.practice.test_input(&Value::Null).unwrap();
    c.practice.test_input(&json!({"kind": "permission_denied"})).unwrap();
    let st = c.practice.start(None, false).unwrap();
    let w = st.watch_only.clone().unwrap();
    eprintln!("measured: permission refused -> {}", w.message);
    assert_eq!((st.phase, w.code), ("watch_only", "permission_denied"));
    assert!(w.message.contains("Privacy"));
    assert_eq!(c.practice.mic_permission().state, "denied");
    c.practice.stop(true).unwrap();

    // The input unplugged mid-song: watch only with input_lost, the take goes on and is
    // flagged; nothing crashes.
    c.practice.test_input(&Value::Null).unwrap();
    let song = a.root.join("Library").join(SONG);
    let g = guide_of(&song);
    let wav = singer_wav(&a.root, "perfect.wav", &g, Some(0.0), 0, &jk_pitch::score::KeyTimeline::constant(0));
    c.practice.test_input(&json!({"kind": "wav", "path": wav, "unplug_after_s": 4.0})).unwrap();
    let st = c.practice.start(None, false).unwrap();
    assert_eq!(st.phase, "listening");
    c.player.play().unwrap();
    let st = wait(10.0, "the input to be lost", || {
        let s = c.practice.state();
        (s.phase == "watch_only").then_some(s)
    });
    let w = st.watch_only.clone().unwrap();
    eprintln!("measured: unplugged -> {} / input state {:?}", w.message, st.input.as_ref().map(|i| i.state));
    assert_eq!(w.code, "input_lost");
    assert_eq!(st.input.as_ref().unwrap().state, "unplugged");
    std::thread::sleep(Duration::from_millis(1500));
    let r = c.practice.finish().unwrap().expect("graded");
    assert!(r["trust"].as_array().unwrap().iter().any(|t| t == "input_lost"), "{r}");
    c.practice.stop(true).unwrap();

    // A song with no lead vocal: not ready, says why, no retry.
    let nolead = a.root.join("Library").join("No lead.jksong");
    write_song_without_lead(&nolead);
    c.song_load("No lead.jksong").unwrap();
    let st = c.practice.start(None, false).unwrap();
    let nr = st.not_ready.clone().unwrap();
    eprintln!("measured: no lead vocal -> {}", nr.message);
    assert_eq!((st.phase, nr.code.as_str(), nr.can_retry), ("not_ready", "guide_no_lead", false));
    c.practice.stop(true).unwrap();
}

#[test]
fn the_mixer_returning_the_music_is_found_measured_and_removed() {
    let _g = serial();
    let a = app("practice-return", true);
    let song = a.root.join("Library").join(SONG);
    let g = guide_of(&song);
    let c = &a.core;
    c.song_load(SONG).unwrap();
    // The guide vocal on, as a mixer would return it at full strength (WO2 trap 6: "the
    // mixer can sing").
    c.player.set(&json!({"guide_vocal": 1.0})).unwrap();

    // 1. Nobody singing, the music coming back 23 ms late at -6 dB: the warning, the delay,
    //    and the marker stays off; the grade is the lowest.
    c.practice.test_input(&json!({"kind": "return", "delay_ms": 23.0, "level_db": -6.0})).unwrap();
    c.practice.start(None, false).unwrap();
    a.sink.clear();
    c.player.play().unwrap();
    let t0 = Instant::now();
    let st = wait(15.0, "the return to be found", || {
        let s = c.practice.state();
        (s.music.presence == "present").then_some(s)
    });
    eprintln!(
        "measured: returned music (nobody singing) found after {:.1} s: delay {:?} ms, level {:?} dB, removal {}, message: {}",
        t0.elapsed().as_secs_f64(),
        st.music.delay_ms,
        st.music.level_db,
        st.music.removal,
        st.music.message.clone().unwrap_or_default()
    );
    assert!(st.music.message.as_deref().is_some_and(|m| m.contains("loopback")));
    let d = st.music.delay_ms.unwrap();
    assert!((d - 23.0).abs() < 2.0, "measured {d} ms for a 23 ms return");
    let stable = wait_for(Duration::from_secs(15), || {
        let s = c.practice.state();
        (s.latency.source == "returned_music").then_some(s)
    });
    let st = c.practice.state();
    eprintln!(
        "measured: after {:.1} s: stability {}, removal {}, suppression {:?} dB, latency {:?} (source returned_music reached: {})",
        t0.elapsed().as_secs_f64(),
        st.music.stability,
        st.music.removal,
        st.music.suppression_db,
        st.latency,
        stable.is_some()
    );
    std::thread::sleep(Duration::from_millis(2000));
    let r = c.practice.finish().unwrap().expect("graded");
    let voiced: usize =
        a.sink.of("practice://live").iter().map(|b| b["pitch"].as_array().unwrap().iter().filter(|p| !p[1].is_null()).count()).sum();
    eprintln!("measured: nobody singing with the music returned: {} ({}), trust {}, marker values shown {voiced}", r["overall"], r["grade"], r["trust"]);
    assert_eq!(voiced, 0, "the returned music never moves the marker");
    assert!(r["overall"].as_f64().is_none_or(|v| v <= 1.0), "{r}");
    assert!(r["trust"].as_array().unwrap().iter().any(|t| t == "music_in_input"), "{r}");
    c.practice.stop(true).unwrap();

    // 2. The same with a generated singer on top (reported: how removal copes).
    let wav = singer_wav(&a.root, "perfect.wav", &g, Some(0.0), 0, &jk_pitch::score::KeyTimeline::constant(0));
    c.practice.test_input(&json!({"kind": "return", "delay_ms": 23.0, "level_db": -6.0, "voice": wav})).unwrap();
    c.practice.start(None, false).unwrap();
    let r = sing_and_finish(&a, 0.0, 15.0);
    let st = c.practice.state();
    eprintln!(
        "measured: perfect singer with the music returned: {} ({}), graded {} of {}, trust {}; music {} / {} / {}",
        r["overall"], r["grade"], r["graded"], r["total"], r["trust"], st.music.presence, st.music.stability, st.music.removal
    );
    assert!(r["trust"].as_array().unwrap().iter().any(|t| t == "music_in_input"), "{r}");
    c.practice.stop(true).unwrap();
}

#[test]
fn the_rig_check_needs_confirmation_and_finds_a_return_on_the_null_device() {
    let _g = serial();
    let a = app("practice-rig", true);
    let c = &a.core;
    c.song_load(SONG).unwrap();
    // Never without the owner's confirmation (it makes a sound on a real device).
    let e = c.practice.rig_check(&rigcheck::RigCheckOptions::default()).unwrap_err();
    assert_eq!(e.code, "audible_not_confirmed");
    // The logic, on the silent null device, with a generated mixer return 17 ms late.
    c.practice.test_input(&json!({"kind": "return", "delay_ms": 17.0, "level_db": -10.0})).unwrap();
    let r = c
        .practice
        .rig_check(&rigcheck::RigCheckOptions { audible_confirmed: true, voice_step: Some(false), label: Some("loopback on (test)".into()), ..Default::default() })
        .unwrap();
    eprintln!("measured: rig check {:?}", r.returned);
    for f in &r.findings {
        eprintln!("  finding: {f}");
    }
    assert!(r.returned.detected, "{r:?}");
    assert!((r.returned.delay_ms.unwrap() - 17.0).abs() < 1.0, "{:?}", r.returned);
    assert!((r.returned.level_db.unwrap() + 10.0).abs() < 1.5, "{:?}", r.returned);
    let text = std::fs::read_to_string(&r.report_path).unwrap();
    assert!(text.contains("AUDIBLE CHECK") && text.contains("loopback on (test)"));
    assert!(Path::new(&r.report_path).starts_with(a.root.join("Settings")));
    // The measured delay is kept for this computer, input and output.
    let s = c.settings.get();
    assert!(s.practice.measured_latency_ms.values().any(|m| m.how == "rig_check" && (m.ms - 17.0).abs() < 1.0), "{:?}", s.practice.measured_latency_ms);
    // Digital silence in: nothing came back; the second run appends to the same report.
    c.practice.test_input(&json!({"kind": "silence"})).unwrap();
    let r2 = c.practice.rig_check(&rigcheck::RigCheckOptions { audible_confirmed: true, voice_step: Some(false), label: Some("loopback off (test)".into()), ..Default::default() }).unwrap();
    assert!(!r2.returned.detected);
    assert_eq!(r2.report_path, r.report_path);
    let text = std::fs::read_to_string(&r.report_path).unwrap();
    assert!(text.contains("loopback off (test)") && text.contains("loopback on (test)"));
    // The song is back, paused where it was.
    assert_eq!(c.player.current().map(|c| c.song_id), Some(SONG.to_string()));
}

#[test]
fn playback_is_bit_identical_with_practice_on_and_off() {
    let _g = serial();
    // The engine on a capturing null device (every played buffer kept), the whole demo song
    // played through the app core once without practice and once with practice listening
    // (its analysis thread reading the output tap, a generated singer in): the device output
    // must be the same to the bit (WO2 section 5).
    let render = |practice: bool| -> (Vec<f32>, usize) {
        let root = scratch(if practice { "bitid-on" } else { "bitid-off" });
        std::fs::create_dir_all(root.join("Library")).unwrap();
        let song = root.join("Library").join(SONG);
        jk_engine::demo::make_demo_song(&song).unwrap();
        jk_guide::add_to_song(&song, &jk_pitch::guide::GuideConfig::default(), &jk_guide::app_stamp("test", "0"), false).unwrap();
        let exe = root.join("Windows/Jukeoke.exe");
        let layout = DataLayout::detect(&exe, None, || None).unwrap();
        let (engine, cap) = jk_engine::Engine::new_with_capture(48_000, false, 1.0).unwrap();
        let core = Core::new(CoreOptions {
            layout,
            exe: Some(exe),
            name: "Jukeoke".into(),
            version: "0.1.0".into(),
            identifier: "org.jukeoke.app".into(),
            sink: Arc::new(RecordingSink::default()),
            engine: Arc::new(engine),
            renderer: Arc::new(EngineRenderer),
            studio: Err("No Studio in this test.".into()),
            demo_song: None,
            slot_save_delay: Duration::from_millis(200),
            webview_env_ignored: None,
        });
        core.player.start_clock();
        core.practice.allow_test_input(true);
        core.song_load(SONG).unwrap();
        let mut live = 0;
        if practice {
            let g = guide_of(&song);
            let wav = singer_wav(&root, "perfect.wav", &g, Some(0.0), 0, &jk_pitch::score::KeyTimeline::constant(0));
            core.practice.test_input(&json!({"kind": "wav", "path": wav})).unwrap();
            assert_eq!(core.practice.start(None, false).unwrap().phase, "listening");
        }
        core.player.play().unwrap();
        wait(60.0, "the end of the song", || (core.player.state().state == "ended").then_some(()));
        if practice {
            live = core.practice.state().take.map_or(0, |t| t.graded as usize);
            core.practice.stop(true).unwrap();
        }
        core.shutdown();
        let mut v = cap.lock().unwrap().clone();
        // Leading and trailing device silence is not part of the song.
        let first = v.iter().position(|x| *x != 0.0).unwrap_or(v.len()) & !1;
        v.drain(..first);
        let last = v.iter().rposition(|x| *x != 0.0).map_or(0, |i| (i | 1) + 1);
        v.truncate(last);
        drop(core);
        let _ = std::fs::remove_dir_all(root);
        (v, live)
    };
    let (off, _) = render(false);
    let (on, graded) = render(true);
    let diff = off.iter().zip(&on).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
    eprintln!(
        "measured: device output {} frames without practice, {} with practice listening ({graded} notes graded during it); {diff} samples differ",
        off.len() / 2,
        on.len() / 2
    );
    assert!(off.len() > 38 * 48_000 * 2);
    assert_eq!(off.len(), on.len());
    assert_eq!(diff, 0, "playback changed with practice on");
}

#[test]
fn choices_exclusions_and_offsets_are_kept_where_they_belong() {
    // APP_API 12.4-12.7: the per-song choices (octave, strictness, "the guide is wrong
    // here") go into the song's slot and pass the format's own check; the input choice and
    // the manual offset belong to this computer (settings.json, keyed by its hash); the
    // test hook is refused outside the test modes. No sound is played or captured here.
    let _g = serial();
    let a = app("practice-choices", true);
    let c = &a.core;
    c.song_load(SONG).unwrap();
    let song = a.root.join("Library").join(SONG);
    let slot = || -> Value {
        let f = jk_format::SongFile::open(&song).unwrap();
        serde_json::to_value(f.user_state().practice().expect("practice data in the slot")).unwrap()
    };

    // Octave and strictness for this song.
    c.practice.song_set(SONG, &json!({"octave": {"mode": "fixed", "shift": -1}, "strictness": "strict"})).unwrap();
    let s = slot();
    assert_eq!((s["octave"]["mode"].as_str(), s["octave"]["shift"].as_i64(), s["strictness"].as_str()), (Some("fixed"), Some(-1), Some("strict")));
    assert!(c.practice.song_set(SONG, &json!({"octave": {"mode": "sideways"}})).is_err());
    assert!(c.practice.song_set(SONG, &json!({"volume": 3})).is_err());
    // Strict octave mode forces the written octave.
    c.practice.song_set(SONG, &json!({"octave": {"mode": "strict", "shift": 2}, "strictness": null})).unwrap();
    let s = slot();
    assert_eq!((s["octave"]["mode"].as_str(), s["octave"]["shift"].as_i64(), s["strictness"].is_null()), (Some("strict"), Some(0), true));

    // "The guide is wrong here" on a line: saved, the lane's guide stops grading it, undone.
    let span = jukeoke_lib::practice::ExcludedSpan { line: Some(1), start: 5.0, end: 9.0 };
    let list = c.practice.exclude(SONG, &span, true).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(slot()["excluded"].as_array().map(Vec::len), Some(1));
    let gv = serde_json::to_value(c.practice.guide_view(Some(SONG)).unwrap()).unwrap();
    let notes = gv["notes"].as_array().unwrap();
    let inside: Vec<&Value> = notes
        .iter()
        .filter(|n| {
            let mid = 0.5 * (n["start"].as_f64().unwrap() + n["end"].as_f64().unwrap());
            (5.0..=9.0).contains(&mid)
        })
        .collect();
    assert!(!inside.is_empty() && inside.iter().all(|n| n["graded"] == json!(false)), "excluded notes still graded");
    assert!(notes.iter().any(|n| n["graded"] == json!(true)), "every note stopped counting");
    assert!(c.practice.exclude(SONG, &span, false).unwrap().is_empty());
    assert!(c.practice.exclude(SONG, &jukeoke_lib::practice::ExcludedSpan { line: None, start: 9.0, end: 5.0 }, true).is_err());
    // The slot keeps to SONG_FORMAT 11's types.
    assert!(jk_format::practice::validate_practice(&slot()).is_empty(), "{:?}", jk_format::practice::validate_practice(&slot()));

    // This computer's input and manual offset (settings.json, keyed by the computer).
    let me = jukeoke_lib::keymode::machine_id();
    let choice = jukeoke_lib::settings::InputChoice { device_id: Some("dev-test".into()), device_name: Some("Mixer (test)".into()), channel: Some(1) };
    c.practice.mic_choose(choice.clone()).unwrap();
    assert_eq!(c.settings.get().practice.inputs.get(&me), Some(&choice));
    assert_eq!(c.practice.chosen_input(), choice);
    c.practice.mic_choose(jukeoke_lib::settings::InputChoice::default()).unwrap();
    assert!(c.settings.get().practice.inputs.get(&me).is_none(), "the automatic choice keeps nothing");
    assert_eq!(c.practice.latency_set(37.4, Some("dev-test".into())).unwrap(), 37.0);
    assert_eq!(c.settings.get().practice.latency_offsets_ms.get(&format!("{me}|dev-test")), Some(&37.0));
    assert_eq!(c.practice.latency_set(900.0, Some("dev-test".into())).unwrap(), 500.0);
    assert_eq!(c.practice.latency_set(0.0, Some("dev-test".into())).unwrap(), 0.0);
    assert!(c.settings.get().practice.latency_offsets_ms.get(&format!("{me}|dev-test")).is_none());
    assert!(c.practice.latency_set(f64::NAN, None).is_err());
    // Nothing per computer went into the song, nothing per song into settings.json.
    let s = slot();
    assert!(s.get("inputs").is_none() && s.get("latency_offsets_ms").is_none());

    // The test hook exists only in the test modes (APP_API 12.1).
    c.practice.allow_test_input(false);
    let e = c.practice.test_input(&json!({"kind": "silence"})).unwrap_err();
    assert_eq!(e.code, "bad_request");
    // The scorer's target needs practice on.
    assert_eq!(c.practice.target_at(1.0, None).unwrap_err().code, "bad_request");
    eprintln!("measured: slot after the choices: {}", serde_json::to_string(&slot()).unwrap());
}

/// A song whose stems have no lead vocal (a recording made before separation, say).
fn write_song_without_lead(path: &Path) {
    let rate = 48_000u32;
    let frames = rate as usize * 4;
    let tone: Vec<i32> = (0..frames * 2).map(|i| (((i / 2) as f64 * 0.05).sin() * 100_000.0) as i32).collect();
    let roles = ["backing_vocals", "drums", "other"];
    let stems: Vec<Value> = roles
        .iter()
        .map(|r| json!({"role": r, "chunk": format!("stem.{r}"), "codec": "flac", "bits": 24, "gain": 1.0, "peak": 0.01, "in_karaoke_mix": true, "pitched": *r != "drums"}))
        .collect();
    let manifest: jk_format::Manifest = serde_json::from_value(json!({
        "format": "jksong", "format_version": jk_format::FORMAT_VERSION, "title": "No lead", "artist": "Test", "album": null,
        "sample_rate": rate, "channels": 2, "duration_frames": frames, "duration_seconds": frames as f64 / f64::from(rate),
        "cover": null, "lyrics": null, "analysis": null, "preset": "full", "stems_sum_to_source": false, "stems": stems,
        "provenance": {"app": {"name": "test", "version": "0"}, "created_utc": "2026-11-19T00:00:00Z", "resampling": [], "models": [],
                       "source": {"filename": "generated", "lossy": false}}
    }))
    .unwrap();
    let mut w = jk_format::SongWriter::new(manifest);
    for r in roles {
        w.add_chunk(format!("stem.{r}"), "audio/flac", jk_format::ChunkData::Bytes(jk_engine::encode_flac(&tone, 2, 24, rate))).unwrap();
    }
    w.write_atomic(path).unwrap();
}

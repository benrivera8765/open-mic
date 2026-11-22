// SPDX-License-Identifier: GPL-3.0-or-later
//! The app core on the real audio engine (crates/jk-engine) with its "null" output
//! device: plays in real time, makes no sound. Checks the wiring the fakes cannot:
//! load -> play -> playback://clock from the engine's DAC clock, a live key change,
//! the per-song settings slot, and an offline export through `render_offline`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::EngineRenderer;
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::portable::DataLayout;
use jukeoke_lib::testsong::{self, ToneSong};
use serde_json::json;

fn core_on_null_device(name: &str) -> (PathBuf, Arc<Core>, Arc<RecordingSink>) {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("Library")).unwrap();
    let exe = if cfg!(target_os = "macos") {
        root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke")
    } else {
        root.join("Windows/Jukeoke.exe")
    };
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    testsong::write(&root.join("Library/Jukeoke - Test Tone.jksong"), ToneSong { seconds: 6.0, ..Default::default() })
        .unwrap();
    let engine = jk_engine::Engine::new(jk_engine::EngineConfig {
        device_id: Some("null:48000".into()),
        exclusive: false,
        master: 1.0,
    })
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
    (root, core, sink)
}

#[test]
fn plays_on_the_null_device_with_a_moving_clock() {
    let (root, core, sink) = core_on_null_device("real-play");
    core.player.start_clock();
    let d = core.player.load("Jukeoke - Test Tone.jksong").unwrap();
    assert_eq!((d.sample_rate, d.channels, d.stems.len()), (48000, 2, 4));
    let st = core.player.state();
    assert_eq!(st.state, "paused", "{st:?}");
    let dev = st.device.expect("null device open");
    eprintln!("measured: device {} {} Hz, latency {:.1} ms", dev.name, dev.sample_rate, dev.latency_ms);

    core.player.play().unwrap();
    sink.clear();
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(1500));
    let wall = t0.elapsed().as_secs_f64();
    let st = core.player.state();
    assert_eq!(st.state, "playing", "{st:?}");
    let clock = sink.of("playback://clock");
    let first = clock.first().unwrap()["position_s"].as_f64().unwrap();
    let last = clock.last().unwrap()["position_s"].as_f64().unwrap();
    let rate = clock.len() as f64 / wall;
    eprintln!(
        "measured: {} clock events in {wall:.3} s ({rate:.1}/s); song time advanced {:.3} s; underruns {}",
        clock.len(),
        last - first,
        st.underruns
    );
    assert!((25.0..=36.0).contains(&rate), "{rate} events/s");
    assert!((last - first - wall).abs() < 0.2, "clock advanced {} in {wall}", last - first);
    assert_eq!(st.underruns, 0);

    // A live key change: the label follows, the slot gets it about 200 ms later.
    core.player.set(&json!({"semitones": 2})).unwrap();
    assert_eq!(core.player.state().key_label.as_deref(), Some("A major → B major (+2)"));
    std::thread::sleep(Duration::from_millis(600));
    let saved = jk_format::SongFile::open(root.join("Library/Jukeoke - Test Tone.jksong")).unwrap();
    assert_eq!(saved.effective_settings().semitones, 2);
    assert_eq!(core.player.state().state, "playing", "the slot write must not stop playback");

    core.player.pause().unwrap();
    assert_eq!(core.player.state().state, "paused");
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn export_renders_the_current_key_to_a_float_wav() {
    let (root, core, sink) = core_on_null_device("real-export");
    core.player.load("Jukeoke - Test Tone.jksong").unwrap();
    core.player.set(&json!({"semitones": -5})).unwrap();
    let t0 = Instant::now();
    let job = core.export_backing("Jukeoke - Test Tone.jksong", "wav32f").unwrap();
    let done = wait_for(Duration::from_secs(60), || {
        sink.of("export://done").into_iter().find(|e| e["job_id"] == job.job_id.as_str())
    });
    let failed = sink.of("export://failed");
    let done = done.unwrap_or_else(|| panic!("export did not finish: {failed:?}"));
    let path = PathBuf::from(done["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "Open Mic - Test Tone (E major, -5).wav");
    let h = jukeoke_lib::recorder::read_wav_header(&path).unwrap();
    assert_eq!((h.sample_rate, h.channels, h.bits), (48000, 2, 32));
    let frames = h.data_bytes / 8;
    eprintln!(
        "measured: export of 6.0 s took {:.2} s; {frames} frames; peak {}",
        t0.elapsed().as_secs_f64(),
        done["peak"]
    );
    assert_eq!(frames, 288_000, "same length as the song");
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

/// The owner's request of 18 Nov 2026 (D-230): deleting the song that is playing in the
/// real engine stops it, writes its pending settings, closes every handle the engine's
/// reader and render threads had on the file, then moves it into Deleted/. On Windows
/// the moved file can be opened with no sharing at all at once: nobody holds it.
#[test]
fn deleting_the_playing_song_releases_every_file_handle_first() {
    let (root, core, _sink) = core_on_null_device("real-delete");
    core.player.start_clock();
    let id = "Jukeoke - Test Tone.jksong";
    core.player.load(id).unwrap();
    core.player.play().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(core.player.state().state, "playing");
    core.player.set(&json!({"semitones": -2})).unwrap();
    let t0 = Instant::now();
    let r = core.library_delete(&[id.to_string()]).unwrap();
    let took = t0.elapsed();
    eprintln!("measured: delete of the playing song took {:.0} ms", took.as_secs_f64() * 1000.0);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.unloaded.as_deref(), Some(id));
    assert!(took < Duration::from_millis(1900), "waited for the file to close ({took:?})");
    let st = core.player.state();
    assert_eq!((st.state, st.song_id), ("empty", None));
    let moved = root.join("Deleted").join(id);
    assert!(!root.join("Library").join(id).exists());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let exclusive = std::fs::OpenOptions::new().read(true).share_mode(0).open(&moved);
        assert!(exclusive.is_ok(), "the moved file is still open somewhere: {exclusive:?}");
    }
    assert_eq!(jk_format::SongFile::open(&moved).unwrap().effective_settings().semitones, -2);
    // Restored, it plays again.
    let back = core.library_restore(&[id.to_string()]).unwrap();
    assert_eq!(back.restored[0].song_id, id);
    let d = core.player.load(id).unwrap();
    assert_eq!(d.settings.semitones, -2);
    core.player.play().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(core.player.state().state, "playing");
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

/// Work Order 2 (APP_API 12.3): a guide built for the song that is playing in the real
/// engine never interrupts it: the guide is used from memory and the swap waits; once
/// paused, the song is released (every handle closed, as for a delete), the guide is
/// swapped in, and the song is loaded again at the same position and plays on.
#[test]
fn a_guide_for_the_playing_song_waits_for_a_pause_then_swaps() {
    let (root, core, _sink) = core_on_null_device("real-guide");
    std::fs::copy(tiny_song(), root.join("Library/Tiny.jksong")).unwrap();
    core.player.start_clock();
    core.guide.start();
    let id = "Tiny.jksong";
    let path = root.join("Library").join(id);
    core.song_load(id).unwrap();
    core.player.play().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let under0 = core.player.state().underruns;
    core.guide_build(id, false).unwrap();
    let s = wait_for(Duration::from_secs(60), || Some(core.guide_status(id).unwrap()).filter(|s| s.pending_write))
        .expect("the guide waits in memory while the song plays");
    assert!(s.in_memory && s.state == "ready", "{s:?}");
    let st = core.player.state();
    assert_eq!(st.state, "playing");
    assert_eq!(st.underruns, under0, "no underrun while the guide was built and written beside playback");
    assert!(jk_format::SongFile::open(&path).unwrap().chunk("guide").is_none());
    core.player.pause().unwrap();
    let pos = core.player.state().position_s;
    let done = wait_for(Duration::from_secs(10), || Some(core.guide_status(id).unwrap()).filter(|s| !s.pending_write));
    assert!(done.as_ref().is_some_and(|s| s.state == "ready" && !s.in_memory), "{done:?} now {:?}", core.guide_status(id));
    assert!(jk_format::SongFile::open(&path).unwrap().chunk("guide").is_some());
    let st = core.player.state();
    assert_eq!((st.state, st.song_id.as_deref()), ("paused", Some(id)));
    assert!((st.position_s - pos).abs() < 0.1, "reloaded at {} (was {pos})", st.position_s);
    core.player.play().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(core.player.state().state, "playing");
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

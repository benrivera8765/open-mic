// SPDX-License-Identifier: GPL-3.0-or-later
//! The command handlers' logic (src/core.rs and the modules it owns) without a
//! window or an audio device: a portable layout in a temporary folder, generated
//! song files, the fake engine and renderer (src/engine.rs `fake`), and a recording
//! event sink. Covers docs/APP_API.md sections 1-7.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::fake::{Call, FakeEngine, FakeRenderer};
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::portable::{DataLayout, DataMode};
use jukeoke_lib::testsong::{self, ToneSong};
use serde_json::{Value, json};

struct App {
    root: PathBuf,
    core: Arc<Core>,
    engine: Arc<FakeEngine>,
    renderer: Arc<FakeRenderer>,
    sink: Arc<RecordingSink>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn app(name: &str, renderer: FakeRenderer) -> App {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("Library/Pop")).unwrap();
    std::fs::create_dir_all(root.join("Windows")).unwrap();
    let exe = if cfg!(target_os = "macos") {
        root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke")
    } else {
        root.join("Windows/Jukeoke.exe")
    };
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    assert_eq!(layout.mode, DataMode::Portable);
    let lib = root.join("Library");
    let short = ToneSong { seconds: 3.0, ..Default::default() };
    testsong::write(&lib.join("Jukeoke - Test Tone.jksong"), short).unwrap();
    testsong::write(&lib.join("Pop/Second.jksong"), short).unwrap();
    std::fs::copy(tiny_song(), lib.join("Tiny.jksong")).unwrap();
    // Things a scan must skip or flag.
    std::fs::write(lib.join("._Jukeoke - Test Tone.jksong"), b"AppleDouble junk").unwrap();
    std::fs::write(lib.join("Half.jksong.tmp-0123abcd"), b"unfinished").unwrap();
    std::fs::write(lib.join("Broken - Song.jksong"), b"this is not a song file at all").unwrap();

    let engine = Arc::new(FakeEngine::default());
    let renderer = Arc::new(renderer);
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(CoreOptions {
        layout,
        exe: Some(exe),
        name: "Jukeoke".into(),
        version: "0.1.0".into(),
        identifier: "org.jukeoke.app".into(),
        sink: sink.clone(),
        engine: engine.clone(),
        renderer: renderer.clone(),
        studio: Err("No Studio in this test.".into()),
        demo_song: None,
        slot_save_delay: Duration::from_millis(300),
        webview_env_ignored: None,
    });
    App { root, core, engine, renderer, sink }
}

fn saved(path: &Path) -> jk_format::UserState {
    jk_format::SongFile::open(path).unwrap().user_state().clone()
}

#[test]
fn library_scan_uses_effective_metadata_and_skips_junk() {
    let a = app("library", FakeRenderer::default());
    let list = a.core.library.list().unwrap();
    let ids: Vec<&str> = list.songs.iter().map(|s| s.song_id.as_str()).collect();
    assert_eq!(ids, ["Broken - Song.jksong", "Jukeoke - Test Tone.jksong", "Pop/Second.jksong", "Tiny.jksong"]);
    let broken = &list.songs[0];
    assert_eq!((broken.artist.as_str(), broken.title.as_str()), ("Broken", "Song"));
    assert!(broken.error.is_some(), "{broken:?}");
    let tone = &list.songs[1];
    assert_eq!(tone.title, "Test Tone");
    assert_eq!(tone.key.as_ref().unwrap().name, "A major");
    assert!(tone.has_lyrics && tone.lyrics_checked && tone.error.is_none());
    assert!((tone.duration_s - 3.0).abs() < 1e-9);
    assert!(list.total_bytes > 0 && list.free_bytes <= list.total_bytes);

    // JSON shape of SongSummary (APP_API section 2).
    let v = serde_json::to_value(tone).unwrap();
    for k in ["song_id", "title", "artist", "album", "duration_s", "key", "tempo_bpm", "has_lyrics",
              "lyrics_checked", "semitones", "file_bytes", "modified_ms", "preset", "error"] {
        assert!(v.get(k).is_some(), "SongSummary.{k} missing");
    }
    assert_eq!(v["key"], json!({"tonic": "A", "mode": "major", "name": "A major"}));

    // The override wins, and the cached summary notices the change.
    a.core.metadata_save("Pop/Second.jksong", &json!({"title": "Corrected", "album": null})).unwrap();
    let list = a.core.library.list().unwrap();
    let second = list.songs.iter().find(|s| s.song_id == "Pop/Second.jksong").unwrap();
    assert_eq!(second.title, "Corrected");
    assert_eq!(second.album, None);
    assert_eq!(a.core.metadata_save("Pop/Second.jksong", &json!({"titel": "x"})).unwrap_err().code, "bad_request");
}

#[test]
fn covers_are_thumbnails_or_null() {
    let a = app("cover", FakeRenderer::default());
    let c = a.core.library.cover("Tiny.jksong", 64).unwrap();
    assert!(c.data_url.unwrap().starts_with("data:image/"));
    assert_eq!(a.core.library.cover("Jukeoke - Test Tone.jksong", 64).unwrap().data_url, None);
    assert_eq!(a.core.library.cover("../outside.jksong", 64).unwrap_err().code, "not_found");
}

#[test]
fn load_play_set_and_debounced_slot_save() {
    let a = app("player", FakeRenderer::default());
    let p = &a.core.player;
    assert_eq!(p.play().unwrap_err().code, "not_loaded");
    let d = p.load("Jukeoke - Test Tone.jksong").unwrap();
    assert_eq!(d.lyrics.as_ref().unwrap().lines[0].text(), "Twinkle, twinkle, little star");
    assert!(!d.lyrics_is_override);
    assert_eq!(d.stems.len(), 4);
    assert_eq!((d.sample_rate, d.channels), (48000, 2));
    let dv = serde_json::to_value(&d).unwrap();
    assert_eq!(dv["settings"]["levels"], json!({"drums": 1.0, "other": 1.0}));
    // Graphemes as [text, start, end] (SONG_FORMAT section 5).
    assert_eq!(dv["lyrics"]["lines"][0]["words"][0]["chars"][0][0], "T");
    assert_eq!(dv["lyrics"]["lines"][0]["words"][0]["chars"][0][1], 0.3);
    let st = p.state();
    assert_eq!((st.state, st.song_id.as_deref()), ("paused", Some("Jukeoke - Test Tone.jksong")));
    assert_eq!(st.key_label.as_deref(), Some("A major (original key)"));

    // Rapid changes: applied to the engine at once, one slot write about 300 ms later.
    let path = a.root.join("Library/Jukeoke - Test Tone.jksong");
    let writes0 = p.saver.writes.load(std::sync::atomic::Ordering::SeqCst);
    for s in [1, 2, 3, 2] {
        p.set(&json!({"semitones": s})).unwrap();
    }
    p.set(&json!({"guide_vocal": 0.5, "levels": {"drums": 0.75}})).unwrap();
    assert_eq!(p.state().key_label.as_deref(), Some("A major → B major (+2)"));
    let last = a.engine.calls.lock().unwrap().iter().rev().find_map(|c| match c {
        Call::SetParams(m) => Some(*m),
        _ => None,
    });
    let last = last.unwrap();
    assert_eq!((last.semitones, last.guide_vocal, last.drums), (2, 0.5, 0.75));
    assert_eq!(saved(&path).settings.semitones, 0, "not written before the quiet period");
    let written = wait_for(Duration::from_secs(5), || {
        (p.saver.writes.load(std::sync::atomic::Ordering::SeqCst) > writes0).then_some(())
    });
    assert!(written.is_some());
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(p.saver.writes.load(std::sync::atomic::Ordering::SeqCst), writes0 + 1, "one write for five changes");
    let s = saved(&path).settings;
    assert_eq!((s.semitones, s.guide_vocal, s.level("drums")), (2, 0.5, 0.75));

    // Seek, play, toggle, stop.
    p.play().unwrap();
    assert_eq!(p.state().state, "playing");
    p.seek(1.5).unwrap();
    assert!(p.seek(f64::NAN).is_err());
    p.toggle().unwrap();
    assert_eq!(p.state().state, "paused");
    p.stop().unwrap();
    assert!(p.state().position_s.abs() < 1e-9);
    assert!(!a.sink.of("playback://state").is_empty());
}

#[test]
fn pending_settings_are_flushed_on_song_change_and_on_exit() {
    let a = app("flush", FakeRenderer::default());
    let p = &a.core.player;
    p.load("Jukeoke - Test Tone.jksong").unwrap();
    p.set(&json!({"semitones": -3, "lyrics_offset_ms": 150})).unwrap();
    // Song change: written before the next song opens, not 300 ms later.
    p.load("Pop/Second.jksong").unwrap();
    let s = saved(&a.root.join("Library/Jukeoke - Test Tone.jksong")).settings;
    assert_eq!((s.semitones, s.lyrics_offset_ms), (-3, 150));
    // The next load uses the saved key.
    let d = p.load("Jukeoke - Test Tone.jksong").unwrap();
    assert_eq!(d.settings.semitones, -3);
    assert_eq!(d.summary.semitones, -3);
    // Exit.
    p.set(&json!({"backing_on": false})).unwrap();
    a.core.shutdown();
    assert!(!saved(&a.root.join("Library/Jukeoke - Test Tone.jksong")).settings.backing_on);
}

#[test]
fn lyrics_override_save_validate_and_reset() {
    let a = app("lyrics", FakeRenderer::default());
    let id = "Jukeoke - Test Tone.jksong";
    let path = a.root.join("Library").join(id);
    let mut lyrics = serde_json::to_value(jk_format::SongFile::open(&path).unwrap().lyrics().unwrap().unwrap()).unwrap();
    lyrics["checked"] = json!(true);
    lyrics["lines"][0]["words"][3]["chars"][0][0] = json!("S"); // "Star"
    a.core.lyrics_save(id, lyrics.clone()).unwrap();
    let d = a.core.player.load(id).unwrap();
    assert!(d.lyrics_is_override);
    assert_eq!(d.lyrics.unwrap().lines[0].text(), "Twinkle, twinkle, little Star");
    // Timing past the end of the song is refused, nothing saved.
    let mut bad = lyrics.clone();
    bad["lines"][0]["end"] = json!(99.0);
    let err = a.core.lyrics_save(id, bad).unwrap_err();
    assert_eq!(err.code, "invalid_lyrics", "{err:?}");
    assert!(saved(&path).lyrics_override.is_some());
    assert_eq!(a.core.lyrics_save(id, json!({"lines": "nope"})).unwrap_err().code, "bad_request");
    a.core.lyrics_reset(id).unwrap();
    assert!(saved(&path).lyrics_override.is_none());
}

#[test]
fn export_uses_live_settings_and_reports_progress() {
    let a = app("export", FakeRenderer::default());
    a.core.player.load("Jukeoke - Test Tone.jksong").unwrap();
    a.core.player.set(&json!({"semitones": 2, "guide_vocal": 0.25})).unwrap();
    let job = a.core.export_backing("Jukeoke - Test Tone.jksong", "wav32f").unwrap();
    let done = wait_for(Duration::from_secs(10), || {
        a.sink.of("export://done").into_iter().find(|e| e["job_id"] == job.job_id.as_str())
    })
    .expect("export done");
    let path = PathBuf::from(done["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "Open Mic - Test Tone (B major, +2).wav");
    assert!(path.starts_with(a.root.join("Exports")) && path.is_file());
    let progress = a.sink.of("export://progress");
    assert!(progress.iter().any(|e| e["progress"] == 1.0), "{progress:?}");
    let (song, params, fmt) = a.renderer.calls.lock().unwrap()[0].clone();
    assert!(song.ends_with("Jukeoke - Test Tone.jksong"));
    assert_eq!((params.semitones, params.guide_vocal), (2, 0.25), "live settings, not yet saved ones");
    assert_eq!(fmt, jk_engine::ExportFormat::Wav32f);
    // Same name again gets " (2)".
    let job2 = a.core.export_backing("Jukeoke - Test Tone.jksong", "wav32f").unwrap();
    let done2 = wait_for(Duration::from_secs(10), || {
        a.sink.of("export://done").into_iter().find(|e| e["job_id"] == job2.job_id.as_str())
    })
    .unwrap();
    assert!(done2["path"].as_str().unwrap().ends_with("Open Mic - Test Tone (B major, +2) (2).wav"));
    assert_eq!(a.core.export_backing("Jukeoke - Test Tone.jksong", "mp3").unwrap_err().code, "bad_request");
}

#[test]
fn export_failure_and_cancel() {
    let failing = FakeRenderer {
        fail: Some(jk_engine::EngineError::new("write_failed", "The drive is full.", "ENOSPC")),
        ..Default::default()
    };
    let a = app("exportfail", failing);
    let job = a.core.export_backing("Pop/Second.jksong", "flac24").unwrap();
    let failed = wait_for(Duration::from_secs(10), || a.sink.of("export://failed").into_iter().next()).unwrap();
    assert_eq!(failed["job_id"], job.job_id.as_str());
    assert_eq!(failed["code"], "write_failed");
    assert_eq!(failed["message"], "The drive is full.");

    let slow = FakeRenderer { steps: 200, step_ms: 10, ..Default::default() };
    let b = app("exportcancel", slow);
    let job = b.core.export_backing("Pop/Second.jksong", "flac24").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    b.core.exports.cancel(&job.job_id).unwrap();
    let failed = wait_for(Duration::from_secs(10), || b.sink.of("export://failed").into_iter().next()).unwrap();
    assert_eq!(failed["code"], "cancelled");
    assert_eq!(b.core.exports.cancel("nope").unwrap_err().code, "not_found");
}

#[test]
fn settings_devices_and_master_volume() {
    let a = app("settings", FakeRenderer::default());
    let s = a.core.settings_set(&json!({"audio": {"master_volume": 0.5}, "ui": {"bubbles": false}})).unwrap();
    assert_eq!(s.audio.master_volume, 0.5);
    assert!(!s.ui.bubbles && s.ui.skin);
    a.core.set_master(0.25).unwrap();
    a.core.device_set(Some("fake-1".into()), true).unwrap();
    let calls = a.engine.take_calls();
    assert!(calls.contains(&Call::SetMaster(0.5)));
    assert!(calls.contains(&Call::SetMaster(0.25)));
    assert!(calls.contains(&Call::SetDevice(Some("fake-1".into()), true)));
    a.core.settings.flush();
    let file: Value = serde_json::from_str(&std::fs::read_to_string(a.root.join("Settings/settings.json")).unwrap()).unwrap();
    assert_eq!(file["audio"]["master_volume"], 0.25);
    assert_eq!(file["audio"]["device_id"], "fake-1");
    assert_eq!(file["audio"]["exclusive"], true);
    assert_eq!(file["ui"]["bubbles"], false);
}

#[test]
fn app_info_matches_the_contract() {
    let a = app("info", FakeRenderer::default());
    let v = serde_json::to_value(a.core.app_info("test-webview".into())).unwrap();
    for k in ["name", "version", "platform", "os_version", "portable", "root", "translocated", "paths", "capabilities"] {
        assert!(v.get(k).is_some(), "AppInfo.{k} missing");
    }
    assert_eq!(v["portable"], true);
    assert_eq!(v["root"], a.root.display().to_string());
    assert_eq!(v["paths"]["library"], a.root.join("Library").display().to_string());
    let caps = &v["capabilities"];
    assert_eq!(caps["studio"]["available"], false);
    assert_eq!(caps["studio"]["reasons"], json!(["No Studio in this test."]));
    for k in ["available", "reason", "permission"] {
        assert!(caps["recorder"].get(k).is_some(), "recorder.{k} missing");
    }
    if cfg!(windows) {
        assert_eq!(v["platform"], "windows");
        assert_eq!(caps["recorder"]["permission"], "not_required");
        assert_eq!(caps["exclusive_mode"], true);
        assert!(v["os_version"].as_str().unwrap().starts_with("Windows 1"));
    }
}

#[test]
fn clock_events_about_30_per_second_while_playing() {
    let a = app("clock", FakeRenderer::default());
    a.core.player.start_clock();
    a.core.player.load("Jukeoke - Test Tone.jksong").unwrap();
    a.core.player.play().unwrap();
    // Let a snapshot taken just before play() drain, then count for one second, measured.
    std::thread::sleep(Duration::from_millis(100));
    // What this machine lets ANY thread do over the same second: a bare ticker with the
    // clock thread's own policy (30 Hz absolute deadlines on a channel's timed wait, 4 ms
    // slack, the grid restarted when more than a period behind) and no work at all. The
    // macOS CI runner gave the clock thread 17 wake-ups in a second (D-279); a fault of
    // the clock loop shows as fewer events than the bare ticker got.
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            const PERIOD: Duration = Duration::from_micros(33_333);
            const SLACK: Duration = Duration::from_millis(4);
            let (_keep, rx) = crossbeam_channel::bounded::<()>(1);
            let mut ticks = Vec::new();
            let mut next = Instant::now();
            while !stop.load(Ordering::SeqCst) {
                let _ = rx.recv_timeout(next.saturating_duration_since(Instant::now()));
                let now = Instant::now();
                if now + SLACK >= next {
                    ticks.push(now);
                    next += PERIOD;
                    if next + PERIOD < now {
                        next = now + PERIOD;
                    }
                }
            }
            ticks
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    a.sink.clear();
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(1000));
    let clock = a.sink.of("playback://clock");
    let t1 = Instant::now();
    stop.store(true, Ordering::SeqCst);
    let bare = ticker.join().unwrap().iter().filter(|t| **t >= t0 && **t <= t1).count();
    let secs = (t1 - t0).as_secs_f64();
    let n = clock.len();
    eprintln!("measured: {n} playback://clock events in {secs:.3} s of playback; a bare 30 Hz ticker got {bare}");
    // The clock loop keeps up with what the machine allows (one tick of slack)...
    assert!(n + 1 >= bare * 9 / 10, "{n} clock events in {secs:.3} s where a bare ticker got {bare}");
    // ...and about 30 per second wherever a thread can be woken 30 times a second (every
    // computer the app supports). Not judged on a machine that cannot: said in the CI
    // job summary, never skipped silently.
    let lo = (25.0 * secs).floor() as usize;
    let hi = (36.0 * secs).ceil() as usize;
    if bare >= lo {
        assert!((lo..=hi).contains(&n), "{n} clock events in {secs:.3} s (bare ticker: {bare})");
    } else {
        let note = format!(
            "clock_events_about_30_per_second_while_playing: this machine woke a bare 30 Hz ticker only {bare} times in {secs:.3} s, so the ~30/s rate was not judged here; the clock thread got {n} events, as many as the machine allowed (D-279)"
        );
        eprintln!("warning: {note}");
        if let Some(p) = std::env::var_os("GITHUB_STEP_SUMMARY") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(p) {
                let _ = writeln!(f, "- :warning: {note}");
            }
        }
    }
    let seqs: Vec<u64> = clock.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]));
    let pos: Vec<f64> = clock.iter().map(|e| e["position_s"].as_f64().unwrap()).collect();
    assert!(pos.windows(2).all(|w| w[1] >= w[0]), "{pos:?}");
    assert!(clock.iter().all(|e| e["playing"] == true));
    // Paused: no more clock events (only one for the state change).
    a.core.player.pause().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    a.sink.clear();
    std::thread::sleep(Duration::from_millis(600));
    assert!(a.sink.of("playback://clock").len() <= 1);
}

#[test]
fn engine_errors_become_playback_error_events() {
    let a = app("errors", FakeRenderer::default());
    a.core.player.start_clock();
    a.engine.inject(jk_engine::EngineEvent::DeviceLost);
    let ev = wait_for(Duration::from_secs(3), || a.sink.of("playback://error").into_iter().next()).unwrap();
    assert_eq!(ev["code"], "device_lost");
    assert!(ev["message"].as_str().unwrap().contains("disconnected"));
    // A damaged song cannot be loaded: a plain error, nothing loaded.
    let err = a.core.player.load("Broken - Song.jksong").unwrap_err();
    assert!(matches!(err.code.as_str(), "not_a_song" | "truncated" | "damaged"), "{err:?}");
    assert_eq!(a.core.player.state().song_id, None);
}

#[test]
fn key_mode_follows_the_setting_and_this_computers_measurement() {
    use jukeoke_lib::engine::KeyMode;
    let a = app("keymode", FakeRenderer::default());
    // The key cache lives in the portable Settings folder (WORK_ORDER 4.8).
    let (dir, cap) = a.engine.key_cache.lock().unwrap().clone().expect("key cache configured");
    assert_eq!(dir, a.root.join("Settings").join("cache").join("keys"));
    assert_eq!(cap, 1024 * 1024 * 1024);
    let last = || *a.engine.key_modes.lock().unwrap().last().expect("a key mode was set");
    // "realtime" first, so no background measurement races the test.
    a.core.settings_set(&json!({"audio": {"key_mode": "realtime"}})).unwrap();
    let id = "Jukeoke - Test Tone.jksong";
    let rate = a.core.player.load(id).unwrap().sample_rate;
    assert_eq!(last(), KeyMode::Realtime);
    let key = jukeoke_lib::keymode::bench_key(&jukeoke_lib::keymode::machine_id(), rate);
    let bench = |f: f64| {
        let mut m = serde_json::Map::new();
        m.insert(key.clone(), json!(f));
        json!({"audio": {"key_benchmarks": Value::Object(m)}})
    };
    // A slow computer (measured 2x): auto renders ahead; a fast one shifts in real time.
    a.core.settings_set(&bench(2.0)).unwrap();
    a.core.settings_set(&json!({"audio": {"key_mode": "auto"}})).unwrap();
    assert_eq!(last(), KeyMode::Prerender);
    a.core.settings_set(&bench(9.0)).unwrap();
    assert_eq!(last(), KeyMode::Realtime);
    a.core.settings_set(&json!({"audio": {"key_mode": "prerender"}})).unwrap();
    assert_eq!(last(), KeyMode::Prerender);
    // A load sets the mode before the engine loads the song.
    a.engine.key_modes.lock().unwrap().clear();
    a.engine.take_calls();
    a.core.player.load(id).unwrap();
    assert_eq!(*a.engine.key_modes.lock().unwrap(), vec![KeyMode::Prerender]);
    // The cache cap follows its setting.
    a.core.settings_set(&json!({"audio": {"key_cache_mb": 256}})).unwrap();
    assert_eq!(a.engine.key_cache.lock().unwrap().clone().unwrap().1, 256 * 1024 * 1024);
    // PlayerState carries the mode fields (APP_API section 3 additions).
    let st = serde_json::to_value(a.core.player.state()).unwrap();
    for k in ["key_mode", "key_preparing", "key_note"] {
        assert!(st.get(k).is_some(), "PlayerState.{k} missing");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody guide queue through the app core (Work Order 2; docs/APP_API.md 12.3,
//! docs/STUDIO_PROTOCOL.md 12, docs/SONG_FORMAT.md 7.1), with the fake engine (which keeps
//! the loaded song's file open as the real one does, so a rename fails while it is loaded):
//!
//! - `guide_build` adds a guide and every other chunk and both slots stay byte-identical;
//! - the playing song keeps playing: the guide is used from memory and written when the
//!   song is paused (released and loaded again at the same position);
//! - a song loaded but paused is released, swapped and reloaded at once;
//! - a failure leaves the song as it was and playable, with a plain reason;
//! - a song with no lead vocal says so;
//! - "Add practice data to all songs" with `guide://batch` progress;
//! - the queue survives a restart (Settings/guide-queue.json);
//! - cancel;
//! - a new song from the (fake) Studio gets the `guide` stage.

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::fake::{Call, FakeEngine, FakeRenderer};
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::guide::GuideStatus;
use jukeoke_lib::portable::DataLayout;
use jukeoke_lib::studio::StudioConfig;
use serde_json::{Value, json};
use sha2::Digest;

struct App {
    root: PathBuf,
    core: Arc<Core>,
    engine: Arc<FakeEngine>,
    sink: Arc<RecordingSink>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn exe_in(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke")
    } else {
        root.join("Windows/Jukeoke.exe")
    }
}

fn core_at(root: &Path, studio: Result<StudioConfig, String>) -> (Arc<Core>, Arc<FakeEngine>, Arc<RecordingSink>) {
    let exe = exe_in(root);
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    let engine = Arc::new(FakeEngine::default());
    engine.hold_open.store(true, Ordering::SeqCst);
    let sink = Arc::new(RecordingSink::default());
    let core = Core::new(CoreOptions {
        layout,
        exe: Some(exe),
        name: "Jukeoke".into(),
        version: "0.1.0".into(),
        identifier: "org.jukeoke.app".into(),
        sink: sink.clone(),
        engine: engine.clone(),
        renderer: Arc::new(FakeRenderer::default()),
        studio,
        demo_song: None,
        slot_save_delay: Duration::from_millis(200),
        webview_env_ignored: None,
    });
    (core, engine, sink)
}

/// A portable root with three copies of the tiny fixture (a 4 s song whose lead vocal is
/// a melody) and the guide builder running.
fn app(name: &str) -> App {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("Library")).unwrap();
    for n in ["A.jksong", "B.jksong", "C.jksong"] {
        std::fs::copy(tiny_song(), root.join("Library").join(n)).unwrap();
    }
    let (core, engine, sink) = core_at(&root, Err("No Studio in this test.".into()));
    core.guide.start();
    App { root, core, engine, sink }
}

fn sha(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// SHA-256 of every chunk except the guide's (by id) and of both slot regions.
fn fingerprint(p: &Path) -> (Vec<(String, String)>, String, String) {
    let bytes = std::fs::read(p).unwrap();
    let s = jk_format::SongFile::open(p).unwrap();
    let h = s.header().clone();
    let chunks = s
        .toc()
        .chunks
        .iter()
        .filter(|c| c.id != "guide" && c.id != "guide.curve")
        .map(|c| (c.id.clone(), sha(&bytes[c.offset as usize..(c.offset + c.length) as usize])))
        .collect();
    let r = |o: u64| sha(&bytes[o as usize..(o + h.slot_size as u64) as usize]);
    (chunks, r(h.slot_a_offset), r(h.slot_b_offset))
}

fn status(a: &App, id: &str) -> GuideStatus {
    a.core.guide_status(id).unwrap()
}

fn wait_state(a: &App, id: &str, want: &str, secs: u64) -> GuideStatus {
    wait_for(Duration::from_secs(secs), || Some(status(a, id)).filter(|s| s.state == want))
        .unwrap_or_else(|| panic!("{id}: never {want}; now {:?}", status(a, id)))
}

fn has_guide(p: &Path) -> bool {
    jk_format::SongFile::open(p).unwrap().chunk("guide").is_some()
}

#[test]
fn guide_build_adds_a_guide_and_keeps_every_other_byte() {
    let a = app("guide-build");
    let p = a.root.join("Library/A.jksong");
    jk_format::update_user_state(&p, |s| s.settings.semitones = -2).unwrap();
    let before = fingerprint(&p);
    assert_eq!(status(&a, "A.jksong").state, "none");
    let s = a.core.guide_build("A.jksong", false).unwrap();
    assert!(matches!(s.state, "queued" | "building" | "ready"), "{s:?}");
    let s = wait_state(&a, "A.jksong", "ready", 60);
    assert!(!s.in_memory && !s.pending_write);
    assert_eq!(s.method.as_deref(), Some(jk_guide::METHOD));
    assert!(s.counts.as_ref().unwrap().notes > 0, "{s:?}");
    assert_eq!(fingerprint(&p), before, "stems, lyrics, cover and slots byte-identical");
    let rep = jk_format::verify_file(&p, &jk_format::VerifyOptions { deep: true, ..Default::default() });
    assert!(rep.ok() && rep.warnings.is_empty(), "{:?} {:?}", rep.errors, rep.warnings);
    // Events: queued / building ... ready; the library lists it ready.
    let ev: Vec<Value> = a.sink.of("guide://status").into_iter().filter(|e| e["song_id"] == "A.jksong").collect();
    let states: Vec<&str> = ev.iter().filter_map(|e| e["status"]["state"].as_str()).collect();
    assert!(states.contains(&"building") && states.last() == Some(&"ready"), "{states:?}");
    let list = a.core.library_list().unwrap();
    let a_song = list.songs.iter().find(|s| s.song_id == "A.jksong").unwrap();
    assert_eq!(a_song.guide.state, "ready");
    assert_eq!(list.songs.iter().find(|s| s.song_id == "B.jksong").unwrap().guide.state, "none");
    // Asked again: a current guide is not rebuilt; force rebuilds it.
    assert_eq!(a.core.guide_build("A.jksong", false).unwrap().state, "ready");
    let s = a.core.guide_build("A.jksong", true).unwrap();
    assert!(matches!(s.state, "queued" | "building"), "{s:?}");
    wait_state(&a, "A.jksong", "ready", 60);
    assert_eq!(fingerprint(&p), before);
    // The status turns "ready" as the builder settles, and the queue file is rewritten
    // just after (GuideService::settle: the state lock is released before the file is
    // written), so wait for it rather than read it once (verifier: this read raced and
    // failed in 3 of 7 runs).
    let q = a.root.join("Settings/guide-queue.json");
    let file = wait_for(Duration::from_secs(5), || {
        let v: Value = serde_json::from_slice(&std::fs::read(&q).ok()?).ok()?;
        (v["items"] == json!([])).then_some(v)
    });
    assert!(file.is_some(), "a finished song leaves the queue file: {:?}", std::fs::read_to_string(&q));
}

#[test]
fn the_playing_song_keeps_playing_and_the_swap_waits_for_a_pause() {
    let a = app("guide-playing");
    let p = a.root.join("Library/A.jksong");
    a.core.song_load("A.jksong").unwrap();
    a.core.player.play().unwrap();
    a.engine.take_calls();
    a.core.guide_build("A.jksong", false).unwrap();
    let s = wait_for(Duration::from_secs(60), || Some(status(&a, "A.jksong")).filter(|s| s.pending_write)).unwrap();
    assert_eq!((s.state, s.in_memory), ("ready", true), "{s:?}");
    assert!(a.core.guide.memory_guide("A.jksong").is_some(), "used from memory meanwhile");
    std::thread::sleep(Duration::from_millis(800));
    assert!(!has_guide(&p), "nothing is swapped while the song plays");
    assert!(!a.engine.take_calls().contains(&Call::Unload), "playback was never interrupted");
    assert_eq!(a.core.player.state().state, "playing");
    // Paused: released, swapped, loaded again at the same position.
    a.core.player.pause().unwrap();
    let pos = a.core.player.state().position_s;
    let s = wait_for(Duration::from_secs(10), || Some(status(&a, "A.jksong")).filter(|s| !s.pending_write)).unwrap();
    assert_eq!((s.state, s.in_memory), ("ready", false), "{s:?}");
    assert!(has_guide(&p));
    let calls = a.engine.take_calls();
    let i = calls.iter().position(|c| *c == Call::Unload).expect("released");
    assert!(matches!(&calls[i + 1], Call::Load(q, _) if q == &p), "{calls:?}");
    assert!(calls[i + 2..].iter().any(|c| matches!(c, Call::Seek(x) if (x - pos).abs() < 0.05)), "{calls:?}");
    let st = a.core.player.state();
    assert_eq!((st.state, st.song_id.as_deref()), ("paused", Some("A.jksong")));
}

#[test]
fn the_loaded_paused_song_is_released_swapped_and_reloaded() {
    let a = app("guide-paused");
    let p = a.root.join("Library/B.jksong");
    a.core.song_load("B.jksong").unwrap();
    a.core.player.seek(1.5).unwrap();
    a.core.player.set(&json!({"semitones": 3})).unwrap(); // a pending settings save
    a.engine.take_calls();
    a.core.guide_build("B.jksong", false).unwrap();
    wait_state(&a, "B.jksong", "ready", 60);
    assert!(has_guide(&p));
    let calls = a.engine.take_calls();
    assert!(calls.contains(&Call::Unload), "{calls:?}");
    assert!(calls.iter().any(|c| matches!(c, Call::Seek(x) if (x - 1.5).abs() < 1e-9)), "{calls:?}");
    // The settings save made just before survived the swap.
    assert_eq!(jk_format::SongFile::open(&p).unwrap().effective_settings().semitones, 3);
    assert_eq!(a.core.player.state().song_id.as_deref(), Some("B.jksong"));
}

#[test]
fn a_failure_leaves_the_song_as_it_was_and_playable() {
    let a = app("guide-fail");
    let p = a.root.join("Library/C.jksong");
    // Damage the drums stem (the guide reads only the lead vocal, the raw copy checks all).
    let s = jk_format::SongFile::open(&p).unwrap();
    let c = s.chunk("stem.drums").unwrap().clone();
    drop(s);
    let mut b = std::fs::read(&p).unwrap();
    b[(c.offset + c.length / 2) as usize] ^= 0xFF;
    std::fs::write(&p, &b).unwrap();
    a.core.guide_build("C.jksong", false).unwrap();
    let s = wait_state(&a, "C.jksong", "failed", 60);
    assert_eq!(s.error_code.as_deref(), Some("guide_damaged"), "{s:?}");
    assert!(s.reason.as_deref().unwrap().contains("damaged"), "{s:?}");
    assert!(s.in_memory, "the guide was built: usable for this session");
    assert_eq!(std::fs::read(&p).unwrap(), b, "the song file is untouched");
    assert!(std::fs::read_dir(a.root.join("Library")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".tmp-")));
    a.core.song_load("C.jksong").unwrap();
    a.core.player.play().unwrap();
    assert_eq!(a.core.player.state().state, "playing");
    // "Try again" is offered: the build can be queued again.
    let s = a.core.guide_build("C.jksong", false).unwrap();
    assert!(matches!(s.state, "queued" | "building" | "failed"), "{s:?}");
}

#[test]
fn a_song_without_a_lead_vocal_says_so() {
    let a = app("guide-nolead");
    let p = a.root.join("Library/A.jksong");
    let mut m = jk_format::SongFile::open(&p).unwrap().manifest().clone();
    m.stems.retain(|s| s.role != "lead_vocal");
    jk_format::rewrite_song(&p, jk_format::Rewrite { manifest: Some(m), remove: vec!["stem.lead_vocal".into()], ..Default::default() }).unwrap();
    let s = status(&a, "A.jksong");
    assert_eq!((s.state, s.error_code.as_deref()), ("failed", Some("guide_no_lead")), "{s:?}");
    assert!(s.reason.unwrap().contains("no separate lead vocal"));
    let s = a.core.guide_build("A.jksong", false).unwrap();
    assert_eq!(s.state, "failed", "not queued: there is nothing to build from");
}

#[test]
fn add_practice_data_to_all_songs_reports_batch_progress() {
    let a = app("guide-all");
    std::fs::write(a.root.join("Library/Broken.jksong"), b"not a song").unwrap();
    let r = a.core.guide_build_all(false).unwrap();
    assert_eq!((r.queued, r.skipped), (3, 1), "the damaged file is skipped");
    for id in ["A.jksong", "B.jksong", "C.jksong"] {
        wait_state(&a, id, "ready", 120);
    }
    let batch = wait_for(Duration::from_secs(10), || {
        a.sink.of("guide://batch").into_iter().find(|e| e["running"] == false)
    })
    .expect("a final guide://batch");
    assert_eq!((batch["total"].as_u64(), batch["done"].as_u64(), batch["failed"].as_u64()), (Some(3), Some(3), Some(0)));
    assert!(a.sink.of("guide://batch").iter().any(|e| e["running"] == true && e["current"].is_string()));
    // Again: nothing left to do.
    assert_eq!(a.core.guide_build_all(false).unwrap().queued, 0);
    assert_eq!(a.core.guide_build_all(true).unwrap().queued, 0, "nothing outdated either");
}

#[test]
fn the_queue_survives_a_restart() {
    let root = scratch("guide-restart");
    std::fs::create_dir_all(root.join("Library")).unwrap();
    std::fs::create_dir_all(root.join("Settings")).unwrap();
    std::fs::copy(tiny_song(), root.join("Library/A.jksong")).unwrap();
    // What a session that quit during a guide build leaves behind.
    std::fs::write(
        root.join("Settings/guide-queue.json"),
        serde_json::to_vec(&json!({"queue_version": 1, "items": [
            {"song_id": "A.jksong", "force": false, "origin": "studio", "job_id": "job-1"},
            {"song_id": "Gone.jksong", "force": false, "origin": "batch"}]}))
        .unwrap(),
    )
    .unwrap();
    let (core, _engine, _sink) = core_at(&root, Err("No Studio in this test.".into()));
    assert_eq!(core.guide_queue().items.len(), 2);
    core.guide.start();
    let ok = wait_for(Duration::from_secs(60), || Some(core.guide_status("A.jksong").unwrap()).filter(|s| s.state == "ready"));
    assert!(ok.is_some(), "{:?}", core.guide_status("A.jksong"));
    wait_for(Duration::from_secs(10), || core.guide_queue().items.is_empty().then_some(())).expect("the gone song is dropped");
    let q = root.join("Settings/guide-queue.json");
    let emptied = wait_for(Duration::from_secs(10), || {
        let file: Value = serde_json::from_slice(&std::fs::read(&q).ok()?).ok()?;
        (file["items"] == json!([])).then_some(())
    });
    assert!(emptied.is_some(), "{}", std::fs::read_to_string(&q).unwrap_or_default());
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cancel_leaves_the_files_as_they_were() {
    let root = scratch("guide-cancel");
    std::fs::create_dir_all(root.join("Library")).unwrap();
    for n in ["A.jksong", "B.jksong"] {
        std::fs::copy(tiny_song(), root.join("Library").join(n)).unwrap();
    }
    let (core, _engine, sink) = core_at(&root, Err("No Studio in this test.".into()));
    // Not started: everything stays queued, then is cancelled.
    core.guide_build_all(false).unwrap();
    assert_eq!(core.guide_queue().items.len(), 2);
    core.guide_cancel(None).unwrap();
    assert!(core.guide_queue().items.is_empty());
    for id in ["A.jksong", "B.jksong"] {
        let s = core.guide_status(id).unwrap();
        assert_eq!((s.state, s.error_code.as_deref()), ("failed", Some("cancelled")), "{s:?}");
        assert!(!has_guide(&root.join("Library").join(id)));
    }
    assert!(sink.of("guide://batch").last().is_some_and(|e| e["running"] == false));
    // Built after all, the cancellation is forgotten.
    core.guide.start();
    core.guide_build("A.jksong", false).unwrap();
    let ok = wait_for(Duration::from_secs(60), || Some(core.guide_status("A.jksong").unwrap()).filter(|s| s.state == "ready"));
    assert!(ok.is_some());
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

fn studio_config(dir: &Path, python: &Path) -> StudioConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_studio.py");
    StudioConfig {
        program: python.to_path_buf(),
        args: vec!["-X".into(), "utf8".into(), script.into_os_string(), "serve".into()],
        cwd: dir.to_path_buf(),
        env: vec![
            (OsString::from("FAKE_STUDIO_FIXTURE"), tiny_song().into_os_string()),
            (OsString::from("FAKE_STUDIO_STEP_S"), OsString::from("0.01")),
        ],
        root: dir.to_path_buf(),
        models_dir: dir.join("Models"),
        library_dir: dir.join("Library"),
        temp_dir: dir.join("Settings/tmp/studio"),
        log_path: dir.join("Settings/logs/studio.log"),
        app_version: "0.1.0-test".into(),
        start_timeout: Duration::from_secs(20),
        request_timeout: Duration::from_secs(20),
        idle_shutdown: Duration::from_secs(600),
    }
}

#[test]
fn a_new_song_from_the_studio_gets_the_guide_stage() {
    let Some(py) = python() else {
        eprintln!("SKIPPED: no Python 3 found (set JK_TEST_PYTHON)");
        return;
    };
    let root = scratch("guide-studio");
    std::fs::create_dir_all(root.join("Library")).unwrap();
    let (core, _engine, sink) = core_at(&root, Ok(studio_config(&root, &py)));
    core.studio.probe_now();
    core.guide.start();
    let input = root.join("song.wav");
    std::fs::write(&input, b"not really audio").unwrap();
    let job_id = core
        .studio
        .submit(&json!({"input": input.display().to_string(), "preset": "full", "language": "en",
                        "metadata": {"title": "Twinkle", "artist": "Traditional", "album": null},
                        "lyrics": {"mode": "text", "text": "x", "lrc": null}}))
        .unwrap();
    let job = wait_for(Duration::from_secs(60), || {
        core.studio.status().queue.into_iter().find(|j| j.job_id == job_id && j.state == "done")
    })
    .expect("the job ends");
    let g = job.guide.as_ref().expect("the job carries its guide status");
    assert_eq!(g.state, "ready", "{g:?}");
    let song = root.join("Library/Traditional - Twinkle.jksong");
    assert!(has_guide(&song));
    // The job showed the guide stage, still running, with overall 0.95..1.0.
    let events: Vec<Value> = sink.of("studio://job").into_iter().filter(|e| e["job_id"] == job_id.as_str()).collect();
    let guide_stage: Vec<&Value> = events.iter().filter(|e| e["stage"] == "guide").collect();
    assert!(!guide_stage.is_empty(), "no guide stage in {events:?}");
    assert!(guide_stage.iter().all(|e| e["state"] == "running" && e["overall"].as_f64().unwrap() >= 0.95));
    assert_eq!(guide_stage[0]["message"], "Making the practice data");
    let last = events.last().unwrap();
    assert_eq!((last["state"].as_str(), last["overall"].as_f64()), (Some("done"), Some(1.0)));
    // The song was listed while the guide was made.
    assert!(sink.of("guide://status").iter().any(|e| e["song_id"] == "Traditional - Twinkle.jksong"));
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The Studio supervisor against the fake Studio (tests/fake_studio.py, standard
//! library only): start-up, probe, jobs with progress, the queue surviving a crash,
//! cancel, timeouts, protocol mismatch, idle shutdown, inspect / lookup / models /
//! realign, and stderr going to studio.log.

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::studio::{Studio, StudioConfig, StudioJob};
use serde_json::{Value, json};

struct Fixture {
    dir: PathBuf,
    studio: Arc<Studio>,
    sink: Arc<RecordingSink>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.studio.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config(dir: &Path, python: &Path, env: &[(&str, &str)]) -> StudioConfig {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_studio.py");
    let mut envs: Vec<(OsString, OsString)> = vec![
        ("FAKE_STUDIO_FIXTURE".into(), tiny_song().into_os_string()),
        ("FAKE_STUDIO_STEP_S".into(), "0.01".into()),
    ];
    for (k, v) in env {
        envs.push(((*k).into(), (*v).into()));
    }
    StudioConfig {
        program: python.to_path_buf(),
        args: vec!["-X".into(), "utf8".into(), script.into_os_string(), "serve".into()],
        cwd: dir.to_path_buf(),
        env: envs,
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

/// Starts a supervisor with the fake Studio and runs the probe. `None` (test skipped
/// with a message) when no Python is available.
fn fixture(name: &str, env: &[(&str, &str)], tweak: impl FnOnce(&mut StudioConfig)) -> Option<Fixture> {
    let Some(py) = python() else {
        eprintln!("SKIPPED {name}: no Python 3 found (set JK_TEST_PYTHON)");
        return None;
    };
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("Library")).unwrap();
    let mut cfg = config(&dir, &py, env);
    tweak(&mut cfg);
    let sink = Arc::new(RecordingSink::default());
    let studio = Studio::new(Ok(cfg), sink.clone());
    studio.probe_now();
    Some(Fixture { dir, studio, sink })
}

fn input(dir: &Path, name: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, b"not really audio").unwrap();
    p.display().to_string()
}

/// Process starts recorded in studio.log (the supervisor's header lines).
fn starts(log: &str) -> usize {
    log.lines().filter(|l| l.starts_with("==== ") && l.contains("Studio started")).count()
}

fn job(f: &Fixture, id: &str) -> Option<StudioJob> {
    f.studio.status().queue.into_iter().find(|j| j.job_id == id)
}

fn wait_job_end(f: &Fixture, id: &str) -> StudioJob {
    wait_for(Duration::from_secs(30), || job(f, id).filter(|j| matches!(j.state, "done" | "failed" | "cancelled")))
        .unwrap_or_else(|| panic!("job {id} did not finish: {:?}", job(f, id)))
}

#[test]
fn probe_job_progress_and_output() {
    let Some(f) = fixture("job", &[], |_| {}) else { return };
    let cap = f.studio.capability();
    assert!(cap.available, "{cap:?}");
    assert_eq!(cap.gpu.as_deref(), Some("Fake GPU 9000"));
    assert_eq!(cap.vram_mb, Some(16303));
    let st = f.studio.status();
    assert!(st.running && st.models_present && !st.checking);

    let submit = |name: &str| {
        f.studio
            .submit(&json!({"input": input(&f.dir, name), "preset": "full", "language": "en",
                            "metadata": {"title": "Twinkle", "artist": "Traditional", "album": null},
                            "lyrics": {"mode": "text", "text": "Twinkle, twinkle, little star", "lrc": null}}))
            .unwrap()
    };
    let a = submit("twinkle.wav");
    let b = submit("twinkle-again.wav");
    let ja = wait_job_end(&f, &a);
    let jb = wait_job_end(&f, &b);
    assert_eq!(ja.state, "done", "{ja:?}");
    assert_eq!(ja.output_song_id.as_deref(), Some("Traditional - Twinkle.jksong"));
    assert_eq!(jb.output_song_id.as_deref(), Some("Traditional - Twinkle (2).jksong"));
    assert!(f.dir.join("Library/Traditional - Twinkle (2).jksong").is_file());
    // The written song opens.
    jk_format::SongFile::open(f.dir.join("Library/Traditional - Twinkle.jksong")).unwrap();

    // Mirrored to the UI: queued -> running (every stage) -> done, overall rising.
    let events: Vec<Value> = f.sink.of("studio://job").into_iter().filter(|e| e["job_id"] == a.as_str()).collect();
    assert_eq!(events.first().unwrap()["state"], "queued");
    assert_eq!(events.last().unwrap()["state"], "done");
    let stages: Vec<&str> = events.iter().filter_map(|e| e["stage"].as_str()).collect();
    for s in ["decode", "separate", "lyrics", "align", "analyse", "write"] {
        assert!(stages.contains(&s), "stage {s} not mirrored: {stages:?}");
    }
    let overall: Vec<f64> = events.iter().map(|e| e["overall"].as_f64().unwrap()).collect();
    assert!(overall.windows(2).all(|w| w[1] >= w[0]), "{overall:?}");
    assert!(!f.sink.of("library://changed").is_empty());

    // stderr went to studio.log.
    let log = std::fs::read_to_string(f.dir.join("Settings/logs/studio.log")).unwrap();
    assert!(log.contains("fake Studio started"), "{log}");
}

#[test]
fn a_bad_file_fails_alone_and_the_queue_goes_on() {
    let Some(f) = fixture("bad", &[], |_| {}) else { return };
    let bad = f.studio.submit(&json!({"input": input(&f.dir, "baddecode.mp3")})).unwrap();
    let good = f.studio.submit(&json!({"input": input(&f.dir, "Good Song.flac")})).unwrap();
    let jb = wait_job_end(&f, &bad);
    assert_eq!((jb.state, jb.error_code.as_deref()), ("failed", Some("decode_failed")));
    assert_eq!(jb.error.as_deref(), Some("This file could not be read as audio."));
    assert_eq!(wait_job_end(&f, &good).state, "done");
}

#[test]
fn a_crash_fails_the_running_job_and_the_next_job_still_runs() {
    let Some(f) = fixture("crash", &[], |_| {}) else { return };
    let crash = f.studio.submit(&json!({"input": input(&f.dir, "crash me.wav")})).unwrap();
    let after = f.studio.submit(&json!({"input": input(&f.dir, "following song.wav")})).unwrap();
    let jc = wait_job_end(&f, &crash);
    assert_eq!(jc.state, "failed");
    assert_eq!(jc.error_code.as_deref(), Some("studio_crashed"));
    assert!(jc.error.unwrap().contains("stopped unexpectedly"));
    // The waiting job was sent again to a new Studio process.
    let ja = wait_job_end(&f, &after);
    assert_eq!(ja.state, "done", "{ja:?}");
    let log = std::fs::read_to_string(f.dir.join("Settings/logs/studio.log")).unwrap();
    assert_eq!(starts(&log), 2, "{log}");
}

#[test]
fn repeated_crashes_stop_the_restarts() {
    let Some(f) = fixture("crashloop", &[], |_| {}) else { return };
    let ids: Vec<String> = (0..5)
        .map(|i| f.studio.submit(&json!({"input": input(&f.dir, &format!("crash {i}.wav"))})).unwrap())
        .collect();
    let ends: Vec<StudioJob> = ids.iter().map(|id| wait_job_end(&f, id)).collect();
    assert!(ends.iter().all(|j| j.state == "failed"), "{ends:?}");
    // At most MAX_CRASHES processes crashed; the rest failed without a restart.
    let log = std::fs::read_to_string(f.dir.join("Settings/logs/studio.log")).unwrap();
    let starts = starts(&log);
    assert!(starts <= jukeoke_lib::studio::MAX_CRASHES + 1, "{starts} starts");
    assert!(ends.iter().any(|j| j.error.as_deref().unwrap_or("").contains("several times")), "{ends:?}");
}

#[test]
fn cancel_a_running_and_a_waiting_job() {
    let Some(f) = fixture("cancel", &[], |_| {}) else { return };
    let slow = f.studio.submit(&json!({"input": input(&f.dir, "slow song.wav")})).unwrap();
    let waiting = f.studio.submit(&json!({"input": input(&f.dir, "next.wav")})).unwrap();
    wait_for(Duration::from_secs(10), || job(&f, &slow).filter(|j| j.state == "running")).expect("running");
    f.studio.cancel(&waiting).unwrap();
    f.studio.cancel(&slow).unwrap();
    assert_eq!(wait_job_end(&f, &slow).state, "cancelled");
    assert_eq!(wait_job_end(&f, &waiting).state, "cancelled");
    f.studio.clear_finished();
    assert!(f.studio.status().queue.is_empty());
}

#[test]
fn requests_inspect_lookup_models_realign() {
    let Some(f) = fixture("requests", &[("FAKE_STUDIO_MODELS", "missing")], |_| {}) else { return };
    assert!(!f.studio.status().models_present);
    let r = f.studio.inspect(&input(&f.dir, "Artist - Song Title.mp3")).unwrap();
    assert_eq!(r["title"], "Song Title");
    assert_eq!(r["artist"], "Artist");
    assert!(r.get("type").is_none() && r.get("id").is_none());
    assert_eq!(f.studio.inspect("Z:/no/such/file.mp3").unwrap_err().code, "not_found");

    let r = f
        .studio
        .lyrics_lookup(&json!({"artist": "Traditional", "title": "Twinkle", "album": null, "duration_s": 30.0}))
        .unwrap();
    assert_eq!(r["results"][0]["service"], "fake");
    assert_eq!(r["results"][0]["duration_seconds"], 30.0);

    let r = f.studio.models_status("full").unwrap();
    assert_eq!(r["all_present"], false);
    let r = f.studio.models_download("full").unwrap();
    assert_eq!(r["ok"], true);
    assert!(f.studio.status().models_present);
    let progress = f.sink.of("studio://models");
    assert_eq!(progress.len(), 3, "{progress:?}");
    assert_eq!(progress[2]["done_bytes"], 1000);

    let song = f.dir.join("Library/song.jksong");
    std::fs::copy(tiny_song(), &song).unwrap();
    let lyrics = f
        .studio
        .realign(&song, "Twinkle twinkle\nlittle star", &json!([{"line": 1, "start": 2.0}]))
        .unwrap();
    assert_eq!(lyrics["lines"].as_array().unwrap().len(), 2);
    let parsed: jk_format::Lyrics = serde_json::from_value(lyrics).unwrap();
    assert_eq!(parsed.lines[1].text(), "little star");
}

#[test]
fn probe_says_no_gpu() {
    let Some(f) = fixture("nogpu", &[("FAKE_STUDIO_PROBE", "no_gpu")], |_| {}) else { return };
    let cap = f.studio.capability();
    assert!(!cap.available);
    assert!(cap.reasons[0].contains("No NVIDIA graphics card"), "{cap:?}");
    assert!(!f.studio.running(), "an unusable Studio is stopped");
    let err = f.studio.submit(&json!({"input": input(&f.dir, "x.wav")})).unwrap_err();
    assert_eq!(err.code, "studio_unavailable");
    assert!(err.message.contains("No NVIDIA"));
}

#[test]
fn a_hanging_probe_times_out_instead_of_hanging() {
    let t = std::time::Instant::now();
    let Some(f) = fixture("hang", &[("FAKE_STUDIO_PROBE", "hang")], |c| c.start_timeout = Duration::from_secs(2)) else {
        return;
    };
    assert!(t.elapsed() < Duration::from_secs(15), "took {:?}", t.elapsed());
    let cap = f.studio.capability();
    assert!(!cap.available);
    assert!(cap.reasons[0].contains("did not answer within 2 seconds"), "{cap:?}");
    assert!(!f.studio.running());
}

#[test]
fn a_slow_hello_times_out() {
    let Some(f) = fixture("slowhello", &[("FAKE_STUDIO_HELLO_DELAY", "5")], |c| {
        c.start_timeout = Duration::from_secs(1)
    }) else {
        return;
    };
    let cap = f.studio.capability();
    assert!(!cap.available);
    assert!(cap.reasons[0].contains("did not answer within 1 seconds after starting"), "{cap:?}");
}

#[test]
fn protocol_mismatch_makes_the_studio_unavailable() {
    let Some(f) = fixture("proto", &[("FAKE_STUDIO_PROTOCOL", "2")], |_| {}) else { return };
    let cap = f.studio.capability();
    assert!(!cap.available);
    assert!(cap.reasons[0].to_lowercase().contains("protocol"), "{cap:?}");
}

#[test]
fn idle_studio_is_stopped_and_restarted_on_demand() {
    let Some(f) = fixture("idle", &[], |c| c.idle_shutdown = Duration::from_millis(500)) else { return };
    assert!(wait_for(Duration::from_secs(10), || (!f.studio.running()).then_some(())).is_some(), "not stopped when idle");
    assert!(f.studio.capability().available, "stopping when idle is not unavailability");
    let r = f.studio.inspect(&input(&f.dir, "Someone - Something.wav")).unwrap();
    assert_eq!(r["artist"], "Someone");
    assert!(f.studio.running());
}

#[test]
fn missing_runtime_is_explained() {
    let sink = Arc::new(RecordingSink::default());
    let studio = Studio::new(Err("The Studio engine is not on this drive.".into()), sink);
    studio.probe_now();
    let st = studio.status();
    assert!(!st.available && !st.running && !st.checking);
    assert_eq!(st.reasons, vec!["The Studio engine is not on this drive.".to_string()]);
    assert_eq!(studio.submit(&json!({"input": "x"})).unwrap_err().code, "studio_unavailable");
}

/// The production command line next to a portable stick: isolated Python from
/// `<root>/Windows/Studio`, and the models folder on the stick (verifier, 2026-11-17).
#[cfg(windows)]
#[test]
fn packaged_studio_command_line_and_models_dir() {
    if std::env::var_os("JK_STUDIO_PYTHON").is_some() {
        eprintln!("SKIPPED: JK_STUDIO_PYTHON overrides the packaged Studio");
        return;
    }
    let root = scratch("locate");
    std::fs::create_dir_all(root.join("Library")).unwrap();
    let studio_dir = root.join("Windows").join("Studio");
    std::fs::create_dir_all(studio_dir.join("python")).unwrap();
    std::fs::write(studio_dir.join("python").join("python.exe"), b"").unwrap();
    let exe = root.join("Windows").join("Jukeoke.exe");
    let layout = jukeoke_lib::portable::DataLayout::detect(&exe, None, || None).unwrap();
    let paths = layout.paths();
    let cfg = StudioConfig::locate(exe.parent(), false, &paths, &root, "0.1.0").unwrap();
    assert_eq!(cfg.program, studio_dir.join("python").join("python.exe"));
    assert_eq!(cfg.cwd, studio_dir);
    let args: Vec<String> = cfg.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, ["-I", "-X", "utf8", "-m", "jukeoke_studio", "serve"]);
    let models = cfg.env.iter().find(|(k, _)| k == "JUKEOKE_MODELS_DIR").map(|(_, v)| PathBuf::from(v));
    assert_eq!(models, Some(root.join("Models")));
    assert_eq!(cfg.models_dir, root.join("Models"));
    assert!(cfg.log_path.starts_with(root.join("Settings")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn closing_the_app_does_not_wait_for_a_slow_start() {
    let Some(py) = python() else {
        eprintln!("SKIPPED: no Python 3 found");
        return;
    };
    let dir = scratch("closeslow");
    let mut cfg = config(&dir, &py, &[("FAKE_STUDIO_HELLO_DELAY", "20")]);
    cfg.start_timeout = Duration::from_secs(60);
    let studio = Studio::new(Ok(cfg), Arc::new(RecordingSink::default()));
    let s = Arc::clone(&studio);
    let probe = std::thread::spawn(move || s.probe_now());
    std::thread::sleep(Duration::from_millis(1500));
    assert!(studio.running(), "the Studio should be starting");
    let t = std::time::Instant::now();
    studio.shutdown();
    let took = t.elapsed();
    eprintln!("measured: shutdown during a 20 s hello took {took:?}");
    assert!(took < Duration::from_secs(3), "shutdown waited {took:?}");
    probe.join().unwrap();
    assert!(!studio.running());
    let _ = std::fs::remove_dir_all(dir);
}

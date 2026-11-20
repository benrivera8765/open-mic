// SPDX-License-Identifier: GPL-3.0-or-later
//! The self-check (src/selfcheck.rs) on the real engine with the null output device
//! (silent mode) and a freshly generated demo song: every item that does not need
//! hardware must pass, and the report must be written into the Settings folder.
//! The recorder item depends on the platform's capture backend and is only printed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::EngineRenderer;
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::portable::DataLayout;
use jukeoke_lib::selfcheck;

#[test]
fn selfcheck_on_the_null_device() {
    let root = scratch("selfcheck");
    std::fs::create_dir_all(root.join("Library")).unwrap();
    let exe = if cfg!(target_os = "macos") {
        root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke")
    } else {
        root.join("Windows/Jukeoke.exe")
    };
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    let demo = root.join("demo-song.jksong");
    jk_engine::demo::make_demo_song(&demo).expect("generate the demo song");
    let engine = jk_engine::Engine::new(jk_engine::EngineConfig {
        device_id: Some("null:48000".into()),
        exclusive: false,
        master: 1.0,
    })
    .unwrap();
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
        demo_song: Some(demo),
        slot_save_delay: Duration::from_millis(200),
        webview_env_ignored: None,
    });
    let r = selfcheck::run(
        &core,
        &selfcheck::Options {
            user_agent: Some("test-agent".into()),
            silent: true,
            // (WO2) As the page would report it (APP_API 10.1).
            lane: Some(jukeoke_lib::practice::checks::LaneCost {
                frames: 600,
                avg_ms: 1.2,
                p95_ms: 2.0,
                simple_avg_ms: 0.8,
                width: 1224.0,
                height: 324.0,
                dpr: 1.0,
            }),
        },
    );
    for i in &r.items {
        eprintln!("[{}] {}: {}", if i.ok { "OK" } else { "FAIL" }, i.name, i.detail);
    }
    let get = |n: &str| r.items.iter().find(|i| i.name == n).unwrap_or_else(|| panic!("no item {n}"));
    for name in ["System", "Memory", "Output device", "Test tone", "Demo song", "Key change"] {
        assert!(get(name).ok, "{name}: {}", get(name).detail);
    }
    assert!(get("Test tone").detail.contains("digital silence"));
    assert!(get("Key change").detail.contains("+2 semitones"));
    assert!(get("Recorder").detail.len() > 10);
    // (WO2, APP_API 10.1) The six practice items follow "Key change", in order, and pass
    // here (no microphone on a build machine is "no input", not a failure).
    let names: Vec<&str> = r.items.iter().map(|i| i.name.as_str()).collect();
    let k = names.iter().position(|n| *n == "Key change").unwrap();
    assert_eq!(
        &names[k + 1..k + 7],
        ["Microphone permission", "Microphone capture", "Pitch tracker", "Practice guide", "Practice score", "Practice lane"]
    );
    for name in ["Pitch tracker", "Practice guide", "Practice score", "Practice lane"] {
        assert!(get(name).ok, "{name}: {}", get(name).detail);
    }
    assert!(get("Practice score").detail.contains("perfect"), "{}", get("Practice score").detail);
    assert!(get("Practice lane").detail.contains("1.20 ms"));
    // The report is in Settings, named selfcheck-<local time>.txt, and says the result.
    let report = std::path::PathBuf::from(&r.report_path);
    assert!(report.starts_with(root.join("Settings")));
    assert!(report.file_name().unwrap().to_string_lossy().starts_with("selfcheck-"));
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains(if r.passed { "result: PASSED" } else { "result: FAILED" }));
    assert!(text.contains("[ OK ] Key change"));
    // Nothing left behind in the scratch folder.
    let leftovers: Vec<_> = std::fs::read_dir(root.join("Settings/tmp/selfcheck"))
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    core.shutdown();
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

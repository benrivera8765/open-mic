// SPDX-License-Identifier: GPL-3.0-or-later
//! The library watcher (library://changed) and the recorder commands' core on the
//! real capture backend. The recording test makes no sound: it captures whatever the
//! computer plays (on Windows the recorder keeps the device running with a stream of
//! digital silence), for one second.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::library::Watcher;

#[test]
fn watcher_reports_new_and_removed_songs_once_per_burst() {
    let dir = scratch("watch");
    let sink = Arc::new(RecordingSink::default());
    let _w = Watcher::start(&dir, sink.clone()).expect("watcher");
    std::thread::sleep(Duration::from_millis(200));
    // A burst: a temp file renamed to a song (what an atomic write looks like).
    let tmp = dir.join("A - B.jksong.tmp-1");
    std::fs::copy(tiny_song(), &tmp).unwrap();
    std::fs::rename(&tmp, dir.join("A - B.jksong")).unwrap();
    let got = wait_for(Duration::from_secs(5), || (!sink.of("library://changed").is_empty()).then_some(()));
    assert!(got.is_some(), "no library://changed after adding a song");
    std::thread::sleep(Duration::from_millis(800));
    let n = sink.of("library://changed").len();
    assert_eq!(n, 1, "one event for one burst, got {n}");
    sink.clear();
    std::fs::remove_file(dir.join("A - B.jksong")).unwrap();
    let got = wait_for(Duration::from_secs(5), || (!sink.of("library://changed").is_empty()).then_some(()));
    assert!(got.is_some(), "no library://changed after removing a song");
    assert_eq!(sink.of("library://changed")[0], serde_json::Value::Null, "no payload");
    drop(_w);
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder of songs moved out of the Library (what Explorer's "Delete" does: a move
/// to the Recycle Bin) is reported, although the watcher can no longer see that the
/// vanished name was a folder (verifier, 2026-11-17).
#[test]
fn watcher_reports_a_song_folder_moved_away() {
    let base = scratch("watch-folder");
    let dir = base.join("Library");
    std::fs::create_dir_all(dir.join("Pop")).unwrap();
    std::fs::copy(tiny_song(), dir.join("Pop").join("A - B.jksong")).unwrap();
    let sink = Arc::new(RecordingSink::default());
    let _w = Watcher::start(&dir, sink.clone()).expect("watcher");
    std::thread::sleep(Duration::from_millis(200));
    std::fs::rename(dir.join("Pop"), base.join("Pop (deleted)")).unwrap();
    let got = wait_for(Duration::from_secs(5), || (!sink.of("library://changed").is_empty()).then_some(()));
    assert!(got.is_some(), "no library://changed after a song folder was moved out of the Library");
    drop(_w);
    let _ = std::fs::remove_dir_all(base);
}

#[cfg(windows)]
#[test]
fn record_one_second_and_list_it() {
    use jukeoke_lib::recorder::RecorderCtl;
    let root = scratch("recorder");
    let sink = Arc::new(RecordingSink::default());
    let rec = RecorderCtl::new(root.join("Recordings"), &root.join("Settings"), sink.clone());
    assert_eq!(rec.permission(), "not_required");
    assert_eq!(rec.stop().unwrap_err().code, "not_recording");
    let st = match rec.start() {
        Ok(st) => st,
        Err(e) if e.code == "not_available" => {
            eprintln!("SKIPPED: the recorder backend is not available in this build: {}", e.message);
            return;
        }
        Err(e) => panic!("recorder_start failed: {e:?}"),
    };
    assert_eq!(st.state, "recording");
    assert_eq!(rec.start().unwrap_err().code, "busy");
    std::thread::sleep(Duration::from_millis(1000));
    let live = rec.status();
    let r = rec.stop().unwrap();
    eprintln!(
        "measured: recording {:.3} s, {} Hz, {} ch, {} bytes, clipped {}; level events {}",
        r.duration_s,
        r.sample_rate,
        r.channels,
        r.bytes,
        r.clipped,
        sink.of("recorder://level").len()
    );
    assert!((0.8..=1.3).contains(&r.duration_s), "{r:?}");
    assert!(r.name.starts_with("Recording ") && r.name.ends_with(".wav"), "{}", r.name);
    let f = live.format.expect("format known while recording");
    assert_eq!((f.sample_rate, f.channels), (r.sample_rate, r.channels));
    assert!(!f.note.is_empty());
    let levels = sink.of("recorder://level");
    assert!(levels.len() >= 10, "about 20 level events per second, got {}", levels.len());
    let list = rec.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, r.name);
    assert!((list[0].duration_s - r.duration_s).abs() < 0.01);
    assert_eq!(rec.status().state, "idle");
    let _ = std::fs::remove_dir_all(root);
}

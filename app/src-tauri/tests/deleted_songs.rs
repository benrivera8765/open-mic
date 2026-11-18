// SPDX-License-Identifier: GPL-3.0-or-later
//! Deleting songs from the library (the owner's request of 18 Nov 2026, DECISIONS
//! D-230, docs/APP_API.md section 2) through the app core, with the fake engine: the
//! loaded or playing song is stopped and closed first (its pending settings written),
//! the file moves into `<root>/Deleted`, the scan never lists it, and it is restored or
//! deleted for good. src/deleted.rs has the file-level tests (name clashes, crash
//! recovery, the copy across volumes, a move failing half-way).

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use jukeoke_lib::core::{Core, CoreOptions};
use jukeoke_lib::engine::fake::{Call, FakeEngine, FakeRenderer};
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::portable::DataLayout;
use jukeoke_lib::testsong::{self, ToneSong};
use serde_json::json;

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

fn app(name: &str) -> App {
    let root = scratch(name);
    std::fs::create_dir_all(root.join("Library/Pop")).unwrap();
    let exe = if cfg!(target_os = "macos") {
        root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke")
    } else {
        root.join("Windows/Jukeoke.exe")
    };
    let layout = DataLayout::detect(&exe, None, || None).unwrap();
    let lib = root.join("Library");
    let short = ToneSong { seconds: 3.0, ..Default::default() };
    testsong::write(&lib.join("Jukeoke - Test Tone.jksong"), short).unwrap();
    testsong::write(&lib.join("Pop/Second.jksong"), short).unwrap();
    std::fs::copy(tiny_song(), lib.join("Tiny.jksong")).unwrap();
    std::fs::write(lib.join("Broken - Song.jksong"), b"this is not a song file at all").unwrap();
    let engine = Arc::new(FakeEngine::default());
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
        studio: Err("No Studio in this test.".into()),
        demo_song: None,
        slot_save_delay: Duration::from_millis(300),
        webview_env_ignored: None,
    });
    App { root, core, engine, sink }
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn saved(path: &std::path::Path) -> jk_format::UserState {
    jk_format::SongFile::open(path).unwrap().user_state().clone()
}

fn names_in(dir: PathBuf) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn deleting_the_playing_song_stops_it_saves_its_settings_then_moves_it() {
    let a = app("delete-playing");
    // The fake engine keeps the file open while it is loaded, like the real one; on
    // Windows without delete sharing, so the move fails unless the song is released first.
    a.engine.hold_open.store(true, Ordering::SeqCst);
    let id = "Jukeoke - Test Tone.jksong";
    let p = &a.core.player;
    p.load(id).unwrap();
    p.play().unwrap();
    p.set(&json!({"semitones": 4})).unwrap(); // pending: written about 300 ms later
    a.engine.take_calls();
    a.sink.clear();
    let r = a.core.library_delete(&ids(&[id])).unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.unloaded.as_deref(), Some(id));
    assert_eq!(r.deleted.len(), 1);
    assert_eq!(r.deleted[0].deleted_id, id);
    // Stopped and closed.
    let calls = a.engine.take_calls();
    assert!(calls.contains(&Call::Pause) && calls.contains(&Call::Unload), "{calls:?}");
    let st = p.state();
    assert_eq!((st.state, st.song_id), ("empty", None));
    assert_eq!(p.set(&json!({"semitones": 1})).unwrap_err().code, "not_loaded");
    // Moved, with the pending key change written into it first.
    assert!(!a.root.join("Library").join(id).exists());
    let moved = a.root.join("Deleted").join(id);
    assert_eq!(saved(&moved).settings.semitones, 4);
    std::thread::sleep(Duration::from_millis(600)); // no late save brings the file back
    assert!(!a.root.join("Library").join(id).exists());
    // The UI hears about it.
    let states = a.sink.of("playback://state");
    assert_eq!(states.last().unwrap()["state"], "empty");
    assert_eq!(a.sink.of("library://deleted").last().unwrap()["count"], 1);
    assert!(!a.sink.of("library://changed").is_empty());
}

/// Verifier finding: `song_load` and `library_delete` of the same song sent together
/// could load the song after it was released and before its file moved, leaving the
/// player on a file in `Deleted/`. Now the move happens while no load can run.
#[test]
fn a_load_racing_with_the_delete_never_leaves_a_deleted_song_loaded() {
    let a = app("delete-load-race");
    let lib = a.root.join("Library");
    for i in 0..12 {
        let id = format!("Race {i}.jksong");
        std::fs::copy(lib.join("Tiny.jksong"), lib.join(&id)).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (core, b, lid) = (a.core.clone(), barrier.clone(), id.clone());
        let loader = std::thread::spawn(move || {
            b.wait();
            core.player.load(&lid).is_ok()
        });
        barrier.wait();
        if i % 2 == 1 {
            std::thread::sleep(Duration::from_micros(200 * i as u64));
        }
        let r = a.core.library_delete(&ids(&[&id])).unwrap();
        let loaded_ok = loader.join().unwrap();
        let in_deleted = a.root.join("Deleted").join(&id).is_file();
        let in_library = lib.join(&id).is_file();
        assert!(in_deleted != in_library, "{id}: in one place only");
        let st = a.core.player.state();
        if st.song_id.as_deref() == Some(id.as_str()) {
            assert!(in_library, "{id}: loaded from a file that moved to Deleted/ (load ok {loaded_ok}, {r:?})");
        }
    }
}

#[test]
fn deleting_another_song_leaves_the_playing_one_alone() {
    let a = app("delete-other");
    a.engine.hold_open.store(true, Ordering::SeqCst);
    a.core.player.load("Pop/Second.jksong").unwrap();
    a.core.player.play().unwrap();
    a.engine.take_calls();
    let r = a.core.library_delete(&ids(&["Tiny.jksong"])).unwrap();
    assert_eq!((r.deleted.len(), r.unloaded), (1, None));
    assert!(!a.engine.take_calls().contains(&Call::Unload));
    assert_eq!(a.core.player.state().state, "playing");
    // On Windows the loaded file cannot be moved while the engine holds it without
    // delete sharing: the failure says so and the song stays in the Library. (This is
    // why the core releases the song first.)
    #[cfg(windows)]
    {
        let path = a.root.join("Library/Pop/Second.jksong");
        let (moved, failed) = a.core.deleted.delete(vec![jukeoke_lib::deleted::ToDelete {
            song_id: "Pop/Second.jksong".into(),
            summary: a.core.library.summary(&path),
            path: path.clone(),
        }]);
        assert!(moved.is_empty());
        assert_eq!(failed[0].code, "in_use", "{failed:?}");
        assert!(path.is_file());
        assert_eq!(a.core.library_deleted_list().count, 1);
    }
}

#[test]
fn deleted_songs_are_listed_restored_and_deleted_for_good() {
    let a = app("delete-flow");
    let before = a.core.library_list().unwrap();
    assert_eq!((before.deleted_count, before.deleted_bytes), (0, 0));
    assert!(!a.root.join("Deleted").exists(), "created on the first delete only");
    let size = |p: &str| std::fs::metadata(a.root.join("Library").join(p)).unwrap().len();
    let (s1, s2, s3) = (size("Tiny.jksong"), size("Pop/Second.jksong"), size("Broken - Song.jksong"));
    // Several at once, a damaged file among them, plus ids that are not songs.
    let r = a
        .core
        .library_delete(&ids(&["Tiny.jksong", "Pop/Second.jksong", "Broken - Song.jksong", "Nope.jksong", "../x.jksong"]))
        .unwrap();
    assert_eq!(r.deleted.len(), 3);
    let failed: Vec<(&str, &str)> = r.failed.iter().map(|f| (f.id.as_str(), f.code.as_str())).collect();
    assert_eq!(failed, [("Nope.jksong", "not_found"), ("../x.jksong", "not_found")]);
    let broken = r.deleted.iter().find(|d| d.deleted_id == "Broken - Song.jksong").unwrap();
    assert_eq!((broken.artist.as_str(), broken.title.as_str()), ("Broken", "Song"));
    // The Library no longer lists them; its header counts what Deleted songs holds.
    let after = a.core.library_list().unwrap();
    let left: Vec<&str> = after.songs.iter().map(|s| s.song_id.as_str()).collect();
    assert_eq!(left, ["Jukeoke - Test Tone.jksong"]);
    assert_eq!((after.deleted_count, after.deleted_bytes), (3, s1 + s2 + s3));
    let v = serde_json::to_value(&after).unwrap();
    assert!(v.get("deleted_count").is_some() && v.get("deleted_bytes").is_some());
    // DeletedList / DeletedSong shape (APP_API section 2).
    let list = a.core.library_deleted_list();
    let v = serde_json::to_value(&list).unwrap();
    for k in ["songs", "count", "total_bytes", "folder"] {
        assert!(v.get(k).is_some(), "DeletedList.{k} missing");
    }
    for k in ["deleted_id", "original_song_id", "restore_to", "title", "artist", "album", "duration_s", "file_bytes", "deleted_ms"] {
        assert!(v["songs"][0].get(k).is_some(), "DeletedSong.{k} missing");
    }
    let second = list.songs.iter().find(|s| s.original_song_id == "Pop/Second.jksong").unwrap();
    assert_eq!(second.deleted_id, "Second.jksong");
    assert!((second.duration_s - 3.0).abs() < 1e-9);
    let info = serde_json::to_value(a.core.app_info("test".into())).unwrap();
    assert_eq!(PathBuf::from(info["paths"]["deleted"].as_str().unwrap()), a.root.join("Deleted"));

    // Restore: back where it was and listed again (no watcher runs here; the core
    // sends library://changed itself).
    a.sink.clear();
    let r = a.core.library_restore(&ids(&["Second.jksong", "gone.jksong"])).unwrap();
    assert_eq!(r.restored.len(), 1);
    assert_eq!(r.restored[0].song_id, "Pop/Second.jksong");
    assert_eq!(r.failed[0].code, "not_found");
    assert!(a.core.library.resolve("Pop/Second.jksong").is_ok());
    assert_eq!(a.sink.of("library://deleted").last().unwrap()["count"], 2);
    assert!(!a.sink.of("library://changed").is_empty());
    // A restore to a place that is taken again gets a free name next to it.
    std::fs::copy(a.root.join("Library/Jukeoke - Test Tone.jksong"), a.root.join("Library/Tiny.jksong")).unwrap();
    let r = a.core.library_restore(&ids(&["Tiny.jksong"])).unwrap();
    assert_eq!(r.restored[0].song_id, "Tiny (2).jksong");
    let lib = a.core.library_list().unwrap();
    let tiny = lib.songs.iter().find(|s| s.song_id == "Tiny (2).jksong").expect("restored song listed");
    assert!(tiny.error.is_none());

    // Delete for good: one, then the rest; the space comes back.
    let r = a.core.library_purge(Some(&ids(&["Broken - Song.jksong"]))).unwrap();
    assert_eq!((r.purged.len(), r.freed_bytes), (1, s3));
    assert!(!a.root.join("Deleted/Broken - Song.jksong").exists());
    assert_eq!(a.core.library_purge(Some(&[])).unwrap_err().code, "bad_request");
    assert_eq!(a.core.library_delete(&[]).unwrap_err().code, "bad_request");
    assert_eq!(a.core.library_restore(&[]).unwrap_err().code, "bad_request");
    a.core.library_delete(&ids(&["Tiny (2).jksong"])).unwrap();
    let r = a.core.library_purge(None).unwrap();
    assert_eq!(r.purged.len(), 1);
    assert_eq!(a.core.library_deleted_list().count, 0);
    assert_eq!(names_in(a.root.join("Deleted")), ["index.json"]);
    let last = a.core.library_list().unwrap();
    assert_eq!((last.deleted_count, last.deleted_bytes), (0, 0));
}

#[test]
fn a_song_put_into_deleted_by_hand_is_never_scanned_but_can_be_restored() {
    let a = app("delete-by-hand");
    std::fs::create_dir_all(a.root.join("Deleted")).unwrap();
    std::fs::copy(tiny_song(), a.root.join("Deleted/Old - Song.jksong")).unwrap();
    let list = a.core.library_list().unwrap();
    assert!(list.songs.iter().all(|s| !s.song_id.contains("Old - Song")));
    assert_eq!(list.deleted_count, 1, "adopted");
    let r = a.core.library_restore(&ids(&["Old - Song.jksong"])).unwrap();
    assert_eq!(r.restored[0].song_id, "Old - Song.jksong");
    assert!(a.core.library.resolve("Old - Song.jksong").is_ok());
}

#[test]
fn the_library_watcher_refreshes_after_delete_and_restore() {
    let a = app("delete-watch");
    let watch = Arc::new(RecordingSink::default());
    let _w = jukeoke_lib::library::Watcher::start(&a.root.join("Library"), watch.clone()).expect("watcher");
    std::thread::sleep(Duration::from_millis(300));
    a.core.library_delete(&ids(&["Pop/Second.jksong"])).unwrap();
    let seen = wait_for(Duration::from_secs(5), || (!watch.of("library://changed").is_empty()).then_some(()));
    assert!(seen.is_some(), "no library://changed from the watcher after a delete");
    watch.clear();
    a.core.library_restore(&ids(&["Second.jksong"])).unwrap();
    let seen = wait_for(Duration::from_secs(5), || (!watch.of("library://changed").is_empty()).then_some(()));
    assert!(seen.is_some(), "no library://changed from the watcher after a restore");
    // Deleted/ is not watched: deleting for good changes nothing in the Library.
    a.core.library_delete(&ids(&["Tiny.jksong"])).unwrap();
    std::thread::sleep(Duration::from_millis(900));
    watch.clear();
    a.core.library_purge(None).unwrap();
    std::thread::sleep(Duration::from_millis(900));
    assert!(watch.of("library://changed").is_empty());
    let _ = &a.sink;
}

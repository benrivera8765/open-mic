// SPDX-License-Identifier: GPL-3.0-or-later
//! The recorder's media-player modes (the owner's request of 18 Nov 2026): "Record &
//! play", "Record songs separately", automatic stops, recovery of an interrupted
//! session, the Studio hand-off metadata. The player is a fake (src/medialink.rs
//! `fake`): no real app is ever listed, played or paused. The capture is the real
//! backend (it records whatever the computer plays; the keep-alive stream adds only
//! digital silence; nothing is played by these tests).

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use jk_recorder::media::PlaybackStatus;
use jk_recorder::session::{
    MASTER_FILE, Marker, MarkerKind, PlayerRef, SessionFile, SessionOptions, SessionState, SplitSettings,
};
use jk_recorder::wav::{WavSpec, WavWriter};
use jukeoke_lib::events::RecordingSink;
use jukeoke_lib::medialink::fake::FakeMedia;
use jukeoke_lib::recorder::RecorderCtl;
use jukeoke_lib::recsession::{SessionStartOptions, SessionView};
use serde_json::json;

const PLAYER: &str = "TestPlayer.exe";
const JPEG: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F'];

struct Rig {
    root: std::path::PathBuf,
    rec: RecorderCtl,
    media: Arc<FakeMedia>,
    sink: Arc<RecordingSink>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.rec.shutdown();
        self.rec.wait_for_splits(Duration::from_secs(30));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Short songs are fine in these tests (the default calls anything under 5 s a skip).
fn rig(name: &str) -> Rig {
    let root = scratch(name);
    let media = FakeMedia::new();
    media.add_player(PLAYER, "Song One", "Band", Some(200.0));
    media.set_thumbnail(JPEG.to_vec());
    let sink = Arc::new(RecordingSink::default());
    let rec = RecorderCtl::with_options(
        root.join("Recordings"),
        &root.join("Settings"),
        sink.clone(),
        media.clone(),
        SplitSettings {
            min_song_s: 0.5,
            ..SplitSettings::default()
        },
    );
    Rig { root, rec, media, sink }
}

/// The capture backend is missing in this build (not Windows / macOS).
fn no_backend(e: &jukeoke_lib::error::ApiError) -> bool {
    if e.code == "not_available" {
        eprintln!("SKIPPED: the recorder backend is not available here: {}", e.message);
        return true;
    }
    false
}

fn opts(idle: f64, max_songs: Option<u32>) -> SessionStartOptions {
    SessionStartOptions {
        player_id: PLAYER.into(),
        auto_stop_idle_s: Some(idle),
        max_songs,
        start_player: Some(true),
        pause_player_at_end: Some(true),
    }
}

fn only_session(r: &Rig) -> SessionView {
    let list = r.rec.sessions_list().unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    list.into_iter().next().unwrap()
}

fn sleep_s(s: f64) {
    std::thread::sleep(Duration::from_secs_f64(s));
}

#[test]
fn media_players_are_listed_and_unknown_players_refused() {
    let r = rig("players");
    let v = r.rec.media_players();
    assert!(v.available);
    assert_eq!(v.players.len(), 1);
    let p = &v.players[0];
    assert_eq!((p.id.as_str(), p.app_name.as_str(), p.status), (PLAYER, "TestPlayer", "paused"));
    assert_eq!((p.title.as_str(), p.artist.as_str(), p.duration_s), ("Song One", "Band", Some(200.0)));
    let e = r.rec.record_and_play("Nobody.exe").unwrap_err();
    assert_eq!(e.code, "player_not_found");
    assert!(e.message.contains("Record button"), "offers plain recording: {}", e.message);
    let e = r
        .rec
        .session_start(&SessionStartOptions {
            player_id: "Nobody.exe".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(e.code, "player_not_found");
    assert!(r.media.calls().is_empty(), "nothing was played");
    assert!(r.rec.sessions_list().unwrap().is_empty(), "no session folder was left");
}

#[test]
fn record_and_play_starts_the_capture_first_then_the_player() {
    let r = rig("rap");
    // At the moment the player is told to play, the capture must already run.
    let seen = Arc::new(Mutex::new(None));
    {
        let (seen, sink) = (seen.clone(), r.sink.clone());
        r.media.on_play(move || {
            *seen.lock().unwrap() = sink.of("recorder://level").last().cloned();
        });
    }
    let st = match r.rec.record_and_play(PLAYER) {
        Ok(st) => st,
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(st.state, "recording");
    assert_eq!(st.mode, "record_and_play");
    assert_eq!(st.player.as_ref().unwrap().id, PLAYER);
    assert!(st.note.is_none(), "{:?}", st.note);
    assert_eq!(r.media.calls(), [format!("play {PLAYER}")]);
    let at_play = seen.lock().unwrap().clone().expect("a level reading before play");
    assert_eq!(at_play["state"], "recording", "{at_play}");
    assert!(at_play["format"].is_object(), "capture format known before play: {at_play}");
    assert!(at_play["elapsed_s"].as_f64().unwrap() > 0.0, "{at_play}");
    // A second recording is refused while this one runs.
    assert_eq!(r.rec.start().unwrap_err().code, "busy");
    sleep_s(0.5);
    let live = r.rec.status();
    assert_eq!(live.mode, "record_and_play");
    // Stop & pause.
    let rec = r.rec.stop_and_pause(true).unwrap();
    assert!(rec.name.starts_with("Recording "), "{}", rec.name);
    assert!(rec.duration_s > 0.3, "{rec:?}");
    assert!(rec.note.is_none());
    assert_eq!(r.media.calls(), [format!("play {PLAYER}"), format!("pause {PLAYER}")]);
    assert_eq!(r.rec.status().state, "idle");
    assert_eq!(r.rec.status().mode, "plain");
}

#[test]
fn record_and_play_keeps_recording_when_the_player_refuses() {
    let r = rig("refuse");
    r.media.refuse_play(true);
    let st = match r.rec.record_and_play(PLAYER) {
        Ok(st) => st,
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(st.state, "recording");
    let note = st.note.expect("a note says the player did not start");
    assert!(note.contains("recording is running") && note.contains("TestPlayer"), "{note}");
    // Plain stop does not touch the player.
    r.rec.stop().unwrap();
    assert_eq!(r.media.calls(), [format!("play {PLAYER}")]);
}

#[test]
fn session_follows_tracks_and_splits_into_songs() {
    let r = rig("session");
    let st = match r.rec.session_start(&opts(30.0, None)) {
        Ok(st) => st,
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(st.mode, "session");
    let live = st.session.clone().expect("live session");
    assert_eq!(live.current.as_ref().unwrap().title, "Song One");
    assert_eq!(r.media.calls(), [format!("play {PLAYER}")], "the paused player was started");
    // Other recordings are refused meanwhile, and the plain stop points to the panel.
    assert_eq!(r.rec.start().unwrap_err().code, "busy");
    assert_eq!(r.rec.stop().unwrap_err().code, "session_running");
    assert_eq!(r.rec.session_start(&opts(30.0, None)).unwrap_err().code, "busy");
    // Changes 2 s apart (closer than 1.5 s counts as one change in two steps).
    sleep_s(2.0);
    r.media.set_track(PLAYER, "Song Two", "Band", Some(1.0));
    sleep_s(2.0);
    r.media.set_track(PLAYER, "Advertisement", "", Some(2.0));
    sleep_s(2.0);
    r.media.set_track(PLAYER, "Song Three: \"Live\"", "Other/Band", None);
    sleep_s(2.0);
    let live = r.rec.status().session.unwrap();
    assert_eq!(live.songs, 3, "ads are not counted");
    assert_eq!(live.current.unwrap().artist, "Other/Band");
    assert_eq!(live.playback, "playing");
    let during = only_session(&r);
    assert_eq!(during.state, "recording");

    let stopped = r.rec.session_stop(true).unwrap();
    assert!(matches!(stopped.state, "splitting" | "split"), "{}", stopped.state);
    assert_eq!(r.media.calls().last().unwrap(), &format!("pause {PLAYER}"));
    assert!(r.rec.wait_for_splits(Duration::from_secs(60)));
    let s = only_session(&r);
    assert_eq!(s.state, "split", "{:?}", s.message);
    assert!(!s.master_present, "master deleted after the verified split");
    assert_eq!(s.stop_reason.as_deref(), Some("user"));
    assert_eq!(s.player, "TestPlayer");
    let titles: Vec<&str> = s.songs.iter().map(|x| x.title.as_str()).collect();
    assert_eq!(titles, ["Song One", "Song Two", "Song Three: \"Live\""]);
    let names: Vec<&str> = s.songs.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["01 Band - Song One.wav", "02 Band - Song Two.wav", "03 Other-Band - Song Three - 'Live'.wav"]);
    assert_eq!(s.excluded.len(), 1);
    assert_eq!(s.excluded[0].reason, "advertisement");
    // Song One was 2 s of a 200 s track: partial. Song Two (1 s track): complete.
    // Song Three: no length, the recording stopped while it played: partial.
    let status: Vec<&str> = s.songs.iter().map(|x| x.status).collect();
    assert_eq!(status, ["partial", "complete", "partial"]);
    assert!(s.songs[2].partial_reason.as_deref().unwrap().contains("stopped during"));
    for song in &s.songs {
        assert!(Path::new(&song.path).is_file());
        assert!((0.4..3.0).contains(&song.duration_s), "{song:?}");
    }
    // Covers: the fake player's picture, fetched shortly after each change.
    let cover = s.songs[0].cover_path.as_deref().expect("cover of song 1");
    assert_eq!(std::fs::read(cover).unwrap(), JPEG);
    // Events: the live session in the level readings, the session list changes.
    assert!(r.sink.of("recorder://level").iter().any(|v| v["mode"] == "session" && v["session"]["songs"] == 3));
    assert!(r.sink.of("recorder://session").iter().any(|v| v["state"] == "split"));

    // Studio hand-off: inspect fills what a WAV cannot hold from the sidecar.
    let mut insp = json!({"ok": true, "title": null, "artist": null, "album": null, "cover_path": null});
    jukeoke_lib::studio::merge_recorded_song(&mut insp, Path::new(&s.songs[1].path));
    assert_eq!(insp["title"], "Song Two");
    assert_eq!(insp["artist"], "Band");
    assert_eq!(insp["album"], "Album");
    assert_eq!(insp["cover_path"], json!(s.songs[1].cover_path.clone().unwrap()));
    assert_eq!(insp["recorded"]["status"], "complete");
    assert_eq!(insp["recorded"]["source_player"], "TestPlayer");
    // What the file itself says wins.
    let mut tagged = json!({"title": "Own title", "artist": "", "cover_path": null});
    jukeoke_lib::studio::merge_recorded_song(&mut tagged, Path::new(&s.songs[1].path));
    assert_eq!(tagged["title"], "Own title");
    assert_eq!(tagged["artist"], "Band");
    // What the real Studio returns for an untagged WAV: the file name as the title
    // (`title_from_tags: false`). The player's title replaces it.
    let stem = Path::new(&s.songs[1].path).file_stem().unwrap().to_string_lossy().into_owned();
    let mut untagged = json!({"ok": true, "title": stem, "artist": null, "album": null, "cover_path": null, "title_from_tags": false});
    jukeoke_lib::studio::merge_recorded_song(&mut untagged, Path::new(&s.songs[1].path));
    assert_eq!(untagged["title"], "Song Two", "not the file name {stem:?}");
    assert_eq!(untagged["artist"], "Band");
    // A splitting that is finished cannot be split again (the master is gone).
    assert_eq!(r.rec.session_split(&s.folder).unwrap_err().code, "nothing_to_split");
    assert_eq!(r.rec.session_split("..").unwrap_err().code, "bad_request");
}

#[test]
fn session_stops_itself_when_the_player_stays_paused() {
    let r = rig("idle");
    match r.rec.session_start(&opts(1.0, None)) {
        Ok(_) => {}
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    }
    sleep_s(0.6);
    r.media.set_status(PLAYER, PlaybackStatus::Paused);
    let done = wait_for(Duration::from_secs(10), || (r.rec.status().state == "idle").then_some(()));
    assert!(done.is_some(), "stopped by itself after 1 s paused");
    assert!(r.rec.wait_for_splits(Duration::from_secs(30)));
    let s = only_session(&r);
    assert_eq!(s.stop_reason.as_deref(), Some("idle"));
    assert_eq!(s.songs.len(), 1);
    assert_eq!(s.excluded.last().map(|p| p.reason.as_str()), Some("after playback stopped"));
    // Already paused: the session did not send another pause.
    assert_eq!(r.media.calls(), [format!("play {PLAYER}")]);
}

#[test]
fn session_stops_at_the_song_limit_and_pauses_the_player() {
    let r = rig("limit");
    match r.rec.session_start(&opts(30.0, Some(1))) {
        Ok(_) => {}
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    }
    sleep_s(1.0);
    r.media.set_track(PLAYER, "Song Two", "Band", Some(100.0));
    let done = wait_for(Duration::from_secs(10), || (r.rec.status().state == "idle").then_some(()));
    assert!(done.is_some());
    assert!(r.rec.wait_for_splits(Duration::from_secs(30)));
    let s = only_session(&r);
    assert_eq!(s.stop_reason.as_deref(), Some("max_songs"));
    assert_eq!(s.songs.len(), 1);
    assert_eq!(s.songs[0].title, "Song One");
    assert_eq!(r.media.calls(), [format!("play {PLAYER}"), format!("pause {PLAYER}")]);
}

#[test]
fn session_stops_when_the_player_is_closed() {
    let r = rig("closed");
    match r.rec.session_start(&opts(0.5, None)) {
        Ok(_) => {}
        Err(e) if no_backend(&e) => return,
        Err(e) => panic!("{e:?}"),
    }
    sleep_s(0.8);
    r.media.close(PLAYER);
    let done = wait_for(Duration::from_secs(10), || (r.rec.status().state == "idle").then_some(()));
    assert!(done.is_some());
    assert!(r.rec.wait_for_splits(Duration::from_secs(30)));
    let s = only_session(&r);
    assert_eq!(s.stop_reason.as_deref(), Some("player_closed"));
}

/// A session a crash left behind: master written up to the crash, markers saved,
/// state "recording". The next start finishes it.
#[test]
fn an_interrupted_session_is_split_at_the_next_start() {
    let r = rig("recover");
    let dir = r.root.join("Recordings").join("2026-11-18 10-00-00");
    std::fs::create_dir_all(&dir).unwrap();
    let rate = 48_000u32;
    let mut data = Vec::new();
    for (secs, amp) in [(3.0, 0.3f32), (1.0, 0.0), (3.0, 0.3)] {
        for i in 0..(secs * rate as f64) as usize {
            let x = amp * (i as f32 * 0.03).sin();
            data.extend([x, x]);
        }
    }
    let mut w = WavWriter::create(&dir.join(MASTER_FILE), WavSpec::new(rate, 2)).unwrap();
    w.write_interleaved(&data[..data.len() / 2]).unwrap();
    w.finalize().unwrap();
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(dir.join(MASTER_FILE)).unwrap();
        let rest: Vec<u8> = data[data.len() / 2..].iter().flat_map(|x| x.to_le_bytes()).collect();
        f.write_all(&rest).unwrap();
    }
    let mut s = SessionFile::new(
        "2026-11-18 10-00-00",
        PlayerRef {
            source_app_id: PLAYER.into(),
            app_name: "TestPlayer".into(),
        },
        SessionOptions::default(),
    );
    s.sample_rate = rate;
    s.channels = 2;
    s.markers = vec![
        Marker {
            kind: MarkerKind::Track,
            frame: 100,
            title: "First".into(),
            artist: "Band".into(),
            initial: true,
            initial_status: Some("paused".into()),
            ..Marker::default()
        },
        Marker {
            kind: MarkerKind::Playing,
            frame: 200,
            ..Marker::default()
        },
        Marker {
            kind: MarkerKind::Track,
            frame: 4 * rate as u64 + 9_600,
            title: "Second".into(),
            artist: "Band".into(),
            ..Marker::default()
        },
    ];
    s.save(&dir).unwrap();
    assert_eq!(only_session(&r).state, "interrupted");
    r.rec.recover_sessions();
    let v = wait_for(Duration::from_secs(30), || {
        let v = only_session(&r);
        (v.state == "split").then_some(v)
    })
    .expect("split at the next start");
    assert_eq!(v.state, "split", "{:?}", v.message);
    assert_eq!(v.stop_reason.as_deref(), Some("interrupted"));
    let titles: Vec<&str> = v.songs.iter().map(|x| x.title.as_str()).collect();
    assert_eq!(titles, ["First", "Second"]);
    let total: f64 = v.songs.iter().map(|x| x.duration_s).sum();
    assert!((total - 7.0).abs() < 1e-6, "everything on disk is in the songs: {total}");
    assert_eq!(v.songs[1].start_confidence, "certain", "cut in the 1 s gap");
    assert_eq!(SessionFile::load(&dir).unwrap().state, SessionState::Split);
}

#[test]
fn preview_plays_only_recordings() {
    let r = rig("preview");
    std::fs::create_dir_all(r.root.join("Recordings")).unwrap();
    let outside = r.root.join("outside.wav");
    std::fs::write(&outside, b"RIFF").unwrap();
    let e = r.rec.preview(outside.to_str().unwrap(), 0.0, 5.0).unwrap_err();
    assert_eq!(e.code, "bad_request");
    let e = r.rec.preview(r.root.join("Recordings/none.wav").to_str().unwrap(), 0.0, 5.0).unwrap_err();
    assert_eq!(e.code, "not_found");
    r.rec.preview_stop(); // nothing playing: fine
}

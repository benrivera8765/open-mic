// SPDX-License-Identifier: GPL-3.0-or-later
//! Splitter, markers, session file and recovery, on generated audio.

use std::path::{Path, PathBuf};

use super::split::{FrameSource, MemSource, Owner, plan, quiet_runs};
use super::*;
use crate::media::{PlaybackStatus, TrackInfo};
use crate::wav::{WavSpec, WavWriter, read_wav_f32};

const RATE: u32 = 48_000;

fn scratch(name: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("jk-session-test-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Stereo test signal with known boundaries.
struct Synth {
    data: Vec<f32>,
    seed: u32,
}

impl Synth {
    fn new() -> Synth {
        Synth { data: Vec::new(), seed: 0x1234_5678 }
    }
    fn frames(&self) -> u64 {
        (self.data.len() / 2) as u64
    }
    fn secs(s: f64) -> u64 {
        (s * f64::from(RATE)).round() as u64
    }
    /// A "song": two sines plus a little noise, peak about `amp`.
    fn song(&mut self, secs: f64, freq: f64, amp: f32) -> (u64, u64) {
        let start = self.frames();
        for i in 0..Self::secs(secs) {
            let t = i as f64 / f64::from(RATE);
            self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (self.seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let s = (0.6 * (std::f64::consts::TAU * freq * t).sin() + 0.3 * (std::f64::consts::TAU * freq * 1.5 * t).sin()) as f32;
            let x = amp * (s + 0.1 * noise * 2.0);
            self.data.push(x);
            self.data.push(-0.8 * x);
        }
        (start, self.frames())
    }
    fn silence(&mut self, secs: f64) -> (u64, u64) {
        let start = self.frames();
        self.data.extend(std::iter::repeat_n(0.0, 2 * Self::secs(secs) as usize));
        (start, self.frames())
    }
}

fn write_master(dir: &Path, data: &[f32]) {
    let mut w = WavWriter::create(&dir.join(MASTER_FILE), WavSpec::new(RATE, 2)).unwrap();
    w.write_interleaved(data).unwrap();
    w.finalize().unwrap();
}

fn track(frame: u64, title: &str, artist: &str, duration_s: Option<f64>) -> Marker {
    Marker {
        kind: MarkerKind::Track,
        frame,
        title: title.into(),
        artist: artist.into(),
        album: "Test album".into(),
        duration_s,
        ..Marker::default()
    }
}

fn status(kind: MarkerKind, frame: u64) -> Marker {
    Marker {
        kind,
        frame,
        ..Marker::default()
    }
}

fn session(markers: Vec<Marker>, stop_reason: &str) -> SessionFile {
    let mut s = SessionFile::new(
        "2026-11-18 14-03-22",
        PlayerRef {
            source_app_id: "TestPlayer.exe".into(),
            app_name: "TestPlayer".into(),
        },
        SessionOptions::default(),
    );
    s.sample_rate = RATE;
    s.channels = 2;
    s.state = SessionState::Stopped;
    s.stop_reason = Some(stop_reason.into());
    s.markers = markers;
    s
}

fn ms(frames: i64) -> f64 {
    frames as f64 * 1000.0 / f64::from(RATE)
}

/// The scenario of the live test, generated: 3 gaps, 1 gapless join, 1 track
/// skipped half-way, an advertisement, then playback pauses and the session runs
/// on in silence until it stops itself.
struct Scenario {
    master: Vec<f32>,
    session: SessionFile,
    /// (song start, song end) of each piece of audio, in order.
    pieces: Vec<(u64, u64)>,
    gaps: Vec<(u64, u64)>,
}

fn scenario(amp: f32) -> Scenario {
    let f = |s: f64| Synth::secs(s);
    let mut y = Synth::new();
    let _lead = y.silence(0.3); // capture started before playback
    let a = y.song(20.0, 220.0, amp);
    let g1 = y.silence(2.0);
    let b = y.song(15.0, 330.0, amp);
    let g2 = y.silence(0.8);
    let ad = y.song(8.0, 990.0, amp);
    let g3 = y.silence(1.0);
    let c = y.song(12.0, 440.0, amp);
    let d = y.song(18.0, 550.0, amp); // gapless after C
    let g4 = y.silence(1.0);
    let e = y.song(10.0, 660.0, amp); // a 60 s track skipped after 10 s
    let ff = y.song(10.0, 770.0, amp); // gapless: the skip
    let g5 = y.silence(30.0); // paused; the session stops itself
    let initial = Marker {
        initial: true,
        initial_status: Some("paused".into()),
        position_s: Some(0.0),
        ..track(0, "Song A", "Band", Some(20.0))
    };
    let markers = vec![
        initial,
        status(MarkerKind::Playing, f(0.05)),
        track(b.0 + f(0.4), "Song B", "Band", Some(15.0)), // player late by 0.4 s
        track(ad.0 - f(0.3), "Advertisement", "", Some(8.0)), // early by 0.3 s
        track(c.0 + f(1.2), "Song C", "Other Band", Some(12.0)), // late by 1.2 s
        track(d.0 + f(0.35), "Song D", "Other Band", Some(18.0)), // gapless, late 0.35 s
        track(e.0 + f(0.2), "Song E", "Band", Some(60.0)),
        track(ff.0 + f(0.25), "Song F", "Band", Some(10.0)), // the skip
        status(MarkerKind::Paused, ff.1 + f(0.3)),
    ];
    let mut s = session(markers, "idle");
    s.markers[2].cover = Some("covers/2.jpg".into());
    Scenario {
        master: y.data,
        session: s,
        pieces: vec![a, b, ad, c, d, e, ff],
        gaps: vec![g1, g2, g3, g4, g5],
    }
}

fn concat_parts(dir: &Path, s: &SessionFile) -> Vec<f32> {
    let mut all = Vec::new();
    for p in &s.parts {
        let (_, data) = read_wav_f32(&dir.join(&p.file)).unwrap();
        all.extend(data);
    }
    all
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn run_scenario(amp: f32) {
    let sc = scenario(amp);
    let dir = scratch(&format!("scenario-{}", (amp * 1e4) as u32));
    write_master(&dir, &sc.master);
    std::fs::create_dir_all(dir.join(COVERS_DIR)).unwrap();
    std::fs::write(dir.join("covers/2.jpg"), [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
    sc.session.save(&dir).unwrap();

    let out = split(&dir, &SplitSettings::default()).unwrap();
    let s = &out.session;
    assert_eq!(s.state, SessionState::Split, "{:?}", s.message);
    assert!(out.master_deleted);
    assert!(!dir.join(MASTER_FILE).exists(), "master deleted after a verified split");
    assert!(!dir.join(COVERS_DIR).exists());
    // Bit for bit: the parts, in order, are the master.
    assert_eq!(bits(&concat_parts(&dir, s)), bits(&sc.master));
    assert!(s.parts.iter().all(|p| p.verified));

    let kinds: Vec<(PartKind, Option<&str>)> = s.parts.iter().map(|p| (p.kind, p.reason.as_deref())).collect();
    assert_eq!(
        kinds,
        [
            (PartKind::Song, None),
            (PartKind::Song, None),
            (PartKind::Excluded, Some("advertisement")),
            (PartKind::Song, None),
            (PartKind::Song, None),
            (PartKind::Song, None),
            (PartKind::Song, None),
            (PartKind::Excluded, Some("after playback stopped")),
        ]
    );
    let names: Vec<&str> = s.parts.iter().map(|p| p.file.as_str()).collect();
    assert_eq!(
        names,
        [
            "01 Band - Song A.wav",
            "02 Band - Song B.wav",
            "_excluded/Part 03 - advertisement - Advertisement.wav",
            "03 Other Band - Song C.wav",
            "04 Other Band - Song D.wav",
            "05 Band - Song E.wav",
            "06 Band - Song F.wav",
            "_excluded/Part 08 - after playback stopped.wav",
        ]
    );

    // Cuts at gaps: in the middle of the gap. Report the error against the true
    // boundary (the next piece's first sample) and against the gap's middle.
    let cut = |i: usize| s.parts[i].start_frame;
    let mid = |g: (u64, u64)| (g.0 + g.1) / 2;
    for (part, gap) in [(1usize, sc.gaps[0]), (2, sc.gaps[1]), (3, sc.gaps[2]), (5, sc.gaps[3])] {
        let c = cut(part) as i64;
        let err_mid = c - mid(gap) as i64;
        eprintln!(
            "amp {amp}: cut {part}: {:.1} ms from the gap middle, {:.1} ms before the next onset (gap {:.0} ms)",
            ms(err_mid),
            ms(gap.1 as i64 - c),
            ms((gap.1 - gap.0) as i64)
        );
        assert!(ms(err_mid.abs()) < 5.0, "cut {part} {err_mid} frames from the gap middle");
        assert!(c as u64 > gap.0 && (c as u64) < gap.1, "cut {part} inside the gap");
    }
    // Gapless join (C|D) and the skip (E|F): at the player's event, uncertain.
    for (part, truth, late) in [(4usize, sc.pieces[4].0, 0.35), (6, sc.pieces[6].0, 0.25)] {
        let err = cut(part) as i64 - truth as i64;
        eprintln!("amp {amp}: cut {part} (no gap): {:.1} ms after the true join", ms(err));
        assert!((ms(err) - late * 1000.0).abs() < 1.0);
    }
    // Playback ended: 1 s into the final silence.
    let tail = s.parts[7].start_frame;
    let keep = tail as i64 - sc.gaps[4].0 as i64;
    eprintln!("amp {amp}: playback ended: {:.1} ms of the final silence kept with the last song", ms(keep));
    assert!((ms(keep) - 1000.0).abs() < 5.0, "about 1 s of the trailing silence stays with the last song");

    // Sidecars.
    let side: Vec<&SongSidecar> = out.songs.iter().map(|(_, sc)| sc).collect();
    assert_eq!(side.len(), 6);
    let st: Vec<SongStatus> = side.iter().map(|x| x.status).collect();
    assert_eq!(
        st,
        [
            SongStatus::Complete,
            SongStatus::Complete,
            SongStatus::Complete,
            SongStatus::Complete,
            SongStatus::Partial,
            SongStatus::Complete
        ]
    );
    assert!(side[4].partial_reason.as_deref().unwrap().contains("skipped"));
    let conf = |x: &SongSidecar| (x.start.confidence, x.end.confidence);
    assert_eq!(conf(side[0]), (Confidence::Edge, Confidence::Certain));
    assert_eq!(conf(side[2]), (Confidence::Certain, Confidence::Uncertain));
    assert_eq!(conf(side[3]), (Confidence::Uncertain, Confidence::Certain));
    assert_eq!(conf(side[5]), (Confidence::Uncertain, Confidence::Certain));
    assert_eq!(side[1].cover.as_deref(), Some("02 Band - Song B.jpg"));
    assert!(dir.join("02 Band - Song B.jpg").is_file());
    assert_eq!(side[1].title, "Song B");
    assert_eq!(side[1].album, "Test album");
    assert_eq!(side[1].track_duration_s, Some(15.0));
    assert_eq!(side[0].source_player.app_name, "TestPlayer");
    // The last song ends with the pause that ended playback: not a pause inside it.
    assert_eq!(side[5].paused_s, 0.0);
    assert_eq!(side[2].number, 3);
    // Each song file holds exactly its own piece of audio (plus silence).
    for (k, (path, sidecar)) in out.songs.iter().enumerate() {
        let piece = [0usize, 1, 3, 4, 5, 6][k];
        let (a, b) = sc.pieces[piece];
        assert!(sidecar.master_start_frame <= a + RATE as u64 / 2 && sidecar.master_end_frame >= b, "song {k}");
        let read = SongSidecar::for_audio(path).unwrap();
        assert_eq!(&read, sidecar);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn split_at_normal_levels() {
    run_scenario(0.5);
}

#[test]
fn split_at_minus_60_dbfs() {
    // The live test's level (people may be around): content peaks near -60 dBFS,
    // the gaps are digital silence. -70 dBFS still tells them apart.
    run_scenario(0.001);
}

#[test]
fn quiet_runs_need_every_channel_quiet_for_100_ms() {
    let thr = 10f32.powf(-70.0 / 20.0);
    let mut v = Vec::new();
    // -60 dBFS sine: never 100 ms below -70 dBFS.
    for i in 0..48_000 {
        let x = 0.001 * (i as f32 * 0.05).sin();
        v.extend([x, x]);
    }
    assert!(quiet_runs(&v, 2, 0, thr, 4_800).is_empty());
    // 150 ms of silence on the left only: not quiet.
    let mut w = vec![0.0f32; 2 * 7_200];
    for f in w.chunks_mut(2) {
        f[1] = 0.01;
    }
    assert!(quiet_runs(&w, 2, 0, thr, 4_800).is_empty());
    // A real gap, and one too short.
    let mut g = v[..9_600].to_vec();
    g.extend(vec![0.0; 2 * 7_200]);
    g.extend(&v[..9_600]);
    g.extend(vec![0.0; 2 * 2_000]);
    g.extend(&v[..9_600]);
    let runs = quiet_runs(&g, 2, 1000, thr, 4_800);
    assert_eq!(runs.len(), 1);
    let (a, b) = runs[0];
    assert!((a as i64 - 5_800).abs() < 20 && (b as i64 - 13_000).abs() < 20, "{runs:?}");
    // A NaN is never quiet.
    let n = vec![f32::NAN; 2 * 6_000];
    assert!(quiet_runs(&n, 2, 0, thr, 4_800).is_empty());
}

#[test]
fn close_track_changes_merge_and_the_first_instant_is_kept() {
    let mut y = Synth::new();
    let a = y.song(10.0, 300.0, 0.5);
    let _g = y.silence(1.0);
    let b = y.song(10.0, 400.0, 0.5);
    let markers = vec![
        Marker { initial: true, ..track(0, "A", "X", None) },
        status(MarkerKind::Playing, 0),
        // The player announces B in two steps 300 ms apart (first a stale title).
        track(b.0 + 4_800, "B (loading)", "X", None),
        track(b.0 + 4_800 + 14_400, "B", "X", Some(10.0)),
    ];
    let s = session(markers, "user");
    let mut src = MemSource { samples: y.data.clone(), channels: 2 };
    let parts = plan(&s, &mut src, &SplitSettings::default()).unwrap();
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert_eq!(parts[1].owner, Owner::Song(Some(3)), "the later metadata wins");
    let gap_mid = (a.1 + b.0) / 2;
    assert!((parts[1].start as i64 - gap_mid as i64).abs() < 10);
}

#[test]
fn song_limit_ends_at_the_next_song_and_starting_mid_song_is_partial() {
    let mut y = Synth::new();
    let a = y.song(10.0, 300.0, 0.5);
    let _ = y.silence(1.0);
    let b = y.song(10.0, 400.0, 0.5);
    let _ = y.silence(1.0);
    let c = y.song(3.0, 500.0, 0.5);
    let markers = vec![
        Marker {
            initial: true,
            initial_status: Some("playing".into()),
            position_s: Some(42.0),
            ..track(0, "A", "X", Some(52.0))
        },
        status(MarkerKind::Playing, 0),
        track(b.0 + 2_000, "B", "X", Some(10.0)),
        track(c.0 + 2_000, "C", "X", Some(200.0)),
    ];
    let dir = scratch("limit");
    let mut s = session(markers, "max_songs");
    s.options.max_songs = Some(2);
    write_master(&dir, &y.data);
    s.save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    let p = &out.session.parts;
    assert_eq!(p.len(), 3, "{p:?}");
    assert_eq!(p[2].reason.as_deref(), Some("after the last song"));
    assert_eq!(out.songs[0].1.status, SongStatus::Partial);
    assert!(out.songs[0].1.partial_reason.as_deref().unwrap().contains("part-way"));
    assert_eq!(out.songs[1].1.status, SongStatus::Complete);
    assert!(p[0].end_frame > a.1 && p[0].end_frame < b.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_player_metadata_still_gives_one_song_and_stop_during_a_song_is_partial() {
    let mut y = Synth::new();
    y.song(6.0, 300.0, 0.5);
    let dir = scratch("nometa");
    write_master(&dir, &y.data);
    session(vec![status(MarkerKind::Playing, 0)], "user").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    assert_eq!(out.session.parts.len(), 1);
    assert_eq!(out.session.parts[0].file, "01 Unknown title.wav");
    assert_eq!(out.songs[0].1.status, SongStatus::Partial, "playing when the recording stopped");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn interrupted_recording_is_repaired_and_split_on_the_next_start() {
    let sc = scenario(0.5);
    let dir = scratch("crash");
    // The app died while recording: the master's header covers only what was
    // checkpointed, the rest is on disk after it (plus half a frame).
    let cut = sc.master.len() / 2 / 2 * 2;
    let mut w = WavWriter::create(&dir.join(MASTER_FILE), WavSpec::new(RATE, 2)).unwrap();
    w.write_interleaved(&sc.master[..cut]).unwrap();
    w.finalize().unwrap();
    {
        use std::io::Write as _;
        let mut f = OpenOptions::new().append(true).open(dir.join(MASTER_FILE)).unwrap();
        let rest: Vec<u8> = sc.master[cut..].iter().flat_map(|x| x.to_le_bytes()).collect();
        f.write_all(&rest).unwrap();
        f.write_all(&[1, 2, 3]).unwrap(); // a torn last write
    }
    let mut s = sc.session.clone();
    s.state = SessionState::Recording;
    s.stop_reason = None;
    s.stop_frame = None;
    s.sample_rate = 0; // not yet known when the app died early
    s.save(&dir).unwrap();
    // Also a temporary file from the crash.
    std::fs::write(dir.join("01 Band - Song A.wav.tmp-0badf00d"), b"junk").unwrap();

    let out = recover(&dir, &SplitSettings::default()).unwrap().expect("something to do");
    assert_eq!(out.session.state, SessionState::Split, "{:?}", out.session.message);
    assert_eq!(out.session.stop_reason.as_deref(), Some("interrupted"));
    assert_eq!(bits(&concat_parts(&dir, &out.session)), bits(&sc.master), "everything on disk was kept");
    assert!(!dir.join("01 Band - Song A.wav.tmp-0badf00d").exists());
    // Nothing more to do the next time.
    assert!(recover(&dir, &SplitSettings::default()).unwrap().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn interrupted_split_resumes_and_keeps_good_files() {
    let sc = scenario(0.5);
    let dir = scratch("resume");
    write_master(&dir, &sc.master);
    sc.session.save(&dir).unwrap();
    let keep = SplitSettings {
        delete_master: false,
        ..SplitSettings::default()
    };
    let first = split(&dir, &keep).unwrap();
    assert!(dir.join(MASTER_FILE).is_file());
    // Crash in the middle of a later run: the session says "splitting", one song is
    // missing, one is damaged, the others are fine.
    let mut s = first.session.clone();
    s.state = SessionState::Splitting;
    s.save(&dir).unwrap();
    let a = dir.join(&s.parts[0].file);
    let b = dir.join(&s.parts[1].file);
    let untouched = dir.join(&s.parts[3].file);
    let before = std::fs::metadata(&untouched).unwrap().modified().unwrap();
    std::fs::remove_file(&a).unwrap();
    let mut bytes = std::fs::read(&b).unwrap();
    let n = bytes.len();
    bytes[n - 100] ^= 0x40;
    std::fs::write(&b, bytes).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));

    let out = recover(&dir, &SplitSettings::default()).unwrap().unwrap();
    assert_eq!(out.session.state, SessionState::Split);
    assert!(out.master_deleted);
    assert_eq!(bits(&concat_parts(&dir, &out.session)), bits(&sc.master));
    assert_eq!(std::fs::metadata(&untouched).unwrap().modified().unwrap(), before, "a good file is not rewritten");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_part_that_no_longer_matches_keeps_the_master() {
    let sc = scenario(0.5);
    let dir = scratch("keep");
    write_master(&dir, &sc.master);
    sc.session.save(&dir).unwrap();
    let keep = SplitSettings {
        delete_master: false,
        ..SplitSettings::default()
    };
    let first = split(&dir, &keep).unwrap();
    assert_eq!(first.session.state, SessionState::Split);
    // Closed after "split" was saved but before the master was deleted, and one
    // song file was damaged meanwhile: the master must stay.
    let b = dir.join(&first.session.parts[1].file);
    let mut bytes = std::fs::read(&b).unwrap();
    let n = bytes.len();
    bytes[n - 4] ^= 1;
    std::fs::write(&b, bytes).unwrap();
    let out = recover(&dir, &SplitSettings::default()).unwrap().unwrap();
    assert_eq!(out.session.state, SessionState::KeptMaster);
    assert!(dir.join(MASTER_FILE).is_file());
    assert!(out.session.message.unwrap().contains("kept"));
    // Splitting again repairs it and then deletes the master.
    let mut s = SessionFile::load(&dir).unwrap();
    s.state = SessionState::Stopped;
    s.save(&dir).unwrap();
    let again = split(&dir, &SplitSettings::default()).unwrap();
    assert_eq!(again.session.state, SessionState::Split);
    assert!(!dir.join(MASTER_FILE).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_file_round_trips_and_tolerates_unknown_fields() {
    let sc = scenario(0.01);
    let dir = scratch("json");
    let mut s = sc.session.clone();
    s.parts.push(PartRecord {
        kind: PartKind::Excluded,
        file: "_excluded/Part 01 - x.wav".into(),
        start_frame: 0,
        end_frame: 10,
        reason: Some("x".into()),
        verified: true,
    });
    s.message = Some("note".into());
    s.options.max_songs = Some(4);
    s.save(&dir).unwrap();
    let back = SessionFile::load(&dir).unwrap();
    assert_eq!(back, s);
    // A newer app may add fields; an older one still reads the file.
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(SESSION_FILE)).unwrap()).unwrap();
    v["future_field"] = serde_json::json!({"a": 1});
    v["markers"][0]["future"] = serde_json::json!(true);
    std::fs::write(dir.join(SESSION_FILE), v.to_string()).unwrap();
    assert_eq!(SessionFile::load(&dir).unwrap(), s);
    // Not a session file.
    std::fs::write(dir.join(SESSION_FILE), r#"{"format":"other"}"#).unwrap();
    assert!(SessionFile::load(&dir).is_err());
    // No temporary files left by the atomic writes.
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left.len(), 1, "{left:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn advertisements_are_recognised() {
    assert!(looks_like_ad("Advertisement", "", "", None));
    assert!(looks_like_ad("Something", "Spotify", "", Some(30.0)));
    assert!(looks_like_ad(" ad ", "x", "", None));
    assert!(looks_like_ad("Brand jingle", "", "", Some(29.0)));
    assert!(!looks_like_ad("Brand jingle", "", "", Some(200.0)));
    assert!(!looks_like_ad("Interlude", "Band", "", Some(20.0)), "a short song with an artist is a song");
    assert!(!looks_like_ad("Adventure", "Band", "", None));
    assert!(!looks_like_ad("", "", "", Some(10.0)), "no metadata is not an ad");
}

fn obs(frame: u64, status: PlaybackStatus, title: &str, artist: &str) -> Observation {
    Observation {
        frame,
        estimate_frame: None,
        status,
        track: TrackInfo {
            title: title.into(),
            artist: artist.into(),
            ..TrackInfo::default()
        },
        gone: false,
    }
}

#[test]
fn marker_builder_rules() {
    let mut b = MarkerBuilder::default();
    let r = b.observe(&obs(0, PlaybackStatus::Paused, "A", "X"), true);
    assert_eq!(r.new_track, Some(0));
    assert!(r.status_changed);
    assert!(b.markers[0].initial);
    assert_eq!(b.markers[0].initial_status.as_deref(), Some("paused"));
    // Playing, same track: a status marker only.
    let r = b.observe(&obs(100, PlaybackStatus::Playing, "A", "X"), false);
    assert_eq!((r.new_track, r.status_changed), (None, true));
    // The player clears the fields while switching: ignored.
    let r = b.observe(&obs(200, PlaybackStatus::Playing, "", ""), false);
    assert_eq!(r, Observed::default());
    // Late details of the same song fill in the marker.
    let mut o = obs(300, PlaybackStatus::Playing, "a ", "x");
    o.track.album = "Album".into();
    o.track.duration_s = Some(200.0);
    let r = b.observe(&o, false);
    assert!(r.updated_track && r.new_track.is_none());
    assert_eq!(b.markers[0].album, "Album");
    assert_eq!(b.markers[0].duration_s, Some(200.0));
    // A new song.
    let r = b.observe(&obs(400, PlaybackStatus::Playing, "B", "X"), false);
    assert_eq!(r.new_track, Some(3));
    assert!(!b.markers[3].initial && b.markers[3].initial_status.is_none());
    // The same song again from the start (repeat): a new track.
    let mut o = obs(500, PlaybackStatus::Playing, "B", "X");
    o.track.position_s = Some(100.0);
    b.observe(&o, false);
    let mut o = obs(600, PlaybackStatus::Playing, "B", "X");
    o.track.position_s = Some(0.5);
    assert!(b.observe(&o, false).new_track.is_some());
    // Paused, then the player is closed.
    assert!(b.observe(&obs(700, PlaybackStatus::Paused, "B", "X"), false).status_changed);
    assert!(!b.observe(&obs(710, PlaybackStatus::Paused, "B", "X"), false).status_changed);
    let mut g = obs(800, PlaybackStatus::Closed, "B", "X");
    g.gone = true;
    b.observe(&g, false);
    assert_eq!(b.markers.last().unwrap().kind(), MarkerKind::Gone);
    let kinds: Vec<MarkerKind> = b.markers.iter().map(Marker::kind).collect();
    use MarkerKind::*;
    assert_eq!(kinds, [Track, Paused, Playing, Track, Track, Paused, Gone]);
}

#[test]
fn track_start_from_the_players_timeline() {
    let now = Instant::now();
    let wall = SystemTime::now();
    let t = TrackInfo {
        position_s: Some(1.5),
        position_updated: Some(wall - std::time::Duration::from_millis(500)),
        ..TrackInfo::default()
    };
    let at = track_start_instant(&t, now, wall).unwrap();
    let d = now.duration_since(at).as_secs_f64();
    assert!((d - 2.0).abs() < 1e-6, "{d}");
    let far = TrackInfo {
        position_s: Some(30.0),
        ..t.clone()
    };
    assert!(track_start_instant(&far, now, wall).is_none());
    let old = TrackInfo {
        position_updated: Some(wall - std::time::Duration::from_secs(60)),
        ..t
    };
    assert!(track_start_instant(&old, now, wall).is_none());
}

#[test]
fn mem_source_and_wav_source_agree() {
    let mut y = Synth::new();
    y.song(1.0, 300.0, 0.5);
    let dir = scratch("sources");
    write_master(&dir, &y.data);
    let mut w = split::WavSource::open(&dir.join(MASTER_FILE)).unwrap();
    let mut m = MemSource { samples: y.data.clone(), channels: 2 };
    assert_eq!(w.frames(), m.frames());
    assert_eq!(w.read_bytes(1000, 500).unwrap(), m.read_bytes(1000, 500).unwrap());
    assert_eq!(w.read_bytes(47_900, 500).unwrap().len(), 100 * 8, "clamped at the end");
    drop(w);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pause_inside_a_song_is_reported() {
    let mut y = Synth::new();
    y.song(2.0, 300.0, 0.5);
    y.silence(3.0); // paused
    y.song(5.0, 300.0, 0.5);
    let markers = vec![
        Marker { initial: true, ..track(0, "A", "X", Some(7.0)) },
        status(MarkerKind::Playing, 0),
        status(MarkerKind::Paused, 2 * u64::from(RATE)),
        status(MarkerKind::Playing, 5 * u64::from(RATE)),
    ];
    let dir = scratch("paused");
    write_master(&dir, &y.data);
    session(markers, "user").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    assert_eq!(out.songs.len(), 1);
    let sc = &out.songs[0].1;
    assert_eq!(sc.paused_s, 3.0);
    // 10 s recorded minus 3 s paused = the 7 s track: complete.
    assert_eq!(sc.status, SongStatus::Complete);
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Independent verification (2026-11-18): edge cases the scenario does not cover.
// ---------------------------------------------------------------------------

/// A master past the RF64 promotion point (the writer's limit lowered so the test
/// does not write 4 GiB) splits like a RIFF one, bit for bit, and an interrupted
/// RF64 recording is repaired and split too.
#[test]
fn verify_rf64_master_splits_bit_for_bit() {
    let sc = scenario(0.5);
    let dir = scratch("rf64");
    let mut w = WavWriter::create_with_limit(&dir.join(MASTER_FILE), WavSpec::new(RATE, 2), 1 << 20).unwrap();
    w.write_interleaved(&sc.master).unwrap();
    w.finalize().unwrap();
    assert!(crate::wav::read_header(&dir.join(MASTER_FILE)).unwrap().rf64, "the master is RF64");
    sc.session.save(&dir).unwrap();
    let keep = SplitSettings {
        delete_master: false,
        ..SplitSettings::default()
    };
    let out = split(&dir, &keep).unwrap();
    assert_eq!(out.session.state, SessionState::Split, "{:?}", out.session.message);
    assert_eq!(bits(&concat_parts(&dir, &out.session)), bits(&sc.master));
    assert_eq!(out.songs.len(), 6);
    // The same cuts as from a RIFF master.
    let dir2 = scratch("rf64-riff");
    write_master(&dir2, &sc.master);
    sc.session.save(&dir2).unwrap();
    let riff = split(&dir2, &keep).unwrap();
    let cuts = |s: &SessionFile| s.parts.iter().map(|p| (p.start_frame, p.end_frame)).collect::<Vec<_>>();
    assert_eq!(cuts(&out.session), cuts(&riff.session));
    // An interrupted RF64 recording.
    let dir3 = scratch("rf64-crash");
    let mut w = WavWriter::create_with_limit(&dir3.join(MASTER_FILE), WavSpec::new(RATE, 2), 1 << 20).unwrap();
    let half = sc.master.len() / 2 / 2 * 2;
    w.write_interleaved(&sc.master[..half]).unwrap();
    w.finalize().unwrap();
    {
        use std::io::Write as _;
        let mut f = OpenOptions::new().append(true).open(dir3.join(MASTER_FILE)).unwrap();
        let rest: Vec<u8> = sc.master[half..].iter().flat_map(|x| x.to_le_bytes()).collect();
        f.write_all(&rest).unwrap();
    }
    let mut s = sc.session.clone();
    s.state = SessionState::Recording;
    s.save(&dir3).unwrap();
    let rec = recover(&dir3, &SplitSettings::default()).unwrap().unwrap();
    assert_eq!(rec.session.state, SessionState::Split, "{:?}", rec.session.message);
    assert_eq!(bits(&concat_parts(&dir3, &rec.session)), bits(&sc.master));
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Two track changes 1.8 s apart (the user skipped straight on): the short piece is
/// not a song, the next one is, and nothing is lost.
#[test]
fn verify_two_track_changes_within_two_seconds() {
    let mut y = Synth::new();
    let _a = y.song(10.0, 300.0, 0.5);
    let g = y.silence(0.5);
    let b = y.song(1.8, 400.0, 0.5); // skipped after 1.8 s, gapless into C
    let c = y.song(10.0, 500.0, 0.5);
    let markers = vec![
        Marker { initial: true, position_s: Some(0.0), ..track(0, "A", "X", Some(10.0)) },
        status(MarkerKind::Playing, 0),
        track(b.0 + 4_800, "B", "X", Some(200.0)),
        track(c.0 + 4_800, "C", "X", Some(10.0)),
    ];
    let dir = scratch("twochanges");
    write_master(&dir, &y.data);
    session(markers, "user").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    let s = &out.session;
    assert_eq!(s.state, SessionState::Split, "{:?}", s.message);
    assert_eq!(bits(&concat_parts(&dir, s)), bits(&y.data));
    let kinds: Vec<(PartKind, Option<&str>)> = s.parts.iter().map(|p| (p.kind, p.reason.as_deref())).collect();
    assert_eq!(
        kinds,
        [(PartKind::Song, None), (PartKind::Excluded, Some("skipped within a few seconds")), (PartKind::Song, None)]
    );
    // A|B is cut in the gap's middle, B|C at the event (no gap): uncertain.
    assert!((s.parts[1].start_frame as i64 - ((g.0 + g.1) / 2) as i64).abs() < 10);
    assert_eq!(s.parts[2].start_frame, c.0 + 4_800);
    assert_eq!(out.songs[1].1.start.confidence, Confidence::Uncertain);
    assert_eq!(out.songs.iter().map(|x| x.1.title.as_str()).collect::<Vec<_>>(), ["A", "C"]);
    assert_eq!(out.songs[1].1.number, 2, "songs are numbered without the skip");
    let _ = std::fs::remove_dir_all(dir);

    // The mirror case: A|B gapless, B skipped after 1.8 s with a short silence before
    // C. The first change must not take the gap that belongs to the second.
    let mut y = Synth::new();
    let _a = y.song(10.0, 300.0, 0.5);
    let b = y.song(1.6, 400.0, 0.5);
    let g = y.silence(0.2);
    let c = y.song(10.0, 500.0, 0.5);
    let markers = vec![
        Marker { initial: true, position_s: Some(0.0), ..track(0, "A", "X", Some(10.0)) },
        status(MarkerKind::Playing, 0),
        track(b.0 + 4_800, "B", "X", Some(200.0)),
        track(c.0 + 4_800, "C", "X", Some(10.0)),
    ];
    let dir = scratch("twochanges-mirror");
    write_master(&dir, &y.data);
    session(markers, "user").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    let s = &out.session;
    assert_eq!(bits(&concat_parts(&dir, s)), bits(&y.data));
    assert_eq!(s.parts[1].start_frame, b.0 + 4_800, "A|B at the event (no gap there)");
    assert!((s.parts[2].start_frame as i64 - ((g.0 + g.1) / 2) as i64).abs() < 10, "B|C in its own gap");
    assert_eq!(s.parts[1].reason.as_deref(), Some("skipped within a few seconds"));
    let _ = std::fs::remove_dir_all(dir);
}

/// The player is closed in the middle of a song: the song ends there (partial), the
/// silence after it is excluded, and the master is still accounted for in full.
#[test]
fn verify_player_closed_mid_song() {
    let mut y = Synth::new();
    let _a = y.song(8.0, 300.0, 0.5);
    let _ = y.silence(1.0);
    let b = y.song(6.0, 400.0, 0.5); // a 30 s track; the player closes after 6 s
    let tail = y.silence(31.0); // the session stops itself after 30 s
    let markers = vec![
        Marker { initial: true, position_s: Some(0.0), ..track(0, "A", "X", Some(8.0)) },
        status(MarkerKind::Playing, 0),
        track(b.0 + 2_400, "B", "X", Some(30.0)),
        status(MarkerKind::Gone, b.1 + 9_600),
    ];
    let dir = scratch("gone");
    write_master(&dir, &y.data);
    session(markers, "player_closed").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    let s = &out.session;
    assert_eq!(s.state, SessionState::Split, "{:?}", s.message);
    assert_eq!(bits(&concat_parts(&dir, s)), bits(&y.data));
    assert_eq!(s.parts.last().unwrap().reason.as_deref(), Some("after playback stopped"));
    assert_eq!(out.songs.len(), 2);
    assert_eq!(out.songs[0].1.status, SongStatus::Complete);
    assert_eq!(out.songs[1].1.status, SongStatus::Partial);
    let end_b = out.songs[1].1.master_end_frame;
    assert!(end_b > b.1 && end_b <= tail.0 + u64::from(RATE), "at most 1 s of the silence stays with B");
    let _ = std::fs::remove_dir_all(dir);
}

/// A player that reports no artist: a long track is a song named after its title; a
/// short one (no artist, at most 65 s) is treated as an advertisement. A cover the
/// session names but that never arrived is simply absent. Odd characters are made
/// safe in the file name and kept in the sidecar.
#[test]
fn verify_no_artist_odd_title_and_missing_cover() {
    let mut y = Synth::new();
    let _a = y.song(8.0, 300.0, 0.5);
    let _ = y.silence(0.5);
    let b = y.song(8.0, 400.0, 0.5);
    let _ = y.silence(0.5);
    let c = y.song(8.0, 500.0, 0.5);
    let mut first = Marker { initial: true, position_s: Some(0.0), ..track(0, "Intro: \"Live\" <Part 1/2>?", "", None) };
    first.cover = Some("covers/0.jpg".into()); // never written (the picture failed to load)
    let markers = vec![
        first,
        status(MarkerKind::Playing, 0),
        track(b.0 + 2_400, "CON", "", Some(300.0)),
        track(c.0 + 2_400, "Jingle", "", Some(8.0)),
    ];
    let dir = scratch("noartist");
    write_master(&dir, &y.data);
    session(markers, "user").save(&dir).unwrap();
    let out = split(&dir, &SplitSettings::default()).unwrap();
    let s = &out.session;
    assert_eq!(s.state, SessionState::Split, "{:?}", s.message);
    assert_eq!(bits(&concat_parts(&dir, s)), bits(&y.data));
    let files: Vec<&str> = s.parts.iter().map(|p| p.file.as_str()).collect();
    assert_eq!(
        files,
        ["01 Intro - 'Live' (Part 1-2).wav", "02 CON.wav", "_excluded/Part 03 - advertisement - Jingle.wav"]
    );
    assert_eq!(out.songs[0].1.cover, None);
    assert_eq!(out.songs[0].1.status, SongStatus::Unknown);
    assert_eq!(out.songs[0].1.title, "Intro: \"Live\" <Part 1/2>?", "the sidecar keeps the player's own title");
    assert_eq!(out.songs[1].1.status, SongStatus::Partial, "8 s of a 300 s track");
    let _ = std::fs::remove_dir_all(dir);
}

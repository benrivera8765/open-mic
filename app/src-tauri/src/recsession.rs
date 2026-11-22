// SPDX-License-Identifier: GPL-3.0-or-later
//! "Record songs separately" (the owner's request of 18 Nov 2026; DECISIONS D-221..,
//! docs/APP_API.md section 8).
//!
//! A session records one continuous master (`<Recordings>/<date time>/master.wav`,
//! the normal recorder, lossless) while following one media player. Every report of
//! the player becomes a marker at its position in the master (jk_recorder::session:
//! `MarkerBuilder`, `Recorder::timeline_frame_at`), saved to `session.json` at once
//! (atomically), with the cover picture fetched shortly after each track change. The
//! session stops on Stop, after the player has been paused or stopped for the chosen
//! time (default 30 s), or when the song after the chosen number of songs starts.
//! Then the master is split on a background thread (jk_recorder::session::split):
//! one WAV + sidecar + cover per song, the rest in `_excluded/`, the master deleted
//! only after every file was read back and matched.
//!
//! A session interrupted by a crash, a closed app or a pulled stick is finished at
//! the next start ([`Inner::recover_sessions`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use jk_recorder::Recorder;
use jk_recorder::media::{Cause, MediaObservation, PlaybackStatus};
use jk_recorder::session::{
    COVERS_DIR, MASTER_FILE, MarkerBuilder, Observation, PartKind, PlayerRef, SESSION_FILE, SessionFile, SessionOptions,
    SessionState, SongSidecar, naming, track_start_instant,
};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::events::{emit, names};
use crate::medialink::WatchHandle;
use crate::recorder::{Extra, Inner, PlayerView, RecorderCtl, RecorderStatusView, lock, user_message, wait_for_audio};

/// `recorder_session_start(options)`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionStartOptions {
    /// The player to follow (an id from `recorder_media_sessions`).
    pub player_id: String,
    /// Stop after the player has been paused / stopped this long (s). Default 30;
    /// 0 = never.
    pub auto_stop_idle_s: Option<f64>,
    /// Stop when the song after this many songs starts. Default: no limit.
    pub max_songs: Option<u32>,
    /// Tell the player to play once the capture runs, if it is not playing.
    /// Default true.
    pub start_player: Option<bool>,
    /// Pause the player when the session stops itself after the song limit.
    /// Default true.
    pub pause_player_at_end: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackView {
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// `RecorderStatus.session` while a session records.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveSession {
    pub folder: String,
    pub path: String,
    /// The track the player reports now.
    pub current: Option<TrackView>,
    /// The player's state: "playing", "paused", ...
    pub playback: &'static str,
    /// Songs started so far (advertisements not counted).
    pub songs: u32,
    pub auto_stop_idle_s: f64,
    pub max_songs: Option<u32>,
    /// Seconds until the session stops itself because the player is idle.
    pub auto_stop_in_s: Option<f64>,
}

/// One song of a session (`RecordingSession.songs`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SongView {
    pub path: String,
    pub name: String,
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The cover picture next to the song, if any.
    pub cover_path: Option<String>,
    pub duration_s: f64,
    pub track_duration_s: Option<f64>,
    /// "complete" | "partial" | "unknown".
    pub status: &'static str,
    pub partial_reason: Option<String>,
    /// "certain" | "uncertain" | "edge".
    pub start_confidence: &'static str,
    pub end_confidence: &'static str,
    pub paused_s: f64,
}

/// An excluded part (`RecordingSession.excluded`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartView {
    pub path: String,
    pub name: String,
    pub reason: String,
    pub duration_s: f64,
}

/// `RecordingSession`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionView {
    pub folder: String,
    pub path: String,
    pub created: String,
    pub player: String,
    pub player_id: String,
    /// "recording" | "stopped" | "splitting" | "split" | "kept_master" | "failed" |
    /// "interrupted" (left by a crash; it is finished at the next start).
    pub state: &'static str,
    /// While splitting: 0..1.
    pub progress: Option<f64>,
    pub stop_reason: Option<String>,
    pub message: Option<String>,
    /// The full recording is still there.
    pub master_present: bool,
    /// Songs started (while recording) or written (after the split).
    pub song_count: u32,
    pub songs: Vec<SongView>,
    pub excluded: Vec<PartView>,
}

struct RunState {
    file: SessionFile,
    builder: MarkerBuilder,
    status: PlaybackStatus,
    idle_since: Option<Instant>,
    gone: bool,
}

/// A session that is recording.
pub struct SessionRun {
    pub(crate) dir: PathBuf,
    pub(crate) player: PlayerView,
    pause_at_end: bool,
    state: Mutex<RunState>,
    watch: Mutex<Option<Box<dyn WatchHandle>>>,
    recorder: OnceLock<Arc<Mutex<Option<Recorder>>>>,
    closed: AtomicBool,
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl SessionRun {
    pub(crate) fn live(&self) -> LiveSession {
        let st = lock(&self.state);
        let cur = st.builder.current_track().map(|m| TrackView {
            title: m.title.clone(),
            artist: m.artist.clone(),
            album: m.album.clone(),
        });
        let idle = st.file.options.auto_stop_idle_s;
        LiveSession {
            folder: folder_name(&self.dir),
            path: self.dir.display().to_string(),
            current: cur,
            playback: if st.gone { "closed" } else { st.status.as_str() },
            songs: SessionFile {
                markers: st.builder.markers.clone(),
                ..st.file.clone()
            }
            .song_count() as u32,
            auto_stop_idle_s: idle,
            max_songs: st.file.options.max_songs,
            auto_stop_in_s: (idle > 0.0)
                .then_some(st.idle_since)
                .flatten()
                .map(|t| (idle - t.elapsed().as_secs_f64()).max(0.0)),
        }
    }

    fn save(&self, st: &mut RunState) {
        st.file.markers = st.builder.markers.clone();
        if let Err(e) = st.file.save(&self.dir) {
            crate::applog::warn(&format!("recording session {}: saving the session file: {e}", self.dir.display()));
        }
    }

    fn frame_at(&self, at: Instant) -> Option<u64> {
        let rec = self.recorder.get()?;
        lock(rec).as_ref().and_then(|r| r.timeline_frame_at(at))
    }
}

/// A watch report: place it on the master's timeline and add markers.
fn on_observation(inner: &Arc<Inner>, run: &Arc<SessionRun>, obs: MediaObservation) {
    if run.closed.load(Ordering::SeqCst) {
        return;
    }
    let Some(frame) = run.frame_at(obs.at) else { return };
    let rate = u64::from(lock(&run.state).file.sample_rate.max(1));
    // The player's own timeline, when it has one and agrees within 3 s.
    let estimate = track_start_instant(&obs.track, Instant::now(), SystemTime::now())
        .and_then(|i| run.frame_at(i))
        .filter(|e| e.abs_diff(frame) <= 3 * rate);
    let (new_track, changed) = {
        let mut st = lock(&run.state);
        let o = st.builder.observe(
            &Observation {
                frame,
                estimate_frame: estimate,
                status: obs.status,
                track: obs.track.clone(),
                gone: obs.gone,
            },
            obs.cause == Cause::Start,
        );
        st.status = obs.status;
        st.gone |= obs.gone;
        if obs.status.is_playing() && !obs.gone {
            st.idle_since = None;
        } else if st.idle_since.is_none() {
            st.idle_since = Some(Instant::now());
        }
        let changed = o.new_track.is_some() || o.updated_track || o.status_changed;
        if changed {
            run.save(&mut st);
        }
        (o.new_track, changed)
    };
    if let Some(i) = new_track {
        let (inner, run) = (Arc::clone(inner), Arc::clone(run));
        let track = obs.track.clone();
        let _ = std::thread::Builder::new()
            .name("recorder-cover".into())
            .spawn(move || fetch_cover(&inner, &run, i, &track));
    }
    if changed {
        inner.refresh_last();
    }
}

/// Fetch the cover of track marker `i` once the player shows it (players often send
/// the picture a moment after the title), and only while it is still that song.
fn fetch_cover(inner: &Arc<Inner>, run: &Arc<SessionRun>, i: usize, track: &jk_recorder::media::TrackInfo) {
    for delay in [Duration::from_millis(700), Duration::from_millis(2500)] {
        std::thread::sleep(delay);
        if run.closed.load(Ordering::SeqCst) {
            return;
        }
        let same = inner
            .media
            .sessions()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == run.player.id))
            .is_some_and(|s| s.track.same_song(track));
        if !same {
            return;
        }
        let Ok(Some(t)) = inner.media.thumbnail(&run.player.id) else { continue };
        let Some(ext) = t.extension() else { continue };
        let rel = format!("{COVERS_DIR}/{i}.{ext}");
        let path = run.dir.join(&rel);
        let written = std::fs::create_dir_all(run.dir.join(COVERS_DIR))
            .and_then(|()| jk_recorder::session::write_atomic(&path, &t.bytes));
        if let Err(e) = written {
            crate::applog::warn(&format!("recording session: cover {}: {e}", path.display()));
            return;
        }
        let mut st = lock(&run.state);
        if let Some(m) = st.builder.markers.get_mut(i) {
            m.cover = Some(rel);
        }
        run.save(&mut st);
        return;
    }
}

/// Stops the session when the player has been idle long enough or the song limit
/// is reached. Runs until the session is closed.
fn monitor(inner: Arc<Inner>, run: Arc<SessionRun>) {
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if run.closed.load(Ordering::SeqCst) {
            return;
        }
        let reason = {
            let st = lock(&run.state);
            let idle = st.file.options.auto_stop_idle_s;
            let songs = SessionFile {
                markers: st.builder.markers.clone(),
                ..st.file.clone()
            }
            .song_count() as u32;
            if st.file.options.max_songs.is_some_and(|n| songs > n) {
                Some("max_songs")
            } else if idle > 0.0 && st.idle_since.is_some_and(|t| t.elapsed().as_secs_f64() >= idle) {
                Some(if st.gone { "player_closed" } else { "idle" })
            } else {
                None
            }
        };
        if let Some(r) = reason {
            let pause = r == "max_songs" && run.pause_at_end;
            crate::applog::info(&format!("recording session: stopping by itself ({r})"));
            if let Err(e) = inner.session_stop(pause, r) {
                crate::applog::warn(&format!("recording session: automatic stop: {e}"));
            }
            return;
        }
    }
}

impl RecorderCtl {
    /// `recorder_media_sessions()`.
    pub fn media_players(&self) -> MediaPlayersView {
        let m = &self.inner.media;
        if let Err(why) = m.available() {
            return MediaPlayersView {
                available: false,
                reason: Some(why),
                players: Vec::new(),
            };
        }
        match m.sessions() {
            Ok(list) => MediaPlayersView {
                available: true,
                reason: None,
                players: list
                    .into_iter()
                    .map(|s| MediaPlayerView {
                        id: s.id,
                        source_app_id: s.source_app_id,
                        app_name: s.app_name,
                        is_current: s.is_current,
                        status: s.status.as_str(),
                        title: s.track.title,
                        artist: s.track.artist,
                        album: s.track.album,
                        duration_s: s.track.duration_s,
                        can_play: s.can_play,
                        can_pause: s.can_pause,
                    })
                    .collect(),
            },
            Err(e) => MediaPlayersView {
                available: false,
                reason: Some(user_message(&e).1),
                players: Vec::new(),
            },
        }
    }

    /// `recorder_session_start(options)`.
    pub fn session_start(&self, opts: &SessionStartOptions) -> ApiResult<RecorderStatusView> {
        self.inner.session_start(opts)
    }

    /// `recorder_session_stop(pause_player)`.
    pub fn session_stop(&self, pause_player: bool) -> ApiResult<SessionView> {
        self.inner.session_stop(pause_player, "user")
    }

    /// `recorder_sessions_list()`, newest first.
    pub fn sessions_list(&self) -> ApiResult<Vec<SessionView>> {
        self.inner.sessions_list()
    }

    /// `recorder_session_split(folder)`: split a stopped session again (after a
    /// failed or unverified split).
    pub fn session_split(&self, folder: &str) -> ApiResult<SessionView> {
        self.inner.session_split(folder)
    }

    /// `recorder_session_delete(folder)`: deletes a finished session's folder for good
    /// (owner, 2026-11-22: "a way to delete these so I don't have to keep seeing them").
    /// Karaoke songs made from it are separate files in the Library and stay.
    pub fn session_delete(&self, folder: &str) -> ApiResult<()> {
        self.inner.session_delete(folder)
    }

    /// Finish sessions a crash or a closed app left behind (at start, in the
    /// background).
    pub fn recover_sessions(&self) {
        let me = Arc::clone(&self.inner);
        let _ = std::thread::Builder::new()
            .name("recorder-recover".into())
            .spawn(move || me.recover_sessions());
    }

    /// For tests: wait until no session is being split.
    pub fn wait_for_splits(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while !lock(&self.inner.splitting).is_empty() {
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}

/// `recorder_media_sessions()` result.
#[derive(Clone, Debug, Serialize)]
pub struct MediaPlayersView {
    pub available: bool,
    pub reason: Option<String>,
    pub players: Vec<MediaPlayerView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaPlayerView {
    pub id: String,
    pub source_app_id: String,
    pub app_name: String,
    pub is_current: bool,
    pub status: &'static str,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: Option<f64>,
    pub can_play: bool,
    pub can_pause: bool,
}

impl Inner {
    fn session_dir(&self) -> ApiResult<PathBuf> {
        std::fs::create_dir_all(&self.dir).map_err(|e| ApiError::io("the Recordings folder", &e))?;
        let stamp = naming::session_folder_name(&crate::sysinfo::local_stamp());
        let mut dir = self.dir.join(&stamp);
        let mut n = 2;
        while dir.exists() {
            dir = self.dir.join(format!("{stamp} ({n})"));
            n += 1;
        }
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::io("the session folder", &e))?;
        Ok(dir)
    }

    fn session_start(self: &Arc<Self>, opts: &SessionStartOptions) -> ApiResult<RecorderStatusView> {
        let player = self.find_player(&opts.player_id)?;
        if self.is_recording() {
            return Err(ApiError::new("busy", "A recording is already running."));
        }
        let dir = self.session_dir()?;
        let created = folder_name(&dir);
        let options = SessionOptions {
            auto_stop_idle_s: opts.auto_stop_idle_s.unwrap_or(30.0).clamp(0.0, 3600.0),
            max_songs: opts.max_songs.filter(|n| *n > 0),
        };
        let file = SessionFile::new(
            &created,
            PlayerRef {
                source_app_id: player.source_app_id.clone(),
                app_name: player.app_name.clone(),
            },
            options,
        );
        file.save(&dir).map_err(|e| ApiError::io("the session file", &e))?;
        let pv = PlayerView {
            id: player.id.clone(),
            app_name: player.app_name.clone(),
        };
        let run = Arc::new(SessionRun {
            dir: dir.clone(),
            player: pv.clone(),
            pause_at_end: opts.pause_player_at_end.unwrap_or(true),
            state: Mutex::new(RunState {
                file,
                builder: MarkerBuilder::default(),
                status: player.status,
                idle_since: Some(Instant::now()),
                gone: false,
            }),
            watch: Mutex::new(None),
            recorder: OnceLock::new(),
            closed: AtomicBool::new(false),
        });
        let rec = match self.start_capture(
            dir.join(MASTER_FILE),
            Extra {
                mode: "session",
                player: Some(pv.clone()),
                note: None,
                run: Some(Arc::clone(&run)),
            },
        ) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(dir.join(SESSION_FILE));
                let _ = std::fs::remove_dir(&dir);
                return Err(e);
            }
        };
        let _ = run.recorder.set(Arc::clone(&rec));
        let has_audio = wait_for_audio(&rec, Duration::from_secs(3));
        if let Some(f) = lock(&rec).as_ref().and_then(Recorder::format) {
            let mut st = lock(&run.state);
            st.file.sample_rate = f.sample_rate;
            st.file.channels = f.channels;
            run.save(&mut st);
        }
        // Follow the player. The first report becomes the initial marker.
        let watch = {
            let (me, r) = (Arc::clone(self), Arc::clone(&run));
            self.media.watch(&player.id, Box::new(move |o| on_observation(&me, &r, o)))
        };
        match watch {
            Ok(w) => *lock(&run.watch) = Some(w),
            Err(e) => {
                run.closed.store(true, Ordering::SeqCst);
                let _ = self.stop_capture();
                let _ = std::fs::remove_dir_all(&dir);
                self.set_last(RecorderStatusView::idle(None));
                let (_, msg) = user_message(&e);
                return Err(ApiError::new("player_not_found", msg));
            }
        }
        {
            let (me, r) = (Arc::clone(self), Arc::clone(&run));
            let _ = std::thread::Builder::new()
                .name("recorder-session".into())
                .spawn(move || monitor(me, r));
        }
        let playing = lock(&run.state).status.is_playing();
        let mut note = None;
        if opts.start_player.unwrap_or(true)
            && !playing
            && let Err(e) = self.media.play(&player.id)
        {
            note = Some(format!(
                "The recording is running, but {} did not start: {} Press play in the player.",
                player.app_name,
                user_message(&e).1
            ));
        }
        crate::applog::info(&format!(
            "recording session: started in {} following {} (first buffer: {has_audio}, player {})",
            dir.display(),
            player.source_app_id,
            if playing { "already playing" } else if note.is_none() { "started" } else { "did not start" }
        ));
        lock(&self.extra).note = note;
        self.refresh_last();
        self.emit_session(&dir);
        Ok(self.status())
    }

    /// Stop the session: stop following the player, stop the capture, pause the
    /// player if asked, save the session file and split in the background.
    pub(crate) fn session_stop(self: &Arc<Self>, pause_player: bool, reason: &str) -> ApiResult<SessionView> {
        let run = {
            let extra = lock(&self.extra);
            match (&extra.run, extra.mode) {
                (Some(r), "session") => Arc::clone(r),
                _ => return Err(ApiError::new("not_recording", "No songs are being recorded separately.")),
            }
        };
        if run.closed.swap(true, Ordering::SeqCst) {
            return Err(ApiError::new("not_recording", "The session is already stopping."));
        }
        let watch = lock(&run.watch).take();
        drop(watch);
        let stopped = self.stop_capture();
        let mut note = None;
        if pause_player && let Err(e) = self.media.pause(&run.player.id) {
            note = Some(format!("{} was not paused: {}", run.player.app_name, user_message(&e).1));
        }
        let dir = run.dir.clone();
        {
            let mut st = lock(&run.state);
            st.file.state = SessionState::Stopped;
            st.file.stop_reason = Some(reason.to_string());
            match &stopped {
                Ok(s) => {
                    if let Some(f) = &s.finished.stats.format {
                        st.file.sample_rate = f.sample_rate;
                        st.file.channels = f.channels;
                    }
                    st.file.stop_frame = s.finished.file.map(|f| f.frames);
                    if s.finished.file.is_none() {
                        st.file.state = SessionState::Failed;
                        st.file.message =
                            Some("Nothing was recorded: no sound arrived from the system.".into());
                    }
                }
                Err(e) => {
                    st.file.message = Some(format!("The capture reported a problem when it stopped: {}", e.message));
                }
            }
            run.save(&mut st);
        }
        self.set_last(RecorderStatusView {
            note,
            ..RecorderStatusView::idle(None)
        });
        let split_now = lock(&run.state).file.state == SessionState::Stopped;
        if split_now {
            self.split_in_background(dir.clone());
        } else {
            self.emit_session(&dir);
        }
        self.session_view(&dir)
    }

    /// On exit: stop the capture and leave the split for the next start.
    pub(crate) fn session_stop_for_exit(self: &Arc<Self>) {
        let run = lock(&self.extra).run.clone();
        let Some(run) = run else { return };
        run.closed.store(true, Ordering::SeqCst);
        drop(lock(&run.watch).take());
        let stopped = self.stop_capture();
        let mut st = lock(&run.state);
        st.file.state = SessionState::Stopped;
        st.file.stop_reason = Some("app_closed".into());
        if let Ok(s) = &stopped {
            st.file.stop_frame = s.finished.file.map(|f| f.frames);
        }
        run.save(&mut st);
    }

    fn split_in_background(self: &Arc<Self>, dir: PathBuf) {
        {
            let mut sp = lock(&self.splitting);
            if sp.contains_key(&dir) {
                return;
            }
            sp.insert(dir.clone(), 0.0);
        }
        self.emit_session(&dir);
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("recorder-split".into()).spawn(move || {
            let t0 = Instant::now();
            let mut last_emit = Instant::now();
            let r = jk_recorder::session::split::split_with_progress(&dir, &me.split_settings, &mut |p| {
                if let Some(v) = lock(&me.splitting).get_mut(&dir) {
                    *v = p;
                }
                if last_emit.elapsed() >= Duration::from_millis(250) {
                    last_emit = Instant::now();
                    me.emit_session(&dir);
                }
            });
            match &r {
                Ok(o) => crate::applog::info(&format!(
                    "recording session {}: split into {} song(s) in {:.1} s, state {}, master deleted: {}",
                    dir.display(),
                    o.songs.len(),
                    t0.elapsed().as_secs_f64(),
                    o.session.state.as_str(),
                    o.master_deleted
                )),
                Err(e) => {
                    crate::applog::warn(&format!("recording session {}: split failed: {e}", dir.display()));
                    if let Ok(mut s) = SessionFile::load(&dir)
                        && s.state != SessionState::Failed
                    {
                        s.state = SessionState::Failed;
                        s.message = Some(format!(
                            "The split stopped: {e}. Nothing was deleted; make room on the drive if it is full, then choose Split again."
                        ));
                        let _ = s.save(&dir);
                    }
                }
            }
            lock(&me.splitting).remove(&dir);
            me.emit_session(&dir);
        });
        if spawned.is_err() {
            lock(&self.splitting).clear();
        }
    }

    fn emit_session(&self, dir: &Path) {
        if let Ok(v) = self.session_view(dir) {
            emit(&*self.sink, names::RECORDER_SESSION, &v);
        }
    }

    fn running_dir(&self) -> Option<PathBuf> {
        let extra = lock(&self.extra);
        (extra.mode == "session").then(|| extra.run.as_ref().map(|r| r.dir.clone())).flatten()
    }

    pub(crate) fn session_view(&self, dir: &Path) -> ApiResult<SessionView> {
        let s = SessionFile::load(dir).map_err(|e| ApiError::io("the session file", &e))?;
        let progress = lock(&self.splitting).get(dir).copied();
        let running = self.running_dir().as_deref() == Some(dir);
        let state = if running {
            "recording"
        } else if progress.is_some() {
            "splitting"
        } else if s.state == SessionState::Recording {
            "interrupted"
        } else {
            s.state.as_str()
        };
        let mut songs = Vec::new();
        let mut excluded = Vec::new();
        for p in &s.parts {
            let path = dir.join(&p.file);
            let name = crate::recorder::file_name(&path);
            match p.kind {
                PartKind::Song => {
                    let Some(sc) = SongSidecar::for_audio(&path) else { continue };
                    songs.push(SongView {
                        cover_path: sc.cover_path(&path).map(|c| c.display().to_string()),
                        path: path.display().to_string(),
                        name,
                        number: sc.number,
                        title: sc.title,
                        artist: sc.artist,
                        album: sc.album,
                        duration_s: sc.recorded_duration_s,
                        track_duration_s: sc.track_duration_s,
                        status: match sc.status {
                            jk_recorder::session::SongStatus::Complete => "complete",
                            jk_recorder::session::SongStatus::Partial => "partial",
                            jk_recorder::session::SongStatus::Unknown => "unknown",
                        },
                        partial_reason: sc.partial_reason,
                        start_confidence: confidence(sc.start.confidence),
                        end_confidence: confidence(sc.end.confidence),
                        paused_s: sc.paused_s,
                    });
                }
                PartKind::Excluded => excluded.push(PartView {
                    path: path.display().to_string(),
                    name,
                    reason: p.reason.clone().unwrap_or_default(),
                    duration_s: (p.end_frame - p.start_frame) as f64 / f64::from(s.sample_rate.max(1)),
                }),
            }
        }
        let song_count = if running || songs.is_empty() { s.song_count() as u32 } else { songs.len() as u32 };
        Ok(SessionView {
            folder: folder_name(dir),
            path: dir.display().to_string(),
            created: s.created.clone(),
            player: s.player.app_name.clone(),
            player_id: s.player.source_app_id.clone(),
            state,
            progress,
            stop_reason: s.stop_reason.clone(),
            message: s.message.clone(),
            master_present: dir.join(&s.master).is_file(),
            song_count,
            songs,
            excluded,
        })
    }

    fn session_dirs(&self) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(SESSION_FILE).is_file())
            .collect();
        v.sort();
        v.reverse();
        v
    }

    fn sessions_list(&self) -> ApiResult<Vec<SessionView>> {
        Ok(self.session_dirs().iter().filter_map(|d| self.session_view(d).ok()).collect())
    }

    fn session_delete(&self, folder: &str) -> ApiResult<()> {
        if folder.is_empty() || folder.contains(['/', '\\', ':']) || folder == "." || folder == ".." {
            return Err(ApiError::bad_request("That is not a session folder name."));
        }
        let dir = self.dir.join(folder);
        if !dir.join(SESSION_FILE).is_file() {
            return Err(ApiError::new("not_found", "That session could not be found."));
        }
        if self.running_dir().as_deref() == Some(dir.as_path()) || lock(&self.splitting).contains_key(&dir) {
            return Err(ApiError::new("busy", "That session is still being recorded or split."));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| {
            ApiError::new(
                "in_use",
                format!("The session could not be deleted ({e}). If one of its songs is playing or being made into a karaoke song, wait for it to finish and try again."),
            )
        })
    }

    fn session_split(self: &Arc<Self>, folder: &str) -> ApiResult<SessionView> {
        if folder.is_empty() || folder.contains(['/', '\\', ':']) || folder == "." || folder == ".." {
            return Err(ApiError::bad_request("That is not a session folder name."));
        }
        let dir = self.dir.join(folder);
        let mut s = SessionFile::load(&dir).map_err(|_| ApiError::new("not_found", "That session could not be found."))?;
        if self.running_dir().as_deref() == Some(dir.as_path()) || lock(&self.splitting).contains_key(&dir) {
            return Err(ApiError::new("busy", "That session is still being recorded or split."));
        }
        if !dir.join(&s.master).is_file() {
            return Err(ApiError::new(
                "nothing_to_split",
                "This session's full recording is no longer there (it was deleted after a verified split), so there is nothing to split again.",
            ));
        }
        if s.state == SessionState::Recording {
            // Left by a crash: recover() repairs the master first.
            let me = Arc::clone(self);
            let d = dir.clone();
            lock(&self.splitting).insert(dir.clone(), 0.0);
            let _ = std::thread::Builder::new().name("recorder-recover".into()).spawn(move || {
                let _ = jk_recorder::session::recover(&d, &me.split_settings);
                lock(&me.splitting).remove(&d);
                me.emit_session(&d);
            });
            return self.session_view(&dir);
        }
        s.state = SessionState::Stopped;
        s.message = None;
        s.save(&dir).map_err(|e| ApiError::io("the session file", &e))?;
        self.split_in_background(dir.clone());
        self.session_view(&dir)
    }

    fn recover_sessions(self: &Arc<Self>) {
        for dir in self.session_dirs() {
            if self.running_dir().as_deref() == Some(dir.as_path()) {
                continue;
            }
            let Ok(s) = SessionFile::load(&dir) else { continue };
            let pending = matches!(s.state, SessionState::Recording | SessionState::Stopped | SessionState::Splitting)
                || (s.state == SessionState::Split && dir.join(&s.master).is_file());
            if !pending {
                continue;
            }
            {
                let mut sp = lock(&self.splitting);
                if sp.contains_key(&dir) {
                    continue;
                }
                sp.insert(dir.clone(), 0.0);
            }
            self.emit_session(&dir);
            crate::applog::info(&format!("recording session {}: finishing (was {})", dir.display(), s.state.as_str()));
            match jk_recorder::session::recover(&dir, &self.split_settings) {
                Ok(Some(o)) => crate::applog::info(&format!(
                    "recording session {}: recovered, {} song(s), state {}",
                    dir.display(),
                    o.songs.len(),
                    o.session.state.as_str()
                )),
                Ok(None) => {}
                Err(e) => crate::applog::warn(&format!("recording session {}: recovery failed: {e}", dir.display())),
            }
            lock(&self.splitting).remove(&dir);
            self.emit_session(&dir);
        }
    }
}

fn confidence(c: jk_recorder::session::Confidence) -> &'static str {
    match c {
        jk_recorder::session::Confidence::Certain => "certain",
        jk_recorder::session::Confidence::Uncertain => "uncertain",
        jk_recorder::session::Confidence::Edge => "edge",
    }
}

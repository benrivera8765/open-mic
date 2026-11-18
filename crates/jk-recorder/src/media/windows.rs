// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows: Global System Media Transport Controls (`Windows.Media.Control`).
//!
//! WinRT objects here are agile (the bindings mark them `Send + Sync`), and
//! `windows-core` joins the process-wide multithreaded apartment on first use
//! (`CoIncrementMTAUsage`), so these calls work from any thread that has not made
//! itself a single-threaded apartment. Every asynchronous call is waited for with a
//! timeout (5 s) so a hung player can never hang the app: the operation is
//! cancelled and a `Timeout` error returned instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::core::Interface;
use windows_future::{AsyncStatus, IAsyncOperation};

use super::{
    Cause, MediaObservation, MediaSessionInfo, PlaybackStatus, Thumbnail, TrackInfo, POLL_EVERY,
    friendly_app_name, parse_session_id, session_ids,
};
use crate::error::RecorderError;

/// Longest wait for one asynchronous WinRT call.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Covers larger than this are ignored (a player's thumbnail is a few hundred KB).
const MAX_THUMBNAIL: u64 = 16 << 20;
/// Events that arrive within this long of the first one are read together (a player
/// often sends the title, the artist and the cover as separate changes).
const COALESCE: Duration = Duration::from_millis(40);

fn winrt_err(doing: &str, e: &windows::core::Error) -> RecorderError {
    RecorderError::Backend {
        domain: "Windows.Media.Control".into(),
        code: i64::from(e.code().0),
        message: format!("{doing}: {}", e.message()),
    }
}

/// Wait for an asynchronous operation, at most `CALL_TIMEOUT`.
fn wait<T: windows::core::RuntimeType + 'static>(
    op: &IAsyncOperation<T>,
    what: &'static str,
) -> Result<T, RecorderError> {
    let end = Instant::now() + CALL_TIMEOUT;
    loop {
        let st = op.Status().map_err(|e| winrt_err(what, &e))?;
        if st != AsyncStatus::Started {
            // Completed: the result; Error / Canceled: GetResults returns the error.
            return op.GetResults().map_err(|e| winrt_err(what, &e));
        }
        if Instant::now() >= end {
            let _ = op.Cancel();
            return Err(RecorderError::Timeout(what));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

/// The session manager, asked for once and kept.
fn manager() -> Result<Manager, RecorderError> {
    let mut m = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = &*m {
        return Ok(m.clone());
    }
    let op = Manager::RequestAsync().map_err(|e| winrt_err("asking Windows for the media players", &e))?;
    let mgr = wait(&op, "asking Windows for the media players")?;
    *m = Some(mgr.clone());
    Ok(mgr)
}

fn forget_manager() {
    *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The sessions with their ids, in the order Windows lists them. One retry with a
/// fresh manager (after Explorer restarts, the old one stops answering).
fn list() -> Result<Vec<(String, Session)>, RecorderError> {
    match list_once() {
        Ok(v) => Ok(v),
        Err(_) => {
            forget_manager();
            list_once()
        }
    }
}

fn list_once() -> Result<Vec<(String, Session)>, RecorderError> {
    let mgr = manager()?;
    let v = mgr
        .GetSessions()
        .map_err(|e| winrt_err("listing the media players", &e))?;
    let n = v.Size().map_err(|e| winrt_err("listing the media players", &e))?;
    let mut sessions = Vec::with_capacity(n as usize);
    let mut app_ids = Vec::with_capacity(n as usize);
    for i in 0..n {
        let s = v
            .GetAt(i)
            .map_err(|e| winrt_err("listing the media players", &e))?;
        app_ids.push(s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
        sessions.push(s);
    }
    Ok(session_ids(&app_ids).into_iter().zip(sessions).collect())
}

fn not_open(id: &str) -> RecorderError {
    let (app, _) = parse_session_id(id);
    RecorderError::Player(format!(
        "{} is no longer open as a media player (or has nothing loaded). Open it, start a song once, then try again.",
        friendly_app_name(&app)
    ))
}

fn find(id: &str) -> Result<Session, RecorderError> {
    list()?
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, s)| s)
        .ok_or_else(|| not_open(id))
}

pub(super) fn available() -> Result<(), String> {
    manager().map(|_| ()).map_err(|e| {
        format!("Windows did not give access to its media-player list ({e}). This needs Windows 10 version 1809 or later.")
    })
}

fn status_of(s: &Session) -> (PlaybackStatus, bool, bool) {
    let Ok(info) = s.GetPlaybackInfo() else {
        return (PlaybackStatus::Unknown, false, false);
    };
    let status = match info.PlaybackStatus() {
        Ok(Status::Closed) => PlaybackStatus::Closed,
        Ok(Status::Opened) => PlaybackStatus::Opened,
        Ok(Status::Changing) => PlaybackStatus::Changing,
        Ok(Status::Stopped) => PlaybackStatus::Stopped,
        Ok(Status::Playing) => PlaybackStatus::Playing,
        Ok(Status::Paused) => PlaybackStatus::Paused,
        _ => PlaybackStatus::Unknown,
    };
    let (can_play, can_pause) = match info.Controls() {
        Ok(c) => (c.IsPlayEnabled().unwrap_or(false), c.IsPauseEnabled().unwrap_or(false)),
        Err(_) => (false, false),
    };
    (status, can_play, can_pause)
}

/// 100 ns ticks since 1601-01-01 (a WinRT DateTime) as a SystemTime.
fn system_time(ticks: i64) -> Option<SystemTime> {
    const UNIX_OFFSET: i64 = 116_444_736_000_000_000;
    let unix = ticks.checked_sub(UNIX_OFFSET)?;
    if ticks <= 0 || unix < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_nanos(unix as u64 * 100))
}

fn track_of(s: &Session) -> Result<TrackInfo, RecorderError> {
    let what = "reading what the player is playing";
    let op = s.TryGetMediaPropertiesAsync().map_err(|e| winrt_err(what, &e))?;
    let p = wait(&op, "reading what the player is playing")?;
    let text = |r: windows::core::Result<windows::core::HSTRING>| r.map(|h| h.to_string()).unwrap_or_default();
    let mut t = TrackInfo {
        title: text(p.Title()),
        artist: text(p.Artist()),
        album: text(p.AlbumTitle()),
        ..TrackInfo::default()
    };
    if t.artist.trim().is_empty() {
        t.artist = text(p.AlbumArtist());
    }
    if let Ok(tl) = s.GetTimelineProperties() {
        let start = tl.StartTime().map(|d| d.Duration).unwrap_or(0);
        let end = tl.EndTime().map(|d| d.Duration).unwrap_or(0);
        if end > start {
            t.duration_s = Some((end - start) as f64 / 1e7);
            if let Ok(pos) = tl.Position() {
                t.position_s = Some(((pos.Duration - start) as f64 / 1e7).max(0.0));
                t.position_updated = tl.LastUpdatedTime().ok().and_then(|d| system_time(d.UniversalTime));
            }
        }
    }
    Ok(t)
}

pub(super) fn sessions() -> Result<Vec<MediaSessionInfo>, RecorderError> {
    let all = list()?;
    let current = manager()
        .ok()
        .and_then(|m| m.GetCurrentSession().ok())
        .and_then(|s| s.SourceAppUserModelId().ok().map(|h| (s, h.to_string())));
    let mut out = Vec::with_capacity(all.len());
    for (id, s) in all {
        let source_app_id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
        let (status, can_play, can_pause) = status_of(&s);
        out.push(MediaSessionInfo {
            is_current: current.as_ref().is_some_and(|(c, _)| *c == s),
            app_name: friendly_app_name(&source_app_id),
            track: track_of(&s).unwrap_or_default(),
            id,
            source_app_id,
            status,
            can_play,
            can_pause,
        });
    }
    Ok(out)
}

fn command(id: &str, play: bool) -> Result<(), RecorderError> {
    let s = find(id)?;
    let what = if play { "asking the player to play" } else { "asking the player to pause" };
    let op = if play { s.TryPlayAsync() } else { s.TryPauseAsync() }.map_err(|e| winrt_err(what, &e))?;
    let accepted = wait(&op, if play { "asking the player to play" } else { "asking the player to pause" })?;
    if accepted {
        Ok(())
    } else {
        let (app, _) = parse_session_id(id);
        Err(RecorderError::Player(format!(
            "{} did not accept the {} command. Press {} in the player itself.",
            friendly_app_name(&app),
            if play { "play" } else { "pause" },
            if play { "play" } else { "pause" },
        )))
    }
}

pub(super) fn play(id: &str) -> Result<(), RecorderError> {
    command(id, true)
}

pub(super) fn pause(id: &str) -> Result<(), RecorderError> {
    command(id, false)
}

pub(super) fn thumbnail(id: &str) -> Result<Option<Thumbnail>, RecorderError> {
    let s = find(id)?;
    let what = "reading the cover picture";
    let p = wait(&s.TryGetMediaPropertiesAsync().map_err(|e| winrt_err(what, &e))?, "reading the cover picture")?;
    // No cover: the property is null, which the bindings report as an error.
    let Ok(r) = p.Thumbnail() else { return Ok(None) };
    let stream = wait(&r.OpenReadAsync().map_err(|e| winrt_err(what, &e))?, "reading the cover picture")?;
    let content_type = stream.ContentType().map(|h| h.to_string()).unwrap_or_default();
    let size = stream.Size().map_err(|e| winrt_err(what, &e))?;
    if size == 0 || size > MAX_THUMBNAIL {
        return Ok(None);
    }
    let reader = DataReader::CreateDataReader(&stream).map_err(|e| winrt_err(what, &e))?;
    let load: IAsyncOperation<u32> = reader
        .LoadAsync(size as u32)
        .and_then(|l| l.cast())
        .map_err(|e| winrt_err(what, &e))?;
    let got = wait(&load, "reading the cover picture")?;
    let mut bytes = vec![0u8; got as usize];
    reader.ReadBytes(&mut bytes).map_err(|e| winrt_err(what, &e))?;
    let _ = stream.Close();
    Ok(Some(Thumbnail { bytes, content_type }))
}

// ---------------------------------------------------------------------------
// Watch
// ---------------------------------------------------------------------------

enum Msg {
    Changed(Cause, Instant),
    Stop,
}

pub(super) struct Watch {
    tx: mpsc::Sender<Msg>,
    alive: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// What a report is compared on (the position moves all the time and is only
/// reported when it jumps).
#[derive(Clone, Debug, PartialEq)]
struct Key {
    status: PlaybackStatus,
    title: String,
    artist: String,
    album: String,
    duration_ms: Option<i64>,
}

fn key(status: PlaybackStatus, t: &TrackInfo) -> Key {
    Key {
        status,
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration_ms: t.duration_s.map(|d| (d * 1000.0).round() as i64),
    }
}

/// Where the player's position should be now, from an earlier reading.
fn expected_position(t: &TrackInfo, playing: bool) -> Option<f64> {
    let p = t.position_s?;
    if !playing {
        return Some(p);
    }
    let since = t.position_updated?.elapsed().ok()?.as_secs_f64();
    Some(p + since)
}

impl Watch {
    pub(super) fn start(
        id: &str,
        on_change: Box<dyn Fn(MediaObservation) + Send + 'static>,
    ) -> Result<Watch, RecorderError> {
        let session = find(id)?;
        let (tx, rx) = mpsc::channel::<Msg>();
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), RecorderError>>(1);
        let thread = {
            let tx = tx.clone();
            let alive = Arc::clone(&alive);
            let id = id.to_string();
            std::thread::Builder::new()
                .name("jk-media-watch".into())
                .spawn(move || watch_thread(&id, session, tx, rx, &alive, ready_tx, &*on_change))?
        };
        match ready_rx.recv_timeout(CALL_TIMEOUT * 2) {
            Ok(Ok(())) => Ok(Watch {
                tx,
                alive,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = tx.send(Msg::Stop);
                Err(RecorderError::Timeout("starting to follow the player"))
            }
        }
    }

    pub(super) fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch_thread(
    id: &str,
    session: Session,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    alive: &AtomicBool,
    ready: mpsc::SyncSender<Result<(), RecorderError>>,
    on_change: &(dyn Fn(MediaObservation) + Send),
) {
    // Event handlers only note the instant and wake this thread; the reading is done
    // here, never inside the player's callback.
    let handler = |cause: Cause| {
        let tx = tx.clone();
        move || {
            let _ = tx.send(Msg::Changed(cause, Instant::now()));
        }
    };
    let what = "following the player";
    let reg = (|| -> windows::core::Result<(i64, i64, i64)> {
        let f = handler(Cause::MediaProperties);
        let a = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            f();
            Ok(())
        }))?;
        let f = handler(Cause::Playback);
        let b = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            f();
            Ok(())
        }))?;
        let f = handler(Cause::Timeline);
        let c = session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
            f();
            Ok(())
        }))?;
        Ok((a, b, c))
    })();
    let tokens = match reg {
        Ok(t) => t,
        Err(e) => {
            alive.store(false, Ordering::SeqCst);
            let _ = ready.send(Err(winrt_err(what, &e)));
            return;
        }
    };
    let read = |cause: Cause, at: Instant| -> MediaObservation {
        let (status, _, _) = status_of(&session);
        MediaObservation {
            at,
            cause,
            status,
            track: track_of(&session).unwrap_or_default(),
            gone: false,
        }
    };
    let first = read(Cause::Start, Instant::now());
    let mut last_key = key(first.status, &first.track);
    let mut last_track = first.track.clone();
    let mut last_status = first.status;
    let _ = ready.send(Ok(()));
    on_change(first);
    let mut last_alive_check = Instant::now();
    loop {
        let (cause, at) = match rx.recv_timeout(POLL_EVERY) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Msg::Changed(c, at)) => {
                // Read once for a burst of events, with the first event's instant.
                let mut cause = c;
                let end = Instant::now() + COALESCE;
                let mut stop = false;
                while let Some(left) = end.checked_duration_since(Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(Msg::Changed(c2, _)) => {
                            if cause == Cause::Timeline && c2 != Cause::Timeline {
                                cause = c2;
                            }
                        }
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                            stop = true;
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                    }
                }
                if stop {
                    break;
                }
                (cause, at)
            }
            Err(RecvTimeoutError::Timeout) => (Cause::Poll, Instant::now()),
        };
        // Is the session still there? (Closing the player removes it.)
        if cause == Cause::Poll && last_alive_check.elapsed() >= Duration::from_secs(1) {
            last_alive_check = Instant::now();
            let still = list().map(|l| l.iter().any(|(i, s)| i == id && *s == session));
            if matches!(still, Ok(false)) {
                alive.store(false, Ordering::SeqCst);
                on_change(MediaObservation {
                    at,
                    cause,
                    status: PlaybackStatus::Closed,
                    track: last_track.clone(),
                    gone: true,
                });
                break;
            }
        }
        let obs = read(cause, at);
        let k = key(obs.status, &obs.track);
        // A jump of the position (seek, the same song restarted) is a change too.
        let jumped = match (
            expected_position(&last_track, last_status.is_playing()),
            obs.track.position_s,
        ) {
            (Some(e), Some(p)) => obs.track.position_updated != last_track.position_updated && (p - e).abs() > 2.0,
            _ => false,
        };
        let position_moved = obs.track.position_updated != last_track.position_updated;
        if k != last_key || jumped {
            last_key = k;
            last_status = obs.status;
            last_track = obs.track.clone();
            on_change(obs);
        } else if position_moved {
            // Keep the newest timeline reading for the next comparison.
            last_track = obs.track;
        }
    }
    let _ = session.RemoveMediaPropertiesChanged(tokens.0);
    let _ = session.RemovePlaybackInfoChanged(tokens.1);
    let _ = session.RemoveTimelinePropertiesChanged(tokens.2);
}

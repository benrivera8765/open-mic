// SPDX-License-Identifier: GPL-3.0-or-later
//! Recording songs separately: one continuous capture, markers from the media
//! player, then sample-exact cuts into one file per song.
//!
//! A session lives in its own folder, `Recordings/<date time>/`:
//!
//! ```text
//! master.wav        the continuous capture (32-bit float, exactly what the
//!                   recorder writes), deleted after a verified split
//! session.json      the session file: player, options, markers, state, parts
//! covers/<n>.jpg    cover pictures as the player gave them (removed after the split)
//! 01 Artist - Title.wav   one file per song (a sample-exact slice of master.wav)
//! 01 Artist - Title.json  its sidecar (title, artist, album, lengths, flags)
//! 01 Artist - Title.jpg   its cover, when the player gave one
//! _excluded/Part 03 - advertisement.wav   everything that is not a song
//! ```
//!
//! **Markers** are what the player reported, each at the position in the master
//! (frame) where it was noticed: a new track (title, artist, album, length, cover)
//! and playback state changes. They are saved after every change, atomically, so a
//! crash or a pulled stick leaves a session that [`recover`] can still split.
//!
//! **Splitting** ([`split`]) cuts the master at each track change and where playback
//! ended. Each cut is moved to the quietest point near the event: the middle of a
//! run of near-digital silence (every sample below -70 dBFS for at least 100 ms)
//! within 2.5 s of it. Without such a run (gapless albums, crossfades) the cut stays
//! at the event and the boundary is marked "uncertain". Cuts copy samples: no
//! processing, no resampling, no fades, so the song files and the excluded parts,
//! joined in order, are the master bit for bit; the master is deleted only after
//! every file has been read back and checked against it.

pub mod naming;
pub mod split;

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::media::{PlaybackStatus, TrackInfo};

pub use split::{SplitOutcome, SplitSettings, split};

/// The session file's name inside the session folder.
pub const SESSION_FILE: &str = "session.json";
/// The continuous capture's name inside the session folder.
pub const MASTER_FILE: &str = "master.wav";
/// Where cover pictures are kept while recording.
pub const COVERS_DIR: &str = "covers";
/// Where everything that is not a song goes.
pub const EXCLUDED_DIR: &str = "_excluded";
/// `format` of a session file.
pub const SESSION_FORMAT: &str = "jukeoke-recording-session";
/// `format` of a song sidecar.
pub const SONG_FORMAT: &str = "jukeoke-recorded-song";

/// The player a session follows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerRef {
    /// The player's app id as the OS reports it (`Spotify.exe`, `MSEdge`, ...).
    pub source_app_id: String,
    /// A readable name ("Spotify").
    pub app_name: String,
}

/// What the user chose when starting the session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionOptions {
    /// Stop on its own once the player has been paused or stopped this long
    /// (seconds; 0 = never).
    pub auto_stop_idle_s: f64,
    /// Stop on its own once this many songs have been recorded (the next song's
    /// start ends the session).
    pub max_songs: Option<u32>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            auto_stop_idle_s: 30.0,
            max_songs: None,
        }
    }
}

/// Where a session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Capturing (or the app stopped while capturing: [`recover`] finishes it).
    Recording,
    /// Capture stopped; not split yet.
    Stopped,
    /// Being split (or the split was interrupted: [`recover`] runs it again).
    Splitting,
    /// Split and verified; the master was deleted.
    Split,
    /// Split, but a check failed, so the master was kept (see `message`).
    KeptMaster,
    /// The split could not be done (see `message`); the master is kept.
    Failed,
}

impl SessionState {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Recording => "recording",
            SessionState::Stopped => "stopped",
            SessionState::Splitting => "splitting",
            SessionState::Split => "split",
            SessionState::KeptMaster => "kept_master",
            SessionState::Failed => "failed",
        }
    }
}

/// What a marker says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// A new track started (or the session began on this track).
    #[default]
    Track,
    /// Playback started or resumed.
    Playing,
    /// Playback paused.
    Paused,
    /// Playback stopped.
    Stopped,
    /// The player was closed.
    Gone,
}

/// One thing the player reported, at a position in the master.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// What happened.
    pub kind: MarkerKind,
    /// The master frame of the moment it was noticed.
    pub frame: u64,
    /// A better estimate of where the track started in the audio, from the
    /// player's own timeline (its position and when it was taken), when the player
    /// reports one and it agrees with `frame` within 3 s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_frame: Option<u64>,
    /// Track title (track markers).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Track artist (track markers).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    /// Album (track markers).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    /// The track's length as the player reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    /// The player's position in the track when the marker was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_s: Option<f64>,
    /// The cover picture, relative to the session folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// The track that was loaded when the recording started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub initial: bool,
    /// The playback state when the recording started (initial marker only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_status: Option<String>,
}

impl Marker {
    /// The marker's kind.
    pub fn kind(&self) -> MarkerKind {
        self.kind
    }

    /// Where the change most likely is in the audio.
    pub fn center(&self) -> u64 {
        self.estimate_frame.unwrap_or(self.frame)
    }

    fn track_info(&self) -> TrackInfo {
        TrackInfo {
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration_s: self.duration_s,
            ..TrackInfo::default()
        }
    }
}

/// What a part of the master became.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// A song (in the session folder).
    Song,
    /// Not a song (in `_excluded/`).
    Excluded,
}

/// One file the split wrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartRecord {
    /// Song or not.
    pub kind: PartKind,
    /// The WAV file, relative to the session folder (`/` separators).
    pub file: String,
    /// First master frame.
    pub start_frame: u64,
    /// One past the last master frame.
    pub end_frame: u64,
    /// Why an excluded part is not a song ("advertisement", "before the first
    /// song", "after playback stopped", "after the last song").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Read back and found equal to the master's samples.
    pub verified: bool,
}

/// `session.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    /// Always [`SESSION_FORMAT`].
    pub format: String,
    /// 1.
    pub version: u32,
    /// Local time the session started, `YYYY-MM-DD HH-MM-SS`.
    pub created: String,
    /// The master file's name in the session folder.
    pub master: String,
    /// The capture rate (0 until the recorder reports it).
    pub sample_rate: u32,
    /// The capture's channel count (0 until known).
    pub channels: u16,
    /// The player followed.
    pub player: PlayerRef,
    /// The user's choices.
    pub options: SessionOptions,
    /// Where the session is.
    pub state: SessionState,
    /// The master's length when the capture stopped (frames).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_frame: Option<u64>,
    /// Why it stopped: "user", "idle", "max_songs", "player_closed", "error",
    /// "interrupted" (the app was closed or crashed while recording).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The player's reports, in order.
    pub markers: Vec<Marker>,
    /// What the split produced, in master order.
    #[serde(default)]
    pub parts: Vec<PartRecord>,
    /// A plain explanation when the state is `kept_master` or `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SessionFile {
    /// A new session that is about to start recording.
    pub fn new(created: &str, player: PlayerRef, options: SessionOptions) -> SessionFile {
        SessionFile {
            format: SESSION_FORMAT.into(),
            version: 1,
            created: created.into(),
            master: MASTER_FILE.into(),
            sample_rate: 0,
            channels: 0,
            player,
            options,
            state: SessionState::Recording,
            stop_frame: None,
            stop_reason: None,
            markers: Vec::new(),
            parts: Vec::new(),
            message: None,
        }
    }

    /// Read `session.json` from a session folder.
    pub fn load(dir: &Path) -> io::Result<SessionFile> {
        let text = std::fs::read_to_string(dir.join(SESSION_FILE))?;
        let s: SessionFile = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("session file: {e}")))?;
        if s.format != SESSION_FORMAT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a recording session file"));
        }
        Ok(s)
    }

    /// Write `session.json` atomically (temporary file, flushed to the device, then
    /// renamed over the old one).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomic(&dir.join(SESSION_FILE), text.as_bytes())
    }

    /// Track markers, in order.
    pub fn tracks(&self) -> impl Iterator<Item = &Marker> {
        self.markers.iter().filter(|m| m.kind() == MarkerKind::Track)
    }

    /// Songs started so far (track markers that do not look like advertisements).
    pub fn song_count(&self) -> usize {
        self.tracks()
            .filter(|m| !looks_like_ad(&m.title, &m.artist, &m.album, m.duration_s))
            .count()
    }
}

/// How sure a cut is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The cut is in a run of near-digital silence near the player's event.
    Certain,
    /// No silence near the event (gapless join, crossfade, other sound): the cut is
    /// at the event, which can be off by the player's reporting delay.
    Uncertain,
    /// The start or the end of the recording.
    Edge,
}

/// One end of a song.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoundaryInfo {
    /// How sure the cut is.
    pub confidence: Confidence,
    /// Length of the silent run the cut is in (ms), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_ms: Option<f64>,
    /// Cut minus the player's event (ms): how far the cut was moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_ms: Option<f64>,
}

/// Whether a song was recorded whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SongStatus {
    /// At least as long as the player said the track is (within 2 s or 3 %).
    Complete,
    /// Clearly shorter than the track, or it started before the recording or was
    /// cut off by the end of the recording.
    Partial,
    /// The player did not report the track's length and nothing else says it is
    /// partial.
    Unknown,
}

/// `<song>.json` next to each song file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SongSidecar {
    /// Always [`SONG_FORMAT`].
    pub format: String,
    /// 1.
    pub version: u32,
    /// Title as the player reported it.
    pub title: String,
    /// Artist as the player reported it.
    pub artist: String,
    /// Album as the player reported it.
    pub album: String,
    /// The cover picture's file name next to the song, if any.
    pub cover: Option<String>,
    /// Length of the song file (s).
    pub recorded_duration_s: f64,
    /// The track's length as the player reported it (s).
    pub track_duration_s: Option<f64>,
    /// Complete, partial or unknown.
    pub status: SongStatus,
    /// Why it is partial, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_reason: Option<String>,
    /// The cut at the start.
    pub start: BoundaryInfo,
    /// The cut at the end.
    pub end: BoundaryInfo,
    /// Time the player was paused inside this song (s).
    #[serde(default)]
    pub paused_s: f64,
    /// The player it was recorded from.
    pub source_player: PlayerRef,
    /// The session folder's name.
    pub session: String,
    /// Song number in the session (1-based).
    pub number: u32,
    /// The slice of the master: first frame and one past the last.
    pub master_start_frame: u64,
    /// One past the last master frame.
    pub master_end_frame: u64,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels.
    pub channels: u16,
}

impl SongSidecar {
    /// The sidecar of a song file (`<stem>.json` next to `<stem>.wav`), if it is one
    /// of ours.
    pub fn for_audio(audio: &Path) -> Option<SongSidecar> {
        let text = std::fs::read_to_string(audio.with_extension("json")).ok()?;
        let s: SongSidecar = serde_json::from_str(&text).ok()?;
        (s.format == SONG_FORMAT).then_some(s)
    }

    /// The cover picture's full path, if the sidecar names one that exists.
    pub fn cover_path(&self, audio: &Path) -> Option<PathBuf> {
        let name = self.cover.as_deref()?;
        let p = audio.parent()?.join(name);
        p.is_file().then_some(p)
    }
}

/// Heuristic for a player's advertisement: title, artist or album is one of the
/// words players use for ads ("Advertisement", "Ad", "Sponsored", or the player's
/// own name: some players list their ads under their own name as the artist), or
/// there is no artist and the reported length is at most 65 s.
pub fn looks_like_ad(title: &str, artist: &str, album: &str, duration_s: Option<f64>) -> bool {
    const AD_WORDS: [&str; 7] = [
        "advertisement",
        "advert",
        "ad",
        "ads",
        "sponsored",
        "spotify",
        "werbung",
    ];
    let n = |s: &str| s.trim().to_lowercase();
    let (t, a, al) = (n(title), n(artist), n(album));
    if AD_WORDS.contains(&t.as_str()) || AD_WORDS.contains(&a.as_str()) || al == "advertisement" {
        return true;
    }
    a.is_empty() && !t.is_empty() && duration_s.is_some_and(|d| d > 0.0 && d <= 65.0)
}

// ---------------------------------------------------------------------------
// Markers from player reports
// ---------------------------------------------------------------------------

/// One report from the player, already placed on the master's timeline.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Master frame of the moment it was noticed.
    pub frame: u64,
    /// A better estimate of where the track started (see [`Marker::estimate_frame`]).
    pub estimate_frame: Option<u64>,
    /// Playback state.
    pub status: PlaybackStatus,
    /// The track.
    pub track: TrackInfo,
    /// The player's session has gone.
    pub gone: bool,
}

/// What [`MarkerBuilder::observe`] changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// A new track marker was added (its index in `markers`).
    pub new_track: Option<usize>,
    /// Details of the current track were filled in (album, length).
    pub updated_track: bool,
    /// A playback-state marker was added.
    pub status_changed: bool,
}

/// Turns player reports into markers. Rules:
///
/// * a report with neither title nor artist is ignored for the track (players
///   clear the fields for a moment while switching);
/// * a new title or artist is a new track; the same song again is a new track only
///   when its position jumped back to the start (repeat);
/// * later details of the same track (album, length) fill in the marker;
/// * playing / paused / stopped / gone are added when they change.
#[derive(Clone, Debug, Default)]
pub struct MarkerBuilder {
    /// The markers so far.
    pub markers: Vec<Marker>,
    last_position: Option<f64>,
}

impl MarkerBuilder {
    /// Continue from saved markers.
    pub fn from_markers(markers: Vec<Marker>) -> MarkerBuilder {
        MarkerBuilder {
            markers,
            last_position: None,
        }
    }

    /// The current track marker.
    pub fn current_track(&self) -> Option<&Marker> {
        self.markers.iter().rev().find(|m| m.kind() == MarkerKind::Track)
    }

    fn last_status(&self) -> Option<MarkerKind> {
        self.markers
            .iter()
            .rev()
            .find(|m| m.kind() != MarkerKind::Track)
            .map(Marker::kind)
    }

    /// Add the report. `initial` marks the report taken when recording started.
    pub fn observe(&mut self, o: &Observation, initial: bool) -> Observed {
        let mut out = Observed::default();
        let t = &o.track;
        if !t.is_empty() && !o.gone {
            let restart = match (self.current_track(), t.position_s, self.last_position) {
                (Some(cur), Some(p), Some(q)) => cur.track_info().same_song(t) && p < 3.0 && q > 15.0,
                _ => false,
            };
            match self.current_track() {
                Some(cur) if cur.track_info().same_song(t) && !restart => {
                    let i = self.markers.iter().rposition(|m| m.kind() == MarkerKind::Track).unwrap_or(0);
                    let m = &mut self.markers[i];
                    if m.album.is_empty() && !t.album.is_empty() {
                        m.album = t.album.clone();
                        out.updated_track = true;
                    }
                    if m.duration_s.is_none() && t.duration_s.is_some() {
                        m.duration_s = t.duration_s;
                        out.updated_track = true;
                    }
                }
                _ => {
                    self.markers.push(Marker {
                        kind: MarkerKind::Track,
                        frame: o.frame,
                        estimate_frame: o.estimate_frame,
                        title: t.title.trim().to_string(),
                        artist: t.artist.trim().to_string(),
                        album: t.album.trim().to_string(),
                        duration_s: t.duration_s,
                        position_s: t.position_s,
                        cover: None,
                        initial,
                        initial_status: initial.then(|| o.status.as_str().to_string()),
                    });
                    out.new_track = Some(self.markers.len() - 1);
                }
            }
            if t.position_s.is_some() {
                self.last_position = t.position_s;
            }
        }
        let kind = if o.gone {
            Some(MarkerKind::Gone)
        } else {
            match o.status {
                PlaybackStatus::Playing => Some(MarkerKind::Playing),
                PlaybackStatus::Paused => Some(MarkerKind::Paused),
                PlaybackStatus::Stopped | PlaybackStatus::Closed => Some(MarkerKind::Stopped),
                _ => None,
            }
        };
        if let Some(k) = kind
            && self.last_status() != Some(k)
        {
            self.markers.push(Marker {
                kind: k,
                frame: o.frame,
                ..Marker::default()
            });
            out.status_changed = true;
        }
        out
    }
}

/// When the player says the current track started, as an instant: its reported
/// position subtracted from the time it took that position. `None` without a
/// timeline, for a position over 10 s, or a reading older than 10 s.
pub fn track_start_instant(track: &TrackInfo, now: Instant, now_wall: SystemTime) -> Option<Instant> {
    let p = track.position_s?;
    let taken = track.position_updated?;
    if !(0.0..10.0).contains(&p) {
        return None;
    }
    let age = now_wall.duration_since(taken).ok()?;
    if age.as_secs_f64() > 10.0 {
        return None;
    }
    now.checked_sub(age)?
        .checked_sub(std::time::Duration::from_secs_f64(p))
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

static TMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary name next to `path` that no other writer uses.
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tag = std::process::id() ^ nanos.rotate_left(7) ^ n.wrapping_mul(0x9E37_79B9);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".tmp-{tag:08x}"));
    path.with_file_name(name)
}

/// Write a file atomically: a temporary file next to it, flushed to the device,
/// then renamed over the target (which may exist).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        sync_dir(path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Copy a file atomically (temporary file, flushed, renamed).
pub fn copy_atomic(from: &Path, to: &Path) -> io::Result<()> {
    let tmp = temp_path(to);
    let result = (|| {
        let mut src = File::open(from)?;
        let mut dst = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(&mut src, &mut dst)?;
        dst.sync_all()?;
        drop(dst);
        std::fs::rename(&tmp, to)?;
        sync_dir(to);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Flush a directory entry change to the device (Unix; Windows has no handle for it
/// and its renames are journalled by the file system).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Remove unfinished temporary files (`*.tmp-xxxxxxxx`) a crash left in a session
/// folder and its `_excluded` folder.
pub fn remove_temp_files(dir: &Path) {
    for d in [dir.to_path_buf(), dir.join(EXCLUDED_DIR), dir.join(COVERS_DIR)] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some((_, tag)) = name.rsplit_once(".tmp-")
                && tag.len() == 8
                && tag.chars().all(|c| c.is_ascii_hexdigit())
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Finish a session that did not finish: one the app was closed (or crashed, or
/// the stick was pulled) during. A session still marked "recording" gets its
/// master's header repaired (everything on disk is kept) and is marked stopped
/// ("interrupted"); then it is split. A session marked "stopped" or "splitting" is
/// split (again: files already written and verified are kept). A "split" session
/// whose master is still there (closed between the last check and the delete) has
/// its parts checked again and the master deleted. Anything else is left alone.
/// Returns `None` when there was nothing to do.
pub fn recover(dir: &Path, settings: &SplitSettings) -> io::Result<Option<SplitOutcome>> {
    let mut s = SessionFile::load(dir)?;
    remove_temp_files(dir);
    match s.state {
        SessionState::Recording => {
            let master = dir.join(&s.master);
            if master.is_file() {
                let r = crate::wav::repair(&master)?;
                let info = crate::wav::read_header(&master)?;
                s.sample_rate = info.sample_rate;
                s.channels = info.channels;
                s.stop_frame = Some(r.frames_after);
            }
            s.state = SessionState::Stopped;
            s.stop_reason = Some("interrupted".into());
            s.save(dir)?;
            split(dir, settings).map(Some)
        }
        SessionState::Stopped | SessionState::Splitting => split(dir, settings).map(Some),
        SessionState::Split if dir.join(&s.master).is_file() => split::finish_cleanup(dir, &mut s).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;

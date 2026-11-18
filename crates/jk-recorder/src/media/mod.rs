// SPDX-License-Identifier: GPL-3.0-or-later
//! The link to the media player (the app the user plays music in): which players
//! are open, what each one is playing, play / pause, and a watch that reports every
//! track or playback change of one player with the instant it was noticed.
//!
//! * **Windows (10 1809+):** the Global System Media Transport Controls
//!   (`Windows.Media.Control`): the same per-app sessions the volume flyout and the
//!   media keys use. Every player that shows there works (desktop music players,
//!   browsers playing audio). Nothing here needs a permission or an account.
//! * **Other platforms:** not available ([`available`] says why).
//!
//! The recorder uses it for "Record & play" (start the capture first, then tell the
//! player to play, so the first note is never missed) and for recording songs
//! separately (track changes become markers on the capture timeline; see
//! [`crate::session`]).
//!
//! Sessions are named by the player's app id (`Spotify.exe`, `MSEdge`, ...); when
//! two sessions share an app id, the second is `<id>#2` and so on, in the order
//! Windows lists them.

use std::time::{Duration, Instant, SystemTime};

use crate::error::RecorderError;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as imp;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
use self::other as imp;

/// What a player says it is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// The player's media is closed.
    Closed,
    /// Media is open but not playing yet.
    Opened,
    /// The player is switching media.
    Changing,
    /// Stopped.
    Stopped,
    /// Playing.
    Playing,
    /// Paused.
    Paused,
    /// The player did not say.
    Unknown,
}

impl PlaybackStatus {
    /// `"closed" | "opened" | "changing" | "stopped" | "playing" | "paused" | "unknown"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PlaybackStatus::Closed => "closed",
            PlaybackStatus::Opened => "opened",
            PlaybackStatus::Changing => "changing",
            PlaybackStatus::Stopped => "stopped",
            PlaybackStatus::Playing => "playing",
            PlaybackStatus::Paused => "paused",
            PlaybackStatus::Unknown => "unknown",
        }
    }

    /// Parse [`PlaybackStatus::as_str`] (anything else is `Unknown`).
    pub fn parse(s: &str) -> PlaybackStatus {
        match s {
            "closed" => PlaybackStatus::Closed,
            "opened" => PlaybackStatus::Opened,
            "changing" => PlaybackStatus::Changing,
            "stopped" => PlaybackStatus::Stopped,
            "playing" => PlaybackStatus::Playing,
            "paused" => PlaybackStatus::Paused,
            _ => PlaybackStatus::Unknown,
        }
    }

    /// Playing (the only state in which a song is being heard).
    pub fn is_playing(self) -> bool {
        self == PlaybackStatus::Playing
    }
}

/// The track a player reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    /// Title ("" when the player gives none).
    pub title: String,
    /// Artist ("" when the player gives none).
    pub artist: String,
    /// Album ("" when the player gives none).
    pub album: String,
    /// The track's length, when the player reports a timeline.
    pub duration_s: Option<f64>,
    /// The play position at `position_updated`, when the player reports a timeline.
    pub position_s: Option<f64>,
    /// When the player last updated `position_s` (wall clock).
    pub position_updated: Option<SystemTime>,
}

impl TrackInfo {
    /// True when there is neither a title nor an artist.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.artist.trim().is_empty()
    }

    /// The same song (title and artist, ignoring case and outer spaces)?
    pub fn same_song(&self, other: &TrackInfo) -> bool {
        fn n(s: &str) -> String {
            s.trim().to_lowercase()
        }
        n(&self.title) == n(&other.title) && n(&self.artist) == n(&other.artist)
    }
}

/// One player session.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSessionInfo {
    /// The id to pass to [`play`], [`pause`], [`thumbnail`] and [`MediaWatch::start`].
    pub id: String,
    /// The player's app id as Windows reports it (`Spotify.exe`, `MSEdge`, ...).
    pub source_app_id: String,
    /// A readable name derived from the app id ("Spotify", "Microsoft Edge").
    pub app_name: String,
    /// The session Windows considers current (the one the media keys control).
    pub is_current: bool,
    /// Playback state.
    pub status: PlaybackStatus,
    /// What it is playing.
    pub track: TrackInfo,
    /// The player accepts a play command now.
    pub can_play: bool,
    /// The player accepts a pause command now.
    pub can_pause: bool,
}

/// Why a watch reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The first report, when the watch starts.
    Start,
    /// The player changed its track information (title, artist, album, cover).
    MediaProperties,
    /// The player changed its playback state.
    Playback,
    /// The player changed its timeline (position, length).
    Timeline,
    /// The periodic check noticed a change the player did not announce.
    Poll,
}

/// One report from a [`MediaWatch`].
#[derive(Clone, Debug)]
pub struct MediaObservation {
    /// When the change was noticed: the instant the player's event arrived (or the
    /// poll that found it), before the properties were read.
    pub at: Instant,
    /// Why this report was made.
    pub cause: Cause,
    /// Playback state.
    pub status: PlaybackStatus,
    /// The track.
    pub track: TrackInfo,
    /// The session has gone (the player was closed). The watch stops after this.
    pub gone: bool,
}

/// A cover picture as the player gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    /// The encoded picture.
    pub bytes: Vec<u8>,
    /// The type the player declared (may be empty).
    pub content_type: String,
}

impl Thumbnail {
    /// `"jpg"` or `"png"` from the picture's own signature; `None` for anything else
    /// (only JPEG and PNG are kept: the Studio accepts those as a cover).
    pub fn extension(&self) -> Option<&'static str> {
        let b = &self.bytes;
        if b.len() > 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF {
            Some("jpg")
        } else if b.len() > 8 && b[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
            Some("png")
        } else {
            None
        }
    }
}

/// `Ok(())` when this computer can see and control media players; otherwise the
/// reason, in plain words.
pub fn available() -> Result<(), String> {
    imp::available()
}

/// Every player session, in the order the OS lists them.
pub fn sessions() -> Result<Vec<MediaSessionInfo>, RecorderError> {
    imp::sessions()
}

/// Ask the player to play. Errors if the session is gone or the player refused.
pub fn play(id: &str) -> Result<(), RecorderError> {
    imp::play(id)
}

/// Ask the player to pause. Errors if the session is gone or the player refused.
pub fn pause(id: &str) -> Result<(), RecorderError> {
    imp::pause(id)
}

/// The cover picture of the player's current track, if it offers one.
pub fn thumbnail(id: &str) -> Result<Option<Thumbnail>, RecorderError> {
    imp::thumbnail(id)
}

/// How often a watch checks the player in case it missed an event.
pub const POLL_EVERY: Duration = Duration::from_millis(250);

/// Follows one player session and calls a function on every change (track,
/// playback state, timeline) with the instant the change was noticed. Stops when
/// dropped.
pub struct MediaWatch {
    inner: imp::Watch,
}

impl std::fmt::Debug for MediaWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaWatch").finish_non_exhaustive()
    }
}

impl MediaWatch {
    /// Start watching session `id`. The first report (cause `Start`) comes at once.
    /// `on_change` runs on the watch's own thread.
    pub fn start(
        id: &str,
        on_change: impl Fn(MediaObservation) + Send + 'static,
    ) -> Result<MediaWatch, RecorderError> {
        Ok(MediaWatch {
            inner: imp::Watch::start(id, Box::new(on_change))?,
        })
    }

    /// Stop watching (also done on drop).
    pub fn stop(self) {
        drop(self);
    }

    /// True while the watched session exists.
    pub fn alive(&self) -> bool {
        self.inner.alive()
    }
}

/// A readable player name from an app id: `Spotify.exe` -> "Spotify",
/// `MSEdge` -> "Microsoft Edge", `Vendor.App_hash!App` -> "App".
pub fn friendly_app_name(source_app_id: &str) -> String {
    let id = source_app_id.trim();
    let known = [
        ("msedge", "Microsoft Edge"),
        ("chrome", "Google Chrome"),
        ("firefox", "Firefox"),
        ("308046b0af4a39cb", "Firefox"),
    ];
    let lower = id.to_lowercase();
    for (k, v) in known {
        if lower == k || lower == format!("{k}.exe") {
            return v.to_string();
        }
        // A site installed or opened as an app: "MSEdge.<site>.<profile>".
        if lower.starts_with(&format!("{k}.")) && !lower.ends_with(".exe") {
            return format!("{v} (web app)");
        }
    }
    // Packaged apps: "Publisher.App_hash!AppId" -> the part after '!'.
    let base = id.rsplit('!').next().unwrap_or(id);
    // Win32 apps: a path or an exe name -> the file name without ".exe".
    let base = base.rsplit(['\\', '/']).next().unwrap_or(base);
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    if base.is_empty() {
        "Unknown player".to_string()
    } else {
        base.to_string()
    }
}

/// Session ids for a list of app ids in OS order: the first of each app id is the
/// app id itself, later ones get `#2`, `#3`...
pub fn session_ids(app_ids: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(app_ids.len());
    for (i, a) in app_ids.iter().enumerate() {
        let n = app_ids[..i].iter().filter(|b| *b == a).count();
        out.push(if n == 0 {
            a.clone()
        } else {
            format!("{a}#{}", n + 1)
        });
    }
    out
}

/// The inverse of [`session_ids`]: (app id, 0-based occurrence) for a session id.
pub fn parse_session_id(id: &str) -> (String, usize) {
    if let Some((a, n)) = id.rsplit_once('#')
        && let Ok(n) = n.parse::<usize>()
        && n >= 2
    {
        return (a.to_string(), n - 1);
    }
    (id.to_string(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_are_readable() {
        assert_eq!(friendly_app_name("Spotify.exe"), "Spotify");
        assert_eq!(friendly_app_name("MSEdge"), "Microsoft Edge");
        assert_eq!(
            friendly_app_name("MSEdge.127.0.0.1_index.html.edgeprofile.Default"),
            "Microsoft Edge (web app)"
        );
        assert_eq!(friendly_app_name("Chrome"), "Google Chrome");
        assert_eq!(
            friendly_app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(friendly_app_name(r"C:\Program Files\Foo\foo.exe"), "foo");
        assert_eq!(friendly_app_name(""), "Unknown player");
    }

    #[test]
    fn duplicate_app_ids_get_numbered_ids_that_parse_back() {
        let ids = session_ids(&["A".into(), "B".into(), "A".into(), "A".into()]);
        assert_eq!(ids, ["A", "B", "A#2", "A#3"]);
        assert_eq!(parse_session_id("A#3"), ("A".to_string(), 2));
        assert_eq!(parse_session_id("A"), ("A".to_string(), 0));
        // A '#' that is part of the app id itself.
        assert_eq!(parse_session_id("X#Y"), ("X#Y".to_string(), 0));
        assert_eq!(parse_session_id("X#1"), ("X#1".to_string(), 0));
    }

    #[test]
    fn thumbnail_types_come_from_the_signature() {
        let jpg = Thumbnail {
            bytes: vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0],
            content_type: String::new(),
        };
        assert_eq!(jpg.extension(), Some("jpg"));
        let png = Thumbnail {
            bytes: vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0],
            content_type: "image/png".into(),
        };
        assert_eq!(png.extension(), Some("png"));
        let bmp = Thumbnail {
            bytes: b"BM\0\0\0\0\0\0\0".to_vec(),
            content_type: "image/bmp".into(),
        };
        assert_eq!(bmp.extension(), None);
    }

    #[test]
    fn same_song_ignores_case_and_spaces() {
        let a = TrackInfo {
            title: "Song ".into(),
            artist: "Band".into(),
            ..Default::default()
        };
        let b = TrackInfo {
            title: "song".into(),
            artist: " BAND".into(),
            album: "x".into(),
            ..Default::default()
        };
        assert!(a.same_song(&b));
        assert!(!a.is_empty());
        assert!(TrackInfo::default().is_empty());
    }
}

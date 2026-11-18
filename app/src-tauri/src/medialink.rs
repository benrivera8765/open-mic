// SPDX-License-Identifier: GPL-3.0-or-later
//! The recorder's link to the media player (jk_recorder::media), behind a trait so
//! the core can be tested without a real player: the app uses [`SystemMedia`],
//! tests use [`fake::FakeMedia`], which never touches any real app.

use std::sync::Arc;

use jk_recorder::RecorderError;
use jk_recorder::media::{MediaObservation, MediaSessionInfo, MediaWatch, Thumbnail};

/// A running watch; dropping it stops the reports.
pub trait WatchHandle: Send + 'static {}

impl WatchHandle for MediaWatch {}

/// Called with every report of a watch.
pub type OnChange = Box<dyn Fn(MediaObservation) + Send + Sync + 'static>;

/// See jk_recorder::media.
pub trait MediaLink: Send + Sync + 'static {
    fn available(&self) -> Result<(), String>;
    fn sessions(&self) -> Result<Vec<MediaSessionInfo>, RecorderError>;
    fn play(&self, id: &str) -> Result<(), RecorderError>;
    fn pause(&self, id: &str) -> Result<(), RecorderError>;
    fn thumbnail(&self, id: &str) -> Result<Option<Thumbnail>, RecorderError>;
    fn watch(&self, id: &str, on_change: OnChange) -> Result<Box<dyn WatchHandle>, RecorderError>;
}

/// The real players on this computer.
pub struct SystemMedia;

impl MediaLink for SystemMedia {
    fn available(&self) -> Result<(), String> {
        jk_recorder::media::available()
    }
    fn sessions(&self) -> Result<Vec<MediaSessionInfo>, RecorderError> {
        jk_recorder::media::sessions()
    }
    fn play(&self, id: &str) -> Result<(), RecorderError> {
        jk_recorder::media::play(id)
    }
    fn pause(&self, id: &str) -> Result<(), RecorderError> {
        jk_recorder::media::pause(id)
    }
    fn thumbnail(&self, id: &str) -> Result<Option<Thumbnail>, RecorderError> {
        jk_recorder::media::thumbnail(id)
    }
    fn watch(&self, id: &str, on_change: OnChange) -> Result<Box<dyn WatchHandle>, RecorderError> {
        Ok(Box::new(MediaWatch::start(id, on_change)?))
    }
}

pub fn system() -> Arc<dyn MediaLink> {
    Arc::new(SystemMedia)
}

/// A scripted player for tests: no real app is ever listed, played or paused.
pub mod fake {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use jk_recorder::RecorderError;
    use jk_recorder::media::{Cause, MediaObservation, MediaSessionInfo, PlaybackStatus, Thumbnail, TrackInfo};

    use super::{MediaLink, OnChange, WatchHandle};

    #[derive(Default)]
    struct State {
        sessions: Vec<MediaSessionInfo>,
        calls: Vec<String>,
        watchers: Vec<(u64, String, Arc<OnChange>)>,
        next_token: u64,
        thumbnail: Option<Thumbnail>,
        refuse_play: bool,
        on_play: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    /// A fake set of players. `set_track` / `set_status` change a player and report
    /// it to every watch of that player, like a real one.
    #[derive(Default)]
    pub struct FakeMedia {
        st: Arc<Mutex<State>>,
    }

    /// Stops the reports when dropped, like a real watch.
    struct FakeWatch {
        st: Arc<Mutex<State>>,
        token: u64,
    }
    impl WatchHandle for FakeWatch {}
    impl Drop for FakeWatch {
        fn drop(&mut self) {
            if let Ok(mut st) = self.st.lock() {
                st.watchers.retain(|(t, _, _)| *t != self.token);
            }
        }
    }

    impl FakeMedia {
        pub fn new() -> Arc<FakeMedia> {
            Arc::new(FakeMedia::default())
        }

        /// Add a player (paused, with a track).
        pub fn add_player(&self, id: &str, title: &str, artist: &str, duration_s: Option<f64>) {
            self.st.lock().unwrap().sessions.push(MediaSessionInfo {
                id: id.into(),
                source_app_id: id.into(),
                app_name: jk_recorder::media::friendly_app_name(id),
                is_current: true,
                status: PlaybackStatus::Paused,
                track: TrackInfo {
                    title: title.into(),
                    artist: artist.into(),
                    album: "Album".into(),
                    duration_s,
                    ..TrackInfo::default()
                },
                can_play: true,
                can_pause: true,
            });
        }

        pub fn set_thumbnail(&self, bytes: Vec<u8>) {
            self.st.lock().unwrap().thumbnail = Some(Thumbnail {
                bytes,
                content_type: "image/jpeg".into(),
            });
        }

        /// Called when a play command arrives (before the player "starts").
        pub fn on_play(&self, f: impl Fn() + Send + Sync + 'static) {
            self.st.lock().unwrap().on_play = Some(Arc::new(f));
        }

        pub fn refuse_play(&self, refuse: bool) {
            self.st.lock().unwrap().refuse_play = refuse;
        }

        /// Commands received, e.g. `["play Player.exe", "pause Player.exe"]`.
        pub fn calls(&self) -> Vec<String> {
            self.st.lock().unwrap().calls.clone()
        }

        fn report(&self, id: &str, cause: Cause, gone: bool) {
            let (obs, watchers) = {
                let st = self.st.lock().unwrap();
                let Some(s) = st.sessions.iter().find(|s| s.id == id) else { return };
                let obs = MediaObservation {
                    at: Instant::now(),
                    cause,
                    status: s.status,
                    track: s.track.clone(),
                    gone,
                };
                let w: Vec<Arc<OnChange>> = st.watchers.iter().filter(|(_, i, _)| i == id).map(|(_, _, f)| Arc::clone(f)).collect();
                (obs, w)
            };
            for w in watchers {
                w(obs.clone());
            }
        }

        pub fn set_track(&self, id: &str, title: &str, artist: &str, duration_s: Option<f64>) {
            {
                let mut st = self.st.lock().unwrap();
                if let Some(s) = st.sessions.iter_mut().find(|s| s.id == id) {
                    s.track = TrackInfo {
                        title: title.into(),
                        artist: artist.into(),
                        album: "Album".into(),
                        duration_s,
                        ..TrackInfo::default()
                    };
                }
            }
            self.report(id, Cause::MediaProperties, false);
        }

        pub fn set_status(&self, id: &str, status: PlaybackStatus) {
            {
                let mut st = self.st.lock().unwrap();
                if let Some(s) = st.sessions.iter_mut().find(|s| s.id == id) {
                    s.status = status;
                }
            }
            self.report(id, Cause::Playback, false);
        }

        /// The player is closed.
        pub fn close(&self, id: &str) {
            self.report(id, Cause::Poll, true);
            self.st.lock().unwrap().sessions.retain(|s| s.id != id);
        }
    }

    impl MediaLink for FakeMedia {
        fn available(&self) -> Result<(), String> {
            Ok(())
        }
        fn sessions(&self) -> Result<Vec<MediaSessionInfo>, RecorderError> {
            Ok(self.st.lock().unwrap().sessions.clone())
        }
        fn play(&self, id: &str) -> Result<(), RecorderError> {
            let hook = self.st.lock().unwrap().on_play.clone();
            if let Some(h) = hook {
                h();
            }
            {
                let mut st = self.st.lock().unwrap();
                st.calls.push(format!("play {id}"));
                if st.refuse_play {
                    return Err(RecorderError::Player("The player did not accept the play command.".into()));
                }
                if !st.sessions.iter().any(|s| s.id == id) {
                    return Err(RecorderError::Player(format!("{id} is no longer open.")));
                }
            }
            self.set_status(id, PlaybackStatus::Playing);
            Ok(())
        }
        fn pause(&self, id: &str) -> Result<(), RecorderError> {
            self.st.lock().unwrap().calls.push(format!("pause {id}"));
            self.set_status(id, PlaybackStatus::Paused);
            Ok(())
        }
        fn thumbnail(&self, _id: &str) -> Result<Option<Thumbnail>, RecorderError> {
            Ok(self.st.lock().unwrap().thumbnail.clone())
        }
        fn watch(&self, id: &str, on_change: OnChange) -> Result<Box<dyn WatchHandle>, RecorderError> {
            let f = Arc::new(on_change);
            let token = {
                let mut st = self.st.lock().unwrap();
                if !st.sessions.iter().any(|s| s.id == id) {
                    return Err(RecorderError::Player(format!("{id} is no longer open.")));
                }
                st.next_token += 1;
                let t = st.next_token;
                st.watchers.push((t, id.to_string(), Arc::clone(&f)));
                t
            };
            self.report(id, Cause::Start, false);
            Ok(Box::new(FakeWatch {
                st: Arc::clone(&self.st),
                token,
            }))
        }
    }
}

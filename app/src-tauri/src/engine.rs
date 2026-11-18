// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio engine as the app core sees it.
//!
//! The real implementation is `jk_engine::Engine` (crates/jk-engine/src/api.rs, the
//! contract between the engine and app-core work streams). The app talks to it through
//! the [`AudioEngine`] trait so that the command handlers, the clock thread and the
//! per-song settings saving can be tested without an audio device ([`fake`]).

use std::path::Path;
use std::sync::atomic::AtomicBool;

pub use jk_engine::{
    DeviceInfo, DeviceStatus, EngineConfig, EngineError, EngineEvent, ExportFormat, KeyMode,
    LoadedSong, MixParams, PlaybackState, RenderReport, Snapshot,
};

/// What the app needs from a player engine (mirrors `jk_engine::Engine`).
pub trait AudioEngine: Send + Sync + 'static {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, EngineError>;
    fn set_device(&self, id: Option<&str>, exclusive: bool) -> Result<(), EngineError>;
    fn load(&self, song: &Path, params: MixParams) -> Result<LoadedSong, EngineError>;
    fn unload(&self);
    fn play(&self);
    fn pause(&self);
    fn seek(&self, seconds: f64);
    fn set_params(&self, p: MixParams);
    fn set_master(&self, v: f32);
    fn snapshot(&self) -> Snapshot;
    fn events(&self) -> crossbeam_channel::Receiver<EngineEvent>;
    /// Key change by real time or render-ahead (WORK_ORDER 4.5). Default: ignored.
    fn set_key_mode(&self, _mode: KeyMode) {}
    /// The key cache folder and size cap. Default: ignored.
    fn set_key_cache(&self, _dir: Option<&Path>, _cap_bytes: u64) -> Result<(), EngineError> {
        Ok(())
    }
    /// Engine counters and timings (underruns, worker lead, longest callback...) for
    /// soak tests and support. Default: none.
    fn diagnostics(&self) -> Option<jk_engine::engine::Diagnostics> {
        None
    }
}

impl AudioEngine for jk_engine::Engine {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, EngineError> {
        jk_engine::Engine::list_devices()
    }
    fn set_device(&self, id: Option<&str>, exclusive: bool) -> Result<(), EngineError> {
        jk_engine::Engine::set_device(self, id, exclusive)
    }
    fn load(&self, song: &Path, params: MixParams) -> Result<LoadedSong, EngineError> {
        jk_engine::Engine::load(self, song, params)
    }
    fn unload(&self) {
        jk_engine::Engine::unload(self)
    }
    fn play(&self) {
        jk_engine::Engine::play(self)
    }
    fn pause(&self) {
        jk_engine::Engine::pause(self)
    }
    fn seek(&self, seconds: f64) {
        jk_engine::Engine::seek(self, seconds)
    }
    fn set_params(&self, p: MixParams) {
        jk_engine::Engine::set_params(self, p)
    }
    fn set_master(&self, v: f32) {
        jk_engine::Engine::set_master(self, v)
    }
    fn snapshot(&self) -> Snapshot {
        jk_engine::Engine::snapshot(self)
    }
    fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        jk_engine::Engine::events(self)
    }
    fn set_key_mode(&self, mode: KeyMode) {
        jk_engine::Engine::set_key_mode(self, mode)
    }
    fn set_key_cache(&self, dir: Option<&Path>, cap_bytes: u64) -> Result<(), EngineError> {
        jk_engine::Engine::set_key_cache(self, dir, cap_bytes)
    }
    fn diagnostics(&self) -> Option<jk_engine::engine::Diagnostics> {
        Some(jk_engine::Engine::diagnostics(self))
    }
}

/// Offline rendering ("Export backing track", self-check). Real implementation:
/// `jk_engine::render_offline`.
pub trait Renderer: Send + Sync + 'static {
    fn render(
        &self,
        song: &Path,
        p: &MixParams,
        out: &Path,
        fmt: ExportFormat,
        progress: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<RenderReport, EngineError>;
}

/// `jk_engine::render_offline`.
pub struct EngineRenderer;

impl Renderer for EngineRenderer {
    fn render(
        &self,
        song: &Path,
        p: &MixParams,
        out: &Path,
        fmt: ExportFormat,
        progress: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<RenderReport, EngineError> {
        jk_engine::render_offline(song, p, out, fmt, progress, cancel)
    }
}

/// An empty snapshot (no song, no device).
pub fn empty_snapshot() -> Snapshot {
    Snapshot {
        state: PlaybackState::Empty,
        position_s: 0.0,
        duration_s: 0.0,
        device: None,
        underruns: 0,
        device_starved: 0,
        output_peak: [0.0; 2],
        clipped: false,
        error: None,
        position_ns: 0,
        key: Default::default(),
    }
}

/// Stand-in used when `Engine::new` itself failed: every playback call reports the
/// reason, the rest of the app (library, Studio, recorder) keeps working.
pub struct NoEngine {
    pub error: EngineError,
    rx: crossbeam_channel::Receiver<EngineEvent>,
    _tx: crossbeam_channel::Sender<EngineEvent>,
}

impl NoEngine {
    pub fn new(error: EngineError) -> NoEngine {
        let (tx, rx) = crossbeam_channel::bounded(1);
        NoEngine { error, rx, _tx: tx }
    }
}

impl AudioEngine for NoEngine {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, EngineError> {
        jk_engine::Engine::list_devices()
    }
    fn set_device(&self, _id: Option<&str>, _exclusive: bool) -> Result<(), EngineError> {
        Err(self.error.clone())
    }
    fn load(&self, _song: &Path, _params: MixParams) -> Result<LoadedSong, EngineError> {
        Err(self.error.clone())
    }
    fn unload(&self) {}
    fn play(&self) {}
    fn pause(&self) {}
    fn seek(&self, _seconds: f64) {}
    fn set_params(&self, _p: MixParams) {}
    fn set_master(&self, _v: f32) {}
    fn snapshot(&self) -> Snapshot {
        let mut s = empty_snapshot();
        s.error = Some(self.error.message.clone());
        s
    }
    fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        self.rx.clone()
    }
}

/// A fake engine for tests: no device, no audio. Time advances with the wall clock
/// while "playing"; every call is recorded.
pub mod fake {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        Load(std::path::PathBuf, MixParams),
        Unload,
        Play,
        Pause,
        Seek(f64),
        SetParams(MixParams),
        SetMaster(f32),
        SetDevice(Option<String>, bool),
    }

    struct St {
        state: PlaybackState,
        base_pos: f64,
        since: Option<Instant>,
        duration: f64,
    }

    pub struct FakeEngine {
        pub calls: Mutex<Vec<Call>>,
        /// Key modes set, in order (kept apart from `calls`, which tests compare whole).
        pub key_modes: Mutex<Vec<KeyMode>>,
        pub key_cache: Mutex<Option<(std::path::PathBuf, u64)>>,
        /// Keep the loaded song's file open until `unload`, as the real engine's reader
        /// does; on Windows without delete sharing, so a move of the file fails while
        /// it is loaded (tests of "release the song before moving it").
        pub hold_open: std::sync::atomic::AtomicBool,
        held: Mutex<Option<std::fs::File>>,
        st: Mutex<St>,
        tx: crossbeam_channel::Sender<EngineEvent>,
        rx: crossbeam_channel::Receiver<EngineEvent>,
    }

    impl Default for FakeEngine {
        fn default() -> Self {
            let (tx, rx) = crossbeam_channel::bounded(256);
            FakeEngine {
                calls: Mutex::new(Vec::new()),
                key_modes: Mutex::new(Vec::new()),
                key_cache: Mutex::new(None),
                hold_open: std::sync::atomic::AtomicBool::new(false),
                held: Mutex::new(None),
                st: Mutex::new(St {
                    state: PlaybackState::Empty,
                    base_pos: 0.0,
                    since: None,
                    duration: 0.0,
                }),
                tx,
                rx,
            }
        }
    }

    impl FakeEngine {
        fn record(&self, c: Call) {
            self.calls.lock().unwrap().push(c);
        }
        fn pos(st: &St) -> f64 {
            let p = st.base_pos + st.since.map_or(0.0, |t| t.elapsed().as_secs_f64());
            p.min(st.duration)
        }
        /// Sends an event as the real engine would.
        pub fn inject(&self, ev: EngineEvent) {
            let _ = self.tx.send(ev);
        }
        pub fn take_calls(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
    }

    impl AudioEngine for FakeEngine {
        fn list_devices(&self) -> Result<Vec<DeviceInfo>, EngineError> {
            Ok(vec![DeviceInfo {
                id: "fake-1".into(),
                name: "Fake Speakers".into(),
                is_default: true,
                sample_rate: 48000,
                channels: 2,
                exclusive_capable: true,
            }])
        }
        fn set_device(&self, id: Option<&str>, exclusive: bool) -> Result<(), EngineError> {
            self.record(Call::SetDevice(id.map(str::to_string), exclusive));
            Ok(())
        }
        fn load(&self, song: &Path, params: MixParams) -> Result<LoadedSong, EngineError> {
            self.record(Call::Load(song.to_path_buf(), params));
            let f = jk_format::SongFile::open(song).map_err(|e| {
                EngineError::new("song_damaged", e.user_message(), e.to_string())
            })?;
            let m = f.manifest();
            if self.hold_open.load(std::sync::atomic::Ordering::SeqCst) {
                let mut o = std::fs::OpenOptions::new();
                o.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    // FILE_SHARE_READ | FILE_SHARE_WRITE: slot saves work, a delete or
                    // rename does not while held.
                    o.share_mode(3);
                }
                *self.held.lock().unwrap() = o.open(song).ok();
            }
            let mut st = self.st.lock().unwrap();
            *st = St {
                state: PlaybackState::Paused,
                base_pos: 0.0,
                since: None,
                duration: m.duration(),
            };
            Ok(LoadedSong {
                duration_s: m.duration(),
                sample_rate: m.sample_rate,
                channels: m.channels,
                stems: m
                    .stems
                    .iter()
                    .map(|s| jk_engine::StemInfo {
                        role: s.role.clone(),
                        in_karaoke_mix: s.in_karaoke_mix(),
                        pitched: s.pitched(),
                    })
                    .collect(),
            })
        }
        fn unload(&self) {
            self.record(Call::Unload);
            *self.held.lock().unwrap() = None;
            let mut st = self.st.lock().unwrap();
            st.state = PlaybackState::Empty;
            st.since = None;
            st.base_pos = 0.0;
        }
        fn play(&self) {
            self.record(Call::Play);
            let mut st = self.st.lock().unwrap();
            if st.state == PlaybackState::Paused {
                st.state = PlaybackState::Playing;
                st.since = Some(Instant::now());
            }
        }
        fn pause(&self) {
            self.record(Call::Pause);
            let mut st = self.st.lock().unwrap();
            if st.state == PlaybackState::Playing {
                st.base_pos = Self::pos(&st);
                st.since = None;
                st.state = PlaybackState::Paused;
            }
        }
        fn seek(&self, seconds: f64) {
            self.record(Call::Seek(seconds));
            let mut st = self.st.lock().unwrap();
            st.base_pos = seconds.clamp(0.0, st.duration);
            if st.since.is_some() {
                st.since = Some(Instant::now());
            }
        }
        fn set_params(&self, p: MixParams) {
            self.record(Call::SetParams(p));
        }
        fn set_master(&self, v: f32) {
            self.record(Call::SetMaster(v));
        }
        fn snapshot(&self) -> Snapshot {
            let st = self.st.lock().unwrap();
            let mut s = empty_snapshot();
            s.state = st.state;
            s.position_s = Self::pos(&st);
            s.duration_s = st.duration;
            s.device = Some(DeviceStatus {
                name: "Fake Speakers".into(),
                sample_rate: 48000,
                channels: 2,
                exclusive: false,
                latency_ms: 20.0,
            });
            s
        }
        fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
            self.rx.clone()
        }
        fn set_key_mode(&self, mode: KeyMode) {
            self.key_modes.lock().unwrap().push(mode);
        }
        fn set_key_cache(&self, dir: Option<&Path>, cap_bytes: u64) -> Result<(), EngineError> {
            *self.key_cache.lock().unwrap() = dir.map(|d| (d.to_path_buf(), cap_bytes));
            Ok(())
        }
    }

    /// Writes a few bytes instead of rendering; can be told to fail or be slow.
    pub struct FakeRenderer {
        pub fail: Option<EngineError>,
        pub steps: u32,
        pub step_ms: u64,
        pub calls: Mutex<Vec<(std::path::PathBuf, MixParams, ExportFormat)>>,
    }

    impl Default for FakeRenderer {
        fn default() -> Self {
            FakeRenderer {
                fail: None,
                steps: 4,
                step_ms: 5,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Renderer for FakeRenderer {
        fn render(
            &self,
            song: &Path,
            p: &MixParams,
            out: &Path,
            fmt: ExportFormat,
            progress: &mut dyn FnMut(f32),
            cancel: &AtomicBool,
        ) -> Result<RenderReport, EngineError> {
            self.calls
                .lock()
                .unwrap()
                .push((song.to_path_buf(), *p, fmt));
            for i in 0..self.steps {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(EngineError::new("cancelled", "Cancelled.", ""));
                }
                std::thread::sleep(std::time::Duration::from_millis(self.step_ms));
                progress((i + 1) as f32 / self.steps as f32);
            }
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            std::fs::write(out, b"fake render")
                .map_err(|e| EngineError::new("write_failed", "Could not write.", e.to_string()))?;
            Ok(RenderReport {
                peak: 0.5,
                applied_gain_db: 0.0,
                frames: 48000,
                sample_rate: 48000,
                channels: 2,
            })
        }
    }
}

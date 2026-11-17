// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading and playback (docs/APP_API.md section 3), on top of the engine.
//!
//! - `song_load` flushes the previous song's pending settings, opens the new song file
//!   (effective lyrics, settings, metadata), and loads it into the engine paused at 0.
//! - `player_set` applies a change to the engine at once (the engine ramps gains and
//!   crossfades key changes, so nothing clicks) and saves it into the song file's
//!   settings slot about 1 s after the last change ([`SlotSaver`]). The save is
//!   flushed on song change and on exit.
//! - A dedicated clock thread reads `Engine::snapshot()` about 30 times per second
//!   while playing and emits `playback://clock`; it also turns engine notifications
//!   into `playback://state` and `playback://error`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::debounce::Debouncer;
use crate::engine::{AudioEngine, EngineError, EngineEvent, MixParams, PlaybackState, Snapshot};
use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};
use crate::keys::{Key, key_label};
use crate::library::{Library, SongSummary};

/// The singer's settings for a song (docs/APP_API.md `SongSettings`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SongSettings {
    pub semitones: i32,
    pub shift_drums: bool,
    /// 0..1.
    pub guide_vocal: f64,
    pub backing_on: bool,
    pub backing_level: f64,
    /// Level per stem role; always has `drums` and `other`.
    pub levels: BTreeMap<String, f64>,
    pub lyrics_offset_ms: i64,
}

impl SongSettings {
    pub fn from_format(s: &jk_format::Settings) -> SongSettings {
        let mut levels = s.levels.clone();
        for role in ["drums", "other"] {
            levels.entry(role.to_string()).or_insert(1.0);
        }
        SongSettings {
            semitones: s.semitones_clamped(),
            shift_drums: s.shift_drums,
            guide_vocal: s.guide_vocal,
            backing_on: s.backing_on,
            backing_level: s.backing_level,
            levels,
            lyrics_offset_ms: s.lyrics_offset_ms,
        }
    }
}

/// Highest level a mix fader can set (+6 dB). Levels are the singer's choice; nothing
/// limits the output afterwards (no hidden processing), the UI shows a clip warning.
pub const MAX_LEVEL: f64 = 2.0;
/// Per-song lyric offset range, in ms.
pub const MAX_OFFSET_MS: i64 = 10_000;

fn level(v: &Value, what: &str) -> ApiResult<f64> {
    let x = v
        .as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| ApiError::bad_request(format!("{what} must be a number.")))?;
    Ok(x.clamp(0.0, MAX_LEVEL))
}

/// Applies a `Partial<SongSettings>` object to stored settings, with checks and
/// clamping. Unknown keys are refused (a typo must not be silently ignored).
pub fn apply_changes(s: &mut jk_format::Settings, changes: &Value) -> ApiResult<()> {
    let obj = changes
        .as_object()
        .ok_or_else(|| ApiError::bad_request("Settings changes must be an object."))?;
    // Validate everything first, so a bad key changes nothing.
    let mut next = s.clone();
    for (k, v) in obj {
        match k.as_str() {
            "semitones" => {
                let x = v
                    .as_f64()
                    .filter(|x| x.is_finite() && x.fract() == 0.0)
                    .ok_or_else(|| ApiError::bad_request("semitones must be a whole number."))?;
                next.semitones = (x as i64).clamp(-12, 12) as i32;
            }
            "shift_drums" => {
                next.shift_drums = v
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request("shift_drums must be true or false."))?;
            }
            "guide_vocal" => next.guide_vocal = level(v, "guide_vocal")?.min(1.0),
            "backing_on" => {
                next.backing_on = v
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request("backing_on must be true or false."))?;
            }
            "backing_level" => next.backing_level = level(v, "backing_level")?,
            "levels" => {
                let m = v
                    .as_object()
                    .ok_or_else(|| ApiError::bad_request("levels must be an object."))?;
                for (role, lv) in m {
                    next.levels.insert(role.clone(), level(lv, "a level")?);
                }
            }
            "lyrics_offset_ms" => {
                let x = v
                    .as_f64()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| ApiError::bad_request("lyrics_offset_ms must be a number."))?;
                next.lyrics_offset_ms = (x.round() as i64).clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
            }
            other => {
                return Err(ApiError::bad_request(format!("Unknown song setting '{other}'.")));
            }
        }
    }
    *s = next;
    Ok(())
}

/// Copies the known settings fields into a stored state (keeps unknown keys).
fn store_settings(dst: &mut jk_format::Settings, src: &jk_format::Settings) {
    dst.semitones = src.semitones;
    dst.shift_drums = src.shift_drums;
    dst.guide_vocal = src.guide_vocal;
    dst.backing_on = src.backing_on;
    dst.backing_level = src.backing_level;
    dst.levels = src.levels.clone();
    dst.lyrics_offset_ms = src.lyrics_offset_ms;
}

/// Saves a song's settings into its settings slot about 1 s after the last change.
pub struct SlotSaver {
    pending: Arc<Mutex<Option<(PathBuf, jk_format::Settings)>>>,
    debouncer: Debouncer,
    /// Number of slot writes done (for tests and diagnostics).
    pub writes: Arc<AtomicU64>,
}

impl SlotSaver {
    pub fn new(delay: Duration) -> SlotSaver {
        let pending: Arc<Mutex<Option<(PathBuf, jk_format::Settings)>>> = Arc::new(Mutex::new(None));
        let writes = Arc::new(AtomicU64::new(0));
        let (p, w) = (Arc::clone(&pending), Arc::clone(&writes));
        let debouncer = Debouncer::new("song-slot", delay, move || {
            let job = p.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some((path, settings)) = job {
                match jk_format::update_user_state(&path, |st| store_settings(&mut st.settings, &settings)) {
                    Ok(r) => {
                        w.fetch_add(1, Ordering::SeqCst);
                        crate::applog::info(&format!(
                            "saved song settings to {} (slot {}, sequence {})",
                            path.display(),
                            if r.slot == 0 { 'A' } else { 'B' },
                            r.sequence
                        ));
                    }
                    Err(e) => crate::applog::error(&format!(
                        "saving song settings to {} failed: {e}",
                        path.display()
                    )),
                }
            }
        });
        SlotSaver {
            pending,
            debouncer,
            writes,
        }
    }

    /// Schedules a save. A pending save for a different song is written first.
    pub fn schedule(&self, path: &Path, settings: &jk_format::Settings) {
        let other_pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|(p, _)| p != path);
        if other_pending {
            self.flush();
        }
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((path.to_path_buf(), settings.clone()));
        self.debouncer.touch();
    }

    /// Writes a pending save now.
    pub fn flush(&self) {
        self.debouncer.flush();
    }

    pub fn pending(&self) -> bool {
        self.debouncer.pending()
    }
}

/// The device as the UI sees it (docs/APP_API.md `PlayerState.device`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeviceView {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub exclusive: bool,
    pub latency_ms: f64,
}

/// docs/APP_API.md `PlayerState` (plus `clipped` and `device_starved`, additions).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlayerState {
    pub state: &'static str,
    pub song_id: Option<String>,
    pub position_s: f64,
    pub duration_s: f64,
    pub device: Option<DeviceView>,
    pub underruns: u64,
    pub error: Option<String>,
    pub key_label: Option<String>,
    /// Addition: the output went above full scale (nothing limits it; the UI warns).
    pub clipped: bool,
    /// Addition: WASAPI device-side starvation events (OS scheduling glitches).
    pub device_starved: u64,
    /// Addition: output peak per channel over the last ~0.3 s, in dBFS (master volume
    /// included; -120 stands for silence, since JSON has no -infinity).
    pub output_peak_db: Vec<f64>,
}

/// Linear peak to dBFS for the UI, floored at -120 dB.
fn peak_db(x: f32) -> f64 {
    let x = x as f64;
    if x.is_finite() && x > 1e-6 { (20.0 * x.log10()).max(-120.0) } else { -120.0 }
}

/// `playback://clock` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClockEvent {
    pub position_s: f64,
    pub playing: bool,
    pub seq: u64,
}

/// `song_load` result (docs/APP_API.md `SongDetail`).
#[derive(Clone, Debug, Serialize)]
pub struct SongDetail {
    pub summary: SongSummary,
    pub lyrics: Option<jk_format::Lyrics>,
    pub lyrics_is_override: bool,
    pub settings: SongSettings,
    pub stems: Vec<jk_engine::StemInfo>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// The loaded song.
#[derive(Clone, Debug)]
pub struct Current {
    pub song_id: String,
    pub path: PathBuf,
    pub settings: jk_format::Settings,
    pub key: Option<Key>,
    pub duration_s: f64,
}

pub struct Player {
    engine: Arc<dyn AudioEngine>,
    sink: Arc<dyn EventSink>,
    library: Arc<Library>,
    current: Mutex<Option<Current>>,
    /// Serialises loads (and the unload-rewrite-reload of a large lyrics save).
    load_lock: Mutex<()>,
    loading: AtomicBool,
    pub saver: SlotSaver,
    seq: AtomicU64,
    clock: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
    /// Wakes the clock thread at once after a command changed the state (it would
    /// otherwise notice a play only at its next idle check, up to 250 ms later).
    clock_wake: (crossbeam_channel::Sender<()>, crossbeam_channel::Receiver<()>),
}

fn state_name(s: PlaybackState) -> &'static str {
    match s {
        PlaybackState::Empty => "empty",
        PlaybackState::Paused => "paused",
        PlaybackState::Playing => "playing",
        PlaybackState::Ended => "ended",
        PlaybackState::Error => "error",
    }
}

/// Song time now, extrapolated from the engine's last device callback (the snapshot's
/// `position_ns`) so the UI anchors on the time the event was made, not on the time
/// of the last device buffer (up to one buffer earlier). Bounded to 100 ms.
fn position_now(s: &Snapshot) -> f64 {
    if s.state == PlaybackState::Playing && s.position_ns != 0 {
        let now = jk_engine::now_ns();
        let ahead = (now.saturating_sub(s.position_ns) as f64 * 1e-9).min(0.1);
        (s.position_s + ahead).min(s.duration_s.max(s.position_s))
    } else {
        s.position_s
    }
}

impl Player {
    pub fn new(
        engine: Arc<dyn AudioEngine>,
        sink: Arc<dyn EventSink>,
        library: Arc<Library>,
        save_delay: Duration,
    ) -> Player {
        Player {
            engine,
            sink,
            library,
            current: Mutex::new(None),
            load_lock: Mutex::new(()),
            loading: AtomicBool::new(false),
            saver: SlotSaver::new(save_delay),
            seq: AtomicU64::new(0),
            clock: Mutex::new(None),
            clock_wake: crossbeam_channel::bounded(1),
        }
    }

    pub fn engine(&self) -> &Arc<dyn AudioEngine> {
        &self.engine
    }

    pub fn current(&self) -> Option<Current> {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_current(&self, c: Option<Current>) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = c;
    }

    /// `player_state`.
    pub fn state(&self) -> PlayerState {
        let snap = self.engine.snapshot();
        self.state_from(&snap)
    }

    fn state_from(&self, snap: &Snapshot) -> PlayerState {
        let cur = self.current();
        let loading = self.loading.load(Ordering::SeqCst);
        PlayerState {
            state: if loading { "loading" } else { state_name(snap.state) },
            song_id: cur.as_ref().map(|c| c.song_id.clone()),
            position_s: position_now(snap),
            duration_s: if snap.duration_s > 0.0 {
                snap.duration_s
            } else {
                cur.as_ref().map_or(0.0, |c| c.duration_s)
            },
            device: snap.device.as_ref().map(|d| DeviceView {
                name: d.name.clone(),
                sample_rate: d.sample_rate,
                channels: d.channels,
                exclusive: d.exclusive,
                latency_ms: d.latency_ms,
            }),
            underruns: snap.underruns,
            error: snap.error.clone(),
            key_label: cur
                .as_ref()
                .map(|c| key_label(c.key, c.settings.semitones_clamped())),
            clipped: snap.clipped,
            device_starved: snap.device_starved,
            output_peak_db: snap.output_peak.iter().map(|&p| peak_db(p)).collect(),
        }
    }

    fn emit_state(&self) -> PlayerState {
        let _ = self.clock_wake.0.try_send(());
        let snap = self.engine.snapshot();
        let st = self.state_from(&snap);
        emit(&*self.sink, names::PLAYBACK_STATE, &st);
        self.emit_clock(&snap);
        st
    }

    fn emit_clock(&self, snap: &Snapshot) {
        let ev = ClockEvent {
            position_s: position_now(snap),
            playing: snap.state == PlaybackState::Playing,
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
        };
        emit(&*self.sink, names::PLAYBACK_CLOCK, &ev);
    }

    fn emit_engine_error(&self, e: &EngineError) {
        crate::applog::warn(&format!("playback error: {e}"));
        emit(
            &*self.sink,
            names::PLAYBACK_ERROR,
            &ApiError::new(e.code.clone(), e.message.clone()),
        );
    }

    /// `song_load`: stops any current song and opens `song_id` paused at 0.
    pub fn load(&self, song_id: &str) -> ApiResult<SongDetail> {
        let path = self.library.resolve(song_id)?;
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        // The previous song's settings go to its file before anything else happens.
        self.saver.flush();
        self.loading.store(true, Ordering::SeqCst);
        self.emit_state();
        let result = self.load_inner(song_id, &path);
        self.loading.store(false, Ordering::SeqCst);
        if result.is_err() {
            self.set_current(None);
        }
        self.emit_state();
        result
    }

    fn load_inner(&self, song_id: &str, path: &Path) -> ApiResult<SongDetail> {
        let song = jk_format::SongFile::open(path)?;
        for w in song.warnings() {
            crate::applog::warn(&format!("{song_id}: {w}"));
        }
        let settings = song.effective_settings().clone();
        let override_lyrics = song.user_state().lyrics_override.is_some();
        let lyrics = match song.effective_lyrics() {
            Ok(l) => l,
            Err(e) => {
                // Damaged lyrics must not stop the music: play without them.
                crate::applog::warn(&format!("{song_id}: lyrics unreadable: {e}"));
                None
            }
        };
        let st = std::fs::metadata(path)
            .map(|m| {
                (
                    m.len(),
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis() as u64),
                )
            })
            .unwrap_or((0, 0));
        let summary = crate::library::summary_of(&song, song_id.to_string(), st);
        let key = crate::library::song_key(&song);
        drop(song);

        // Unload first so the old song's file is closed even if the new one fails.
        self.set_current(None);
        let loaded = self
            .engine
            .load(path, MixParams::from_settings(&settings))
            .map_err(|e| {
                self.emit_engine_error(&e);
                ApiError::new(e.code.clone(), e.message.clone())
            })?;
        self.set_current(Some(Current {
            song_id: song_id.to_string(),
            path: path.to_path_buf(),
            settings: settings.clone(),
            key,
            duration_s: loaded.duration_s,
        }));
        Ok(SongDetail {
            summary,
            lyrics,
            lyrics_is_override: override_lyrics,
            settings: SongSettings::from_format(&settings),
            stems: loaded.stems,
            sample_rate: loaded.sample_rate,
            channels: loaded.channels,
        })
    }

    fn require_loaded(&self) -> ApiResult<Current> {
        self.current()
            .ok_or_else(|| ApiError::new("not_loaded", "No song is loaded. Choose a song first."))
    }

    pub fn play(&self) -> ApiResult<()> {
        self.require_loaded()?;
        self.engine.play();
        self.emit_state();
        Ok(())
    }

    pub fn pause(&self) -> ApiResult<()> {
        self.require_loaded()?;
        self.engine.pause();
        self.emit_state();
        Ok(())
    }

    pub fn toggle(&self) -> ApiResult<()> {
        self.require_loaded()?;
        if self.engine.snapshot().state == PlaybackState::Playing {
            self.engine.pause();
        } else {
            self.engine.play();
        }
        self.emit_state();
        Ok(())
    }

    /// `player_stop`: pause and go back to the start (the song stays loaded).
    pub fn stop(&self) -> ApiResult<()> {
        self.require_loaded()?;
        self.engine.pause();
        self.engine.seek(0.0);
        self.emit_state();
        Ok(())
    }

    pub fn seek(&self, position_s: f64) -> ApiResult<()> {
        let cur = self.require_loaded()?;
        if !position_s.is_finite() {
            return Err(ApiError::bad_request("The position must be a number of seconds."));
        }
        self.engine.seek(position_s.clamp(0.0, cur.duration_s.max(0.0)));
        self.emit_state();
        Ok(())
    }

    /// `player_set`: applies live and schedules the slot save.
    pub fn set(&self, changes: &Value) -> ApiResult<SongSettings> {
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let c = cur
            .as_mut()
            .ok_or_else(|| ApiError::new("not_loaded", "No song is loaded. Choose a song first."))?;
        apply_changes(&mut c.settings, changes)?;
        let params = MixParams::from_settings(&c.settings);
        let out = SongSettings::from_format(&c.settings);
        self.saver.schedule(&c.path, &c.settings);
        drop(cur);
        self.engine.set_params(params);
        self.emit_state();
        Ok(out)
    }

    pub fn set_master(&self, volume: f64) -> ApiResult<f32> {
        if !volume.is_finite() {
            return Err(ApiError::bad_request("The volume must be a number from 0 to 1."));
        }
        let v = volume.clamp(0.0, 1.0) as f32;
        self.engine.set_master(v);
        Ok(v)
    }

    /// Updates the in-memory copy of the current song's user state after a direct slot
    /// write (lyrics or metadata saved), so `player_state` and exports stay right.
    pub fn refresh_current_from_disk(&self, path: &Path) {
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cur.as_mut().filter(|c| c.path == path)
            && let Ok(song) = jk_format::SongFile::open(path)
        {
            // Keep live (maybe unsaved) mix settings; take everything else from disk.
            c.key = crate::library::song_key(&song);
        }
    }

    /// Runs `f` with the current song closed in the engine when it is `path` (a full
    /// file rewrite on Windows needs every handle closed), then reloads it at the same
    /// position and play state.
    pub fn with_song_released<T>(&self, path: &Path, f: impl FnOnce() -> ApiResult<T>) -> ApiResult<T> {
        let _guard = self.load_lock.lock().unwrap_or_else(|e| e.into_inner());
        let cur = self.current().filter(|c| c.path == path);
        let Some(cur) = cur else { return f() };
        self.saver.flush();
        let snap = self.engine.snapshot();
        let was_playing = snap.state == PlaybackState::Playing;
        self.engine.unload();
        let result = f();
        match self.engine.load(path, MixParams::from_settings(&cur.settings)) {
            Ok(_) => {
                self.engine.seek(snap.position_s);
                if was_playing {
                    self.engine.play();
                }
            }
            Err(e) => {
                self.set_current(None);
                self.emit_engine_error(&e);
            }
        }
        self.emit_state();
        result
    }

    /// `device_set`.
    pub fn set_device(&self, id: Option<&str>, exclusive: bool) -> ApiResult<()> {
        let r = self.engine.set_device(id, exclusive);
        self.emit_state();
        r.map_err(|e| {
            crate::applog::warn(&format!("device_set: {e}"));
            ApiError::new(e.code.clone(), e.message.clone())
        })
    }

    /// Starts the clock thread (playback://clock, state and error events).
    pub fn start_clock(self: &Arc<Self>) {
        let stop = Arc::new(AtomicBool::new(false));
        let (me, s) = (Arc::clone(self), Arc::clone(&stop));
        let handle = std::thread::Builder::new()
            .name("playback-clock".into())
            .spawn(move || me.clock_loop(&s))
            .expect("spawn clock thread");
        *self.clock.lock().unwrap_or_else(|e| e.into_inner()) = Some((stop, handle));
    }

    fn clock_loop(&self, stop: &AtomicBool) {
        // ~30 Hz while playing, on absolute deadlines: the OS timer granularity (15.6 ms
        // on Windows by default) then makes single ticks early or late, but the rate
        // stays 30 per second instead of drifting down to every other tick.
        const PLAYING_PERIOD: Duration = Duration::from_micros(33_333);
        // While not playing, look for state changes 4 times a second.
        const IDLE_PERIOD: Duration = Duration::from_millis(250);
        // A tick this close to its deadline is taken now rather than waited for.
        const SLACK: Duration = Duration::from_millis(4);
        let rx = self.engine.events();
        let mut last_state: Option<(&'static str, Option<String>)> = None;
        let mut next_tick = Instant::now();
        let mut last_pos = f64::NAN;
        while !stop.load(Ordering::SeqCst) {
            let wait = next_tick.saturating_duration_since(Instant::now());
            let event = crossbeam_channel::select! {
                recv(rx) -> ev => ev.map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected),
                recv(self.clock_wake.1) -> _ => {
                    // A command changed the state: look now, and restart the grid.
                    next_tick = Instant::now();
                    Err(crossbeam_channel::RecvTimeoutError::Timeout)
                }
                default(wait) => Err(crossbeam_channel::RecvTimeoutError::Timeout),
            };
            match event {
                Ok(EngineEvent::Error { code, message }) => {
                    self.emit_engine_error(&EngineError::new(&code, message, ""));
                }
                Ok(EngineEvent::DeviceLost) => {
                    self.emit_engine_error(&EngineError::new(
                        "device_lost",
                        "The audio output device was disconnected. Playback is paused; press play to continue on the default output.",
                        "",
                    ));
                }
                Ok(EngineEvent::StateChanged(_)) | Ok(EngineEvent::Ended) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(wait.max(Duration::from_millis(10)));
                }
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            // The snapshot is the truth; engine events only wake us early.
            let snap = self.engine.snapshot();
            let st = self.state_from(&snap);
            let key = (st.state, st.song_id.clone());
            let changed = last_state.as_ref() != Some(&key);
            if changed {
                emit(&*self.sink, names::PLAYBACK_STATE, &st);
                last_state = Some(key);
            }
            let now = Instant::now();
            let due = now + SLACK >= next_tick;
            let is_playing = snap.state == PlaybackState::Playing;
            let moved = snap.position_s != last_pos;
            if changed || (due && (is_playing || moved)) {
                self.emit_clock(&snap);
                last_pos = snap.position_s;
            }
            if due || changed {
                let period = if is_playing { PLAYING_PERIOD } else { IDLE_PERIOD };
                next_tick += period;
                // Fell behind (a slow event, a suspended laptop): restart the grid.
                if next_tick + period < now || !is_playing {
                    next_tick = now + period;
                }
            }
        }
    }

    /// On exit: write pending settings, stop the clock thread, close the song.
    pub fn shutdown(&self) {
        self.saver.flush();
        if let Some((stop, handle)) = self.clock.lock().unwrap_or_else(|e| e.into_inner()).take() {
            stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        self.engine.unload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn changes_are_checked_clamped_and_atomic() {
        let mut s = jk_format::Settings::default();
        apply_changes(
            &mut s,
            &json!({"semitones": 15, "guide_vocal": 1.5, "levels": {"drums": 0.5},
                    "lyrics_offset_ms": 120.4, "backing_on": false}),
        )
        .unwrap();
        assert_eq!(s.semitones, 12);
        assert_eq!(s.guide_vocal, 1.0);
        assert_eq!(s.level("drums"), 0.5);
        assert_eq!(s.lyrics_offset_ms, 120);
        assert!(!s.backing_on);
        let before = s.clone();
        assert!(apply_changes(&mut s, &json!({"semitones": 2, "bogus": 1})).is_err());
        assert!(apply_changes(&mut s, &json!({"semitones": 2.5})).is_err());
        assert!(apply_changes(&mut s, &json!({"shift_drums": "yes"})).is_err());
        assert_eq!(s, before, "a refused change changes nothing");
        let view = SongSettings::from_format(&jk_format::Settings::default());
        assert_eq!(view.levels["drums"], 1.0);
        assert_eq!(view.levels["other"], 1.0);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The public engine API: the contract between the audio engine (this crate) and the
//! app core (`app/src-tauri`). Everything the app needs is here; the modules behind it
//! are implementation detail.
//!
//! ```text
//! Engine::new(cfg)            opens the output device (or the "null" device), starts
//!                             the house-keeping thread. Never fails because a device
//!                             is missing: playback then reports a `no_device` error.
//! engine.load(path, params)   opens a .jksong, paused at 0.
//! engine.play() / pause() / seek(s) / set_params(p) / set_master(v)
//! engine.snapshot()           state, song time at the DAC, device, counters, meter,
//!                             the key heard there and the next key change (`key`).
//! engine.key_at(t, segment)   the key that was heard at song time t (recent past).
//! engine.events()             StateChanged / Ended / Error / DeviceLost notifications.
//! render_offline(...)         "Export backing track": same graph, single-threaded.
//! ```
//!
//! Thread safety: `Engine` is `Send + Sync`; every method takes `&self` and returns
//! quickly (no method waits for audio to play). Methods may be called from any thread.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

/// How to start the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Output device id from [`Engine::list_devices`]; `None` = the system default.
    /// The special id `"null"` (or `"null:<rate>"`, e.g. `"null:44100"`) is a device
    /// that plays nothing but consumes audio in real time (tests, headless self-check).
    pub device_id: Option<String>,
    /// WASAPI exclusive mode (Windows only; ignored elsewhere).
    pub exclusive: bool,
    /// Master volume, linear 0..1 (applied last, smoothed; never above 1).
    pub master: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig { device_id: None, exclusive: false, master: 1.0 }
    }
}

/// An output device as listed for the device picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable id to pass back to [`Engine::set_device`] (WASAPI endpoint id on
    /// Windows, the device name on macOS).
    pub id: String,
    /// Friendly name for the UI, e.g. "Speakers (Realtek(R) Audio)".
    pub name: String,
    pub is_default: bool,
    /// Shared-mode (mix) sample rate: what playback runs at unless exclusive mode is on.
    pub sample_rate: u32,
    pub channels: u16,
    /// The device accepts at least one exclusive-mode format (Windows only).
    pub exclusive_capable: bool,
}

/// The device that is open right now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub exclusive: bool,
    /// Output latency the device reports (queued buffer + stream latency), which the
    /// playback clock already subtracts.
    pub latency_ms: f64,
}

/// The singer's effective settings for the loaded song (SONG_FORMAT section 6, with
/// the app's defaults filled in). Gains are linear.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixParams {
    /// Key shift, -12..=12 (clamped).
    pub semitones: i32,
    /// Send the drums through the key change too (default false).
    pub shift_drums: bool,
    /// Guide vocal (the original lead singer), 0..1, default 0.
    pub guide_vocal: f32,
    pub backing_on: bool,
    pub backing_level: f32,
    pub drums: f32,
    pub other: f32,
}

impl Default for MixParams {
    fn default() -> Self {
        MixParams {
            semitones: 0,
            shift_drums: false,
            guide_vocal: 0.0,
            backing_on: true,
            backing_level: 1.0,
            drums: 1.0,
            other: 1.0,
        }
    }
}

impl MixParams {
    /// The song file's saved settings as engine parameters.
    pub fn from_settings(s: &jk_format::Settings) -> MixParams {
        MixParams {
            semitones: s.semitones_clamped(),
            shift_drums: s.shift_drums,
            guide_vocal: s.guide_vocal as f32,
            backing_on: s.backing_on,
            backing_level: s.backing_level as f32,
            drums: s.level("drums") as f32,
            other: s.level("other") as f32,
        }
    }

    /// The DSP graph's parameter struct.
    pub fn to_dsp(&self) -> jk_dsp::MixParams {
        jk_dsp::MixParams {
            guide: self.guide_vocal,
            backing_on: self.backing_on,
            backing: self.backing_level,
            other: self.other,
            drums: self.drums,
            shift_drums: self.shift_drums,
            semitones: self.semitones,
        }
        .clamped()
    }
}

/// One stem of the loaded song, as the engine plays it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StemInfo {
    /// `lead_vocal`, `backing_vocals`, `drums`, `other`, or a newer role.
    pub role: String,
    pub in_karaoke_mix: bool,
    pub pitched: bool,
}

/// What [`Engine::load`] found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadedSong {
    pub duration_s: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub stems: Vec<StemInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    /// No song loaded.
    Empty,
    Paused,
    Playing,
    /// Played to the end (`position_s == duration_s`). `play()` starts again from 0.
    Ended,
    /// Playback stopped because of an error (see `Snapshot::error`). `play()` retries.
    Error,
}

/// Everything the UI shows about playback, read lock-free from the real-time threads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: PlaybackState,
    /// Song time (seconds) of the sample leaving the DAC now: the device's reported
    /// output latency is already subtracted (WORK_ORDER trap 7). Not interpolated.
    pub position_s: f64,
    pub duration_s: f64,
    pub device: Option<DeviceStatus>,
    /// Device callbacks that found the engine's output ring empty since the song was
    /// loaded (a glitch the engine caused).
    pub underruns: u64,
    /// WASAPI shared mode: device events where the endpoint buffer had run dry (the
    /// device thread was late; a glitch the OS scheduling caused). 0 elsewhere.
    pub device_starved: u64,
    /// Output peak per channel (linear, master volume included) over the last ~0.3 s,
    /// measured in the render worker (about 150 ms ahead of the DAC).
    pub output_peak: [f32; 2],
    /// The output went above full scale since the song was loaded or the mix / master
    /// volume last changed (audio of the previous mix still in the graph, about 0.2 s,
    /// does not count). Nothing limits it (no hidden processing): the UI warns.
    pub clipped: bool,
    /// Plain-language error when `state` is `Error`.
    pub error: Option<String>,
    /// Monotonic nanoseconds (engine clock) of the device callback that produced
    /// `position_s`; lets a caller interpolate between snapshots. 0 = no callback yet.
    pub position_ns: u64,
    /// Key-change mode in effect and, in prerender mode, the key or mix being prepared.
    #[serde(default)]
    pub key: crate::keycache::KeyStatus,
}

impl Snapshot {
    /// The snapshot of an engine with no song.
    pub fn empty() -> Snapshot {
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
}

/// Notifications. Each event goes to one receiver (the channel is MPMC); the app is
/// expected to have one consumer. Events are dropped when nobody reads them for a
/// long time (the channel holds 256); [`Engine::snapshot`] is always the truth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EngineEvent {
    StateChanged(Snapshot),
    Ended,
    Error { code: String, message: String },
    /// The output device disappeared (unplugged, disabled, driver reset). Playback
    /// was paused; `play()` reopens the chosen device or the default.
    DeviceLost,
}

/// Export file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// 24-bit FLAC. Scaled down (the whole file, by one constant) only if the render
    /// would exceed full scale; the applied gain is reported.
    Flac24,
    /// 32-bit float WAV (RF64 above 4 GiB). Never scaled.
    Wav32f,
}

/// What [`render_offline`] did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderReport {
    /// Largest absolute sample of the render before any export gain.
    pub peak: f32,
    /// Gain applied to the whole file (dB, <= 0). 0.0 when nothing was changed.
    pub applied_gain_db: f64,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
}

/// An error with a stable code for the app and a plain-language message for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    /// One of: `no_device`, `device_not_found`, `device_failed`, `device_lost`,
    /// `exclusive_unavailable`, `song_not_found`, `song_unreadable`, `song_newer_version`,
    /// `song_damaged`, `song_truncated`, `not_loaded`, `write_failed`, `cancelled`,
    /// `invalid_argument`, `internal`.
    pub code: String,
    /// Plain language, shown to the user as is.
    pub message: String,
    /// Technical detail for the log.
    #[serde(default)]
    pub detail: String,
}

impl EngineError {
    pub fn new(code: &str, message: impl Into<String>, detail: impl Into<String>) -> EngineError {
        EngineError { code: code.into(), message: message.into(), detail: detail.into() }
    }

    pub(crate) fn internal(detail: impl std::fmt::Display) -> EngineError {
        EngineError::new("internal", "The audio engine hit an internal error.", detail.to_string())
    }

    pub(crate) fn from_format(e: &jk_format::FormatError) -> EngineError {
        use jk_format::ErrorKind as K;
        let code = match e.kind() {
            K::NotASongFile | K::Damaged | K::MissingChunk => "song_damaged",
            K::NewerVersion => "song_newer_version",
            K::Truncated => "song_truncated",
            K::Io => match e {
                jk_format::FormatError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "song_not_found",
                _ => "song_unreadable",
            },
            _ => "song_unreadable",
        };
        EngineError::new(code, e.user_message(), e.to_string())
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{} [{}]", self.message, self.code)
        } else {
            write!(f, "{} [{}: {}]", self.message, self.code, self.detail)
        }
    }
}

impl std::error::Error for EngineError {}

/// The real-time player. See the module documentation.
pub struct Engine {
    inner: crate::engine::EngineInner,
}

impl Engine {
    /// Start the engine. Opens the chosen device (or the default; a missing saved
    /// device falls back to the default). A computer without any output device still
    /// gets an engine: `snapshot().device` is `None` and `play()` reports `no_device`.
    pub fn new(cfg: EngineConfig) -> Result<Engine, EngineError> {
        Ok(Engine { inner: crate::engine::EngineInner::new(cfg)? })
    }

    /// Output devices that are active right now, default first.
    pub fn list_devices() -> Result<Vec<DeviceInfo>, EngineError> {
        crate::devices::list_devices()
    }

    /// Switch the output device: pause, switch, resume at the same song position.
    /// `None` = system default. If exclusive mode is refused the device is opened in
    /// shared mode and `exclusive_unavailable` is returned.
    pub fn set_device(&self, id: Option<&str>, exclusive: bool) -> Result<(), EngineError> {
        self.inner.set_device(id, exclusive)
    }

    /// Open a `.jksong` (stops any current song). Paused at 0. Stems are streamed from
    /// the file; nothing is decoded ahead beyond the ~4 s read-ahead buffers.
    pub fn load(&self, song: &Path, params: MixParams) -> Result<LoadedSong, EngineError> {
        self.inner.load(song, params)
    }

    /// Stop and close the current song (state `Empty`).
    pub fn unload(&self) {
        self.inner.unload()
    }

    pub fn play(&self) {
        self.inner.play()
    }

    /// Pause with a 10 ms fade-out.
    pub fn pause(&self) {
        self.inner.pause()
    }

    /// Seek to a song time (clamped to the song). In place: the device keeps running;
    /// sample-accurate; the new position fades in over 10 ms.
    pub fn seek(&self, seconds: f64) {
        self.inner.seek(seconds)
    }

    /// Change key and mix. Gains ramp over 20 ms; a key change crossfades (40 ms) and
    /// is heard about 280 ms later (DECISIONS D-34).
    pub fn set_params(&self, p: MixParams) {
        self.inner.set_params(p)
    }

    /// Master volume, linear 0..1, smoothed per device buffer.
    pub fn set_master(&self, v: f32) {
        self.inner.set_master(v)
    }

    /// How key changes are made (WORK_ORDER 4.5): `Realtime` (default) or `Prerender`
    /// (render ahead into the key cache, keep the previous key until enough is ready,
    /// then crossfade). Prerender needs [`set_key_cache`](Self::set_key_cache); without a
    /// usable cache the real-time path is used and `Snapshot::key.note` says why. The
    /// app's "auto" uses [`crate::choose_key_mode`] with [`crate::benchmark_realtime_factor`].
    pub fn set_key_mode(&self, mode: crate::keycache::KeyMode) {
        self.inner.set_key_mode(mode)
    }

    /// The key cache folder (`<root>/Settings/cache/keys`; `None` = no cache) and its
    /// size cap in bytes (least recently used entries are evicted; entries in use are
    /// kept even above the cap). Deletes partial files left by an earlier session.
    pub fn set_key_cache(&self, dir: Option<&Path>, cap_bytes: u64) -> Result<(), EngineError> {
        self.inner.set_key_cache(dir, cap_bytes)
    }

    /// Test hook: run the render worker's graph and the key renderer `factor` times
    /// slower (busy-waiting), as on a slow computer. 1.0 = normal.
    #[doc(hidden)]
    pub fn debug_set_slowdown(&self, factor: f64) {
        self.inner.debug_set_slowdown(factor)
    }

    /// A reader of the output tap: a lock-free copy of exactly what the output device plays
    /// (after the graph, the output resampler, fades and the master volume), interleaved
    /// stereo at the device rate, each piece stamped with the host time ([`crate::now_ns`])
    /// its first frame leaves the DAC. For practice mode's echo canceller, read on its own
    /// analysis thread. The ring (about 2 MiB) is allocated on the first call; while no
    /// reader is attached the device callback skips the tap (two atomic loads). Playback is
    /// bit-identical with or without a reader. See [`crate::tap`].
    pub fn output_tap(&self) -> crate::tap::TapReader {
        self.inner.output_tap()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.snapshot()
    }

    /// Which key (semitones) was heard at song time `pos_s`, for the recent past (the
    /// last 64 playback segments of the loaded song), so practice mode can grade each
    /// note in the key that was sounding when it was sung (Work Order 2, 4.6). With
    /// `segment` (`Snapshot::key.segment` read when the position was stamped) the answer
    /// is for that segment, from its start on, including changes already scheduled but
    /// not heard yet. Without it, the newest segment that played `pos_s` answers. `None`:
    /// no song, or the position was not played (in that segment).
    pub fn key_at(&self, pos_s: f64, segment: Option<u64>) -> Option<i32> {
        self.inner.key_at(pos_s, segment)
    }

    /// The recent history of the key heard, per playback segment (diagnostics, tests,
    /// evidence; practice mode uses [`key_at`](Self::key_at) and `Snapshot::key`).
    pub fn key_timeline(&self) -> Vec<crate::keyheard::KeySegment> {
        self.inner.key_timeline()
    }

    pub fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        self.inner.events()
    }

    /// Diagnostics for tests and benchmarks (not part of the app contract).
    #[doc(hidden)]
    pub fn diagnostics(&self) -> crate::engine::Diagnostics {
        self.inner.diagnostics()
    }

    /// Test hook: an engine on the null device at `rate` whose every played buffer is
    /// appended to the returned vector (interleaved stereo). `fast`: consume as fast as
    /// the worker produces instead of in real time.
    #[doc(hidden)]
    pub fn new_with_capture(rate: u32, fast: bool, master: f32) -> Result<(Engine, crate::output::Capture), EngineError> {
        let (inner, cap) = crate::engine::EngineInner::new_with_capture(rate, fast, master)?;
        Ok((Engine { inner }, cap))
    }

    /// Test hook: make the open device fail as if it had been unplugged.
    #[doc(hidden)]
    pub fn debug_simulate_device_loss(&self) {
        self.inner.debug_simulate_device_loss()
    }
}

/// "Export backing track": render the song with `p` offline (same graph as playback,
/// single-threaded, at the song's sample rate, streamed from the file) to `out`.
///
/// `progress` is called with 0..1. Setting `cancel` stops the render; the partial file
/// is removed and `cancelled` is returned. The output is written to a temporary file
/// next to `out` and renamed when complete.
pub fn render_offline(
    song: &Path,
    p: &MixParams,
    out: &Path,
    fmt: ExportFormat,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<RenderReport, EngineError> {
    crate::export::render_offline(song, p, out, fmt, progress, cancel)
}

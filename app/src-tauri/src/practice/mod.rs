// SPDX-License-Identifier: GPL-3.0-or-later
//! Practice mode in the app core (Work Order 2; docs/APP_API.md section 12, docs/
//! ARCHITECTURE.md section 12).
//!
//! Practice is a mode of the singing screen, not a second player: the song plays through
//! the normal player and engine (transport, key dial and mixer keep working) and this
//! module only listens. Pieces:
//!
//! - [`mic`]: the input list, the permission (Windows' privacy switches read only), meters,
//!   the input picker's meter stream, the sources (microphone, or generated test sound
//!   through the same path).
//! - [`guide_view`]: the session's guide, from which the lane's `practice_guide` and the
//!   scorer's guide are both made.
//! - [`align`]: the engine's output tap lined up with the microphone (the echo reference).
//! - [`worker`]: the analysis thread (input -> `jk_pitch::practice::Chain` -> events, the
//!   take, the result into the song's slot).
//! - [`result`]: results as stored (SONG_FORMAT 11) and as the result screen gets them.
//! - [`rigcheck`]: the practice rig check (audible, started by hand only, APP_API 10.2).
//! - This file: the session's life (APP_API 12.2), the commands, the per-song settings in
//!   the slot and the per-computer ones in settings.json.
//!
//! Rules kept everywhere: the microphone is open only between `practice_start` and
//! `practice_stop` (and while the input picker meters it), never while the window is
//! hidden, never monitored; its sound is analysed in memory and dropped (never written,
//! sent or played); nothing here feeds the engine's output, so playback is bit-identical
//! with practice on or off; nothing is kept in web storage.

pub mod align;
pub mod checks;
pub mod guide_view;
pub mod mic;
pub mod result;
pub mod rigcheck;
pub mod synth;
pub mod testsource;
pub mod worker;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use jk_pitch::score::{OctaveMode, Strictness};
use jk_recorder::input::{self as inp, InputDeviceInfo, MicPermission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit};
use crate::guide::{GuideService, GuideStatus};
use crate::library::Library;
use crate::playback::Player;
use crate::settings::{InputChoice, OctaveModeSetting, SettingsStore, StrictnessSetting};
use guide_view::{PracticeGuideView, SessionGuide};
use mic::{InputErrorView, Meter, MicInput, MicPermissionView, SourceSpec};
use worker::{Cmd, TakeOutcome, Worker, WorkerInit};

pub use worker::names;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------
// The contract's types (docs/APP_API.md 12.4-12.6)

/// `MicCapability` (`AppInfo.capabilities.microphone`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MicCapability {
    pub available: bool,
    pub reason: Option<String>,
    pub permission: MicPermissionView,
    pub backend: &'static str,
}

/// `AppInfo.capabilities.practice`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PracticeCapability {
    pub guide_method: String,
    pub guide_version: String,
}

/// `mic_list()`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MicList {
    pub inputs: Vec<MicInput>,
    pub chosen: InputChoice,
    pub automatic: Option<String>,
}

/// `mic_meter()`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeterResult {
    pub device: Option<MicInput>,
    pub error: Option<InputErrorView>,
}

/// `PracticeInput`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PracticeInputView {
    pub device: MicInput,
    pub channel: usize,
    pub sample_rate: u32,
    pub state: &'static str,
    pub message: Option<String>,
    pub meter: Option<mic::ChannelMeter>,
    pub dropped_blocks: u64,
}

/// `MusicInInput`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MusicView {
    pub presence: &'static str,
    pub level_db: Option<f64>,
    pub delay_ms: Option<f64>,
    pub stability: &'static str,
    pub drift_ppm: Option<f64>,
    pub removal: &'static str,
    pub suppression_db: Option<f64>,
    pub grading: &'static str,
    pub message: Option<String>,
}

impl MusicView {
    /// Nothing listening (watch only, preparing): nothing comes back.
    fn idle() -> MusicView {
        MusicView {
            presence: "absent",
            level_db: None,
            delay_ms: None,
            stability: "unknown",
            drift_ppm: None,
            removal: "not_needed",
            suppression_db: None,
            grading: "normal",
            message: None,
        }
    }
}

/// `PracticeState.latency`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LatencyView {
    pub applied_ms: f64,
    pub source: &'static str,
    pub measured_ms: Option<f64>,
    pub manual_ms: f64,
}

/// `PracticeState.take`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TakeView {
    pub running: bool,
    pub started_s: f64,
    pub covered: f64,
    pub graded: u32,
    pub total: u32,
    pub overall: Option<f64>,
    pub seeks: u32,
    pub key_changes: u32,
}

/// `PracticeState.not_ready`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NotReady {
    pub code: String,
    pub message: String,
    pub can_retry: bool,
}

/// `PracticeState.watch_only`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WatchOnly {
    /// chosen | no_device | permission_denied | not_determined | restricted | input_lost
    pub code: &'static str,
    pub message: String,
}

/// `PracticeState.octave`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OctaveView {
    pub mode: &'static str,
    pub shift: i32,
    pub settled: bool,
}

/// An excluded span ("the guide is wrong here"), SONG_FORMAT 11 `excluded`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExcludedSpan {
    #[serde(default)]
    pub line: Option<i64>,
    pub start: f64,
    pub end: f64,
}

/// docs/APP_API.md 12.5 `PracticeState`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PracticeStateView {
    pub active: bool,
    pub song_id: Option<String>,
    pub seq: u64,
    pub phase: &'static str,
    pub guide: GuideStatus,
    pub not_ready: Option<NotReady>,
    pub watch_only: Option<WatchOnly>,
    pub input: Option<PracticeInputView>,
    pub music: MusicView,
    pub octave: OctaveView,
    pub strictness: &'static str,
    pub latency: LatencyView,
    pub take: Option<TakeView>,
    pub excluded: Vec<ExcludedSpan>,
    pub last_result: Option<Value>,
}

/// `practice_target_at` (internal, screen checks).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TargetView {
    pub note: Option<usize>,
    pub midi: Option<f64>,
    pub lane: Option<f64>,
    pub name: Option<String>,
    pub key: Option<i32>,
    pub octave: i32,
    pub graded: bool,
}

/// `practice_history`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryView {
    pub best: Option<Value>,
    pub recent: Vec<Value>,
}

/// What the analysis thread reports every 250 ms (and on changes).
pub struct WorkerStatus {
    pub input: Option<PracticeInputView>,
    /// The input stopped: plain words (watch only with `input_lost`).
    pub lost: Option<String>,
    pub music: MusicView,
    pub octave_shift: i32,
    pub octave_settled: bool,
    pub latency: LatencyView,
    pub take: Option<TakeView>,
    pub take_running: bool,
}

/// What `practice_test_input` set for the next `practice_start` (APP_API 12.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestInput {
    pub source: Option<SourceSpec>,
    /// Simulated permission (the "permission refused" case of WO2 section 9).
    pub permission: Option<MicPermission>,
    /// Simulated "no input device at all".
    pub no_device: bool,
}

// ---------------------------------------------------------------------------------------
// The service

struct WorkerHandle {
    tx: Sender<Cmd>,
    join: Option<JoinHandle<()>>,
}

struct Session {
    song_id: String,
    path: PathBuf,
    phase: &'static str,
    guide: Option<Arc<SessionGuide>>,
    guide_status: GuideStatus,
    not_ready: Option<NotReady>,
    watch_only: Option<WatchOnly>,
    input: Option<PracticeInputView>,
    music: MusicView,
    octave_mode: OctaveMode,
    octave_shift: i32,
    octave_settled: bool,
    strictness: Strictness,
    latency: LatencyView,
    take: Option<TakeView>,
    excluded: Vec<ExcludedSpan>,
    last_result: Option<Value>,
    worker: Option<WorkerHandle>,
    /// Cancels an on-the-spot guide build wait.
    cancel: Arc<AtomicBool>,
    want_watch_only: bool,
    /// The last `practice://state` sent (without `seq`), to skip repeats.
    last_sent: Option<Value>,
}

/// Practice mode's state in the core.
pub struct PracticeService {
    library: Arc<Library>,
    player: Arc<Player>,
    guide: Arc<GuideService>,
    settings: Arc<SettingsStore>,
    sink: Arc<dyn EventSink>,
    settings_dir: PathBuf,
    tmp_dir: PathBuf,
    app_name: String,
    app_version: String,
    test_allowed: AtomicBool,
    test: Mutex<TestInput>,
    session: Mutex<Option<Session>>,
    meter: Mutex<Option<Meter>>,
    seq: AtomicU64,
    polling_permission: AtomicBool,
    /// The rig check runs (practice input suspended, nothing else may start it twice).
    pub(crate) rig_running: AtomicBool,
    /// The lane's drawing cost the page reported (`selfcheck_lane`, APP_API 10.1).
    pub lane_cost: Mutex<Option<checks::LaneCost>>,
    /// The `mic://devices` watcher runs (started with the first session or meter).
    watching_devices: AtomicBool,
    me: OnceLock<Weak<PracticeService>>,
}

fn octave_mode_of(s: &str) -> OctaveMode {
    OctaveMode::parse(s).unwrap_or(OctaveMode::Auto)
}

fn strictness_setting(s: StrictnessSetting) -> Strictness {
    match s {
        StrictnessSetting::Relaxed => Strictness::Relaxed,
        StrictnessSetting::Standard => Strictness::Standard,
        StrictnessSetting::Strict => Strictness::Strict,
    }
}

fn octave_setting(s: OctaveModeSetting) -> OctaveMode {
    match s {
        OctaveModeSetting::Auto => OctaveMode::Auto,
        OctaveModeSetting::Fixed => OctaveMode::Fixed,
        OctaveModeSetting::Strict => OctaveMode::Strict,
    }
}

impl PracticeService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        library: Arc<Library>,
        player: Arc<Player>,
        guide: Arc<GuideService>,
        settings: Arc<SettingsStore>,
        sink: Arc<dyn EventSink>,
        settings_dir: &Path,
        tmp_dir: &Path,
        app_name: &str,
        app_version: &str,
    ) -> Arc<PracticeService> {
        let svc = Arc::new(PracticeService {
            library,
            player,
            guide,
            settings,
            sink,
            settings_dir: settings_dir.to_path_buf(),
            tmp_dir: tmp_dir.to_path_buf(),
            app_name: app_name.to_string(),
            app_version: app_version.to_string(),
            test_allowed: AtomicBool::new(false),
            test: Mutex::new(TestInput::default()),
            session: Mutex::new(None),
            meter: Mutex::new(None),
            seq: AtomicU64::new(0),
            polling_permission: AtomicBool::new(false),
            rig_running: AtomicBool::new(false),
            lane_cost: Mutex::new(None),
            watching_devices: AtomicBool::new(false),
            me: OnceLock::new(),
        });
        let _ = svc.me.set(Arc::downgrade(&svc));
        svc
    }

    fn me(&self) -> Option<Arc<PracticeService>> {
        self.me.get().and_then(Weak::upgrade)
    }

    /// `practice_test_input` is accepted (the app was started with `--smoke-test` or the
    /// internal `--practice-test`; tests set it directly).
    pub fn allow_test_input(&self, on: bool) {
        self.test_allowed.store(on, Ordering::SeqCst);
    }

    pub(crate) fn settings_dir(&self) -> &Path {
        &self.settings_dir
    }

    pub(crate) fn tmp_dir(&self) -> &Path {
        &self.tmp_dir
    }

    pub(crate) fn player(&self) -> &Arc<Player> {
        &self.player
    }

    pub(crate) fn settings_store(&self) -> &Arc<SettingsStore> {
        &self.settings
    }

    pub(crate) fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }

    pub(crate) fn test_source(&self) -> Option<SourceSpec> {
        lock(&self.test).source.clone()
    }

    // ------------------------------------------------------------------ microphone (12.4)

    /// The permission, simulated when a test asked for it.
    pub fn effective_permission(&self) -> MicPermission {
        lock(&self.test).permission.unwrap_or_else(inp::mic_permission)
    }

    /// `mic_permission()` (never asks).
    pub fn mic_permission(&self) -> MicPermissionView {
        mic::permission_view(self.effective_permission(), inp::mic_privacy_switches())
    }

    /// `mic_request_permission()`: macOS asks (only after the UI showed the explanation)
    /// and the answer comes as `mic://permission` (polled every 0.5 s for 2 minutes).
    /// Windows asks nothing and returns the switches.
    pub fn mic_request_permission(&self) -> MicPermissionView {
        if lock(&self.test).permission.is_some() {
            return self.mic_permission();
        }
        let p = inp::request_mic_permission();
        if p == MicPermission::NotDetermined && !self.polling_permission.swap(true, Ordering::SeqCst) {
            let me = self.me();
            let _ = std::thread::Builder::new().name("mic-permission".into()).spawn(move || {
                let Some(me) = me else { return };
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_secs(120) {
                    std::thread::sleep(Duration::from_millis(500));
                    let now = inp::mic_permission();
                    if now != MicPermission::NotDetermined {
                        emit(&*me.sink, mic::names::MIC_PERMISSION, &mic::permission_view(now, inp::mic_privacy_switches()));
                        break;
                    }
                }
                me.polling_permission.store(false, Ordering::SeqCst);
            });
        }
        self.mic_permission()
    }

    /// `mic_open_settings()`: opens the system's microphone privacy page (never changes it).
    pub fn mic_open_settings(&self) -> ApiResult<()> {
        let url = self.mic_permission().settings_url.unwrap_or_default();
        let r = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else {
            std::process::Command::new("explorer").arg(&url).spawn()
        };
        r.map(|_| ()).map_err(|e| ApiError::new("internal", format!("The settings page could not be opened ({e}). Open it by hand: {url}")))
    }

    fn output_name(&self) -> Option<String> {
        self.player.engine().snapshot().device.map(|d| d.name)
    }

    fn list_devices(&self) -> Vec<InputDeviceInfo> {
        if lock(&self.test).no_device {
            return Vec::new();
        }
        inp::list_input_devices().unwrap_or_else(|e| {
            crate::applog::warn(&format!("practice: listing inputs failed: {e}"));
            Vec::new()
        })
    }

    /// This computer's saved input choice.
    pub fn chosen_input(&self) -> InputChoice {
        let me = crate::keymode::machine_id();
        self.settings.get().practice.inputs.get(&me).cloned().unwrap_or_default()
    }

    /// `AppInfo.capabilities.microphone`.
    pub fn capability(&self) -> MicCapability {
        let list = self.list_devices();
        let test = lock(&self.test).source.is_some();
        let available = !list.is_empty() || test;
        MicCapability {
            available,
            reason: (!available).then(|| {
                "No microphone or audio input is connected. Plug in the mixer (or a microphone); practice still shows the \
                 notes without one."
                    .to_string()
            }),
            permission: self.mic_permission(),
            backend: if cfg!(target_os = "macos") { "Core Audio" } else { "WASAPI" },
        }
    }

    /// `mic_list()`: every input (never opened), the saved choice and the automatic one.
    pub fn mic_list(&self) -> MicList {
        let list = self.list_devices();
        let out = self.output_name();
        let auto = inp::preferred_input(&list).map(|d| d.id.clone());
        MicList {
            inputs: list.iter().map(|d| mic::mic_input(d, auto.as_deref(), out.as_deref())).collect(),
            chosen: self.chosen_input(),
            automatic: auto,
        }
    }

    /// `mic_choose(choice)`: saved for this computer; a listening session reopens at once.
    pub fn mic_choose(&self, choice: InputChoice) -> ApiResult<InputChoice> {
        if choice.channel.is_some_and(|c| c > 255) {
            return Err(ApiError::bad_request("The channel number is out of range."));
        }
        let mut choice = choice;
        // Remember the name too, so a mixer moved to another USB port is found again.
        if let Some(id) = &choice.device_id
            && choice.device_name.is_none()
        {
            choice.device_name = self.list_devices().iter().find(|d| &d.id == id).map(|d| d.name.clone());
        }
        let me = crate::keymode::machine_id();
        let c2 = choice.clone();
        self.settings.update(|s| {
            if c2.device_id.is_none() && c2.channel.is_none() {
                s.practice.inputs.remove(&me);
            } else {
                s.practice.inputs.insert(me.clone(), c2.clone());
            }
        });
        // A listening session (on a real input) moves to the new choice now.
        let tx = {
            let g = lock(&self.session);
            g.as_ref()
                .filter(|s| s.watch_only.is_none() && s.input.as_ref().is_some_and(|i| !i.device.id.starts_with("test:")))
                .and_then(|s| s.worker.as_ref().map(|w| w.tx.clone()))
        };
        if let Some(tx) = tx {
            let list = self.list_devices();
            if let Some((d, _)) = inp::resolve_input(&list, choice.device_id.as_deref(), choice.device_name.as_deref()) {
                let _ = tx.send(Cmd::Reopen { spec: SourceSpec::Device { id: d.id.clone() }, channel: choice.channel.map(|c| c as usize) });
            }
        }
        Ok(choice)
    }

    /// `mic_meter(device_id | null)`: the input picker's meters for one device.
    pub fn mic_meter(&self, device_id: Option<String>) -> ApiResult<MeterResult> {
        if let Some(m) = lock(&self.meter).take() {
            m.close();
        }
        let Some(id) = device_id.filter(|s| !s.is_empty()) else {
            return Ok(MeterResult { device: None, error: None });
        };
        // The session's own input is metered by the session (mic://levels).
        let session_dev = lock(&self.session).as_ref().and_then(|s| s.input.as_ref().map(|i| i.device.clone()));
        if let Some(d) = session_dev.filter(|d| d.id == id) {
            return Ok(MeterResult { device: Some(d), error: None });
        }
        let perm = self.effective_permission();
        if matches!(perm, MicPermission::Denied | MicPermission::Restricted) {
            return Ok(MeterResult {
                device: None,
                error: Some(InputErrorView { kind: "permission_denied", message: inp::mic_denied_steps().to_string() }),
            });
        }
        let list = self.list_devices();
        let out = self.output_name();
        let spec = if id.starts_with("test:") {
            match self.test_source() {
                Some(s) => s,
                None => return Err(ApiError::new("not_found", "That test input is not set.")),
            }
        } else {
            if !list.iter().any(|d| d.id == id) {
                return Ok(MeterResult {
                    device: None,
                    error: Some(InputErrorView { kind: "no_device", message: "That input is no longer connected.".into() }),
                });
            }
            SourceSpec::Device { id: id.clone() }
        };
        match mic::open_source(&spec, &list, out.as_deref(), self.player.engine()) {
            Ok((stream, view)) => {
                *lock(&self.meter) = Some(Meter::start(view.id.clone(), stream, Arc::clone(&self.sink)));
                self.watch_devices();
                Ok(MeterResult { device: Some(view), error: None })
            }
            Err(e) => Ok(MeterResult { device: None, error: Some(InputErrorView::from(&e)) }),
        }
    }

    /// `mic://devices` (APP_API 12.4): while practice or the picker's meter is open, the
    /// input list is looked at every 3 s (listing never opens a device) and the event is
    /// sent when inputs appear or disappear. One thread, started on first use.
    fn watch_devices(&self) {
        if self.watching_devices.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.get().cloned().unwrap_or_default();
        let _ = std::thread::Builder::new().name("mic-devices".into()).spawn(move || {
            let mut last: Option<Vec<String>> = None;
            loop {
                std::thread::sleep(Duration::from_secs(3));
                let Some(svc) = me.upgrade() else { return };
                let busy = lock(&svc.session).is_some() || lock(&svc.meter).is_some();
                if !busy {
                    last = None;
                    continue;
                }
                let mut ids: Vec<String> = svc.list_devices().into_iter().map(|d| d.id).collect();
                ids.sort();
                if last.as_ref().is_some_and(|l| *l != ids) {
                    svc.sink.emit_value(mic::names::MIC_DEVICES, Value::Null);
                }
                last = Some(ids);
            }
        });
    }

    fn close_meter(&self) {
        if let Some(m) = lock(&self.meter).take() {
            m.close();
        }
    }

    // ------------------------------------------------------------------ test input (12.1)

    /// `practice_test_input(source)`: internal, only in the test modes.
    pub fn test_input(&self, source: &Value) -> ApiResult<()> {
        if !self.test_allowed.load(Ordering::SeqCst) {
            return Err(ApiError::bad_request(
                "practice_test_input is only accepted when the app runs a test (--smoke-test or --practice-test).",
            ));
        }
        let mut t = lock(&self.test);
        if source.is_null() {
            *t = TestInput::default();
            return Ok(());
        }
        let kind = source.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "wav" => {
                let p = source.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).ok_or_else(|| {
                    ApiError::bad_request("A test input of kind \"wav\" needs a path to a WAV of generated sound.")
                })?;
                let p = PathBuf::from(p);
                if !p.is_file() {
                    return Err(ApiError::new("not_found", format!("{} does not exist.", p.display())));
                }
                // `sync` (default true): the singer sings along to the song (the WAV's sample
                // k is song time k / rate); false: the WAV plays from the moment it is opened.
                let sync = source.get("sync").and_then(Value::as_bool).unwrap_or(true);
                let late = source.get("late_ms").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0);
                let unplug = source.get("unplug_after_s").and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
                t.source = Some(if sync {
                    SourceSpec::TestSyncWav { path: p, late_ms: late, unplug_after_s: unplug }
                } else {
                    SourceSpec::TestWav(p)
                });
            }
            "silence" => t.source = Some(SourceSpec::TestSilence),
            "return" => {
                let num = |k: &str, d: f64| source.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(d);
                let voice = source.get("voice").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
                if let Some(v) = &voice
                    && !v.is_file()
                {
                    return Err(ApiError::new("not_found", format!("{} does not exist.", v.display())));
                }
                t.source = Some(SourceSpec::TestReturn { delay_ms: num("delay_ms", 20.0), level_db: num("level_db", -6.0), voice });
            }
            "permission_denied" => t.permission = Some(MicPermission::Denied),
            "not_determined" => t.permission = Some(MicPermission::NotDetermined),
            "permission_granted" => t.permission = None,
            "no_device" => {
                t.no_device = true;
                t.source = None;
            }
            _ => return Err(ApiError::bad_request(format!("Unknown test input kind '{kind}'."))),
        }
        Ok(())
    }

    // ------------------------------------------------------------------ the session (12.2, 12.5)

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn view_of(&self, s: Option<&Session>) -> PracticeStateView {
        let seq = self.seq.load(Ordering::SeqCst);
        match s {
            None => {
                let p = self.settings.get().practice;
                PracticeStateView {
                    active: false,
                    song_id: None,
                    seq,
                    phase: "off",
                    guide: GuideStatus::none(),
                    not_ready: None,
                    watch_only: None,
                    input: None,
                    music: MusicView::idle(),
                    octave: OctaveView { mode: octave_setting(p.octave_mode).name(), shift: 0, settled: false },
                    strictness: strictness_setting(p.strictness).name(),
                    latency: LatencyView { applied_ms: 0.0, source: "none", measured_ms: None, manual_ms: 0.0 },
                    take: None,
                    excluded: Vec::new(),
                    last_result: None,
                }
            }
            Some(s) => PracticeStateView {
                active: true,
                song_id: Some(s.song_id.clone()),
                seq,
                phase: s.phase,
                guide: s.guide_status.clone(),
                not_ready: s.not_ready.clone(),
                watch_only: s.watch_only.clone(),
                input: s.input.clone(),
                music: s.music.clone(),
                octave: OctaveView {
                    mode: s.octave_mode.name(),
                    shift: if s.octave_mode == OctaveMode::Strict { 0 } else { s.octave_shift },
                    settled: s.octave_settled,
                },
                strictness: s.strictness.name(),
                latency: s.latency.clone(),
                take: s.take.clone(),
                excluded: s.excluded.clone(),
                last_result: s.last_result.clone(),
            },
        }
    }

    /// Sends `practice://state` when something changed (`force`: always).
    fn emit_state_locked(&self, g: &mut Option<Session>, force: bool) {
        let mut view = self.view_of(g.as_ref());
        let mut cmp = serde_json::to_value(&view).unwrap_or(Value::Null);
        if let Some(o) = cmp.as_object_mut() {
            o.remove("seq");
        }
        let changed = match g.as_mut() {
            Some(s) => {
                let c = s.last_sent.as_ref() != Some(&cmp);
                s.last_sent = Some(cmp);
                c
            }
            None => true,
        };
        if changed || force {
            view.seq = self.next_seq();
            emit(&*self.sink, names::PRACTICE_STATE, &view);
        }
    }

    /// `practice_state()`.
    pub fn state(&self) -> PracticeStateView {
        self.view_of(lock(&self.session).as_ref())
    }

    /// True while a take runs (the self-check and the rig check refuse then).
    pub fn take_running(&self) -> bool {
        lock(&self.session).as_ref().and_then(|s| s.take.as_ref()).is_some_and(|t| t.running)
    }

    /// `practice_start(options)`.
    pub fn start(&self, song_id: Option<String>, watch_only: bool) -> ApiResult<PracticeStateView> {
        let loaded = self.player.current();
        let song_id = match song_id.filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => loaded
                .as_ref()
                .map(|c| c.song_id.clone())
                .ok_or_else(|| ApiError::new("not_loaded", "No song is loaded. Choose a song first."))?,
        };
        let path = self.library.resolve(&song_id)?;
        {
            let g = lock(&self.session);
            if let Some(s) = g.as_ref()
                && s.song_id == song_id
                && s.want_watch_only == watch_only
            {
                return Ok(self.view_of(Some(s)));
            }
        }
        // Practice on another song (or with another input choice): end the first.
        if lock(&self.session).is_some() {
            self.stop(true)?;
        }
        if loaded.as_ref().is_none_or(|c| c.song_id != song_id) {
            self.player.load(&song_id)?;
        }
        self.close_meter();
        // The song's own choices (slot), else this computer's defaults.
        let app = self.settings.get().practice;
        let slot = jk_format::SongFile::open(&path).ok().and_then(|f| f.user_state().practice());
        let (octave_mode, octave_shift) = match &slot {
            Some(p) => (octave_mode_of(&p.octave.mode), p.octave.shift.clamp(-2, 2)),
            None => (octave_setting(app.octave_mode), 0),
        };
        let strictness = slot
            .as_ref()
            .and_then(|p| p.strictness.as_deref().and_then(Strictness::parse))
            .unwrap_or_else(|| strictness_setting(app.strictness));
        let excluded: Vec<ExcludedSpan> = slot
            .as_ref()
            .map(|p| p.excluded.iter().map(|e| ExcludedSpan { line: e.line, start: e.start, end: e.end }).collect())
            .unwrap_or_default();
        let session = Session {
            song_id: song_id.clone(),
            path: path.clone(),
            phase: "preparing",
            guide: None,
            guide_status: self.guide.status(&song_id).unwrap_or_else(|_| GuideStatus::none()),
            not_ready: None,
            watch_only: None,
            input: None,
            music: MusicView::idle(),
            octave_mode,
            octave_shift,
            octave_settled: octave_mode != OctaveMode::Auto,
            strictness,
            latency: LatencyView { applied_ms: 0.0, source: "none", measured_ms: None, manual_ms: 0.0 },
            take: None,
            excluded,
            last_result: None,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            want_watch_only: watch_only,
            last_sent: None,
        };
        crate::applog::info(&format!("practice: start on {song_id}{}", if watch_only { " (watch only)" } else { "" }));
        self.watch_devices();
        let mut g = lock(&self.session);
        *g = Some(session);
        match self.acquire_guide(&song_id, &path) {
            Acquire::Ready(sg) => {
                let s = g.as_mut().expect("just set");
                s.guide_status = sg.status.clone();
                s.guide = Some(Arc::new(sg));
                self.begin_listening(s);
            }
            Acquire::NotReady(nr) => {
                let s = g.as_mut().expect("just set");
                s.phase = "not_ready";
                s.not_ready = Some(nr);
            }
            how @ (Acquire::Build | Acquire::InMemory) => {
                let s = g.as_mut().expect("just set");
                s.phase = "preparing";
                let cancel = Arc::clone(&s.cancel);
                let me = self.me();
                let in_memory = matches!(how, Acquire::InMemory);
                let (id, p) = (song_id.clone(), path.clone());
                let _ = std::thread::Builder::new().name("practice-prepare".into()).spawn(move || {
                    if let Some(me) = me {
                        me.prepare(&id, &p, &cancel, in_memory);
                    }
                });
            }
        }
        self.emit_state_locked(&mut g, true);
        Ok(self.view_of(g.as_ref()))
    }

    /// The guide of `song_id` as practice can use it now, or what has to happen first.
    fn acquire_guide(&self, song_id: &str, path: &Path) -> Acquire {
        let status = self.guide.status(song_id).unwrap_or_else(|_| GuideStatus::none());
        let song = match jk_format::SongFile::open(path) {
            Ok(s) => s,
            Err(e) => {
                return Acquire::NotReady(NotReady { code: "no_guide".into(), message: e.user_message(), can_retry: false });
            }
        };
        // A guide built but not (yet) in the file (the swap waits, or it could not be written).
        if let Some(b) = self.guide.memory_guide(song_id) {
            return Acquire::Ready(SessionGuide::new(song_id, &song, b.doc.clone(), Some(b.curve.clone()), status));
        }
        match status.state {
            "ready" | "outdated" => match jk_format::guide::read_guide(&song) {
                Ok(Some((doc, curve))) => Acquire::Ready(SessionGuide::new(song_id, &song, doc, curve, status)),
                // Listed as usable but unreadable now: build it again.
                Ok(None) | Err(_) => Acquire::Build,
            },
            "newer" => Acquire::InMemory,
            "failed" if status.error_code.as_deref() == Some("guide_no_lead") => Acquire::NotReady(NotReady {
                code: "guide_no_lead".into(),
                message: status.reason.clone().unwrap_or_else(|| {
                    "This song has no separate lead vocal, so there is no melody to practise.".into()
                }),
                can_retry: false,
            }),
            _ => Acquire::Build,
        }
    }

    /// "preparing": the guide is built on the spot (any computer, no Studio) and practice
    /// goes on when it is ready, or says why not (`not_ready`, with Try again).
    fn prepare(&self, song_id: &str, path: &Path, cancel: &AtomicBool, in_memory: bool) {
        let finish = |outcome: Result<SessionGuide, NotReady>| {
            let mut g = lock(&self.session);
            let Some(s) = g.as_mut().filter(|s| s.song_id == song_id && !cancel.load(Ordering::SeqCst)) else { return };
            match outcome {
                Ok(sg) => {
                    s.guide_status = sg.status.clone();
                    s.guide = Some(Arc::new(sg));
                    self.begin_listening(s);
                }
                Err(nr) => {
                    s.phase = "not_ready";
                    s.not_ready = Some(nr);
                    if let Ok(st) = self.guide.status(song_id) {
                        s.guide_status = st;
                    }
                }
            }
            self.emit_state_locked(&mut g, false);
        };
        if in_memory {
            // A guide made by a newer Jukeoke is kept; this session uses its own (APP_API 12.3).
            let app = jk_guide::app_stamp(&self.app_name, &self.app_version);
            let cfg = jk_pitch::guide::GuideConfig::default();
            let mut progress = |p: f64| {
                let mut g = lock(&self.session);
                if let Some(s) = g.as_mut().filter(|s| s.song_id == song_id) {
                    s.guide_status.progress = Some((p * 100.0).round() / 100.0);
                }
                self.emit_state_locked(&mut g, false);
            };
            let mut ctl = jk_guide::BuildControl { cancel: Some(cancel), progress: Some(&mut progress) };
            let r = jk_guide::build_for_song(path, &cfg, &app, &mut ctl);
            let out = match (r, jk_format::SongFile::open(path)) {
                (Ok(b), Ok(song)) => {
                    let mut st = GuideStatus::from_file(&song);
                    st.in_memory = true;
                    st.progress = None;
                    Ok(SessionGuide::new(song_id, &song, b.doc, Some(b.curve), st))
                }
                (Err(e), _) => Err(NotReady { code: e.code.into(), message: e.message, can_retry: true }),
                (_, Err(e)) => Err(NotReady { code: "guide_read_failed".into(), message: e.user_message(), can_retry: true }),
            };
            finish(out);
            return;
        }
        if let Err(e) = self.guide.build(song_id, false) {
            finish(Err(NotReady { code: e.code.clone(), message: e.message, can_retry: true }));
            return;
        }
        let t0 = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(150));
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            let st = match self.guide.status(song_id) {
                Ok(s) => s,
                Err(e) => {
                    finish(Err(NotReady { code: e.code.clone(), message: e.message, can_retry: false }));
                    return;
                }
            };
            match st.state {
                "queued" | "building" => {
                    let mut g = lock(&self.session);
                    if let Some(s) = g.as_mut().filter(|s| s.song_id == song_id) {
                        s.guide_status = st;
                    }
                    self.emit_state_locked(&mut g, false);
                }
                "ready" | "outdated" => {
                    match self.acquire_guide(song_id, Path::new(path)) {
                        Acquire::Ready(sg) => finish(Ok(sg)),
                        Acquire::NotReady(nr) => finish(Err(nr)),
                        _ => finish(Err(NotReady {
                            code: "guide_read_failed".into(),
                            message: "The practice data was made but could not be read. Press Try again.".into(),
                            can_retry: true,
                        })),
                    }
                    return;
                }
                "failed" => {
                    finish(Err(NotReady {
                        code: st.error_code.clone().unwrap_or_else(|| "no_guide".into()),
                        message: st.reason.clone().unwrap_or_else(|| "The practice data could not be made.".into()),
                        can_retry: st.error_code.as_deref() != Some("guide_no_lead"),
                    }));
                    return;
                }
                _ if t0.elapsed() > Duration::from_secs(600) => {
                    finish(Err(NotReady {
                        code: "no_guide".into(),
                        message: "Making the practice data took too long. Press Try again.".into(),
                        can_retry: true,
                    }));
                    return;
                }
                _ => {}
            }
        }
    }

    /// The guide is there: open the input (unless watch only) and start the analysis
    /// thread, or say why it is watch only.
    fn begin_listening(&self, s: &mut Session) {
        s.not_ready = None;
        s.guide_status.progress = None;
        if s.want_watch_only {
            s.phase = "watch_only";
            s.watch_only = Some(WatchOnly { code: "chosen", message: "Watching only: the notes and the piano keys show the melody; nothing is heard or graded.".into() });
            return;
        }
        let (spec, channel) = match self.choose_input() {
            Ok(c) => c,
            Err(w) => {
                crate::applog::info(&format!("practice: watch only ({}): {}", w.code, w.message));
                s.phase = "watch_only";
                s.watch_only = Some(w);
                return;
            }
        };
        let list = if spec.is_test() { Vec::new() } else { self.list_devices() };
        let out = self.output_name();
        let (stream, device) = match mic::open_source(&spec, &list, out.as_deref(), self.player.engine()) {
            Ok(x) => x,
            Err(e) => {
                let code = match e.kind {
                    inp::FailureKind::PermissionDenied => "permission_denied",
                    _ => "no_device",
                };
                s.phase = "watch_only";
                s.watch_only = Some(WatchOnly { code, message: mic::plain_input_error(&e) });
                return;
            }
        };
        crate::applog::info(&format!(
            "practice: listening on {} ({} Hz, {} ch)",
            device.name, device.sample_rate, device.channels
        ));
        let settings = self.settings.get();
        let me = crate::keymode::machine_id();
        let out_id = settings.audio.device_id.clone().unwrap_or_else(|| "default".into());
        let manual = settings.practice.latency_offsets_ms.get(&format!("{me}|{}", device.id)).copied().unwrap_or(0.0)
            + settings.audio.device_offsets_ms.get(&out_id).copied().unwrap_or(0.0);
        let measured_key = format!("{me}|{}|{out_id}", device.id);
        let stored = settings.practice.measured_latency_ms.get(&measured_key).map(|m| m.ms / 1000.0);
        let Some(guide) = s.guide.clone() else { return };
        let (tx, rx) = crossbeam_channel::unbounded();
        let init = WorkerInit {
            svc: self.me.get().cloned().unwrap_or_default(),
            engine: Arc::clone(self.player.engine()),
            sink: Arc::clone(&self.sink),
            player: Arc::clone(&self.player),
            guide_service: Arc::clone(&self.guide),
            settings: Arc::clone(&self.settings),
            song_id: s.song_id.clone(),
            path: s.path.clone(),
            guide,
            stream: Some(stream),
            device: device.clone(),
            spec,
            channel_choice: channel,
            octave: (s.octave_mode, s.octave_shift),
            strictness: s.strictness,
            excluded: s.excluded.iter().map(|e| (e.start, e.end)).collect(),
            auto_latency: settings.practice.auto_latency,
            stored_latency_s: stored.filter(|_| settings.practice.auto_latency),
            manual_ms: manual,
            measured_key,
            app_version: self.app_version.clone(),
            machine: me,
            rx,
        };
        s.input = Some(PracticeInputView {
            device,
            channel: channel.unwrap_or(0),
            sample_rate: 0,
            state: "starting",
            message: None,
            meter: None,
            dropped_blocks: 0,
        });
        s.phase = "listening";
        s.watch_only = None;
        let join = std::thread::Builder::new()
            .name("jk-practice".into())
            .spawn(move || Worker::new(init).run())
            .ok();
        s.worker = Some(WorkerHandle { tx, join });
    }

    /// Which input practice opens: a test source, else the saved choice for this
    /// computer (by id, then by name), else the automatic one; or why there is none.
    fn choose_input(&self) -> Result<(SourceSpec, Option<usize>), WatchOnly> {
        if let Some(src) = self.test_source() {
            return Ok((src, None));
        }
        match self.effective_permission() {
            MicPermission::Denied => {
                return Err(WatchOnly { code: "permission_denied", message: inp::mic_denied_steps().to_string() });
            }
            MicPermission::Restricted => {
                return Err(WatchOnly {
                    code: "restricted",
                    message: mic::permission_view(MicPermission::Restricted, None).steps.unwrap_or_default(),
                });
            }
            MicPermission::NotDetermined => {
                return Err(WatchOnly { code: "not_determined", message: inp::MIC_PERMISSION_EXPLANATION.to_string() });
            }
            MicPermission::Granted => {}
        }
        let list = self.list_devices();
        let choice = self.chosen_input();
        match inp::resolve_input(&list, choice.device_id.as_deref(), choice.device_name.as_deref()) {
            Some((d, saved)) => Ok((SourceSpec::Device { id: d.id.clone() }, if saved { choice.channel.map(|c| c as usize) } else { None })),
            None => Err(WatchOnly {
                code: "no_device",
                message: if list.is_empty() {
                    // The practice screen adds that the notes still show and nothing is graded.
                    "No microphone or audio input is connected. Plug in the mixer (or a microphone) and press Practice again."
                        .into()
                } else {
                    "The only inputs are Bluetooth or software devices, which practice never opens by itself. Connect the \
                     mixer, or pick an input by hand."
                        .into()
                },
            }),
        }
    }

    // ------------------------------------------------------------------ from the analysis thread

    pub(crate) fn worker_status(&self, song_id: &str, st: WorkerStatus) {
        let mut g = lock(&self.session);
        let Some(s) = g.as_mut().filter(|s| s.song_id == song_id && s.worker.is_some()) else { return };
        s.input = st.input;
        s.music = st.music;
        s.octave_shift = st.octave_shift;
        if s.octave_mode == OctaveMode::Auto {
            s.octave_settled = st.octave_settled;
        }
        s.latency = st.latency;
        s.take = st.take;
        match (&st.lost, s.phase) {
            (Some(msg), "listening") => {
                s.phase = "watch_only";
                s.watch_only = Some(WatchOnly { code: "input_lost", message: msg.clone() });
            }
            // The result stays on screen; the reason the input is gone is kept with it.
            (Some(msg), "watch_only" | "result") => {
                s.watch_only = Some(WatchOnly { code: "input_lost", message: msg.clone() });
            }
            (None, "result") if s.watch_only.as_ref().is_some_and(|w| w.code == "input_lost") => {
                s.watch_only = None;
            }
            (None, "watch_only") if s.watch_only.as_ref().is_some_and(|w| w.code == "input_lost") => {
                s.phase = if s.last_result.is_some() && !st.take_running { "result" } else { "listening" };
                s.watch_only = None;
            }
            _ => {}
        }
        self.emit_state_locked(&mut g, false);
    }

    pub(crate) fn take_started(&self, song_id: &str) {
        let mut g = lock(&self.session);
        if let Some(s) = g.as_mut().filter(|s| s.song_id == song_id)
            && s.phase == "result"
        {
            s.phase = "listening";
        }
        self.emit_state_locked(&mut g, false);
    }

    pub(crate) fn take_finished(&self, song_id: &str, view: Value) {
        let mut g = lock(&self.session);
        if let Some(s) = g.as_mut().filter(|s| s.song_id == song_id) {
            s.last_result = Some(view);
            // A take that lost its input is graded too (flagged `input_lost`).
            let lost = s.phase == "watch_only" && s.watch_only.as_ref().is_some_and(|w| w.code == "input_lost");
            if s.phase == "listening" || lost {
                s.phase = "result";
            }
        }
        self.emit_state_locked(&mut g, true);
    }

    fn worker_tx(&self) -> Option<Sender<Cmd>> {
        lock(&self.session).as_ref().and_then(|s| s.worker.as_ref().map(|w| w.tx.clone()))
    }

    // ------------------------------------------------------------------ commands

    /// `practice_stop({ keep_result })`: the microphone closes, a running take ends
    /// (graded and saved when enough was sung), practice off. The song stays as it was.
    pub fn stop(&self, keep_result: bool) -> ApiResult<Option<Value>> {
        let session = lock(&self.session).take();
        self.close_meter();
        let Some(mut s) = session else { return Ok(None) };
        s.cancel.store(true, Ordering::SeqCst);
        let mut out = None;
        if let Some(mut w) = s.worker.take() {
            let (rtx, rrx) = crossbeam_channel::bounded(1);
            if w.tx.send(Cmd::Stop { keep_result, reply: rtx }).is_ok() {
                out = rrx.recv_timeout(Duration::from_secs(10)).ok().flatten();
            }
            drop(w.tx);
            if let Some(j) = w.join.take() {
                let _ = j.join();
            }
        }
        crate::applog::info(&format!("practice: stopped on {}", s.song_id));
        let mut g = lock(&self.session);
        if g.is_none() {
            self.emit_state_locked(&mut g, true);
        }
        Ok(out.map(|o: TakeOutcome| o.view))
    }

    /// Called before another song is loaded: practice on the old one ends.
    pub fn song_changing(&self, new_song_id: &str) {
        let other = lock(&self.session).as_ref().is_some_and(|s| s.song_id != new_song_id);
        if other {
            let _ = self.stop(true);
        }
    }

    /// Called before songs are moved to Deleted songs: practice on one of them ends
    /// (graded and saved first when enough was sung, while the file is still there).
    pub fn songs_leaving(&self, song_ids: &[String]) {
        let ours = lock(&self.session).as_ref().is_some_and(|s| song_ids.iter().any(|i| i == &s.song_id));
        if ours {
            let _ = self.stop(true);
        }
    }

    /// `practice_finish()`: "Finish": pauses the song, grades what was sung.
    pub fn finish(&self) -> ApiResult<Option<Value>> {
        let tx = self.worker_tx();
        if lock(&self.session).is_none() {
            return Err(ApiError::bad_request("Practice is not on."));
        }
        let _ = self.player.pause();
        let Some(tx) = tx else { return Ok(None) };
        let (rtx, rrx) = crossbeam_channel::bounded(1);
        tx.send(Cmd::Finish { reply: rtx }).map_err(|_| ApiError::internal("the practice analysis has stopped"))?;
        let out = rrx.recv_timeout(Duration::from_secs(10)).map_err(|_| ApiError::new("timeout", "Grading took too long."))?;
        Ok(out.map(|o| o.view))
    }

    /// `practice_again()`: "Sing again": from the start, a new take, playing.
    pub fn again(&self) -> ApiResult<PracticeStateView> {
        let tx = self.worker_tx();
        if lock(&self.session).is_none() {
            return Err(ApiError::bad_request("Practice is not on."));
        }
        if let Some(tx) = tx {
            let (rtx, rrx) = crossbeam_channel::bounded(1);
            let _ = tx.send(Cmd::Again { reply: rtx });
            let _ = rrx.recv_timeout(Duration::from_secs(5));
        }
        {
            let mut g = lock(&self.session);
            if let Some(s) = g.as_mut()
                && s.phase == "result"
            {
                s.phase = "listening";
            }
            self.emit_state_locked(&mut g, false);
        }
        self.player.seek(0.0)?;
        self.player.play()?;
        Ok(self.state())
    }

    /// `practice_suspend_input(suspended)`: the microphone closes while the window is
    /// hidden and opens again when it is back (the rig check uses it too).
    pub fn suspend_input(&self, suspended: bool) -> ApiResult<()> {
        if suspended {
            self.close_meter();
        }
        if let Some(tx) = self.worker_tx() {
            let _ = tx.send(Cmd::Suspend(suspended));
        }
        Ok(())
    }

    /// Writes practice data into a song's slot through the song-file lock (never during a
    /// guide swap), after any pending mix save of that song.
    fn save_slot(&self, path: &Path, edit: impl FnMut(&mut jk_format::practice::Practice)) -> ApiResult<()> {
        let _files = self.guide.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.player.saver.flush();
        jk_format::practice::save_practice(path, edit).map_err(ApiError::from)?;
        self.player.refresh_current_from_disk(path);
        Ok(())
    }

    /// `practice_song_set(song_id, { octave?, strictness? })`: saved in the song's slot;
    /// the lane moves at once and a running take grades the notes still to come with it.
    pub fn song_set(&self, song_id: &str, changes: &Value) -> ApiResult<PracticeStateView> {
        let path = self.library.resolve(song_id)?;
        let obj = changes.as_object().ok_or_else(|| ApiError::bad_request("The changes must be an object."))?;
        if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "octave" | "strictness")) {
            return Err(ApiError::bad_request(format!("Unknown practice setting '{k}'.")));
        }
        let mut octave: Option<(OctaveMode, Option<i32>)> = None;
        if let Some(o) = obj.get("octave") {
            let mode = o
                .get("mode")
                .and_then(Value::as_str)
                .and_then(OctaveMode::parse)
                .ok_or_else(|| ApiError::bad_request("octave.mode must be auto, fixed or strict."))?;
            let shift = match o.get("shift") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_f64()
                        .filter(|x| x.is_finite() && x.fract() == 0.0)
                        .ok_or_else(|| ApiError::bad_request("octave.shift must be a whole number from -2 to 2."))?
                        .clamp(-2.0, 2.0) as i32,
                ),
            };
            octave = Some((mode, shift));
        }
        let mut strictness: Option<Option<Strictness>> = None;
        if let Some(v) = obj.get("strictness") {
            strictness = Some(match v {
                Value::Null => None,
                Value::String(s) => Some(
                    Strictness::parse(s).ok_or_else(|| ApiError::bad_request("strictness must be relaxed, standard, strict or null."))?,
                ),
                _ => return Err(ApiError::bad_request("strictness must be relaxed, standard, strict or null.")),
            });
        }
        let mut new_octave = None;
        let mut new_strict = None;
        self.save_slot(&path, |p| {
            if let Some((mode, shift)) = octave {
                p.octave.mode = mode.name().into();
                if let Some(sh) = shift {
                    p.octave.shift = sh;
                }
                if mode == OctaveMode::Strict {
                    p.octave.shift = 0;
                }
                new_octave = Some((mode, p.octave.shift));
            }
            if let Some(st) = strictness {
                p.strictness = st.map(|x| x.name().to_string());
                new_strict = Some(st);
            }
        })?;
        let app_strictness = strictness_setting(self.settings.get().practice.strictness);
        let tx = {
            let mut g = lock(&self.session);
            let tx = match g.as_mut().filter(|s| s.song_id == song_id) {
                Some(s) => {
                    if let Some((m, sh)) = new_octave {
                        s.octave_mode = m;
                        s.octave_shift = sh;
                        s.octave_settled = m != OctaveMode::Auto;
                    }
                    if let Some(st) = new_strict {
                        s.strictness = st.unwrap_or(app_strictness);
                    }
                    s.worker.as_ref().map(|w| w.tx.clone())
                }
                None => None,
            };
            self.emit_state_locked(&mut g, false);
            tx
        };
        if let Some(tx) = tx {
            let _ = tx.send(Cmd::Settings {
                octave: new_octave,
                strictness: new_strict.map(|s| s.unwrap_or(app_strictness)),
            });
        }
        Ok(self.state())
    }

    /// `practice_exclude(song_id, span, excluded)`: "the guide is wrong here" and its undo,
    /// saved in the song's slot at once.
    pub fn exclude(&self, song_id: &str, span: &ExcludedSpan, excluded: bool) -> ApiResult<Vec<ExcludedSpan>> {
        let path = self.library.resolve(song_id)?;
        if !(span.start.is_finite() && span.end.is_finite() && span.start <= span.end && span.start >= 0.0) {
            return Err(ApiError::bad_request("The span needs a start and an end in seconds, start before end."));
        }
        let same = |a: &jk_format::practice::Excluded| (a.start - span.start).abs() < 0.005 && (a.end - span.end).abs() < 0.005;
        let mut list = Vec::new();
        self.save_slot(&path, |p| {
            if excluded {
                if !p.excluded.iter().any(same) {
                    p.excluded.push(jk_format::practice::Excluded {
                        line: span.line,
                        start: (span.start * 100.0).round() / 100.0,
                        end: (span.end * 100.0).round() / 100.0,
                        extra: Default::default(),
                    });
                    p.excluded.sort_by(|a, b| a.start.total_cmp(&b.start));
                }
            } else {
                p.excluded.retain(|e| !same(e) && !(span.line.is_some() && e.line == span.line));
            }
            list = p.excluded.iter().map(|e| ExcludedSpan { line: e.line, start: e.start, end: e.end }).collect();
        })?;
        let tx = {
            let mut g = lock(&self.session);
            let tx = match g.as_mut().filter(|s| s.song_id == song_id) {
                Some(s) => {
                    s.excluded = list.clone();
                    s.worker.as_ref().map(|w| w.tx.clone())
                }
                None => None,
            };
            self.emit_state_locked(&mut g, false);
            tx
        };
        if let Some(tx) = tx {
            let _ = tx.send(Cmd::Excluded(list.iter().map(|e| (e.start, e.end)).collect()));
        }
        Ok(list)
    }

    /// `practice_history(song_id)`: the best result and the 20 most recent, newest first,
    /// from the song's slot (it travels with the song).
    pub fn history(&self, song_id: &str) -> ApiResult<HistoryView> {
        let path = self.library.resolve(song_id)?;
        let song = jk_format::SongFile::open(&path)?;
        let key = crate::library::song_key(&song);
        let p = song.user_state().practice().unwrap_or_default();
        let is_best = |r: &jk_format::practice::PracticeResult| p.best.as_ref().is_some_and(|b| b.date == r.date && b.overall == r.overall);
        Ok(HistoryView {
            best: p.best.as_ref().map(|b| result::screen_view(b, true, true, key)),
            recent: p.recent.iter().map(|r| result::screen_view(r, is_best(r), true, key)).collect(),
        })
    }

    /// `practice_latency_set(manual_ms, device_id?)`: the manual offset for this computer
    /// and input (default: the chosen one), -500..500 ms; 0 removes it.
    pub fn latency_set(&self, manual_ms: f64, device_id: Option<String>) -> ApiResult<f64> {
        if !manual_ms.is_finite() {
            return Err(ApiError::bad_request("The delay must be a number of milliseconds."));
        }
        let ms = manual_ms.clamp(-crate::settings::MAX_PRACTICE_OFFSET_MS, crate::settings::MAX_PRACTICE_OFFSET_MS).round();
        let session_dev = lock(&self.session).as_ref().and_then(|s| s.input.as_ref().map(|i| i.device.id.clone()));
        let id = device_id
            .filter(|s| !s.is_empty())
            .or_else(|| session_dev.clone())
            .or_else(|| self.chosen_input().device_id)
            .or_else(|| inp::preferred_input(&self.list_devices()).map(|d| d.id.clone()))
            .unwrap_or_else(|| "default".into());
        let me = crate::keymode::machine_id();
        let key = format!("{me}|{id}");
        let s = self.settings.update(|s| {
            if ms == 0.0 {
                s.practice.latency_offsets_ms.remove(&key);
            } else {
                s.practice.latency_offsets_ms.insert(key.clone(), ms);
            }
        });
        if session_dev.as_deref() == Some(id.as_str())
            && let Some(tx) = self.worker_tx()
        {
            let out_id = s.audio.device_id.clone().unwrap_or_else(|| "default".into());
            let _ = tx.send(Cmd::Manual(ms + s.audio.device_offsets_ms.get(&out_id).copied().unwrap_or(0.0)));
        }
        Ok(ms)
    }

    /// `practice_guide(song_id?)`: the guide as the lane draws it, exclusions applied.
    pub fn guide_view(&self, song_id: Option<&str>) -> ApiResult<PracticeGuideView> {
        let id = match song_id.filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => self
                .player
                .current()
                .map(|c| c.song_id)
                .ok_or_else(|| ApiError::new("not_loaded", "No song is loaded. Choose a song first."))?,
        };
        if let Some((g, ex)) = lock(&self.session)
            .as_ref()
            .filter(|s| s.song_id == id)
            .and_then(|s| s.guide.clone().map(|g| (g, s.excluded.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>())))
        {
            return Ok(g.view(&ex));
        }
        let path = self.library.resolve(&id)?;
        match self.acquire_guide(&id, &path) {
            Acquire::Ready(sg) => {
                let song = jk_format::SongFile::open(&path)?;
                let ex: Vec<(f64, f64)> = song
                    .user_state()
                    .practice()
                    .map(|p| p.excluded.iter().map(|e| (e.start, e.end)).collect())
                    .unwrap_or_default();
                Ok(sg.view(&ex))
            }
            Acquire::NotReady(nr) => Err(ApiError::new("no_guide", nr.message)),
            _ => Err(ApiError::new(
                "no_guide",
                "This song has no practice data yet. Press Practice to add it (it takes a few seconds).",
            )),
        }
    }

    /// `practice_target_at(position_s, segment?)` (internal): the scorer's target at that
    /// song time by APP_API 12.1's rule.
    pub fn target_at(&self, position_s: f64, segment: Option<u64>) -> ApiResult<TargetView> {
        if !position_s.is_finite() {
            return Err(ApiError::bad_request("The position must be a number of seconds."));
        }
        let (guide, shift, excluded) = {
            let g = lock(&self.session);
            let s = g.as_ref().ok_or_else(|| ApiError::bad_request("Practice is not on."))?;
            let guide = s.guide.clone().ok_or_else(|| ApiError::new("no_guide", "The practice data is not ready yet."))?;
            let shift = if s.octave_mode == OctaveMode::Strict { 0 } else { s.octave_shift };
            (guide, shift, s.excluded.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>())
        };
        let engine = self.player.engine();
        let key = engine.key_at(position_s, segment).or_else(|| {
            let snap = engine.snapshot();
            (segment.is_none() || segment == Some(snap.key.segment)).then_some(snap.key.heard)
        });
        let Some(i) = guide.note_at(position_s) else {
            return Ok(TargetView { note: None, midi: None, lane: None, name: None, key, octave: shift, graded: false });
        };
        let n = &guide.doc.notes[i];
        let k = key.unwrap_or(0);
        let midi = n.midi + f64::from(k) + 12.0 * f64::from(shift);
        let lane = midi - guide.tuning_cents() / 100.0;
        let flats = crate::keys::note_spelling(guide.song_key, k) == "flats";
        Ok(TargetView {
            note: Some(i),
            midi: Some((midi * 1000.0).round() / 1000.0),
            lane: Some((lane * 1000.0).round() / 1000.0),
            name: n.is_pitched().then(|| jk_pitch::score::note_name(lane.round() as i32, flats)),
            key,
            octave: shift,
            graded: guide.graded(i, &excluded),
        })
    }

    /// On exit: a running take is graded and saved when enough was sung, the microphone
    /// closes.
    pub fn shutdown(&self) {
        let _ = self.stop(true);
        self.close_meter();
    }

    /// For the rig check: the input the session or the saved choice uses.
    pub(crate) fn rig_input(&self, device_id: Option<&str>) -> Result<SourceSpec, String> {
        if let Some(t) = self.test_source() {
            return Ok(t);
        }
        let list = self.list_devices();
        if let Some(id) = device_id.filter(|s| !s.is_empty()) {
            return if list.iter().any(|d| d.id == id) {
                Ok(SourceSpec::Device { id: id.to_string() })
            } else {
                Err("That input is not connected.".into())
            };
        }
        let c = self.chosen_input();
        inp::resolve_input(&list, c.device_id.as_deref(), c.device_name.as_deref())
            .map(|(d, _)| SourceSpec::Device { id: d.id.clone() })
            .ok_or_else(|| "No microphone or audio input is connected. Plug in the mixer and run the check again.".into())
    }

    /// `practice_guide` capability text (`AppInfo.capabilities.practice`).
    pub fn practice_capability() -> PracticeCapability {
        PracticeCapability { guide_method: jk_guide::METHOD.to_string(), guide_version: jk_guide::VERSION.to_string() }
    }
}

/// What `acquire_guide` found.
enum Acquire {
    Ready(SessionGuide),
    /// Build it on the spot through the guide queue.
    Build,
    /// Build it in memory for this session (a guide made by a newer Jukeoke is kept).
    InMemory,
    NotReady(NotReady),
}

impl Drop for PracticeService {
    fn drop(&mut self) {
        if let Some(m) = lock(&self.meter).take() {
            m.close();
        }
    }
}

/// A JSON object of `practice_start`'s options.
pub fn start_options(v: &Value) -> (Option<String>, bool) {
    let song = v.get("song_id").and_then(Value::as_str).map(str::to_string);
    let watch = v.get("watch_only").and_then(Value::as_bool).unwrap_or(false);
    (song, watch)
}

/// `{ result }` of `practice_stop` / `practice_finish`.
pub fn result_payload(r: Option<Value>) -> Value {
    json!({ "result": r })
}

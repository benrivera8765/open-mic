// SPDX-License-Identifier: GPL-3.0-or-later
//! The microphone side of practice mode (docs/APP_API.md 12.4): the input list as the UI
//! sees it, the permission (with Windows' privacy switches, read only), per-channel
//! meters, the input picker's meter stream, and the sources practice reads from.
//!
//! **Privacy.** Sound from an input is read into memory, measured and dropped. Nothing in
//! this module (or anywhere in practice mode) writes captured sound to disk, sends it
//! anywhere or plays it back. The only files read are generated test WAVs handed to
//! `practice_test_input` (APP_API 12.1), and only in the app's test modes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jk_recorder::input::{
    self as inp, ChannelLevel, FailureKind, InputDeviceInfo, InputError, InputLevels, InputSource, InputState,
    InputStream, LevelAdvice, MicPermission, MicPrivacySwitches, Pacing, SignalEnd, SignalSource, Transport,
};
use serde::Serialize;

use crate::events::{EventSink, emit};

/// `mic://levels`, `mic://permission`, `mic://devices` (APP_API 12.4).
pub mod names {
    pub const MIC_LEVELS: &str = "mic://levels";
    pub const MIC_PERMISSION: &str = "mic://permission";
    pub const MIC_DEVICES: &str = "mic://devices";
}

/// The `MicInput.kind` name of a transport.
pub fn kind_name(t: Transport) -> &'static str {
    match t {
        Transport::Usb => "usb",
        Transport::BuiltIn => "built_in",
        Transport::WiredExternal => "wired_external",
        Transport::Bluetooth => "bluetooth",
        Transport::Display => "display",
        Transport::Virtual => "virtual",
        Transport::Unknown => "unknown",
    }
}

/// docs/APP_API.md 12.4 `MicInput`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MicInput {
    pub id: String,
    pub name: String,
    pub adapter: Option<String>,
    pub kind: &'static str,
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: Option<String>,
    pub is_default: bool,
    pub bluetooth: bool,
    pub preferred: bool,
    pub declared_latency_ms: Option<f64>,
    pub same_device_as_output: bool,
    pub warning: Option<String>,
}

/// The text inside the last pair of parentheses of an endpoint name ("Speakers (MAONO
/// AME2)" -> "maono ame2"), else the whole name; lower case, trimmed.
fn hardware_part(name: &str) -> String {
    let n = name.trim();
    let inner = match (n.rfind('('), n.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => &n[a + 1..b],
        _ => n,
    };
    inner.trim().to_lowercase()
}

/// True when the input and the output device look like one piece of hardware (a USB
/// mixer used both ways: one clock). Windows names endpoints "Microphone (X)" and
/// "Speakers (X)"; macOS shows one device under one name. A heuristic for the picker.
pub fn same_hardware(input: &InputDeviceInfo, output_name: Option<&str>) -> bool {
    let Some(out) = output_name.map(hardware_part).filter(|s| !s.is_empty()) else { return false };
    let mut candidates = vec![hardware_part(&input.name)];
    if let Some(a) = &input.adapter {
        candidates.push(hardware_part(a));
    }
    candidates.iter().any(|c| !c.is_empty() && *c == out)
}

/// The UI's view of a listed input.
pub fn mic_input(d: &InputDeviceInfo, preferred_id: Option<&str>, output_name: Option<&str>) -> MicInput {
    MicInput {
        id: d.id.clone(),
        name: d.name.clone(),
        adapter: d.adapter.clone(),
        kind: kind_name(d.transport),
        channels: d.channels,
        sample_rate: d.sample_rate,
        sample_format: d.sample_format.clone(),
        is_default: d.is_default,
        bluetooth: d.transport == Transport::Bluetooth || d.hands_free,
        preferred: preferred_id == Some(d.id.as_str()),
        declared_latency_ms: d
            .declared_latency_frames
            .filter(|_| d.sample_rate > 0)
            .map(|f| f64::from(f) * 1000.0 / f64::from(d.sample_rate)),
        same_device_as_output: same_hardware(d, output_name),
        warning: d.warning().map(str::to_string).or_else(|| {
            d.hands_free.then(|| {
                "This is a Bluetooth hands-free microphone. Opening it turns the headset's sound into low-quality \
                 mono. Use a cable or the mixer's USB connection for practice."
                    .to_string()
            })
        }),
    }
}

/// A test source as the UI sees it (`practice_test_input`): never a real device.
pub fn test_input_view(name: &str, rate: u32, channels: u16) -> MicInput {
    MicInput {
        id: format!("test:{name}"),
        name: format!("{name} (generated test input)"),
        adapter: None,
        kind: "virtual",
        channels,
        sample_rate: rate,
        sample_format: Some("f32".into()),
        is_default: false,
        bluetooth: false,
        preferred: false,
        declared_latency_ms: Some(0.0),
        same_device_as_output: false,
        warning: None,
    }
}

/// docs/APP_API.md 12.4 `ChannelMeter`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelMeter {
    pub index: usize,
    pub peak_db: f64,
    pub rms_db: f64,
    pub noise_floor_db: Option<f64>,
    pub active_db: Option<f64>,
    pub clipping: bool,
    pub clipped_samples: u64,
    pub advice: &'static str,
    pub message: Option<String>,
}

fn db1(x: f32) -> f64 {
    let v = f64::from(x).max(-120.0);
    (v * 10.0).round() / 10.0
}

/// The `ChannelMeter.advice` name.
pub fn advice_name(a: LevelAdvice) -> &'static str {
    match a {
        LevelAdvice::Good => "good",
        LevelAdvice::Waiting => "waiting",
        LevelAdvice::TooQuiet => "too_quiet",
        LevelAdvice::Clipping => "clipping",
        LevelAdvice::Silent => "silent",
    }
}

pub fn channel_meter(index: usize, c: &ChannelLevel) -> ChannelMeter {
    let msg = c.advice.message();
    ChannelMeter {
        index,
        peak_db: db1(c.peak_dbfs),
        rms_db: db1(c.rms_dbfs),
        noise_floor_db: c.noise_floor_dbfs.map(db1),
        active_db: c.active_rms_dbfs.map(db1),
        clipping: c.clipped_recently,
        clipped_samples: c.clipped_samples,
        advice: advice_name(c.advice),
        message: (!msg.is_empty()).then(|| msg.to_string()),
    }
}

/// `mic://levels`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelsEvent {
    pub device_id: String,
    pub seconds: f64,
    pub channels: Vec<ChannelMeter>,
    pub suggested_channel: Option<usize>,
    pub chosen_channel: Option<usize>,
}

pub fn levels_event(device_id: &str, lv: &InputLevels, chosen: Option<usize>) -> LevelsEvent {
    LevelsEvent {
        device_id: device_id.to_string(),
        seconds: (lv.seconds * 100.0).round() / 100.0,
        channels: lv.channels.iter().enumerate().map(|(i, c)| channel_meter(i, c)).collect(),
        suggested_channel: inp::suggest_voice_channel(lv),
        chosen_channel: chosen,
    }
}

/// docs/APP_API.md 12.4 `MicPermission`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MicPermissionView {
    pub state: &'static str,
    pub explanation: String,
    pub steps: Option<String>,
    pub settings_url: Option<String>,
    pub windows: Option<MicPrivacySwitches>,
}

pub fn permission_name(p: MicPermission) -> &'static str {
    match p {
        MicPermission::Granted => "granted",
        MicPermission::Denied => "denied",
        MicPermission::NotDetermined => "not_determined",
        MicPermission::Restricted => "restricted",
    }
}

/// The permission as the UI shows it. `switches` are Windows' privacy switches (read
/// only, never changed: WO2 section 3).
pub fn permission_view(p: MicPermission, switches: Option<MicPrivacySwitches>) -> MicPermissionView {
    let refused = matches!(p, MicPermission::Denied | MicPermission::Restricted);
    MicPermissionView {
        state: permission_name(p),
        explanation: inp::MIC_PERMISSION_EXPLANATION.to_string(),
        steps: refused.then(|| {
            if p == MicPermission::Restricted {
                format!(
                    "A policy on this computer (parental controls or a management profile) does not allow apps to use the \
                     microphone. {}",
                    inp::mic_denied_steps()
                )
            } else {
                inp::mic_denied_steps().to_string()
            }
        }),
        settings_url: Some(
            if cfg!(target_os = "macos") { inp::MACOS_MICROPHONE_SETTINGS_URL } else { inp::WINDOWS_MICROPHONE_SETTINGS_URI }
                .to_string(),
        ),
        windows: switches,
    }
}

/// The `InputError.kind` name.
pub fn failure_name(k: FailureKind) -> &'static str {
    match k {
        FailureKind::NoDevice => "no_device",
        FailureKind::Unplugged => "unplugged",
        FailureKind::PermissionDenied => "permission_denied",
        FailureKind::Stalled => "stalled",
        FailureKind::Format => "format",
        FailureKind::Device => "device",
    }
}

/// docs/APP_API.md 12.4 `InputError`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputErrorView {
    pub kind: &'static str,
    pub message: String,
}

impl From<&InputError> for InputErrorView {
    fn from(e: &InputError) -> Self {
        InputErrorView { kind: failure_name(e.kind), message: plain_input_error(e) }
    }
}

/// An input failure in plain words with what to do (the recorder's messages are already
/// plain; a few get the practice screen's advice added).
pub fn plain_input_error(e: &InputError) -> String {
    let m = e.message.trim();
    let first_cap = |s: &str| {
        let mut c = s.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    };
    let base = first_cap(m);
    let base = if base.ends_with('.') { base } else { format!("{base}.") };
    match e.kind {
        // The practice screen adds that the notes still show and nothing is graded.
        FailureKind::Unplugged | FailureKind::Stalled => format!("{base} Plug it back in (or switch it on) and it is picked up again."),
        FailureKind::PermissionDenied => format!("{base} {}", inp::mic_denied_steps()),
        _ => base,
    }
}

/// Where practice reads sound from.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceSpec {
    /// A microphone or mixer, by the id `list_input_devices` gave.
    Device { id: String },
    /// A WAV of generated sound, played at real-time pace from the moment it is opened;
    /// its end behaves like an unplugged device (`practice_test_input`, APP_API 12.1).
    TestWav(PathBuf),
    /// A WAV of generated sound sung along to the song: each block is the part of the WAV
    /// at the song time heard when it was captured (src/practice/testsource.rs); `late_ms`
    /// makes the singer late; after `unplug_after_s` s it reports an unplugged device.
    TestSyncWav { path: PathBuf, late_ms: f64, unplug_after_s: Option<f64> },
    /// Digital silence at 48 kHz, mono, for as long as it is read.
    TestSilence,
    /// A generated mixer return: the app's own playback (the engine's output tap) back
    /// `delay_ms` late at `level_db`, plus an optional generated singer (src/practice/
    /// testsource.rs).
    TestReturn { delay_ms: f64, level_db: f64, voice: Option<PathBuf> },
}

impl SourceSpec {
    pub fn is_test(&self) -> bool {
        !matches!(self, SourceSpec::Device { .. })
    }
}

/// Opens a source as an [`InputStream`] (the same rings, meters and stamps for every
/// kind) and the UI's view of it.
pub fn open_source(
    spec: &SourceSpec,
    list: &[InputDeviceInfo],
    output_name: Option<&str>,
    engine: &Arc<dyn crate::engine::AudioEngine>,
) -> Result<(InputStream, MicInput), InputError> {
    match spec {
        SourceSpec::Device { id } => {
            let info = list.iter().find(|d| &d.id == id);
            let stream = InputStream::open(Box::new(inp::DeviceSource::new(id.clone())))?;
            let f = stream.format();
            let view = match info {
                Some(d) => {
                    let mut v = mic_input(d, None, output_name);
                    v.channels = f.channels;
                    v.sample_rate = f.sample_rate;
                    v.sample_format = Some(f.sample_format.clone());
                    v
                }
                None => MicInput {
                    id: id.clone(),
                    name: f.device_name.clone(),
                    adapter: None,
                    kind: "unknown",
                    channels: f.channels,
                    sample_rate: f.sample_rate,
                    sample_format: Some(f.sample_format.clone()),
                    is_default: false,
                    bluetooth: false,
                    preferred: false,
                    declared_latency_ms: f.declared_latency_ns.map(|n| n as f64 / 1e6),
                    same_device_as_output: false,
                    warning: None,
                },
            };
            Ok((stream, view))
        }
        SourceSpec::TestWav(path) => {
            let src = SignalSource::from_wav(path)?.pacing(Pacing::RealTime).at_end(SignalEnd::Unplugs);
            let stream = InputStream::open(Box::new(src))?;
            let f = stream.format().clone();
            Ok((stream, test_input_view(&f.device_name, f.sample_rate, f.channels)))
        }
        SourceSpec::TestSilence => {
            let rate = 48_000u32;
            let src = SignalSource::new("digital silence", rate, 1, vec![0.0; rate as usize])
                .pacing(Pacing::RealTime)
                .at_end(SignalEnd::Loops);
            let stream = InputStream::open(Box::new(src) as Box<dyn InputSource>)?;
            Ok((stream, test_input_view("digital silence", rate, 1)))
        }
        SourceSpec::TestSyncWav { path, late_ms, unplug_after_s } => {
            let src = super::testsource::SyncWavSource::new(Arc::clone(engine), path, *late_ms, *unplug_after_s)?;
            let stream = InputStream::open(Box::new(src))?;
            let f = stream.format().clone();
            Ok((stream, test_input_view(&f.device_name, f.sample_rate, f.channels)))
        }
        SourceSpec::TestReturn { delay_ms, level_db, voice } => {
            let src = super::testsource::ReturnSource::new(Arc::clone(engine), *delay_ms, *level_db, voice.as_deref())?;
            let stream = InputStream::open(Box::new(src))?;
            let f = stream.format().clone();
            Ok((stream, test_input_view("mixer return", f.sample_rate, f.channels)))
        }
    }
}

/// The input picker's meters (`mic_meter`): one device open for its per-channel meters
/// only, about 20 `mic://levels` events a second, until it is closed. No tracking.
pub struct Meter {
    pub device_id: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Meter {
    /// Starts metering an already opened stream on its own thread.
    pub fn start(device_id: String, mut stream: InputStream, sink: Arc<dyn EventSink>) -> Meter {
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let id = device_id.clone();
        let thread = std::thread::Builder::new()
            .name("jk-mic-meter".into())
            .spawn(move || {
                let mut buf = Vec::new();
                let mut last = Instant::now() - Duration::from_secs(1);
                while !s.load(Ordering::SeqCst) {
                    // Read and drop: only the meters see the sound.
                    while stream.read(&mut buf).is_some() {}
                    buf.iter_mut().for_each(|x| *x = 0.0);
                    if last.elapsed() >= Duration::from_millis(50) {
                        last = Instant::now();
                        let lv = stream.levels();
                        emit(&*sink, names::MIC_LEVELS, &levels_event(&id, &lv, None));
                        if let InputState::Failed { .. } = stream.state() {
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                stream.stop();
            })
            .ok();
        Meter { device_id, stop, thread }
    }

    /// Closes the device (joins the thread).
    pub fn close(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, adapter: Option<&str>, t: Transport) -> InputDeviceInfo {
        InputDeviceInfo {
            id: format!("id:{name}"),
            name: name.into(),
            adapter: adapter.map(str::to_string),
            transport: t,
            form_factor: None,
            bus: None,
            channels: 2,
            sample_rate: 48_000,
            sample_format: Some("i24".into()),
            is_default: false,
            hands_free: t == Transport::Bluetooth,
            declared_latency_frames: Some(480),
        }
    }

    #[test]
    fn a_usb_mixer_is_recognised_on_both_sides() {
        let mixer = dev("Microphone (MAONO AME2)", Some("MAONO AME2"), Transport::Usb);
        assert!(same_hardware(&mixer, Some("Speakers (MAONO AME2)")));
        assert!(!same_hardware(&mixer, Some("Speakers (Realtek(R) Audio)")));
        assert!(!same_hardware(&mixer, None));
        let v = mic_input(&mixer, Some("id:Microphone (MAONO AME2)"), Some("Speakers (MAONO AME2)"));
        assert_eq!((v.kind, v.preferred, v.same_device_as_output, v.bluetooth), ("usb", true, true, false));
        assert_eq!(v.declared_latency_ms, Some(10.0));
        let bt = mic_input(&dev("Headset (AirPods Hands-Free)", None, Transport::Bluetooth), None, None);
        assert!(bt.bluetooth && bt.warning.is_some() && !bt.preferred);
    }

    #[test]
    fn permission_views_say_what_to_do() {
        let d = permission_view(MicPermission::Denied, None);
        assert_eq!(d.state, "denied");
        assert!(d.steps.as_deref().is_some_and(|s| s.contains("Microphone")));
        assert!(d.explanation.contains("never"));
        let g = permission_view(MicPermission::Granted, None);
        assert!(g.steps.is_none() && g.settings_url.is_some());
    }

    #[test]
    fn a_wav_test_source_ends_like_an_unplugged_device() {
        let dir = std::env::temp_dir().join(format!("jk-mic-test-{}-{:08x}", std::process::id(), crate::fs_atomic::random_u32()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("generated.wav");
        let rate = 16_000;
        let mut w = jk_recorder::wav::WavWriter::create(&p, jk_recorder::wav::WavSpec::new(rate, 1)).unwrap();
        w.write_interleaved(&vec![0.01f32; rate as usize / 5]).unwrap();
        w.finalize().unwrap();
        let engine: Arc<dyn crate::engine::AudioEngine> = Arc::new(crate::engine::fake::FakeEngine::default());
        let (mut s, view) = open_source(&SourceSpec::TestWav(p.clone()), &[], None, &engine).unwrap();
        assert_eq!((view.sample_rate, view.channels, view.kind), (rate, 1, "virtual"));
        let t = Instant::now();
        let mut buf = Vec::new();
        let mut failed = None;
        while t.elapsed() < Duration::from_secs(3) {
            while s.read(&mut buf).is_some() {}
            if let InputState::Failed { kind, .. } = s.state() {
                failed = Some(kind);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(failed, Some(FailureKind::Unplugged));
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

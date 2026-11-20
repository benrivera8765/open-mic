// SPDX-License-Identifier: GPL-3.0-or-later
//! # Practice-mode input: listening to the singer
//!
//! WORK_ORDER-2 sections 4.4 ("Listening to the singer"), 4.8 (the Mac), 4.10
//! (the owner's rig: a MAONO AME2 podcast mixer over USB), 5 (real-time safety)
//! and traps 2, 3, 4 and 14.
//!
//! **Privacy.** Microphone sound is analysed in memory and thrown away. Nothing
//! in this module writes captured sound to disk or sends it anywhere; the only
//! file this module reads is a test WAV given to [`SignalSource::from_wav`].
//!
//! ## The path
//!
//! ```text
//!  InputSource (microphone | generated signal | WAV file)
//!     | capture callback / thread: copies samples + a host-clock time stamp
//!     v
//!  lock-free rings (ring.rs: samples + one BlockStamp per block)
//!     |
//!     v analysis thread
//!  InputStream::read()  -> interleaved block + stamp, per-channel meters updated
//!  extract_channel()    -> the voice channel, bit-exact (never summed, never halved)
//! ```
//!
//! * [`InputSource`] is the small interface a microphone, a generated signal
//!   and a WAV file all implement, so tests and the probe feed the very same
//!   path the microphone uses.
//! * The capture callback only copies (with an exact integer-to-float mapping
//!   where the OS delivers integers) and stamps. It never allocates, locks,
//!   meters or analyses ([`ring`]).
//! * Time stamps are host-clock nanoseconds of the block's first frame as the
//!   OS reports them ([`clock`]: QPC on Windows, mach host time on macOS), never
//!   a sample count (trap 4).
//! * Any sample rate and channel count are accepted as the device runs them;
//!   nothing is resampled or mixed here (trap 14).
//! * Levels per channel (peak, RMS, noise floor, clipping, advice) are computed
//!   on the reading thread ([`levels`]).
//! * A device that is unplugged or stops delivering ends in a clean
//!   [`InputState::Failed`] with a plain message (a stall watchdog covers
//!   drivers that just go quiet). Nothing panics.
//!
//! ## Backends
//!
//! * **Windows:** WASAPI shared-mode capture of an input endpoint at its mix
//!   format, event driven, asking for the raw stream (no Windows voice
//!   processing) when the device allows it, stream category "Other" (never
//!   "Communications", which would make Windows duck other sounds).
//! * **macOS:** cpal 0.16 (pinned: it runs on macOS 13), which opens the device
//!   with the plain HAL output unit. Apple's voice-processing I/O unit is never
//!   used: on macOS 13 it lowers and reshapes the app's own playback (WO2 4.8).
//!   Microphone permission via `AVCaptureDevice` (macOS 10.14+).
//! * **Elsewhere:** listing returns nothing; generated sources still work.

pub mod clock;
pub mod device;
pub mod levels;
pub mod ring;
pub mod signal;
pub mod source;

// (Named mac / win so they never shadow the `windows` crate.)
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as platform;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as platform;

#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(not(any(windows, target_os = "macos")))]
use other as platform;

use std::sync::Arc;

use serde::Serialize;

pub use device::{InputDeviceInfo, Transport, preferred_input, resolve_input};
pub use levels::{ChannelLevel, ChannelLevels, InputLevels, LevelAdvice, suggest_voice_channel};
pub use ring::{BlockStamp, CaptureConsumer, CaptureProducer, FailureKind, RunState, StreamStatus};
pub use source::{Pacing, SignalEnd, SignalSource};

use crate::error::RecorderError;

/// Why an input could not be opened or listed, in plain words, with a kind the
/// UI can act on. (Its own type, because [`RecorderError`] describes system-audio
/// recording.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputError {
    /// What kind of failure.
    pub kind: FailureKind,
    /// Plain-language message (what happened and what to do).
    pub message: String,
}

impl InputError {
    /// An error of `kind` with `message`.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn stopped() -> Self {
        Self::new(FailureKind::Device, "the input was already stopped")
    }
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InputError {}

impl From<std::io::Error> for InputError {
    fn from(e: std::io::Error) -> Self {
        Self::new(FailureKind::Device, format!("could not read the file: {e}"))
    }
}

impl From<InputError> for RecorderError {
    fn from(e: InputError) -> Self {
        match e.kind {
            FailureKind::PermissionDenied => RecorderError::PermissionDenied(e.message),
            FailureKind::Format => RecorderError::Format(e.message),
            _ => RecorderError::Backend {
                domain: "input".into(),
                code: -1,
                message: e.message,
            },
        }
    }
}
use clock::host_time_ns;

/// Seconds of sound the rings hold: how late the analysis thread may be before
/// blocks are dropped (and counted).
pub const RING_SECONDS: f64 = 2.0;
/// A running stream that delivers nothing for this long has failed (`Stalled`).
pub const STALL_TIMEOUT_NS: i64 = 1_000_000_000;
/// A stream that has not delivered its first block after this long has failed.
pub const FIRST_BLOCK_TIMEOUT_NS: i64 = 3_000_000_000;

/// What a source delivers, as it really is (never the request).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputFormat {
    /// The technology ("WASAPI shared mode, raw stream", "Core Audio (cpal 0.16)",
    /// "generated signal ...").
    pub source: String,
    /// The device's name.
    pub device_name: String,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels per frame (all of the device's input channels).
    pub channels: u16,
    /// Sample format the OS delivers ("f32", "i16", "i24", "i32"); always handed
    /// on as f32.
    pub sample_format: String,
    /// The largest block the source will deliver (sizes the rings).
    pub max_block_frames: usize,
    /// Input latency the OS declares, in nanoseconds, when it declares one.
    pub declared_latency_ns: Option<i64>,
    /// Anything else worth reporting (raw mode refused, ...).
    pub notes: Vec<String>,
}

/// Something that delivers blocks of sound like a microphone.
///
/// Implementations: the platform microphone ([`DeviceSource`]), a generated
/// or file signal ([`SignalSource`]), and test doubles.
pub trait InputSource: Send {
    /// Open the source and report the format it will deliver. Nothing is
    /// delivered yet.
    fn open(&mut self) -> Result<InputFormat, InputError>;
    /// Start delivering into `producer`, which moves into the capture callback
    /// or thread. That callback may only call the producer's push methods.
    /// Failures after this point go to `producer.status().fail(..)`.
    fn start(&mut self, producer: CaptureProducer) -> Result<(), InputError>;
    /// Stop delivering and release the device. Idempotent.
    fn stop(&mut self);
}

/// The stream's state, as the app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InputState {
    /// Opened, waiting for the first block.
    Starting,
    /// Sound is arriving.
    Running,
    /// A generated or file source reached its end.
    Ended,
    /// Stopped by the app.
    Stopped,
    /// Failed; the message says what happened in plain words.
    Failed {
        /// What kind of failure.
        kind: FailureKind,
        /// Plain-language message.
        message: String,
    },
}

/// Counters for reports and the self-check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InputHealth {
    /// Blocks delivered by the source (dropped ones included).
    pub blocks: u64,
    /// Frames delivered by the source (dropped ones included).
    pub frames: u64,
    /// Blocks dropped because the reader was late.
    pub dropped_blocks: u64,
    /// Frames dropped because the reader was late.
    pub dropped_frames: u64,
}

/// An open input: the source, the reading end of the rings and the meters.
/// Owned by the analysis thread.
pub struct InputStream {
    source: Box<dyn InputSource>,
    consumer: CaptureConsumer,
    status: Arc<StreamStatus>,
    format: InputFormat,
    levels: ChannelLevels,
    stopped: bool,
}

impl InputStream {
    /// Open `source`, size the rings for its format and start it.
    pub fn open(mut source: Box<dyn InputSource>) -> Result<Self, InputError> {
        let format = source.open()?;
        if format.channels == 0 || format.sample_rate == 0 {
            source.stop();
            return Err(InputError::new(
                FailureKind::Format,
                format!(
                    "the input reports {} Hz with {} channels",
                    format.sample_rate, format.channels
                ),
            ));
        }
        let ring_frames = ((f64::from(format.sample_rate) * RING_SECONDS).ceil() as usize)
            .max(format.max_block_frames.saturating_mul(8));
        let blocks = (ring_frames / 32).max(256);
        let (producer, consumer, status) = ring::capture_ring(format.channels, ring_frames, blocks);
        if let Err(e) = source.start(producer) {
            source.stop();
            return Err(e);
        }
        Ok(Self {
            levels: ChannelLevels::new(format.channels, format.sample_rate),
            source,
            consumer,
            status,
            format,
            stopped: false,
        })
    }

    /// Open a microphone: the device with this id (from [`list_input_devices`]),
    /// or, with `None`, the automatic choice ([`preferred_input`]). Never opens
    /// a Bluetooth device unless its id is given.
    pub fn open_device(id: Option<&str>) -> Result<Self, InputError> {
        let id = match id {
            Some(id) => id.to_string(),
            None => {
                let devices = list_input_devices()?;
                match preferred_input(&devices) {
                    Some(d) => d.id.clone(),
                    None => {
                        return Err(InputError::new(
                            FailureKind::NoDevice,
                            if devices.is_empty() {
                                "No microphone or audio input is connected. Plug in the mixer \
                                 (or a microphone) and open practice mode again."
                            } else {
                                "The only inputs are Bluetooth or software devices, which \
                                 practice mode never opens by itself. Connect the mixer, or pick \
                                 a device by hand."
                            },
                        ));
                    }
                }
            }
        };
        Self::open(Box::new(DeviceSource::new(id)))
    }

    /// The format the source delivers.
    pub fn format(&self) -> &InputFormat {
        &self.format
    }

    /// Take the next block, if one is waiting: interleaved samples (all
    /// channels) into `out`, the stamp returned. Updates the meters. Reuse
    /// `out` between calls so it stops allocating.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Option<BlockStamp> {
        let stamp = self.consumer.pop_block(out)?;
        self.levels.process(out);
        Some(stamp)
    }

    /// The meters now.
    pub fn levels(&self) -> InputLevels {
        self.levels.snapshot()
    }

    /// The state, including the stall watchdog (a stream that delivers nothing
    /// for [`STALL_TIMEOUT_NS`], or nothing at all within
    /// [`FIRST_BLOCK_TIMEOUT_NS`], is marked failed here).
    pub fn state(&self) -> InputState {
        let now = host_time_ns();
        match self.status.state() {
            RunState::Starting if now - self.status.opened_ns() > FIRST_BLOCK_TIMEOUT_NS => {
                self.status.fail(
                    FailureKind::Stalled,
                    format!(
                        "\"{}\" was opened but sent no sound for {} seconds. Unplug it and plug it \
                         back in, or close other apps that may be using it.",
                        self.format.device_name,
                        FIRST_BLOCK_TIMEOUT_NS / 1_000_000_000
                    ),
                );
            }
            RunState::Running if now - self.status.last_push_ns() > STALL_TIMEOUT_NS => {
                self.status.fail(
                    FailureKind::Stalled,
                    format!(
                        "\"{}\" stopped sending sound. It may have been unplugged or switched off.",
                        self.format.device_name
                    ),
                );
            }
            _ => {}
        }
        match self.status.state() {
            RunState::Starting => InputState::Starting,
            RunState::Running => InputState::Running,
            RunState::Ended => InputState::Ended,
            RunState::Stopped => InputState::Stopped,
            RunState::Failed => {
                let (kind, message) = self
                    .status
                    .failure()
                    .unwrap_or((FailureKind::Device, "the input failed".into()));
                InputState::Failed { kind, message }
            }
        }
    }

    /// Delivery counters.
    pub fn health(&self) -> InputHealth {
        let (blocks, frames) = self.status.delivered();
        let (dropped_blocks, dropped_frames) = self.status.dropped();
        InputHealth {
            blocks,
            frames,
            dropped_blocks,
            dropped_frames,
        }
    }

    /// Stop the source and close the device. Idempotent; also done on drop.
    pub fn stop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            self.source.stop();
            self.status.mark_stopped();
        }
    }
}

impl Drop for InputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Copy channel `ch` of an interleaved block into `out`, exactly (no gain, no
/// summing: a voice on one channel of two arrives at full level, and whatever
/// is on the other channel does not leak in; trap 14).
pub fn extract_channel(interleaved: &[f32], channels: usize, ch: usize, out: &mut Vec<f32>) {
    out.clear();
    let channels = channels.max(1);
    let ch = ch.min(channels - 1);
    out.extend(interleaved.chunks_exact(channels).map(|f| f[ch]));
}

/// The platform microphone, by device id (from [`list_input_devices`]).
pub struct DeviceSource {
    inner: platform::Capture,
}

impl DeviceSource {
    /// A microphone source for the device with this id.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            inner: platform::Capture::new(id.into()),
        }
    }
}

impl InputSource for DeviceSource {
    fn open(&mut self) -> Result<InputFormat, InputError> {
        self.inner.open()
    }
    fn start(&mut self, producer: CaptureProducer) -> Result<(), InputError> {
        self.inner.start(producer)
    }
    fn stop(&mut self) {
        self.inner.stop()
    }
}

/// Windows only, for tests and the probe: the id of the default *output*
/// device. Given to [`DeviceSource`], it captures that output's loopback
/// through exactly the microphone's capture code, while a silent keep-alive
/// (digital zeros, inaudible) keeps packets coming, so the capture path can be
/// proven on a computer with no wired microphone. Practice mode never uses it.
pub fn loopback_test_device_id() -> Option<String> {
    #[cfg(windows)]
    {
        win::default_output_id()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows: input endpoints that exist but cannot be used now, as (name,
/// "unplugged" | "not present" | "disabled"). Empty elsewhere.
pub fn list_inactive_inputs() -> Vec<(String, &'static str)> {
    #[cfg(windows)]
    {
        win::list_inactive_inputs()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Every input device the OS lists (active ones only), in OS order. Opens no
/// stream.
pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>, InputError> {
    platform::list_input_devices()
}

/// Microphone permission, as the OS reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicPermission {
    /// The app may use the microphone.
    Granted,
    /// The user (or Windows' privacy switches) refused. Show
    /// [`mic_denied_steps`].
    Denied,
    /// macOS has not asked yet: show [`MIC_PERMISSION_EXPLANATION`], then call
    /// [`request_mic_permission`].
    NotDetermined,
    /// A device policy (parental controls, management profile) forbids it.
    Restricted,
}

/// The current permission, without showing anything to the user.
/// macOS: `AVCaptureDevice authorizationStatusForMediaType:` (10.14+).
/// Windows: the "Microphone access" and "Let desktop apps access your
/// microphone" privacy switches (read from the registry, read only).
pub fn mic_permission() -> MicPermission {
    platform::mic_permission()
}

/// Windows' microphone privacy switches, read only (never changed): "allow", "deny" or
/// "unset" (absent, which Windows treats as allowed). App core section 12.4
/// `MicPermission.windows`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MicPrivacySwitches {
    /// "Microphone access" for this computer.
    pub microphone_access: &'static str,
    /// "Microphone access" for this user.
    pub user_access: &'static str,
    /// "Let desktop apps access your microphone".
    pub desktop_apps: &'static str,
}

/// The switches on Windows; `None` on other systems.
pub fn mic_privacy_switches() -> Option<MicPrivacySwitches> {
    #[cfg(windows)]
    {
        let word = |v: &Option<String>| match v.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("Deny") => "deny",
            Some(s) if s.eq_ignore_ascii_case("Allow") => "allow",
            _ => "unset",
        };
        let [machine, user, desktop] = win::privacy_switches();
        Some(MicPrivacySwitches {
            microphone_access: word(&machine),
            user_access: word(&user),
            desktop_apps: word(&desktop),
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Ask the OS for microphone access. macOS shows its prompt the first time
/// only (afterwards this returns the stored answer), and the answer arrives
/// asynchronously: this returns at once and the caller polls
/// [`mic_permission`]. Call it only after the app has shown
/// [`MIC_PERMISSION_EXPLANATION`]. Windows asks nothing (returns the switches).
pub fn request_mic_permission() -> MicPermission {
    platform::request_mic_permission()
}

/// What the app says, in its own words, before the OS asks (WO2 section 4.4,
/// "Permission").
pub const MIC_PERMISSION_EXPLANATION: &str = "Practice mode listens to your microphone so it can \
show the note you are singing and grade it. It listens only while the practice screen is open, \
works out the pitch in memory on this computer, and never records, saves or sends your voice. \
Your computer will now ask whether Jukeoke may use the microphone: choose Allow (OK on a Mac).";

/// The text macOS shows in its own permission prompt: the bundle's
/// `NSMicrophoneUsageDescription` (app/src-tauri/Info.plist must carry exactly
/// this; a test checks it).
pub const MACOS_MIC_USAGE_DESCRIPTION: &str = "Jukeoke listens to your microphone only while \
practice mode is open, to show and grade the pitch you sing. The sound is analysed in memory on \
this Mac and is never recorded, saved or sent anywhere.";

/// macOS: opens System Settings > Privacy & Security > Microphone.
pub const MACOS_MICROPHONE_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
/// Windows: opens Settings > Privacy & security > Microphone.
pub const WINDOWS_MICROPHONE_SETTINGS_URI: &str = "ms-settings:privacy-microphone";

/// The exact steps to allow the microphone after a refusal, for this system.
pub fn mic_denied_steps() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS is not letting Jukeoke use the microphone. Open System Settings > Privacy & \
         Security > Microphone and switch on Jukeoke, then open practice mode again (if it still \
         says no microphone, quit and reopen Jukeoke). Each new build of the app may have to be \
         allowed again, because it is signed ad hoc."
    } else {
        "Windows is not letting apps use the microphone. Open Settings > Privacy & security > \
         Microphone and switch on both \"Microphone access\" and \"Let desktop apps access your \
         microphone\", then open practice mode again."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn read_all(stream: &mut InputStream, until: Duration) -> (Vec<f32>, Vec<BlockStamp>) {
        let t = Instant::now();
        let mut all = Vec::new();
        let mut stamps = Vec::new();
        let mut buf = Vec::new();
        while t.elapsed() < until {
            match stream.read(&mut buf) {
                Some(s) => {
                    all.extend_from_slice(&buf);
                    stamps.push(s);
                }
                None => {
                    if matches!(
                        stream.state(),
                        InputState::Ended | InputState::Failed { .. }
                    ) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        (all, stamps)
    }

    #[test]
    fn a_generated_source_arrives_bit_exact_with_contiguous_stamps() {
        let rate = 44_100;
        let voice = signal::VoiceTone::default_a3().render(rate, 1.0);
        let x = signal::place_voice(&voice, rate, 2, 1, signal::OtherChannel::Silence);
        let src = SignalSource::new("test", rate, 2, x.clone()).pacing(Pacing::AsFastAsPossible);
        let mut s = InputStream::open(Box::new(src)).unwrap();
        let (all, stamps) = read_all(&mut s, Duration::from_secs(10));
        assert_eq!(all, x);
        assert_eq!(s.state(), InputState::Ended);
        let mut frame = 0;
        for st in &stamps {
            assert_eq!(st.first_frame, frame);
            assert!(!st.discontinuity());
            frame += u64::from(st.frames);
        }
        // Stamps follow frames / rate.
        let last = stamps.last().unwrap();
        let expect_ns = last.first_frame as i64 * 1_000_000_000 / i64::from(rate);
        assert!((last.host_ns - stamps[0].host_ns - expect_ns).abs() <= 1);
        let mut v = Vec::new();
        extract_channel(&all, 2, 1, &mut v);
        assert_eq!(v, voice);
        assert_eq!(s.health().dropped_blocks, 0);
    }

    #[test]
    fn unplugged_mid_stream_is_a_clean_failure() {
        let rate = 16_000;
        let x = vec![0.1f32; rate as usize / 5];
        let src = SignalSource::new("mixer", rate, 1, x).at_end(SignalEnd::Unplugs);
        let mut s = InputStream::open(Box::new(src)).unwrap();
        let _ = read_all(&mut s, Duration::from_secs(3));
        match s.state() {
            InputState::Failed { kind, .. } => assert_eq!(kind, FailureKind::Unplugged),
            other => panic!("expected a failure, got {other:?}"),
        }
        s.stop();
        // Stopping after a failure keeps the failure visible.
        assert!(matches!(s.state(), InputState::Failed { .. }));
    }

    #[test]
    fn a_device_that_goes_quiet_is_caught_by_the_watchdog() {
        let rate = 16_000;
        let x = vec![0.1f32; rate as usize / 10];
        let src = SignalSource::new("dead driver", rate, 1, x).at_end(SignalEnd::GoesQuiet);
        let mut s = InputStream::open(Box::new(src)).unwrap();
        let t = Instant::now();
        let mut buf = Vec::new();
        let mut failed_after = None;
        while t.elapsed() < Duration::from_secs(4) {
            while s.read(&mut buf).is_some() {}
            if let InputState::Failed { kind, message } = s.state() {
                assert_eq!(kind, FailureKind::Stalled);
                assert!(message.contains("dead driver"));
                failed_after = Some(t.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let after = failed_after.expect("the watchdog did not fire");
        // 0.1 s of sound, then 1 s of nothing.
        assert!(after >= Duration::from_millis(1000) && after < Duration::from_millis(1600));
    }

    #[test]
    fn a_reader_that_falls_behind_loses_whole_blocks_and_sees_a_discontinuity() {
        let rate = 48_000;
        // 3 s in real time; the reader sleeps 2.5 s first, longer than the ring.
        let x: Vec<f32> = (0..rate as usize * 3).map(|i| i as f32).collect();
        let src = SignalSource::new("slow reader", rate, 1, x);
        let mut s = InputStream::open(Box::new(src)).unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        let (all, stamps) = read_all(&mut s, Duration::from_secs(5));
        let h = s.health();
        assert!(h.dropped_blocks > 0, "{h:?}");
        assert!(stamps.iter().any(|st| st.discontinuity()));
        // Every delivered block is intact: its samples equal its frame numbers.
        let mut i = 0;
        for st in &stamps {
            for k in 0..st.frames as usize {
                assert_eq!(all[i + k], (st.first_frame as usize + k) as f32);
            }
            i += st.frames as usize;
        }
    }

    #[test]
    fn permission_texts_are_plain_and_promise_no_recording() {
        for t in [MIC_PERMISSION_EXPLANATION, MACOS_MIC_USAGE_DESCRIPTION] {
            assert!(t.contains("never"));
            assert!(t.contains("practice"));
        }
        assert!(!mic_denied_steps().is_empty());
    }

    #[test]
    fn info_plist_carries_the_microphone_text() {
        // app/src-tauri/Info.plist is merged into the Mac bundle by the Tauri
        // CLI; scripts/build-mac.sh checks the built bundle too.
        let plist =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src-tauri/Info.plist");
        let text = std::fs::read_to_string(&plist).expect("app/src-tauri/Info.plist");
        assert!(text.contains("<key>NSMicrophoneUsageDescription</key>"));
        assert!(text.contains(&format!("<string>{MACOS_MIC_USAGE_DESCRIPTION}</string>")));
    }
}

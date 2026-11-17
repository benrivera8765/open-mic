// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows backend: WASAPI loopback capture of an output device (the default
//! render endpoint, or the one named by `RecorderConfig::device_id`).
//!
//! # What it does
//!
//! 1. Opens the render endpoint's `IAudioClient` in **shared mode with
//!    `AUDCLNT_STREAMFLAGS_LOOPBACK`** at the endpoint's own mix format
//!    (`GetMixFormat`, normally 32-bit float at the device rate). Windows does
//!    no format conversion for this stream and neither do we: the packets are
//!    copied into the file bit for bit (32-bit float; 16/32-bit integer mixes
//!    would be mapped exactly to float, but Windows' shared mix is float).
//! 2. Declares that format to the pipeline at once, so the file exists from
//!    Start even if the system stays silent.
//! 3. Keeps the timeline continuous (WORK_ORDER section 7, trap 5: loopback
//!    delivers no packets while nothing plays):
//!    * (a) a **silent render stream** on the same endpoint runs while
//!      recording. It only ever submits buffers flagged
//!      `AUDCLNT_BUFFERFLAGS_SILENT` (digital zeros), so it adds exactly 0.0 to
//!      the mix and changes nothing audible, but it keeps the audio engine
//!      processing the endpoint, so loopback packets keep arriving;
//!    * (b) every packet carries its **QPC timestamp** (`GetBuffer`'s
//!      `pu64QPCPosition`, 100 ns units) and the pipeline fills any remaining
//!      hole, plus the stretch from Start to the first packet and from the last
//!      packet to Stop, with digital silence of exactly the right length.
//!
//!    Start and Stop are the instants `Recorder::start()` and `stop()` were
//!    called, converted to QPC time. Loopback packet timestamps run about one
//!    engine period (measured 9.8 to 10.6 ms) ahead of the moment a packet can
//!    be read, so both instants are moved onto the packets' timeline by the
//!    same measured lead, and the packet straddling Stop is held back and cut
//!    there: the recording is Stop - Start long (measured within 0.4 ms over
//!    20 s, docs/benchmarks/recorder.md).
//! 4. Watches for the stream dying: device removed or disabled, its format
//!    changed in Sound settings, or another app taking exclusive control. The
//!    pipeline then closes the file (complete and valid) and the state becomes
//!    `Failed` with a message that says which of these happened.
//!
//! # Threads
//!
//! `run_session` runs the capture loop on the recorder's private worker thread
//! (COM multithreaded apartment, MMCSS "Pro Audio"). It wakes on the WASAPI
//! event or every 20 ms, drains all packets, and hands them to the pipeline,
//! which never blocks or (after warm-up) allocates. The keep-alive stream runs
//! on its own small thread. Device queries from other threads
//! (`output_device`, `capture_devices`) run on a short-lived helper thread so
//! the caller's COM apartment is never touched.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasapi::{
    AudioCaptureClient, AudioClient, BufferFlags, BufferInfo, Device, DeviceEnumerator,
    DeviceState, Direction, Handle, SampleType, StreamMode, WasapiError, WaveFormat,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::{PCWSTR, w};

use crate::error::RecorderError;
use crate::format::{CaptureDevice, CaptureFormat, CaptureSetup, OutputDeviceInfo, SourceEncoding};
use crate::pipeline::{CapturePipeline, MediaTime};
use crate::{Backend, PermissionState, RecorderConfig};

pub(crate) const BACKEND: Backend = Backend::WasapiLoopback;

/// Shared-mode buffer asked for on the loopback stream (100 ns units). 200 ms
/// lets the capture thread be late by that much without losing a sample.
const CAPTURE_BUFFER_HNS: i64 = 2_000_000;
/// The keep-alive render stream's buffer (100 ns units), topped up every
/// `KEEPALIVE_POLL`.
const KEEPALIVE_BUFFER_HNS: i64 = 1_000_000;
const KEEPALIVE_POLL: Duration = Duration::from_millis(25);
/// The capture loop waits this long for the WASAPI event before polling anyway
/// (loopback signals nothing while no packets come) and checking for Stop.
const WAIT_MS: u32 = 20;
/// After Stop, keep reading for at most this long until a packet reaches the
/// stop instant (audio captured just before Stop is still in the engine).
const STOP_DRAIN: Duration = Duration::from_millis(100);

/// QPC timestamps in WASAPI are in 100 ns units.
const HNS_PER_SECOND: u32 = 10_000_000;

// HRESULTs we name in messages (audioclient.h).
const AUDCLNT_E_DEVICE_INVALIDATED: u32 = 0x8889_0004;
const AUDCLNT_E_SERVICE_NOT_RUNNING: u32 = 0x8889_0010;
const AUDCLNT_E_RESOURCES_INVALIDATED: u32 = 0x8889_0026;
const E_NOTFOUND: u32 = 0x8007_0490;

// ---------------------------------------------------------------------------
// Permission and system information
// ---------------------------------------------------------------------------

/// Loopback capture needs no permission on Windows.
pub(crate) fn permission_preflight() -> PermissionState {
    PermissionState::NotRequired
}

pub(crate) fn permission_request() -> PermissionState {
    PermissionState::NotRequired
}

/// Run `f` on a short-lived thread with COM initialised (multithreaded
/// apartment), so the caller's thread and its COM apartment are untouched.
fn with_com<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        s.spawn(|| {
            let hr = wasapi::initialize_mta();
            let r = f();
            if hr.is_ok() {
                wasapi::deinitialize();
            }
            r
        })
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// The HRESULT inside a WASAPI error, if it carries one.
fn hresult(e: &WasapiError) -> Option<u32> {
    match e {
        WasapiError::Windows(w) => Some(w.code().0 as u32),
        WasapiError::RegisterNotifications(w) => Some(w.code().0 as u32),
        _ => None,
    }
}

/// A short description of a WASAPI error for messages: the documented name
/// and value of the HRESULTs we expect, else the error text and value.
fn describe(e: &WasapiError) -> String {
    let name = match hresult(e) {
        Some(AUDCLNT_E_DEVICE_INVALIDATED) => Some("AUDCLNT_E_DEVICE_INVALIDATED"),
        Some(AUDCLNT_E_SERVICE_NOT_RUNNING) => Some("AUDCLNT_E_SERVICE_NOT_RUNNING"),
        Some(AUDCLNT_E_RESOURCES_INVALIDATED) => Some("AUDCLNT_E_RESOURCES_INVALIDATED"),
        _ => None,
    };
    match (name, hresult(e)) {
        (Some(n), Some(h)) => format!("{n}, 0x{h:08X}"),
        (None, Some(h)) => format!("{e}, HRESULT 0x{h:08X}"),
        _ => e.to_string(),
    }
}

/// A WASAPI failure as a `RecorderError::Backend`, saying what we were doing.
fn backend_error(doing: &str, e: &WasapiError) -> RecorderError {
    let code = hresult(e);
    let mut message = format!("{doing}: {e}");
    if let Some(h) = code {
        message.push_str(&format!(" (HRESULT 0x{h:08X})"));
    }
    RecorderError::Backend {
        domain: "WASAPI".into(),
        code: code.map_or(-1, i64::from),
        message,
    }
}

pub(crate) fn output_device() -> Option<OutputDeviceInfo> {
    with_com(|| {
        let e = DeviceEnumerator::new().ok()?;
        let d = e.get_default_device(Direction::Render).ok()?;
        device_info(&d).ok()
    })
}

/// Name, mix rate and the hardware format Windows reports for an endpoint.
fn device_info(d: &Device) -> Result<OutputDeviceInfo, WasapiError> {
    let name = d
        .get_friendlyname()
        .unwrap_or_else(|_| "unnamed output device".into());
    let mix = d.get_iaudioclient()?.get_mixformat()?;
    // PKEY_AudioEngine_DeviceFormat: the format the audio engine uses with
    // the hardware (for example 24-bit, 2 ch).
    let physical = d.get_device_format().ok();
    Ok(OutputDeviceInfo {
        name,
        nominal_sample_rate: f64::from(mix.get_samplespersec()),
        physical_bits: physical
            .as_ref()
            .map(|f| u32::from(f.get_validbitspersample())),
        physical_channels: physical.as_ref().map(|f| u32::from(f.get_nchannels())),
    })
}

pub(crate) fn capture_devices() -> Result<Vec<CaptureDevice>, RecorderError> {
    with_com(|| {
        let e = DeviceEnumerator::new().map_err(|e| backend_error("listing output devices", &e))?;
        let default_id = e
            .get_default_device(Direction::Render)
            .and_then(|d| d.get_id())
            .ok();
        let devices = e
            .get_device_collection(Direction::Render)
            .map_err(|e| backend_error("listing output devices", &e))?;
        let mut out = Vec::new();
        for d in &devices {
            let d = d.map_err(|e| backend_error("reading an output device", &e))?;
            let id = d
                .get_id()
                .map_err(|e| backend_error("reading an output device id", &e))?;
            let name = d
                .get_friendlyname()
                .unwrap_or_else(|_| "unnamed output device".into());
            out.push(CaptureDevice {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok(out)
    })
}

/// Read one string or DWORD value under
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` (read only).
fn current_version_value(name: PCWSTR, dword: bool) -> Option<String> {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    if dword {
        let mut v = 0u32;
        let mut size = 4u32;
        // SAFETY: valid key/value names; the output buffer is a u32 with its size.
        let r = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                name,
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut v).cast()),
                Some(&mut size),
            )
        };
        return (r == ERROR_SUCCESS).then(|| v.to_string());
    }
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: as above; `size` is the buffer length in bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// "Windows 11 (build 26200.7171, 25H2)". The product name in the registry
/// still says "Windows 10" on Windows 11, so the name comes from the build
/// number (22000 and later is Windows 11).
pub(crate) fn os_version() -> String {
    let build = current_version_value(w!("CurrentBuildNumber"), false);
    let ubr = current_version_value(w!("UBR"), true);
    let display = current_version_value(w!("DisplayVersion"), false);
    let Some(build) = build else {
        return "Windows (version unknown)".to_string();
    };
    let name = match build.parse::<u32>() {
        Ok(b) if b >= 22_000 => "Windows 11",
        Ok(b) if b >= 10_240 => "Windows 10",
        _ => "Windows",
    };
    let mut s = format!("{name} (build {build}");
    if let Some(u) = ubr {
        s.push('.');
        s.push_str(&u);
    }
    if let Some(d) = display {
        s.push_str(", ");
        s.push_str(&d);
    }
    s.push(')');
    s
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// The current QPC time in 100 ns units: the clock WASAPI uses for
/// `GetBuffer`'s `pu64QPCPosition`, so start/stop instants and packet
/// timestamps can be compared directly.
fn qpc_now() -> MediaTime {
    let mut count = 0i64;
    let mut freq = 0i64;
    // SAFETY: plain Win32 calls with valid out pointers; they cannot fail on
    // Windows XP or later.
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    let hns = i128::from(count) * i128::from(HNS_PER_SECOND) / i128::from(freq.max(1));
    MediaTime {
        value: hns as i64,
        timescale: HNS_PER_SECOND,
    }
}

/// MMCSS "Pro Audio" for the capture thread; reverted when dropped.
struct Mmcss(Option<HANDLE>, String);

impl Mmcss {
    fn join() -> Self {
        let mut task = 0u32;
        // SAFETY: plain Win32 call with a static string and a valid out pointer.
        match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) } {
            Ok(h) => Mmcss(
                Some(h),
                "capture thread runs under MMCSS \"Pro Audio\"".into(),
            ),
            Err(e) => Mmcss(
                None,
                format!("MMCSS was not available for the capture thread: {e}"),
            ),
        }
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            // SAFETY: `h` came from AvSetMmThreadCharacteristicsW on this thread.
            let _ = unsafe { AvRevertMmThreadCharacteristics(h) };
        }
    }
}

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

/// How packet bytes become f32 samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conv {
    /// IEEE float, copied bit for bit.
    F32,
    /// 16-bit PCM, `x / 32768` (exact).
    I16,
    /// 32-bit container PCM, `x / 2^31` (exact for up to 24 significant bits).
    I32,
}

/// The capture format for an endpoint's mix format. Refuses (with a reason)
/// layouts that cannot be stored without conversion.
fn capture_format(mix: &WaveFormat) -> Result<(CaptureFormat, Conv), RecorderError> {
    let sub = mix
        .get_subformat()
        .map_err(|e| backend_error("reading the device mix format", &e))?;
    let bits = mix.get_bitspersample();
    let (conv, source) = match (sub, bits) {
        (SampleType::Float, 32) => (Conv::F32, SourceEncoding::Float32 { interleaved: true }),
        (SampleType::Int, 16) => (Conv::I16, SourceEncoding::Int16 { interleaved: true }),
        (SampleType::Int, 32) => (Conv::I32, SourceEncoding::Int32 { interleaved: true }),
        (st, b) => {
            return Err(RecorderError::Format(format!(
                "the output device's shared mix format is {b}-bit {st:?}, which this recorder does \
                 not store without conversion (expected 32-bit float, 16-bit or 32-bit integer)"
            )));
        }
    };
    let channels = mix.get_nchannels();
    let rate = mix.get_samplespersec();
    if channels == 0 || rate == 0 {
        return Err(RecorderError::Format(format!(
            "the output device reports an invalid mix format ({rate} Hz, {channels} channels)"
        )));
    }
    Ok((
        CaptureFormat {
            sample_rate: rate,
            channels,
            source,
        },
        conv,
    ))
}

/// Convert one packet. `out.len()` samples are produced from `bytes`.
fn convert(conv: Conv, bytes: &[u8], out: &mut [f32]) {
    match conv {
        Conv::F32 => {
            for (o, b) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
                *o = f32::from_le_bytes(*b);
            }
        }
        Conv::I16 => {
            for (o, b) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
                *o = f32::from(i16::from_le_bytes(*b)) / 32_768.0;
            }
        }
        Conv::I32 => {
            for (o, b) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
                *o = i32::from_le_bytes(*b) as f32 / 2_147_483_648.0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The silent keep-alive render stream
// ---------------------------------------------------------------------------

/// A render stream on the recorded endpoint that plays digital silence
/// (buffers flagged `AUDCLNT_BUFFERFLAGS_SILENT`) so the audio engine keeps
/// producing loopback packets while nothing else plays.
struct KeepAlive {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<String>>,
}

impl KeepAlive {
    /// Start it and wait until it runs. Returns a one-line description.
    fn start(device_id: &str) -> Result<(Self, String), String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String, String>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let id = device_id.to_string();
        let thread = std::thread::Builder::new()
            .name("jk-recorder-keepalive".into())
            .spawn(move || keepalive_thread(&id, &ready_tx, &stop_rx))
            .map_err(|e| format!("could not start its thread: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(desc)) => Ok((
                Self {
                    stop: stop_tx,
                    thread: Some(thread),
                },
                desc,
            )),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("it did not start within 5 s".into()), // dropping stop_tx ends it
        }
    }

    /// Stop it; returns its final statistics line.
    fn stop(mut self) -> String {
        let _ = self.stop.send(());
        self.thread
            .take()
            .map(|t| {
                t.join()
                    .unwrap_or_else(|_| "silent keep-alive stream: its thread panicked".into())
            })
            .unwrap_or_default()
    }
}

fn keepalive_thread(
    id: &str,
    ready: &SyncSender<Result<String, String>>,
    stop: &Receiver<()>,
) -> String {
    let com = wasapi::initialize_mta();
    let report = keepalive_run(id, ready, stop);
    if com.is_ok() {
        wasapi::deinitialize();
    }
    report
}

/// Buffer flags for the keep-alive stream: "this buffer is silence".
fn silent_flags() -> Option<BufferFlags> {
    Some(BufferFlags {
        data_discontinuity: false,
        silent: true,
        timestamp_error: false,
    })
}

fn keepalive_run(
    id: &str,
    ready: &SyncSender<Result<String, String>>,
    stop: &Receiver<()>,
) -> String {
    let opened = (|| -> Result<_, WasapiError> {
        let en = DeviceEnumerator::new()?;
        let dev = en.get_device(id)?;
        let mut client = dev.get_iaudioclient()?;
        let mix = client.get_mixformat()?;
        client.initialize_client(
            &mix,
            Direction::Render,
            StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: KEEPALIVE_BUFFER_HNS,
            },
            None,
        )?;
        let render = client.get_audiorenderclient()?;
        let buffer_frames = client.get_buffer_size()?;
        let block = mix.get_blockalign() as usize;
        let zeros = vec![0u8; buffer_frames as usize * block];
        // Fill the whole buffer with silence, then start.
        let avail = client.get_available_space_in_frames()? as usize;
        render.write_to_device(&zeros[..avail * block], silent_flags())?;
        client.start_stream()?;
        let buffer_ms = u64::from(buffer_frames) * 1000 / u64::from(mix.get_samplespersec().max(1));
        Ok((client, render, buffer_frames, block, zeros, buffer_ms))
    })();
    let (client, render, buffer_frames, block, zeros, buffer_ms) = match opened {
        Ok(v) => v,
        Err(e) => {
            let msg = match hresult(&e) {
                Some(h) => format!("{e} (HRESULT 0x{h:08X})"),
                None => e.to_string(),
            };
            let _ = ready.send(Err(msg.clone()));
            return format!("silent keep-alive stream: could not start: {msg}");
        }
    };
    let _ = ready.send(Ok(format!(
        "silent keep-alive stream running on the same device: shared mode, {buffer_ms} ms buffer, \
         every buffer flagged silent (digital zeros, adds exactly 0.0 to the mix)"
    )));
    let mut topups = 0u64;
    let mut ran_empty = 0u64;
    let mut ended = None;
    loop {
        match stop.recv_timeout(KEEPALIVE_POLL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let avail = match client.get_available_space_in_frames() {
            Ok(a) => a,
            Err(e) => {
                // The device went away; the capture loop reports it.
                ended = Some(e.to_string());
                break;
            }
        };
        if avail == buffer_frames {
            ran_empty += 1;
        }
        if avail > 0 {
            if let Err(e) = render.write_to_device(&zeros[..avail as usize * block], silent_flags())
            {
                ended = Some(e.to_string());
                break;
            }
            topups += 1;
        }
    }
    let _ = client.stop_stream();
    let mut s = format!(
        "silent keep-alive stream: {topups} top-ups of digital silence; its buffer ran empty {ran_empty} \
         times (inaudible either way: it only ever holds zeros)"
    );
    if let Some(e) = ended {
        s.push_str(&format!("; it ended early: {e}"));
    }
    s
}

// ---------------------------------------------------------------------------
// The loopback capture
// ---------------------------------------------------------------------------

/// Per-packet facts for the final report (evidence for "no dropouts").
#[derive(Default)]
struct PacketStats {
    packets: u64,
    silent_packets: u64,
    /// `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` after the first packet (the
    /// first one is commonly flagged by Windows and means nothing).
    discontinuities: u64,
    first_flagged_discontinuity: bool,
    timestamp_errors: u64,
    /// Packets whose device position did not continue from the previous one.
    position_jumps: u64,
    next_position: Option<u64>,
    /// End of the previous packet on the QPC clock (100 ns).
    prev_end_hns: Option<i64>,
    /// Largest time between the end of one packet and the start of the next.
    max_gap_hns: i64,
    /// Range of (packet start timestamp - QPC time when it was read): how far
    /// the packet timestamps run ahead of (or behind) the moment they arrive.
    lead_hns: Option<(i64, i64)>,
}

/// A packet waiting in `Capture::held`.
#[derive(Clone, Copy)]
struct Held {
    start: Option<MediaTime>,
    frames: u64,
    silent: bool,
}

struct Capture {
    device_id: String,
    device_name: String,
    format: CaptureFormat,
    conv: Conv,
    block_align: usize,
    client: AudioClient,
    capture: AudioCaptureClient,
    /// `None` if event-driven loopback was refused (then the loop polls).
    event: Option<Handle>,
    bytes: Vec<u8>,
    samples: Vec<f32>,
    stats: PacketStats,
    /// The Start instant (QPC "now" and wall clock), until it has been
    /// declared to the pipeline on the packets' timeline (see `on_packet`).
    pending_start: Option<(MediaTime, Instant)>,
    /// The lead used to move Start onto the packets' timeline; Stop is moved
    /// by the same amount, so the recording is exactly Stop - Start long
    /// whatever error the estimate has.
    timeline_shift: i64,
    /// After Stop: the stop instant on the packets' timeline. Packets are cut
    /// there, so the recording ends at Stop.
    horizon: Option<i64>,
    /// Frames read after Stop that lay beyond the stop instant (not recorded).
    frames_after_stop: u64,
    /// The most recent packet, held back one packet so that, at Stop, the
    /// packet straddling the stop instant can still be cut there. Its samples
    /// are in `held_samples`.
    held: Option<Held>,
    held_samples: Vec<f32>,
}

/// Why the capture loop ended.
enum LoopEnd {
    /// The user stopped the recording.
    Stopped,
    /// The pipeline closed itself (it already holds the error).
    Closed,
    /// WASAPI failed: the stream is gone.
    Lost(WasapiError),
}

impl Capture {
    /// Open the endpoint and initialise the loopback stream (not started).
    fn open(config: &RecorderConfig) -> Result<(Self, CaptureSetup), RecorderError> {
        let en = DeviceEnumerator::new()
            .map_err(|e| backend_error("opening the Windows audio device list", &e))?;
        let device = match config.device_id.as_deref() {
            Some(id) => {
                let d = en.get_device(id).map_err(|e| {
                    let mut err = backend_error("opening the chosen output device", &e);
                    if let RecorderError::Backend { message, .. } = &mut err {
                        message.push_str(&format!(
                            ". The device id {id:?} is not known to Windows (unplugged?)."
                        ));
                    }
                    err
                })?;
                if d.get_direction() != Direction::Render {
                    return Err(RecorderError::Unsupported(format!(
                        "the chosen device {id:?} is an input (recording) device; loopback records \
                         what an output device plays"
                    )));
                }
                d
            }
            None => en.get_default_device(Direction::Render).map_err(|e| {
                if hresult(&e) == Some(E_NOTFOUND) {
                    RecorderError::Unsupported(
                        "Windows has no output (playback) device, so there is no sound to record. \
                         Connect or enable a playback device in Sound settings."
                            .into(),
                    )
                } else {
                    backend_error("opening the default output device", &e)
                }
            })?,
        };
        match device.get_state() {
            Ok(DeviceState::Active) => {}
            Ok(s) => {
                return Err(RecorderError::Unsupported(format!(
                    "the output device is not active ({s:?}); connect or enable it first"
                )));
            }
            Err(e) => return Err(backend_error("reading the output device state", &e)),
        }
        let device_id = device
            .get_id()
            .map_err(|e| backend_error("reading the output device id", &e))?;
        let info =
            device_info(&device).map_err(|e| backend_error("reading the output device", &e))?;
        let device_name = info.name.clone();

        let mut client = device
            .get_iaudioclient()
            .map_err(|e| backend_error("opening the output device", &e))?;
        let mix = client
            .get_mixformat()
            .map_err(|e| backend_error("reading the device mix format", &e))?;
        let (format, conv) = capture_format(&mix)?;

        // Event-driven loopback (Windows 10 and later); fall back to polling.
        let mut notes = Vec::new();
        let events = client.initialize_client(
            &mix,
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: false,
                buffer_duration_hns: CAPTURE_BUFFER_HNS,
            },
            None,
        );
        let event = match events {
            Ok(()) => Some(
                client
                    .set_get_eventhandle()
                    .map_err(|e| backend_error("creating the capture event", &e))?,
            ),
            Err(e) => {
                notes.push(format!(
                    "event-driven loopback was refused ({e}); polling every {WAIT_MS} ms instead"
                ));
                client = device
                    .get_iaudioclient()
                    .map_err(|e| backend_error("opening the output device", &e))?;
                client
                    .initialize_client(
                        &mix,
                        Direction::Capture,
                        StreamMode::PollingShared {
                            autoconvert: false,
                            buffer_duration_hns: CAPTURE_BUFFER_HNS,
                        },
                        None,
                    )
                    .map_err(|e| {
                        backend_error("starting loopback capture on the output device", &e)
                    })?;
                None
            }
        };
        let capture = client
            .get_audiocaptureclient()
            .map_err(|e| backend_error("opening the loopback capture client", &e))?;
        let buffer_frames = client
            .get_buffer_size()
            .map_err(|e| backend_error("reading the capture buffer size", &e))?
            as usize;
        let block_align = mix.get_blockalign() as usize;
        let channels = usize::from(format.channels);

        notes.insert(
            0,
            format!(
                "WASAPI loopback of \"{device_name}\" at its shared-mode mix format ({} Hz, {} ch, {}): \
                 no resampling, no conversion, no processing in this app. This is the system mix of \
                 that device; its format is the quality ceiling.",
                format.sample_rate, format.channels, format.source
            ),
        );
        if let Some(r) = config.sample_rate.filter(|&r| r != format.sample_rate) {
            notes.push(format!(
                "{r} Hz was requested, but loopback can only deliver the device mix rate ({} Hz); \
                 recording at {} Hz rather than resampling",
                format.sample_rate, format.sample_rate
            ));
        }
        if config.channels != format.channels {
            notes.push(format!(
                "recording all {} channels of the device mix ({} were requested)",
                format.channels, config.channels
            ));
        }
        if config.exclude_own_audio {
            notes.push(
                "leaving this app's own sound out is not possible with endpoint loopback; \
                 everything the device plays is recorded"
                    .into(),
            );
        }
        let setup = CaptureSetup {
            backend: BACKEND.name(),
            device: Some(info),
            requested_sample_rate: Some(format.sample_rate),
            requested_channels: format.channels,
            notes,
        };
        let cap = Self {
            device_id,
            device_name,
            format,
            conv,
            block_align,
            client,
            capture,
            event,
            bytes: vec![0u8; buffer_frames * block_align],
            samples: vec![0.0f32; buffer_frames * channels],
            stats: PacketStats::default(),
            pending_start: None,
            timeline_shift: 0,
            horizon: None,
            frames_after_stop: 0,
            held: None,
            held_samples: vec![0.0f32; buffer_frames * channels],
        };
        let _ = mix; // the mix format is fully described by `format`
        Ok((cap, setup))
    }

    /// Wait for the next WASAPI event (or the poll interval).
    fn wait(&self) {
        match &self.event {
            Some(h) => {
                let _ = h.wait_for_event(WAIT_MS);
            }
            None => std::thread::sleep(Duration::from_millis(u64::from(WAIT_MS))),
        }
    }

    /// Hand every packet that is ready to the pipeline.
    fn drain(&mut self, pipeline: &CapturePipeline) -> Result<(), WasapiError> {
        loop {
            let next = self.capture.get_next_packet_size()?.unwrap_or(0) as usize;
            if next == 0 {
                return Ok(());
            }
            if next * self.block_align > self.bytes.len() {
                // Never expected (a packet is at most the buffer); grow once.
                self.bytes.resize(next * self.block_align, 0);
                self.samples
                    .resize(next * usize::from(self.format.channels), 0.0);
                self.held_samples
                    .resize(next * usize::from(self.format.channels), 0.0);
            }
            let (frames, info) = self.capture.read_from_device(&mut self.bytes)?;
            if frames == 0 {
                return Ok(());
            }
            self.on_packet(pipeline, u64::from(frames), &info);
        }
    }

    fn on_packet(&mut self, pipeline: &CapturePipeline, frames: u64, info: &BufferInfo) {
        let st = &mut self.stats;
        st.packets += 1;
        let flags = &info.flags;
        if flags.silent {
            st.silent_packets += 1;
        }
        if flags.data_discontinuity {
            if st.packets == 1 {
                st.first_flagged_discontinuity = true;
            } else {
                st.discontinuities += 1;
            }
        }
        if let Some(expected) = st.next_position
            && info.index != expected
        {
            st.position_jumps += 1;
        }
        st.next_position = Some(info.index + frames);
        let rate = i64::from(self.format.sample_rate);
        let mut lead = 0;
        let start = if flags.timestamp_error {
            st.timestamp_errors += 1;
            None
        } else {
            let t = info.timestamp as i64;
            // Measured on this machine: loopback packet timestamps run about
            // one engine period (10 ms) AHEAD of the moment the packet can be
            // read (docs/benchmarks/recorder.md). Start and Stop are "now"
            // instants, so they are moved onto the packets' timeline by this
            // lead before the pipeline compares them with packet timestamps.
            lead = t - qpc_now().value;
            st.lead_hns = Some(match st.lead_hns {
                Some((lo, hi)) => (lo.min(lead), hi.max(lead)),
                None => (lead, lead),
            });
            if let Some(prev_end) = st.prev_end_hns {
                st.max_gap_hns = st.max_gap_hns.max(t - prev_end);
            }
            st.prev_end_hns = Some(t + frames as i64 * i64::from(HNS_PER_SECOND) / rate);
            Some(MediaTime {
                value: t,
                timescale: HNS_PER_SECOND,
            })
        };
        // Declare Start on the packets' timeline just before the first packet,
        // so the pipeline fills exactly the stretch between Start and it.
        if self.pending_start.is_some() {
            self.timeline_shift = lead;
        }
        if let Some((s, wall)) = self.pending_start.take() {
            pipeline.mark_start_since(
                MediaTime {
                    value: s.value + lead,
                    timescale: HNS_PER_SECOND,
                },
                wall,
            );
        }
        // Convert this packet now, hand the previous one to the pipeline, and
        // hold this one back (see `held`).
        if !flags.silent {
            let n = frames as usize * usize::from(self.format.channels);
            let bytes = frames as usize * self.block_align;
            convert(self.conv, &self.bytes[..bytes], &mut self.samples[..n]);
        }
        self.release(pipeline);
        if !flags.silent {
            std::mem::swap(&mut self.samples, &mut self.held_samples);
        }
        self.held = Some(Held {
            start,
            frames,
            silent: flags.silent,
        });
    }

    /// Hand the held packet to the pipeline. After Stop it is cut at the stop
    /// instant, so the recording ends exactly at Stop.
    fn release(&mut self, pipeline: &CapturePipeline) {
        let Some(h) = self.held.take() else {
            return;
        };
        let mut frames = h.frames;
        if let (Some(horizon), Some(t)) = (self.horizon, h.start) {
            let keep = if t.value >= horizon {
                0
            } else {
                // Frames from the packet start to the stop instant, rounded.
                let rate = i128::from(self.format.sample_rate);
                ((i128::from(horizon - t.value) * rate + 5_000_000) / 10_000_000) as u64
            };
            if keep < frames {
                self.frames_after_stop += frames - keep;
                frames = keep;
            }
            if frames == 0 {
                return;
            }
        }
        if h.silent {
            // Windows says the packet is silence: write digital zeros.
            let _ = pipeline.push_silence(h.start, frames);
        } else {
            let n = frames as usize * usize::from(self.format.channels);
            let _ = pipeline.push(h.start, &self.held_samples[..n]);
        }
    }

    /// The capture loop. Returns when Stop is requested (or the recorder is
    /// dropped), the pipeline closes, or WASAPI fails.
    fn run(&mut self, pipeline: &CapturePipeline, stop: &Receiver<Instant>) -> LoopEnd {
        // When `stop()` was called (the loop notices it up to one wait later).
        let stop_called = loop {
            if pipeline.is_closed() {
                return LoopEnd::Closed;
            }
            self.wait();
            // Check for Stop between the wait and the drain, so packets read
            // after the stop() call go through the cut at the stop instant.
            match stop.try_recv() {
                Ok(at) => break at,
                Err(TryRecvError::Disconnected) => break Instant::now(),
                Err(TryRecvError::Empty) => {}
            }
            if let Err(e) = self.drain(pipeline) {
                // Keep the last good packet.
                self.release(pipeline);
                return LoopEnd::Lost(e);
            }
        };
        // Stop, moved onto the packets' timeline by the same shift as Start.
        // The stop() call instant on the QPC clock (Instant is QPC-based on
        // Windows, so the conversion is exact to a microsecond or so).
        let now = qpc_now().earlier_by(stop_called.elapsed());
        if let Some((s, wall)) = self.pending_start.take() {
            // Not a single packet arrived: the timeline is Start..Stop as is.
            pipeline.mark_start_since(s, wall);
        }
        let horizon = now.value + self.timeline_shift;
        pipeline.mark_stop(MediaTime {
            value: horizon,
            timescale: HNS_PER_SECOND,
        });
        self.horizon = Some(horizon);
        // Audio from just before Stop may still be in the engine: keep reading
        // briefly until a packet reaches the stop instant (one engine period
        // while packets flow; otherwise the pipeline fills the rest).
        let deadline = Instant::now() + STOP_DRAIN;
        while Instant::now() < deadline {
            if self.drain(pipeline).is_err() {
                break;
            }
            if self.stats.prev_end_hns.is_some_and(|end| end >= horizon) {
                break;
            }
            self.wait();
        }
        self.release(pipeline);
        LoopEnd::Stopped
    }

    /// A plain explanation of why the stream died, after checking the device.
    fn diagnose(&self, e: &WasapiError) -> String {
        // "AUDCLNT_E_DEVICE_INVALIDATED, 0x88890004" (or the plain error text).
        let what = describe(e);
        let name = &self.device_name;
        let saved = "Everything recorded up to that moment is saved.";
        let service = matches!(hresult(e), Some(AUDCLNT_E_SERVICE_NOT_RUNNING));
        if service {
            return format!(
                "The Windows audio service stopped during the recording ({what}). {saved}"
            );
        }
        let now = DeviceEnumerator::new()
            .ok()
            .and_then(|en| en.get_device(&self.device_id).ok());
        let state = now.as_ref().and_then(|d| d.get_state().ok());
        if !matches!(state, Some(DeviceState::Active)) {
            return format!(
                "The output device \"{name}\" was removed, unplugged or disabled during the \
                 recording ({what}). {saved}"
            );
        }
        let new_format = now
            .as_ref()
            .and_then(|d| d.get_iaudioclient().ok())
            .and_then(|c| c.get_mixformat().ok())
            .and_then(|m| capture_format(&m).ok())
            .map(|(f, _)| f);
        if let Some(f) = new_format
            && f != self.format
        {
            return format!(
                "The output device \"{name}\" changed its format during the recording, from {} Hz, \
                 {} ch, {} to {} Hz, {} ch, {} (Windows Sound settings, device properties, \
                 Advanced). The recording stopped at that point so the file never mixes two \
                 formats; everything recorded before the change is saved. Start a new recording \
                 to continue.",
                self.format.sample_rate,
                self.format.channels,
                self.format.source,
                f.sample_rate,
                f.channels,
                f.source
            );
        }
        let why = match hresult(e) {
            Some(AUDCLNT_E_DEVICE_INVALIDATED) | Some(AUDCLNT_E_RESOURCES_INVALIDATED) => {
                " This usually means another app took exclusive control of the device, or its \
                 driver restarted."
            }
            _ => "",
        };
        format!("Windows stopped the loopback stream on \"{name}\" ({what}).{why} {saved}")
    }

    /// One line of packet statistics for the report.
    fn report(&self) -> String {
        let s = &self.stats;
        format!(
            "WASAPI delivered {} packets ({} flagged silent); largest gap between packets {:.1} ms; \
             device-position jumps {}; discontinuity flags after the first packet {}{}; \
             timestamp errors {}; packet timestamps minus read time {}; frames read after Stop and cut {}",
            s.packets,
            s.silent_packets,
            s.max_gap_hns as f64 / 10_000.0,
            s.position_jumps,
            s.discontinuities,
            if s.first_flagged_discontinuity {
                " (the first packet was flagged, as Windows usually does)"
            } else {
                ""
            },
            s.timestamp_errors,
            s.lead_hns.map_or("n/a".to_string(), |(lo, hi)| format!(
                "{:.1} to {:.1} ms",
                lo as f64 / 10_000.0,
                hi as f64 / 10_000.0
            )),
            self.frames_after_stop
        )
    }
}

pub(crate) fn run_session(
    config: &RecorderConfig,
    pipeline: Arc<CapturePipeline>,
    ready: SyncSender<Result<CaptureSetup, RecorderError>>,
    stop: Receiver<Instant>,
    requested: Instant,
) -> Result<Vec<String>, RecorderError> {
    // This is the recorder's own thread: COM multithreaded apartment for the
    // whole session, released after every COM object is dropped.
    let com = wasapi::initialize_mta();
    let result = session(config, &pipeline, &ready, &stop, requested);
    if com.is_ok() {
        wasapi::deinitialize();
    }
    result
}

fn session(
    config: &RecorderConfig,
    pipeline: &CapturePipeline,
    ready: &SyncSender<Result<CaptureSetup, RecorderError>>,
    stop: &Receiver<Instant>,
    requested: Instant,
) -> Result<Vec<String>, RecorderError> {
    let (mut cap, mut setup) = match Capture::open(config) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(Vec::new());
        }
    };
    // The format is fixed by the endpoint: declare it now, so the file exists
    // from Start even if nothing ever plays.
    if (cap.format.channels > 2)
        && let Ok(mix) = cap.client.get_mixformat()
    {
        pipeline.set_channel_mask(mix.get_dwchannelmask());
    }
    if let Err(e) = pipeline.set_format(cap.format.clone()) {
        let _ = ready.send(Err(e));
        return Ok(Vec::new());
    }

    // (a) Keep-alive first, so packets flow from the very start.
    let keepalive = if config.silent_keepalive {
        match KeepAlive::start(&cap.device_id) {
            Ok((k, desc)) => {
                setup.notes.push(desc);
                Some(k)
            }
            Err(e) => {
                setup.notes.push(format!(
                    "the silent keep-alive stream could not start ({e}); silence is filled from the \
                     packet timestamps instead"
                ));
                None
            }
        }
    } else {
        setup.notes.push(
            "silent keep-alive stream switched off: Windows may deliver no packets while nothing \
             plays; those stretches are filled from the packet timestamps"
                .into(),
        );
        None
    };

    let mmcss = Mmcss::join();
    setup.notes.push(mmcss.1.clone());
    // Start: the recording covers the time from the Recorder::start() call
    // (the few ms of setup before the stream runs are filled with silence).
    // Declared to the pipeline with the first packet, see `Capture::on_packet`.
    cap.pending_start = Some((qpc_now().earlier_by(requested.elapsed()), requested));
    if let Err(e) = cap.client.start_stream() {
        let err = backend_error("starting the loopback stream", &e);
        if let Some(k) = keepalive {
            k.stop();
        }
        // No audio can have been written; close the (empty) file cleanly.
        pipeline.close_with_error(err.to_string());
        let _ = ready.send(Err(err));
        return Ok(Vec::new());
    }
    let _ = ready.send(Ok(setup));

    let end = cap.run(pipeline, stop);
    let _ = cap.client.stop_stream();
    let mut notes = vec![cap.report()];
    if let LoopEnd::Lost(e) = &end {
        let msg = cap.diagnose(e);
        notes.push(format!("stream lost: {msg}"));
        // Finish the file now and make the state Failed with the message.
        pipeline.close_with_error(msg);
    }
    if let Some(k) = keepalive {
        notes.push(k.stop());
    }
    drop(mmcss);
    if !matches!(end, LoopEnd::Stopped) {
        // Release the device now, then wait for stop() so the worker lives as
        // long as the Recorder (the run_session contract).
        drop(cap);
        let _ = stop.recv();
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_are_exact() {
        let mut out = [0.0f32; 3];
        let f: Vec<u8> = [0.25f32, -1.5, 1e-9]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        convert(Conv::F32, &f, &mut out);
        assert_eq!(out, [0.25, -1.5, 1e-9]);
        let i: Vec<u8> = [i16::MIN, 0, 16_384]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        convert(Conv::I16, &i, &mut out);
        assert_eq!(out, [-1.0, 0.0, 0.5]);
        // 24-bit content in a 32-bit container maps exactly.
        let v: Vec<u8> = [i32::MIN, 0x0000_0100, 0x7FFF_FF00]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        convert(Conv::I32, &v, &mut out);
        assert_eq!(out[0], -1.0);
        assert_eq!(out[1], 1.0 / 8_388_608.0);
        assert_eq!(f64::from(out[2]), (0x7FFF_FF00u32 as f64) / 2_147_483_648.0);
    }

    #[test]
    fn qpc_clock_is_100ns_and_monotonic() {
        let a = qpc_now();
        std::thread::sleep(Duration::from_millis(20));
        let b = qpc_now();
        assert_eq!(a.timescale, HNS_PER_SECOND);
        let d = (b.value - a.value) as f64 / 1e7;
        assert!((0.015..0.5).contains(&d), "{d}");
    }

    #[test]
    fn os_version_names_windows() {
        let v = os_version();
        assert!(v.starts_with("Windows"), "{v}");
    }
}

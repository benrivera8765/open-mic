// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS microphone input: Core Audio HAL listing, cpal 0.16 capture,
//! `AVCaptureDevice` permission. TYPE-CHECKED ON WINDOWS ONLY until a MacBook
//! report comes back (WO2 section 3: "built but not verified").
//!
//! * **Listing** (HAL, every property since macOS 10.x): each device with input
//!   channels (`kAudioDevicePropertyStreamConfiguration`, input scope), its UID,
//!   name, manufacturer, transport (`kAudioDevicePropertyTransportType`: USB,
//!   built-in, Bluetooth, ...), nominal rate and declared input latency
//!   (device latency + safety offset + first input stream's latency).
//! * **Capture**: cpal 0.16 (pinned; it runs on macOS 13, D-37). cpal opens the
//!   device with the HAL output unit (`kAudioUnitSubType_HALOutput`), not Apple's
//!   voice-processing unit, which on macOS 13 would lower and reshape the app's
//!   own playback (WO2 4.8). The device keeps the rate and channel count it runs
//!   at (cpal's `default_input_config` is the device's current nominal format,
//!   so cpal does not change the device rate). cpal's `Stream` is not `Send`, so
//!   it lives on a private thread for its whole life. Devices are taken from
//!   cpal's enumeration (not its "default device" object), so cpal installs its
//!   `kAudioDevicePropertyDeviceIsAlive` listener and an unplugged mixer ends as
//!   `Unplugged`; the stall watchdog covers anything else.
//! * **Time stamps**: the input callback's `AudioTimeStamp.mHostTime` (the
//!   capture time of the buffer's first frame on the host clock), which cpal
//!   hands over as `InputCallbackInfo::timestamp().callback` already scaled to
//!   nanoseconds ([`super::clock::stream_instant_ns`]). cpal's own `capture`
//!   field subtracts one more buffer as a guess ("TODO" in cpal), so it is not
//!   used.
//! * **Permission**: `+[AVCaptureDevice authorizationStatusForMediaType:]` and
//!   `requestAccessForMediaType:completionHandler:` (macOS 10.14). macOS hands
//!   denied apps silence instead of an error, so opening a device first checks
//!   the permission and refuses with the steps when it is missing.

use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{NonNull, null};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use objc2::msg_send;
use objc2::runtime::{AnyClass, Bool};
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyStreams,
    kAudioDevicePropertyTransportType, kAudioDeviceTransportTypeAVB,
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeAirPlay,
    kAudioDeviceTransportTypeAutoAggregate, kAudioDeviceTransportTypeBluetooth,
    kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeBuiltIn,
    kAudioDeviceTransportTypeContinuityCaptureWired,
    kAudioDeviceTransportTypeContinuityCaptureWireless, kAudioDeviceTransportTypeDisplayPort,
    kAudioDeviceTransportTypeFireWire, kAudioDeviceTransportTypeHDMI, kAudioDeviceTransportTypePCI,
    kAudioDeviceTransportTypeThunderbolt, kAudioDeviceTransportTypeUSB,
    kAudioDeviceTransportTypeVirtual, kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyManufacturer, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, kAudioObjectUnknown,
    kAudioStreamPropertyLatency,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_foundation::NSString;

use super::InputError;
use super::clock::{host_time_ns, stream_instant_ns};
use super::device::{InputDeviceInfo, Transport, name_says_hands_free};
use super::ring::{CaptureProducer, FLAG_TIMESTAMP_ESTIMATED, FailureKind};
use super::{InputFormat, MicPermission, mic_denied_steps};

// ---------------------------------------------------------------------------
// HAL property helpers
// ---------------------------------------------------------------------------

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size property of the documented plain-old-data type `T`.
unsafe fn get<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let addr = address(selector, scope);
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `addr`, `size` and `value` are valid for the call; `value` has
    // room for `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast::<c_void>()),
        )
    };
    if status == 0 && size as usize == size_of::<T>() {
        // SAFETY: the HAL filled exactly size_of::<T>() bytes.
        Some(unsafe { value.assume_init() })
    } else {
        None
    }
}

/// Read a variable-size property into 8-byte-aligned storage. Returns the
/// storage and the number of bytes the HAL filled.
unsafe fn get_bytes(object: AudioObjectID, selector: u32, scope: u32) -> Option<(Vec<u64>, usize)> {
    let addr = address(selector, scope);
    let mut size: u32 = 0;
    // SAFETY: valid pointers for the duration of the calls; the buffer has at
    // least the size the HAL asked for.
    unsafe {
        if AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
        ) != 0
            || size == 0
        {
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let status = AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(buf.as_mut_ptr().cast::<c_void>()),
        );
        (status == 0).then_some((buf, size as usize))
    }
}

/// An array property of `T` (device lists, stream lists).
unsafe fn get_array<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Vec<T> {
    // SAFETY: forwarded; the storage is 8-byte aligned, enough for u32 ids.
    let Some((buf, bytes)) = (unsafe { get_bytes(object, selector, scope) }) else {
        return Vec::new();
    };
    let n = bytes / size_of::<T>();
    let p = buf.as_ptr().cast::<T>();
    // SAFETY: the HAL wrote `n` values of T into `buf`.
    (0..n).map(|i| unsafe { *p.add(i) }).collect()
}

/// A CFString property (the caller owns the returned reference).
unsafe fn get_string(object: AudioObjectID, selector: u32, scope: u32) -> Option<String> {
    // SAFETY: the property's documented type is CFStringRef, +1 retained.
    unsafe { get::<*const CFString>(object, selector, scope) }
        .and_then(|p| NonNull::new(p.cast_mut()))
        // SAFETY: we own this +1 reference and release it on drop.
        .map(|p| unsafe { CFRetained::from_raw(p) }.to_string())
}

/// Input channels: the sum over the input stream configuration's buffers.
unsafe fn input_channels(device: AudioObjectID) -> u32 {
    // SAFETY: AudioBufferList is a u32 count followed by AudioBuffers; the
    // storage is 8-byte aligned and holds what the HAL wrote.
    unsafe {
        let Some((buf, bytes)) = get_bytes(
            device,
            kAudioDevicePropertyStreamConfiguration,
            kAudioObjectPropertyScopeInput,
        ) else {
            return 0;
        };
        if bytes < size_of::<u32>() {
            return 0;
        }
        let list = buf.as_ptr().cast::<AudioBufferList>();
        let n = (*list).mNumberBuffers as usize;
        let first = std::ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>();
        let header = (first as usize) - (list as usize);
        let fits = (bytes.saturating_sub(header)) / size_of::<AudioBuffer>();
        (0..n.min(fits))
            .map(|i| (*first.add(i)).mNumberChannels)
            .sum()
    }
}

fn transport_of(code: u32) -> Transport {
    #[allow(non_upper_case_globals)]
    match code {
        kAudioDeviceTransportTypeUSB => Transport::Usb,
        kAudioDeviceTransportTypeBuiltIn => Transport::BuiltIn,
        kAudioDeviceTransportTypeFireWire
        | kAudioDeviceTransportTypePCI
        | kAudioDeviceTransportTypeThunderbolt
        | kAudioDeviceTransportTypeAVB => Transport::WiredExternal,
        kAudioDeviceTransportTypeBluetooth | kAudioDeviceTransportTypeBluetoothLE => {
            Transport::Bluetooth
        }
        kAudioDeviceTransportTypeHDMI | kAudioDeviceTransportTypeDisplayPort => Transport::Display,
        kAudioDeviceTransportTypeAggregate
        | kAudioDeviceTransportTypeAutoAggregate
        | kAudioDeviceTransportTypeVirtual
        | kAudioDeviceTransportTypeAirPlay
        | kAudioDeviceTransportTypeContinuityCaptureWired
        | kAudioDeviceTransportTypeContinuityCaptureWireless => Transport::Virtual,
        _ => Transport::Unknown,
    }
}

/// A four-character code as text ("usb ", "bltn", "blue").
fn fourcc(code: u32) -> String {
    let b = code.to_be_bytes();
    if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        format!("0x{code:08x}")
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

struct HalDevice {
    id: AudioObjectID,
    info: InputDeviceInfo,
}

fn hal_input_devices() -> Vec<HalDevice> {
    let system = kAudioObjectSystemObject as AudioObjectID;
    // SAFETY: every call passes a property with its documented type.
    unsafe {
        let default: AudioObjectID = get(
            system,
            kAudioHardwarePropertyDefaultInputDevice,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(kAudioObjectUnknown);
        let ids: Vec<AudioObjectID> = get_array(
            system,
            kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal,
        );
        let mut out = Vec::new();
        for id in ids {
            let channels = input_channels(id);
            if channels == 0 {
                continue;
            }
            let g = kAudioObjectPropertyScopeGlobal;
            let i = kAudioObjectPropertyScopeInput;
            let name = get_string(id, kAudioObjectPropertyName, g)
                .unwrap_or_else(|| format!("input device {id}"));
            let uid =
                get_string(id, kAudioDevicePropertyDeviceUID, g).unwrap_or_else(|| id.to_string());
            let transport_code: u32 = get(id, kAudioDevicePropertyTransportType, g).unwrap_or(0);
            let mut transport = transport_of(transport_code);
            let mut hands_free = transport == Transport::Bluetooth;
            if name_says_hands_free(&name) {
                hands_free = true;
                if transport == Transport::Unknown {
                    transport = Transport::Bluetooth;
                }
            }
            let rate: f64 = get(id, kAudioDevicePropertyNominalSampleRate, g).unwrap_or(0.0);
            let latency = get::<u32>(id, kAudioDevicePropertyLatency, i);
            let safety = get::<u32>(id, kAudioDevicePropertySafetyOffset, i);
            let stream_latency = get_array::<AudioObjectID>(id, kAudioDevicePropertyStreams, i)
                .first()
                .and_then(|&s| get::<u32>(s, kAudioStreamPropertyLatency, g));
            let declared = match (latency, safety, stream_latency) {
                (None, None, None) => None,
                (a, b, c) => Some(a.unwrap_or(0) + b.unwrap_or(0) + c.unwrap_or(0)),
            };
            out.push(HalDevice {
                id,
                info: InputDeviceInfo {
                    id: uid,
                    name,
                    adapter: get_string(id, kAudioObjectPropertyManufacturer, g),
                    transport,
                    form_factor: None,
                    bus: Some(fourcc(transport_code)),
                    channels: channels.min(u32::from(u16::MAX)) as u16,
                    sample_rate: rate.round() as u32,
                    sample_format: Some("f32".into()),
                    is_default: id == default,
                    hands_free,
                    declared_latency_frames: declared,
                },
            });
        }
        out
    }
}

pub(super) fn list_input_devices() -> Result<Vec<InputDeviceInfo>, InputError> {
    Ok(hal_input_devices().into_iter().map(|d| d.info).collect())
}

// ---------------------------------------------------------------------------
// Permission (AVFoundation, macOS 10.14+)
// ---------------------------------------------------------------------------

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    /// `AVMediaTypeAudio` (AVMediaFormat.h, macOS 10.7).
    static AVMediaTypeAudio: &'static NSString;
}

fn capture_device_class() -> Option<&'static AnyClass> {
    AnyClass::get(c"AVCaptureDevice")
}

pub(super) fn mic_permission() -> MicPermission {
    let Some(cls) = capture_device_class() else {
        return MicPermission::Denied;
    };
    // SAFETY: documented class method taking an AVMediaType and returning
    // AVAuthorizationStatus (NSInteger).
    let status: isize =
        unsafe { msg_send![cls, authorizationStatusForMediaType: AVMediaTypeAudio] };
    match status {
        0 => MicPermission::NotDetermined,
        1 => MicPermission::Restricted,
        3 => MicPermission::Granted,
        _ => MicPermission::Denied,
    }
}

pub(super) fn request_mic_permission() -> MicPermission {
    let now = mic_permission();
    if now != MicPermission::NotDetermined {
        return now;
    }
    let Some(cls) = capture_device_class() else {
        return MicPermission::Denied;
    };
    // The answer arrives on an arbitrary queue; the app polls mic_permission().
    let handler = block2::RcBlock::new(|_granted: Bool| {});
    // SAFETY: documented class method; the block is copied by AVFoundation.
    let _: () = unsafe {
        msg_send![cls, requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*handler]
    };
    MicPermission::NotDetermined
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

pub(super) struct Capture {
    uid: String,
    stop: Arc<AtomicBool>,
    start_tx: Option<SyncSender<CaptureProducer>>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub(super) fn new(uid: String) -> Self {
        Self {
            uid,
            stop: Arc::new(AtomicBool::new(false)),
            start_tx: None,
            thread: None,
        }
    }

    pub(super) fn open(&mut self) -> Result<InputFormat, InputError> {
        match mic_permission() {
            MicPermission::Granted => {}
            MicPermission::NotDetermined => {
                return Err(InputError::new(
                    FailureKind::PermissionDenied,
                    "Open Mic has not asked macOS for the microphone yet. Show the explanation, \
                     then ask (request_mic_permission).",
                ));
            }
            MicPermission::Denied | MicPermission::Restricted => {
                return Err(InputError::new(
                    FailureKind::PermissionDenied,
                    mic_denied_steps(),
                ));
            }
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (start_tx, start_rx) = mpsc::sync_channel(1);
        let uid = self.uid.clone();
        let stop = self.stop.clone();
        let thread = std::thread::Builder::new()
            .name("jk-input-coreaudio".into())
            .spawn(move || capture_thread(&uid, &ready_tx, &start_rx, &stop))
            .map_err(|e| {
                InputError::new(
                    FailureKind::Device,
                    format!("could not start a thread: {e}"),
                )
            })?;
        self.thread = Some(thread);
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(format)) => {
                self.start_tx = Some(start_tx);
                Ok(format)
            }
            Ok(Err(e)) => {
                self.stop();
                Err(e)
            }
            Err(_) => {
                self.stop();
                Err(InputError::new(
                    FailureKind::Stalled,
                    "the input device did not open within 10 seconds",
                ))
            }
        }
    }

    pub(super) fn start(&mut self, producer: CaptureProducer) -> Result<(), InputError> {
        let tx = self.start_tx.take().ok_or_else(InputError::stopped)?;
        tx.send(producer).map_err(|_| InputError::stopped())
    }

    pub(super) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.start_tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The cpal device for a HAL device: the first cpal input device with the same
/// name and channel count (names are unique in practice; the channel count
/// separates the rare duplicates).
fn find_cpal_device(
    name: &str,
    channels: u16,
) -> Option<(cpal::Device, cpal::SupportedStreamConfig)> {
    let host = cpal::default_host();
    let mut fallback = None;
    for d in host.input_devices().ok()? {
        if d.name().ok().as_deref() != Some(name) {
            continue;
        }
        let Ok(cfg) = d.default_input_config() else {
            continue;
        };
        if cfg.channels() == channels {
            return Some((d, cfg));
        }
        if fallback.is_none() {
            fallback = Some((d, cfg));
        }
    }
    fallback
}

fn capture_thread(
    uid: &str,
    ready: &SyncSender<Result<InputFormat, InputError>>,
    start: &Receiver<CaptureProducer>,
    stop: &AtomicBool,
) {
    let Some(hal) = hal_input_devices().into_iter().find(|d| d.info.id == uid) else {
        let _ = ready.send(Err(InputError::new(
            FailureKind::Unplugged,
            "The chosen input device is not connected. Plug it in, or pick another one.",
        )));
        return;
    };
    let name = hal.info.name.clone();
    let Some((device, cfg)) = find_cpal_device(&name, hal.info.channels) else {
        let _ = ready.send(Err(InputError::new(
            FailureKind::Device,
            format!("\"{name}\" is listed by macOS but could not be opened."),
        )));
        return;
    };
    let sample_format = cfg.sample_format();
    let format_name = match sample_format {
        cpal::SampleFormat::F32 => "f32",
        cpal::SampleFormat::I16 => "i16",
        cpal::SampleFormat::I32 => "i32",
        other => {
            let _ = ready.send(Err(InputError::new(
                FailureKind::Format,
                format!("\"{name}\" delivers {other} samples, which practice mode does not read"),
            )));
            return;
        }
    };
    let rate = cfg.sample_rate().0;
    let channels = cfg.channels();
    let config: cpal::StreamConfig = cfg.config();
    let latency_ns = hal
        .info
        .declared_latency_frames
        .filter(|_| rate > 0)
        .map(|f| i64::from(f) * 1_000_000_000 / i64::from(rate));
    let _ = hal.id;
    let format = InputFormat {
        source: "Core Audio HAL output unit via cpal 0.16 (no voice processing)".into(),
        device_name: name.clone(),
        sample_rate: rate,
        channels,
        sample_format: format_name.into(),
        // cpal does not report the device buffer before the stream runs; the
        // HAL's maximum is far below one second, which sizes the rings safely.
        max_block_frames: (rate as usize / 2).max(4096),
        declared_latency_ns: latency_ns,
        notes: Vec::new(),
    };
    let _ = ready.send(Ok(format));
    let Ok(mut producer) = start.recv() else {
        return;
    };
    let status = producer.status();
    let err_status = status.clone();
    let err_name = name.clone();
    let ch = usize::from(channels.max(1));
    // The capture callback: copy (with an exact integer mapping) and stamp.
    let data_cb = move |data: &cpal::Data, info: &cpal::InputCallbackInfo| {
        let (host_ns, flags) = match stream_instant_ns(&info.timestamp().callback) {
            Some(t) => (t, 0),
            None => (host_time_ns(), FLAG_TIMESTAMP_ESTIMATED),
        };
        let frames = data.len() / ch;
        match data.sample_format() {
            cpal::SampleFormat::F32 => {
                if let Some(s) = data.as_slice::<f32>() {
                    producer.push_interleaved(s, host_ns, flags);
                }
            }
            cpal::SampleFormat::I16 => {
                if let Some(s) = data.as_slice::<i16>() {
                    producer.push_iter(
                        frames,
                        s.iter().map(|&x| f32::from(x) / 32_768.0),
                        host_ns,
                        flags,
                    );
                }
            }
            cpal::SampleFormat::I32 => {
                if let Some(s) = data.as_slice::<i32>() {
                    producer.push_iter(
                        frames,
                        s.iter().map(|&x| x as f32 / 2_147_483_648.0),
                        host_ns,
                        flags,
                    );
                }
            }
            _ => {}
        }
    };
    // Not real-time: cpal calls this from its own thread.
    let err_cb = move |e: cpal::StreamError| match e {
        cpal::StreamError::DeviceNotAvailable => err_status.fail(
            FailureKind::Unplugged,
            format!(
                "\"{err_name}\" was unplugged or switched off. Plug it back in and open practice \
                 mode again."
            ),
        ),
        other => err_status.fail(FailureKind::Device, format!("\"{err_name}\": {other}")),
    };
    let stream = match device.build_input_stream_raw(&config, sample_format, data_cb, err_cb, None)
    {
        Ok(s) => s,
        Err(e) => {
            status.fail(
                FailureKind::Device,
                format!("opening \"{name}\" failed: {e}"),
            );
            return;
        }
    };
    if let Err(e) = stream.play() {
        status.fail(
            FailureKind::Device,
            format!("starting \"{name}\" failed: {e}"),
        );
        return;
    }
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
}

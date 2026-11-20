// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows microphone input: WASAPI shared-mode capture of an input endpoint.
//!
//! * **Listing** reads each active capture endpoint's property store: friendly
//!   name, adapter name, form factor (`PKEY_AudioEndpoint_FormFactor`) and the
//!   bus enumerator (`PKEY_Device_EnumeratorName`: "USB", "HDAUDIO",
//!   "BTHHFENUM", ...; when an endpoint lacks it, the id of the adapter it is
//!   connected to, from its device topology). The format is the shared-mode
//!   mix format; for a Bluetooth endpoint it is read from the property store
//!   (`PKEY_AudioEngine_DeviceFormat`) so that no audio client is ever
//!   activated on it (trap 2).
//! * **Capture** runs on a private thread (COM multithreaded apartment, MMCSS
//!   "Pro Audio"): event-driven shared mode at the mix format, the stream
//!   category "Other" (never "Communications", which makes Windows duck other
//!   sounds) and the raw option (`AUDCLNT_STREAMOPTIONS_RAW`: no Windows voice
//!   processing such as automatic gain or noise suppression, trap 7) when the
//!   endpoint accepts it. Each packet's capture time comes from `GetBuffer`'s
//!   QPC position. The loop only converts the packet's bytes to f32 straight
//!   into the ring (an exact mapping for integer formats) and stamps it.
//! * **Failures**: `AUDCLNT_E_DEVICE_INVALIDATED` (unplugged, disabled) and
//!   "not found" end as `Unplugged`; `E_ACCESSDENIED` (Windows' microphone
//!   privacy switches) as `PermissionDenied` with the steps; exclusive use by
//!   another app as `Device`. The thread then stops the stream and exits.
//! * **Test path**: given a *render* endpoint id, the same code captures that
//!   endpoint's loopback (WASAPI sets the loopback flag) while a silent
//!   keep-alive stream (buffers flagged silent: digital zeros, inaudible) keeps
//!   packets coming. This proves the capture path on a PC with no wired
//!   microphone; practice mode never uses it.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use wasapi::{
    AudioCaptureClient, AudioClient, AudioClientProperties, AudioRenderClient, BufferFlags,
    BufferInfo, Device, DeviceEnumerator, DeviceState, Direction, Handle, Role, SampleType,
    StreamCategory, StreamMode, StreamOption, WasapiError, WaveFormat,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, PROPERTYKEY};
use windows::Win32::Media::Audio::IDeviceTopology;
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::{GUID, HSTRING, PCWSTR, PWSTR, w};

use super::InputError;
use super::clock::{host_time_ns, qpc_100ns_to_ns};
use super::device::{InputDeviceInfo, Transport, name_says_hands_free};
use super::ring::{
    CaptureProducer, FLAG_DISCONTINUITY, FLAG_SILENT, FLAG_TIMESTAMP_ESTIMATED, FailureKind,
    StreamStatus,
};
use super::{InputFormat, MicPermission, mic_denied_steps};

/// Shared-mode buffer (100 ns units): how late the capture thread may be.
const CAPTURE_BUFFER_HNS: i64 = 1_000_000; // 100 ms
/// The loop waits this long for the WASAPI event before checking again.
const WAIT_MS: u32 = 50;

// HRESULTs named in messages (winerror.h, audioclient.h).
const E_ACCESSDENIED: u32 = 0x8007_0005;
const E_NOTFOUND: u32 = 0x8007_0490;
const AUDCLNT_E_DEVICE_INVALIDATED: u32 = 0x8889_0004;
const AUDCLNT_E_DEVICE_IN_USE: u32 = 0x8889_000A;
const AUDCLNT_E_SERVICE_NOT_RUNNING: u32 = 0x8889_0010;

/// PKEY_AudioEndpoint_FormFactor (mmdeviceapi.h): VT_UI4, an EndpointFormFactor.
const PKEY_AUDIOENDPOINT_FORMFACTOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e),
    pid: 0,
};
/// PKEY_Device_EnumeratorName (functiondiscoverykeys_devpkey.h): the bus enumerator.
const PKEY_DEVICE_ENUMERATORNAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 24,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `f` on a short-lived thread with COM initialised (multithreaded
/// apartment), so the caller's COM apartment is untouched.
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

fn hresult(e: &WasapiError) -> Option<u32> {
    match e {
        WasapiError::Windows(w) => Some(w.code().0 as u32),
        WasapiError::RegisterNotifications(w) => Some(w.code().0 as u32),
        _ => None,
    }
}

/// A WASAPI failure as a stream failure with a plain message.
fn classify_error(device: &str, doing: &str, e: &WasapiError) -> (FailureKind, String) {
    match hresult(e) {
        Some(AUDCLNT_E_DEVICE_INVALIDATED) | Some(E_NOTFOUND) => (
            FailureKind::Unplugged,
            format!(
                "\"{device}\" was unplugged, switched off or disabled. Plug it back in and open \
                 practice mode again."
            ),
        ),
        Some(E_ACCESSDENIED) => (
            FailureKind::PermissionDenied,
            mic_denied_steps().to_string(),
        ),
        Some(AUDCLNT_E_DEVICE_IN_USE) => (
            FailureKind::Device,
            format!(
                "Another app is using \"{device}\" on its own (exclusive mode). Close that app, or \
                 untick \"Allow applications to take exclusive control\" for this device."
            ),
        ),
        Some(AUDCLNT_E_SERVICE_NOT_RUNNING) => (
            FailureKind::Device,
            "The Windows Audio service is not running.".to_string(),
        ),
        Some(h) => (
            FailureKind::Device,
            format!("{doing} \"{device}\" failed: {e} (HRESULT 0x{h:08X})"),
        ),
        None => (
            FailureKind::Device,
            format!("{doing} \"{device}\" failed: {e}"),
        ),
    }
}

fn input_error(kind: FailureKind, message: String) -> InputError {
    InputError::new(kind, message)
}

/// Read one property from an endpoint's property store.
fn read_prop<T>(
    d: &Device,
    key: &PROPERTYKEY,
    parse: impl FnOnce(&windows::Win32::System::Com::StructuredStorage::PROPVARIANT) -> Option<T>,
) -> Option<T> {
    // SAFETY: COM calls on a live IMMDevice; the PROPVARIANT is cleared after use.
    unsafe {
        let store = d.as_immdevice().OpenPropertyStore(STGM_READ).ok()?;
        let mut v = store.GetValue(key).ok()?;
        let r = parse(&v);
        let _ = PropVariantClear(&mut v);
        r
    }
}

fn prop_u32(d: &Device, key: &PROPERTYKEY) -> Option<u32> {
    // SAFETY: PropVariantToUInt32 only reads the variant (fails on a wrong type).
    read_prop(d, key, |v| unsafe { PropVariantToUInt32(v).ok() })
}

fn take_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: a CoTaskMemAlloc'ed, NUL-terminated string from the API; freed here.
    unsafe {
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn prop_string(d: &Device, key: &PROPERTYKEY) -> Option<String> {
    // SAFETY: PropVariantToStringAlloc allocates a copy that take_pwstr frees.
    read_prop(d, key, |v| {
        unsafe { PropVariantToStringAlloc(v).ok() }.and_then(take_pwstr)
    })
    .filter(|s| !s.is_empty())
}

/// The id of the adapter device an endpoint is connected to, from its device
/// topology (for example `{2}.\\?\usb#vid_...` or `{2}.\\?\hdaudio#func_...`).
fn topology_adapter_id(d: &Device) -> Option<String> {
    // SAFETY: COM calls on live interfaces; the string is freed by take_pwstr.
    unsafe {
        let topo: IDeviceTopology = d.as_immdevice().Activate(CLSCTX_ALL, None).ok()?;
        let connector = topo.GetConnector(0).ok()?;
        take_pwstr(connector.GetDeviceIdConnectedTo().ok()?)
    }
}

fn form_factor_name(v: u32) -> &'static str {
    match v {
        0 => "remote_network_device",
        1 => "speakers",
        2 => "line_level",
        3 => "headphones",
        4 => "microphone",
        5 => "headset",
        6 => "handset",
        7 => "digital_passthrough",
        8 => "spdif",
        9 => "display_audio",
        _ => "unknown",
    }
}

/// Transport and "hands-free" from the enumerator name, the adapter id from
/// the topology, the endpoint name and form factor. Pure; unit-tested.
pub(super) fn classify(
    enumerator: Option<&str>,
    adapter_id: Option<&str>,
    name: &str,
    form_factor: Option<u32>,
) -> (Transport, bool) {
    // The bus: the enumerator name, else the start of the adapter id after
    // the "{2}.\\?\" prefix ("usb#vid_..." -> "USB").
    let bus = enumerator.map(|e| e.to_ascii_uppercase()).or_else(|| {
        adapter_id.map(|id| {
            let s = id.find("?\\").map_or(id, |i| &id[i + 2..]);
            s.split(['#', '\\'])
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
        })
    });
    let bus = bus.unwrap_or_default();
    let mut transport = if bus.starts_with("BTH") {
        Transport::Bluetooth
    } else if bus == "USB" {
        Transport::Usb
    } else if bus == "HDAUDIO" || bus == "INTELAUDIO" || bus == "ACPI" || bus == "AMDAUDIO" {
        if form_factor == Some(9) {
            Transport::Display
        } else {
            Transport::BuiltIn
        }
    } else if bus == "PCI" || bus == "1394" || bus == "THUNDERBOLT" {
        Transport::WiredExternal
    } else if bus == "ROOT" {
        Transport::Virtual
    } else {
        Transport::Unknown
    };
    let mut hands_free = transport == Transport::Bluetooth;
    if name_says_hands_free(name) {
        hands_free = true;
        if transport == Transport::Unknown {
            transport = Transport::Bluetooth;
        }
    }
    (transport, hands_free)
}

fn sample_format_name(f: &WaveFormat) -> Option<String> {
    let sub = f.get_subformat().ok()?;
    let bits = f.get_bitspersample();
    let valid = f.get_validbitspersample();
    Some(match (sub, bits, valid) {
        (SampleType::Float, 32, _) => "f32".into(),
        (SampleType::Int, 16, _) => "i16".into(),
        (SampleType::Int, 24, _) => "i24".into(),
        (SampleType::Int, 32, 24) => "i24in32".into(),
        (SampleType::Int, 32, _) => "i32".into(),
        (st, b, _) => format!("{st:?}{b}"),
    })
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

pub(super) fn list_input_devices() -> Result<Vec<InputDeviceInfo>, InputError> {
    with_com(|| {
        let en = DeviceEnumerator::new().map_err(|e| list_error(&e))?;
        let default_id = en
            .get_default_device_for_role(Direction::Capture, Role::Console)
            .and_then(|d| d.get_id())
            .ok();
        let devices = en
            .get_device_collection(Direction::Capture)
            .map_err(|e| list_error(&e))?;
        let mut out = Vec::new();
        for d in &devices {
            let Ok(d) = d else { continue };
            if !matches!(d.get_state(), Ok(DeviceState::Active)) {
                continue;
            }
            let Ok(id) = d.get_id() else { continue };
            out.push(describe(&d, id, default_id.as_deref()));
        }
        Ok(out)
    })
}

/// Input endpoints Windows knows but that are not usable now (unplugged, not
/// present, disabled), as (name, state). For reports and for telling "your
/// mixer is not connected" apart from "this computer has no input at all".
pub(super) fn list_inactive_inputs() -> Vec<(String, &'static str)> {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
        DEVICE_STATEMASK_ALL, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    };
    use windows::Win32::System::Com::CoCreateInstance;
    with_com(|| {
        let mut out = Vec::new();
        // SAFETY: COM calls on live interfaces within this COM-initialised thread.
        unsafe {
            let Ok(en) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                return out;
            };
            let Ok(all) = en.EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL))
            else {
                return out;
            };
            for i in 0..all.GetCount().unwrap_or(0) {
                let Ok(dev) = all.Item(i) else { continue };
                let state = match dev.GetState() {
                    Ok(s) if s == DEVICE_STATE_UNPLUGGED => "unplugged",
                    Ok(s) if s == DEVICE_STATE_NOTPRESENT => "not present",
                    Ok(s) if s == DEVICE_STATE_DISABLED => "disabled",
                    _ => continue, // active ones are in list_input_devices
                };
                let name = Device::from_immdevice(dev)
                    .ok()
                    .and_then(|d| d.get_friendlyname().ok())
                    .unwrap_or_else(|| "unnamed input".into());
                out.push((name, state));
            }
        }
        out
    })
}

fn list_error(e: &WasapiError) -> InputError {
    InputError::new(
        FailureKind::Device,
        format!("listing the input devices failed: {e}"),
    )
}

fn describe(d: &Device, id: String, default_id: Option<&str>) -> InputDeviceInfo {
    let name = d
        .get_friendlyname()
        .unwrap_or_else(|_| "unnamed input device".into());
    let adapter = d
        .get_interface_friendlyname()
        .ok()
        .filter(|s| !s.is_empty());
    let form_factor = prop_u32(d, &PKEY_AUDIOENDPOINT_FORMFACTOR);
    let enumerator = prop_string(d, &PKEY_DEVICE_ENUMERATORNAME);
    let adapter_id = if enumerator.is_none() {
        topology_adapter_id(d)
    } else {
        None
    };
    let (transport, hands_free) = classify(
        enumerator.as_deref(),
        adapter_id.as_deref(),
        &name,
        form_factor,
    );
    // Never activate an audio client on a Bluetooth endpoint: read the device
    // format from the property store instead (trap 2).
    let fmt = if hands_free || transport == Transport::Bluetooth {
        d.get_device_format().ok()
    } else {
        d.get_iaudioclient()
            .and_then(|c| c.get_mixformat())
            .ok()
            .or_else(|| d.get_device_format().ok())
    };
    let bus = enumerator.or_else(|| {
        adapter_id.map(|a| {
            let (t, _) = classify(None, Some(&a), "", None);
            format!("{} (from the device topology)", t.label())
        })
    });
    InputDeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
        adapter,
        transport,
        form_factor: form_factor.map(|f| form_factor_name(f).to_string()),
        bus,
        channels: fmt.as_ref().map_or(0, |f| f.get_nchannels()),
        sample_rate: fmt.as_ref().map_or(0, |f| f.get_samplespersec()),
        sample_format: fmt.as_ref().and_then(sample_format_name),
        hands_free,
        declared_latency_frames: None,
    }
}

/// The default output (render) endpoint's id: for the loopback test path only.
pub(super) fn default_output_id() -> Option<String> {
    with_com(|| {
        DeviceEnumerator::new()
            .ok()?
            .get_default_device(Direction::Render)
            .ok()?
            .get_id()
            .ok()
    })
}

// ---------------------------------------------------------------------------
// Permission: Windows' microphone privacy switches (read only)
// ---------------------------------------------------------------------------

/// Read `Value` (REG_SZ) under `root\path`. `None` if absent.
fn reg_value(root: HKEY, path: &str) -> Option<String> {
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let path = HSTRING::from(path);
    // SAFETY: valid key path / value name; the buffer and its size in bytes.
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(path.as_ptr()),
            w!("Value"),
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

/// "Microphone access" (machine and user) and "Let desktop apps access your
/// microphone" (user). A value of "Deny" in any of them blocks a desktop app.
pub(super) fn mic_permission() -> MicPermission {
    const STORE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";
    let machine = reg_value(HKEY_LOCAL_MACHINE, STORE);
    let user = reg_value(HKEY_CURRENT_USER, STORE);
    let desktop = reg_value(HKEY_CURRENT_USER, &format!("{STORE}\\NonPackaged"));
    let denied = |v: &Option<String>| v.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("Deny"));
    if denied(&machine) || denied(&user) || denied(&desktop) {
        MicPermission::Denied
    } else {
        MicPermission::Granted
    }
}

pub(super) fn request_mic_permission() -> MicPermission {
    mic_permission()
}

/// The three switches as stored (read only): the machine's "Microphone access", the
/// user's "Microphone access" and "Let desktop apps access your microphone". `None` =
/// the value is absent (Windows' default, which allows).
pub(super) fn privacy_switches() -> [Option<String>; 3] {
    const STORE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";
    [
        reg_value(HKEY_LOCAL_MACHINE, STORE),
        reg_value(HKEY_CURRENT_USER, STORE),
        reg_value(HKEY_CURRENT_USER, &format!("{STORE}\\NonPackaged")),
    ]
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// How packet bytes become f32 samples (exact for every integer format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conv {
    F32,
    I16,
    I24,
    I32,
}

impl Conv {
    fn bytes(self) -> usize {
        match self {
            Conv::I16 => 2,
            Conv::I24 => 3,
            Conv::F32 | Conv::I32 => 4,
        }
    }
}

/// The capture thread's handle.
pub(super) struct Capture {
    id: String,
    stop: Arc<AtomicBool>,
    start_tx: Option<SyncSender<CaptureProducer>>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub(super) fn new(id: String) -> Self {
        Self {
            id,
            stop: Arc::new(AtomicBool::new(false)),
            start_tx: None,
            thread: None,
        }
    }

    pub(super) fn open(&mut self) -> Result<InputFormat, InputError> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (start_tx, start_rx) = mpsc::sync_channel(1);
        let id = self.id.clone();
        let stop = self.stop.clone();
        let thread = std::thread::Builder::new()
            .name("jk-input-wasapi".into())
            .spawn(move || capture_thread(&id, &ready_tx, &start_rx, &stop))
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
        self.start_tx = None; // a thread still waiting to start exits
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Everything opened on the capture thread.
struct Opened {
    name: String,
    format: InputFormat,
    conv: Conv,
    block_align: usize,
    buffer_frames: usize,
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Handle,
    keepalive: Option<KeepAlive>,
}

/// The loopback test path's silent render stream (see the module docs).
struct KeepAlive {
    client: AudioClient,
    render: AudioRenderClient,
    zeros: Vec<u8>,
    block_align: usize,
}

impl KeepAlive {
    fn silent() -> Option<BufferFlags> {
        Some(BufferFlags {
            data_discontinuity: false,
            silent: true,
            timestamp_error: false,
        })
    }

    fn open(device: &Device) -> Result<Self, WasapiError> {
        let mut client = device.get_iaudioclient()?;
        let mix = client.get_mixformat()?;
        client.initialize_client(
            &mix,
            Direction::Render,
            StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: 1_000_000,
            },
            None,
        )?;
        let render = client.get_audiorenderclient()?;
        let block_align = mix.get_blockalign() as usize;
        let frames = client.get_buffer_size()? as usize;
        let ka = Self {
            client,
            render,
            zeros: vec![0u8; frames * block_align],
            block_align,
        };
        ka.top_up()?;
        ka.client.start_stream()?;
        Ok(ka)
    }

    /// Fill the free space with buffers flagged silent (digital zeros).
    fn top_up(&self) -> Result<(), WasapiError> {
        let avail = self.client.get_available_space_in_frames()? as usize;
        if avail > 0 {
            self.render
                .write_to_device(&self.zeros[..avail * self.block_align], Self::silent())?;
        }
        Ok(())
    }
}

fn open_stream(id: &str) -> Result<Opened, InputError> {
    let en = DeviceEnumerator::new().map_err(|e| {
        let (k, m) = classify_error("the audio system", "Opening", &e);
        input_error(k, m)
    })?;
    let device = en.get_device(id).map_err(|e| {
        let (k, m) = classify_error(id, "Opening", &e);
        input_error(k, m)
    })?;
    let name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "the input device".into());
    if !matches!(device.get_state(), Ok(DeviceState::Active)) {
        return Err(input_error(
            FailureKind::Unplugged,
            format!("\"{name}\" is not connected or is disabled."),
        ));
    }
    let loopback = device.get_direction() == Direction::Render;
    let err = |doing: &str, e: &WasapiError| {
        let (k, m) = classify_error(&name, doing, e);
        input_error(k, m)
    };
    let mut notes = Vec::new();

    // First with the raw option, then without if the endpoint refuses it.
    let attempt = |raw: bool| -> Result<(AudioClient, WaveFormat), WasapiError> {
        let mut client = device.get_iaudioclient()?;
        if !loopback {
            let mut props = AudioClientProperties::new().set_category(StreamCategory::Other);
            if raw {
                props = props.set_option(StreamOption::Raw);
            }
            client.set_properties(props)?;
        }
        let mix = client.get_mixformat()?;
        client.initialize_client(
            &mix,
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: false,
                buffer_duration_hns: CAPTURE_BUFFER_HNS,
            },
            None,
        )?;
        Ok((client, mix))
    };
    let (client, mix, raw) = if loopback {
        let (c, m) = attempt(false).map_err(|e| err("Opening", &e))?;
        (c, m, false)
    } else {
        match attempt(true) {
            Ok((c, m)) => (c, m, true),
            Err(e) if hresult(&e) == Some(E_ACCESSDENIED) => return Err(err("Opening", &e)),
            Err(e) => {
                notes.push(format!(
                    "the raw stream (no Windows voice processing) was refused ({e}); Windows' own \
                     input processing for this device may be active"
                ));
                let (c, m) = attempt(false).map_err(|e| err("Opening", &e))?;
                (c, m, false)
            }
        }
    };
    let sub = mix
        .get_subformat()
        .map_err(|e| err("Reading the format of", &e))?;
    let conv = match (sub, mix.get_bitspersample()) {
        (SampleType::Float, 32) => Conv::F32,
        (SampleType::Int, 16) => Conv::I16,
        (SampleType::Int, 24) => Conv::I24,
        (SampleType::Int, 32) => Conv::I32,
        (st, b) => {
            return Err(InputError::new(
                FailureKind::Format,
                format!(
                    "\"{name}\" delivers {b}-bit {st:?} samples, which practice mode does not read"
                ),
            ));
        }
    };
    let event = client
        .set_get_eventhandle()
        .map_err(|e| err("Setting up", &e))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| err("Setting up", &e))?;
    let buffer_frames = client
        .get_buffer_size()
        .map_err(|e| err("Setting up", &e))? as usize;
    let latency_ns = client.get_stream_latency().ok().map(|hns| hns * 100);
    let keepalive = if loopback {
        Some(KeepAlive::open(&device).map_err(|e| err("Starting the silent keep-alive on", &e))?)
    } else {
        None
    };
    let source = if loopback {
        "WASAPI loopback of an output (TEST PATH: same capture code as a microphone)".to_string()
    } else if raw {
        "WASAPI shared mode, event driven, raw stream".to_string()
    } else {
        "WASAPI shared mode, event driven".to_string()
    };
    let format = InputFormat {
        source,
        device_name: name.clone(),
        sample_rate: mix.get_samplespersec(),
        channels: mix.get_nchannels(),
        sample_format: sample_format_name(&mix).unwrap_or_else(|| "?".into()),
        max_block_frames: buffer_frames,
        declared_latency_ns: latency_ns,
        notes,
    };
    Ok(Opened {
        name,
        format,
        conv,
        block_align: mix.get_blockalign() as usize,
        buffer_frames,
        client,
        capture,
        event,
        keepalive,
    })
}

/// MMCSS "Pro Audio" for the capture thread, reverted on drop.
struct Mmcss(Option<HANDLE>);

impl Mmcss {
    fn join() -> Self {
        let mut task = 0u32;
        // SAFETY: plain Win32 call with a static string and a valid out pointer.
        Mmcss(unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) }.ok())
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

fn capture_thread(
    id: &str,
    ready: &SyncSender<Result<InputFormat, InputError>>,
    start: &Receiver<CaptureProducer>,
    stop: &AtomicBool,
) {
    let com = wasapi::initialize_mta();
    match open_stream(id) {
        Err(e) => {
            let _ = ready.send(Err(e));
        }
        Ok(opened) => {
            let _ = ready.send(Ok(opened.format.clone()));
            // Wait for the ring (or for the stream to be dropped unopened).
            if let Ok(producer) = start.recv() {
                run(opened, producer, stop);
            }
        }
    }
    if com.is_ok() {
        wasapi::deinitialize();
    }
}

/// The capture loop: the only code that touches each packet. No allocation:
/// `bytes` is sized once, conversion writes straight into the ring.
fn run(o: Opened, mut producer: CaptureProducer, stop: &AtomicBool) {
    let status: Arc<StreamStatus> = producer.status();
    let _mmcss = Mmcss::join();
    let mut bytes = vec![0u8; o.buffer_frames.max(1) * o.block_align];
    if let Err(e) = o.client.start_stream() {
        let (k, m) = classify_error(&o.name, "Starting", &e);
        status.fail(k, m);
        return;
    }
    let mut first = true;
    let channels = usize::from(o.format.channels);
    'outer: while !stop.load(Ordering::Acquire) {
        let _ = o.event.wait_for_event(WAIT_MS); // a timeout just loops
        if let Some(ka) = &o.keepalive {
            let _ = ka.top_up();
        }
        loop {
            match o.capture.get_next_packet_size() {
                Ok(Some(0)) | Ok(None) => break,
                Ok(Some(n)) if n as usize * o.block_align > bytes.len() => {
                    // Never expected (a packet is at most the buffer).
                    status.fail(
                        FailureKind::Device,
                        format!("\"{}\" delivered a packet larger than its buffer", o.name),
                    );
                    break 'outer;
                }
                Ok(Some(_)) => {}
                Err(e) => {
                    let (k, m) = classify_error(&o.name, "Reading from", &e);
                    status.fail(k, m);
                    break 'outer;
                }
            }
            let (frames, info) = match o.capture.read_from_device(&mut bytes) {
                Ok(v) => v,
                Err(e) => {
                    let (k, m) = classify_error(&o.name, "Reading from", &e);
                    status.fail(k, m);
                    break 'outer;
                }
            };
            if frames == 0 {
                break;
            }
            push_packet(
                &mut producer,
                &o,
                &bytes,
                frames as usize,
                channels,
                &info,
                first,
            );
            first = false;
        }
    }
    let _ = o.client.stop_stream();
    if let Some(ka) = &o.keepalive {
        let _ = ka.client.stop_stream();
    }
}

#[inline]
fn push_packet(
    producer: &mut CaptureProducer,
    o: &Opened,
    bytes: &[u8],
    frames: usize,
    channels: usize,
    info: &BufferInfo,
    first: bool,
) {
    let mut flags = 0u8;
    if info.flags.silent {
        flags |= FLAG_SILENT;
    }
    // Windows commonly flags the very first packet; it means nothing there.
    if info.flags.data_discontinuity && !first {
        flags |= FLAG_DISCONTINUITY;
    }
    let host_ns = if info.flags.timestamp_error {
        flags |= FLAG_TIMESTAMP_ESTIMATED;
        host_time_ns()
    } else {
        qpc_100ns_to_ns(info.timestamp)
    };
    let n = frames * channels;
    let data = &bytes[..n * o.conv.bytes()];
    match o.conv {
        Conv::F32 => producer.push_iter(
            frames,
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b)),
            host_ns,
            flags,
        ),
        Conv::I16 => producer.push_iter(
            frames,
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from(i16::from_le_bytes(*b)) / 32_768.0),
            host_ns,
            flags,
        ),
        Conv::I24 => producer.push_iter(
            frames,
            data.as_chunks::<3>()
                .0
                .iter()
                .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32) / 2_147_483_648.0),
            host_ns,
            flags,
        ),
        Conv::I32 => producer.push_iter(
            frames,
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0),
            host_ns,
            flags,
        ),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_from_enumerator_topology_and_name() {
        assert_eq!(
            classify(Some("USB"), None, "Microphone (AME2)", Some(4)),
            (Transport::Usb, false)
        );
        assert_eq!(
            classify(Some("BTHHFENUM"), None, "Headset", Some(5)),
            (Transport::Bluetooth, true)
        );
        assert_eq!(
            classify(Some("BTHLEDEVICE"), None, "Buds", Some(5)),
            (Transport::Bluetooth, true)
        );
        assert_eq!(
            classify(Some("HDAUDIO"), None, "Mic in", Some(4)),
            (Transport::BuiltIn, false)
        );
        assert_eq!(
            classify(Some("ROOT"), None, "Cable Output", Some(2)),
            (Transport::Virtual, false)
        );
        assert_eq!(
            classify(
                None,
                Some(r"{2}.\\?\usb#vid_352f&pid_0110&mi_00#7&1&0000#{6994ad04}"),
                "x",
                None
            ),
            (Transport::Usb, false)
        );
        assert_eq!(
            classify(
                None,
                Some(r"{2}.\\?\bthhfenum#{0000111e-0000}_vid&0001004c#8&2b&0#{guid}"),
                "x",
                None
            ),
            (Transport::Bluetooth, true)
        );
        assert_eq!(
            classify(
                None,
                None,
                "Microphone (iPhone Hands-Free HF Audio)",
                Some(4)
            ),
            (Transport::Bluetooth, true)
        );
        assert_eq!(
            classify(None, None, "Mystery", None),
            (Transport::Unknown, false)
        );
    }

    #[test]
    fn integer_conversions_are_exact() {
        // i24 packed: 0x7FFFFF -> just under 1.0; 0x800000 -> -1.0.
        let max = i32::from_le_bytes([0, 0xFF, 0xFF, 0x7F]) as f32 / 2_147_483_648.0;
        let min = i32::from_le_bytes([0, 0x00, 0x00, 0x80]) as f32 / 2_147_483_648.0;
        assert!((max - (1.0 - 1.0 / 8_388_608.0)).abs() < 1e-7);
        assert_eq!(min, -1.0);
        assert_eq!(f32::from(i16::MIN) / 32_768.0, -1.0);
    }
}

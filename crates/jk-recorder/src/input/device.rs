// SPDX-License-Identifier: GPL-3.0-or-later
//! Input devices: what the OS lists, and which one practice mode uses when the
//! singer has not chosen (WO2 section 4.4 "Choosing the input", trap 2).
//!
//! The rule, in order:
//!
//! 1. A device the singer chose (saved per computer by the app) is used if it
//!    is present, matched by id first and by name second (a USB mixer moved to
//!    another port can get a new id on Windows but keeps its name).
//! 2. Otherwise the best automatic candidate: an external USB interface or
//!    mixer, then other wired external devices (Thunderbolt, FireWire, PCI
//!    cards), then the computer's built-in microphone or jacks, then anything
//!    unclassified. The system default breaks ties within a class.
//! 3. **Never** a Bluetooth device (hands-free headsets, AirPods, an iPhone's
//!    hands-free link): opening a Bluetooth microphone switches the headset to
//!    its hands-free profile and the music becomes low-quality mono (trap 2).
//!    Virtual and aggregate devices, AirPlay and Continuity (an iPhone used as a
//!    Mac microphone) are never chosen automatically either. They can all still
//!    be picked by hand, with [`InputDeviceInfo::warning`] shown.

use serde::Serialize;

/// How a device is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// USB (audio interfaces, podcast mixers, USB microphones).
    Usb,
    /// The computer's own microphone or audio jacks (macOS "built-in"; on
    /// Windows the HD Audio / Intel SST / ACPI codecs on the mainboard).
    BuiltIn,
    /// Thunderbolt, FireWire or a PCI(e) card: wired external interfaces.
    WiredExternal,
    /// Bluetooth Classic or Bluetooth LE Audio.
    Bluetooth,
    /// HDMI / DisplayPort (a monitor's microphone input, rare).
    Display,
    /// A software device: virtual cables, aggregate devices, AirPlay,
    /// Continuity Camera, network audio.
    Virtual,
    /// The OS did not say.
    Unknown,
}

impl Transport {
    /// Rank for automatic choice; `None` = never automatic.
    pub fn auto_rank(self) -> Option<u8> {
        match self {
            Transport::Usb => Some(4),
            Transport::WiredExternal => Some(3),
            Transport::BuiltIn => Some(2),
            Transport::Unknown => Some(1),
            Transport::Display => Some(0),
            Transport::Bluetooth | Transport::Virtual => None,
        }
    }

    /// Plain name for lists and reports.
    pub fn label(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::BuiltIn => "built-in",
            Transport::WiredExternal => "wired (Thunderbolt/FireWire/PCI)",
            Transport::Bluetooth => "Bluetooth",
            Transport::Display => "HDMI/DisplayPort",
            Transport::Virtual => "virtual",
            Transport::Unknown => "unknown",
        }
    }
}

/// One input device, as listed by the OS. Opening nothing: listing never
/// starts a stream (on Windows a Bluetooth device's format is read from its
/// property store, so not even an audio client is activated on it).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputDeviceInfo {
    /// Stable id to save and reopen (Windows endpoint id, macOS device UID).
    pub id: String,
    /// The name the OS shows (Windows: "Microphone (MAONO AME2)").
    pub name: String,
    /// The hardware it belongs to, when the OS says (Windows: the adapter's
    /// friendly name; macOS: the manufacturer).
    pub adapter: Option<String>,
    /// How it is attached.
    pub transport: Transport,
    /// Windows endpoint form factor ("microphone", "line_level", "headset", ...).
    pub form_factor: Option<String>,
    /// Windows: the device's bus enumerator ("USB", "HDAUDIO", "BTHHFENUM", ...),
    /// macOS: the Core Audio transport code; for reports.
    pub bus: Option<String>,
    /// Input channels the device offers in shared mode.
    pub channels: u16,
    /// Sample rate it runs at now (Windows mix format, macOS nominal rate).
    pub sample_rate: u32,
    /// Sample format the OS delivers ("f32", "i16", "i24", "i32"), if known.
    pub sample_format: Option<String>,
    /// The system default input.
    pub is_default: bool,
    /// Bluetooth hands-free: never chosen automatically (trap 2).
    pub hands_free: bool,
    /// Input latency the device declares, in frames (macOS: device latency +
    /// safety offset + stream latency), when known. Undeclared delay is
    /// measured by the rig check (WO2 section 4.8).
    pub declared_latency_frames: Option<u32>,
}

impl InputDeviceInfo {
    /// A warning to show when this device is picked by hand, if any.
    pub fn warning(&self) -> Option<&'static str> {
        match self.transport {
            Transport::Bluetooth => Some(
                "This is a Bluetooth microphone. Opening it switches the headset to its \
                 hands-free mode: the music turns into low-quality phone sound and the delay \
                 wanders. Practise with a wired or USB microphone, or your mixer, instead.",
            ),
            Transport::Virtual => Some(
                "This is a software device, not a microphone. Pick it only if you know it carries \
                 your voice.",
            ),
            _ => None,
        }
    }

    /// True if the automatic choice may pick this device.
    pub fn auto_eligible(&self) -> bool {
        !self.hands_free && self.channels > 0 && self.transport.auto_rank().is_some()
    }
}

/// True for names that announce a Bluetooth hands-free link (Windows calls
/// them "... Hands-Free AG Audio" or "... Hands-Free HF Audio").
pub fn name_says_hands_free(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("hands-free") || n.contains("handsfree") || n.contains("hands free")
}

/// The device practice mode uses when the singer has not chosen (rule 2 in
/// the module docs). `None` if no device may be chosen automatically.
pub fn preferred_input(devices: &[InputDeviceInfo]) -> Option<&InputDeviceInfo> {
    devices.iter().filter(|d| d.auto_eligible()).max_by(|a, b| {
        let ka = (a.transport.auto_rank(), a.is_default);
        let kb = (b.transport.auto_rank(), b.is_default);
        // max_by keeps the LAST maximum; reverse the list order so the
        // first-listed device wins a full tie.
        ka.cmp(&kb).then(std::cmp::Ordering::Greater)
    })
}

/// The saved choice if it is present (rule 1), else the automatic one.
/// Returns the device and whether it is the saved choice.
pub fn resolve_input<'a>(
    devices: &'a [InputDeviceInfo],
    saved_id: Option<&str>,
    saved_name: Option<&str>,
) -> Option<(&'a InputDeviceInfo, bool)> {
    if let Some(id) = saved_id
        && let Some(d) = devices.iter().find(|d| d.id == id)
    {
        return Some((d, true));
    }
    if let Some(name) = saved_name
        && let Some(d) = devices.iter().find(|d| d.name == name)
    {
        return Some((d, true));
    }
    preferred_input(devices).map(|d| (d, false))
}

#[cfg(test)]
pub(crate) fn test_device(name: &str, transport: Transport, is_default: bool) -> InputDeviceInfo {
    InputDeviceInfo {
        id: format!("id:{name}"),
        name: name.into(),
        adapter: None,
        transport,
        form_factor: None,
        bus: None,
        channels: 2,
        sample_rate: 48_000,
        sample_format: Some("f32".into()),
        is_default,
        hands_free: transport == Transport::Bluetooth,
        declared_latency_frames: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_mixer_beats_the_builtin_default_and_bluetooth_is_never_automatic() {
        let devs = vec![
            test_device("AirPods", Transport::Bluetooth, true),
            test_device("MacBook Air Microphone", Transport::BuiltIn, false),
            test_device("AME2", Transport::Usb, false),
        ];
        assert_eq!(preferred_input(&devs).unwrap().name, "AME2");
        let only_bt = vec![test_device("iPhone Hands-Free", Transport::Bluetooth, true)];
        assert!(preferred_input(&only_bt).is_none());
        let bt_and_builtin = vec![
            test_device("Headset", Transport::Bluetooth, true),
            test_device("Built-in", Transport::BuiltIn, false),
        ];
        assert_eq!(preferred_input(&bt_and_builtin).unwrap().name, "Built-in");
    }

    #[test]
    fn default_breaks_ties_then_list_order() {
        let devs = vec![
            test_device("USB A", Transport::Usb, false),
            test_device("USB B", Transport::Usb, true),
            test_device("USB C", Transport::Usb, false),
        ];
        assert_eq!(preferred_input(&devs).unwrap().name, "USB B");
        let devs = vec![
            test_device("USB A", Transport::Usb, false),
            test_device("USB C", Transport::Usb, false),
        ];
        assert_eq!(preferred_input(&devs).unwrap().name, "USB A");
    }

    #[test]
    fn virtual_devices_and_hands_free_names_are_not_automatic() {
        let mut hf = test_device(
            "Microphone (Phone Hands-Free HF Audio)",
            Transport::Unknown,
            true,
        );
        hf.hands_free = name_says_hands_free(&hf.name);
        let devs = vec![hf, test_device("Virtual Cable", Transport::Virtual, false)];
        assert!(preferred_input(&devs).is_none());
        assert!(devs[1].warning().is_some());
    }

    #[test]
    fn saved_choice_by_id_then_name() {
        let mut moved = test_device("AME2", Transport::Usb, false);
        moved.id = "new-port".into();
        let devs = vec![test_device("Built-in", Transport::BuiltIn, true), moved];
        let (d, saved) = resolve_input(&devs, Some("old-port"), Some("AME2")).unwrap();
        assert!(saved);
        assert_eq!(d.id, "new-port");
        // A saved Bluetooth choice is honoured (the singer chose it), with its warning.
        let devs = vec![test_device("Headset", Transport::Bluetooth, true)];
        let (d, saved) = resolve_input(&devs, Some("id:Headset"), None).unwrap();
        assert!(saved && d.warning().is_some());
        // Nothing saved and nothing eligible.
        assert!(resolve_input(&devs, None, None).is_none());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Descriptions of what the OS delivers and of the output device, so the UI and
//! the self-check can tell the user exactly which format they are getting
//! (WORK_ORDER section 4.6: "Tell the user what format they are getting").

use std::fmt;

/// How the samples arrived from the OS, before we store them.
///
/// The recorder never resamples, dithers, normalises or mixes. The only
/// transformation is a lossless re-layout (planar -> interleaved) and, for
/// 16-bit integer sources, the exact integer -> float mapping `x / 32768`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEncoding {
    /// IEEE 32-bit float. `interleaved == false` means one buffer per channel
    /// (what ScreenCaptureKit is reported to deliver).
    Float32 {
        /// True if all channels share one buffer.
        interleaved: bool,
    },
    /// Signed 16-bit integer PCM (converted exactly to float).
    Int16 {
        /// True if all channels share one buffer.
        interleaved: bool,
    },
    /// Signed 32-bit integer PCM. Converting to 32-bit float keeps 24 bits of
    /// mantissa, which is lossless for 24-bit content in a 32-bit container.
    Int32 {
        /// True if all channels share one buffer.
        interleaved: bool,
    },
}

impl SourceEncoding {
    /// The short name used by the app API (`docs/APP_API.md` section 8,
    /// `RecorderStatus.format.sample_format`): `"f32"`, `"i16"` or `"i32"`.
    pub fn short_name(&self) -> &'static str {
        match self {
            SourceEncoding::Float32 { .. } => "f32",
            SourceEncoding::Int16 { .. } => "i16",
            SourceEncoding::Int32 { .. } => "i32",
        }
    }
}

impl fmt::Display for SourceEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, interleaved) = match self {
            SourceEncoding::Float32 { interleaved } => ("32-bit float", *interleaved),
            SourceEncoding::Int16 { interleaved } => ("16-bit integer", *interleaved),
            SourceEncoding::Int32 { interleaved } => ("32-bit integer", *interleaved),
        };
        write!(
            f,
            "{name}, {}",
            if interleaved { "interleaved" } else { "planar" }
        )
    }
}

/// The real format of the captured audio, read from the first buffer the OS
/// delivered (not what we asked for). This is the format written to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureFormat {
    /// Frames per second as delivered by the OS.
    pub sample_rate: u32,
    /// Number of channels as delivered by the OS.
    pub channels: u16,
    /// Sample encoding as delivered by the OS.
    pub source: SourceEncoding,
}

impl CaptureFormat {
    /// The file encoding is always 32-bit float WAV (RF64 above 4 GiB).
    pub const FILE_ENCODING: &'static str = "32-bit float WAV (RF64 above 4 GiB)";
}

impl fmt::Display for CaptureFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} Hz, {} ch, {} (stored as {})",
            self.sample_rate,
            self.channels,
            self.source,
            Self::FILE_ENCODING
        )
    }
}

/// What we know about the default output device (the thing being recorded).
#[derive(Clone, Debug, PartialEq)]
pub struct OutputDeviceInfo {
    /// Device name as the OS shows it.
    pub name: String,
    /// The device's nominal sample rate (what the hardware runs at).
    pub nominal_sample_rate: f64,
    /// Bits per sample of the device's physical format, if the OS reports it.
    pub physical_bits: Option<u32>,
    /// Channels of the device's physical output format, if known.
    pub physical_channels: Option<u32>,
}

/// An output device whose sound can be recorded (Windows: an active render
/// endpoint; pass `id` as `RecorderConfig::device_id`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureDevice {
    /// The OS endpoint id (Windows: the `IMMDevice` id string, stable across
    /// restarts for the same device).
    pub id: String,
    /// Device name as the OS shows it.
    pub name: String,
    /// True for the current default output device.
    pub is_default: bool,
}

/// What the backend decided when it started, for the UI, logs and self-check.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureSetup {
    /// "ScreenCaptureKit", "WASAPI loopback", ...
    pub backend: &'static str,
    /// The default output device, if it could be queried.
    pub device: Option<OutputDeviceInfo>,
    /// The sample rate we asked the OS for (ScreenCaptureKit only offers a
    /// fixed list; see docs/MAC.md).
    pub requested_sample_rate: Option<u32>,
    /// The channel count we asked for.
    pub requested_channels: u16,
    /// Plain-language notes: conversions the OS will do, fallbacks taken, the
    /// size of the throw-away video stream, and so on. Shown in the self-check.
    pub notes: Vec<String>,
}

/// The sample rates ScreenCaptureKit accepts for `SCStreamConfiguration.sampleRate`
/// (Apple documentation, checked 2026-11-17: "The framework supports sample rates
/// of 8000, 16000, 24000, and 48000"; anything else silently becomes 48 kHz).
pub const SCK_SUPPORTED_RATES: [u32; 4] = [8_000, 16_000, 24_000, 48_000];

/// Pick the rate to request from ScreenCaptureKit for a device running at
/// `device_rate`, and say in plain words whether the OS will convert.
///
/// Rule (WORK_ORDER section 4.10, "pick the capture rate that avoids a second
/// conversion"): if the device's own rate is one ScreenCaptureKit offers, ask for
/// exactly that, so no sample-rate conversion happens anywhere. Otherwise ask for
/// 48 kHz (the highest rate offered, and the default) and say that macOS converts.
pub fn choose_sck_rate(device_rate: Option<f64>) -> (u32, Option<String>) {
    match device_rate {
        Some(r) if r.is_finite() && r > 0.0 => {
            let rounded = r.round() as u32;
            if (r - rounded as f64).abs() < 0.5 && SCK_SUPPORTED_RATES.contains(&rounded) {
                (rounded, None)
            } else {
                (
                    48_000,
                    Some(format!(
                        "The output device runs at {r} Hz. ScreenCaptureKit only offers 8000, 16000, 24000 \
                         or 48000 Hz, so macOS converts the system mix to 48000 Hz before we receive it. \
                         To avoid that conversion, set the output device to 48,000 Hz in Audio MIDI Setup."
                    )),
                )
            }
        }
        _ => (
            48_000,
            Some(
                "The output device's sample rate could not be read; requesting 48000 Hz \
                 (ScreenCaptureKit's default)."
                    .to_string(),
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sck_rate_matches_device_when_offered() {
        assert_eq!(choose_sck_rate(Some(48_000.0)), (48_000, None));
        assert_eq!(choose_sck_rate(Some(24_000.0)), (24_000, None));
    }

    #[test]
    fn sck_rate_falls_back_to_48k_with_a_note() {
        let (rate, note) = choose_sck_rate(Some(44_100.0));
        assert_eq!(rate, 48_000);
        assert!(note.unwrap().contains("Audio MIDI Setup"));
        let (rate, note) = choose_sck_rate(Some(96_000.0));
        assert_eq!(rate, 48_000);
        assert!(note.is_some());
        let (rate, note) = choose_sck_rate(None);
        assert_eq!(rate, 48_000);
        assert!(note.is_some());
    }

    #[test]
    fn format_display_names_the_real_format() {
        let f = CaptureFormat {
            sample_rate: 48_000,
            channels: 2,
            source: SourceEncoding::Float32 { interleaved: false },
        };
        assert_eq!(
            f.to_string(),
            "48000 Hz, 2 ch, 32-bit float, planar (stored as 32-bit float WAV (RF64 above 4 GiB))"
        );
    }
}

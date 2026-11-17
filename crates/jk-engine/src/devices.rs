// SPDX-License-Identifier: GPL-3.0-or-later
//! Output device enumeration for the device picker.
//!
//! * Windows: active WASAPI render endpoints, with their friendly names ("Speakers
//!   (Realtek(R) Audio)"), endpoint ids, shared-mode mix format and whether any
//!   exclusive-mode format is accepted. Runs on a short-lived thread of its own so the
//!   caller's COM apartment (often the UI thread's STA) is never touched.
//! * macOS: cpal 0.16 output devices; the id is the device name.

use crate::api::{DeviceInfo, EngineError};

fn enum_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new("device_failed", "The list of audio outputs could not be read.", e.to_string())
}

/// Active output devices, the default first.
pub fn list_devices() -> Result<Vec<DeviceInfo>, EngineError> {
    #[cfg(windows)]
    {
        let h = std::thread::Builder::new()
            .name("jk-list-devices".into())
            .spawn(windows_list)
            .map_err(enum_err)?;
        let mut v = h.join().map_err(|_| enum_err("device enumeration thread panicked"))??;
        v.sort_by_key(|d| !d.is_default);
        Ok(v)
    }
    #[cfg(target_os = "macos")]
    {
        let mut v = macos_list()?;
        v.sort_by_key(|d| !d.is_default);
        Ok(v)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
fn windows_list() -> Result<Vec<DeviceInfo>, EngineError> {
    use wasapi::{initialize_mta, DeviceEnumerator, Direction};
    initialize_mta().ok().map_err(enum_err)?;
    let en = DeviceEnumerator::new().map_err(enum_err)?;
    let default_id = en.get_default_device(Direction::Render).ok().and_then(|d| d.get_id().ok());
    let coll = en.get_device_collection(Direction::Render).map_err(enum_err)?;
    let n = coll.get_nbr_devices().map_err(enum_err)?;
    let mut out = Vec::new();
    for i in 0..n {
        let Ok(dev) = coll.get_device_at_index(i) else { continue };
        let Ok(id) = dev.get_id() else { continue };
        let name = dev.get_friendlyname().unwrap_or_else(|_| id.clone());
        let (rate, ch, excl) = match dev.get_iaudioclient() {
            Ok(client) => match client.get_mixformat() {
                Ok(mix) => (
                    mix.get_samplespersec(),
                    mix.get_nchannels(),
                    crate::output::wasapi_out::exclusive_capable(&client, &mix),
                ),
                Err(_) => (0, 0, false),
            },
            Err(_) => (0, 0, false),
        };
        out.push(DeviceInfo {
            is_default: default_id.as_deref() == Some(id.as_str()),
            id,
            name,
            sample_rate: rate,
            channels: ch,
            exclusive_capable: excl,
        });
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn macos_list() -> Result<Vec<DeviceInfo>, EngineError> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    for dev in host.output_devices().map_err(enum_err)? {
        let Ok(name) = dev.name() else { continue };
        let (rate, ch) = match dev.default_output_config() {
            Ok(c) => (c.sample_rate().0, c.channels()),
            Err(_) => continue, // not an output we can open
        };
        out.push(DeviceInfo {
            is_default: default_name.as_deref() == Some(name.as_str()),
            id: name.clone(),
            name,
            sample_rate: rate,
            channels: ch,
            exclusive_capable: false,
        });
    }
    Ok(out)
}

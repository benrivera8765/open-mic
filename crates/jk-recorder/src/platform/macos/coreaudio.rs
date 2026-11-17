// SPDX-License-Identifier: GPL-3.0-or-later
//! Core Audio HAL queries for the default output device: name, nominal sample
//! rate, physical format. All properties used here exist since macOS 10.x;
//! `kAudioObjectPropertyElementMain` is the macOS 12+ spelling of the old
//! `...ElementMaster` constant and has the same value (0).

use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{NonNull, null};

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject, kAudioObjectUnknown, kAudioStreamPropertyPhysicalFormat,
};
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::{CFRetained, CFString};

use crate::format::OutputDeviceInfo;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size property. `T` must be a plain-old-data type matching the
/// property's documented type.
unsafe fn get<T: Copy>(object: AudioObjectID, selector: u32, scope: u32) -> Option<T> {
    let addr = address(selector, scope);
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `addr`, `size` and `value` are valid for the duration of the call,
    // and `value` has room for `size` bytes.
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

/// The default output device, or `None` if the HAL does not answer.
pub(crate) fn default_output_device() -> Option<OutputDeviceInfo> {
    // SAFETY: each call passes a property with its documented type.
    unsafe {
        let system = kAudioObjectSystemObject as AudioObjectID;
        let device: AudioObjectID = get(
            system,
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
        )?;
        if device == kAudioObjectUnknown {
            return None;
        }
        let nominal: f64 = get(
            device,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )?;
        // kAudioObjectPropertyName returns a CFStringRef the caller must release.
        let name = get::<*const CFString>(
            device,
            kAudioObjectPropertyName,
            kAudioObjectPropertyScopeGlobal,
        )
        .and_then(|p| NonNull::new(p.cast_mut()))
        .map(|p| CFRetained::from_raw(p).to_string())
        .unwrap_or_else(|| format!("device {device}"));
        let physical = first_output_stream(device).and_then(|s| {
            get::<AudioStreamBasicDescription>(
                s,
                kAudioStreamPropertyPhysicalFormat,
                kAudioObjectPropertyScopeGlobal,
            )
        });
        Some(OutputDeviceInfo {
            name,
            nominal_sample_rate: nominal,
            physical_bits: physical.map(|f| f.mBitsPerChannel).filter(|&b| b > 0),
            physical_channels: physical.map(|f| f.mChannelsPerFrame).filter(|&c| c > 0),
        })
    }
}

/// The device's first output stream.
unsafe fn first_output_stream(device: AudioObjectID) -> Option<AudioObjectID> {
    let addr = address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
    let mut size: u32 = 0;
    // SAFETY: valid pointers for the duration of the calls; the buffer has the
    // size the HAL asked for.
    unsafe {
        if AudioObjectGetPropertyDataSize(
            device,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
        ) != 0
        {
            return None;
        }
        let count = size as usize / size_of::<AudioObjectID>();
        if count == 0 {
            return None;
        }
        let mut ids = vec![0 as AudioObjectID; count];
        let status = AudioObjectGetPropertyData(
            device,
            NonNull::from(&addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(ids.as_mut_ptr().cast::<c_void>()),
        );
        if status != 0 {
            return None;
        }
        ids.first().copied()
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Raw C bindings for `vendor/rubberband/rubberband/rubberband-c.h` (Rubber Band 4.0.0)
//! plus the two helpers in `csrc/jk_rubberband_single.cpp`.
//!
//! Hand-written (no bindgen) so that cross-checking for macOS from Windows needs no
//! clang or Apple SDK. Only the functions the wrapper uses are declared.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_double, c_int, c_uint};

/// Opaque `struct RubberBandState_`.
#[repr(C)]
pub struct RubberBandState_ {
    _private: [u8; 0],
}
pub type RubberBandState = *mut RubberBandState_;
pub type RubberBandOptions = c_int;

// Option bits, copied from `enum RubberBandOption` in rubberband-c.h.
pub const OPTION_PROCESS_REAL_TIME: RubberBandOptions = 0x0000_0001;
pub const OPTION_TRANSIENTS_CRISP: RubberBandOptions = 0x0000_0000;
pub const OPTION_THREADING_NEVER: RubberBandOptions = 0x0001_0000;
pub const OPTION_WINDOW_STANDARD: RubberBandOptions = 0x0000_0000;
pub const OPTION_WINDOW_SHORT: RubberBandOptions = 0x0010_0000;
pub const OPTION_FORMANT_SHIFTED: RubberBandOptions = 0x0000_0000;
pub const OPTION_FORMANT_PRESERVED: RubberBandOptions = 0x0100_0000;
pub const OPTION_PITCH_HIGH_SPEED: RubberBandOptions = 0x0000_0000;
pub const OPTION_PITCH_HIGH_QUALITY: RubberBandOptions = 0x0200_0000;
pub const OPTION_PITCH_HIGH_CONSISTENCY: RubberBandOptions = 0x0400_0000;
pub const OPTION_CHANNELS_TOGETHER: RubberBandOptions = 0x1000_0000;
pub const OPTION_ENGINE_FINER: RubberBandOptions = 0x2000_0000;

// When the C++ was not compiled (type-checking for macOS on Windows) these symbols do not
// exist; that is fine because `cargo check` never links.
unsafe extern "C" {
    pub fn rubberband_new(
        sample_rate: c_uint,
        channels: c_uint,
        options: RubberBandOptions,
        initial_time_ratio: c_double,
        initial_pitch_scale: c_double,
    ) -> RubberBandState;
    pub fn rubberband_delete(state: RubberBandState);
    pub fn rubberband_reset(state: RubberBandState);
    pub fn rubberband_get_engine_version(state: RubberBandState) -> c_int;
    pub fn rubberband_set_pitch_scale(state: RubberBandState, scale: c_double);
    pub fn rubberband_get_pitch_scale(state: RubberBandState) -> c_double;
    pub fn rubberband_get_preferred_start_pad(state: RubberBandState) -> c_uint;
    pub fn rubberband_get_start_delay(state: RubberBandState) -> c_uint;
    pub fn rubberband_get_latency(state: RubberBandState) -> c_uint;
    pub fn rubberband_get_samples_required(state: RubberBandState) -> c_uint;
    pub fn rubberband_set_max_process_size(state: RubberBandState, samples: c_uint);
    pub fn rubberband_get_process_size_limit(state: RubberBandState) -> c_uint;
    pub fn rubberband_process(
        state: RubberBandState,
        input: *const *const f32,
        samples: c_uint,
        final_: c_int,
    );
    pub fn rubberband_available(state: RubberBandState) -> c_int;
    pub fn rubberband_retrieve(
        state: RubberBandState,
        output: *const *mut f32,
        samples: c_uint,
    ) -> c_uint;
    pub fn rubberband_get_channel_count(state: RubberBandState) -> c_uint;
    pub fn rubberband_set_default_debug_level(level: c_int);

    pub fn jk_rubberband_version() -> *const c_char;
    pub fn jk_rubberband_fft_implementations(buf: *mut c_char, len: c_int) -> c_int;
}

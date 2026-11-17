// SPDX-License-Identifier: GPL-3.0-or-later
//! Safe Rust wrapper around the vendored **Rubber Band Library 4.0.0** (GPL), restricted to
//! what Jukeoke needs: the *real-time* stretcher with the R3 ("finer") engine, used as a
//! pitch shifter (time ratio fixed at 1.0).
//!
//! Configuration (fixed, see [`StretcherConfig`]):
//! `OptionEngineFiner | OptionProcessRealTime | OptionChannelsTogether |
//!  OptionThreadingNever` plus a pitch option (High Quality by default).
//!
//! Timing contract (from RubberBandStretcher.h, verified by the impulse tests in jk-dsp):
//! feed [`Stretcher::preferred_start_pad`] samples before the first real input sample and
//! discard [`Stretcher::start_delay`] samples of output; then output sample *n* is aligned
//! with input sample *n*. Equivalently: raw output index `o` corresponds to raw input index
//! `o - start_delay + preferred_start_pad`.
//!
//! Real-time notes: construction allocates; `process`/`retrieve`/`available`/
//! `set_pitch_scale`/`reset` are intended to be real-time safe after construction when
//! called with at most `max_process_size` frames. Jukeoke calls them from the render-ahead
//! worker, never from the device callback.

pub mod ffi;

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint};
use std::ptr::NonNull;

/// The Rubber Band release vendored in `vendor/rubberband` (checked at run time against
/// the compiled C++ in the tests).
pub const RUBBERBAND_VERSION: &str = "4.0.0";

/// Upper bound for channels in one stretcher. Jukeoke uses stereo; this only sizes the
/// on-stack pointer arrays used to call the C API without allocating.
pub const MAX_CHANNELS: usize = 8;

/// How Rubber Band changes pitch in real-time mode (the `OptionPitch*` flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchMode {
    /// `OptionPitchHighQuality`: best quality for a *fixed* pitch scale. Resamples after
    /// the stretch when shifting up and before it when shifting down.
    HighQuality,
    /// `OptionPitchHighConsistency`: always resamples after the stretch (same method as
    /// Rubber Band's offline mode); designed for continuously changing pitch.
    HighConsistency,
    /// `OptionPitchHighSpeed`: cheapest. Not used by Jukeoke.
    HighSpeed,
}

/// Construction-time options for [`Stretcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StretcherConfig {
    pub sample_rate: u32,
    pub channels: usize,
    pub pitch_mode: PitchMode,
    /// `OptionFormantPreserved` (for voice); default off = formants shift with pitch,
    /// the natural behaviour for a band.
    pub preserve_formants: bool,
    /// `OptionWindowShort`: lower latency, lower quality. Default off.
    pub short_window: bool,
    /// Largest number of frames that will be passed to one `process` call.
    pub max_process_size: usize,
}

impl StretcherConfig {
    /// Jukeoke's defaults: stereo, R3, real-time, channels together, high-quality pitch.
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        Self {
            sample_rate,
            channels,
            pitch_mode: PitchMode::HighQuality,
            preserve_formants: false,
            short_window: false,
            max_process_size: 4096,
        }
    }

    /// The raw Rubber Band option bits this configuration selects.
    pub fn options(&self) -> c_int {
        let mut o = ffi::OPTION_ENGINE_FINER
            | ffi::OPTION_PROCESS_REAL_TIME
            | ffi::OPTION_CHANNELS_TOGETHER
            | ffi::OPTION_THREADING_NEVER
            | ffi::OPTION_TRANSIENTS_CRISP;
        o |= match self.pitch_mode {
            PitchMode::HighQuality => ffi::OPTION_PITCH_HIGH_QUALITY,
            PitchMode::HighConsistency => ffi::OPTION_PITCH_HIGH_CONSISTENCY,
            PitchMode::HighSpeed => ffi::OPTION_PITCH_HIGH_SPEED,
        };
        o |= if self.preserve_formants {
            ffi::OPTION_FORMANT_PRESERVED
        } else {
            ffi::OPTION_FORMANT_SHIFTED
        };
        o |= if self.short_window {
            ffi::OPTION_WINDOW_SHORT
        } else {
            ffi::OPTION_WINDOW_STANDARD
        };
        o
    }
}

/// Errors from [`Stretcher::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Parameters out of the supported range (checked before calling C++, because a C++
    /// exception must never cross the FFI boundary).
    InvalidConfig(&'static str),
    /// `rubberband_new` returned null.
    CreateFailed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidConfig(why) => write!(f, "invalid Rubber Band configuration: {why}"),
            Error::CreateFailed => write!(f, "Rubber Band failed to create a stretcher"),
        }
    }
}
impl std::error::Error for Error {}

/// `2^(k/12)` for k = -12..=12, correctly rounded to f64 (computed with 60-digit decimal
/// arithmetic). A table instead of `powf`, because platform `pow` implementations differ in
/// the last bit: measured on Windows (MinGW/msvcrt), `2f64.powf(k/12)` is one ulp off at
/// k = +8 and +11, and the pure-Rust `libm::pow` is one ulp off at other keys. The scale is
/// an input to Rubber Band, so Windows and macOS must pass it bit-identically.
const SEMITONE_SCALES: [f64; 25] = [
    0.5,                // -12
    0.5297315471796477, // -11
    0.5612310241546865, // -10
    0.5946035575013605, // -9
    0.6299605249474366, // -8
    0.6674199270850172, // -7
    std::f64::consts::FRAC_1_SQRT_2, // -6 (0.7071067811865476)
    0.7491535384383408, // -5
    0.7937005259840998, // -4
    0.8408964152537145, // -3
    0.8908987181403393, // -2
    0.9438743126816935, // -1
    1.0,                // 0
    1.0594630943592953, // +1
    1.122462048309373,  // +2
    1.189207115002721,  // +3
    1.2599210498948732, // +4
    1.3348398541700344, // +5
    std::f64::consts::SQRT_2, // +6 (1.4142135623730951)
    1.4983070768766815, // +7
    1.5874010519681996, // +8
    1.681792830507429,  // +9
    1.7817974362806785, // +10
    1.887748625363387,  // +11
    2.0,                // +12
];

/// Convert semitones to a Rubber Band pitch scale (`2^(semitones/12)`).
///
/// Whole semitones in -12..=12 (everything Jukeoke uses) come from a correctly rounded
/// table, so every platform gets the same bits; other values fall back to `f64::powf`.
pub fn semitones_to_scale(semitones: f64) -> f64 {
    if semitones.fract() == 0.0 && (-12.0..=12.0).contains(&semitones) {
        return SEMITONE_SCALES[(semitones as i32 + 12) as usize];
    }
    2f64.powf(semitones / 12.0)
}

/// A real-time Rubber Band R3 stretcher used as a pitch shifter.
pub struct Stretcher {
    state: NonNull<ffi::RubberBandState_>,
    config: StretcherConfig,
}

// The C++ object has no thread affinity; it is used from one thread at a time (&mut self).
unsafe impl Send for Stretcher {}

impl Stretcher {
    /// Create a stretcher at pitch scale 1.0 and time ratio 1.0.
    pub fn new(config: StretcherConfig) -> Result<Self, Error> {
        if config.channels == 0 || config.channels > MAX_CHANNELS {
            return Err(Error::InvalidConfig("channels must be 1..=8"));
        }
        if !(8_000..=384_000).contains(&config.sample_rate) {
            return Err(Error::InvalidConfig("sample rate must be 8000..=384000 Hz"));
        }
        if config.max_process_size == 0 || config.max_process_size > 1 << 20 {
            return Err(Error::InvalidConfig("max_process_size must be 1..=1048576"));
        }
        // SAFETY: plain C call with validated arguments.
        let raw = unsafe {
            // Quiet: warnings only (level 0 prints genuine misuse to stderr).
            ffi::rubberband_set_default_debug_level(0);
            ffi::rubberband_new(
                config.sample_rate as c_uint,
                config.channels as c_uint,
                config.options(),
                1.0,
                1.0,
            )
        };
        let state = NonNull::new(raw).ok_or(Error::CreateFailed)?;
        let mut s = Stretcher { state, config };
        let limit = s.process_size_limit();
        let max = config.max_process_size.min(limit);
        // SAFETY: valid state.
        unsafe { ffi::rubberband_set_max_process_size(s.raw(), max as c_uint) };
        s.config.max_process_size = max;
        Ok(s)
    }

    #[inline]
    fn raw(&self) -> ffi::RubberBandState {
        self.state.as_ptr()
    }

    pub fn config(&self) -> &StretcherConfig {
        &self.config
    }

    pub fn channels(&self) -> usize {
        self.config.channels
    }

    /// Rubber Band's engine version: 3 for R3 ("finer").
    pub fn engine_version(&self) -> i32 {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_engine_version(self.raw()) }
    }

    /// Set the pitch scale (frequency ratio, `> 0`). In real-time mode this may be called
    /// at any time; [`start_delay`](Self::start_delay) depends on it.
    pub fn set_pitch_scale(&mut self, scale: f64) {
        assert!(scale.is_finite() && scale > 0.0, "pitch scale must be finite and > 0");
        // SAFETY: valid state.
        unsafe { ffi::rubberband_set_pitch_scale(self.raw(), scale) }
    }

    pub fn pitch_scale(&self) -> f64 {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_pitch_scale(self.raw()) }
    }

    /// Number of samples to feed before the first real input sample (real-time mode).
    pub fn preferred_start_pad(&self) -> usize {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_preferred_start_pad(self.raw()) as usize }
    }

    /// Number of output samples to discard after padding the input with
    /// [`preferred_start_pad`](Self::preferred_start_pad) samples.
    pub fn start_delay(&self) -> usize {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_start_delay(self.raw()) as usize }
    }

    /// Legacy latency query (`getLatency`); kept for diagnostics only.
    pub fn latency(&self) -> usize {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_latency(self.raw()) as usize }
    }

    /// Samples Rubber Band would like before it can produce more output.
    pub fn samples_required(&self) -> usize {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_samples_required(self.raw()) as usize }
    }

    /// The largest `process` block this instance accepts (after construction clamping).
    pub fn max_process_size(&self) -> usize {
        self.config.max_process_size
    }

    fn process_size_limit(&self) -> usize {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_get_process_size_limit(self.raw()) as usize }
    }

    /// Return to the freshly constructed state (keeps options, pitch scale and allocations).
    pub fn reset(&mut self) {
        // SAFETY: valid state.
        unsafe { ffi::rubberband_reset(self.raw()) }
    }

    /// Feed `frames` frames of planar input (`input[ch][0..frames]`).
    ///
    /// Panics if the channel count is wrong, a channel is shorter than `frames`, or
    /// `frames > max_process_size`.
    pub fn process(&mut self, input: &[&[f32]], frames: usize, final_block: bool) {
        let ch = self.config.channels;
        assert_eq!(input.len(), ch, "wrong number of input channels");
        assert!(frames <= self.config.max_process_size, "block larger than max_process_size");
        let mut ptrs: [*const f32; MAX_CHANNELS] = [std::ptr::null(); MAX_CHANNELS];
        for (c, buf) in input.iter().enumerate() {
            assert!(buf.len() >= frames, "input channel shorter than frames");
            ptrs[c] = buf.as_ptr();
        }
        // SAFETY: `ptrs[..ch]` point at `frames` readable floats each; state valid.
        unsafe {
            ffi::rubberband_process(
                self.raw(),
                ptrs.as_ptr(),
                frames as c_uint,
                final_block as c_int,
            )
        }
    }

    /// Frames ready to retrieve. `None` when the stretcher has finished (only after a final
    /// block, which Jukeoke never sends in real-time use).
    pub fn available(&self) -> Option<usize> {
        // SAFETY: valid state.
        let n = unsafe { ffi::rubberband_available(self.raw()) };
        if n < 0 { None } else { Some(n as usize) }
    }

    /// Retrieve up to `output[ch].len()` frames (all channels must have the same length).
    /// Returns the number of frames written.
    pub fn retrieve(&mut self, output: &mut [&mut [f32]]) -> usize {
        let ch = self.config.channels;
        assert_eq!(output.len(), ch, "wrong number of output channels");
        let frames = output[0].len();
        let mut ptrs: [*mut f32; MAX_CHANNELS] = [std::ptr::null_mut(); MAX_CHANNELS];
        for (c, buf) in output.iter_mut().enumerate() {
            assert_eq!(buf.len(), frames, "output channels must have equal length");
            ptrs[c] = buf.as_mut_ptr();
        }
        // SAFETY: `ptrs[..ch]` point at `frames` writable floats each; state valid.
        unsafe { ffi::rubberband_retrieve(self.raw(), ptrs.as_ptr(), frames as c_uint) as usize }
    }
}

impl Drop for Stretcher {
    fn drop(&mut self) {
        // SAFETY: state came from rubberband_new and is deleted once.
        unsafe { ffi::rubberband_delete(self.raw()) }
    }
}

/// `RUBBERBAND_VERSION` as compiled into the C++ (should equal [`RUBBERBAND_VERSION`]).
pub fn compiled_version() -> String {
    // SAFETY: returns a pointer to a static NUL-terminated string.
    unsafe { CStr::from_ptr(ffi::jk_rubberband_version()) }
        .to_string_lossy()
        .into_owned()
}

/// The FFT implementations compiled into Rubber Band, comma separated. Jukeoke requires
/// exactly `"builtin,dft"` on every platform: `builtin` serves every power-of-two size (all
/// R3 FFTs), `dft` is the always-present naive fallback for other sizes. In particular no
/// `vdsp` on macOS, so Windows and macOS run the same FFT code.
pub fn compiled_fft_implementations() -> String {
    let mut buf = [0 as c_char; 256];
    // SAFETY: buffer is valid for 256 bytes; the helper NUL-terminates.
    unsafe {
        ffi::jk_rubberband_fft_implementations(buf.as_mut_ptr(), buf.len() as c_int);
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_backend() {
        assert_eq!(compiled_version(), RUBBERBAND_VERSION);
        assert_eq!(compiled_fft_implementations(), "builtin,dft");
    }

    #[test]
    fn semitone_table_is_correctly_rounded() {
        // Bit patterns of the correctly rounded values (independently computed).
        let bits: [u64; 13] = [
            0x3fe0000000000000, 0x3fe0f38f92d97963, 0x3fe1f59ac3c7d6c0, 0x3fe306fe0a31b715,
            0x3fe428a2f98d728b, 0x3fe55b8108f0ec5e, 0x3fe6a09e667f3bcd, 0x3fe7f910d768cfb0,
            0x3fe965fea53d6e3d, 0x3feae89f995ad3ad, 0x3fec823e074ec129, 0x3fee3437e7101344,
            0x3ff0000000000000,
        ];
        for k in -12..=0 {
            let lo = semitones_to_scale(k as f64);
            assert_eq!(lo.to_bits(), bits[(k + 12) as usize], "k={k}");
            // Doubling is exact in binary floating point: 2^((k+12)/12) = 2 * 2^(k/12).
            assert_eq!(semitones_to_scale((k + 12) as f64), 2.0 * lo, "k={}", k + 12);
            // Within one ulp of the platform pow.
            let p = 2f64.powf(k as f64 / 12.0);
            assert!((lo.to_bits() as i64 - p.to_bits() as i64).abs() <= 1, "k={k}");
        }
        // Non-integer shifts still work.
        assert!((semitones_to_scale(0.5) - 2f64.powf(0.5 / 12.0)).abs() < 1e-15);
    }

    #[test]
    fn creates_r3_and_reports_delays() {
        for &rate in &[44_100u32, 48_000] {
            let mut s = Stretcher::new(StretcherConfig::new(rate, 2)).unwrap();
            assert_eq!(s.engine_version(), 3, "R3 (finer) engine expected");
            for semis in -12..=12 {
                s.set_pitch_scale(semitones_to_scale(semis as f64));
                let pad = s.preferred_start_pad();
                let delay = s.start_delay();
                assert!(pad > 0 && delay > 0, "rate {rate} semis {semis}");
            }
        }
    }

    #[test]
    fn passes_audio_through() {
        let rate = 48_000;
        let mut s = Stretcher::new(StretcherConfig::new(rate, 2)).unwrap();
        s.set_pitch_scale(semitones_to_scale(3.0));
        let block = 512;
        let input: Vec<f32> = (0..block)
            .map(|i| (i as f32 * 0.05).sin() * 0.25)
            .collect();
        let mut l = vec![0.0f32; 4096];
        let mut r = vec![0.0f32; 4096];
        let mut got = 0usize;
        for _ in 0..100 {
            s.process(&[&input, &input], block, false);
            while let Some(n) = s.available() {
                if n == 0 {
                    break;
                }
                let n = n.min(4096);
                let mut outs: [&mut [f32]; 2] = [&mut l[..n], &mut r[..n]];
                got += s.retrieve(&mut outs);
            }
        }
        // After 100 blocks, nearly everything fed (minus latency) has come out.
        assert!(got > 100 * block - 16_384, "got {got}");
    }
}

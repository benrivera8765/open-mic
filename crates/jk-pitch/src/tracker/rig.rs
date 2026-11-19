// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated "owner's rig" effects for testing the tracker (WORK_ORDER-2 4.10 and section 9,
//! row "The owner's rig"): what a podcast mixer and a handheld dynamic microphone may do to
//! the voice before the app hears it.
//!
//! - [`room_ir`] / [`reverb`]: a generated room impulse response (direct sound, a few early
//!   reflections and an exponentially decaying, slowly darkening noise tail with a given
//!   RT60) convolved with the input, the wet part at a chosen level against the dry voice.
//! - [`compress`]: a feed-forward compressor (threshold, ratio, attack, release, make-up)
//!   followed by a fast limiter, as a mixer's "voice" compressor; it lifts breaths and the
//!   room's noise floor (WO2 trap 7).
//! - [`Eq`] and [`mixer_eq`]: RBJ-cookbook shelves and peaks, as a mixer's channel EQ.
//! - [`proximity`]: the bass boost of a directional microphone sung into at close range.
//! - [`add_handling`]: popped plosives and handling thumps dropped into the signal at random
//!   places ([`super::testvoice::plosive`], [`super::testvoice::thump`]).
//!
//! Test fixtures only (they allocate freely); nothing here runs on the live path. All of it
//! is deterministic for a given seed.

use std::f64::consts::PI;

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

use super::testvoice::{self as tv, Rng};

/// A generated room impulse response at `rate` Hz with reverberation time `rt60` seconds
/// (time for the tail to decay by 60 dB). Sample 0 is the direct sound (1.0); six early
/// reflections follow within 3..25 ms, then a noise tail from 8 ms on whose high
/// frequencies decay faster than its lows (air and wall absorption). `wet_db` is the energy
/// of everything after the direct sound relative to the direct sound (minus the
/// direct-to-reverberant ratio): -6 is a clearly reverberant mixer effect, 0 is as much
/// reverb as voice. Length 1.2 x `rt60`.
pub fn room_ir(rate: u32, rt60: f64, wet_db: f64, seed: u64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = ((1.2 * rt60 * fs) as usize).max(16);
    let mut rng = Rng::new(seed);
    let mut wet = vec![0.0f64; n];
    // Early reflections.
    for _ in 0..6 {
        let at = ((0.003 + 0.022 * rng.unit()) * fs) as usize;
        let sign = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
        if at < n {
            wet[at] += sign * (0.3 + 0.3 * rng.unit());
        }
    }
    // Tail: Gaussian-like noise (sum of three uniforms) under an exponential decay, through
    // a one-pole low-pass whose corner falls from 8 kHz to 1.5 kHz over the tail.
    let start = (0.008 * fs) as usize;
    let mut lp = 0.0f64;
    for (i, w) in wet.iter_mut().enumerate().skip(start) {
        let t = (i - start) as f64 / fs;
        let g = (-6.907_755 * t / rt60).exp();
        let corner = 1500.0 + 6500.0 * (-3.0 * t / rt60).exp();
        let a = 1.0 - (-2.0 * PI * corner.min(0.45 * fs) / fs).exp();
        let z = (rng.bipolar() + rng.bipolar() + rng.bipolar()) / 3.0;
        lp += a * (z - lp);
        *w += g * lp;
    }
    let energy: f64 = wet.iter().map(|v| v * v).sum();
    let gain = if energy > 0.0 {
        (10f64.powf(wet_db / 10.0) / energy).sqrt()
    } else {
        0.0
    };
    let mut ir: Vec<f32> = wet.iter().map(|v| (v * gain) as f32).collect();
    ir[0] += 1.0;
    ir
}

/// Linear convolution of `x` with `h` by FFT, trimmed to the length of `x` (the reverb
/// tail after the end of the input is dropped, so the output lines up with the input).
pub fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    if x.is_empty() || h.is_empty() {
        return vec![0.0; x.len()];
    }
    let n = (x.len() + h.len() - 1).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut a = fwd.make_input_vec();
    let mut b = fwd.make_input_vec();
    for (d, s) in a.iter_mut().zip(x) {
        *d = f64::from(*s);
    }
    for (d, s) in b.iter_mut().zip(h) {
        *d = f64::from(*s);
    }
    let mut fa = fwd.make_output_vec();
    let mut fb = fwd.make_output_vec();
    fwd.process(&mut a, &mut fa).expect("fft");
    fwd.process(&mut b, &mut fb).expect("fft");
    for (p, q) in fa.iter_mut().zip(&fb) {
        *p *= *q;
    }
    // DC and Nyquist bins of a real product are real; clear rounding residue.
    let last = fa.len() - 1;
    fa[0] = Complex::new(fa[0].re, 0.0);
    fa[last] = Complex::new(fa[last].re, 0.0);
    let mut y = inv.make_output_vec();
    inv.process(&mut fa, &mut y).expect("ifft");
    let scale = 1.0 / n as f64;
    y[..x.len()].iter().map(|v| (v * scale) as f32).collect()
}

/// `x` through a generated room (see [`room_ir`]).
pub fn reverb(x: &[f32], rate: u32, rt60: f64, wet_db: f64, seed: u64) -> Vec<f32> {
    convolve(x, &room_ir(rate, rt60, wet_db, seed))
}

/// Compressor settings.
#[derive(Clone, Copy, Debug)]
pub struct Compressor {
    /// Level above which the gain is reduced, dBFS (of the detector's RMS-like envelope).
    pub threshold_dbfs: f64,
    /// Compression ratio above the threshold.
    pub ratio: f64,
    /// Attack and release time constants of the level detector, seconds.
    pub attack_s: f64,
    pub release_s: f64,
    /// Gain after compression, dB.
    pub makeup_db: f64,
    /// Ceiling of the limiter after the compressor, dBFS.
    pub ceiling_dbfs: f64,
}

impl Compressor {
    /// A typical voice compressor on a mixer: -30 dBFS, 4:1, 5 / 120 ms, +12 dB, limiter at
    /// -1 dBFS.
    pub const MODERATE: Self = Self {
        threshold_dbfs: -30.0,
        ratio: 4.0,
        attack_s: 0.005,
        release_s: 0.120,
        makeup_db: 12.0,
        ceiling_dbfs: -1.0,
    };
    /// A podcast mixer's "broadcast" voice: -45 dBFS, 10:1, 2 / 80 ms, +24 dB: breaths and
    /// the room come up close to the voice.
    pub const HEAVY: Self = Self {
        threshold_dbfs: -45.0,
        ratio: 10.0,
        attack_s: 0.002,
        release_s: 0.080,
        makeup_db: 24.0,
        ceiling_dbfs: -1.0,
    };
}

/// `x` through a feed-forward compressor and a limiter (see [`Compressor`]).
pub fn compress(x: &[f32], rate: u32, c: Compressor) -> Vec<f32> {
    let fs = f64::from(rate);
    let att = (-1.0 / (c.attack_s * fs)).exp();
    let rel = (-1.0 / (c.release_s * fs)).exp();
    let lim_att = (-1.0 / (0.0005 * fs)).exp();
    let lim_rel = (-1.0 / (0.050 * fs)).exp();
    let ceiling = 10f64.powf(c.ceiling_dbfs / 20.0);
    let makeup = 10f64.powf(c.makeup_db / 20.0);
    let mut env = 0.0f64; // mean square
    let mut lim_gain = 1.0f64;
    x.iter()
        .map(|&s| {
            let s = f64::from(s);
            let p = s * s;
            env = if p > env {
                att * env + (1.0 - att) * p
            } else {
                rel * env + (1.0 - rel) * p
            };
            let level_db = 10.0 * env.max(1e-20).log10();
            let over = level_db - c.threshold_dbfs;
            let gain_db = if over > 0.0 {
                -over * (1.0 - 1.0 / c.ratio)
            } else {
                0.0
            };
            let y = s * 10f64.powf(gain_db / 20.0) * makeup;
            // Limiter: instant-ish attack, 50 ms release, then a hard clip at the ceiling for
            // what the 0.5 ms attack lets through.
            let need = if y.abs() > ceiling {
                ceiling / y.abs()
            } else {
                1.0
            };
            lim_gain = if need < lim_gain {
                lim_att * lim_gain + (1.0 - lim_att) * need
            } else {
                lim_rel * lim_gain + (1.0 - lim_rel) * need
            };
            (y * lim_gain).clamp(-ceiling, ceiling) as f32
        })
        .collect()
}

/// One RBJ-cookbook equaliser section.
#[derive(Clone, Copy, Debug)]
pub enum Eq {
    /// Low shelf: gain in dB below `hz`.
    LowShelf { hz: f64, db: f64 },
    /// High shelf: gain in dB above `hz`.
    HighShelf { hz: f64, db: f64 },
    /// Peak / dip of `db` at `hz` with quality `q`.
    Peak { hz: f64, db: f64, q: f64 },
    /// Second-order high-pass (a mixer's low cut).
    LowCut { hz: f64 },
}

/// `x` through the EQ sections in order.
pub fn equalise(x: &[f32], rate: u32, sections: &[Eq]) -> Vec<f32> {
    let fs = f64::from(rate);
    let mut y: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    for s in sections {
        let (b, a) = coefficients(fs, *s);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in &mut y {
            let out = b[0] * *v + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2;
            x2 = x1;
            x1 = *v;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    }
    y.into_iter().map(|v| v as f32).collect()
}

/// Normalised biquad coefficients (b0, b1, b2), (1, a1, a2) of an [`Eq`] section.
fn coefficients(fs: f64, s: Eq) -> ([f64; 3], [f64; 3]) {
    let shelf = |hz: f64, db: f64, high: bool| {
        let a = 10f64.powf(db / 40.0);
        let w0 = 2.0 * PI * hz / fs;
        let (sn, cs) = w0.sin_cos();
        // Shelf slope S = 1.
        let alpha = sn / 2.0 * 2f64.sqrt();
        let k = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = if high {
            (
                a * ((a + 1.0) + (a - 1.0) * cs + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - k),
                (a + 1.0) - (a - 1.0) * cs + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - k,
            )
        } else {
            (
                a * ((a + 1.0) - (a - 1.0) * cs + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - k),
                (a + 1.0) + (a - 1.0) * cs + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - k,
            )
        };
        ([b0 / a0, b1 / a0, b2 / a0], [1.0, a1 / a0, a2 / a0])
    };
    match s {
        Eq::LowShelf { hz, db } => shelf(hz, db, false),
        Eq::HighShelf { hz, db } => shelf(hz, db, true),
        Eq::Peak { hz, db, q } => {
            let a = 10f64.powf(db / 40.0);
            let w0 = 2.0 * PI * hz / fs;
            let (sn, cs) = w0.sin_cos();
            let alpha = sn / (2.0 * q);
            let a0 = 1.0 + alpha / a;
            (
                [
                    (1.0 + alpha * a) / a0,
                    -2.0 * cs / a0,
                    (1.0 - alpha * a) / a0,
                ],
                [1.0, -2.0 * cs / a0, (1.0 - alpha / a) / a0],
            )
        }
        Eq::LowCut { hz } => {
            let w0 = 2.0 * PI * hz / fs;
            let (sn, cs) = w0.sin_cos();
            let alpha = sn / 2f64.sqrt();
            let a0 = 1.0 + alpha;
            (
                [
                    (1.0 + cs) / 2.0 / a0,
                    -(1.0 + cs) / a0,
                    (1.0 + cs) / 2.0 / a0,
                ],
                [1.0, -2.0 * cs / a0, (1.0 - alpha) / a0],
            )
        }
    }
}

/// Two mixer channel-EQ settings a singer might leave on: "bright" (low cut at 80 Hz, a
/// small dip at 300 Hz, +5 dB presence at 3 kHz, +6 dB air above 8 kHz) and "warm"
/// (+6 dB below 150 Hz, -3 dB at 2.5 kHz, -6 dB above 6 kHz).
pub fn mixer_eq(x: &[f32], rate: u32, bright: bool) -> Vec<f32> {
    let air = (0.4 * f64::from(rate)).min(8000.0);
    if bright {
        equalise(
            x,
            rate,
            &[
                Eq::LowCut { hz: 80.0 },
                Eq::Peak {
                    hz: 300.0,
                    db: -3.0,
                    q: 1.0,
                },
                Eq::Peak {
                    hz: 3000.0,
                    db: 5.0,
                    q: 1.0,
                },
                Eq::HighShelf { hz: air, db: 6.0 },
            ],
        )
    } else {
        equalise(
            x,
            rate,
            &[
                Eq::LowShelf {
                    hz: 150.0,
                    db: 6.0,
                },
                Eq::Peak {
                    hz: 2500.0,
                    db: -3.0,
                    q: 1.0,
                },
                Eq::HighShelf {
                    hz: (0.4 * f64::from(rate)).min(6000.0),
                    db: -6.0,
                },
            ],
        )
    }
}

/// The proximity effect of a handheld dynamic cardioid microphone sung into at a few
/// centimetres: a low shelf of `db` (about +10 to +15 dB) below 200 Hz.
pub fn proximity(x: &[f32], rate: u32, db: f64) -> Vec<f32> {
    equalise(x, rate, &[Eq::LowShelf { hz: 200.0, db }])
}

/// Adds `pops` popped plosives (peaks -3..-12 dBFS) and `thumps` handling thumps (40..200 Hz,
/// peaks -1..-12 dBFS) at random places in `x`. Returns the positions (in samples) of
/// everything added, for checks.
pub fn add_handling(x: &mut [f32], rate: u32, pops: usize, thumps: usize, seed: u64) -> Vec<usize> {
    let mut rng = Rng::new(seed);
    let fs = f64::from(rate);
    let len_s = 0.15;
    let span = x.len().saturating_sub((len_s * fs) as usize);
    let mut at = Vec::new();
    if span == 0 {
        return at;
    }
    for k in 0..pops + thumps {
        let start = (rng.unit() * span as f64) as usize;
        let peak = -3.0 - 9.0 * rng.unit();
        let burst = if k < pops {
            tv::plosive(rate, len_s, 0.0, peak, seed.wrapping_add(k as u64))
        } else {
            let hz = 40.0 + 160.0 * rng.unit();
            tv::thump(rate, len_s, 0.0, hz, peak + 2.0)
        };
        for (d, s) in x[start..].iter_mut().zip(&burst) {
            *d += *s;
        }
        at.push(start);
    }
    at
}

/// Adds brownish room noise at `level_dbfs` RMS (see [`super::testvoice::room_noise`]).
pub fn add_noise_floor(x: &mut [f32], rate: u32, level_dbfs: f64, seed: u64) {
    let seconds = x.len() as f64 / f64::from(rate);
    let n = tv::room_noise(rate, seconds, level_dbfs, None, seed);
    for (d, s) in x.iter_mut().zip(n) {
        *d += s;
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Measurement helpers used by the tests and `jukeoke-cli bench` (not used for playback).

use core::f64::consts::PI;

/// 4-term Blackman-Harris window (sidelobes about -92 dB).
pub fn blackman_harris(n: usize) -> Vec<f64> {
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    let m = (n - 1) as f64;
    (0..n)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / m;
            a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()
        })
        .collect()
}

/// |DTFT| of `x * w` at frequency `f` (Hz).
pub fn dtft_mag(x: &[f32], w: &[f64], rate: f64, f: f64) -> f64 {
    let d = 2.0 * PI * f / rate;
    let (c, s) = (d.cos(), d.sin());
    let (mut pr, mut pi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, (&v, &wv)) in x.iter().zip(w).enumerate() {
        let a = v as f64 * wv;
        re += a * pr;
        im -= a * pi;
        let npr = pr * c - pi * s;
        let npi = pr * s + pi * c;
        pr = npr;
        pi = npi;
        if n & 1023 == 1023 {
            let norm = (pr * pr + pi * pi).sqrt();
            pr /= norm;
            pi /= norm;
        }
    }
    (re * re + im * im).sqrt()
}

/// Estimate the frequency of the strongest component within `+-search_cents` of `guess`:
/// coarse scan in 5-cent steps, then golden-section search to 0.0001 cent.
pub fn estimate_frequency(x: &[f32], rate: u32, guess: f64, search_cents: f64) -> f64 {
    let w = blackman_harris(x.len());
    let rate = rate as f64;
    let f_of = |cents: f64| guess * (cents / 1200.0).exp2();
    let mag = |cents: f64| dtft_mag(x, &w, rate, f_of(cents));
    let mut best = (f64::MIN, 0.0);
    let mut c = -search_cents;
    while c <= search_cents + 1e-9 {
        let m = mag(c);
        if m > best.0 {
            best = (m, c);
        }
        c += 5.0;
    }
    let (mut lo, mut hi) = (best.1 - 5.0, best.1 + 5.0);
    let g = (5f64.sqrt() - 1.0) / 2.0;
    let mut x1 = hi - g * (hi - lo);
    let mut x2 = lo + g * (hi - lo);
    let mut f1 = mag(x1);
    let mut f2 = mag(x2);
    while hi - lo > 1e-4 {
        if f1 < f2 {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + g * (hi - lo);
            f2 = mag(x2);
        } else {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - g * (hi - lo);
            f1 = mag(x1);
        }
    }
    f_of((lo + hi) / 2.0)
}

/// Pitch difference in cents.
pub fn cents(measured: f64, reference: f64) -> f64 {
    1200.0 * (measured / reference).log2()
}

pub fn peak_abs(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

pub fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Largest |x[n] - x[n-1]| in `range`.
pub fn max_first_diff(x: &[f32], range: std::ops::Range<usize>) -> f32 {
    let lo = range.start.max(1);
    (lo..range.end.min(x.len())).fold(0.0f32, |m, n| m.max((x[n] - x[n - 1]).abs()))
}

/// Largest |x[n] - 2 x[n-1] + x[n-2]| in `range` (a click detector: small for smooth
/// signals, large at any step or kink).
pub fn max_second_diff(x: &[f32], range: std::ops::Range<usize>) -> f32 {
    let lo = range.start.max(2);
    (lo..range.end.min(x.len())).fold(0.0f32, |m, n| m.max((x[n] - 2.0 * x[n - 1] + x[n - 2]).abs()))
}

/// RMS envelope over a centred window of `win` samples (computed with a running sum).
pub fn rms_envelope(x: &[f32], win: usize) -> Vec<f32> {
    let n = x.len();
    let mut pre = vec![0.0f64; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + (x[i] as f64).powi(2);
    }
    let h = win / 2;
    (0..n)
        .map(|i| {
            let a = i.saturating_sub(h);
            let b = (i + h + 1).min(n);
            ((pre[b] - pre[a]) / (b - a) as f64).sqrt() as f32
        })
        .collect()
}

/// Onset = first index in `range` where the 1 ms RMS envelope reaches `frac` of its
/// maximum within the range.
pub fn onset(x: &[f32], rate: u32, range: std::ops::Range<usize>, frac: f32) -> Option<usize> {
    let env = rms_envelope(&x[range.clone()], (rate as usize / 1000).max(3));
    let max = env.iter().cloned().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return None;
    }
    env.iter().position(|&e| e >= max * frac).map(|p| p + range.start)
}

/// Index of the largest |x| in `range`.
pub fn argmax_abs(x: &[f32], range: std::ops::Range<usize>) -> usize {
    let mut best = (0.0f32, range.start);
    for i in range {
        if x[i].abs() > best.0 {
            best = (x[i].abs(), i);
        }
    }
    best.1
}

/// Where the pitch of a steady tone changes from `f_old` to `f_new` (a key change, as
/// heard in rendered or played audio): the sample index where the two tones' windowed
/// energies, each divided by its own steady level, are equal. For the engine's
/// equal-power crossfade that is the crossfade's midpoint, whatever the window length,
/// because both the fade and the window are symmetric (Work Order 2, 4.6: the key in
/// effect must match the pitch really heard).
///
/// `around` is where the change is expected. The steady levels are measured one window
/// plus `margin` samples before and after it, and the crossing is searched between those
/// two points in steps of `hop` samples (linear interpolation between steps). The window
/// (4-term Blackman-Harris) is long enough to separate the two frequencies: about six
/// times the inverse of their difference, at least 1024 samples. Returns `None` if `x`
/// is too short around `around` or no crossing is found.
pub fn tone_switch_index(x: &[f32], rate: u32, f_old: f64, f_new: f64, around: usize, margin: usize, hop: usize) -> Option<f64> {
    let r = rate as f64;
    let df = (f_new - f_old).abs().max(1.0);
    let n = ((6.0 * r / df).ceil() as usize).clamp(1024, rate as usize);
    let w = blackman_harris(n);
    let half = n / 2;
    let reach = half + margin;
    if around < reach + half || around + reach + half > x.len() || hop == 0 {
        return None;
    }
    let energies = |c: usize| -> (f64, f64) {
        let seg = &x[c - half..c - half + n];
        (dtft_mag(seg, &w, r, f_old).powi(2), dtft_mag(seg, &w, r, f_new).powi(2))
    };
    let old_ref = energies(around - reach).0.max(1e-30);
    let new_ref = energies(around + reach).1.max(1e-30);
    // d(c) > 0 while the old tone dominates, < 0 once the new one does.
    let d = |c: usize| -> f64 {
        let (a, b) = energies(c);
        a / old_ref - b / new_ref
    };
    let (mut c0, mut d0) = (around - reach, d(around - reach));
    let mut c = c0 + hop;
    while c <= around + reach {
        let d1 = d(c);
        if d0 > 0.0 && d1 <= 0.0 {
            return Some(c0 as f64 + hop as f64 * d0 / (d0 - d1));
        }
        (c0, d0) = (c, d1);
        c += hop;
    }
    None
}

/// Result of [`impulse_alignment`]: where impulses sent through the band (pitch) path come
/// out, relative to the same impulses sent through the unshifted drum path.
#[derive(Clone, Debug)]
pub struct Alignment {
    /// Per impulse: (output peak index) - (drum impulse index), in samples.
    pub offsets: Vec<i64>,
    /// Peak position of the drum-path impulse minus its input position (must be 0).
    pub drum_offsets: Vec<i64>,
}

impl Alignment {
    pub fn mean(&self) -> f64 {
        self.offsets.iter().sum::<i64>() as f64 / self.offsets.len() as f64
    }
    pub fn min(&self) -> i64 {
        *self.offsets.iter().min().unwrap()
    }
    pub fn max(&self) -> i64 {
        *self.offsets.iter().max().unwrap()
    }
}

/// Send `count` isolated impulses (amplitude 0.5, 8192 samples apart, sub-positions
/// stepping by `stride` samples modulo 1024 so they sample every position of Rubber Band's
/// hop grid) through the full graph at `semitones`: once in the backing stem (pitch path)
/// and once in the drum stem (unshifted path, separate render). Peak positions are compared.
/// The trim table was calibrated with stride 17; the tests validate with other strides.
pub fn impulse_alignment(rate: u32, semitones: i32, count: usize, stride: usize) -> Result<Alignment, crate::DspError> {
    use crate::params::{MixParams, Stem};
    use crate::render::{render, RenderOptions, StemsRef};
    let spacing = 8192usize;
    let lead_in = 8192usize;
    let frames = lead_in + count * spacing + 8192;
    let positions: Vec<usize> = (0..count).map(|k| lead_in + k * spacing + (k * stride) % 1024).collect();
    let mut imp = vec![0.0f32; frames];
    for &p in &positions {
        imp[p] = 0.5;
    }
    let zeros = vec![0.0f32; frames];
    let params = MixParams { semitones, ..MixParams::default() };
    let run = |stem: Stem| -> Result<Vec<f32>, crate::DspError> {
        let mut st: [[&[f32]; 2]; 4] = [[&zeros[..], &zeros[..]]; 4];
        st[stem as usize] = [&imp[..], &imp[..]];
        let out = render(
            StemsRef { sample_rate: rate, frames, stems: st },
            params,
            &[],
            &RenderOptions::native(rate),
        )?;
        Ok(out.channels[0].clone())
    };
    let band = run(Stem::Backing)?;
    let drums = run(Stem::Drums)?;
    let mut offsets = Vec::with_capacity(count);
    let mut drum_offsets = Vec::with_capacity(count);
    for &p in &positions {
        let w = 3000usize;
        let d = argmax_abs(&drums, p - w..p + w);
        let b = argmax_abs(&band, p - w..p + w);
        drum_offsets.push(d as i64 - p as i64);
        offsets.push(b as i64 - d as i64);
    }
    Ok(Alignment { offsets, drum_offsets })
}

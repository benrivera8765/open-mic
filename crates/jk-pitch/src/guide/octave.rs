// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent octave check of a note (WO2 4.1 and trap 8).
//!
//! The tracker decided the pitch in the time domain (YIN lags). This check looks at the
//! note's average magnitude spectrum instead and asks two questions a wrong octave would
//! answer differently:
//!
//! - Is there harmonic energy halfway between the note's harmonics (at f/2, 3f/2, 5f/2...)?
//!   Then the voice's real fundamental is probably f/2: the guide is an octave HIGH.
//! - Are the odd harmonics (f, 3f, 5f...) missing while the even ones are there? Then the
//!   real fundamental is probably 2f: the guide is an octave LOW.

use super::OctaveCheck;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Analysis window: at least 64 ms and 8 periods of the note, so that the half-way
/// partials (f/2 apart from the harmonics) are resolved even for C2; at most 160 ms.
const WIN_MIN: f64 = 0.064;
const WIN_MAX: f64 = 0.160;
const N_FFT: usize = 8192;
/// The floor never sits more than this far (dB) below the strongest harmonic, so a clean
/// generated tone's window leakage does not count as a partial.
const MAX_RANGE_DB: f64 = 60.0;
/// Octave high when the half-way partials reach this share of the harmonics' mean level
/// (dB above the spectral floor).
const HALVES_SHARE: f64 = 0.7;
/// Octave low when the odd harmonics stay below this share of the even ones' mean level.
const ODD_SHARE: f64 = 0.4;

pub struct OctaveChecker {
    fft: Arc<dyn RealToComplex<f64>>,
    sr: f64,
}

impl OctaveChecker {
    pub fn new(sr: f64) -> Self {
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N_FFT);
        OctaveChecker { fft, sr }
    }

    /// Checks the note sung from `start_s` to `end_s` at `midi` in `x` (the analysis signal).
    pub fn check(&self, x: &[f32], start_s: f64, end_s: f64, midi: f64) -> OctaveCheck {
        let f0 = super::midi_to_hz(midi);
        let win_s = (8.0 / f0).clamp(WIN_MIN, WIN_MAX);
        let win = ((win_s * self.sr).round() as usize).min(N_FFT);
        let a = (start_s * self.sr).max(0.0) as usize;
        let b = ((end_s * self.sr) as usize).min(x.len());
        if b <= a || b - a < win / 2 {
            return OctaveCheck::Unchecked;
        }
        // Average magnitude spectrum over Hann windows (half overlap) covering the note.
        let mut input = self.fft.make_input_vec();
        let mut spec = self.fft.make_output_vec();
        let mut mag = vec![0.0f64; N_FFT / 2 + 1];
        let hop = win / 2;
        let mut starts = Vec::new();
        if b - a <= win {
            starts.push(((a + b) / 2).saturating_sub(win / 2));
        } else {
            let mut s = a;
            while s + win <= b {
                starts.push(s);
                s += hop;
            }
        }
        for &s in &starts {
            input.iter_mut().for_each(|v| *v = 0.0);
            for (j, v) in input.iter_mut().enumerate().take(win) {
                if let Some(&xi) = x.get(s + j) {
                    let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * j as f64 / win as f64).cos();
                    *v = xi as f64 * w;
                }
            }
            if self.fft.process(&mut input, &mut spec).is_err() {
                return OctaveCheck::Unchecked;
            }
            for (m, c) in mag.iter_mut().zip(&spec) {
                *m += c.norm();
            }
        }
        let bin_hz = self.sr / N_FFT as f64;
        let limit = (0.45 * self.sr).min(4000.0);
        // Spectral floor: the median magnitude between f0/2 and the limit.
        let lo_bin = ((0.5 * f0) / bin_hz) as usize;
        let hi_bin = ((limit / bin_hz) as usize).min(mag.len() - 1);
        if hi_bin <= lo_bin + 4 {
            return OctaveCheck::Unchecked;
        }
        let mut band: Vec<f64> = mag[lo_bin..hi_bin].to_vec();
        band.sort_by(|p, q| p.total_cmp(q));
        let strongest = (1..=10)
            .map(|h| h as f64 * f0)
            .take_while(|&f| f <= limit)
            .map(|f| {
                let c = (f / bin_hz).round() as usize;
                mag[c.saturating_sub(2)..=(c + 2).min(mag.len() - 1)]
                    .iter()
                    .copied()
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max);
        let floor = band[band.len() / 2].max(strongest * 10f64.powf(-MAX_RANGE_DB / 20.0));
        // Peak level in dB above the floor: a log scale, so a weak but clearly present
        // fundamental (a voice whose second harmonic dominates) still counts.
        let amp = |f: f64| -> f64 {
            let c = f / bin_hz;
            let w = (0.02 * c).max(1.5);
            let lo = (c - w).floor().max(0.0) as usize;
            let hi = ((c + w).ceil() as usize).min(mag.len() - 1);
            let peak = mag[lo..=hi].iter().copied().fold(0.0, f64::max);
            (20.0 * (peak / floor.max(1e-12)).log10()).max(0.0)
        };
        let harmonics: Vec<f64> = (1..=10)
            .map(|h| h as f64 * f0)
            .take_while(|&f| f <= limit)
            .map(amp)
            .collect();
        if harmonics.len() < 2 {
            return OctaveCheck::Unchecked;
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let all = mean(&harmonics);
        if all <= 0.0 {
            return OctaveCheck::Unchecked;
        }
        let halves: Vec<f64> = (0..10)
            .map(|m| (2 * m + 1) as f64 * f0 / 2.0)
            .take_while(|&f| f <= limit)
            .map(amp)
            .collect();
        if mean(&halves) > HALVES_SHARE * all {
            return OctaveCheck::MaybeHigh;
        }
        if harmonics.len() >= 4 {
            let odd: Vec<f64> = harmonics.iter().step_by(2).copied().collect();
            let even: Vec<f64> = harmonics.iter().skip(1).step_by(2).copied().collect();
            if mean(&odd) < ODD_SHARE * mean(&even) {
                return OctaveCheck::MaybeLow;
            }
        }
        OctaveCheck::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A harmonic tone at `f0` (12 harmonics, 1/h amplitudes, below 0.45 sr).
    fn tone(f0: f64, sr: f64, seconds: f64) -> Vec<f32> {
        (0..(seconds * sr) as usize)
            .map(|i| {
                let t = i as f64 / sr;
                let s: f64 = (1..=12)
                    .filter(|&h| f0 * (h as f64) < 0.45 * sr)
                    .map(|h| (2.0 * std::f64::consts::PI * f0 * h as f64 * t).sin() / h as f64)
                    .sum();
                (0.2 * s) as f32
            })
            .collect()
    }

    /// The check must be able to fail: the same tone, claimed an octave too high or too
    /// low, is flagged in the right direction (the steady-tone test in guide.rs covers
    /// "Ok" through the whole builder).
    #[test]
    fn a_wrong_octave_is_flagged_in_both_directions() {
        let sr = 16_000.0;
        let c = OctaveChecker::new(sr);
        for &midi in &[45.0, 57.0, 64.0, 72.0] {
            let x = tone(super::super::midi_to_hz(midi), sr, 1.0);
            assert_eq!(c.check(&x, 0.1, 0.9, midi), OctaveCheck::Ok, "midi {midi}");
            assert_eq!(
                c.check(&x, 0.1, 0.9, midi + 12.0),
                OctaveCheck::MaybeHigh,
                "midi {midi} claimed an octave high"
            );
            assert_eq!(
                c.check(&x, 0.1, 0.9, midi - 12.0),
                OctaveCheck::MaybeLow,
                "midi {midi} claimed an octave low"
            );
        }
    }
}

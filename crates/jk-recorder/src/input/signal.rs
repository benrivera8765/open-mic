// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated test input shaped like a singer through a mixer (WO2 section 9,
//! "The owner's rig": "the voice on one channel of two; at 44.1, 48 and 96 kHz").
//!
//! Nobody will sing into this during the build, so every input-side check feeds
//! these signals through the same [`super::InputSource`] path a microphone
//! uses. They may be written to disk (they are generated, not recorded).

use std::f64::consts::TAU;

/// A voice-like tone: a fundamental with decaying harmonics and vibrato.
#[derive(Clone, Copy, Debug)]
pub struct VoiceTone {
    /// Fundamental frequency in Hz.
    pub f0_hz: f64,
    /// Peak level of the result in dBFS.
    pub peak_dbfs: f64,
    /// Number of harmonics (1 = a plain sine).
    pub harmonics: u32,
    /// Vibrato rate in Hz (0 = none).
    pub vibrato_hz: f64,
    /// Vibrato depth in cents, peak (0 = none).
    pub vibrato_cents: f64,
}

impl VoiceTone {
    /// A sung A3 (220 Hz) at -12 dBFS peak, 8 harmonics, 5.5 Hz / 30 cent vibrato.
    pub fn default_a3() -> Self {
        Self {
            f0_hz: 220.0,
            peak_dbfs: -12.0,
            harmonics: 8,
            vibrato_hz: 5.5,
            vibrato_cents: 30.0,
        }
    }

    /// Render `seconds` of the tone at `rate` (mono), scaled so its peak is
    /// exactly `peak_dbfs`.
    pub fn render(&self, rate: u32, seconds: f64) -> Vec<f32> {
        let rate_f = f64::from(rate);
        let n = (rate_f * seconds).round() as usize;
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f64;
        for i in 0..n {
            let t = i as f64 / rate_f;
            let cents = self.vibrato_cents * (TAU * self.vibrato_hz * t).sin();
            let f = self.f0_hz * 2f64.powf(cents / 1200.0);
            phase += TAU * f / rate_f;
            let mut s = 0.0;
            for k in 1..=self.harmonics.max(1) {
                // Harmonics above Nyquist are left out (no aliasing at 16 kHz).
                if f * f64::from(k) < rate_f * 0.45 {
                    s += (phase * f64::from(k)).sin() / f64::from(k);
                }
            }
            out.push(s);
        }
        let peak = out.iter().fold(0.0f64, |m, &x| m.max(x.abs())).max(1e-12);
        let gain = 10f64.powf(self.peak_dbfs / 20.0) / peak;
        out.into_iter().map(|x| (x * gain) as f32).collect()
    }
}

/// What the channels without the voice carry.
#[derive(Clone, Copy, Debug)]
pub enum OtherChannel {
    /// Exact digital silence.
    Silence,
    /// White noise at this RMS level in dBFS (deterministic generator).
    Noise {
        /// RMS level, dBFS.
        rms_dbfs: f64,
    },
    /// A sine at this frequency and peak level (standing in for music).
    Tone {
        /// Frequency, Hz.
        hz: f64,
        /// Peak level, dBFS.
        peak_dbfs: f64,
    },
}

/// Lay a mono voice out on channel `voice_channel` of `channels`, the other
/// channels carrying `other`. Returns interleaved samples.
pub fn place_voice(
    voice: &[f32],
    rate: u32,
    channels: u16,
    voice_channel: u16,
    other: OtherChannel,
) -> Vec<f32> {
    let c = usize::from(channels.max(1));
    let vc = usize::from(voice_channel).min(c - 1);
    let mut out = vec![0.0f32; voice.len() * c];
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    for (i, &v) in voice.iter().enumerate() {
        for ch in 0..c {
            out[i * c + ch] = if ch == vc {
                v
            } else {
                match other {
                    OtherChannel::Silence => 0.0,
                    OtherChannel::Noise { rms_dbfs } => {
                        // xorshift64*, uniform in [-1, 1): RMS 1/sqrt(3).
                        rng ^= rng >> 12;
                        rng ^= rng << 25;
                        rng ^= rng >> 27;
                        let u = (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64
                            / (1u64 << 53) as f64;
                        let a = 10f64.powf(rms_dbfs / 20.0) * 3f64.sqrt();
                        ((u * 2.0 - 1.0) * a) as f32
                    }
                    OtherChannel::Tone { hz, peak_dbfs } => {
                        let a = 10f64.powf(peak_dbfs / 20.0);
                        (a * (TAU * hz * i as f64 / f64::from(rate)).sin()) as f32
                    }
                }
            };
        }
    }
    out
}

/// A plain autocorrelation estimate of the fundamental of a mono signal, for
/// **checking test plumbing only** (that a channel arrived intact and at the
/// right rate). It is not the practice-mode pitch tracker, which lives in
/// jk-pitch. Searches 60 to 1100 Hz; refines the lag by parabolic
/// interpolation; picks the shortest lag within 10 % of the best correlation
/// (so it does not jump an octave down).
pub fn check_f0_hz(mono: &[f32], rate: u32) -> Option<f64> {
    let rate_f = f64::from(rate);
    let min_lag = (rate_f / 1100.0).floor().max(2.0) as usize;
    let max_lag = (rate_f / 60.0).ceil() as usize;
    let n = mono.len().min(rate as usize / 5 + max_lag); // about 200 ms
    if n < max_lag * 2 {
        return None;
    }
    let x = &mono[..n];
    let w = n - max_lag - 1;
    let r0: f64 = x[..w].iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    if r0 <= 0.0 {
        return None;
    }
    let corr: Vec<f64> = (0..=max_lag + 1)
        .map(|lag| {
            if lag < min_lag - 1 {
                return 0.0;
            }
            let mut s = 0.0;
            let mut e = 0.0;
            for i in 0..w {
                s += f64::from(x[i]) * f64::from(x[i + lag]);
                e += f64::from(x[i + lag]) * f64::from(x[i + lag]);
            }
            s / (r0 * e).sqrt().max(1e-30)
        })
        .collect();
    let best = (min_lag..=max_lag)
        .map(|l| corr[l])
        .fold(f64::MIN, f64::max);
    if best < 0.5 {
        return None;
    }
    let lag = (min_lag..=max_lag)
        .find(|&l| corr[l] >= best * 0.9 && corr[l] >= corr[l - 1] && corr[l] >= corr[l + 1])?;
    let (a, b, c) = (corr[lag - 1], corr[lag], corr[lag + 1]);
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-12 {
        0.5 * (a - c) / denom
    } else {
        0.0
    };
    Some(rate_f / (lag as f64 + shift))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_has_the_asked_peak_and_pitch() {
        let tone = VoiceTone {
            vibrato_cents: 0.0,
            ..VoiceTone::default_a3()
        };
        for rate in [16_000, 44_100, 48_000, 96_000] {
            let v = tone.render(rate, 0.5);
            let peak = v.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
            assert!((20.0 * peak.log10() + 12.0).abs() < 0.01);
            let f = check_f0_hz(&v, rate).unwrap();
            let cents = 1200.0 * (f / 220.0).log2();
            assert!(cents.abs() < 5.0, "{rate} Hz: {f} Hz ({cents} cents)");
        }
    }

    #[test]
    fn voice_lands_on_its_channel_only() {
        let v = vec![0.5f32, -0.25, 0.125];
        let x = place_voice(&v, 48_000, 2, 1, OtherChannel::Silence);
        assert_eq!(x, vec![0.0, 0.5, 0.0, -0.25, 0.0, 0.125]);
        let n = place_voice(
            &vec![0.0; 48_000],
            48_000,
            2,
            0,
            OtherChannel::Noise { rms_dbfs: -40.0 },
        );
        let rms = (n
            .iter()
            .skip(1)
            .step_by(2)
            .map(|&s| f64::from(s).powi(2))
            .sum::<f64>()
            / 48_000.0)
            .sqrt();
        assert!((20.0 * rms.log10() + 40.0).abs() < 0.2);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-sample gain ramps and the equal-power crossfade table.
//!
//! Both are defined in samples, not in blocks, so the result does not depend on how the
//! audio is cut into blocks. Transcendentals come from the pure-Rust `libm` crate, so the
//! tables are bit-identical on every platform (std's `sin`/`cos` call the platform maths
//! library, which may differ in the last bit between Windows and macOS).

/// A linear gain ramp: when the target changes, the gain moves there over `ramp` samples.
/// Once settled it is exactly the target value (so a gain of 1.0 is bit-transparent).
#[derive(Clone, Debug)]
pub struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp: u32,
}

impl GainRamp {
    pub fn new(initial: f32, ramp_samples: u32) -> Self {
        GainRamp { current: initial, target: initial, step: 0.0, remaining: 0, ramp: ramp_samples.max(1) }
    }

    pub fn set_target(&mut self, target: f32) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.remaining = self.ramp;
        self.step = (target - self.current) / self.ramp as f32;
    }

    /// Jump straight to `value` (used on reset).
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.remaining = 0;
        self.step = 0.0;
    }

    #[inline]
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.current
    }

    /// Advance one sample and return the gain for it.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else {
                self.current += self.step;
            }
        }
        self.current
    }
}

/// Equal-power crossfade gains: `fade_in[k] = sin(pi/2 * k/len)`, `fade_out[k] =
/// cos(pi/2 * k/len)`, for `k = 0..=len`. `fade_in^2 + fade_out^2 = 1` (to float precision),
/// which keeps the loudness constant when crossfading between uncorrelated signals (two
/// different keys are uncorrelated).
pub struct EqualPowerTable {
    pub fade_in: Vec<f32>,
    pub fade_out: Vec<f32>,
}

impl EqualPowerTable {
    pub fn new(len: usize) -> Self {
        let len = len.max(1);
        let mut fade_in = Vec::with_capacity(len + 1);
        let mut fade_out = Vec::with_capacity(len + 1);
        for k in 0..=len {
            let x = core::f64::consts::FRAC_PI_2 * k as f64 / len as f64;
            fade_in.push(libm::sin(x) as f32);
            fade_out.push(libm::cos(x) as f32);
        }
        // Exact end points.
        fade_in[0] = 0.0;
        fade_out[0] = 1.0;
        fade_in[len] = 1.0;
        fade_out[len] = 0.0;
        EqualPowerTable { fade_in, fade_out }
    }

    pub fn len(&self) -> usize {
        self.fade_in.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_lands_exactly() {
        let mut g = GainRamp::new(0.0, 10);
        g.set_target(1.0);
        let v: Vec<f32> = (0..12).map(|_| g.tick()).collect();
        assert_eq!(v[9], 1.0);
        assert_eq!(v[11], 1.0);
        assert!(v[4] > 0.4 && v[4] < 0.6);
    }

    #[test]
    fn equal_power() {
        let t = EqualPowerTable::new(1764);
        for k in 0..=t.len() {
            let p = t.fade_in[k] as f64 * t.fade_in[k] as f64 + t.fade_out[k] as f64 * t.fade_out[k] as f64;
            assert!((p - 1.0).abs() < 1e-6);
        }
    }
}

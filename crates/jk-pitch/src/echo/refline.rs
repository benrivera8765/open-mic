// SPDX-License-Identifier: GPL-3.0-or-later
//! Reference delay line of the echo canceller: the app's own output, kept long enough for
//! the longest delay searched, read back at a *fractional* position.
//!
//! The canceller reads the reference at `now - delay(t)`, where `delay(t)` follows the clock
//! drift between the output and the input device (WO2 trap 4). Moving the read position by a
//! fraction of a sample per second keeps the returned music still in the adaptive filter's
//! view, so the filter does not have to chase a slowly sliding echo (which costs a lot of
//! suppression at high frequencies: a 1 microsecond error is already -32 dB at 4 kHz).
//!
//! Interpolation: a 32-tap Kaiser-windowed sinc (beta 8), tabulated at 512 fractional phases
//! with linear interpolation between neighbouring phases; each phase is normalised to unity
//! gain at 0 Hz. Error below about -80 dB up to 0.42 x the sample rate. An integer position
//! is read directly (no filtering), so a drift-free echo path sees the reference untouched.
//!
//! No allocation after [`RefLine::new`].

/// Taps of the interpolation kernel.
pub(crate) const TAPS: usize = 32;
/// The kernel reaches this many samples past the read position: a read at `p` needs the
/// reference up to `floor(p) + HALF`.
pub(crate) const HALF: usize = TAPS / 2;
/// Tabulated fractional phases (plus one row for phase 1.0).
const PHASES: usize = 512;
/// Kaiser window shape.
const KAISER_BETA: f64 = 8.0;

/// Modified Bessel function of the first kind, order 0 (power series).
pub(crate) fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// `sin(pi x) / (pi x)`.
pub(crate) fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let a = std::f64::consts::PI * x;
        a.sin() / a
    }
}

/// Multi-channel ring of reference samples with fractional read-back.
pub(crate) struct RefLine {
    channels: usize,
    cap: usize,
    mask: usize,
    /// Channel-major: channel `c`, absolute sample `i` lives at `c * cap + (i & mask)`.
    buf: Vec<f32>,
    /// Samples written per channel since construction or [`RefLine::clear`].
    written: u64,
    /// `(PHASES + 1) * TAPS` kernel coefficients; row `p` is the kernel for fraction
    /// `p / PHASES`.
    table: Vec<f32>,
    /// Kernel for the current read (interpolated between two rows).
    kern: [f32; TAPS],
}

impl RefLine {
    /// A line for `channels` channels that can read back at least `history` samples behind
    /// the newest one.
    pub(crate) fn new(channels: usize, history: usize) -> Self {
        let cap = (history + TAPS + 2).next_power_of_two();
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        let i0b = bessel_i0(KAISER_BETA);
        let mut row = [0.0f64; TAPS];
        for p in 0..=PHASES {
            let f = p as f64 / PHASES as f64;
            let mut sum = 0.0;
            for (j, h) in row.iter_mut().enumerate() {
                // Tap j multiplies x[i - (HALF - 1) + j]; its distance from the read point
                // i + f is t = f + (HALF - 1) - j, inside (-HALF, HALF].
                let t = f + (HALF as f64 - 1.0) - j as f64;
                let r = t / HALF as f64;
                let w = if r.abs() >= 1.0 {
                    0.0
                } else {
                    bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / i0b
                };
                *h = sinc(t) * w;
                sum += *h;
            }
            for (j, h) in row.iter().enumerate() {
                table[p * TAPS + j] = (*h / sum) as f32;
            }
        }
        Self {
            channels,
            cap,
            mask: cap - 1,
            buf: vec![0.0; channels * cap],
            written: 0,
            table,
            kern: [0.0; TAPS],
        }
    }

    /// Forget everything written (the line reads as silence). Does not allocate.
    pub(crate) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.written = 0;
    }

    /// Samples written per channel so far.
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// Append one frame (`channels` samples, one per channel).
    #[inline]
    pub(crate) fn push(&mut self, frame: &[f32]) {
        let slot = (self.written as usize) & self.mask;
        for (c, &v) in frame.iter().take(self.channels).enumerate() {
            self.buf[c * self.cap + slot] = if v.is_finite() { v } else { 0.0 };
        }
        self.written += 1;
    }

    /// Sample `i` of channel `c`, or 0 outside what the ring holds.
    #[inline]
    fn at(&self, c: usize, i: i64) -> f32 {
        if i < 0 || i as u64 >= self.written || self.written - i as u64 > self.cap as u64 {
            0.0
        } else {
            self.buf[c * self.cap + (i as usize & self.mask)]
        }
    }

    /// Mean of the channels at absolute sample `i` (0 outside what the ring holds).
    #[inline]
    pub(crate) fn mono_at(&self, i: i64) -> f32 {
        let mut acc = 0.0f32;
        for c in 0..self.channels {
            acc += self.at(c, i);
        }
        acc / self.channels as f32
    }

    /// Read every channel at absolute (fractional) sample position `pos` into `out`. The
    /// caller keeps `pos + HALF` below [`RefLine::written`]; samples outside what the ring
    /// holds read as zero.
    pub(crate) fn read(&mut self, pos: f64, out: &mut [f32]) {
        let fl = pos.floor();
        let i = fl as i64;
        let frac = pos - fl;
        if frac < 1e-9 {
            for (c, o) in out.iter_mut().take(self.channels).enumerate() {
                *o = self.at(c, i);
            }
            return;
        }
        let fp = frac * PHASES as f64;
        let p = (fp as usize).min(PHASES - 1);
        let a = (fp - p as f64) as f32;
        let (lo, hi) = self.table.split_at(p * TAPS + TAPS);
        let lo = &lo[p * TAPS..];
        let hi = &hi[..TAPS];
        for j in 0..TAPS {
            self.kern[j] = lo[j] + a * (hi[j] - lo[j]);
        }
        let first = i - (HALF as i64 - 1);
        let last = i + HALF as i64;
        let inside = first >= 0
            && (last as u64) < self.written
            && self.written - first as u64 <= self.cap as u64;
        for (c, o) in out.iter_mut().take(self.channels).enumerate() {
            let mut acc = 0.0f32;
            if inside {
                let base = c * self.cap;
                for (j, &k) in self.kern.iter().enumerate() {
                    acc += k * self.buf[base + ((first as usize + j) & self.mask)];
                }
            } else {
                for (j, &k) in self.kern.iter().enumerate() {
                    acc += k * self.at(c, first + j as i64);
                }
            }
            *o = acc;
        }
    }
}

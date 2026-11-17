// SPDX-License-Identifier: GPL-3.0-or-later
//! Lock-free level meter and clip counter.
//!
//! The capture callback calls [`LevelMeter::process`] for every block it
//! receives; the UI polls [`LevelMeter::take_levels`] (about 30 times a second)
//! from another thread. Nothing here allocates or locks, so it is safe to call
//! from an audio callback.
//!
//! Peak values are stored as the bit pattern of a non-negative `f32` inside an
//! `AtomicU32`. For non-negative IEEE-754 floats the bit patterns sort in the
//! same order as the values, so `fetch_max` on the bits is `max` on the values.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// The most channels the meter tracks. System mixes beyond 7.1 are not expected;
/// extra channels are written to the file but not metered.
pub const MAX_METER_CHANNELS: usize = 8;

/// A sample whose magnitude is at or above this value is counted as clipped.
///
/// The recording itself is 32-bit float, so values above full scale are stored
/// without damage. But a system mix that reaches full scale was (or, on a
/// fixed-point device, would be) clipping at the output, so the user is warned.
pub const CLIP_LEVEL: f32 = 1.0;

/// Convert a linear magnitude to dBFS. Zero (or less) gives negative infinity.
pub fn to_dbfs(x: f32) -> f32 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Per-channel levels returned by [`LevelMeter::take_levels`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    /// Highest magnitude per channel since the previous `take_levels` call (linear).
    pub peak: Vec<f32>,
    /// RMS per channel of the most recent block (linear).
    pub rms: Vec<f32>,
    /// Highest magnitude per channel since the recording started (linear).
    pub peak_total: Vec<f32>,
}

impl Levels {
    /// The recent peaks in dBFS.
    pub fn peak_dbfs(&self) -> Vec<f32> {
        self.peak.iter().copied().map(to_dbfs).collect()
    }
    /// The whole-recording peaks in dBFS.
    pub fn peak_total_dbfs(&self) -> Vec<f32> {
        self.peak_total.iter().copied().map(to_dbfs).collect()
    }
}

/// Lock-free meter shared between the capture callback and the UI.
#[derive(Debug)]
pub struct LevelMeter {
    channels: AtomicUsize,
    peak_recent: [AtomicU32; MAX_METER_CHANNELS],
    peak_total: [AtomicU32; MAX_METER_CHANNELS],
    rms_last: [AtomicU32; MAX_METER_CHANNELS],
    clipped_samples: AtomicU64,
    non_finite_samples: AtomicU64,
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelMeter {
    /// A meter with no channels yet; call [`LevelMeter::set_channels`] once the
    /// real capture format is known.
    pub fn new() -> Self {
        Self {
            channels: AtomicUsize::new(0),
            peak_recent: Default::default(),
            peak_total: Default::default(),
            rms_last: Default::default(),
            clipped_samples: AtomicU64::new(0),
            non_finite_samples: AtomicU64::new(0),
        }
    }

    /// Set how many interleaved channels the blocks passed to `process` carry.
    pub fn set_channels(&self, channels: usize) {
        self.channels.store(channels, Ordering::Relaxed);
    }

    /// Meter one interleaved block. Real-time safe: no allocation, no locks.
    pub fn process(&self, interleaved: &[f32]) {
        let channels = self.channels.load(Ordering::Relaxed);
        if channels == 0 {
            return;
        }
        let metered = channels.min(MAX_METER_CHANNELS);
        let mut peak = [0.0f32; MAX_METER_CHANNELS];
        let mut sum_sq = [0.0f64; MAX_METER_CHANNELS];
        let mut clipped = 0u64;
        let mut non_finite = 0u64;
        let frames = interleaved.len() / channels;
        for frame in interleaved.chunks_exact(channels) {
            for (ch, &s) in frame.iter().enumerate() {
                if !s.is_finite() {
                    non_finite += 1;
                    continue;
                }
                let a = s.abs();
                if a >= CLIP_LEVEL {
                    clipped += 1;
                }
                if ch < metered {
                    if a > peak[ch] {
                        peak[ch] = a;
                    }
                    sum_sq[ch] += f64::from(s) * f64::from(s);
                }
            }
        }
        for ch in 0..metered {
            let bits = peak[ch].to_bits();
            self.peak_recent[ch].fetch_max(bits, Ordering::Relaxed);
            self.peak_total[ch].fetch_max(bits, Ordering::Relaxed);
            let rms = if frames > 0 {
                (sum_sq[ch] / frames as f64).sqrt() as f32
            } else {
                0.0
            };
            self.rms_last[ch].store(rms.to_bits(), Ordering::Relaxed);
        }
        if clipped > 0 {
            self.clipped_samples.fetch_add(clipped, Ordering::Relaxed);
        }
        if non_finite > 0 {
            self.non_finite_samples
                .fetch_add(non_finite, Ordering::Relaxed);
        }
    }

    /// Read the levels and reset the "recent" peaks (for a UI meter with its
    /// own fall-back animation).
    pub fn take_levels(&self) -> Levels {
        let n = self
            .channels
            .load(Ordering::Relaxed)
            .min(MAX_METER_CHANNELS);
        let mut out = Levels::default();
        for ch in 0..n {
            out.peak.push(f32::from_bits(
                self.peak_recent[ch].swap(0, Ordering::Relaxed),
            ));
            out.rms
                .push(f32::from_bits(self.rms_last[ch].load(Ordering::Relaxed)));
            out.peak_total
                .push(f32::from_bits(self.peak_total[ch].load(Ordering::Relaxed)));
        }
        out
    }

    /// Read the levels without resetting anything.
    pub fn peek_levels(&self) -> Levels {
        let n = self
            .channels
            .load(Ordering::Relaxed)
            .min(MAX_METER_CHANNELS);
        let mut out = Levels::default();
        for ch in 0..n {
            out.peak
                .push(f32::from_bits(self.peak_recent[ch].load(Ordering::Relaxed)));
            out.rms
                .push(f32::from_bits(self.rms_last[ch].load(Ordering::Relaxed)));
            out.peak_total
                .push(f32::from_bits(self.peak_total[ch].load(Ordering::Relaxed)));
        }
        out
    }

    /// Samples (counting each channel separately) at or above [`CLIP_LEVEL`].
    pub fn clipped_samples(&self) -> u64 {
        self.clipped_samples.load(Ordering::Relaxed)
    }

    /// NaN or infinite samples received (written to the file unchanged, but
    /// reported because they indicate a broken source).
    pub fn non_finite_samples(&self) -> u64 {
        self.non_finite_samples.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_rms_and_clips() {
        let m = LevelMeter::new();
        m.set_channels(2);
        // L: 0.5, -0.25 ; R: -1.0 (clip), 0.0
        m.process(&[0.5, -1.0, -0.25, 0.0]);
        let l = m.take_levels();
        assert_eq!(l.peak, vec![0.5, 1.0]);
        assert_eq!(l.peak_total, vec![0.5, 1.0]);
        let rms_l = ((0.25f64 + 0.0625) / 2.0).sqrt() as f32;
        assert!((l.rms[0] - rms_l).abs() < 1e-7);
        assert_eq!(m.clipped_samples(), 1);
        // Recent peak is reset by take_levels, total is not.
        m.process(&[0.1, 0.1]);
        let l = m.take_levels();
        assert_eq!(l.peak, vec![0.1, 0.1]);
        assert_eq!(l.peak_total, vec![0.5, 1.0]);
    }

    #[test]
    fn values_above_full_scale_are_counted_not_altered() {
        let m = LevelMeter::new();
        m.set_channels(1);
        m.process(&[1.5, f32::NAN, -2.0, 0.999]);
        assert_eq!(m.clipped_samples(), 2);
        assert_eq!(m.non_finite_samples(), 1);
        assert_eq!(m.peek_levels().peak_total, vec![2.0]);
    }

    #[test]
    fn dbfs() {
        assert_eq!(to_dbfs(1.0), 0.0);
        assert!((to_dbfs(0.1) + 20.0).abs() < 1e-5);
        assert_eq!(to_dbfs(0.0), f32::NEG_INFINITY);
    }
}

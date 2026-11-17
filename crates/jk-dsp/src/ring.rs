// SPDX-License-Identifier: GPL-3.0-or-later
//! A planar, fixed-capacity ring buffer addressed by **absolute frame index**.
//!
//! The graph keeps every signal on one timeline (frame index since the graph started), so
//! "the bypass path at frame u", "the drums at frame u" and "Rubber Band's output aligned to
//! frame u" are all simple reads at the same index. Allocation happens only in `new`.

/// Stereo-or-more planar ring. Frames `[end - capacity, end)` are readable; anything older
/// has been overwritten. Freshly created rings read as digital silence for every index
/// below `end` (they start zero-filled).
pub struct AbsRing {
    chans: Vec<Vec<f32>>,
    mask: usize,
    /// One past the newest frame written.
    end: u64,
}

impl AbsRing {
    /// `min_capacity` is rounded up to a power of two. `start` is the absolute index the
    /// first write will land on; everything before it reads as zeros.
    pub fn new(channels: usize, min_capacity: usize, start: u64) -> Self {
        let cap = min_capacity.next_power_of_two();
        AbsRing {
            chans: (0..channels).map(|_| vec![0.0; cap]).collect(),
            mask: cap - 1,
            end: start,
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    pub fn channels(&self) -> usize {
        self.chans.len()
    }

    /// One past the newest frame written.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// Oldest frame still readable.
    pub fn start(&self) -> u64 {
        self.end.saturating_sub(self.capacity() as u64)
    }

    /// Zero everything and restart the timeline at `start`.
    pub fn clear(&mut self, start: u64) {
        for c in &mut self.chans {
            c.iter_mut().for_each(|v| *v = 0.0);
        }
        self.end = start;
    }

    /// Append `frames` frames (`input[ch][0..frames]`) at `end`.
    pub fn push(&mut self, input: &[&[f32]], frames: usize) {
        debug_assert_eq!(input.len(), self.chans.len());
        debug_assert!(frames <= self.capacity());
        let pos = (self.end as usize) & self.mask;
        let first = frames.min(self.capacity() - pos);
        for (dst, src) in self.chans.iter_mut().zip(input) {
            dst[pos..pos + first].copy_from_slice(&src[..first]);
            dst[..frames - first].copy_from_slice(&src[first..frames]);
        }
        self.end += frames as u64;
    }

    /// Append `frames` frames of silence.
    pub fn push_silence(&mut self, frames: usize) {
        let pos = (self.end as usize) & self.mask;
        let first = frames.min(self.capacity() - pos);
        for dst in self.chans.iter_mut() {
            dst[pos..pos + first].iter_mut().for_each(|v| *v = 0.0);
            dst[..frames - first].iter_mut().for_each(|v| *v = 0.0);
        }
        self.end += frames as u64;
    }

    /// True if `[abs, abs + frames)` is fully readable.
    pub fn has(&self, abs: u64, frames: usize) -> bool {
        abs >= self.start() && abs + frames as u64 <= self.end
    }

    /// Copy `[abs, abs + out[ch].len())` of channel `ch` into `out`.
    /// Panics (debug) if the range is not readable.
    pub fn read_channel(&self, ch: usize, abs: u64, out: &mut [f32]) {
        let frames = out.len();
        debug_assert!(self.has(abs, frames), "ring read out of range");
        let pos = (abs as usize) & self.mask;
        let first = frames.min(self.capacity() - pos);
        let src = &self.chans[ch];
        out[..first].copy_from_slice(&src[pos..pos + first]);
        out[first..].copy_from_slice(&src[..frames - first]);
    }

    /// Add `[abs, abs + out.len())` of channel `ch` into `out` (out += ring).
    pub fn add_channel_into(&self, ch: usize, abs: u64, out: &mut [f32]) {
        let frames = out.len();
        debug_assert!(self.has(abs, frames), "ring read out of range");
        let pos = (abs as usize) & self.mask;
        let first = frames.min(self.capacity() - pos);
        let src = &self.chans[ch];
        for (o, s) in out[..first].iter_mut().zip(&src[pos..pos + first]) {
            *o += *s;
        }
        for (o, s) in out[first..].iter_mut().zip(&src[..frames - first]) {
            *o += *s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_reads_back() {
        let mut r = AbsRing::new(2, 8, 100);
        let a: Vec<f32> = (0..6).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..6).map(|i| -(i as f32)).collect();
        r.push(&[&a, &b], 6);
        r.push(&[&a, &b], 6);
        assert_eq!(r.end(), 112);
        assert!(r.has(104, 8));
        assert!(!r.has(103, 8));
        let mut out = [0.0f32; 8];
        r.read_channel(0, 104, &mut out);
        assert_eq!(out, [4.0, 5.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut acc = [1.0f32; 2];
        r.add_channel_into(1, 110, &mut acc);
        assert_eq!(acc, [1.0 - 4.0, 1.0 - 5.0]);
    }
}

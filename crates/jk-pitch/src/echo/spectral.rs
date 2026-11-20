// SPDX-License-Identifier: GPL-3.0-or-later
//! Spectral look at the canceller's output, every four blocks (about 10 ms) over about
//! 43 ms, for two things the per-block power comparison cannot do:
//!
//! 1. **Voice evidence that does not need the left-overs measured** ("tier 2" of the gate).
//!    A singer's energy sits in a few harmonic peaks; the returned music, whatever is left
//!    of it, can never be louder in any frequency bin than the whole echo would be there
//!    (the echo path's gain times the reference's spectrum, or the filter's own echo
//!    estimate where the path is louder). So the output energy that stands more than
//!    [`MARGIN`] above that bound in its own bin cannot be music: if it is a large enough
//!    share of the output, the output is the singer. This recognises a quiet note under the
//!    returned music (a high "ee" whose note is 6-12 dB under the music at the input) as
//!    soon as removal has made the output voice-dominated, and it works from the first
//!    moment the delay is known, with no stretch of the song free of singing (continuous
//!    singing from the first second). With nobody singing the output never exceeds the
//!    bound: a filter that adds its estimate with the wrong sign can at most double the
//!    amplitude (6 dB); the guide vocal in the playback is part of the reference, so its
//!    left-over is bounded like the rest (WO2 trap 6).
//! 2. **The share of the echo left in the output during singing**, by the regression of
//!    the output's per-bin power fluctuations on the reference's (after the leakage estimate
//!    of J.-M. Valin, IEEE TASLP 15(3), 2007, which regresses on the filter's echo estimate;
//!    the reference is used here so that bands the filter has not learnt count whole, and
//!    only bins without voice evidence take part). The singer's power fluctuates
//!    independently of the music's, so it adds noise but no bias. Used when the song never
//!    leaves a stretch of music without singing to measure the left-overs directly.
//!
//! Pure DSP; everything is allocated in [`Spectral::new`].

use std::f64::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Output energy in a bin counts as "not music" above this multiple of the most music
/// that can be there (6 dB: the largest a misadjusted filter can make the left-overs).
pub(crate) const MARGIN: f32 = 4.0;
/// Share of the output energy (in the band) that must be "not music" for a frame to be
/// the singer. Below it the left-overs are loud enough beside the voice to pull the
/// tracker.
pub(crate) const VOICE_SHARE: f32 = 0.5;
/// The excess must also stand this far above the band's noise floor.
const NOISE_FACTOR: f32 = 4.0;
/// Analysis band, Hz: the singing range's fundamentals and the harmonics that carry the
/// pitch; below it thumps and hum, above it hiss and consonants.
const BAND_LO_HZ: f64 = 60.0;
const BAND_HI_HZ: f64 = 4_000.0;
/// Top of the regression's band, Hz (the left-overs of the music matter wherever the
/// music is; capped at 0.45 of the rate).
const REG_HI_HZ: f64 = 10_000.0;
/// Time constants of the leakage regression: the per-bin means the fluctuations are taken
/// around, and the averaging of the regression sums, s.
const MEAN_S: f64 = 0.05;
const REG_S: f64 = 3.0;
/// Frames of music the regression needs before it is used.
const REG_MIN_S: f64 = 1.5;
/// Time constant of the slow per-bin means that choose the regression's bins, s.
const SLOW_S: f64 = 0.25;

/// One frame's findings.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Frame {
    /// The output in this frame is the singer (tier 2).
    pub voice: bool,
}

/// See the module documentation.
pub(crate) struct Spectral {
    n: usize,
    hop_blocks: usize,
    k_lo: usize,
    k_hi: usize,
    k_reg: usize,
    win: Vec<f32>,
    ring_e: Vec<f32>,
    ring_y: Vec<f32>,
    ring_x: Vec<f32>,
    pos: usize,
    filled: usize,
    blocks: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<f32>,
    spec: Vec<Complex<f32>>,
    pe: Vec<f32>,
    py: Vec<f32>,
    px: Vec<f32>,
    // Noise floor of the band's output energy: minimum over two one-second windows.
    nf_cur: f32,
    nf_prev: f32,
    nf_frames: u32,
    nf_window: u32,
    // Leakage regression.
    me: Vec<f32>,
    my: Vec<f32>,
    seen: Vec<bool>,
    /// Slow means of the echo estimate's and the reference's power per bin, for choosing
    /// the bins of the regression without looking at the frame's own fluctuation.
    sy: Vec<f32>,
    sx: Vec<f32>,
    a_slow: f32,
    a_mean: f32,
    a_reg: f64,
    /// Per-bin regression sums (fluctuation of the output x of the reference, and of the
    /// reference squared), averaged over [`REG_S`].
    cov: Vec<f64>,
    var: Vec<f64>,
    reg_frames: u32,
    reg_min: u32,
}

impl Spectral {
    /// For input at `rate` Hz in blocks of `b` samples.
    pub(crate) fn new(rate: u32, b: usize) -> Self {
        let fs = f64::from(rate);
        let n = (1usize << ((0.043 * fs).log2().round() as u32)).max(4 * b);
        let nb = n / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_scratch_len()];
        let k_lo = ((BAND_LO_HZ * n as f64 / fs).floor() as usize).max(1);
        let k_hi = ((BAND_HI_HZ * n as f64 / fs).ceil() as usize).min(nb - 1);
        let k_reg = ((REG_HI_HZ.min(0.45 * fs) * n as f64 / fs).ceil() as usize)
            .clamp(k_hi, nb - 1);
        let hop_blocks = 4;
        let frame_s = (hop_blocks * b) as f64 / fs;
        let win = (0..n)
            .map(|j| (0.5 - 0.5 * (2.0 * PI * (j as f64 + 0.5) / n as f64).cos()) as f32)
            .collect();
        Self {
            n,
            hop_blocks,
            k_lo,
            k_hi,
            k_reg,
            win,
            ring_e: vec![0.0; n],
            ring_y: vec![0.0; n],
            ring_x: vec![0.0; n],
            pos: 0,
            filled: 0,
            blocks: 0,
            spec: vec![Complex::new(0.0, 0.0); nb],
            buf: vec![0.0; n],
            fft,
            scratch,
            pe: vec![0.0; nb],
            py: vec![0.0; nb],
            px: vec![0.0; nb],
            nf_cur: f32::MAX,
            nf_prev: f32::MAX,
            nf_frames: 0,
            nf_window: (1.0 / frame_s).round().max(1.0) as u32,
            me: vec![0.0; nb],
            my: vec![0.0; nb],
            seen: vec![false; nb],
            sy: vec![0.0; nb],
            sx: vec![0.0; nb],
            a_slow: (1.0 - (-frame_s / SLOW_S).exp()) as f32,
            a_mean: (1.0 - (-frame_s / MEAN_S).exp()) as f32,
            a_reg: 1.0 - (-frame_s / REG_S).exp(),
            cov: vec![0.0; nb],
            var: vec![0.0; nb],
            reg_frames: 0,
            reg_min: (REG_MIN_S / frame_s).round() as u32,
        }
    }

    /// Frame length in samples.
    pub(crate) fn len(&self) -> usize {
        self.n
    }

    /// Blocks between frames.
    pub(crate) fn hop_blocks(&self) -> usize {
        self.hop_blocks
    }

    /// Forget the signals and the regression (a restart of the filter at a new delay, or a
    /// new device). Keeps the noise floor unless `all`.
    pub(crate) fn reset(&mut self, all: bool) {
        self.ring_e.fill(0.0);
        self.ring_y.fill(0.0);
        self.ring_x.fill(0.0);
        self.pos = 0;
        self.filled = 0;
        self.blocks = 0;
        self.seen.fill(false);
        self.sy.fill(0.0);
        self.sx.fill(0.0);
        self.cov.fill(0.0);
        self.var.fill(0.0);
        self.reg_frames = 0;
        if all {
            self.nf_cur = f32::MAX;
            self.nf_prev = f32::MAX;
            self.nf_frames = 0;
        }
    }

    /// Output power left per unit of reference power, from the regression, once it has
    /// seen enough music: each bin's slope, weighted by the bin's mean reference power (so
    /// it is the left-over power over the reference power, as a suppression figure means
    /// it; weighting by the fluctuations instead favours the loudest bins, where the filter
    /// learns first, and overstated the removal by up to 10 dB). Divided by the echo
    /// path's gain it is the share of the echo left in the output.
    pub(crate) fn leak_slope(&self) -> Option<f32> {
        if self.reg_frames < self.reg_min {
            return None;
        }
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for k in self.k_lo..self.k_reg {
            if self.var[k] > 1e-40 {
                let w = f64::from(self.sx[k]);
                num += w * (self.cov[k] / self.var[k]).max(0.0);
                den += w;
            }
        }
        (den > 1e-40).then(|| (num / den) as f32)
    }

    /// Add one block: the output `e`, the output filter's echo estimate `y` and the aligned
    /// mono reference `x` (all `b` samples, the same input samples). Every
    /// [`Spectral::hop_blocks`] blocks, once the frame is full, analyses the newest
    /// [`Spectral::len`] samples and returns the frame's findings. `gain` is the echo path's
    /// power gain in band relative to the mono reference (the detector's level);
    /// `wandering` makes the bound ignore the filter's estimate and add 3 dB (the
    /// detector's level is biased low by a wandering delay); `music` false when the
    /// reference is silent (no regression).
    pub(crate) fn push(
        &mut self,
        e: &[f32],
        y: &[f32],
        x: &[f32],
        gain: f32,
        wandering: bool,
        music: bool,
    ) -> Option<Frame> {
        let n = self.n;
        for i in 0..e.len() {
            self.ring_e[self.pos] = e[i];
            self.ring_y[self.pos] = y[i];
            self.ring_x[self.pos] = x[i];
            self.pos = (self.pos + 1) % n;
        }
        self.filled = (self.filled + e.len()).min(n);
        self.blocks += 1;
        if self.blocks < self.hop_blocks {
            return None;
        }
        self.blocks = 0;
        if self.filled < n {
            return None;
        }
        // Power spectra of the newest frame (oldest sample at `pos`).
        for which in 0..3 {
            let ring = match which {
                0 => &self.ring_e,
                1 => &self.ring_y,
                _ => &self.ring_x,
            };
            for j in 0..n {
                self.buf[j] = ring[(self.pos + j) % n] * self.win[j];
            }
            let _ = self
                .fft
                .process_with_scratch(&mut self.buf, &mut self.spec, &mut self.scratch);
            let dst = match which {
                0 => &mut self.pe,
                1 => &mut self.py,
                _ => &mut self.px,
            };
            for k in self.k_lo..self.k_reg {
                dst[k] = self.spec[k].norm_sqr();
            }
        }
        let (mut total, mut excess) = (0.0f32, 0.0f32);
        let a = self.a_mean;
        let al = self.a_slow;
        let ar = self.a_reg;
        let first = self.reg_frames == 0;
        let mut regressed = false;
        for k in self.k_lo..self.k_reg {
            let (pe, py, px) = (self.pe[k], self.py[k], self.px[k]);
            if k < self.k_hi {
                let bound = if wandering {
                    2.0 * gain * px
                } else {
                    py.max(gain * px)
                };
                total += pe;
                let ex = pe - MARGIN * bound;
                if ex > 0.0 {
                    excess += ex;
                }
            }
            self.sy[k] += al * (py - self.sy[k]);
            self.sx[k] += al * (px - self.sx[k]);
            // Leakage regression on the bins without voice evidence against the slow means
            // (not the frame's own bound: choosing bins by it would keep a voice bin exactly
            // when the music there is loud, which biases the slope upwards).
            if !music || pe > MARGIN * self.sy[k].max(gain * self.sx[k]) {
                continue;
            }
            if !self.seen[k] {
                self.seen[k] = true;
                self.me[k] = pe;
                self.my[k] = px;
                continue;
            }
            let de = f64::from(pe - self.me[k]);
            let dx = f64::from(px - self.my[k]);
            if first {
                self.cov[k] = de * dx;
                self.var[k] = dx * dx;
            } else {
                self.cov[k] += ar * (de * dx - self.cov[k]);
                self.var[k] += ar * (dx * dx - self.var[k]);
            }
            regressed = true;
            self.me[k] += a * de as f32;
            self.my[k] += a * dx as f32;
        }
        if regressed {
            self.reg_frames = self.reg_frames.saturating_add(1);
        }
        // Noise floor of the band's energy.
        self.nf_cur = self.nf_cur.min(total);
        self.nf_frames += 1;
        if self.nf_frames >= self.nf_window {
            self.nf_prev = self.nf_cur;
            self.nf_cur = f32::MAX;
            self.nf_frames = 0;
        }
        let noise = self.nf_cur.min(self.nf_prev);
        let share = if total > 0.0 { excess / total } else { 0.0 };
        let voice = total > 1e-20
            && share >= VOICE_SHARE
            && excess > NOISE_FACTOR * noise.min(total);
        Some(Frame { voice })
    }
}

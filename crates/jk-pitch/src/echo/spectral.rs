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
/// Bands of the per-frequency left-over: a third of an octave, at least this many bins.
const BAND_MIN_BINS: usize = 4;
/// Standard errors added to a measured left-over to make it a cautious one (about the 98th
/// percentile of the estimate's noise).
pub(crate) const REG_Z: f64 = 2.0;
/// The band's measured left-over bounds a bin only once the regression has seen this much
/// music (s): early on the filter is still learning and (over a cable) the drift is not yet
/// measured, so removal is uneven from bin to bin and moment to moment. Measured with 4 s:
/// 0.07 % of the frames shown as pitch with nobody singing over a cable return with the
/// guide vocal on, 5-7 s into the song; none with 8 s.
const LEAK_MIN_S: f64 = 8.0;
/// A bin whose reference power over the last quarter second is this many times its mean
/// over [`LONG_S`] has just been lit by the music (a new note of the guide vocal, an
/// instrument entering): the filter has hardly learnt the echo path there, and its
/// left-over may be the whole echo, so the bound there stays the whole echo. Measured
/// without it: up to 0.27 % of the frames shown as pitch with nobody singing over a cable
/// return with the guide vocal on, at the guide's new notes.
const JUMP: f32 = 4.0;
/// The bound in a bin is never below this share of the whole echo there (-12 dB): removal
/// varies more from bin to bin than a band's average says (measured with nobody singing: a
/// cable return with the guide vocal on left half of the echo at two bins of the guide's
/// note while their band's left-over said -16 dB, early in the song while the drift was
/// still being measured, and at the low notes the filter cannot hold).
const LEAK_FLOOR: f32 = 1.0 / 16.0;
const LONG_S: f64 = 10.0;
/// Margin on each band's cautious left-over for the bound of the voice evidence (6 dB): a
/// band's left-over is an average over its bins and the last seconds, and with nobody
/// singing a bin that removal does worse than its band's average, carrying most of a
/// frame's energy (a bass note through a cable's long low-frequency tail), must not pass
/// for the singer. Measured without it: up to 1.3 % of the frames shown as pitch with
/// nobody singing over a cable return as loud as the music (0.02 % with it).
pub(crate) const BAND_BIAS: f64 = 4.0;
/// The regression also takes the reference's fluctuation this many frames earlier (32 ms)
/// as a second regressor, and the left-over is the sum of both slopes: an echo path longer
/// than the filter (a cable's low-frequency tail) leaves a left-over that follows the music
/// of a few frames before, which the frame's own fluctuation does not explain (it even
/// pulls the slope down: the tail of quieter frames meets the frame's onset). Measured with
/// the frame's fluctuation alone: the left-over of cable returns with nobody singing 4-14 dB
/// under the truth; USB returns were right to 3 dB.
const LAG_FRAMES: usize = 3;
/// Successive frames overlap by three quarters and their power fluctuations are taken
/// around a 50 ms mean, so they are far from independent: the regression's sums count about
/// one independent frame in this many (for the standard errors).
const REG_KAPPA: f64 = 4.0;

/// Windows of the noise floor's minimum statistics (one window is about a second).
const NF_WINDOWS: usize = 8;

/// Noise floor by minimum statistics: the smallest value over the last [`NF_WINDOWS`]
/// windows (about 8 s) and the current one. Two one-second windows were used before, and
/// a singer who sang for more than a second or two without a pause became the "noise
/// floor" himself: his quieter moments then fell under the floor and were judged music
/// (measured: most of the singer's frames the gate took away with the music returned 10 dB
/// under the singer, docs/benchmarks/practice-echo.md section 13).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MinFloor {
    cur: f32,
    ring: [f32; NF_WINDOWS],
    pos: usize,
    len: usize,
    n: u32,
    window: u32,
}

impl MinFloor {
    /// Windows of `window` values.
    pub(crate) fn new(window: u32) -> Self {
        Self { cur: f32::MAX, ring: [f32::MAX; NF_WINDOWS], pos: 0, len: 0, n: 0, window: window.max(1) }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.window);
    }

    pub(crate) fn push(&mut self, v: f32) {
        self.cur = self.cur.min(v);
        self.n += 1;
        if self.n >= self.window {
            self.ring[self.pos] = self.cur;
            self.pos = (self.pos + 1) % NF_WINDOWS;
            self.len = (self.len + 1).min(NF_WINDOWS);
            self.cur = f32::MAX;
            self.n = 0;
        }
    }

    /// The floor (`f32::MAX` before any value).
    pub(crate) fn floor(&self) -> f32 {
        self.ring[..self.len].iter().fold(self.cur, |a, &b| a.min(b))
    }
}

/// Frame length (samples) of the spectral look at `rate` Hz with blocks of `b`: about 43 ms,
/// a power of two, at least four blocks.
pub(crate) fn frame_len(rate: u32, b: usize) -> usize {
    (1usize << ((0.043 * f64::from(rate)).log2().round() as u32)).max(4 * b)
}

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
    /// Noise floor of the band's output energy.
    nf: MinFloor,
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
    /// Long mean of the reference power per bin (see [`JUMP`]) and its averaging weight.
    lx: Vec<f32>,
    a_long: f32,
    /// `(1 - a_long)^frames` of music since the reset: the long mean is divided by
    /// `1 - long_pw`, so it is unbiased from the start.
    long_pw: f32,
    a_reg: f64,
    /// Per-bin regression sums (fluctuation of the output x of the reference, and of the
    /// reference squared), averaged over [`REG_S`].
    cov: Vec<f64>,
    var: Vec<f64>,
    reg_frames: u32,
    reg_min: u32,
    leak_min: u32,
    /// Per-bin average of the output's squared fluctuation (for the regression's standard
    /// errors) and frames regressed.
    vare: Vec<f64>,
    cnt: Vec<u32>,
    /// The lagged regressor (see [`LAG_FRAMES`]): per-bin averages of the output's
    /// fluctuation x the lagged reference fluctuation, of the lagged fluctuation squared and
    /// of the frame's x the lagged one; and a ring of the last frames' reference
    /// fluctuations (`[slot * nb + k]`, zero where a bin was not regressed) with its slot.
    cov1: Vec<f64>,
    var1: Vec<f64>,
    var01: Vec<f64>,
    dxh: Vec<f32>,
    dxh_pos: usize,
    /// Effective number of frames in the regression's average when it is full.
    n_full: f64,
    /// Third-octave band of each bin (`u16::MAX` outside the regression band); per band the
    /// sums of the reference-power-weighted slopes, of the weights and of the weighted
    /// slopes' variances; and per bin the cautious left-over per unit of reference power of
    /// its band (estimate + [`REG_Z`] standard errors), for the bound (see [`Spectral::push`]).
    band_of: Vec<u16>,
    bnum: Vec<f64>,
    bden: Vec<f64>,
    bvar: Vec<f64>,
    leak: Vec<f32>,
    /// The whole band's left-over per unit of reference power and its standard error, as of
    /// the latest frame (see [`Spectral::leak_slope`]).
    tot: Option<(f64, f64)>,
}

impl Spectral {
    /// For input at `rate` Hz in blocks of `b` samples.
    pub(crate) fn new(rate: u32, b: usize) -> Self {
        let fs = f64::from(rate);
        let n = frame_len(rate, b);
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
        let mut band_of = vec![u16::MAX; nb];
        let mut nbands = 0usize;
        let mut start = k_lo;
        let hz = |k: usize| k as f64 * fs / n as f64;
        for (k, b) in band_of.iter_mut().enumerate().take(k_reg).skip(k_lo) {
            if k - start >= BAND_MIN_BINS && hz(k) >= hz(start.max(1)) * 2f64.powf(1.0 / 3.0) {
                nbands += 1;
                start = k;
            }
            *b = nbands as u16;
        }
        let nbands = nbands + 1;
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
            nf: MinFloor::new((1.0 / frame_s).round().max(1.0) as u32),
            me: vec![0.0; nb],
            my: vec![0.0; nb],
            seen: vec![false; nb],
            sy: vec![0.0; nb],
            sx: vec![0.0; nb],
            a_slow: (1.0 - (-frame_s / SLOW_S).exp()) as f32,
            a_mean: (1.0 - (-frame_s / MEAN_S).exp()) as f32,
            lx: vec![0.0; nb],
            a_long: (1.0 - (-frame_s / LONG_S).exp()) as f32,
            long_pw: 1.0,
            a_reg: 1.0 - (-frame_s / REG_S).exp(),
            cov: vec![0.0; nb],
            var: vec![0.0; nb],
            reg_frames: 0,
            reg_min: (REG_MIN_S / frame_s).round() as u32,
            leak_min: (LEAK_MIN_S / frame_s).round() as u32,
            vare: vec![0.0; nb],
            cnt: vec![0; nb],
            cov1: vec![0.0; nb],
            var1: vec![0.0; nb],
            var01: vec![0.0; nb],
            dxh: vec![0.0; (LAG_FRAMES + 1) * nb],
            dxh_pos: 0,
            n_full: {
                let a = 1.0 - (-frame_s / REG_S).exp();
                (2.0 - a) / a
            },
            band_of,
            bnum: vec![0.0; nbands],
            bden: vec![0.0; nbands],
            bvar: vec![0.0; nbands],
            leak: vec![1.0; nb],
            tot: None,
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
        self.lx.fill(0.0);
        self.long_pw = 1.0;
        self.cov.fill(0.0);
        self.var.fill(0.0);
        self.vare.fill(0.0);
        self.cnt.fill(0);
        self.cov1.fill(0.0);
        self.var1.fill(0.0);
        self.var01.fill(0.0);
        self.dxh.fill(0.0);
        self.dxh_pos = 0;
        self.leak.fill(1.0);
        self.tot = None;
        self.reg_frames = 0;
        if all {
            self.nf.reset();
        }
    }

    /// Output power left per unit of reference power, from the regression, once it has
    /// seen enough music, and its standard error: each bin's slope, weighted by the bin's
    /// mean reference power (so it is the left-over power over the reference power, as a
    /// suppression figure means it; weighting by the fluctuations instead favours the
    /// loudest bins, where the filter learns first, and overstated the removal by up to
    /// 10 dB). Divided by the echo path's gain it is the share of the echo left in the
    /// output. The slopes are averaged as they are, negative ones included: a singer adds
    /// noise to every bin's slope, and clipping each at zero (as before) turned that noise
    /// into a bias that made the figure several times too pessimistic beside a singer far
    /// above the left-overs. The standard error comes from each bin's unexplained
    /// fluctuation (the singer, mostly), so a cautious figure is `estimate + REG_Z x error`.
    pub(crate) fn leak_slope(&self) -> Option<(f32, f32)> {
        if self.reg_frames < self.reg_min {
            return None;
        }
        self.tot.map(|(l, se)| (l as f32, se as f32))
    }

    /// Add one block: the output `e`, the output filter's echo estimate `y` and the aligned
    /// mono reference `x` (all `b` samples, the same input samples). Every
    /// [`Spectral::hop_blocks`] blocks, once the frame is full, analyses the newest
    /// [`Spectral::len`] samples and returns the frame's findings. `gain` is the echo path's
    /// power gain in band relative to the mono reference (the detector's level);
    /// `wandering` makes the bound ignore the filter's estimate and add 3 dB (the
    /// detector's level is biased low by a wandering delay); `music` false when the
    /// reference is silent (no regression). `use_leak`: the regression's left-over can be
    /// believed (removal running on a still echo path): then the bound in each bin is the
    /// most left-over there can be (the band's measured left-over per unit of reference
    /// power, with its margin, times the bin's reference power), not the whole echo, so a
    /// note quieter than the music it sits in (a high "ee" or "oo") is recognised once
    /// removal has taken the music well under it. The regression works on powers, so it
    /// does not care how often the filter takes new weights, and the singer adds noise to
    /// it but no bias.
    #[allow(clippy::too_many_arguments)] // one frame's signals and the canceller's state, read plainly
    pub(crate) fn push(
        &mut self,
        e: &[f32],
        y: &[f32],
        x: &[f32],
        gain: f32,
        wandering: bool,
        music: bool,
        use_leak: bool,
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
        let nbins = self.pe.len();
        let cur = self.dxh_pos * nbins;
        let lag = ((self.dxh_pos + 1) % (LAG_FRAMES + 1)) * nbins;
        if music {
            self.long_pw *= 1.0 - self.a_long;
        }
        let long_norm = 1.0 / (1.0 - self.long_pw).max(1e-6);
        for k in self.k_lo..self.k_reg {
            let (pe, py, px) = (self.pe[k], self.py[k], self.px[k]);
            if k < self.k_hi {
                let whole = if wandering {
                    2.0 * gain * px
                } else {
                    py.max(gain * px)
                };
                let lit = self.sx[k] > JUMP * self.lx[k] * long_norm;
                let bound = if use_leak && !wandering && !lit && self.reg_frames >= self.leak_min {
                    (self.leak[k] * px).max(LEAK_FLOOR * whole).min(whole)
                } else {
                    whole
                };
                total += pe;
                let ex = pe - MARGIN * bound;
                if ex > 0.0 {
                    excess += ex;
                }
            }
            self.sy[k] += al * (py - self.sy[k]);
            self.sx[k] += al * (px - self.sx[k]);
            if music {
                self.lx[k] += self.a_long * (px - self.lx[k]);
            }
            // Leakage regression on the bins without voice evidence against the slow means
            // (not the frame's own bound: choosing bins by it would keep a voice bin exactly
            // when the music there is loud, which biases the slope upwards).
            self.dxh[cur + k] = 0.0;
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
            let dx1 = f64::from(self.dxh[lag + k]);
            self.dxh[cur + k] = dx as f32;
            if first || self.cnt[k] == 0 {
                self.cov[k] = de * dx;
                self.var[k] = dx * dx;
                self.vare[k] = de * de;
                self.cov1[k] = de * dx1;
                self.var1[k] = dx1 * dx1;
                self.var01[k] = dx * dx1;
            } else {
                self.cov[k] += ar * (de * dx - self.cov[k]);
                self.var[k] += ar * (dx * dx - self.var[k]);
                self.vare[k] += ar * (de * de - self.vare[k]);
                self.cov1[k] += ar * (de * dx1 - self.cov1[k]);
                self.var1[k] += ar * (dx1 * dx1 - self.var1[k]);
                self.var01[k] += ar * (dx * dx1 - self.var01[k]);
            }
            self.cnt[k] = self.cnt[k].saturating_add(1);
            regressed = true;
            self.me[k] += a * de as f32;
            self.my[k] += a * dx as f32;
        }
        if regressed {
            self.reg_frames = self.reg_frames.saturating_add(1);
        }
        self.dxh_pos = (self.dxh_pos + 1) % (LAG_FRAMES + 1);
        // The per-band left-over (weights: the bins' reference power, as in `leak_slope`)
        // with its standard error: a slope's variance is the bin's unexplained output
        // fluctuation over its reference fluctuation and the effective number of frames.
        self.bnum.fill(0.0);
        self.bden.fill(0.0);
        self.bvar.fill(0.0);
        for k in self.k_lo..self.k_reg {
            let bi = usize::from(self.band_of[k]);
            if bi < self.bnum.len() && self.var[k] > 1e-40 && self.cnt[k] >= 2 {
                let w = f64::from(self.sx[k]);
                let n = (f64::from(self.cnt[k]).min(self.n_full) / REG_KAPPA).max(1.0);
                // Two regressors (the frame's and the lagged fluctuation); the left-over is
                // the sum of their slopes, with variance sigma^2 (1 1) V^-1 (1 1)' / n.
                let (v00, v11, v01) = (self.var[k], self.var1[k], self.var01[k]);
                let (c0, c1) = (self.cov[k], self.cov1[k]);
                let det = v00 * v11 - v01 * v01;
                let (slope, var_slope) = if v11 > 1e-40 && det > 1e-3 * v00 * v11 {
                    let b0 = (v11 * c0 - v01 * c1) / det;
                    let b1 = (v00 * c1 - v01 * c0) / det;
                    let unexplained = (self.vare[k] - b0 * c0 - b1 * c1).max(0.0);
                    (b0 + b1, unexplained * (v00 + v11 - 2.0 * v01) / det / n)
                } else {
                    let b0 = c0 / v00;
                    let unexplained = (self.vare[k] - b0 * c0).max(0.0);
                    (b0, unexplained / (v00 * n))
                };
                self.bnum[bi] += w * slope;
                self.bden[bi] += w;
                self.bvar[bi] += w * w * var_slope;
            }
        }
        let (mut tn, mut td, mut tv) = (0.0f64, 0.0f64, 0.0f64);
        for bi in 0..self.bnum.len() {
            tn += self.bnum[bi];
            td += self.bden[bi];
            tv += self.bvar[bi];
        }
        self.tot = (td > 1e-40).then(|| (tn / td, tv.sqrt() / td));
        for k in self.k_lo..self.k_reg {
            let bi = usize::from(self.band_of[k]);
            if bi < self.bnum.len() {
                self.leak[k] = if self.bden[bi] > 1e-40 {
                    let (l, se) = (self.bnum[bi] / self.bden[bi], self.bvar[bi].sqrt() / self.bden[bi]);
                    ((l.max(0.0) + REG_Z * se) * BAND_BIAS).clamp(1e-7, 1e3) as f32
                } else {
                    1e3
                };
            }
        }
        // Noise floor of the band's energy.
        self.nf.push(total);
        let noise = self.nf.floor();
        let share = if total > 0.0 { excess / total } else { 0.0 };
        let voice = total > 1e-20
            && share >= VOICE_SHARE
            && excess > NOISE_FACTOR * noise.min(total);
        Some(Frame { voice })
    }
}

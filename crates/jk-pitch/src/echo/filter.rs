// SPDX-License-Identifier: GPL-3.0-or-later
//! The adaptive filter of the echo canceller: a partitioned-block frequency-domain NLMS
//! filter (multi-delay filter, overlap-save, constrained gradient) with one set of
//! partitions per reference channel, run as two paths:
//!
//! - the *background* filter learns all the time, with a learning rate per frequency bin
//!   that follows the double-talk robust rule of J.-M. Valin, "On adjusting the learning
//!   rate in frequency domain echo cancellation with double-talk", IEEE Trans. Audio, Speech
//!   and Language Processing 15(3), 2007: the step in a bin is the expected share of the
//!   error that is left-over echo, `leak * |Y(k)|^2 / |E(k)|^2`, where `leak` is the
//!   measured leakage (the regression of the error's power fluctuations on the echo
//!   estimate's), floored at -23 dB so the filter keeps learning, and never below the share
//!   the error and the echo estimate visibly have in common. When the singer sings, the error
//!   grows and the step shrinks by itself;
//! - the *foreground* filter makes the output. It takes the background's weights only when
//!   the background's error has been clearly smaller for a while (a statistically
//!   significant drop, judged on two time scales), and gives its own weights back when the
//!   background has clearly gone wrong (a burst of singing it adapted to). So a mistake of
//!   the background never reaches the output. The caller may also hold the foreground
//!   while somebody sings (`allow_update` false): the background's error is then lower
//!   whenever it has learnt a little of the voice, which is not an improvement. Measured on
//!   a cable return with a singer, without this hold the foreground took the background's
//!   weights about 60 times a second and the suppression swung between 6 and 24 dB.
//!
//! The two-path structure and the rate rule follow the published design of the Speex
//! echo canceller (Valin, 2007); this is an independent implementation.
//!
//! Before the leakage can be trusted (the first moments after the filter starts) the same
//! rule runs on the echo the detector expects instead of the filter's own estimate: in each
//! bin the step is the expected echo power (the detector's level times the reference power)
//! over the error power, at most 0.25. A singer over a quiet return therefore barely moves
//! the filter even while it starts.
//!
//! The share the error and the echo estimate have in common (a lower bound on the step) is
//! measured over about 40 ms, not over one 2.7 ms block: in so short a block a voice and the
//! music are correlated by chance often enough to keep the filter learning the voice.
//!
//! Block `b` samples, FFT length `2b`, `p` partitions per channel: the filter covers
//! `p * b` taps. No allocation after [`Pbfdaf::new`].

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Smallest leakage used for the learning rate (-23 dB): the filter never stops learning
/// because it believes itself better than it is.
const MIN_LEAK: f32 = 0.005;
/// Fixed step of the start-up phase.
const MU_START: f32 = 0.25;
/// Largest step once adapted.
const MU_MAX: f32 = 0.5;
/// The steps above are relative to the reference power summed over all partitions (the
/// whole filter); this factor speeds learning up. Measured on generated music: 2 reaches
/// 33 dB of suppression after 4 s of a USB return instead of 26 dB, and keeps the
/// steady-state suppression of a cable return; 4 loses 2 dB of it.
const STEP_GAIN: f32 = 2.0;

/// Per-block powers (sums of squares over the block, time domain).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BlockStats {
    /// Input (microphone) block.
    pub pd: f32,
    /// Output (foreground error) block.
    pub pe: f32,
    /// Foreground echo estimate block.
    pub py: f32,
    /// Reference block (all channels).
    pub px: f32,
}

/// See the module documentation.
pub(crate) struct Pbfdaf {
    b: usize,
    n: usize,
    nb: usize,
    p: usize,
    ch: usize,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Last `n` time samples of every reference channel (channel-major).
    xtime: Vec<f32>,
    /// Reference spectra, a ring of `p` per channel: `[(c * p + slot) * nb + k]`.
    xs: Vec<Complex<f32>>,
    head: usize,
    /// Background and foreground weights `[(c * p + part) * nb + k]`.
    w: Vec<Complex<f32>>,
    wf: Vec<Complex<f32>>,
    tbuf: Vec<f32>,
    /// Echo estimates of the two paths in time (last `b` samples valid).
    yf_t: Vec<f32>,
    yb_t: Vec<f32>,
    /// Background error in time.
    eb_t: Vec<f32>,
    y: Vec<Complex<f32>>,
    e: Vec<Complex<f32>>,
    yb: Vec<Complex<f32>>,
    g: Vec<Complex<f32>>,
    sxx: Vec<f32>,
    mu: Vec<f32>,
    /// Smoothed per-bin powers of the background error and echo estimate (leakage).
    pe_avg: Vec<f32>,
    py_avg: Vec<f32>,
    pey: f32,
    pyy: f32,
    leak: f32,
    sum_adapt: f32,
    adapted: bool,
    /// Echo power expected per unit of reference power (from the detector), for the
    /// start-up steps.
    hint: f32,
    /// Error x echo-estimate, error and echo-estimate energies smoothed over about 40 ms.
    sey_s: f32,
    see_s: f32,
    syy_s: f32,
    a_corr: f32,
    /// Foreground/background decision statistics (two time scales).
    davg1: f32,
    davg2: f32,
    dvar1: f32,
    dvar2: f32,
    /// Smoothing weights per block.
    a_spec: f32,
    beta0: f32,
    beta_max: f32,
    /// Divergence guard: smoothed input and output power, and blocks the output has been
    /// louder than the input.
    pd_s: f32,
    pe_s: f32,
    diverge_blocks: u32,
    resets: u32,
    /// Foreground updates (diagnostics).
    updates: u32,
    /// The background does not learn while set: the canceller's spectral look found the
    /// singer in the output (a voice holding a note over the same note in the music is
    /// otherwise slowly learnt as if the music predicted it; measured with continuous
    /// singing: the output filter then took weights 2.3 times the echo).
    frozen: bool,
}

impl Pbfdaf {
    /// A filter of `p` partitions of `b` samples for `ch` reference channels at `rate` Hz.
    pub(crate) fn new(rate: u32, b: usize, p: usize, ch: usize) -> Self {
        let n = 2 * b;
        let nb = b + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let zc = Complex::new(0.0f32, 0.0);
        let blk_s = b as f64 / f64::from(rate);
        let w = |tau: f64| (1.0 - (-blk_s / tau).exp()) as f32;
        let beta_max = w(0.5);
        Self {
            b,
            n,
            nb,
            p,
            ch,
            r2c,
            c2r,
            scratch: vec![zc; scratch_len],
            xtime: vec![0.0; ch * n],
            xs: vec![zc; ch * p * nb],
            head: 0,
            w: vec![zc; ch * p * nb],
            wf: vec![zc; ch * p * nb],
            tbuf: vec![0.0; n],
            yf_t: vec![0.0; b],
            yb_t: vec![0.0; b],
            eb_t: vec![0.0; b],
            y: vec![zc; nb],
            e: vec![zc; nb],
            yb: vec![zc; nb],
            g: vec![zc; nb],
            sxx: vec![0.0; nb],
            mu: vec![0.0; nb],
            pe_avg: vec![0.0; nb],
            py_avg: vec![0.0; nb],
            pey: 0.0,
            pyy: 0.0,
            leak: 1.0,
            sum_adapt: 0.0,
            adapted: false,
            hint: 1.0,
            sey_s: 0.0,
            see_s: 0.0,
            syy_s: 0.0,
            a_corr: w(0.04),
            davg1: 0.0,
            davg2: 0.0,
            dvar1: 0.0,
            dvar2: 0.0,
            a_spec: w(0.02),
            beta0: 4.0 * beta_max,
            beta_max,
            pd_s: 0.0,
            pe_s: 0.0,
            diverge_blocks: 0,
            resets: 0,
            updates: 0,
            frozen: false,
        }
    }

    /// Forget the echo path (weights and statistics); the reference history is kept unless
    /// `history` is true. Does not allocate.
    pub(crate) fn reset(&mut self, history: bool) {
        let zc = Complex::new(0.0f32, 0.0);
        self.w.fill(zc);
        self.wf.fill(zc);
        self.frozen = false;
        if history {
            self.xtime.fill(0.0);
            self.xs.fill(zc);
        }
        self.pe_avg.fill(0.0);
        self.py_avg.fill(0.0);
        self.pey = 0.0;
        self.pyy = 0.0;
        self.leak = 1.0;
        self.sum_adapt = 0.0;
        self.adapted = false;
        self.sey_s = 0.0;
        self.see_s = 0.0;
        self.syy_s = 0.0;
        self.davg1 = 0.0;
        self.davg2 = 0.0;
        self.dvar1 = 0.0;
        self.dvar2 = 0.0;
        self.pd_s = 0.0;
        self.pe_s = 0.0;
        self.diverge_blocks = 0;
    }

    /// The echo power the detector expects, relative to the reference power (linear), for
    /// the start-up steps. Takes effect from the next block.
    pub(crate) fn set_hint(&mut self, level: f32) {
        self.hint = level.clamp(1e-6, 4.0);
    }

    /// Whether the start-up phase is over (the learning rate follows the leakage).
    pub(crate) fn adapted(&self) -> bool {
        self.adapted
    }

    /// Leakage of the background filter as used for its learning rate (floored at -23 dB).
    pub(crate) fn leak(&self) -> f32 {
        self.leak
    }

    /// Times the divergence guard reset the filter.
    pub(crate) fn resets(&self) -> u32 {
        self.resets
    }

    /// Hold the background's learning (true) or let it learn (false), from the next block.
    pub(crate) fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Whether the learning filter is held (diagnostics).
    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Times the foreground took the background's weights.
    pub(crate) fn updates(&self) -> u32 {
        self.updates
    }

    /// The foreground's echo estimate for the block just processed (`b` samples): what
    /// [`Pbfdaf::step`] subtracted from the input.
    pub(crate) fn fore_estimate(&self) -> &[f32] {
        &self.yf_t
    }

    /// Weight index.
    #[inline]
    fn wi(&self, c: usize, part: usize) -> usize {
        (c * self.p + part) * self.nb
    }

    /// Reference spectrum index of the block `age` blocks old.
    #[inline]
    fn xi(&self, c: usize, age: usize) -> usize {
        (c * self.p + (self.head + self.p - age) % self.p) * self.nb
    }

    /// Echo estimate of the weights `fore` (foreground) or the background, in time, into
    /// `tbuf[b..]` (scaled).
    fn estimate(&mut self, fore: bool) {
        let (nb, p, ch) = (self.nb, self.p, self.ch);
        let zc = Complex::new(0.0f32, 0.0);
        self.y.fill(zc);
        for c in 0..ch {
            for part in 0..p {
                let wi = self.wi(c, part);
                let xi = self.xi(c, part);
                let w = if fore { &self.wf } else { &self.w };
                for k in 0..nb {
                    self.y[k] += w[wi + k] * self.xs[xi + k];
                }
            }
        }
        self.y[0].im = 0.0;
        self.y[nb - 1].im = 0.0;
        let _ = self
            .c2r
            .process_with_scratch(&mut self.y, &mut self.tbuf, &mut self.scratch);
        let inv_n = 1.0 / self.n as f32;
        for v in &mut self.tbuf[self.b..] {
            *v *= inv_n;
        }
    }

    /// Process one block: `x` holds `b` new reference samples per channel (channel-major),
    /// `d` the `b` input samples; the echo-free output goes to `out`.
    pub(crate) fn step(
        &mut self,
        x: &[f32],
        d: &[f32],
        out: &mut [f32],
        allow_update: bool,
    ) -> BlockStats {
        let (b, n, nb, p, ch) = (self.b, self.n, self.nb, self.p, self.ch);
        let inv_n = 1.0 / n as f32;
        let mut st = BlockStats::default();

        // 1. New reference spectra (the last 2b samples of each channel).
        self.head = (self.head + 1) % p;
        for c in 0..ch {
            let hist = &mut self.xtime[c * n..(c + 1) * n];
            hist.copy_within(b..n, 0);
            let new = &x[c * b..(c + 1) * b];
            hist[b..].copy_from_slice(new);
            st.px += new.iter().map(|v| v * v).sum::<f32>();
            self.tbuf.copy_from_slice(hist);
            let at = self.xi(c, 0);
            let _ = self.r2c.process_with_scratch(
                &mut self.tbuf,
                &mut self.xs[at..at + nb],
                &mut self.scratch,
            );
        }

        // 2. Echo estimates of both paths, and their errors.
        self.estimate(true);
        self.yf_t.copy_from_slice(&self.tbuf[b..]);
        self.estimate(false);
        self.yb_t.copy_from_slice(&self.tbuf[b..]);
        let (mut sff, mut see, mut dbf, mut sey, mut syy) =
            (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for i in 0..b {
            let ef = d[i] - self.yf_t[i];
            let eb = d[i] - self.yb_t[i];
            self.eb_t[i] = eb;
            out[i] = ef;
            st.pd += d[i] * d[i];
            sff += ef * ef;
            see += eb * eb;
            sey += eb * self.yb_t[i];
            syy += self.yb_t[i] * self.yb_t[i];
            let dy = self.yf_t[i] - self.yb_t[i];
            dbf += dy * dy;
        }
        dbf += 1e-10;

        // 3. Foreground or background: does the background do clearly better (copy it to
        //    the foreground), or clearly worse (give it the foreground's weights back)?
        let diff = sff - see;
        self.davg1 = 0.6 * self.davg1 + 0.4 * diff;
        self.davg2 = 0.85 * self.davg2 + 0.15 * diff;
        self.dvar1 = 0.36 * self.dvar1 + 0.16 * sff * dbf;
        self.dvar2 = 0.7225 * self.dvar2 + 0.0225 * sff * dbf;
        let better = diff * diff.abs() > sff * dbf
            || self.davg1 * self.davg1.abs() > 0.5 * self.dvar1
            || self.davg2 * self.davg2.abs() > 0.25 * self.dvar2;
        let worse = -diff * diff.abs() > 4.0 * sff * dbf
            || -self.davg1 * self.davg1.abs() > 4.0 * self.dvar1
            || -self.davg2 * self.davg2.abs() > 4.0 * self.dvar2;
        if better && allow_update {
            self.wf.copy_from_slice(&self.w);
            self.updates += 1;
            // Cross-fade the output from the old to the new foreground over the block.
            for i in 0..b {
                let r = (i as f32 + 0.5) / b as f32;
                out[i] = (1.0 - r) * out[i] + r * self.eb_t[i];
            }
            self.yf_t.copy_from_slice(&self.yb_t);
            self.davg1 = 0.0;
            self.davg2 = 0.0;
            self.dvar1 = 0.0;
            self.dvar2 = 0.0;
        } else if worse && !better {
            self.w.copy_from_slice(&self.wf);
            self.yb_t.copy_from_slice(&self.yf_t);
            self.eb_t.copy_from_slice(&out[..b]);
            see = sff;
            sey = 0.0;
            syy = 0.0;
            for i in 0..b {
                sey += self.eb_t[i] * self.yb_t[i];
                syy += self.yb_t[i] * self.yb_t[i];
            }
            self.davg1 = 0.0;
            self.davg2 = 0.0;
            self.dvar1 = 0.0;
            self.dvar2 = 0.0;
        }
        for i in 0..b {
            st.pe += out[i] * out[i];
            st.py += self.yf_t[i] * self.yf_t[i];
        }

        // 4. Spectra of the background error and echo estimate, zero-padded in front.
        self.tbuf[..b].fill(0.0);
        self.tbuf[b..].copy_from_slice(&self.yb_t);
        let _ = self
            .r2c
            .process_with_scratch(&mut self.tbuf, &mut self.yb, &mut self.scratch);
        self.tbuf[..b].fill(0.0);
        self.tbuf[b..].copy_from_slice(&self.eb_t);
        let _ = self
            .r2c
            .process_with_scratch(&mut self.tbuf, &mut self.e, &mut self.scratch);

        // Divergence guard: an output much louder than the input for a while means the
        // filter is adding echo instead of removing it (an echo path that changed at once).
        let a = self.a_spec;
        self.pd_s += a * (st.pd - self.pd_s);
        self.pe_s += a * (st.pe - self.pe_s);
        if self.pe_s > 2.0 * self.pd_s + 1e-12 {
            self.diverge_blocks += 1;
            if self.diverge_blocks > 40 {
                self.reset(false);
                self.resets += 1;
            }
        } else {
            self.diverge_blocks = 0;
        }

        // 5. Leakage: regress the error's per-bin power fluctuations on the echo estimate's.
        let mut see_f = 0.0f32;
        let mut syy_f = 0.0f32;
        let mut pey = 0.0f32;
        let mut pyy = 0.0f32;
        for k in 0..nb {
            let pe = self.e[k].norm_sqr();
            let py = self.yb[k].norm_sqr();
            see_f += pe;
            syy_f += py;
            let eh = pe - self.pe_avg[k];
            let yh = py - self.py_avg[k];
            pey += eh * yh;
            pyy += yh * yh;
            self.pe_avg[k] += a * (pe - self.pe_avg[k]);
            self.py_avg[k] += a * (py - self.py_avg[k]);
        }
        let ref_active = st.px > 1e-9 * b as f32;
        if !ref_active || self.frozen {
            return st;
        }
        if syy_f > 1e-20 && see_f > 1e-20 {
            let beta = (self.beta0 * syy_f / see_f).min(self.beta_max);
            self.pey += beta * (pey - self.pey);
            self.pyy += beta * (pyy - self.pyy);
            if self.pyy > 1e-30 {
                self.leak = (self.pey / self.pyy).clamp(MIN_LEAK, 1.0);
            }
        }

        // 6. Learning rate per bin.
        if self.adapted {
            // Residual-to-error ratio over the whole block: three times the leakage's share,
            // and at least the share the error visibly has in common with the echo estimate.
            let ac = self.a_corr;
            self.sey_s += ac * (sey - self.sey_s);
            self.see_s += ac * (see - self.see_s);
            self.syy_s += ac * (syy - self.syy_s);
            let mut rer = 3.0 * self.leak * syy / (see + 1e-20);
            rer = rer
                .max(self.sey_s * self.sey_s / (self.see_s * self.syy_s + 1e-20))
                .min(0.5);
            for k in 0..nb {
                let pe = self.e[k].norm_sqr() + 1e-20;
                let r = (self.leak * self.yb[k].norm_sqr()).min(0.5 * pe);
                self.mu[k] = ((0.7 * r + 0.3 * rer * pe) / pe).min(MU_MAX);
            }
        } else {
            // Start-up: the expected echo (detector level x reference power per bin) over the
            // error, per bin. The newest reference spectra cover 2b samples, the error's b.
            let scale = 0.5 * self.hint / ch as f32;
            let mut sum = 0.0f32;
            for k in 0..nb {
                let mut xp = 0.0f32;
                for c in 0..ch {
                    xp += self.xs[self.xi(c, 0) + k].norm_sqr();
                }
                let pe = self.e[k].norm_sqr() + 1e-20;
                self.mu[k] = (scale * xp / pe).min(MU_START);
                sum += self.mu[k];
            }
            let rate = sum / nb as f32;
            self.sum_adapt += rate;
            if self.sum_adapt > 2.0 * p as f32 {
                self.adapted = true;
            }
        }

        // 7. Normalised, constrained gradient step of the background for every channel and
        //    partition.
        self.sxx.fill(0.0);
        for c in 0..ch {
            for age in 0..p {
                let xi = self.xi(c, age);
                for k in 0..nb {
                    self.sxx[k] += self.xs[xi + k].norm_sqr();
                }
            }
        }
        let mean = self.sxx.iter().sum::<f32>() / nb as f32;
        let delta = 1e-3 * mean + (ch * p * n) as f32 * 1e-9;
        for k in 0..nb {
            self.mu[k] *= STEP_GAIN / (self.sxx[k] + delta);
        }
        for c in 0..ch {
            for part in 0..p {
                let xi = self.xi(c, part);
                for k in 0..nb {
                    self.g[k] = self.xs[xi + k].conj() * self.e[k] * self.mu[k];
                }
                self.g[0].im = 0.0;
                self.g[nb - 1].im = 0.0;
                let _ =
                    self.c2r
                        .process_with_scratch(&mut self.g, &mut self.tbuf, &mut self.scratch);
                // Keep the first b taps (the partition's span), scaled back from the
                // unnormalised inverse transform.
                for v in &mut self.tbuf[..b] {
                    *v *= inv_n;
                }
                self.tbuf[b..].fill(0.0);
                let _ =
                    self.r2c
                        .process_with_scratch(&mut self.tbuf, &mut self.g, &mut self.scratch);
                let wi = self.wi(c, part);
                for k in 0..nb {
                    self.w[wi + k] += self.g[k];
                }
            }
        }
        st
    }

    /// The foreground filter's impulse response for channel `c` (`p * b` taps) into `out`.
    /// For diagnostics and the delay measurement.
    pub(crate) fn impulse(&mut self, c: usize, out: &mut [f32]) {
        let (b, nb) = (self.b, self.nb);
        let inv_n = 1.0 / self.n as f32;
        for part in 0..self.p {
            let wi = self.wi(c, part);
            self.g.copy_from_slice(&self.wf[wi..wi + nb]);
            self.g[0].im = 0.0;
            self.g[nb - 1].im = 0.0;
            let _ = self
                .c2r
                .process_with_scratch(&mut self.g, &mut self.tbuf, &mut self.scratch);
            for i in 0..b {
                if part * b + i < out.len() {
                    out[part * b + i] = self.tbuf[i] * inv_n;
                }
            }
        }
    }
}

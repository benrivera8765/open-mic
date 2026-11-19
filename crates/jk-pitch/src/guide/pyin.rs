// SPDX-License-Identifier: GPL-3.0-or-later
//! pYIN-style pitch tracking for the guide (offline, whole file at once).
//!
//! Follows M. Mauch and S. Dixon, "pYIN: a fundamental frequency estimator using
//! probabilistic threshold distributions", ICASSP 2014, with the parameter choices of
//! librosa's `pyin` (thresholds 0.01..1 under a Beta(2, 18) prior, Boltzmann prior over
//! troughs, `no_trough_prob` 0.01, voicing switch 0.01, at most 35.92 octaves/s), written
//! from the paper, not copied:
//!
//! 1. YIN cumulative-mean-normalised difference function per frame, computed with one
//!    real FFT cross-correlation (realfft) and running energies.
//! 2. Every threshold picks the first trough below it; the thresholds' prior turns the
//!    troughs into pitch candidates with probabilities. Their sum is the frame's voicing
//!    probability.
//! 3. An HMM over (pitch bin x {voiced, unvoiced}) with local triangular pitch transitions,
//!    decoded with Viterbi, chooses one path through the candidates.
//! 4. Each voiced frame reports the exact (parabolically interpolated) candidate nearest
//!    to its decoded bin, so the bin size only limits the path, not the output accuracy.

use super::{GuideConfig, PitchCurve};
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

/// Frame layout at one analysis rate.
struct Geometry {
    sr: f64,
    win: usize,
    tau_min: usize,
    tau_max: usize,
    span: usize,
    n_fft: usize,
}

impl Geometry {
    fn new(sr: f64, cfg: &GuideConfig) -> Self {
        let win = (cfg.window_s * sr).round().max(16.0) as usize;
        let tau_max = (sr / cfg.fmin_hz).ceil() as usize + 1;
        let tau_min = ((sr / cfg.fmax_hz).floor() as usize)
            .saturating_sub(1)
            .max(2);
        let span = win + tau_max + 1;
        Geometry {
            sr,
            win,
            tau_min,
            tau_max,
            span,
            n_fft: span.next_power_of_two(),
        }
    }

    /// First sample of frame `k` (centred on `k * hop_s`); may be negative.
    fn start(&self, k: usize, hop_s: f64) -> isize {
        (k as f64 * hop_s * self.sr).round() as isize - (self.span / 2) as isize
    }
}

fn frame_count(n_samples: usize, sr: f64, hop_s: f64) -> usize {
    if n_samples == 0 {
        return 0;
    }
    ((n_samples as f64 / sr) / hop_s).floor() as usize + 1
}

fn fill(buf: &mut [f64], x: &[f32], start: isize) {
    for (j, v) in buf.iter_mut().enumerate() {
        let i = start + j as isize;
        *v = if i >= 0 && (i as usize) < x.len() {
            x[i as usize] as f64
        } else {
            0.0
        };
    }
}

fn level_db(frame: &[f64], g: &Geometry) -> f32 {
    let off = (g.span - g.win) / 2;
    let e: f64 = frame[off..off + g.win].iter().map(|v| v * v).sum::<f64>() / g.win as f64;
    (10.0 * (e + 1e-12).log10()) as f32
}

/// Frame levels in dBFS on the guide's frame grid (used when the pitch curve comes from
/// another tracker).
pub fn frame_levels(x: &[f32], sr: f64, cfg: &GuideConfig, frames: usize) -> Vec<f32> {
    let g = Geometry::new(sr, cfg);
    let mut buf = vec![0.0f64; g.span];
    (0..frames)
        .map(|k| {
            fill(&mut buf, x, g.start(k, cfg.hop_s));
            level_db(&buf, &g)
        })
        .collect()
}

/// Beta(a, b) CDF for integer a, b (a binomial tail sum).
fn beta_cdf_int(x: f64, a: u32, b: u32) -> f64 {
    let n = a + b - 1;
    let mut sum = 0.0;
    for j in a..=n {
        sum += binom(n, j) * x.powi(j as i32) * (1.0 - x).powi((n - j) as i32);
    }
    sum
}

fn binom(n: u32, k: u32) -> f64 {
    let mut r = 1.0f64;
    for i in 0..k {
        r = r * (n - i) as f64 / (i + 1) as f64;
    }
    r
}

/// Probabilities of the 100 thresholds 0.01..1.00 under Beta(2, 18) (mean 0.1).
fn threshold_prior() -> ([f64; 100], [f64; 100]) {
    let mut thr = [0.0; 100];
    let mut p = [0.0; 100];
    let mut prev = 0.0;
    for i in 0..100 {
        let s = (i + 1) as f64 / 100.0;
        thr[i] = s;
        let c = beta_cdf_int(s, 2, 18);
        p[i] = c - prev;
        prev = c;
    }
    (thr, p)
}

/// Pitch candidates `(Hz, probability)` of every frame, and the frames' levels.
fn candidates(
    x: &[f32],
    g: &Geometry,
    cfg: &GuideConfig,
    frames: usize,
) -> (Vec<Vec<(f64, f64)>>, Vec<f32>) {
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(g.n_fft);
    let inv = planner.plan_fft_inverse(g.n_fft);
    let mut a_in = fwd.make_input_vec();
    let mut b_in = fwd.make_input_vec();
    let mut a_sp = fwd.make_output_vec();
    let mut b_sp = fwd.make_output_vec();
    let mut r = inv.make_output_vec();
    let mut scratch_f = fwd.make_scratch_vec();
    let mut scratch_i = inv.make_scratch_vec();
    let mut frame = vec![0.0f64; g.span];
    let mut sq_prefix = vec![0.0f64; g.span + 1];
    let mut cmnd = vec![1.0f64; g.tau_max + 2];
    let mut diff = vec![0.0f64; g.tau_max + 2];
    let (thr, thr_p) = threshold_prior();
    let boltzmann = 2.0f64;
    let no_trough_prob = 0.01;

    let mut all = Vec::with_capacity(frames);
    let mut levels = Vec::with_capacity(frames);
    let mut troughs: Vec<(usize, f64)> = Vec::new();
    let mut probs: Vec<f64> = Vec::new();
    for k in 0..frames {
        fill(&mut frame, x, g.start(k, cfg.hop_s));
        levels.push(level_db(&frame, g));
        for (i, v) in frame.iter().enumerate() {
            sq_prefix[i + 1] = sq_prefix[i] + v * v;
        }
        let e0 = sq_prefix[g.win];
        if e0 < 1e-10 * g.win as f64
            || sq_prefix[g.span] - sq_prefix[g.span - g.win] < 1e-10 * g.win as f64
        {
            all.push(Vec::new());
            continue;
        }
        // r[tau] = sum_j a[j] b[j + tau]: a = first `win` samples, b = whole frame.
        a_in.iter_mut().for_each(|v| *v = 0.0);
        a_in[..g.win].copy_from_slice(&frame[..g.win]);
        b_in.iter_mut().for_each(|v| *v = 0.0);
        b_in[..g.span].copy_from_slice(&frame);
        fwd.process_with_scratch(&mut a_in, &mut a_sp, &mut scratch_f)
            .expect("fft sizes");
        fwd.process_with_scratch(&mut b_in, &mut b_sp, &mut scratch_f)
            .expect("fft sizes");
        for (p, q) in a_sp.iter_mut().zip(&b_sp) {
            *p = p.conj() * q;
        }
        // The inverse transform needs real DC and Nyquist bins.
        a_sp[0] = Complex::new(a_sp[0].re, 0.0);
        let last = a_sp.len() - 1;
        a_sp[last] = Complex::new(a_sp[last].re, 0.0);
        inv.process_with_scratch(&mut a_sp, &mut r, &mut scratch_i)
            .expect("fft sizes");
        let scale = 1.0 / g.n_fft as f64;

        // Cumulative mean normalised difference d'(tau).
        let mut running = 0.0;
        cmnd[0] = 1.0;
        for tau in 1..=g.tau_max + 1 {
            let e_tau = sq_prefix[tau + g.win] - sq_prefix[tau];
            let d = (e0 + e_tau - 2.0 * r[tau] * scale).max(0.0);
            diff[tau] = d;
            running += d;
            cmnd[tau] = if running > 0.0 {
                d * tau as f64 / running
            } else {
                1.0
            };
        }

        // Troughs (local minima) inside the searched lag range.
        troughs.clear();
        for tau in g.tau_min.max(1)..=g.tau_max {
            if cmnd[tau] < cmnd[tau - 1] && cmnd[tau] <= cmnd[tau + 1] {
                troughs.push((tau, cmnd[tau]));
            }
        }
        if troughs.is_empty() {
            all.push(Vec::new());
            continue;
        }
        probs.clear();
        probs.resize(troughs.len(), 0.0);
        // Every threshold gives its prior mass to the troughs below it, earlier troughs
        // first (Boltzmann over the rank among the troughs below the threshold).
        for (s, ps) in thr.iter().zip(&thr_p) {
            let below = troughs.iter().filter(|t| t.1 < *s).count();
            if below == 0 {
                continue;
            }
            let norm = (1.0 - (-boltzmann).exp()) / (1.0 - (-boltzmann * below as f64).exp());
            let mut rank = 0;
            for (i, t) in troughs.iter().enumerate() {
                if t.1 < *s {
                    probs[i] += ps * norm * (-boltzmann * rank as f64).exp();
                    rank += 1;
                }
            }
        }
        // Thresholds below every trough: a small share goes to the global minimum.
        let (gmin, gval) = troughs
            .iter()
            .enumerate()
            .fold(
                (0, f64::INFINITY),
                |acc, (i, t)| if t.1 < acc.1 { (i, t.1) } else { acc },
            );
        let unclaimed: f64 = thr
            .iter()
            .zip(&thr_p)
            .filter(|(s, _)| gval >= **s)
            .map(|(_, p)| p)
            .sum();
        probs[gmin] += no_trough_prob * unclaimed;

        let mut c = Vec::with_capacity(troughs.len());
        for (t, p) in troughs.iter().zip(&probs) {
            if *p <= 0.0 {
                continue;
            }
            let tau = t.0;
            // Interpolate on the raw difference function (YIN step 5): the cumulative-mean
            // normalisation tilts it and biases short lags (high notes) by several cents.
            let (y0, y1, y2) = (diff[tau - 1], diff[tau], diff[tau + 1]);
            let den = y0 - 2.0 * y1 + y2;
            let shift = if den.abs() > 1e-12 {
                (0.5 * (y0 - y2) / den).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            c.push((g.sr / (tau as f64 + shift), *p));
        }
        all.push(c);
    }
    (all, levels)
}

/// Tracks the pitch of `x` (mono, rate `sr`).
pub fn track(x: &[f32], sr: f64, cfg: &GuideConfig) -> PitchCurve {
    let g = Geometry::new(sr, cfg);
    let frames = frame_count(x.len(), sr, cfg.hop_s);
    let (cands, level_db) = candidates(x, &g, cfg, frames);
    let (f0_hz, conf) = viterbi(&cands, cfg);
    PitchCurve {
        hop_s: cfg.hop_s,
        f0_hz,
        conf,
        level_db,
    }
}

/// HMM decoding over pitch bins x {voiced, unvoiced}.
fn viterbi(cands: &[Vec<(f64, f64)>], cfg: &GuideConfig) -> (Vec<f32>, Vec<f32>) {
    let frames = cands.len();
    let bins_per_octave = 1200.0 / cfg.bin_cents;
    let n = (bins_per_octave * (cfg.fmax_hz / cfg.fmin_hz).log2()).floor() as usize + 1;
    let bin_of = |f: f64| -> usize {
        ((bins_per_octave * (f / cfg.fmin_hz).log2())
            .round()
            .max(0.0) as usize)
            .min(n - 1)
    };
    let hz_of = |b: usize| cfg.fmin_hz * 2f64.powf(b as f64 / bins_per_octave);
    let semis = (cfg.max_transition_octaves_per_s * 12.0 * cfg.hop_s)
        .round()
        .max(1.0);
    let hw = (semis * bins_per_octave / 12.0).round() as usize;
    // Triangular local transitions, each row normalised over the bins that exist.
    let log_w: Vec<f64> = (0..=hw).map(|d| ((hw + 1 - d) as f64).ln()).collect();
    let log_z: Vec<f64> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(hw);
            let hi = (i + hw).min(n - 1);
            (lo..=hi)
                .map(|j| (hw + 1 - i.abs_diff(j)) as f64)
                .sum::<f64>()
                .ln()
        })
        .collect();
    let p = cfg.voicing_switch_prob;
    let (ls, lw) = ((1.0 - p).ln(), p.ln());
    let floor = 1e-30f64;

    let mut obs_v = vec![0.0f64; n];
    let mut voiced_prob = vec![0.0f64; frames];
    let mut dv = vec![0.0f64; n];
    let mut du = vec![0.0f64; n];
    let mut mv = vec![0.0f64; n];
    let mut mu = vec![0.0f64; n];
    let mut mv_from = vec![0u16; n];
    let mut mu_from = vec![0u16; n];
    let mut back = vec![0u16; frames * 2 * n];
    let init = -((2 * n) as f64).ln();

    for t in 0..frames {
        obs_v.iter_mut().for_each(|v| *v = 0.0);
        let mut vp = 0.0;
        for &(f, pr) in &cands[t] {
            if f >= cfg.fmin_hz * 0.97 && f <= cfg.fmax_hz * 1.03 {
                obs_v[bin_of(f)] += pr;
                vp += pr;
            }
        }
        let vp = vp.clamp(0.0, 1.0 - 1e-9);
        voiced_prob[t] = vp;
        let lu = ((1.0 - vp) / n as f64).ln();
        if t == 0 {
            for j in 0..n {
                dv[j] = init + obs_v[j].max(floor).ln();
                du[j] = init + lu;
            }
            continue;
        }
        // Best predecessor class for each source bin, for each target class.
        for i in 0..n {
            let (a, b) = (dv[i] + ls, du[i] + lw);
            if a >= b {
                mv[i] = a;
                mv_from[i] = i as u16;
            } else {
                mv[i] = b;
                mv_from[i] = (n + i) as u16;
            }
            let (a, b) = (du[i] + ls, dv[i] + lw);
            if a >= b {
                mu[i] = a;
                mu_from[i] = (n + i) as u16;
            } else {
                mu[i] = b;
                mu_from[i] = i as u16;
            }
        }
        let row = &mut back[t * 2 * n..(t + 1) * 2 * n];
        for j in 0..n {
            let lo = j.saturating_sub(hw);
            let hi = (j + hw).min(n - 1);
            let (mut best_v, mut arg_v) = (f64::NEG_INFINITY, 0u16);
            let (mut best_u, mut arg_u) = (f64::NEG_INFINITY, 0u16);
            for i in lo..=hi {
                let tr = log_w[i.abs_diff(j)] - log_z[i];
                let cv = mv[i] + tr;
                if cv > best_v {
                    best_v = cv;
                    arg_v = mv_from[i];
                }
                let cu = mu[i] + tr;
                if cu > best_u {
                    best_u = cu;
                    arg_u = mu_from[i];
                }
            }
            dv[j] = best_v + obs_v[j].max(floor).ln();
            du[j] = best_u + lu;
            row[j] = arg_v;
            row[n + j] = arg_u;
        }
    }

    let mut f0 = vec![0.0f32; frames];
    let mut conf = vec![0.0f32; frames];
    if frames == 0 {
        return (f0, conf);
    }
    // Best final state, then back-track.
    let mut state = 0usize;
    let mut best = f64::NEG_INFINITY;
    for j in 0..n {
        if dv[j] > best {
            best = dv[j];
            state = j;
        }
        if du[j] > best {
            best = du[j];
            state = n + j;
        }
    }
    for t in (0..frames).rev() {
        conf[t] = voiced_prob[t] as f32;
        if state < n {
            let fb = hz_of(state);
            // The exact candidate nearest to the decoded bin (within 1.5 bins).
            let mut pick = fb;
            let mut dist = 1.5 / bins_per_octave;
            for &(f, _) in &cands[t] {
                let d = (f / fb).log2().abs();
                if d <= dist {
                    dist = d;
                    pick = f;
                }
            }
            f0[t] = pick as f32;
        }
        if t > 0 {
            state = back[t * 2 * n + state] as usize;
        }
    }
    (f0, conf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_prior_sums_to_one() {
        let (_, p) = threshold_prior();
        let s: f64 = p.iter().sum();
        assert!((s - 1.0).abs() < 1e-9, "{s}");
        // Mean 0.1 for Beta(2, 18).
        let mean: f64 = p
            .iter()
            .enumerate()
            .map(|(i, v)| v * (i as f64 + 0.5) / 100.0)
            .sum();
        assert!((mean - 0.1).abs() < 0.01, "{mean}");
    }
}

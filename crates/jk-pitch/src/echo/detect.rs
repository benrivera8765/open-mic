// SPDX-License-Identifier: GPL-3.0-or-later
//! Detection of the app's own playback in the input: is it there, how late, how loud, and
//! is its delay steady (see the [`super`] module documentation for the method).
//!
//! Everything here runs at a decimated rate of about 8 kHz (`q` = rate / 8000, rounded):
//! the music that matters for detection lies below 3.4 kHz, and the decimation makes a
//! 0.6 s lag search cheap. Both signals go through the same 8th-order Butterworth low-pass
//! before decimation, so nothing between them shifts.
//!
//! Three things keep the decision honest on music and singing, both of which are made of
//! steady tones:
//!
//! - The input frame carries a Hann window and the reference segment tapered ends. Without
//!   them the spectral leakage of steady tones in both signals is itself coherent and puts a
//!   false peak at the offset between the two windows (measured: z 18.7 with only a singer in
//!   the input).
//! - The coherence normaliser (SCOT) is regularised with 30 dB below the mean band power, so
//!   bins where either signal has next to nothing do not get full weight.
//! - "Present" needs the correlation peak to stay at the same lag (within 3 ms) for about
//!   a second, not only to be tall: a singer holding a note in tune with the music can make a
//!   tall peak for a moment, but not a steady one across notes.

use std::f64::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::Presence;

/// Target decimated rate, Hz.
const TARGET_FD: f64 = 8_000.0;
/// Lowest frequency used by the correlation, Hz (below: hum, thumps, the mixer's high-pass).
const BAND_LO_HZ: f64 = 80.0;
/// Top of the band used, as a share of the decimated rate (below the anti-alias corner).
const BAND_HI: f64 = 0.42;
/// Averaging time of the fast correlation, seconds: a delay that wanders (Bluetooth) by a
/// millisecond or two a second smears the longer averages (measured: z 6-14 for 27 s on
/// one song), not this one.
const TAU_FAST_S: f64 = 0.3;
/// Averaging time of the short correlation, seconds.
const TAU_SHORT_S: f64 = 1.0;
/// Averaging time of the long (sensitive) correlation and of the level estimate, seconds.
const TAU_LONG_S: f64 = 4.0;
/// Delay history used for the drift fit, seconds.
const FIT_WINDOW_S: f64 = 20.0;
/// Delay history entries kept (a hop is about 0.128 s, so this covers more than the fit).
const HISTORY: usize = 256;
/// Hops (about 0.128 s each) over which the correlation peak must stay put before
/// "present", and how many of them must have a confident peak.
const STAB_HOPS: usize = 8;
const STAB_MIN: usize = 5;
/// How far the peak may move over those hops and still count as steady, seconds. Wide
/// enough for a Bluetooth delay that wanders by a few milliseconds a second (0.4 ms delayed
/// the warning by 25 s on one song), narrow enough to refuse a peak that jumps from note to
/// note.
const STAB_TOL_S: f64 = 0.003;
/// Regularisation of the coherence normaliser, relative to the mean band power (-30 dB).
const SCOT_REG: f32 = 1e-3;
/// Tapered length at each end of the reference segment, seconds.
const TAPER_S: f64 = 0.032;

/// One second-order section (direct form I, f64 state).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Low-pass section (RBJ cookbook, bilinear transform) at `fc` Hz with quality `q`.
    pub(crate) fn lowpass(fs: f64, fc: f64, q: f64) -> Self {
        let w = 2.0 * PI * fc / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - c) / 2.0 / a0,
            b1: (1.0 - c) / a0,
            b2: (1.0 - c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    #[inline]
    pub(crate) fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Flush denormals: a long silence must not slow the filter down.
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        y
    }

    pub(crate) fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Quality factors of an 8th-order Butterworth low-pass as four sections.
const BUTTER8_Q: [f64; 4] = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];

/// Straight-line fit of the measured delay over the last [`FIT_WINDOW_S`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fit {
    /// Change of delay per second of input (s/s): the clock drift (1e-6 = 1 ppm).
    pub slope: f64,
    /// The fitted delay at the newest hop, seconds.
    pub delay_now_s: f64,
    /// RMS distance of the measurements from the line, seconds.
    pub rms_s: f64,
    /// Largest distance of a measurement from the line, seconds.
    pub max_s: f64,
    /// Time between the oldest and the newest measurement used, seconds.
    pub span_s: f64,
    /// Measurements used.
    pub n: usize,
}

/// What the detector knows after its latest hop.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DetectState {
    pub presence: Presence,
    /// Peak-to-floor ratio of the fast (about 0.3 s), short (about 1 s) and long (about 4 s)
    /// correlations.
    pub z_fast: f32,
    pub z_short: f32,
    pub z_long: f32,
    /// Latest fine delay measurement (seconds), when the short correlation was confident.
    pub delay_s: Option<f64>,
    /// Fit over the recent delay measurements.
    pub fit: Option<Fit>,
    /// Echo power relative to the reference power (in the analysis band), linear.
    pub level: Option<f64>,
    /// Share of the input power (in band) that is echo, linear 0..1.
    pub echo_share: Option<f64>,
    /// Mean square of the full-band mono reference over the long averaging time.
    pub ref_ms: f64,
    /// Mean square of the full-band input over the long averaging time.
    pub in_ms: f64,
    /// Seconds of reference with music in it analysed so far.
    pub music_s: f64,
    /// Input time (seconds since the first sample) at the centre of the latest analysis frame.
    pub time_s: f64,
}

/// Thresholds of the decision (see `EchoConfig`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Thresholds {
    pub z_on_fast: f32,
    pub z_on: f32,
    pub z_on_long: f32,
    pub z_off: f32,
    pub z_track: f32,
    pub absent_after_s: f64,
    pub hold_s: f64,
    /// Linear power (decimated, band-limited reference) below which the music counts as silent.
    pub music_floor: f64,
}

/// The detector. See the module documentation.
pub(crate) struct Detector {
    q: usize,
    fd: f64,
    aa_x: [Biquad; 4],
    aa_r: [Biquad; 4],
    dec_phase: usize,
    ring_x: Vec<f32>,
    ring_r: Vec<f32>,
    mask: usize,
    /// Decimated samples written.
    count: u64,
    /// Frame length, longest lag, FFT length, spectrum bins, hop (decimated samples).
    lx: usize,
    lmax: usize,
    n: usize,
    nb: usize,
    hop: usize,
    since_hop: usize,
    /// Correlation band, FFT bins `k_lo..k_hi`.
    k_lo: usize,
    k_hi: usize,
    /// Averaging weights per hop.
    a_f: f32,
    a_s: f32,
    a_l: f32,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    scratch: Vec<Complex<f32>>,
    xbuf: Vec<f32>,
    rbuf: Vec<f32>,
    abuf: Vec<f32>,
    corr: Vec<f32>,
    xs: Vec<Complex<f32>>,
    rs: Vec<Complex<f32>>,
    ras: Vec<Complex<f32>>,
    gspec: Vec<Complex<f32>>,
    // Cross- and auto-spectra of the lag search, fast, short and long averages.
    s_f: Vec<Complex<f32>>,
    pxx_f: Vec<f32>,
    prr_f: Vec<f32>,
    s_s: Vec<Complex<f32>>,
    pxx_s: Vec<f32>,
    prr_s: Vec<f32>,
    s_l: Vec<Complex<f32>>,
    pxx_l: Vec<f32>,
    prr_l: Vec<f32>,
    // Spectra with the reference aligned at the integer lag `lag`.
    sa_s: Vec<Complex<f32>>,
    pxa_s: Vec<f32>,
    pra_s: Vec<f32>,
    sa_l: Vec<Complex<f32>>,
    pxa_l: Vec<f32>,
    pra_l: Vec<f32>,
    /// Hann window of the input frame (also used on the aligned reference frame) and the
    /// tapered-end window of the reference segment.
    win_x: Vec<f32>,
    win_r: Vec<f32>,
    /// Recent peak lags (fractional, decimated samples; NaN when that hop's peak was not
    /// confident) of the short and long correlations, for the steadiness test.
    lags_f: [f64; STAB_HOPS],
    lags_s: [f64; STAB_HOPS],
    lags_l: [f64; STAB_HOPS],
    lag_pos: usize,
    lag: Option<usize>,
    aligned_hops: u32,
    music_hops: u32,
    low_hops: u32,
    // Full-band powers, accumulated between hops, then averaged over the long time.
    acc_r: f64,
    acc_x: f64,
    acc_n: u64,
    hist_t: [f64; HISTORY],
    hist_d: [f64; HISTORY],
    hist_len: usize,
    hist_pos: usize,
    th: Thresholds,
    state: DetectState,
}

impl Detector {
    /// A detector for input and reference at `rate` Hz searching delays up to
    /// `max_delay_s`.
    pub(crate) fn new(rate: u32, max_delay_s: f64, th: Thresholds) -> Self {
        let fs = f64::from(rate);
        let q = ((fs / TARGET_FD).round() as usize).max(1);
        let fd = fs / q as f64;
        let lx = 1usize << ((0.25 * fd).log2().round() as u32);
        let lmax = (max_delay_s * fd).ceil() as usize + 2;
        let n = (lx + lmax + 1).next_power_of_two();
        let nb = n / 2 + 1;
        let ring = (lx + lmax + lx).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let aa = if q > 1 {
            let fc = 0.45 * fd;
            [
                Biquad::lowpass(fs, fc, BUTTER8_Q[0]),
                Biquad::lowpass(fs, fc, BUTTER8_Q[1]),
                Biquad::lowpass(fs, fc, BUTTER8_Q[2]),
                Biquad::lowpass(fs, fc, BUTTER8_Q[3]),
            ]
        } else {
            [Biquad::default(); 4]
        };
        let hop = lx / 2;
        let hop_s = hop as f64 / fd;
        let k_lo = ((BAND_LO_HZ * n as f64 / fd).ceil() as usize).max(1);
        let k_hi = ((BAND_HI * n as f64) as usize).min(nb - 2);
        let zc = Complex::new(0.0f32, 0.0);
        let win_x: Vec<f32> = (0..lx)
            .map(|j| (0.5 - 0.5 * (2.0 * PI * (j as f64 + 0.5) / lx as f64).cos()) as f32)
            .collect();
        let taper = ((TAPER_S * fd) as usize).max(1);
        let seg = lx + lmax;
        let win_r: Vec<f32> = (0..seg)
            .map(|j| {
                let e = j.min(seg - 1 - j);
                if e >= taper {
                    1.0
                } else {
                    (0.5 - 0.5 * (PI * (e as f64 + 0.5) / taper as f64).cos()) as f32
                }
            })
            .collect();
        Self {
            win_x,
            win_r,
            lags_f: [f64::NAN; STAB_HOPS],
            lags_s: [f64::NAN; STAB_HOPS],
            lags_l: [f64::NAN; STAB_HOPS],
            lag_pos: 0,
            q,
            fd,
            aa_x: aa,
            aa_r: aa,
            dec_phase: 0,
            ring_x: vec![0.0; ring],
            ring_r: vec![0.0; ring],
            mask: ring - 1,
            count: 0,
            lx,
            lmax,
            n,
            nb,
            hop,
            since_hop: 0,
            k_lo,
            k_hi,
            a_f: (1.0 - (-hop_s / TAU_FAST_S).exp()) as f32,
            a_s: (1.0 - (-hop_s / TAU_SHORT_S).exp()) as f32,
            a_l: (1.0 - (-hop_s / TAU_LONG_S).exp()) as f32,
            r2c,
            c2r,
            scratch: vec![zc; scratch_len],
            xbuf: vec![0.0; n],
            rbuf: vec![0.0; n],
            abuf: vec![0.0; n],
            corr: vec![0.0; n],
            xs: vec![zc; nb],
            rs: vec![zc; nb],
            ras: vec![zc; nb],
            gspec: vec![zc; nb],
            s_f: vec![zc; nb],
            pxx_f: vec![0.0; nb],
            prr_f: vec![0.0; nb],
            s_s: vec![zc; nb],
            pxx_s: vec![0.0; nb],
            prr_s: vec![0.0; nb],
            s_l: vec![zc; nb],
            pxx_l: vec![0.0; nb],
            prr_l: vec![0.0; nb],
            sa_s: vec![zc; nb],
            pxa_s: vec![0.0; nb],
            pra_s: vec![0.0; nb],
            sa_l: vec![zc; nb],
            pxa_l: vec![0.0; nb],
            pra_l: vec![0.0; nb],
            lag: None,
            aligned_hops: 0,
            music_hops: 0,
            low_hops: 0,
            acc_r: 0.0,
            acc_x: 0.0,
            acc_n: 0,
            hist_t: [0.0; HISTORY],
            hist_d: [0.0; HISTORY],
            hist_len: 0,
            hist_pos: 0,
            th,
            state: DetectState {
                presence: Presence::Searching,
                z_fast: 0.0,
                z_short: 0.0,
                z_long: 0.0,
                delay_s: None,
                fit: None,
                level: None,
                echo_share: None,
                ref_ms: 0.0,
                in_ms: 0.0,
                music_s: 0.0,
                time_s: 0.0,
            },
        }
    }

    /// Forget everything (a new device, a broken stream). Does not allocate.
    pub(crate) fn reset(&mut self) {
        for f in self.aa_x.iter_mut().chain(self.aa_r.iter_mut()) {
            f.clear();
        }
        self.dec_phase = 0;
        self.ring_x.fill(0.0);
        self.ring_r.fill(0.0);
        self.count = 0;
        self.since_hop = 0;
        let zc = Complex::new(0.0f32, 0.0);
        for v in [
            &mut self.s_f,
            &mut self.s_s,
            &mut self.s_l,
            &mut self.sa_s,
            &mut self.sa_l,
        ] {
            v.fill(zc);
        }
        for v in [
            &mut self.pxx_f,
            &mut self.prr_f,
            &mut self.pxx_s,
            &mut self.prr_s,
            &mut self.pxx_l,
            &mut self.prr_l,
            &mut self.pxa_s,
            &mut self.pra_s,
            &mut self.pxa_l,
            &mut self.pra_l,
        ] {
            v.fill(0.0);
        }
        self.lag = None;
        self.lags_f = [f64::NAN; STAB_HOPS];
        self.lags_s = [f64::NAN; STAB_HOPS];
        self.lags_l = [f64::NAN; STAB_HOPS];
        self.lag_pos = 0;
        self.aligned_hops = 0;
        self.music_hops = 0;
        self.low_hops = 0;
        self.acc_r = 0.0;
        self.acc_x = 0.0;
        self.acc_n = 0;
        self.hist_len = 0;
        self.hist_pos = 0;
        self.state = DetectState {
            presence: Presence::Searching,
            z_fast: 0.0,
            z_short: 0.0,
            z_long: 0.0,
            delay_s: None,
            fit: None,
            level: None,
            echo_share: None,
            ref_ms: 0.0,
            in_ms: 0.0,
            music_s: 0.0,
            time_s: 0.0,
        };
    }

    /// The latest results.
    pub(crate) fn state(&self) -> &DetectState {
        &self.state
    }

    /// Seconds per hop.
    pub(crate) fn hop_s(&self) -> f64 {
        self.hop as f64 / self.fd
    }

    /// Feed one input sample and the matching mono reference sample. Returns true when an
    /// analysis hop finished (the state may have changed).
    #[inline]
    pub(crate) fn push(&mut self, x: f32, r: f32) -> bool {
        let (x, r) = (f64::from(x), f64::from(r));
        self.acc_x += x * x;
        self.acc_r += r * r;
        self.acc_n += 1;
        let (xd, rd) = if self.q > 1 {
            let mut xf = x;
            let mut rf = r;
            for s in &mut self.aa_x {
                xf = s.tick(xf);
            }
            for s in &mut self.aa_r {
                rf = s.tick(rf);
            }
            self.dec_phase += 1;
            if self.dec_phase < self.q {
                return false;
            }
            self.dec_phase = 0;
            (xf, rf)
        } else {
            (x, r)
        };
        let slot = (self.count as usize) & self.mask;
        self.ring_x[slot] = xd as f32;
        self.ring_r[slot] = rd as f32;
        self.count += 1;
        self.since_hop += 1;
        if self.since_hop >= self.hop && self.count >= self.lx as u64 {
            self.since_hop = 0;
            self.analyse();
            return true;
        }
        false
    }

    /// Decimated sample `i` of a ring (0 before the stream started).
    #[inline]
    fn ring_at(ring: &[f32], mask: usize, count: u64, i: i64) -> f32 {
        if i < 0 || i as u64 >= count || count - i as u64 > ring.len() as u64 {
            0.0
        } else {
            ring[i as usize & mask]
        }
    }

    /// One analysis hop.
    fn analyse(&mut self) {
        let (lx, lmax) = (self.lx, self.lmax);
        let end = self.count as i64;
        let seg0 = end - (lx + lmax) as i64;
        // Reference segment covering every lag, and the input frame at the matching place.
        self.rbuf.fill(0.0);
        self.xbuf.fill(0.0);
        let mut ref_pow = 0.0f64;
        for j in 0..lx + lmax {
            let v = Self::ring_at(&self.ring_r, self.mask, self.count, seg0 + j as i64);
            self.rbuf[j] = v * self.win_r[j];
            ref_pow += f64::from(v) * f64::from(v);
        }
        ref_pow /= (lx + lmax) as f64;
        for j in 0..lx {
            self.xbuf[lmax + j] = Self::ring_at(
                &self.ring_x,
                self.mask,
                self.count,
                end - lx as i64 + j as i64,
            ) * self.win_x[j];
        }
        self.state.time_s = (end as f64 - lx as f64 / 2.0) / self.fd;

        // Full-band powers since the last hop, averaged over the long time constant.
        if self.acc_n > 0 {
            let a = f64::from(self.a_l);
            let mr = self.acc_r / self.acc_n as f64;
            let mx = self.acc_x / self.acc_n as f64;
            if self.music_hops == 0 {
                self.state.ref_ms = mr;
                self.state.in_ms = mx;
            } else {
                self.state.ref_ms += a * (mr - self.state.ref_ms);
                self.state.in_ms += a * (mx - self.state.in_ms);
            }
        }
        self.acc_r = 0.0;
        self.acc_x = 0.0;
        self.acc_n = 0;

        if ref_pow <= self.th.music_floor {
            // No music: no evidence either way. Keep everything as it is.
            return;
        }
        self.music_hops += 1;
        self.state.music_s += self.hop_s();

        let _ = self
            .r2c
            .process_with_scratch(&mut self.xbuf, &mut self.xs, &mut self.scratch);
        let _ = self
            .r2c
            .process_with_scratch(&mut self.rbuf, &mut self.rs, &mut self.scratch);
        // The transforms destroyed their inputs: the reference segment is read again from
        // the ring below when the aligned frame is built.
        let (a_f, a_s, a_l) = (self.a_f, self.a_s, self.a_l);
        for k in 0..self.nb {
            let x = self.xs[k];
            let r = self.rs[k];
            let c = x * r.conj();
            let px = x.norm_sqr();
            let pr = r.norm_sqr();
            let o = self.s_f[k];
            self.s_f[k] = o + (c - o) * a_f;
            self.pxx_f[k] += a_f * (px - self.pxx_f[k]);
            self.prr_f[k] += a_f * (pr - self.prr_f[k]);
            let o = self.s_s[k];
            self.s_s[k] = o + (c - o) * a_s;
            self.pxx_s[k] += a_s * (px - self.pxx_s[k]);
            self.prr_s[k] += a_s * (pr - self.prr_s[k]);
            let o = self.s_l[k];
            self.s_l[k] = o + (c - o) * a_l;
            self.pxx_l[k] += a_l * (px - self.pxx_l[k]);
            self.prr_l[k] += a_l * (pr - self.prr_l[k]);
        }
        let (_, f_f, z_f) = self.gcc(0);
        let (m_s, f_s, z_s) = self.gcc(1);
        let (m_l, f_l, z_l) = self.gcc(2);
        self.state.z_fast = z_f;
        self.state.z_short = z_s;
        self.state.z_long = z_l;
        let conf = |z: f32, f: f64| if z >= self.th.z_track { f } else { f64::NAN };
        self.lags_f[self.lag_pos] = conf(z_f, f_f);
        self.lags_s[self.lag_pos] = conf(z_s, f_s);
        self.lags_l[self.lag_pos] = conf(z_l, f_l);
        self.lag_pos = (self.lag_pos + 1) % STAB_HOPS;

        // Integer lag for the aligned spectra: the confident peak, short first (it follows a
        // jump within about a second), else the long one.
        let lag = if z_s >= self.th.z_track {
            Some(m_s)
        } else if z_l >= self.th.z_track {
            Some(m_l)
        } else {
            self.lag
        };
        self.state.delay_s = None;
        if let Some(a) = lag {
            self.aligned_update(a);
            if z_s >= self.th.z_track {
                // The delay at this hop: the short correlation's peak, refined between lags.
                let d_s = f_s / self.fd;
                self.state.delay_s = Some(d_s);
                self.hist_t[self.hist_pos] = self.state.time_s;
                self.hist_d[self.hist_pos] = d_s;
                self.hist_pos = (self.hist_pos + 1) % HISTORY;
                self.hist_len = (self.hist_len + 1).min(HISTORY);
            }
            self.level();
        }
        self.state.fit = self.fit();
        self.decide();
    }

    /// Smoothed-coherence-weighted cross-correlation (GCC-SCOT, regularised) of the fast (0),
    /// short (1) or long (2) averages; returns the lag of the largest magnitude, that lag
    /// refined between samples (parabola through the peak and its neighbours), and the
    /// peak-to-floor ratio.
    fn gcc(&mut self, which: u8) -> (usize, f64, f32) {
        let (s, pxx, prr) = match which {
            0 => (&self.s_f, &self.pxx_f, &self.prr_f),
            1 => (&self.s_s, &self.pxx_s, &self.prr_s),
            _ => (&self.s_l, &self.pxx_l, &self.prr_l),
        };
        let zc = Complex::new(0.0f32, 0.0);
        self.gspec.fill(zc);
        let band = (self.k_hi - self.k_lo).max(1) as f32;
        let ex = SCOT_REG * pxx[self.k_lo..self.k_hi].iter().sum::<f32>() / band;
        let er = SCOT_REG * prr[self.k_lo..self.k_hi].iter().sum::<f32>() / band;
        for k in self.k_lo..self.k_hi {
            let d = ((pxx[k] + ex) * (prr[k] + er)).sqrt();
            if d > 1e-30 {
                self.gspec[k] = s[k] / d;
            }
        }
        let _ = self
            .c2r
            .process_with_scratch(&mut self.gspec, &mut self.corr, &mut self.scratch);
        let lags = &self.corr[..=self.lmax];
        let mut best = 0usize;
        let mut peak = 0.0f32;
        for (m, &v) in lags.iter().enumerate() {
            if v.abs() > peak {
                peak = v.abs();
                best = m;
            }
        }
        // Floor: RMS of the correlation away from the peak (+-4 ms).
        let excl = ((0.004 * self.fd) as usize).max(8);
        let mut sum = 0.0f64;
        let mut cnt = 0usize;
        for (m, &v) in lags.iter().enumerate() {
            if m + excl < best || m > best + excl {
                sum += f64::from(v) * f64::from(v);
                cnt += 1;
            }
        }
        let floor = if cnt > 0 {
            (sum / cnt as f64).sqrt() as f32
        } else {
            0.0
        };
        let z = if floor > 1e-30 { peak / floor } else { 0.0 };
        let mut frac = best as f64;
        if best > 0 && best < self.lmax {
            let (ym, y0, yp) = (
                f64::from(lags[best - 1].abs()),
                f64::from(peak),
                f64::from(lags[best + 1].abs()),
            );
            let den = ym - 2.0 * y0 + yp;
            if den < -1e-30 {
                frac += (0.5 * (ym - yp) / den).clamp(-0.5, 0.5);
            }
        }
        (best, frac, z)
    }

    /// Update the spectra with the reference aligned at integer lag `a` (decimated samples).
    fn aligned_update(&mut self, a: usize) {
        let (lx, lmax, n) = (self.lx, self.lmax, self.n);
        if let Some(prev) = self.lag
            && prev != a
        {
            // Re-align the accumulated cross-spectra to the new lag: a delay of one sample
            // more in the reference turns the cross-spectrum by e^{+j 2 pi k / n}.
            let d = a as f64 - prev as f64;
            for k in 0..self.nb {
                let ph = 2.0 * PI * k as f64 * d / n as f64;
                let rot = Complex::new(ph.cos() as f32, ph.sin() as f32);
                self.sa_s[k] *= rot;
                self.sa_l[k] *= rot;
            }
        }
        self.lag = Some(a);
        let end = self.count as i64;
        self.abuf.fill(0.0);
        let first = end - lx as i64 - a as i64;
        for j in 0..lx {
            self.abuf[lmax + j] =
                Self::ring_at(&self.ring_r, self.mask, self.count, first + j as i64)
                    * self.win_x[j];
        }
        let _ = self
            .r2c
            .process_with_scratch(&mut self.abuf, &mut self.ras, &mut self.scratch);
        let (a_s, a_l) = (self.a_s, self.a_l);
        for k in 0..self.nb {
            let x = self.xs[k];
            let r = self.ras[k];
            let c = x * r.conj();
            let px = x.norm_sqr();
            let pr = r.norm_sqr();
            let o = self.sa_s[k];
            self.sa_s[k] = o + (c - o) * a_s;
            self.pxa_s[k] += a_s * (px - self.pxa_s[k]);
            self.pra_s[k] += a_s * (pr - self.pra_s[k]);
            let o = self.sa_l[k];
            self.sa_l[k] = o + (c - o) * a_l;
            self.pxa_l[k] += a_l * (px - self.pxa_l[k]);
            self.pra_l[k] += a_l * (pr - self.pra_l[k]);
        }
        self.aligned_hops += 1;
    }

    /// Echo level from the long aligned spectra: the power of the part of the input that the
    /// reference predicts (|S_xr|^2 / S_rr per bin, minus the bias that finite averaging
    /// leaves from the rest of the input), relative to the reference power.
    fn level(&mut self) {
        if self.aligned_hops < 2 {
            return;
        }
        let f = (f64::from(self.aligned_hops).min(1.0 / f64::from(self.a_l)) * 0.75).max(1.2);
        let mut echo = 0.0f64;
        let mut refp = 0.0f64;
        let mut inp = 0.0f64;
        for k in self.k_lo..self.k_hi {
            let s2 = f64::from(self.sa_l[k].norm_sqr());
            let px = f64::from(self.pxa_l[k]);
            let pr = f64::from(self.pra_l[k]);
            if pr > 1e-30 {
                echo += ((s2 - px * pr / f) / (1.0 - 1.0 / f)).max(0.0) / pr;
            }
            refp += pr;
            inp += px;
        }
        if refp > 1e-30 {
            self.state.level = Some(echo / refp);
        }
        if inp > 1e-30 {
            self.state.echo_share = Some((echo / inp).min(1.0));
        }
    }

    /// Straight line through the recent delay measurements (see [`fit_history`]).
    fn fit(&self) -> Option<Fit> {
        fit_history(
            &self.hist_t[..self.hist_len],
            &self.hist_d[..self.hist_len],
            self.state.time_s,
            FIT_WINDOW_S,
        )
    }

    /// Whether at least [`STAB_MIN`] of the last [`STAB_HOPS`] peak lags were confident and
    /// those lay within [`STAB_TOL_S`] of each other. Hops where the music correlates too
    /// little to say (sparse music, a weak return under a codec) do not count against it.
    fn steady(&self, lags: &[f64; STAB_HOPS]) -> bool {
        let (mut lo, mut hi, mut n) = (f64::MAX, f64::MIN, 0usize);
        for &l in lags.iter().filter(|l| !l.is_nan()) {
            lo = lo.min(l);
            hi = hi.max(l);
            n += 1;
        }
        n >= STAB_MIN && hi - lo <= STAB_TOL_S * self.fd
    }

    /// Present / absent decision with hysteresis.
    fn decide(&mut self) {
        let th = self.th;
        let (z_s, z_l) = (self.state.z_short, self.state.z_long);
        let z_f = self.state.z_fast;
        let found = (self.music_hops >= 3 && z_s >= th.z_on && self.steady(&self.lags_s))
            || (self.music_hops >= 8 && z_l >= th.z_on_long && self.steady(&self.lags_l))
            || (self.music_hops >= 3 && z_f >= th.z_on_fast && self.steady(&self.lags_f));
        match self.state.presence {
            Presence::Searching | Presence::Absent => {
                if found {
                    self.state.presence = Presence::Present;
                    self.low_hops = 0;
                } else if self.state.presence == Presence::Searching
                    && self.state.music_s >= th.absent_after_s
                    && z_l < th.z_on_long
                    && z_s < th.z_on
                    && z_f < th.z_on_fast
                {
                    self.state.presence = Presence::Absent;
                }
            }
            Presence::Present => {
                if z_s < th.z_off && z_l < th.z_on_long && z_f < th.z_on_fast {
                    self.low_hops += 1;
                } else {
                    self.low_hops = 0;
                }
                if f64::from(self.low_hops) * self.hop_s() >= th.hold_s {
                    self.state.presence = Presence::Absent;
                    self.low_hops = 0;
                    self.hist_len = 0;
                    self.hist_pos = 0;
                    self.state.fit = None;
                }
            }
        }
    }
}

/// Least-squares line through delay measurements `d` (seconds) taken at times `t`
/// (seconds), using those no older than `window` before `now`, with one pass of outlier
/// trimming (points further than 3 x RMS, at least 0.05 ms, from the first line). The spread
/// reported is that of *all* the recent measurements around the trimmed line, so a jump
/// (Bluetooth) shows instead of being trimmed away. Order of the entries does not matter.
pub(crate) fn fit_history(t: &[f64], d: &[f64], now: f64, window: f64) -> Option<Fit> {
    let line = |trim: Option<(f64, f64, f64)>| -> Option<(f64, f64, usize, f64, f64)> {
        let (mut n, mut st, mut sd, mut stt, mut std) = (0usize, 0.0, 0.0, 0.0, 0.0);
        let mut tmin = f64::MAX;
        let mut tmax = f64::MIN;
        for (&ti, &di) in t.iter().zip(d) {
            let ti = ti - now;
            if ti < -window {
                continue;
            }
            if let Some((b, c, lim)) = trim
                && (di - (c + b * ti)).abs() > lim
            {
                continue;
            }
            n += 1;
            st += ti;
            sd += di;
            stt += ti * ti;
            std += ti * di;
            tmin = tmin.min(ti);
            tmax = tmax.max(ti);
        }
        if n < 2 {
            return None;
        }
        let nf = n as f64;
        let den = nf * stt - st * st;
        let b = if den.abs() > 1e-12 {
            (nf * std - st * sd) / den
        } else {
            0.0
        };
        let c = (sd - b * st) / nf;
        Some((b, c, n, tmin, tmax))
    };
    let resid = |b: f64, c: f64| {
        let (mut ss, mut mx, mut n) = (0.0f64, 0.0f64, 0usize);
        for (&ti, &di) in t.iter().zip(d) {
            let ti = ti - now;
            if ti < -window {
                continue;
            }
            let r = (di - (c + b * ti)).abs();
            ss += r * r;
            mx = mx.max(r);
            n += 1;
        }
        ((ss / n.max(1) as f64).sqrt(), mx)
    };
    let (b, c, _, _, _) = line(None)?;
    let (rms0, _) = resid(b, c);
    let lim = (3.0 * rms0).max(0.000_05);
    let (b, c, n, tmin, tmax) = line(Some((b, c, lim)))?;
    let (rms, mx) = resid(b, c);
    Some(Fit {
        slope: b,
        delay_now_s: c,
        rms_s: rms,
        max_s: mx,
        span_s: tmax - tmin,
        n,
    })
}

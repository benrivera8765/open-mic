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
/// After the echo was forgotten, hops (about 3 s) before the long average, whose threshold
/// is lower because it normally averages 4 s, may declare it present again. Measured: with
/// only a singer over the music, the long average refilled from zero reached z 13.5 after
/// about a second and declared a vanished return present again for 3 s. A return switched
/// back on is found by the short averages in under a second anyway.
const LONG_HOPS_AFTER_FORGET: u32 = 24;
/// A detection this strong (peak-to-floor ratio of the average that found it) is believed
/// at once. Returned music with nobody singing is found at 24 or more (USB and cable, -40
/// to 0 dB, 8 songs; docs/benchmarks/practice-echo.md section 13).
const Z_SURE: f32 = 24.0;
/// A weaker detection is a candidate until the short correlation has stood out at its lag
/// (within [`CAND_TRACK_S`], peak-to-floor at least [`Z_HOLD`]) in [`CAND_HOPS`] of the
/// last [`CAND_SPAN`] hops (about 3 s of 4); it is dropped after [`CAND_MAX`] hops without.
/// A singer holding a note in tune with a note of the music makes a peak that lasts about
/// as long as the note (measured: false "present" on 2 of 8 songs with only a singer, one
/// of them for 93 s of a 4-minute song); a returned song stays at its delay for good.
const Z_HOLD: f32 = 6.0;
const CAND_HOPS: u32 = 24;
const CAND_SPAN: u32 = 32;
const CAND_MAX: u32 = 48;
/// How far the candidate's lag may move from hop to hop and still be the same echo
/// (a Bluetooth delay wanders by a few milliseconds a second), seconds.
const CAND_TRACK_S: f64 = 0.003;
/// A detection within this of a delay already confirmed in this session is believed at
/// once (the same return found again after the loopback was switched back on, or after a
/// filter restart), seconds.
const CONFIRMED_TOL_S: f64 = 0.002;

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
    /// Measurements used (on the line).
    pub n: usize,
    /// Measurements in the window, on the line or not.
    pub total: usize,
    /// Of those off the line, the most that agree with each other (within
    /// [`CLUSTER_S`]): a second delay the echo jumped to, rather than scattered false peaks.
    pub cluster2: usize,
}

impl Fit {
    /// Whether the delay wanders: the measurements on the line spread by more than
    /// `wander_s` (RMS), or the delay jumped (a second group of measurements that agree with
    /// each other off the line, or fewer than half of them on it). Peaks that land
    /// anywhere off the line (a singer holding a note in tune with the music, under a weak
    /// return) neither move the line nor make it wander: measured, six fixed-delay USB
    /// returns 30-40 dB under a singer were called "wandering" when the spread of every
    /// measurement was used (docs/benchmarks/practice-echo.md section 13).
    pub fn wanders(&self, wander_s: f64) -> bool {
        self.rms_s > wander_s
            || (self.total >= 10 && (2 * self.n < self.total || (self.cluster2 >= 5 && 5 * self.cluster2 >= self.total)))
    }
}

/// Measurements off the line that agree within this (seconds) form a second delay.
const CLUSTER_S: f64 = 0.001;
/// The line is first fitted to the measurements within this (seconds) of the densest
/// group, so that up to half of them can be false peaks anywhere else.
const BAND_S: f64 = 0.0025;
/// Measurements kept per fit (the histories hold at most this many).
const FIT_MAX: usize = 256;

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
    /// Whether the fast or the short correlation stood out (peak-to-floor at least
    /// [`Z_HOLD`]) within 0.5 ms of the delay given to [`Detector::watch`] at the latest hop.
    pub watch_held: bool,
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
    /// After [`Detector::forget_echo`] with the echo kept "present": waiting for the fresh
    /// averages to find it again.
    probation: bool,
    /// Hops the long average needs before it may declare the echo present: 8 at the start
    /// (as measured in docs/benchmarks/practice-echo.md), [`LONG_HOPS_AFTER_FORGET`] after
    /// [`Detector::forget_echo`].
    long_min_hops: u32,
    /// A detection not yet believed (see [`Z_SURE`]): its lag (fractional, decimated
    /// samples), hops since it appeared, and which of the last [`CAND_SPAN`] hops saw the
    /// short correlation stand out there (a bit ring, newest in bit 0).
    cand: Option<f64>,
    cand_hops: u32,
    cand_bits: u64,
    /// The latest delay confirmed in this session (decimated samples).
    confirmed: Option<f64>,
    /// Floor of the latest correlation computed by `gcc`.
    last_floor: f32,
    /// A delay the canceller asked about (decimated samples, see [`Detector::watch`]).
    watch: Option<f64>,
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
            probation: false,
            long_min_hops: 8,
            cand: None,
            cand_hops: 0,
            cand_bits: 0,
            confirmed: None,
            last_floor: 0.0,
            watch: None,
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
                watch_held: false,
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
        self.probation = false;
        self.long_min_hops = 8;
        self.cand = None;
        self.cand_hops = 0;
        self.cand_bits = 0;
        self.confirmed = None;
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
            watch_held: false,
        };
        self.watch = None;
    }

    /// The canceller found the returned playback gone (the mixer's loopback switched off),
    /// or suspects it: forget everything learnt about it. With `keep_present` false say
    /// "absent" at once; with it true stay "present" on probation until the fresh averages
    /// find the echo again ([`Detector::probation`] turns false) or the canceller gives up.
    /// The correlation averages are scale-invariant (GCC-SCOT), so on their own they
    /// forget a vanished echo only as fast as the new input outweighs it: 12-25 s for a
    /// loud return over a quiet room (docs/benchmarks/practice-echo.md). The signal history
    /// is kept, so a return that is still there (or comes back) is found again as quickly
    /// as at the start (about a second). Does not allocate.
    pub(crate) fn forget_echo(&mut self, keep_present: bool) {
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
        // Averages restart: the first hops must count as the first again (no instant
        // "present" from three hops of a half-filled average).
        self.music_hops = 0;
        self.low_hops = 0;
        self.hist_len = 0;
        self.hist_pos = 0;
        self.probation = keep_present && self.state.presence == Presence::Present;
        self.long_min_hops = LONG_HOPS_AFTER_FORGET;
        self.cand = None;
        self.cand_hops = 0;
        self.cand_bits = 0;
        if !self.probation {
            self.state.presence = Presence::Absent;
        }
        self.state.z_fast = 0.0;
        self.state.z_short = 0.0;
        self.state.z_long = 0.0;
        if !self.probation {
            // On probation the last delay, its line and the level stay the best knowledge
            // (the canceller's gate keeps expecting that echo until it is confirmed gone).
            self.state.delay_s = None;
            self.state.fit = None;
            self.state.level = None;
            self.state.echo_share = None;
        }
    }

    /// Still on probation after [`Detector::forget_echo`] with `keep_present`.
    pub(crate) fn probation(&self) -> bool {
        self.probation
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
        // The fast correlation is in `corr`: a wandering delay (Bluetooth) smears the
        // short one, so the candidate counts as standing out in either.
        let fast_held = self.cand_stands_out();
        let watch_fast = self.watch_stands_out();
        let (m_s, f_s, z_s) = self.gcc(1);
        self.state.watch_held = watch_fast || self.watch_stands_out();
        // The short correlation is in `corr` now: does it stand out at the candidate's lag?
        self.probe_candidate(fast_held);
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
        self.last_floor = floor;
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

    /// During a candidate's probation, after the short correlation was computed into
    /// `corr`: whether it stands out near the candidate's lag this hop (and follow the lag
    /// to where it stands out most, for a delay that wanders).
    /// Ask, from the next hop on, whether the correlation still stands out at the delay
    /// `delay_s` (where the canceller has the echo): a peak elsewhere is then a false peak,
    /// not a jump of the delay (see [`DetectState::watch_held`]). `None` stops asking.
    pub(crate) fn watch(&mut self, delay_s: Option<f64>) {
        self.watch = delay_s.map(|d| d * self.fd);
    }

    /// Whether the correlation in `corr` stands out within 0.5 ms of the watched delay.
    fn watch_stands_out(&self) -> bool {
        let Some(c) = self.watch else { return false };
        let w = (0.0005 * self.fd).ceil() as usize;
        let ci = (c.round().max(0.0) as usize).min(self.lmax);
        let (lo, hi) = (ci.saturating_sub(w), (ci + w).min(self.lmax));
        let peak = self.corr[lo..=hi].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        self.last_floor > 1e-30 && peak >= Z_HOLD * self.last_floor
    }

    /// Whether the correlation in `corr` stands out near the candidate's lag.
    fn cand_stands_out(&self) -> bool {
        let Some(c) = self.cand else { return false };
        let w = (CAND_TRACK_S * self.fd).ceil() as usize;
        let ci = (c.round().max(0.0) as usize).min(self.lmax);
        let (lo, hi) = (ci.saturating_sub(w), (ci + w).min(self.lmax));
        let peak = self.corr[lo..=hi].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        self.last_floor > 1e-30 && peak >= Z_HOLD * self.last_floor
    }

    fn probe_candidate(&mut self, fast_held: bool) {
        let Some(c) = self.cand else { return };
        let w = (CAND_TRACK_S * self.fd).ceil() as usize;
        let ci = (c.round().max(0.0) as usize).min(self.lmax);
        let (lo, hi) = (ci.saturating_sub(w), (ci + w).min(self.lmax));
        let (mut best, mut peak) = (ci, 0.0f32);
        for (m, v) in self.corr.iter().enumerate().take(hi + 1).skip(lo) {
            if v.abs() > peak {
                peak = v.abs();
                best = m;
            }
        }
        let held = fast_held || (self.last_floor > 1e-30 && peak >= Z_HOLD * self.last_floor);
        self.cand_bits = (self.cand_bits << 1) | u64::from(held);
        self.cand_hops += 1;
        if held {
            self.cand = Some(best as f64);
        }
    }

    /// Whether a detection `hit` (peak-to-floor ratio, lag) is believed now: at once when
    /// it is strong or at a delay confirmed before, else after the candidate's probation.
    fn confirm(&mut self, hit: Option<(f32, f64)>) -> bool {
        let tol = CONFIRMED_TOL_S * self.fd;
        if let Some((z, lag)) = hit {
            if z >= Z_SURE || self.confirmed.is_some_and(|c| (c - lag).abs() <= tol) {
                return self.accept(lag);
            }
            let track = CAND_TRACK_S * self.fd;
            if self.cand.is_none_or(|c| (c - lag).abs() > track) && (self.cand_bits & 0xF) == 0 {
                // A new candidate (the old one, if any, has not stood out in the last four
                // hops: a Bluetooth delay that jumped, or a passing coincidence).
                self.cand = Some(lag);
                self.cand_hops = 0;
                self.cand_bits = 0;
            }
        }
        if let Some(c) = self.cand {
            if self.cand_held() >= CAND_HOPS {
                return self.accept(c);
            }
            if self.cand_hops >= CAND_MAX {
                self.cand = None;
                self.cand_hops = 0;
                self.cand_bits = 0;
            }
        }
        false
    }

    /// Diagnostics: the candidate's lag (ms) and the hops it stood out in, of the last
    /// [`CAND_SPAN`], and of all since it appeared.
    pub(crate) fn candidate(&self) -> Option<(f64, u32, u32)> {
        self.cand.map(|c| (c / self.fd * 1e3, self.cand_held(), self.cand_hops))
    }

    /// A candidate is on probation and stood out in at least one of the last eight hops
    /// (about a second): the playback may be coming back, so "absent" must wait, and the
    /// canceller treats the input as unknown rather than clean meanwhile. Measured without
    /// this on held-out songs: a Bluetooth return as loud as the music was declared absent
    /// 3 s into its probation and 12 % of the frames showed the returned music as pitch
    /// with nobody singing.
    pub(crate) fn pending(&self) -> bool {
        self.cand.is_some() && (self.cand_bits & 0xFF) != 0
    }

    /// Hops of the last [`CAND_SPAN`] in which the candidate stood out.
    fn cand_held(&self) -> u32 {
        (self.cand_bits & ((1u64 << CAND_SPAN) - 1)).count_ones()
    }

    fn accept(&mut self, lag: f64) -> bool {
        self.confirmed = Some(lag);
        self.cand = None;
        self.cand_hops = 0;
        self.cand_bits = 0;
        true
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
        let newest = (self.lag_pos + STAB_HOPS - 1) % STAB_HOPS;
        // Which average found it (its lag and peak-to-floor ratio), strongest first.
        let mut hit: Option<(f32, f64)> = None;
        let mut consider = |ok: bool, z: f32, lag: f64| {
            if ok && !lag.is_nan() && hit.is_none_or(|h| z > h.0) {
                hit = Some((z, lag));
            }
        };
        consider(
            self.music_hops >= 3 && z_s >= th.z_on && self.steady(&self.lags_s),
            z_s,
            self.lags_s[newest],
        );
        consider(
            self.music_hops >= self.long_min_hops
                && z_l >= th.z_on_long
                && self.steady(&self.lags_l),
            z_l,
            self.lags_l[newest],
        );
        consider(
            self.music_hops >= 3 && z_f >= th.z_on_fast && self.steady(&self.lags_f),
            z_f,
            self.lags_f[newest],
        );
        let found = self.confirm(hit);
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
                    && !self.pending()
                {
                    self.state.presence = Presence::Absent;
                }
            }
            Presence::Present => {
                if self.probation {
                    // Fresh averages: found again confirms the echo; the canceller decides
                    // when to give up.
                    if found {
                        self.probation = false;
                        self.low_hops = 0;
                    }
                    return;
                }
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
/// (seconds), using those no older than `window` before `now`, robust to false peaks:
/// the line is fitted first to the measurements within [`BAND_S`] of the densest group
/// (the measurement with the most others within 1 ms), then once more to those within
/// 3 x its RMS (at least 0.05 ms). The spread reported is that of the measurements on the
/// line; [`Fit::wanders`] also looks at those off it, so a jump (Bluetooth) shows while
/// scattered false peaks do not. Order of the entries does not matter. No allocation.
pub(crate) fn fit_history(t: &[f64], d: &[f64], now: f64, window: f64) -> Option<Fit> {
    let mut tv = [0.0f64; FIT_MAX];
    let mut dv = [0.0f64; FIT_MAX];
    let mut n = 0usize;
    for (&ti, &di) in t.iter().zip(d) {
        let ti = ti - now;
        if ti < -window || n >= FIT_MAX {
            continue;
        }
        tv[n] = ti;
        dv[n] = di;
        n += 1;
    }
    if n < 2 {
        return None;
    }
    let (tv, dv) = (&tv[..n], &dv[..n]);
    // The densest group: the measurement with the most others within 1 ms.
    let (mut center, mut best) = (dv[0], 0usize);
    for &a in dv {
        let c = dv.iter().filter(|&&b| (a - b).abs() <= CLUSTER_S).count();
        if c > best {
            best = c;
            center = a;
        }
    }
    let line = |keep: &dyn Fn(f64, f64) -> bool| -> Option<(f64, f64, usize, f64, f64)> {
        let (mut m, mut st, mut sd, mut stt, mut std) = (0usize, 0.0, 0.0, 0.0, 0.0);
        let (mut tmin, mut tmax) = (f64::MAX, f64::MIN);
        for (&ti, &di) in tv.iter().zip(dv) {
            if !keep(ti, di) {
                continue;
            }
            m += 1;
            st += ti;
            sd += di;
            stt += ti * ti;
            std += ti * di;
            tmin = tmin.min(ti);
            tmax = tmax.max(ti);
        }
        if m < 2 {
            return None;
        }
        let mf = m as f64;
        let den = mf * stt - st * st;
        let b = if den.abs() > 1e-12 { (mf * std - st * sd) / den } else { 0.0 };
        let c = (sd - b * st) / mf;
        Some((b, c, m, tmin, tmax))
    };
    let resid = |b: f64, c: f64, keep: &dyn Fn(f64, f64) -> bool| {
        let (mut ss, mut mx, mut m) = (0.0f64, 0.0f64, 0usize);
        for (&ti, &di) in tv.iter().zip(dv) {
            if !keep(ti, di) {
                continue;
            }
            let r = (di - (c + b * ti)).abs();
            ss += r * r;
            mx = mx.max(r);
            m += 1;
        }
        ((ss / m.max(1) as f64).sqrt(), mx)
    };
    let in_band = |_: f64, di: f64| (di - center).abs() <= BAND_S;
    let (b, c, _, _, _) = line(&in_band).or_else(|| line(&|_, _| true))?;
    let (rms0, _) = resid(b, c, &in_band);
    let lim = (3.0 * rms0).max(0.000_05);
    let on = |ti: f64, di: f64| (di - (c + b * ti)).abs() <= lim;
    let (b2, c2, m, tmin, tmax) = line(&on)?;
    let on2 = |ti: f64, di: f64| (di - (c2 + b2 * ti)).abs() <= lim;
    let (rms, mx) = resid(b2, c2, &on2);
    // Off the line: the largest group that agrees within 1 ms.
    let mut cluster2 = 0usize;
    for (&ta, &a) in tv.iter().zip(dv) {
        if on2(ta, a) {
            continue;
        }
        let c = tv
            .iter()
            .zip(dv)
            .filter(|&(&tb, &bb)| !on2(tb, bb) && (a - bb).abs() <= CLUSTER_S)
            .count();
        cluster2 = cluster2.max(c);
    }
    let _ = m;
    let n_on = tv.iter().zip(dv).filter(|&(&ti, &di)| on2(ti, di)).count();
    Some(Fit {
        slope: b2,
        delay_now_s: c2,
        rms_s: rms,
        max_s: mx,
        span_s: tmax - tmin,
        n: n_on,
        total: n,
        cluster2,
    })
}

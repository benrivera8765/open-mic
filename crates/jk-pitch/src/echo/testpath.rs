// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated return paths, for tests and the benchmark: what a mixer does to the app's
//! playback before it comes back in the input (WO2 4.4 "Music in the input", 4.10, section 9
//! row "The owner's rig").
//!
//! Three shapes, each a [`ReturnPath`]:
//!
//! - [`ReturnPath::usb`]: the mixer's USB loopback. Digital: a fixed delay and level on the
//!   same clock as the output, optionally the mixer's EQ.
//! - [`ReturnPath::cable`]: the 3.5 mm cable into AUX-IN. Analog: a fixed delay and level,
//!   a colouring (coupling high-pass, the mixer's channel EQ, the converters' roll-off),
//!   a noise floor, and a clock drift between the output and input devices.
//! - [`ReturnPath::bluetooth`]: music sent to the mixer over Bluetooth. A delay of
//!   100-300 ms that wanders (slow random walk plus the occasional buffer re-sync jump), a
//!   codec band limit, and coding noise that follows the music's loudness.
//!
//! The fractional delay is a 64-tap Kaiser-windowed sinc (beta 10) tabulated at 4096
//! phases: deliberately not the canceller's own interpolator, so the canceller cannot win
//! by modelling its own arithmetic. Everything here allocates freely; it is test code.

use std::f64::consts::PI;

use crate::tracker::testvoice::Rng;

use super::refline::{bessel_i0, sinc};

/// Taps of the simulation's interpolator.
const SIM_TAPS: usize = 64;
/// Tabulated phases of the simulation's interpolator.
const SIM_PHASES: usize = 4096;

/// Which part of the stereo reference comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channels {
    /// The mono sum (L + R) / 2: a mixer's mono AUX-IN or USB return.
    Mono,
    /// Only the left channel: a mono cable plugged into one side.
    Left,
}

/// One filter section of the colouring (RBJ cookbook shapes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    /// Second-order high-pass at `hz`, quality 0.707.
    HighPass { hz: f64 },
    /// Second-order low-pass at `hz`, quality 0.707.
    LowPass { hz: f64 },
    /// Low shelf at `hz`, `db` gain.
    LowShelf { hz: f64, db: f64 },
    /// High shelf at `hz`, `db` gain.
    HighShelf { hz: f64, db: f64 },
    /// Peaking bell at `hz`, `db` gain, quality `q`.
    Peak { hz: f64, db: f64, q: f64 },
}

/// A wandering delay (Bluetooth): a slow random walk plus re-sync jumps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wander {
    /// RMS of the slow random walk around the base delay, ms.
    pub walk_rms_ms: f64,
    /// Time constant of the random walk, s.
    pub walk_tau_s: f64,
    /// Mean time between buffer re-sync jumps, s (0: none).
    pub jump_every_s: f64,
    /// Largest jump, ms (each jump is uniform in +-this, kept within 100..320 ms total).
    pub jump_ms: f64,
}

/// What happens to the app's playback on its way back into the input.
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnPath {
    /// Gain of the returned playback relative to the reference, dB (before the colouring,
    /// which is roughly unity gain in the middle of the band).
    pub level_db: f64,
    /// Delay at the start, s.
    pub delay_s: f64,
    /// Clock drift: the delay grows by this many millionths of a second per second.
    pub drift_ppm: f64,
    pub channels: Channels,
    pub colour: Vec<Section>,
    /// White noise floor added at the input, dBFS RMS (`None`: none).
    pub noise_dbfs: Option<f64>,
    /// Coding noise relative to the returned playback, dB, following its loudness over
    /// about 20 ms (`None`: none). Models a lossy Bluetooth codec.
    pub codec_noise_db: Option<f64>,
    pub wander: Option<Wander>,
    pub seed: u64,
}

impl ReturnPath {
    /// USB loopback: digital, same clock, fixed delay and level.
    pub fn usb(level_db: f64, delay_s: f64) -> Self {
        Self {
            level_db,
            delay_s,
            drift_ppm: 0.0,
            channels: Channels::Mono,
            colour: Vec::new(),
            noise_dbfs: None,
            codec_noise_db: None,
            wander: None,
            seed: 1,
        }
    }

    /// USB loopback through the mixer's EQ (a bass cut, a presence lift).
    pub fn usb_eq(level_db: f64, delay_s: f64) -> Self {
        Self {
            colour: vec![
                Section::HighPass { hz: 40.0 },
                Section::Peak {
                    hz: 3_000.0,
                    db: 4.0,
                    q: 1.0,
                },
                Section::HighShelf {
                    hz: 10_000.0,
                    db: -3.0,
                },
            ],
            ..Self::usb(level_db, delay_s)
        }
    }

    /// A 3.5 mm cable into AUX-IN: analog colouring, a noise floor and `drift_ppm` of clock
    /// drift between the two devices.
    pub fn cable(level_db: f64, delay_s: f64, drift_ppm: f64) -> Self {
        Self {
            level_db,
            delay_s,
            drift_ppm,
            channels: Channels::Mono,
            colour: vec![
                Section::HighPass { hz: 25.0 },
                Section::LowShelf { hz: 120.0, db: 3.0 },
                Section::Peak {
                    hz: 900.0,
                    db: -2.0,
                    q: 0.8,
                },
                Section::HighShelf {
                    hz: 6_000.0,
                    db: -2.5,
                },
                Section::LowPass { hz: 18_000.0 },
            ],
            noise_dbfs: Some(-90.0),
            codec_noise_db: None,
            wander: None,
            seed: 2,
        }
    }

    /// Bluetooth into the mixer: a delay starting at `delay_s` (100-300 ms) that wanders,
    /// a codec band limit and coding noise 30 dB under the music.
    pub fn bluetooth(level_db: f64, delay_s: f64) -> Self {
        Self {
            level_db,
            delay_s,
            drift_ppm: 40.0,
            channels: Channels::Mono,
            colour: vec![
                Section::HighPass { hz: 25.0 },
                Section::LowPass { hz: 15_000.0 },
                Section::HighShelf {
                    hz: 6_000.0,
                    db: -2.0,
                },
            ],
            noise_dbfs: Some(-90.0),
            codec_noise_db: Some(-30.0),
            wander: Some(Wander {
                walk_rms_ms: 2.0,
                walk_tau_s: 3.0,
                jump_every_s: 12.0,
                jump_ms: 25.0,
            }),
            seed: 3,
        }
    }
}

/// Biquad (direct form I, f64).
#[derive(Clone, Copy, Debug, Default)]
struct Bq {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Bq {
    fn new(fs: f64, s: Section) -> Self {
        let q707 = std::f64::consts::FRAC_1_SQRT_2;
        let (hz, db, q) = match s {
            Section::HighPass { hz } | Section::LowPass { hz } => (hz, 0.0, q707),
            Section::LowShelf { hz, db } | Section::HighShelf { hz, db } => (hz, db, q707),
            Section::Peak { hz, db, q } => (hz, db, q),
        };
        let w = 2.0 * PI * hz.min(0.49 * fs) / fs;
        let (sn, cs) = w.sin_cos();
        let alpha = sn / (2.0 * q);
        let a_ = 10f64.powf(db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match s {
            Section::HighPass { .. } => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            Section::LowPass { .. } => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            Section::Peak { .. } => (
                1.0 + alpha * a_,
                -2.0 * cs,
                1.0 - alpha * a_,
                1.0 + alpha / a_,
                -2.0 * cs,
                1.0 - alpha / a_,
            ),
            Section::LowShelf { .. } => {
                let k = 2.0 * a_.sqrt() * alpha;
                (
                    a_ * ((a_ + 1.0) - (a_ - 1.0) * cs + k),
                    2.0 * a_ * ((a_ - 1.0) - (a_ + 1.0) * cs),
                    a_ * ((a_ + 1.0) - (a_ - 1.0) * cs - k),
                    (a_ + 1.0) + (a_ - 1.0) * cs + k,
                    -2.0 * ((a_ - 1.0) + (a_ + 1.0) * cs),
                    (a_ + 1.0) + (a_ - 1.0) * cs - k,
                )
            }
            Section::HighShelf { .. } => {
                let k = 2.0 * a_.sqrt() * alpha;
                (
                    a_ * ((a_ + 1.0) + (a_ - 1.0) * cs + k),
                    -2.0 * a_ * ((a_ - 1.0) + (a_ + 1.0) * cs),
                    a_ * ((a_ + 1.0) + (a_ - 1.0) * cs - k),
                    (a_ + 1.0) - (a_ - 1.0) * cs + k,
                    2.0 * ((a_ - 1.0) - (a_ + 1.0) * cs),
                    (a_ + 1.0) - (a_ - 1.0) * cs - k,
                )
            }
        };
        Self {
            b: [b0 / a0, b1 / a0, b2 / a0],
            a: [a1 / a0, a2 / a0],
            ..Self::default()
        }
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// The delay of `path` at every input sample, seconds (the truth the canceller is
/// measured against).
pub fn delay_track(path: &ReturnPath, rate: u32, frames: usize) -> Vec<f64> {
    let fs = f64::from(rate);
    let mut rng = Rng::new(path.seed.wrapping_mul(0x9E37_79B9) ^ 0x5bd1);
    let mut walk = 0.0f64;
    let mut jump = 0.0f64;
    let mut out = Vec::with_capacity(frames);
    let (a, gain) = match path.wander {
        Some(w) if w.walk_tau_s > 0.0 => {
            let a = (-1.0 / (w.walk_tau_s * fs)).exp();
            // First-order random walk with stationary RMS walk_rms_ms.
            (a, w.walk_rms_ms * 1e-3 * (1.0 - a * a).sqrt() * 3f64.sqrt())
        }
        _ => (0.0, 0.0),
    };
    for i in 0..frames {
        let t = i as f64 / fs;
        let mut d = path.delay_s + path.drift_ppm * 1e-6 * t;
        if let Some(w) = path.wander {
            walk = a * walk + gain * rng.bipolar();
            if w.jump_every_s > 0.0 && rng.unit() < 1.0 / (w.jump_every_s * fs) {
                let base = d + walk + jump;
                let mut step = rng.bipolar() * w.jump_ms * 1e-3;
                if !(0.100..=0.320).contains(&(base + step)) {
                    step = -step;
                }
                jump += step;
            }
            d += walk + jump;
        }
        out.push(d.max(0.0));
    }
    out
}

/// The returned playback for a stereo (interleaved) or mono reference at `rate` Hz, as the
/// input would carry it, plus the delay track (seconds, per input sample) used. The
/// output has as many samples as the reference has frames.
pub fn apply(
    path: &ReturnPath,
    reference: &[f32],
    channels: usize,
    rate: u32,
) -> (Vec<f32>, Vec<f64>) {
    assert!(channels == 1 || channels == 2);
    let fs = f64::from(rate);
    let frames = reference.len() / channels;
    let src: Vec<f64> = (0..frames)
        .map(|i| {
            let f = &reference[i * channels..(i + 1) * channels];
            match (channels, path.channels) {
                (1, _) => f64::from(f[0]),
                (_, Channels::Mono) => 0.5 * (f64::from(f[0]) + f64::from(f[1])),
                (_, Channels::Left) => f64::from(f[0]),
            }
        })
        .collect();
    // Interpolator table.
    let half = SIM_TAPS / 2;
    let beta = 10.0;
    let i0b = bessel_i0(beta);
    let mut table = vec![0.0f64; (SIM_PHASES + 1) * SIM_TAPS];
    for p in 0..=SIM_PHASES {
        let f = p as f64 / SIM_PHASES as f64;
        let mut sum = 0.0;
        for j in 0..SIM_TAPS {
            let t = f + (half as f64 - 1.0) - j as f64;
            let r = t / half as f64;
            let w = if r.abs() >= 1.0 {
                0.0
            } else {
                bessel_i0(beta * (1.0 - r * r).sqrt()) / i0b
            };
            let h = sinc(t) * w;
            table[p * SIM_TAPS + j] = h;
            sum += h;
        }
        for j in 0..SIM_TAPS {
            table[p * SIM_TAPS + j] /= sum;
        }
    }
    let delays = delay_track(path, rate, frames);
    let g = 10f64.powf(path.level_db / 20.0);
    let mut bqs: Vec<Bq> = path.colour.iter().map(|&s| Bq::new(fs, s)).collect();
    let mut out = vec![0.0f32; frames];
    let mut rng = Rng::new(path.seed ^ 0xC0DEC);
    let mut env = 0.0f64;
    let a_env = 1.0 - (-1.0 / (0.020 * fs)).exp();
    let noise_amp = path.noise_dbfs.map(|d| 10f64.powf(d / 20.0) * 3f64.sqrt());
    let codec = path
        .codec_noise_db
        .map(|d| 10f64.powf(d / 20.0) * 3f64.sqrt());
    let mut kern = [0.0f64; SIM_TAPS];
    for (n, o) in out.iter_mut().enumerate() {
        let pos = n as f64 - delays[n] * fs;
        let fl = pos.floor();
        let i = fl as i64;
        let fp = (pos - fl) * SIM_PHASES as f64;
        let p = (fp as usize).min(SIM_PHASES - 1);
        let a = fp - p as f64;
        for j in 0..SIM_TAPS {
            let lo = table[p * SIM_TAPS + j];
            let hi = table[(p + 1) * SIM_TAPS + j];
            kern[j] = lo + a * (hi - lo);
        }
        let first = i - (half as i64 - 1);
        let mut acc = 0.0f64;
        for (j, &k) in kern.iter().enumerate() {
            let idx = first + j as i64;
            if idx >= 0 && (idx as usize) < frames {
                acc += k * src[idx as usize];
            }
        }
        let mut y = acc * g;
        for b in &mut bqs {
            y = b.tick(y);
        }
        if let Some(c) = codec {
            env += a_env * (y * y - env);
            y += c * env.sqrt() * rng.bipolar();
        }
        if let Some(na) = noise_amp {
            y += na * rng.bipolar();
        }
        *o = y as f32;
    }
    (out, delays)
}

/// Mean square of a signal.
pub fn power(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64
}

/// Mean square of the mono mix of an interleaved reference.
pub fn mono_power(reference: &[f32], channels: usize) -> f64 {
    let frames = reference.len() / channels;
    if frames == 0 {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..frames {
        let f = &reference[i * channels..(i + 1) * channels];
        let m = f.iter().map(|&v| f64::from(v)).sum::<f64>() / channels as f64;
        s += m * m;
    }
    s / frames as f64
}

/// Generated stereo "music" (interleaved), for the unit tests that may not use real songs:
/// a four-chord progression of bright harmonic tones with a slow chorus wobble, a bass
/// line, a kick, a snare and hi-hat noise, and a quiet broadband bed (reverb and room tone
/// in a real mix: real music is never a handful of pure lines), `rms_dbfs` RMS in the mono
/// mix. A stretch from `gap.0` to `gap.1` seconds is
/// silent (a break in the song), when given. Deterministic for a seed.
pub fn synth_music(
    rate: u32,
    seconds: f64,
    rms_dbfs: f64,
    gap: Option<(f64, f64)>,
    seed: u64,
) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let mut rng = Rng::new(seed);
    // Chords (MIDI notes), two beats each at 120 bpm.
    let chords: [[f64; 3]; 4] = [
        [57.0, 60.0, 64.0],
        [53.0, 57.0, 60.0],
        [55.0, 59.0, 62.0],
        [52.0, 55.0, 59.0],
    ];
    let bass = [45.0, 41.0, 43.0, 40.0];
    let hz = |m: f64| 440.0 * 2f64.powf((m - 69.0) / 12.0);
    let mut ph = [0.0f64; 4];
    let mut hat = Bq::new(fs, Section::HighPass { hz: 7_000.0 });
    let mut snare_bp = Bq::new(
        fs,
        Section::Peak {
            hz: 1_800.0,
            db: 9.0,
            q: 0.7,
        },
    );
    let mut bed_l = Bq::new(fs, Section::LowPass { hz: 2_500.0 });
    let mut bed_r = Bq::new(fs, Section::LowPass { hz: 2_500.0 });
    let mut out = vec![0.0f32; 2 * n];
    for i in 0..n {
        let t = i as f64 / fs;
        let bar = ((t / 1.0) as usize) % 4;
        let beat_t = t % 0.5;
        let mut l = 0.0f64;
        let mut r = 0.0f64;
        for (v, &m) in chords[bar].iter().enumerate() {
            let wob = 1.0 + 0.0015 * (2.0 * PI * (0.3 + 0.17 * v as f64) * t).sin();
            let f = hz(m) * (1.0 + 0.002 * (v as f64 - 1.0)) * wob;
            ph[v] = (ph[v] + f / fs).fract();
            // Six harmonics, falling 1/h: a bright pad.
            let mut s = 0.0;
            for h in 1..=6 {
                s += (2.0 * PI * h as f64 * ph[v]).sin() / h as f64;
            }
            let pan = 0.3 + 0.2 * v as f64;
            l += s * (1.0 - pan) * 0.25;
            r += s * pan * 0.25;
        }
        let fb = hz(bass[bar]);
        ph[3] = (ph[3] + fb / fs).fract();
        let b = (2.0 * PI * ph[3]).sin() * 0.5 + (4.0 * PI * ph[3]).sin() * 0.2;
        let benv = (-beat_t / 0.3).exp();
        l += b * benv;
        r += b * benv;
        // Kick on every beat, hat on the off-beats.
        let kick =
            (2.0 * PI * (60.0 * beat_t - 40.0 * beat_t * beat_t)).sin() * (-beat_t / 0.08).exp();
        let off = (t + 0.25) % 0.5;
        let hn = hat.tick(rng.bipolar()) * (-off / 0.03).exp() * 0.6;
        let bt = t % 1.0;
        let snare = if bt >= 0.5 {
            snare_bp.tick(rng.bipolar()) * (-(bt - 0.5) / 0.06).exp() * 0.5
        } else {
            snare_bp.tick(0.0)
        };
        let bl = bed_l.tick(rng.bipolar()) * 0.06;
        let br = bed_r.tick(rng.bipolar()) * 0.06;
        l += 0.8 * kick + hn * 0.8 + snare + bl;
        r += 0.8 * kick + hn + snare * 0.9 + br;
        let silent = gap.is_some_and(|(a, b)| t >= a && t < b);
        let g = if silent { 0.0 } else { 1.0 };
        out[2 * i] = (l * g) as f32;
        out[2 * i + 1] = (r * g) as f32;
    }
    // Normalise the mono mix of the non-silent part.
    let mut s = 0.0f64;
    let mut cnt = 0usize;
    for i in 0..n {
        let m = 0.5 * (f64::from(out[2 * i]) + f64::from(out[2 * i + 1]));
        if m != 0.0 {
            s += m * m;
            cnt += 1;
        }
    }
    let rms = (s / cnt.max(1) as f64).sqrt().max(1e-12);
    let g = 10f64.powf(rms_dbfs / 20.0) / rms;
    for v in &mut out {
        *v = (f64::from(*v) * g) as f32;
    }
    out
}

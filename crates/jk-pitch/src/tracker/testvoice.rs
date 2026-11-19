// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated test signals for the pitch tracker: a simple synthetic singer and the
//! not-singing sounds a practice microphone hears (WORK_ORDER-2 section 9, "Tracker").
//!
//! The singer is a source-filter model: a band-limited pulse train (the closed-form sum of
//! equal harmonics up to `min(0.45 fs, 10 kHz)`, so nothing aliases at any rate and the
//! harmonic count follows the pitch sample by sample), a one-pole spectral tilt
//! (-6 dB/octave above 300 Hz: glottal roll-off plus lip radiation) and four cascaded
//! two-pole formant resonators (Klatt style, unity gain at 0 Hz) for the vowel. Optional
//! aspiration noise goes through the same formants. Onsets and releases are 10 ms linear
//! ramps. Everything is deterministic (a fixed-seed xorshift generator), so a test gives the
//! same numbers every run.
//!
//! These are test fixtures for the tracker and for self-checks ("the pitch tracker run on a
//! generated tone", WO2 4.8). They are not meant to sound like a person.

use std::f64::consts::PI;

/// Seconds of the linear onset and release ramps of a sung note.
pub const RAMP_S: f64 = 0.010;

/// Equal-tempered frequency of a MIDI note number (A4 = 69 = 440 Hz).
pub fn midi_hz(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

/// Pitch difference in cents (`est` relative to `truth`).
pub fn cents(est: f64, truth: f64) -> f64 {
    1200.0 * (est / truth).log2()
}

/// Note name with octave for a MIDI note number, sharps only ("C2", "F#4").
pub fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[note.rem_euclid(12) as usize],
        note.div_euclid(12) - 1
    )
}

/// Vowel colour of the synthetic singer (formants of an adult male voice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vowel {
    /// "ah": high first formant, so low notes have a weak fundamental against strong
    /// harmonics near 730 Hz (a test of octave errors).
    Ah,
    /// "ee": low first formant, high second formant.
    Ee,
    /// "oo": low first and second formants, a strong fundamental.
    Oo,
}

impl Vowel {
    /// All vowels, for sweeps.
    pub const ALL: [Vowel; 3] = [Vowel::Ah, Vowel::Ee, Vowel::Oo];

    /// (frequency, bandwidth) of the four formants in Hz.
    pub fn formants(self) -> [(f64, f64); 4] {
        match self {
            Vowel::Ah => [
                (730.0, 90.0),
                (1090.0, 110.0),
                (2440.0, 170.0),
                (3400.0, 250.0),
            ],
            Vowel::Ee => [
                (270.0, 60.0),
                (2290.0, 100.0),
                (3010.0, 170.0),
                (3700.0, 250.0),
            ],
            Vowel::Oo => [
                (300.0, 60.0),
                (870.0, 90.0),
                (2240.0, 170.0),
                (3400.0, 250.0),
            ],
        }
    }

    /// Short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Vowel::Ah => "ah",
            Vowel::Ee => "ee",
            Vowel::Oo => "oo",
        }
    }
}

/// Deterministic xorshift64* noise source.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator from a seed (any value; 0 is remapped).
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

/// Two-pole resonator with unity gain at 0 Hz (D. Klatt, JASA 1980), or a
/// high-pass / low-pass biquad, in f64.
#[derive(Clone, Copy, Debug)]
pub struct Resonator {
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

impl Resonator {
    /// Formant resonator at `f` Hz with bandwidth `bw` Hz.
    pub fn formant(rate: f64, f: f64, bw: f64) -> Self {
        let r = (-PI * bw / rate).exp();
        let c = -r * r;
        let b = 2.0 * r * (2.0 * PI * f / rate).cos();
        Self::raw(1.0 - b - c, 0.0, 0.0, -b, -c)
    }

    /// Second-order (Butterworth, Q = 0.7071) high-pass at `fc` Hz.
    pub fn highpass(rate: f64, fc: f64) -> Self {
        let w0 = 2.0 * PI * fc / rate;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self::raw(
            (1.0 + c) / 2.0 / a0,
            -(1.0 + c) / a0,
            (1.0 + c) / 2.0 / a0,
            -2.0 * c / a0,
            (1.0 - alpha) / a0,
        )
    }

    /// Second-order (Butterworth) low-pass at `fc` Hz.
    pub fn lowpass(rate: f64, fc: f64) -> Self {
        let w0 = 2.0 * PI * fc / rate;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self::raw(
            (1.0 - c) / 2.0 / a0,
            (1.0 - c) / a0,
            (1.0 - c) / 2.0 / a0,
            -2.0 * c / a0,
            (1.0 - alpha) / a0,
        )
    }

    fn raw(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// One sample.
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn cascade(rate: f64, vowel: Vowel) -> [Resonator; 4] {
    vowel
        .formants()
        .map(|(f, bw)| Resonator::formant(rate, f, bw))
}

/// Scale `x` so its RMS over the samples where `mask` is true is `rms_dbfs`.
fn normalise(x: &mut [f32], mask: &[bool], rms_dbfs: f64) {
    let (sum, count) = x
        .iter()
        .zip(mask)
        .filter(|(_, m)| **m)
        .fold((0.0f64, 0usize), |(s, c), (v, _)| {
            (s + f64::from(*v) * f64::from(*v), c + 1)
        });
    if count == 0 || sum <= 0.0 {
        return;
    }
    let rms = (sum / count as f64).sqrt();
    let gain = 10f64.powf(rms_dbfs / 20.0) / rms;
    for v in x {
        *v = (f64::from(*v) * gain) as f32;
    }
}

/// How the synthetic singer sings.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    /// Vowel colour.
    pub vowel: Vowel,
    /// RMS level of the held part of the notes, dBFS.
    pub level_dbfs: f64,
    /// Aspiration noise relative to the voiced sound, in dB (for example -20 for a breathy
    /// voice); `None` for none.
    pub breath_db: Option<f64>,
    /// Noise seed.
    pub seed: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            vowel: Vowel::Ah,
            level_dbfs: -20.0,
            breath_db: None,
            seed: 1,
        }
    }
}

/// Sings `seconds` of audio at `rate` Hz following `f0`: a function from time (seconds) to
/// the pitch in Hz, or `None` for silence. Onsets and releases are [`RAMP_S`] ramps.
pub fn sing(rate: u32, seconds: f64, voice: Voice, f0: impl Fn(f64) -> Option<f64>) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let top = (0.45 * fs).min(10_000.0);
    let mut voiced_path = cascade(fs, voice.vowel);
    let mut noise_path = cascade(fs, voice.vowel);
    let tilt_a = 1.0 - (-2.0 * PI * 300.0 / fs).exp();
    let mut tilt = 0.0f64;
    let mut rng = Rng::new(voice.seed);
    let mut phase = 0.0f64;
    let mut env = 0.0f64;
    let mut last_hz = 220.0f64;
    let step = 1.0 / (RAMP_S * fs);
    // Aspiration gain relative to the pulse train, calibrated so breath_db is roughly the
    // ratio of noise to voice power after the formants (white noise of unit amplitude has
    // power 1/3; the pulse train's power grows with its harmonic count).
    let mut out = vec![0.0f32; n];
    let mut mask = vec![false; n];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / fs;
        let target = match f0(t) {
            Some(hz) => {
                last_hz = hz;
                1.0
            }
            None => 0.0,
        };
        env = if target > env {
            (env + step).min(1.0)
        } else {
            (env - step).max(0.0)
        };
        mask[i] = env >= 1.0;
        phase += 2.0 * PI * last_hz / fs;
        if phase >= 2.0 * PI {
            phase -= 2.0 * PI;
        }
        let harmonics = (top / last_hz).floor().max(1.0);
        // Sum of cos(h phase), h = 1..H, in closed form (Dirichlet kernel), divided by H.
        let s = (0.5 * phase).sin();
        let pulse = if s.abs() < 1e-9 {
            harmonics
        } else {
            ((harmonics + 0.5) * phase).sin() / (2.0 * s) - 0.5
        } / harmonics.sqrt();
        tilt += tilt_a * (pulse - tilt);
        let mut y = tilt;
        for r in &mut voiced_path {
            y = r.tick(y);
        }
        if let Some(db) = voice.breath_db {
            let mut z = rng.bipolar() * 10f64.powf(db / 20.0) * 0.25;
            for r in &mut noise_path {
                z = r.tick(z);
            }
            y += z;
        }
        *o = (y * env) as f32;
    }
    normalise(&mut out, &mask, voice.level_dbfs);
    out
}

/// A held note: `lead` seconds of silence, then `hold` seconds at `hz`, then `tail`
/// seconds of silence.
pub fn note(rate: u32, voice: Voice, hz: f64, lead: f64, hold: f64, tail: f64) -> Vec<f32> {
    sing(rate, lead + hold + tail, voice, |t| {
        (t >= lead && t < lead + hold).then_some(hz)
    })
}

/// Vibrato: `hz` modulated by +-`depth_cents` at `rate_hz`, starting at phase 0 at `t0`.
pub fn vibrato_hz(hz: f64, depth_cents: f64, rate_hz: f64, t: f64, t0: f64) -> f64 {
    hz * 2f64.powf(depth_cents / 1200.0 * (2.0 * PI * rate_hz * (t - t0)).sin())
}

/// Digital silence.
pub fn silence(rate: u32, seconds: f64) -> Vec<f32> {
    vec![0.0; (seconds * f64::from(rate)).round() as usize]
}

/// Breath noise: inhalations and exhalations close to the microphone. Turbulent noise
/// through a vowel tract (high-passed at 400 Hz), in 0.6 s swells with gaps, at
/// `level_dbfs` RMS during the swells.
pub fn breath(rate: u32, seconds: f64, level_dbfs: f64, seed: u64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let mut rng = Rng::new(seed);
    let mut tract = cascade(fs, Vowel::Ah);
    let mut hp = Resonator::highpass(fs, 400.0);
    let mut out = vec![0.0f32; n];
    let mut mask = vec![false; n];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / fs;
        // 0.6 s swell (raised cosine) every 0.9 s.
        let p = t % 0.9;
        let env = if p < 0.6 {
            0.5 - 0.5 * (2.0 * PI * p / 0.6).cos()
        } else {
            0.0
        };
        mask[i] = env > 0.5;
        let mut z = hp.tick(rng.bipolar());
        for r in &mut tract {
            z = r.tick(z);
        }
        *o = (z * env) as f32;
    }
    normalise(&mut out, &mask, level_dbfs);
    out
}

/// Speech-like noise: noise with the long-term spectrum of speech (100 Hz to about 4 kHz,
/// falling above 800 Hz), switched in syllables of 120..280 ms with short gaps and a
/// changing vowel colour (whispered speech). Unvoiced throughout: it must give no pitch.
pub fn speech_noise(rate: u32, seconds: f64, level_dbfs: f64, seed: u64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let mut rng = Rng::new(seed);
    let mut hp = Resonator::highpass(fs, 100.0);
    let mut lp = Resonator::lowpass(fs, 4000.0_f64.min(0.45 * fs));
    let tilt_a = 1.0 - (-2.0 * PI * 800.0 / fs).exp();
    let mut tilt = 0.0f64;
    let mut out = vec![0.0f32; n];
    let mut mask = vec![false; n];
    let mut i = 0usize;
    let mut syl = 0usize;
    while i < n {
        let len = ((0.12 + 0.16 * rng.unit()) * fs) as usize;
        let gap = ((0.03 + 0.08 * rng.unit()) * fs) as usize;
        let mut tract = cascade(fs, Vowel::ALL[syl % 3]);
        syl += 1;
        for k in 0..len.min(n - i) {
            let env = (PI * k as f64 / len as f64).sin();
            let mut z = hp.tick(rng.bipolar());
            tilt += tilt_a * (z - tilt);
            z = lp.tick(tilt);
            for r in &mut tract {
                z = r.tick(z);
            }
            out[i + k] = (z * env) as f32;
            mask[i + k] = env > 0.7;
        }
        i += len + gap;
    }
    normalise(&mut out, &mask, level_dbfs);
    out
}

/// Fricative consonants ("s" 4..8 kHz and "sh" 2..5 kHz, 150 ms each, alternating with
/// 100 ms gaps), at `level_dbfs` RMS while sounding.
pub fn fricatives(rate: u32, seconds: f64, level_dbfs: f64, seed: u64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let mut rng = Rng::new(seed);
    let nyq = 0.45 * fs;
    let mut s_band = [
        Resonator::highpass(fs, 4000.0_f64.min(nyq * 0.8)),
        Resonator::lowpass(fs, 8000.0_f64.min(nyq)),
    ];
    let mut sh_band = [
        Resonator::highpass(fs, 2000.0),
        Resonator::lowpass(fs, 5000.0_f64.min(nyq)),
    ];
    let mut out = vec![0.0f32; n];
    let mut mask = vec![false; n];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / fs;
        let slot = (t / 0.25) as usize;
        let p = t - slot as f64 * 0.25;
        let x = rng.bipolar();
        let band = if slot.is_multiple_of(2) {
            &mut s_band
        } else {
            &mut sh_band
        };
        let mut z = x;
        for r in band.iter_mut() {
            z = r.tick(z);
        }
        let env = if p < 0.15 { (PI * p / 0.15).sin() } else { 0.0 };
        mask[i] = env > 0.7;
        *o = (z * env) as f32;
    }
    normalise(&mut out, &mask, level_dbfs);
    out
}

/// Room noise: brownish background noise (ventilation, traffic; -6 dB/octave above 50 Hz)
/// at `level_dbfs` RMS, plus optional mains hum at `hum_hz` with its first four harmonics
/// at `hum_dbfs` RMS.
pub fn room_noise(
    rate: u32,
    seconds: f64,
    level_dbfs: f64,
    hum: Option<(f64, f64)>,
    seed: u64,
) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let mut rng = Rng::new(seed);
    let a = 1.0 - (-2.0 * PI * 50.0 / fs).exp();
    let mut lp = 0.0f64;
    let mut hp = Resonator::highpass(fs, 20.0);
    let mut out: Vec<f32> = (0..n)
        .map(|_| {
            lp += a * (rng.bipolar() - lp);
            hp.tick(lp) as f32
        })
        .collect();
    normalise(&mut out, &vec![true; n], level_dbfs);
    if let Some((hz, db)) = hum {
        let mut h: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                (1..=5)
                    .map(|k| (2.0 * PI * hz * k as f64 * t).sin() / k as f64)
                    .sum::<f64>() as f32
            })
            .collect();
        normalise(&mut h, &vec![true; n], db);
        for (o, v) in out.iter_mut().zip(h) {
            *o += v;
        }
    }
    out
}

/// A handling thump of a handheld microphone: an impulse into a low resonance (`hz`,
/// 40 Hz wide, so it rings for about 10 ms) plus a smaller body resonance at 4 x `hz`,
/// with a peak near `peak_dbfs`. `at` seconds into a `seconds`-long buffer.
pub fn thump(rate: u32, seconds: f64, at: f64, hz: f64, peak_dbfs: f64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let start = (at * fs).round() as usize;
    let mut low = Resonator::formant(fs, hz, 40.0);
    let mut body = Resonator::formant(fs, 4.0 * hz, 120.0);
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate().skip(start) {
        // A 2 ms raised-cosine knock as the excitation.
        let k = i - start;
        let len = (0.002 * fs) as usize;
        let x = if k < len {
            0.5 - 0.5 * (2.0 * PI * k as f64 / len as f64).cos()
        } else {
            0.0
        };
        *o = (low.tick(x) + 0.3 * body.tick(x)) as f32;
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        let g = 10f32.powf(peak_dbfs as f32 / 20.0) / peak;
        out.iter_mut().for_each(|v| *v *= g);
    }
    out
}

/// A popped plosive ("p" or "b" too close to the microphone): a pressure pulse (one
/// half-cycle of about 20 Hz, 25 ms, peak near `peak_dbfs`) followed by a 30 ms burst of
/// broadband noise 16 dB lower, `at` seconds into a `seconds`-long buffer.
pub fn plosive(rate: u32, seconds: f64, at: f64, peak_dbfs: f64, seed: u64) -> Vec<f32> {
    let fs = f64::from(rate);
    let n = (seconds * fs).round() as usize;
    let start = (at * fs).round() as usize;
    let mut rng = Rng::new(seed);
    let peak = 10f64.powf(peak_dbfs / 20.0);
    let pulse = (0.025 * fs) as usize;
    let burst = (0.030 * fs) as usize;
    let mut hp = Resonator::highpass(fs, 500.0);
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate().skip(start) {
        let k = i - start;
        let mut v = 0.0;
        if k < pulse {
            v += peak * (PI * k as f64 / pulse as f64).sin();
        }
        if k < burst {
            v += peak * 0.16 * hp.tick(rng.bipolar()) * (1.0 - k as f64 / burst as f64);
        }
        *o = v as f32;
    }
    out
}

/// Interleave mono channels into one buffer (frames of `channels.len()` samples).
pub fn interleave(channels: &[&[f32]]) -> Vec<f32> {
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n * channels.len());
    for i in 0..n {
        for c in channels {
            out.push(c[i]);
        }
    }
    out
}

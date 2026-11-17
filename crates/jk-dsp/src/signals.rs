// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic test-signal generators (tests, golden files, CLI fixtures).
//!
//! Everything here uses integer PRNGs and the pure-Rust `libm` crate, so the same call
//! gives bit-identical samples on Windows and macOS. No copyrighted material: every
//! "song" is synthesised from sines and noise.

use core::f64::consts::PI;

/// `amp * sin(2*pi*freq*n/rate + phase)` for `n in 0..frames`.
pub fn sine(freq: f64, rate: u32, frames: usize, amp: f64, phase: f64) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            // Reduce the phase in cycles first so precision does not degrade with n.
            let cyc = (freq * n as f64 / rate as f64).fract();
            (amp * libm::sin(2.0 * PI * cyc + phase)) as f32
        })
        .collect()
}

/// A harmonic tone: partial k (1-based) has amplitude `amps[k-1]`, scaled so the sum of
/// amplitudes is `amp`.
pub fn harmonic(f0: f64, amps: &[f64], rate: u32, frames: usize, amp: f64) -> Vec<f32> {
    let total: f64 = amps.iter().sum();
    let mut out = vec![0.0f64; frames];
    for (k, a) in amps.iter().enumerate() {
        let f = f0 * (k + 1) as f64;
        if f >= rate as f64 / 2.0 {
            break;
        }
        for (n, o) in out.iter_mut().enumerate() {
            let cyc = (f * n as f64 / rate as f64).fract();
            *o += a / total * amp * libm::sin(2.0 * PI * cyc + 0.3 * k as f64);
        }
    }
    out.into_iter().map(|v| v as f32).collect()
}

/// Small deterministic PRNG (xorshift32), uniform in [-1, 1).
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Self {
        Noise(seed.max(1))
    }
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// Add `src * gain` into `dst` starting at `at` (clipped to `dst`).
pub fn add_at(dst: &mut [f32], src: &[f32], at: usize, gain: f32) {
    if at >= dst.len() {
        return;
    }
    let n = src.len().min(dst.len() - at);
    for i in 0..n {
        dst[at + i] += src[i] * gain;
    }
}

/// A percussive "kick": exponentially decaying sine sweeping 150 -> 50 Hz.
pub fn kick(rate: u32) -> Vec<f32> {
    let n = (rate as f64 * 0.35) as usize;
    let mut ph = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let f = 50.0 + 100.0 * libm::exp(-t * 30.0);
            ph += f / rate as f64;
            (libm::sin(2.0 * PI * ph.fract()) * libm::exp(-t * 9.0)) as f32
        })
        .collect()
}

/// A "snare": noise burst plus a 190 Hz body, fast decay.
pub fn snare(rate: u32, seed: u32) -> Vec<f32> {
    let n = (rate as f64 * 0.25) as usize;
    let mut nz = Noise::new(seed);
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let body = libm::sin(2.0 * PI * (190.0 * t).fract()) * libm::exp(-t * 25.0) * 0.5;
            let noise = nz.next_f32() as f64 * libm::exp(-t * 18.0) * 0.6;
            (body + noise) as f32
        })
        .collect()
}

/// A "hi-hat": first difference of noise (high-passed), very fast decay.
pub fn hihat(rate: u32, seed: u32) -> Vec<f32> {
    let n = (rate as f64 * 0.06) as usize;
    let mut nz = Noise::new(seed);
    let mut prev = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let x = nz.next_f32();
            let y = (x - prev) * 0.5;
            prev = x;
            (y as f64 * libm::exp(-t * 70.0)) as f32
        })
        .collect()
}

/// A plucked note: harmonic tone with a 2 ms attack and exponential decay.
pub fn pluck(f0: f64, rate: u32, dur_s: f64, amp: f64) -> Vec<f32> {
    let n = (rate as f64 * dur_s) as usize;
    let tone = harmonic(f0, &[1.0, 0.5, 0.33, 0.25, 0.2, 0.12], rate, n, amp);
    tone.into_iter()
        .enumerate()
        .map(|(i, v)| {
            let t = i as f64 / rate as f64;
            let env = (t / 0.002).min(1.0) * libm::exp(-t * 3.0);
            (v as f64 * env) as f32
        })
        .collect()
}

/// A sustained note with vibrato and soft attack/release (a stand-in for a singer).
pub fn sung_note(f0: f64, rate: u32, dur_s: f64, amp: f64) -> Vec<f32> {
    let n = (rate as f64 * dur_s) as usize;
    let amps = [1.0, 0.6, 0.4, 0.25, 0.15, 0.1, 0.06];
    let total: f64 = amps.iter().sum();
    let mut ph = [0.0f64; 7];
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let vib = 1.0 + 0.006 * libm::sin(2.0 * PI * (5.5 * t).fract());
            let env = (t / 0.04).min(1.0) * ((dur_s - t) / 0.06).clamp(0.0, 1.0);
            let mut s = 0.0;
            for (k, a) in amps.iter().enumerate() {
                let f = f0 * (k + 1) as f64 * vib;
                if f < rate as f64 / 2.0 {
                    ph[k] += f / rate as f64;
                    ph[k] = ph[k].fract();
                    s += a / total * libm::sin(2.0 * PI * ph[k]);
                }
            }
            (s * env * amp) as f32
        })
        .collect()
}

fn midi_hz(note: i32) -> f64 {
    440.0 * libm::pow(2.0, (note - 69) as f64 / 12.0)
}

/// A synthetic four-stem song (stereo), entirely generated: lead (sung melody), backing
/// (pad chords), drums (kick/snare/hats) and other (bass + plucked arpeggio).
/// Returns `stems[stem][channel]` in [`crate::params::Stem`] order. Peak levels stay
/// below 0.5 per stem so the sum stays below full scale.
pub fn synth_song(rate: u32, seconds: f64, seed: u32) -> [[Vec<f32>; 2]; 4] {
    let frames = (rate as f64 * seconds) as usize;
    let bpm = 100.0;
    let beat = (60.0 / bpm * rate as f64) as usize;
    let mut lead = vec![0.0f32; frames];
    let mut back_l = vec![0.0f32; frames];
    let mut back_r = vec![0.0f32; frames];
    let mut drums_l = vec![0.0f32; frames];
    let mut drums_r = vec![0.0f32; frames];
    let mut bass = vec![0.0f32; frames];
    let mut arp_l = vec![0.0f32; frames];
    let mut arp_r = vec![0.0f32; frames];

    // A-minor-ish progression: Am F C G, one bar (4 beats) each.
    let chords: [[i32; 3]; 4] = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]];
    let melody = [69, 72, 71, 69, 67, 69, 64, 65, 67, 69, 72, 74, 72, 71, 69, 67];
    let kick_s = kick(rate);
    let n_beats = frames / beat + 1;
    for b in 0..n_beats {
        let at = b * beat;
        let bar = (b / 4) % 4;
        // drums
        if b % 2 == 0 {
            add_at(&mut drums_l, &kick_s, at, 0.45);
            add_at(&mut drums_r, &kick_s, at, 0.45);
        } else {
            let sn = snare(rate, seed.wrapping_add(b as u32 * 7 + 1));
            add_at(&mut drums_l, &sn, at, 0.35);
            add_at(&mut drums_r, &sn, at, 0.30);
        }
        for h in 0..2 {
            let hh = hihat(rate, seed.wrapping_add(b as u32 * 13 + h + 5));
            let pos = at + h as usize * beat / 2;
            add_at(&mut drums_l, &hh, pos, 0.12);
            add_at(&mut drums_r, &hh, pos, 0.18);
        }
        // bass on every beat (root, octave below)
        let root = chords[bar][0] - 12;
        let bn = pluck(midi_hz(root), rate, 0.5, 0.35);
        add_at(&mut bass, &bn, at, 1.0);
        // arpeggio eighths
        for e in 0..2 {
            let note = chords[bar][(b * 2 + e) % 3] + 12;
            let p = pluck(midi_hz(note), rate, 0.3, 0.15);
            let pos = at + e * beat / 2;
            add_at(&mut arp_l, &p, pos, if e == 0 { 1.0 } else { 0.6 });
            add_at(&mut arp_r, &p, pos, if e == 0 { 0.6 } else { 1.0 });
        }
        // backing pad: whole bar chords
        if b % 4 == 0 {
            for (k, &n) in chords[bar].iter().enumerate() {
                let pad = sung_note(midi_hz(n), rate, 4.0 * beat as f64 / rate as f64, 0.08);
                add_at(&mut back_l, &pad, at, if k == 0 { 1.0 } else { 0.7 });
                add_at(&mut back_r, &pad, at, if k == 2 { 1.0 } else { 0.7 });
            }
        }
        // lead: one sung note per beat
        let ln = sung_note(midi_hz(melody[b % melody.len()]), rate, 0.9 * beat as f64 / rate as f64, 0.3);
        add_at(&mut lead, &ln, at, 1.0);
    }
    let other_l: Vec<f32> = bass.iter().zip(&arp_l).map(|(a, b)| a + b).collect();
    let other_r: Vec<f32> = bass.iter().zip(&arp_r).map(|(a, b)| a + b).collect();
    [
        [lead.clone(), lead],
        [back_l, back_r],
        [drums_l, drums_r],
        [other_l, other_r],
    ]
}

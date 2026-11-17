// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic: how the output position of an isolated impulse depends on where the impulse
//! falls relative to Rubber Band's hop grid. Run with --release.
#![allow(clippy::needless_range_loop)]
use jk_rubberband::{semitones_to_scale, PitchMode, Stretcher, StretcherConfig};

fn render(s: &mut Stretcher, x: Option<usize>, amp_noise: f32) -> Vec<f32> {
    s.reset();
    let pad = s.preferred_start_pad();
    let n_in = 40000usize;
    let mut l = vec![0f32; n_in];
    // optional low-level noise bed (a more music-like context than digital silence)
    let mut seed = 777u32;
    for v in l.iter_mut() {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *v = ((seed >> 8) as f32 / 16777216.0 - 0.5) * amp_noise;
    }
    if let Some(x) = x { l[pad + x] += 0.5; }
    let r = l.clone();
    let mut out = Vec::new();
    let mut ol = vec![0f32; 16384];
    let mut or = vec![0f32; 16384];
    let mut i = 0;
    while i < n_in {
        let n = 512.min(n_in - i);
        s.process(&[&l[i..i + n], &r[i..i + n]], n, false);
        i += n;
        loop {
            let a = s.available().unwrap_or(0).min(16384);
            if a == 0 { break; }
            let mut o: [&mut [f32]; 2] = [&mut ol[..a], &mut or[..a]];
            let g = s.retrieve(&mut o);
            out.extend_from_slice(&ol[..g]);
        }
    }
    out
}

fn run(s: &mut Stretcher, x: usize, amp_noise: f32) -> (f64, f64) {
    let delay = s.start_delay();
    let with = render(s, Some(x), amp_noise);
    let out: Vec<f32> = if amp_noise > 0.0 {
        let without = render(s, None, amp_noise);
        with.iter().zip(&without).map(|(a, b)| a - b).collect()
    } else { with };
    let want = delay + x; // raw output index where the impulse should land
    let lo = want.saturating_sub(3000);
    let hi = (want + 3000).min(out.len());
    let (mut best, mut bi) = (0f32, 0);
    let (mut e, mut ek) = (0f64, 0f64);
    for k in lo..hi {
        if out[k].abs() > best { best = out[k].abs(); bi = k; }
        let p = (out[k] as f64).powi(2);
        e += p; ek += p * k as f64;
    }
    (bi as f64 - want as f64, ek / e - want as f64)
}

fn main() {
    for &mode in &[PitchMode::HighQuality, PitchMode::HighConsistency] {
        for &semis in &[-12, -5, -2, 2, 5, 12] {
            let mut cfg = StretcherConfig::new(44_100, 2);
            cfg.pitch_mode = mode;
            let mut s = Stretcher::new(cfg).unwrap();
            s.set_pitch_scale(semitones_to_scale(semis as f64));
            let mut peaks = vec![];
            let mut cents = vec![];
            for k in 0..32 {
                let x = 10_000 + k * 37;
                let (p, c) = run(&mut s, x, std::env::var("NOISE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0));
                peaks.push(p);
                cents.push(c);
            }
            let mean = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
            let mn = |v: &Vec<f64>| v.iter().cloned().fold(f64::MAX, f64::min);
            let mx = |v: &Vec<f64>| v.iter().cloned().fold(f64::MIN, f64::max);
            println!("{mode:?} {semis:+3}: peak err mean {:7.1} min {:6.0} max {:6.0} | centroid mean {:7.1} min {:7.1} max {:7.1}",
                mean(&peaks), mn(&peaks), mx(&peaks), mean(&cents), mn(&cents), mx(&cents));
        }
    }
}

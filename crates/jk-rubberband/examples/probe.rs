// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic probe for the real-time R3 stretcher: start pad / start delay per pitch,
//! how far output lags input in streaming use (this sets the graph latency in jk-dsp),
//! and where an impulse comes out.
//!
//! Run: cargo run --release -p jk-rubberband --example probe
#![allow(clippy::needless_range_loop)]

use jk_rubberband::{semitones_to_scale, PitchMode, Stretcher, StretcherConfig};

fn main() {
    for &rate in &[44_100u32, 48_000] {
        for &mode in &[PitchMode::HighQuality, PitchMode::HighConsistency] {
            println!("== rate {rate} mode {mode:?}");
            println!("semis  pad  delay  latency  maxlag(b=64) maxlag(b=512) maxlag(b=4096)  impulse_err");
            for semis in -12..=12 {
                let scale = semitones_to_scale(semis as f64);
                let mut cfg = StretcherConfig::new(rate, 2);
                cfg.pitch_mode = mode;
                cfg.max_process_size = 8192;
                let mut s = Stretcher::new(cfg).unwrap();
                s.set_pitch_scale(scale);
                let pad = s.preferred_start_pad();
                let delay = s.start_delay();
                let lat = s.latency();
                let mut lags = vec![];
                for &b in &[64usize, 512, 4096] {
                    lags.push(max_lag(&mut s, b, pad, delay));
                }
                let imp = impulse_err(&mut s, pad, delay);
                println!(
                    "{semis:>5} {pad:>5} {delay:>6} {lat:>8} {:>13} {:>13} {:>14}  {imp:?}",
                    lags[0], lags[1], lags[2]
                );
            }
        }
    }
}

/// Feed `b`-frame blocks of noise; after each block retrieve everything. Return the largest
/// gap between "input fed" and "input index of the newest available output" (in the
/// aligned index space: raw output o <-> raw input o - delay + pad).
fn max_lag(s: &mut Stretcher, b: usize, pad: usize, delay: usize) -> i64 {
    s.reset();
    let mut seed = 12345u32;
    let mut l = vec![0f32; b];
    let mut r = vec![0f32; b];
    let mut out_l = vec![0f32; 16384];
    let mut out_r = vec![0f32; 16384];
    let mut fed: i64 = 0;
    let mut out: i64 = 0;
    let mut worst = 0i64;
    let total = 48_000 * 4;
    while fed < total {
        for i in 0..b {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            l[i] = ((seed >> 8) as f32 / 16777216.0 - 0.5) * 0.5;
            r[i] = l[i] * 0.7;
        }
        s.process(&[&l, &r], b, false);
        fed += b as i64;
        loop {
            let n = s.available().unwrap_or(0).min(16384);
            if n == 0 {
                break;
            }
            let mut o: [&mut [f32]; 2] = [&mut out_l[..n], &mut out_r[..n]];
            out += s.retrieve(&mut o) as i64;
        }
        // newest aligned input index available at output = out - delay + pad
        let lag = fed - (out - delay as i64 + pad as i64);
        if fed > 16384 {
            worst = worst.max(lag);
        }
    }
    worst
}

/// Impulse at raw input index X (after `pad` zeros); find the peak of the output and map it
/// back to input index space. Returns (peak_index_error, centroid_error).
fn impulse_err(s: &mut Stretcher, pad: usize, delay: usize) -> (i64, f64) {
    s.reset();
    let n_in = 32768usize;
    let x = 10_000usize;
    let mut l = vec![0f32; n_in];
    l[pad + x] = 0.5;
    let r = l.clone();
    let mut outl = Vec::new();
    let b = 512;
    let mut ol = vec![0f32; 16384];
    let mut or = vec![0f32; 16384];
    let mut i = 0;
    while i < n_in {
        let n = b.min(n_in - i);
        s.process(&[&l[i..i + n], &r[i..i + n]], n, false);
        i += n;
        loop {
            let a = s.available().unwrap_or(0).min(16384);
            if a == 0 {
                break;
            }
            let mut o: [&mut [f32]; 2] = [&mut ol[..a], &mut or[..a]];
            let got = s.retrieve(&mut o);
            outl.extend_from_slice(&ol[..got]);
        }
    }
    let (mut best, mut bi) = (0f32, 0usize);
    for (k, v) in outl.iter().enumerate() {
        if v.abs() > best {
            best = v.abs();
            bi = k;
        }
    }
    // energy centroid in a +-2048 window around the peak
    let lo = bi.saturating_sub(2048);
    let hi = (bi + 2048).min(outl.len());
    let (mut e, mut ek) = (0f64, 0f64);
    for k in lo..hi {
        let p = (outl[k] as f64).powi(2);
        e += p;
        ek += p * k as f64;
    }
    let centroid = ek / e;
    // aligned: raw output o <-> raw input o - delay + pad; impulse raw input = pad + x
    let peak_err = bi as i64 - delay as i64 - x as i64;
    let cen_err = centroid - delay as f64 - x as f64;
    (peak_err, (cen_err * 100.0).round() / 100.0)
}

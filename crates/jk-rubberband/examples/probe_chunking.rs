// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic: does R3 real-time output depend on how the input is split into process()
//! calls? Run with --release.
use jk_rubberband::{semitones_to_scale, Stretcher, StretcherConfig};

fn run(semis: f64, chunks: &dyn Fn(usize) -> usize) -> Vec<f32> {
    let mut cfg = StretcherConfig::new(44_100, 2);
    cfg.max_process_size = 8192;
    let mut s = Stretcher::new(cfg).unwrap();
    s.set_pitch_scale(semitones_to_scale(semis));
    let n = 44_100 * 3;
    let mut seed = 99u32;
    let x: Vec<f32> = (0..n).map(|i| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((seed >> 8) as f32 / 16777216.0 - 0.5) * 0.3 + (i as f32 * 0.0627).sin() * 0.3
    }).collect();
    let mut out = Vec::new();
    let mut ol = vec![0f32; 16384];
    let mut or = vec![0f32; 16384];
    let mut pos = 0;
    let mut k = 0;
    while pos < n {
        let c = chunks(k).min(n - pos);
        k += 1;
        s.process(&[&x[pos..pos + c], &x[pos..pos + c]], c, false);
        pos += c;
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

fn main() {
    for semis in [-7.0, 0.0, 3.0, 12.0] {
        let a = run(semis, &|_| 512);
        for (name, f) in [("64", &(|_: usize| 64usize) as &dyn Fn(usize) -> usize), ("1000", &|_: usize| 1000usize), ("4096", &|_: usize| 4096usize), ("varying", &|k: usize| 37 + (k * 101) % 900)] {
            let b = run(semis, f);
            let n = a.len().min(b.len());
            let mut maxd = 0f32; let mut first = None;
            for i in 0..n { let d = (a[i] - b[i]).abs(); if d > 0.0 && first.is_none() { first = Some(i); } maxd = maxd.max(d); }
            println!("semis {semis:+}: chunks {name:>7} vs 512: len {} vs {}, max diff {maxd:e}, first diff at {first:?}", a.len(), b.len());
        }
    }
}

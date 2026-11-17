// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic: is a used-then-reset stretcher identical to a fresh one? Run with --release.
use jk_rubberband::{semitones_to_scale, Stretcher, StretcherConfig};

fn signal(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n).map(|i| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); ((s >> 8) as f32 / 16777216.0 - 0.5) * 0.3 + (i as f32 * 0.0627).sin() * 0.3 }).collect()
}

fn run(s: &mut Stretcher, x: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut ol = vec![0f32; 16384];
    let mut or = vec![0f32; 16384];
    for c in x.chunks(256) {
        s.process(&[c, c], c.len(), false);
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
    let x = signal(44_100 * 2, 5);
    let junk = signal(44_100, 77);
    for (first, second) in [(3.0, -12.0), (-7.0, -12.0), (12.0, 3.0), (3.0, 3.0)] {
        let mut cfg = StretcherConfig::new(44_100, 2);
        cfg.max_process_size = 256;
        let mut fresh = Stretcher::new(cfg).unwrap();
        fresh.set_pitch_scale(semitones_to_scale(second));
        let a = run(&mut fresh, &x);
        let mut used = Stretcher::new(cfg).unwrap();
        used.set_pitch_scale(semitones_to_scale(first));
        let _ = run(&mut used, &junk[..(44_100 - 1000)]);
        used.reset();
        used.set_pitch_scale(semitones_to_scale(second));
        let b = run(&mut used, &x);
        let n = a.len().min(b.len());
        let first_diff = (0..n).position(|i| a[i] != b[i]);
        let maxd = (0..n).fold(0f32, |m, i| m.max((a[i] - b[i]).abs()));
        println!("used at {first:+}, reset, run at {second:+}: len {} vs {}, first diff {first_diff:?}, max diff {maxd:e}", a.len(), b.len());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic: how long does constructing an R3 real-time stretcher take? Run with --release.
use jk_rubberband::{Stretcher, StretcherConfig};
fn main() {
    for rate in [44_100u32, 48_000, 96_000] {
        let t0 = std::time::Instant::now();
        let mut worst = 0f64;
        for _ in 0..50 {
            let t = std::time::Instant::now();
            let s = Stretcher::new(StretcherConfig::new(rate, 2)).unwrap();
            drop(s);
            worst = worst.max(t.elapsed().as_secs_f64());
        }
        println!("{rate} Hz: create+drop mean {:.3} ms, worst {:.3} ms", t0.elapsed().as_secs_f64() * 1000.0 / 50.0, worst * 1000.0);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Measures Rubber Band R3's transient timing bias per key shift through the full graph
//! and prints the table for `TRIM_FFT_4096` or `TRIM_FFT_8192` in src/shifter.rs (whichever
//! serves this rate), plus the residual with the current table.
//! Run: cargo run --release -p jk-dsp --example calibrate_trim [rate] [stride] [count]
use jk_dsp::analysis::impulse_alignment;
use jk_dsp::shifter::{r3_longest_fft, transient_trim};

fn main() {
    let rate: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(44_100);
    let stride: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(17);
    let count: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(64);
    // The tables are stored in samples at their own FFT size (4096: measured at 44.1 kHz,
    // 8192: measured at 96 kHz); `transient_trim` scales by fft / base for other sizes.
    let fft = r3_longest_fft(rate) as f64;
    let base = if fft <= 4096.0 { 4096.0 } else { 8192.0 };
    println!("rate {rate} stride {stride} count {count}: semis, current trim, mean offset with current table, min, max");
    let mut table = Vec::new();
    for semis in -12..=12 {
        if semis == 0 {
            table.push(0);
            continue;
        }
        let a = impulse_alignment(rate, semis, count, stride).unwrap();
        let trim = transient_trim(rate, semis);
        println!("{semis:+3} trim {trim:5}  mean {:8.2}  min {:5}  max {:5}", a.mean(), a.min(), a.max());
        // New trim = current trim + residual mean, expressed in the table's own units.
        table.push((((trim as f64) + a.mean()) * base / fft).round() as i32);
    }
    println!("const TRIM_FFT_{}: [i32; 25] = {table:?};", base as u32);
}

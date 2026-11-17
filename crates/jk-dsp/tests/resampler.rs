// SPDX-License-Identifier: GPL-3.0-or-later
//! Output resampler quality (rubato windowed sinc, used only when device rate != song rate).

mod common;
use common::*;
use jk_dsp::analysis::{argmax_abs, db};
use jk_dsp::resample_whole;
use jk_dsp::signals::sine;

/// Least-squares fit of `a sin + b cos` at frequency `f` over `y`; returns
/// (amplitude, residual RMS).
fn fit(y: &[f32], f: f64, rate: u32) -> (f64, f64) {
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, &v) in y.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += v as f64 * s;
        yc += v as f64 * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    let mut e = 0.0;
    for (n, &v) in y.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        e += (v as f64 - a * s - b * c).powi(2);
    }
    ((a * a + b * b).sqrt(), (e / y.len() as f64).sqrt())
}

#[test]
fn resampler_quality() {
    let mut rep = Report::new("resampler");
    rep.line("rubato async sinc: 256 taps, Blackman-Harris^2, 256x oversampling, cubic; input sine at -6.02 dBFS (amplitude 0.5), 2 s; fit over the middle 1 s");
    rep.line("conversion | tone | output length (expected) | gain (dB) | THD+N (dB re tone)");
    let mut worst_thdn = f64::MIN;
    let mut worst_gain_20k = 0.0f64;
    for &(fin, fout) in &[(44_100u32, 48_000u32), (48_000, 44_100), (44_100, 96_000), (48_000, 96_000)] {
        for &f in &[100.0, 1_000.0, 10_000.0, 18_000.0, 20_000.0] {
            let frames = 2 * fin as usize;
            let x = sine(f, fin, frames, 0.5, 0.0);
            let (out, _delay) = resample_whole(&[x.clone(), x], fin, fout).unwrap();
            let expect_len = (frames as u64 * fout as u64 + fin as u64 / 2) / fin as u64;
            assert_eq!(out[0].len() as u64, expect_len);
            let mid = &out[0][fout as usize / 2..3 * fout as usize / 2];
            let (amp, res) = fit(mid, f, fout);
            let gain = db(amp / 0.5);
            let thdn = db(res / (amp / 2f64.sqrt()));
            rep.line(format!("{fin}->{fout} | {f} Hz | {} ({expect_len}) | {gain:+.4} | {thdn:.1}", out[0].len()));
            if f <= 18_000.0 {
                worst_thdn = worst_thdn.max(thdn);
            } else {
                worst_gain_20k = worst_gain_20k.min(gain);
            }
        }
    }
    // Timing: an impulse must land at the rate-converted position.
    let fin = 44_100u32;
    let fout = 48_000u32;
    let mut x = vec![0.0f32; fin as usize];
    let p = 20_000usize;
    x[p] = 0.5;
    let (out, delay) = resample_whole(&[x.clone(), x], fin, fout).unwrap();
    let want = p as f64 * fout as f64 / fin as f64;
    let got = argmax_abs(&out[0], 0..out[0].len());
    rep.line(format!(
        "impulse at {p} (44.1 kHz) -> peak at {got} (48 kHz), exact position {want:.2}; resampler delay removed: {delay} output frames"
    ));
    assert!((got as f64 - want).abs() <= 1.0);
    rep.line(format!("SUMMARY: worst THD+N up to 18 kHz {worst_thdn:.1} dB; worst gain at 20 kHz {worst_gain_20k:+.3} dB"));
    assert!(worst_thdn < -100.0, "resampler THD+N above -100 dB");
}

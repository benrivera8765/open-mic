// SPDX-License-Identifier: GPL-3.0-or-later
//! Downmix and integer-factor decimation for the guide's analysis signal.
//!
//! The guide analyses a lead vocal stem, so averaging its channels is right here (the
//! microphone path never adds channels: WO2 trap 14). Decimating by an integer factor
//! with a linear-phase FIR keeps every sample rate on the same frame grid in seconds and
//! needs no resampler crate.

/// Average of the channels of interleaved audio.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    let scale = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() * scale)
        .collect()
}

/// Decimates `x` (rate `sr`) by the integer factor closest to `sr / target`, after a
/// Blackman-windowed sinc low-pass at 90 % of the new Nyquist frequency. Returns the
/// samples and their rate. Output sample `k` is input sample `k * factor` (zero delay:
/// the filter is applied centred).
pub fn decimate_to_about(x: &[f32], sr: f64, target: f64) -> (Vec<f32>, f64) {
    let factor = ((sr / target).round() as usize).max(1);
    if factor == 1 {
        return (x.to_vec(), sr);
    }
    let half = 16 * factor; // taps on each side
    let cutoff = 0.45 / factor as f64; // cycles per input sample
    let len = 2 * half + 1;
    let mut h = vec![0.0f64; len];
    for (n, v) in h.iter_mut().enumerate() {
        let m = n as f64 - half as f64;
        let sinc = if m == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * std::f64::consts::PI * cutoff * m).sin() / (std::f64::consts::PI * m)
        };
        let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / (len - 1) as f64).cos()
            + 0.08 * (4.0 * std::f64::consts::PI * n as f64 / (len - 1) as f64).cos();
        *v = sinc * w;
    }
    let sum: f64 = h.iter().sum();
    let h: Vec<f32> = h.iter().map(|v| (v / sum) as f32).collect();

    let out_len = x.len().div_ceil(factor);
    let mut out = Vec::with_capacity(out_len);
    for k in 0..out_len {
        let centre = (k * factor) as isize;
        let first = centre - half as isize;
        let mut acc = 0.0f32;
        if first >= 0 && (first as usize + len) <= x.len() {
            let seg = &x[first as usize..first as usize + len];
            for (a, b) in seg.iter().zip(&h) {
                acc += a * b;
            }
        } else {
            for (j, b) in h.iter().enumerate() {
                let i = first + j as isize;
                if i >= 0 && (i as usize) < x.len() {
                    acc += x[i as usize] * b;
                }
            }
        }
        out.push(acc);
    }
    (out, sr / factor as f64)
}

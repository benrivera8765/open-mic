// SPDX-License-Identifier: GPL-3.0-or-later
//! Measures how far "shift each pitched stem, then mix" is from "mix, then shift" (the
//! real-time graph) on a real song: the evidence for the prerender cache design
//! (DECISIONS: key-change render-ahead cache). Usage:
//! `cargo run --release -p jk-engine --example keycache_linearity <song.jksong> [start_s] [len_s] [semitones]`
use jk_dsp::{render, MixParams, RenderOptions, StemsRef};
use jk_engine::source::{slot_buffers, SongSource};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let start_s: f64 = a.get(2).map_or(60.0, |s| s.parse().unwrap());
    let len_s: f64 = a.get(3).map_or(20.0, |s| s.parse().unwrap());
    let key: i32 = a.get(4).map_or(2, |s| s.parse().unwrap());
    let mut src = SongSource::open(&path).unwrap();
    let rate = src.info().sample_rate;
    let n = (len_s * rate as f64) as usize;
    src.seek((start_s * rate as f64) as u64).unwrap();
    let mut bufs = slot_buffers(n);
    src.read(&mut bufs, n).unwrap();
    let stems = StemsRef {
        sample_rate: rate,
        frames: n,
        stems: std::array::from_fn(|s| [&bufs[s][0][..], &bufs[s][1][..]]),
    };
    let opts = RenderOptions::native(rate);
    let base = MixParams { guide: 1.0, backing_on: true, backing: 1.0, other: 1.0, drums: 0.0, shift_drums: false, semitones: key };
    let t = std::time::Instant::now();
    let mix = render(stems, base, &[], &opts).unwrap();
    let t_mix = t.elapsed().as_secs_f64();
    let only = |g: [f32; 3]| MixParams { guide: g[0], backing: g[1], other: g[2], ..base };
    let t = std::time::Instant::now();
    let parts = [only([1.0, 0.0, 0.0]), only([0.0, 1.0, 0.0]), only([0.0, 0.0, 1.0])]
        .map(|p| render(stems, p, &[], &opts).unwrap());
    let t_parts = t.elapsed().as_secs_f64();
    let mut se = 0.0f64;
    let mut ss = 0.0f64;
    let mut mx = 0.0f32;
    for c in 0..2 {
        for i in 0..n {
            let sum = (parts[0].channels[c][i] + parts[1].channels[c][i]) + parts[2].channels[c][i];
            let d = sum - mix.channels[c][i];
            mx = mx.max(d.abs());
            se += (d as f64).powi(2);
            ss += (mix.channels[c][i] as f64).powi(2);
        }
    }
    println!(
        "key {key:+}: {len_s} s from {start_s} s @ {rate} Hz: residual (sum of per-stem shifts - shift of mix): max |d| {mx:.4}, RMS {:.1} dB rel. signal; mix render {:.1}x RT, per-stem {:.1}x RT",
        10.0 * (se / ss).log10(),
        len_s / t_mix,
        len_s / t_parts
    );
}

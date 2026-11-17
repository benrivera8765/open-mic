// SPDX-License-Identifier: GPL-3.0-or-later
//! FLAC decoding is bit-exact (24-bit -> f32) and seeking is sample-accurate.

use jk_engine::FlacStem;
use std::path::PathBuf;

fn write_flac(path: &PathBuf, samples: &[i32], channels: usize, rate: u32) -> usize {
    let bytes = jk_engine::encode_flac(samples, channels, 24, rate);
    std::fs::write(path, &bytes).unwrap();
    bytes.len()
}

/// Deterministic 24-bit test signal: noise + a sine, interleaved.
fn signal(frames: usize, channels: usize) -> Vec<i32> {
    let mut s = 12345u32;
    let mut v = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for c in 0..channels {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let noise = ((s >> 8) as i32 - (1 << 23)) / 8;
            let tone = ((i as f64 * 0.01 * (c + 1) as f64).sin() * 3_000_000.0) as i32;
            v.push((noise + tone).clamp(-8_388_608, 8_388_607));
        }
    }
    v
}

#[test]
fn decode_exact_and_seek_sample_accurate() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-engine-flac");
    std::fs::create_dir_all(&dir).unwrap();
    for &(channels, frames) in &[(2usize, 300_000usize), (1, 123_457)] {
        let path = dir.join(format!("t{channels}.flac"));
        let pcm = signal(frames, channels);
        let size = write_flac(&path, &pcm, channels, 44_100);

        let (rate, all) = FlacStem::decode_all(&path).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(all[0].len(), frames, "decoded length");
        // Bit-exact: every decoded sample equals the source integer / 2^23.
        for i in 0..frames {
            for c in 0..2 {
                let src = pcm[i * channels + c.min(channels - 1)] as f32 / 8_388_608.0;
                assert_eq!(all[c][i], src, "sample {i} ch {c}");
            }
        }
        // Seek to many positions (block starts, block interiors, last frames) and compare.
        let mut stem = FlacStem::open(&path).unwrap();
        let mut buf = [vec![0.0f32; 5000], vec![0.0f32; 5000]];
        let mut positions: Vec<u64> = vec![0, 1, 4095, 4096, 4097, 65_535, 100_000, frames as u64 - 5000, frames as u64 - 1];
        let mut s = 99u64;
        for _ in 0..40 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            positions.push((s >> 33) % frames as u64);
        }
        let mut checked = 0;
        for &p in &positions {
            stem.seek(p).unwrap();
            let n = stem.read(&mut buf, 0, 5000).unwrap();
            let expect = (frames as u64 - p).min(5000) as usize;
            assert_eq!(n, expect, "frames read after seek to {p}");
            for i in 0..n {
                for c in 0..2 {
                    assert_eq!(buf[c][i], all[c][p as usize + i], "seek to {p}: frame {i} ch {c}");
                }
            }
            checked += 1;
        }
        println!(
            "{channels} ch, {frames} frames, 24-bit FLAC {size} bytes ({:.1} bits/sample): decode bit-exact; {checked} seeks sample-accurate (5000 frames compared after each)",
            size as f64 * 8.0 / (frames * channels) as f64
        );
    }
}

#[test]
fn encoder_edge_cases_round_trip() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-engine-flac");
    std::fs::create_dir_all(&dir).unwrap();
    // (bits, frames, generator): silence (constant subframes), a short file (< one block),
    // full-scale extremes, a quiet 16-bit sine.
    type Gen = Box<dyn Fn(usize, usize) -> i32>;
    let cases: Vec<(u32, usize, Gen)> = vec![
        (24, 10_000, Box::new(|_, _| 0)),
        (24, 100, Box::new(|i, c| (i as i32 * 1000 + c as i32) % 8_000_000)),
        (24, 9_000, Box::new(|i, _| if i % 2 == 0 { 8_388_607 } else { -8_388_608 })),
        (16, 50_000, Box::new(|i, c| ((i as f64 * 0.05 + c as f64).sin() * 300.0) as i32)),
    ];
    for (k, (bits, frames, f)) in cases.iter().enumerate() {
        let pcm: Vec<i32> = (0..*frames).flat_map(|i| [f(i, 0), f(i, 1)]).collect();
        let path = dir.join(format!("edge{k}.flac"));
        std::fs::write(&path, jk_engine::encode_flac(&pcm, 2, *bits, 48_000)).unwrap();
        let (rate, all) = FlacStem::decode_all(&path).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(all[0].len(), *frames);
        let scale = (1u32 << (bits - 1)) as f32;
        for i in 0..*frames {
            for c in 0..2 {
                assert_eq!(all[c][i], pcm[i * 2 + c] as f32 / scale, "case {k} frame {i}");
            }
        }
    }
}

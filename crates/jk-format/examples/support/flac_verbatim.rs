//! A tiny, independent FLAC *encoder* for tests: 24-bit, VERBATIM subframes only (no
//! compression), STREAMINFO with total samples and MD5, and a SEEKTABLE.
//!
//! It exists so the Rust side can write a song file from scratch without depending on
//! libFLAC, and so the Rust -> Python conformance direction exercises a FLAC stream that
//! libsndfile/libFLAC did not produce. Uncompressed, so do not use it for real songs.
//! Shared by `examples/jksong-write-test.rs` and the integration tests via `#[path]`.

#![allow(dead_code)]

use md5::{Digest, Md5};

pub const BLOCK: usize = 4096;

/// CRC-8, polynomial 0x07, init 0 (FLAC frame header).
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

/// CRC-16, polynomial 0x8005, init 0 (FLAC frame footer).
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

/// FLAC's UTF-8-like coding of the frame number.
fn utf8_number(n: u64, out: &mut Vec<u8>) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    let mut bytes = Vec::new();
    let mut v = n;
    // Continuation bytes carry 6 bits each.
    let mut len = 2;
    while v >= (1u64 << (5 * len + 1)) {
        len += 1;
    }
    for _ in 1..len {
        bytes.push(0x80 | (v & 0x3f) as u8);
        v >>= 6;
    }
    let lead_mask: u8 = !(0xffu8 >> len); // len leading ones
    out.push(lead_mask | v as u8);
    out.extend(bytes.iter().rev());
}

fn rate_code(rate: u32) -> u8 {
    match rate {
        88_200 => 0b0001,
        176_400 => 0b0010,
        192_000 => 0b0011,
        8_000 => 0b0100,
        16_000 => 0b0101,
        22_050 => 0b0110,
        24_000 => 0b0111,
        32_000 => 0b1000,
        44_100 => 0b1001,
        48_000 => 0b1010,
        96_000 => 0b1011,
        _ => 0b0000, // take it from STREAMINFO
    }
}

/// Encodes interleaved 24-bit integer samples (-8388608..=8388607) as a FLAC stream.
/// `seek_every` adds a SEEKTABLE point at the first frame of every `seek_every` frames.
pub fn encode(samples: &[i32], channels: usize, sample_rate: u32, seek_every: u64) -> Vec<u8> {
    assert!((1..=8).contains(&channels) && samples.len() % channels == 0);
    let total = (samples.len() / channels) as u64;

    // MD5 over the samples, interleaved, 3 bytes little-endian each.
    let mut md5 = Md5::new();
    let mut le = Vec::with_capacity(samples.len() * 3);
    for &s in samples {
        assert!((-8_388_608..=8_388_607).contains(&s), "sample {s} is not 24-bit");
        le.extend_from_slice(&s.to_le_bytes()[..3]);
    }
    md5.update(&le);
    let digest: [u8; 16] = md5.finalize().into();

    // Frames.
    let mut frames = Vec::new();
    let mut seek = Vec::new(); // (sample, offset from first frame, frame samples)
    for (fi, start) in (0..total as usize).step_by(BLOCK).enumerate() {
        let n = BLOCK.min(total as usize - start);
        if seek_every > 0 && (start as u64) % seek_every < BLOCK as u64 {
            seek.push((start as u64, frames.len() as u64, n as u16));
        }
        let frame_start = frames.len();
        frames.extend_from_slice(&[0xFF, 0xF8]);
        frames.push((0b0111 << 4) | rate_code(sample_rate)); // 16-bit block size at end
        frames.push((((channels - 1) as u8) << 4) | (0b110 << 1)); // independent, 24-bit
        utf8_number(fi as u64, &mut frames);
        frames.extend_from_slice(&((n - 1) as u16).to_be_bytes());
        let c8 = crc8(&frames[frame_start..]);
        frames.push(c8);
        for c in 0..channels {
            frames.push(0x02); // VERBATIM, no wasted bits
            for f in 0..n {
                let s = samples[(start + f) * channels + c];
                frames.extend_from_slice(&s.to_be_bytes()[1..]);
            }
        }
        let c16 = crc16(&frames[frame_start..]);
        frames.extend_from_slice(&c16.to_be_bytes());
    }

    // Metadata: STREAMINFO, SEEKTABLE (last).
    let mut out = Vec::with_capacity(frames.len() + 128);
    out.extend_from_slice(b"fLaC");
    out.push(0x00); // STREAMINFO, not last
    out.extend_from_slice(&34u32.to_be_bytes()[1..]);
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // min/max frame size unknown
    let packed: u64 = ((sample_rate as u64) << 44) | (((channels - 1) as u64) << 41) | (23u64 << 36) | (total & 0xF_FFFF_FFFF);
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&digest);
    out.push(0x80 | 3); // SEEKTABLE, last
    out.extend_from_slice(&((seek.len() * 18) as u32).to_be_bytes()[1..]);
    for (s, o, n) in seek {
        out.extend_from_slice(&s.to_be_bytes());
        out.extend_from_slice(&o.to_be_bytes());
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&frames);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn utf8_numbers() {
        let enc = |n| {
            let mut v = Vec::new();
            super::utf8_number(n, &mut v);
            v
        };
        assert_eq!(enc(0x7f), vec![0x7f]);
        assert_eq!(enc(0x80), vec![0xC2, 0x80]);
        assert_eq!(enc(0x7ff), vec![0xDF, 0xBF]);
        assert_eq!(enc(0x800), vec![0xE0, 0xA0, 0x80]);
        assert_eq!(enc(0xFFFF), vec![0xEF, 0xBF, 0xBF]);
        assert_eq!(enc(0x10000), vec![0xF0, 0x90, 0x80, 0x80]);
    }
}

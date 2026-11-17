// SPDX-License-Identifier: GPL-3.0-or-later
//! A small, dependable FLAC **encoder** (fixed predictors + partitioned Rice coding).
//!
//! Why our own: the pure-Rust `flacenc` 0.5.1 caps the Rice parameter at 14 (it does not
//! use the 5-bit RICE2 method), so loud or noisy 24-bit audio produced frames of ~100 bits
//! per sample (a 90 s stereo mix came out at 24 GB) that symphonia then refused to read.
//! This encoder writes standard FLAC (FIXED subframes order 0..4, RICE2 for >16-bit audio,
//! VERBATIM fallback), is lossless by construction and is checked by decoding with
//! symphonia in the tests. It is used for test fixtures and lossless exports; it is not
//! tuned for the best compression ratio.
//!
//! Limits: 1..=8 channels coded independently, 8..=24 bits per sample, fixed block size
//! 4096, MD5 field left as zero ("not computed", allowed by the format).

/// Bit writer, MSB first.
struct Bits {
    buf: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { buf: Vec::new(), acc: 0, n: 0 }
    }
    #[inline]
    fn put(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (value & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
    }
    fn put_signed(&mut self, v: i64, bits: u32) {
        self.put(v as u64, bits);
    }
    /// `count` zero bits then a one bit.
    fn unary(&mut self, mut count: u64) {
        while count >= 32 {
            self.put(0, 32);
            count -= 32;
        }
        self.put(1, count as u32 + 1);
    }
    fn align(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
    }
    fn len_bytes(&self) -> usize {
        self.buf.len()
    }
}

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
fn utf8_number(v: u64, out: &mut Vec<u8>) {
    if v < 0x80 {
        out.push(v as u8);
        return;
    }
    let mut bytes = 2;
    while v >= 1u64 << (5 * bytes + 1) {
        bytes += 1;
    }
    let first_bits = 7 - bytes;
    let lead: u8 = !(0xFFu8 >> bytes);
    out.push(lead | ((v >> (6 * (bytes - 1))) as u8 & ((1u8 << first_bits) - 1)));
    for i in (0..bytes - 1).rev() {
        out.push(0x80 | ((v >> (6 * i)) & 0x3F) as u8);
    }
}

fn fixed_residuals(x: &[i32], order: usize) -> Vec<i64> {
    let n = x.len();
    let mut r = Vec::with_capacity(n.saturating_sub(order));
    for i in order..n {
        let v = |k: usize| x[i - k] as i64;
        r.push(match order {
            0 => v(0),
            1 => v(0) - v(1),
            2 => v(0) - 2 * v(1) + v(2),
            3 => v(0) - 3 * v(1) + 3 * v(2) - v(3),
            _ => v(0) - 4 * v(1) + 6 * v(2) - 4 * v(3) + v(4),
        });
    }
    r
}

#[inline]
fn zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// Best Rice parameter and its cost in bits for a slice of zigzagged residuals.
fn best_rice(u: &[u64], max_k: u32) -> (u32, u64) {
    let sum: u64 = u.iter().sum();
    let mean = if u.is_empty() { 0 } else { sum / u.len() as u64 };
    let k0 = if mean == 0 { 0 } else { 63 - mean.leading_zeros() };
    let mut best = (0u32, u64::MAX);
    for k in k0.saturating_sub(1)..=(k0 + 1).min(max_k) {
        let bits: u64 = u.iter().map(|&v| (v >> k) + 1 + k as u64).sum();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// Residual coding plan: (partition order, per-partition parameters, total bits incl. headers).
fn plan_residual(u: &[u64], block: usize, order: usize, param_bits: u32) -> (u32, Vec<u32>, u64) {
    let max_k = if param_bits == 4 { 14 } else { 30 };
    let mut best: (u32, Vec<u32>, u64) = (0, Vec::new(), u64::MAX);
    for p in 0..=8u32 {
        let parts = 1usize << p;
        if block % parts != 0 || block / parts <= order {
            break;
        }
        let psize = block / parts;
        let mut ks = Vec::with_capacity(parts);
        let mut bits = 2 + 4u64;
        let mut start = 0usize;
        for i in 0..parts {
            let len = if i == 0 { psize - order } else { psize };
            let (k, b) = best_rice(&u[start..start + len], max_k);
            ks.push(k);
            bits += param_bits as u64 + b;
            start += len;
        }
        if bits < best.2 {
            best = (p, ks, bits);
        }
    }
    best
}

fn encode_subframe(w: &mut Bits, x: &[i32], bps: u32) {
    let block = x.len();
    // Constant block (e.g. digital silence).
    if x.iter().all(|&v| v == x[0]) {
        w.put(0, 1);
        w.put(0b000000, 6);
        w.put(0, 1);
        w.put_signed(x[0] as i64, bps);
        return;
    }
    let param_bits = if bps > 16 { 5 } else { 4 };
    // (predictor order, zigzagged residuals, residual plan)
    type Candidate = (usize, Vec<u64>, (u32, Vec<u32>, u64));
    let mut best: Option<Candidate> = None;
    for order in 0..=4usize.min(block.saturating_sub(1)) {
        let u: Vec<u64> = fixed_residuals(x, order).into_iter().map(zigzag).collect();
        let plan = plan_residual(&u, block, order, param_bits);
        let total = plan.2 + order as u64 * bps as u64;
        if best.as_ref().is_none_or(|b| total < b.2 .2 + b.0 as u64 * bps as u64) {
            best = Some((order, u, plan));
        }
    }
    let (order, u, (p, ks, bits)) = best.unwrap();
    let verbatim_bits = block as u64 * bps as u64;
    if bits + order as u64 * bps as u64 >= verbatim_bits {
        w.put(0, 1);
        w.put(0b000001, 6);
        w.put(0, 1);
        for &v in x {
            w.put_signed(v as i64, bps);
        }
        return;
    }
    w.put(0, 1);
    w.put(0b001000 | order as u64, 6);
    w.put(0, 1);
    for &v in &x[..order] {
        w.put_signed(v as i64, bps);
    }
    w.put(if param_bits == 4 { 0 } else { 1 }, 2);
    w.put(p as u64, 4);
    let parts = 1usize << p;
    let psize = block / parts;
    let mut start = 0usize;
    for (i, &k) in ks.iter().enumerate() {
        let len = if i == 0 { psize - order } else { psize };
        w.put(k as u64, param_bits);
        for &v in &u[start..start + len] {
            w.unary(v >> k);
            w.put(v & ((1u64 << k) - 1), k);
        }
        start += len;
    }
}

/// Encode interleaved integer samples (`frames * channels`, each within `bits` bits signed)
/// as a complete FLAC file.
pub fn encode_flac(samples: &[i32], channels: usize, bits: u32, sample_rate: u32) -> Vec<u8> {
    assert!((1..=8).contains(&channels));
    assert!((8..=24).contains(&bits));
    assert!(sample_rate > 0 && sample_rate < (1 << 20));
    const BLOCK: usize = 4096;
    let frames = samples.len() / channels;
    let mut body = Vec::new();
    let (mut min_fs, mut max_fs) = (u32::MAX, 0u32);
    let mut chan = vec![0i32; BLOCK];
    let rate_code: u8 = match sample_rate {
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
        _ => 0b0000, // from STREAMINFO
    };
    let size_code: u8 = match bits {
        8 => 0b001,
        12 => 0b010,
        16 => 0b100,
        20 => 0b101,
        24 => 0b110,
        _ => 0b000,
    };
    for (fi, start) in (0..frames).step_by(BLOCK).enumerate() {
        let n = BLOCK.min(frames - start);
        let mut hdr = vec![0xFF, 0xF8];
        let bs_code: u8 = if n == BLOCK { 0b1100 } else { 0b0111 };
        hdr.push((bs_code << 4) | rate_code);
        hdr.push(((channels as u8 - 1) << 4) | (size_code << 1));
        utf8_number(fi as u64, &mut hdr);
        if bs_code == 0b0111 {
            hdr.extend_from_slice(&((n - 1) as u16).to_be_bytes());
        }
        hdr.push(crc8(&hdr));
        let mut w = Bits::new();
        for c in 0..channels {
            for i in 0..n {
                chan[i] = samples[(start + i) * channels + c];
            }
            encode_subframe(&mut w, &chan[..n], bits);
        }
        w.align();
        let mut frame = hdr;
        frame.extend_from_slice(&w.buf);
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        min_fs = min_fs.min(frame.len() as u32);
        max_fs = max_fs.max(frame.len() as u32);
        body.extend_from_slice(&frame);
    }
    // STREAMINFO.
    let mut si = Bits::new();
    // A file shorter than one block is a single block of `frames` samples.
    let (min_block, max_block) = if frames >= BLOCK { (BLOCK, BLOCK) } else { (frames.max(16), frames.max(16)) };
    si.put(min_block as u64, 16);
    si.put(max_block as u64, 16);
    si.put(if min_fs == u32::MAX { 0 } else { min_fs as u64 }, 24);
    si.put(max_fs as u64, 24);
    si.put(sample_rate as u64, 20);
    si.put(channels as u64 - 1, 3);
    si.put(bits as u64 - 1, 5);
    si.put((frames as u64 >> 32) & 0xF, 4);
    si.put(frames as u64 & 0xFFFF_FFFF, 32);
    for _ in 0..4 {
        si.put(0, 32); // MD5 not computed
    }
    debug_assert_eq!(si.len_bytes(), 34);
    let mut out = Vec::with_capacity(body.len() + 42);
    out.extend_from_slice(b"fLaC");
    out.push(0x80); // last metadata block, type 0 = STREAMINFO
    out.extend_from_slice(&[0, 0, 34]);
    out.extend_from_slice(&si.buf);
    out.extend_from_slice(&body);
    out
}

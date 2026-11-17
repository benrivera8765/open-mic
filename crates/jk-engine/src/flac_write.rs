// SPDX-License-Identifier: GPL-3.0-or-later
//! A small, dependable, **streaming** FLAC encoder (fixed predictors + partitioned Rice
//! coding), used for song-file stems made by our tools (fixtures, the demo song) and for
//! "Export backing track" (24-bit FLAC).
//!
//! Why our own: the pure-Rust `flacenc` 0.5.1 caps the Rice parameter at 14 (it does not
//! use the 5-bit RICE2 method), so loud or noisy 24-bit audio produced frames of ~100 bits
//! per sample (a 90 s stereo mix came out at 24 GB) that symphonia then refused to read.
//! This encoder writes standard FLAC and is lossless by construction; the tests decode
//! its output with symphonia and jk-format's verifier checks it. It is not tuned for the
//! best compression ratio.
//!
//! What it writes:
//! * STREAMINFO with the total sample count and the **MD5** of the audio (computed while
//!   encoding; SONG_FORMAT 4.2 requires it).
//! * A **SEEKTABLE** with a point every second (space reserved up front from the expected
//!   length; unused points are standard placeholders).
//! * Frames of 4096 samples (the last one shorter), channels coded independently:
//!   CONSTANT (digital silence), FIXED order 0..4 or VERBATIM, whichever is smallest;
//!   Rice parameters 4-bit (RICE) or 5-bit (RICE2) as needed; "wasted bits" when every
//!   sample of a block has trailing zero bits (e.g. 16-bit material on the 24-bit grid).
//!
//! Limits: 1..=8 channels, 8..=24 bits per sample, sample rate < 2^20 Hz.

use std::io::{self, Cursor, Seek, SeekFrom, Write};

use md5::{Digest, Md5};

/// Samples per FLAC frame.
pub const BLOCK: usize = 4096;
const STREAMINFO_LEN: usize = 34;
const SEEKPOINT_LEN: usize = 18;

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
    fn clear(&mut self) {
        self.buf.clear();
        self.acc = 0;
        self.n = 0;
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

fn fixed_residuals(x: &[i32], order: usize, r: &mut Vec<u64>) {
    r.clear();
    for i in order..x.len() {
        let v = |k: usize| x[i - k] as i64;
        let res = match order {
            0 => v(0),
            1 => v(0) - v(1),
            2 => v(0) - 2 * v(1) + v(2),
            3 => v(0) - 3 * v(1) + 3 * v(2) - v(3),
            _ => v(0) - 4 * v(1) + 6 * v(2) - 4 * v(3) + v(4),
        };
        r.push(zigzag(res));
    }
}

#[inline]
fn zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// Best Rice parameter (`<= max_k`) and its cost in bits for zigzagged residuals.
fn best_rice(u: &[u64], max_k: u32) -> (u32, u64) {
    let sum: u64 = u.iter().sum();
    let mean = if u.is_empty() { 0 } else { sum / u.len() as u64 };
    let k0 = if mean == 0 { 0 } else { 63 - mean.leading_zeros() };
    let mut best = (0u32, u64::MAX);
    for k in k0.saturating_sub(1).min(max_k)..=(k0 + 1).min(max_k) {
        let bits: u64 = u.iter().map(|&v| (v >> k) + 1 + k as u64).sum();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// Residual coding plan.
struct Plan {
    order: usize,
    partition_order: u32,
    params: Vec<u32>,
    /// 4 (RICE) or 5 (RICE2) bits per parameter.
    param_bits: u32,
    /// Total residual bits, headers included.
    bits: u64,
}

/// Plan the residual for one predictor order. RICE2 (5-bit parameters, up to 30) is used
/// only when some partition needs a parameter above 14 (4-bit RICE's limit; 15 is the
/// escape code).
fn plan_residual(u: &[u64], block: usize, order: usize) -> Plan {
    let mut best = Plan { order, partition_order: 0, params: Vec::new(), param_bits: 5, bits: u64::MAX };
    for p in 0..=8u32 {
        let parts = 1usize << p;
        if !block.is_multiple_of(parts) || block / parts <= order {
            break;
        }
        let psize = block / parts;
        let mut ks = Vec::with_capacity(parts);
        let mut data_bits = 0u64;
        let mut start = 0usize;
        for i in 0..parts {
            let len = if i == 0 { psize - order } else { psize };
            let (k, b) = best_rice(&u[start..start + len], 30);
            ks.push(k);
            data_bits += b;
            start += len;
        }
        let param_bits = if ks.iter().all(|&k| k <= 14) { 4 } else { 5 };
        let bits = 2 + 4 + parts as u64 * param_bits as u64 + data_bits;
        if bits < best.bits {
            best = Plan { order, partition_order: p, params: ks, param_bits, bits };
        }
    }
    best
}

/// Encode one channel of one block into `w` with `bps` bits per sample.
fn encode_subframe(w: &mut Bits, x: &[i32], bps: u32, shifted: &mut Vec<i32>, res: &mut Vec<u64>) {
    let block = x.len();
    // Constant block (e.g. digital silence).
    if x.iter().all(|&v| v == x[0]) {
        w.put(0, 1);
        w.put(0b000000, 6);
        w.put(0, 1);
        w.put_signed(x[0] as i64, bps);
        return;
    }
    // Wasted bits: every sample has `wasted` trailing zero bits (e.g. 16-bit audio on
    // the 24-bit grid); code the samples shifted right and say so in the header.
    let or = x.iter().fold(0i32, |a, &v| a | v);
    let wasted = (or.trailing_zeros()).min(bps - 1);
    let (data, ebps): (&[i32], u32) = if wasted > 0 {
        shifted.clear();
        shifted.extend(x.iter().map(|&v| v >> wasted));
        (&shifted[..], bps - wasted)
    } else {
        (x, bps)
    };
    let header = |w: &mut Bits, ty: u64| {
        w.put(0, 1);
        w.put(ty, 6);
        if wasted > 0 {
            w.put(1, 1);
            w.unary((wasted - 1) as u64);
        } else {
            w.put(0, 1);
        }
    };
    let mut best: Option<Plan> = None;
    for order in 0..=4usize.min(block.saturating_sub(1)) {
        fixed_residuals(data, order, res);
        let plan = plan_residual(res, block, order);
        let total = plan.bits + order as u64 * ebps as u64;
        if best.as_ref().is_none_or(|b| total < b.bits + b.order as u64 * ebps as u64) {
            best = Some(plan);
        }
    }
    let plan = best.expect("block of at least one sample");
    let verbatim_bits = block as u64 * ebps as u64;
    if plan.bits + plan.order as u64 * ebps as u64 >= verbatim_bits {
        header(w, 0b000001);
        for &v in data {
            w.put_signed(v as i64, ebps);
        }
        return;
    }
    let order = plan.order;
    fixed_residuals(data, order, res);
    header(w, 0b001000 | order as u64);
    for &v in &data[..order] {
        w.put_signed(v as i64, ebps);
    }
    w.put(if plan.param_bits == 4 { 0 } else { 1 }, 2);
    w.put(plan.partition_order as u64, 4);
    let parts = 1usize << plan.partition_order;
    let psize = block / parts;
    let mut start = 0usize;
    for (i, &k) in plan.params.iter().enumerate() {
        let len = if i == 0 { psize - order } else { psize };
        w.put(k as u64, plan.param_bits);
        for &v in &res[start..start + len] {
            w.unary(v >> k);
            w.put(v & ((1u64 << k) - 1), k);
        }
        start += len;
    }
}

/// Summary of a finished stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlacSummary {
    pub frames: u64,
    pub md5: [u8; 16],
    /// Bytes written, metadata included.
    pub bytes: u64,
    /// Seek points written (not counting placeholders).
    pub seek_points: usize,
}

/// Streaming FLAC encoder writing to any `Write + Seek` (a file, a `Cursor<Vec<u8>>`).
/// The metadata blocks are written as placeholders first and filled in by
/// [`finish`](Self::finish).
pub struct FlacEncoder<W: Write + Seek> {
    w: W,
    channels: usize,
    bits: u32,
    sample_rate: u32,
    /// Interleaved samples waiting for a full block.
    pending: Vec<i32>,
    frame_index: u64,
    total_frames: u64,
    min_fs: u32,
    max_fs: u32,
    md5: Md5,
    md5_buf: Vec<u8>,
    /// Stream position of the `fLaC` marker.
    start_pos: u64,
    /// Bytes of audio frames written so far (seek point offsets are relative to the
    /// first frame).
    audio_bytes: u64,
    seek_slots: usize,
    seek_interval: u64,
    seek_points: Vec<(u64, u64, u16)>,
    // Scratch, reused for every block.
    chan: Vec<i32>,
    shifted: Vec<i32>,
    res: Vec<u64>,
    bitbuf: Bits,
    frame: Vec<u8>,
}

impl<W: Write + Seek> FlacEncoder<W> {
    /// Start a stream. `expected_frames` sizes the seek table (a point every second);
    /// `None` reserves points every 10 s for up to two hours.
    pub fn new(mut w: W, channels: usize, bits: u32, sample_rate: u32, expected_frames: Option<u64>) -> io::Result<Self> {
        if !(1..=8).contains(&channels) || !(8..=24).contains(&bits) || sample_rate == 0 || sample_rate >= 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported FLAC parameters"));
        }
        let (seek_interval, seek_slots) = match expected_frames {
            Some(n) => (sample_rate as u64, n.div_ceil(sample_rate as u64).max(1) as usize),
            None => (10 * sample_rate as u64, 720),
        };
        let start_pos = w.stream_position()?;
        let mut enc = FlacEncoder {
            w,
            channels,
            bits,
            sample_rate,
            pending: Vec::with_capacity(BLOCK * channels),
            frame_index: 0,
            total_frames: 0,
            min_fs: u32::MAX,
            max_fs: 0,
            md5: Md5::new(),
            md5_buf: Vec::with_capacity(BLOCK * channels * 3),
            start_pos,
            audio_bytes: 0,
            seek_slots,
            seek_interval,
            seek_points: Vec::with_capacity(seek_slots),
            chan: vec![0; BLOCK],
            shifted: Vec::with_capacity(BLOCK),
            res: Vec::with_capacity(BLOCK),
            bitbuf: Bits::new(),
            frame: Vec::with_capacity(BLOCK * channels * 4),
        };
        let meta = enc.metadata_bytes(None);
        enc.w.write_all(&meta)?;
        Ok(enc)
    }

    fn metadata_bytes(&self, finished: Option<[u8; 16]>) -> Vec<u8> {
        let frames = self.total_frames;
        let mut si = Bits::new();
        // A stream shorter than one block is a single block of `frames` samples.
        let (min_block, max_block) =
            if frames >= BLOCK as u64 { (BLOCK, BLOCK) } else { ((frames as usize).max(16), (frames as usize).max(16)) };
        si.put(min_block as u64, 16);
        si.put(max_block as u64, 16);
        si.put(if self.min_fs == u32::MAX { 0 } else { self.min_fs as u64 }, 24);
        si.put(self.max_fs as u64, 24);
        si.put(self.sample_rate as u64, 20);
        si.put(self.channels as u64 - 1, 3);
        si.put(self.bits as u64 - 1, 5);
        si.put((frames >> 32) & 0xF, 4);
        si.put(frames & 0xFFFF_FFFF, 32);
        let md5 = finished.unwrap_or([0; 16]);
        for b in md5 {
            si.put(b as u64, 8);
        }
        debug_assert_eq!(si.buf.len(), STREAMINFO_LEN);
        let mut out = Vec::with_capacity(4 + 4 + STREAMINFO_LEN + 4 + self.seek_slots * SEEKPOINT_LEN);
        out.extend_from_slice(b"fLaC");
        let last = self.seek_slots == 0;
        out.push(if last { 0x80 } else { 0x00 }); // type 0 = STREAMINFO
        out.extend_from_slice(&(STREAMINFO_LEN as u32).to_be_bytes()[1..]);
        out.extend_from_slice(&si.buf);
        if self.seek_slots > 0 {
            out.push(0x80 | 3); // last block, type 3 = SEEKTABLE
            out.extend_from_slice(&((self.seek_slots * SEEKPOINT_LEN) as u32).to_be_bytes()[1..]);
            for i in 0..self.seek_slots {
                match self.seek_points.get(i) {
                    Some(&(sample, offset, n)) => {
                        out.extend_from_slice(&sample.to_be_bytes());
                        out.extend_from_slice(&offset.to_be_bytes());
                        out.extend_from_slice(&n.to_be_bytes());
                    }
                    None => {
                        // Placeholder point.
                        out.extend_from_slice(&u64::MAX.to_be_bytes());
                        out.extend_from_slice(&[0u8; 10]);
                    }
                }
            }
        }
        out
    }

    /// Append interleaved samples (whole frames, each within `bits` bits signed).
    pub fn write_interleaved(&mut self, samples: &[i32]) -> io::Result<()> {
        debug_assert_eq!(samples.len() % self.channels, 0);
        let lim = 1i64 << (self.bits - 1);
        if samples.iter().any(|&s| (s as i64) < -lim || (s as i64) >= lim) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "sample outside the declared bit depth"));
        }
        let mut rest = samples;
        let full = BLOCK * self.channels;
        while !rest.is_empty() {
            let take = (full - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == full {
                self.flush_block()?;
            }
        }
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        let ch = self.channels;
        let n = self.pending.len() / ch;
        if n == 0 {
            return Ok(());
        }
        // MD5 of the samples as signed little-endian, ceil(bits/8) bytes each, interleaved.
        let bytes_per = self.bits.div_ceil(8) as usize;
        self.md5_buf.clear();
        for &s in &self.pending {
            self.md5_buf.extend_from_slice(&s.to_le_bytes()[..bytes_per]);
        }
        self.md5.update(&self.md5_buf);

        // Seek point at the first block starting at or after each interval.
        let first_sample = self.total_frames;
        let next_point = self.seek_points.len() as u64 * self.seek_interval;
        if self.seek_points.len() < self.seek_slots && first_sample >= next_point {
            self.seek_points.push((first_sample, self.audio_bytes, n as u16));
        }

        let rate_code: u8 = match self.sample_rate {
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
        let size_code: u8 = match self.bits {
            8 => 0b001,
            12 => 0b010,
            16 => 0b100,
            20 => 0b101,
            24 => 0b110,
            _ => 0b000,
        };
        let frame = &mut self.frame;
        frame.clear();
        frame.extend_from_slice(&[0xFF, 0xF8]);
        let bs_code: u8 = if n == BLOCK { 0b1100 } else { 0b0111 };
        frame.push((bs_code << 4) | rate_code);
        frame.push(((ch as u8 - 1) << 4) | (size_code << 1));
        utf8_number(self.frame_index, frame);
        if bs_code == 0b0111 {
            frame.extend_from_slice(&((n - 1) as u16).to_be_bytes());
        }
        let c8 = crc8(frame);
        frame.push(c8);
        self.bitbuf.clear();
        for c in 0..ch {
            for i in 0..n {
                self.chan[i] = self.pending[i * ch + c];
            }
            encode_subframe(&mut self.bitbuf, &self.chan[..n], self.bits, &mut self.shifted, &mut self.res);
        }
        self.bitbuf.align();
        frame.extend_from_slice(&self.bitbuf.buf);
        let c16 = crc16(frame);
        frame.extend_from_slice(&c16.to_be_bytes());
        self.w.write_all(frame)?;
        self.min_fs = self.min_fs.min(frame.len() as u32);
        self.max_fs = self.max_fs.max(frame.len() as u32);
        self.audio_bytes += frame.len() as u64;
        self.frame_index += 1;
        self.total_frames += n as u64;
        self.pending.clear();
        Ok(())
    }

    /// Write the last (short) block, fill in STREAMINFO (length, MD5) and the seek
    /// table, and return the writer positioned at the end of the stream.
    pub fn finish(mut self) -> io::Result<(W, FlacSummary)> {
        self.flush_block()?;
        let md5: [u8; 16] = self.md5.clone().finalize().into();
        let meta = self.metadata_bytes(Some(md5));
        let end = self.w.stream_position()?;
        self.w.seek(SeekFrom::Start(self.start_pos))?;
        self.w.write_all(&meta)?;
        self.w.seek(SeekFrom::Start(end))?;
        self.w.flush()?;
        let summary = FlacSummary {
            frames: self.total_frames,
            md5,
            bytes: end - self.start_pos,
            seek_points: self.seek_points.len(),
        };
        Ok((self.w, summary))
    }
}

/// Encode interleaved integer samples (`frames * channels`, each within `bits` bits signed)
/// as a complete FLAC file in memory.
pub fn encode_flac(samples: &[i32], channels: usize, bits: u32, sample_rate: u32) -> Vec<u8> {
    let frames = (samples.len() / channels) as u64;
    let mut enc = FlacEncoder::new(Cursor::new(Vec::new()), channels, bits, sample_rate, Some(frames))
        .expect("valid FLAC parameters");
    enc.write_interleaved(samples).expect("in-memory write");
    enc.finish().expect("in-memory write").0.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_numbers() {
        let mut v = Vec::new();
        utf8_number(0x7F, &mut v);
        assert_eq!(v, [0x7F]);
        v.clear();
        utf8_number(0x80, &mut v);
        assert_eq!(v, [0xC2, 0x80]);
        v.clear();
        utf8_number(0x800, &mut v);
        assert_eq!(v, [0xE0, 0xA0, 0x80]);
    }
}

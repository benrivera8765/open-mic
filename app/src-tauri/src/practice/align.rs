// SPDX-License-Identifier: GPL-3.0-or-later
//! The echo reference, lined up with the microphone (docs/ARCHITECTURE.md 12.2 step 3,
//! 12.3 "Clocks").
//!
//! `jk_pitch::practice::Chain` wants, for every input frame, the frame of the app's own
//! output that left the DAC when that input frame was captured ("to the best knowledge"
//! of the two clocks), at the input's rate, handed `lead` early so a return that arrives
//! slightly before the estimate is still found. This module builds exactly that from the
//! engine's output tap (`jk_engine::TapReader`: stereo at the device rate, each piece
//! stamped with the host time its first frame leaves the DAC) and the input blocks'
//! host-clock stamps:
//!
//! - The tap is converted to the input rate when the two differ (the engine's own
//!   windowed-sinc resampler, `jk_dsp::OutputResampler`, fixed ratio) and kept in a ring of
//!   a few seconds with a time stamp per piece.
//! - The first input block is lined up once by the stamps; after that the reference is
//!   read continuously, frame after frame, so the slow drift between the two device clocks
//!   (WO2 trap 4) reaches the canceller as the steady slide it is built to follow
//!   (jk-pitch's echo `RefLine` reads at a fractional, drifting position). It is lined up
//!   again only after a discontinuity: frames lost on either side, a device or rate change,
//!   or the stamps saying the two have moved more than [`REALIGN_S`] apart.
//!
//! The two host clocks are the engine's (`jk_engine::now_ns`, `Instant` from a process
//! epoch) and the recorder's (QPC / mach host time): `offset_ns` turns the first into the
//! second; it is measured once when the session starts ([`clock_offset_ns`]).
//!
//! Runs on the practice analysis thread only. Allocates in [`RefAligner::new`] and when the
//! output rate changes; never on an audio thread.

use std::collections::VecDeque;

/// Seconds of reference kept (longer than the echo stage's 0.6 s search plus the lead,
/// the input ring's 2 s and any scheduling hiccup).
const RING_S: f64 = 4.0;
/// Stamps kept (one per tap piece; 10 ms pieces: 20 s).
const MAX_ANCHORS: usize = 2048;
/// The stamps and the running position may disagree by this much before the reference is
/// lined up again (a device switch, lost frames). Clock drift stays far below it over a
/// song (at most about 24 ms in four minutes).
pub const REALIGN_S: f64 = 0.2;
/// Tap frames per resampler call when the rates differ.
const CHUNK: usize = 480;
/// Pieces whose stamps the lineup line is fitted through (about 2 s of 10 ms pieces).
const FIT_PIECES: usize = 200;
/// A tap that wrote nothing for this long is idle (no output device running).
const STALE_TAP_NS: i64 = 250_000_000;

/// The engine's clock (`jk_engine::now_ns`) to the recorder's host clock: add this.
/// Measured from the tightest of 200 back-to-back readings.
pub fn clock_offset_ns() -> i64 {
    let mut best = (i64::MAX, 0i64);
    for _ in 0..200 {
        let a = jk_engine::now_ns() as i64;
        let h = jk_recorder::input::clock::host_time_ns();
        let b = jk_engine::now_ns() as i64;
        if b - a < best.0 {
            best = (b - a, h - (a + b) / 2);
        }
    }
    best.1
}

/// The tap converted to the input rate, with what it needs to do it.
struct Conv {
    tap_rate: u32,
    rs: Option<jk_dsp::OutputResampler>,
    /// Planar tap frames waiting for a full chunk.
    pend: [Vec<f32>; 2],
    /// Host time of the first pending frame.
    pend_ns: i64,
    /// The resampler's delay, in output (input-rate) frames, as nanoseconds.
    delay_ns: i64,
}

/// Lines the output tap up with the microphone. See the module documentation.
pub struct RefAligner {
    in_rate: u32,
    lead_ns: i64,
    offset_ns: i64,
    /// Interleaved stereo ring at the input rate.
    ring: Vec<f32>,
    cap: usize,
    /// Frames written into the ring since the start.
    written: u64,
    /// (frame index, host ns of that frame) per piece written.
    anchors: VecDeque<(u64, i64)>,
    conv: Option<Conv>,
    /// Next reference frame (fractional index into the ring stream) for the next input frame.
    cursor: Option<f64>,
    /// Host time the cursor stands for (for the drift check).
    realigned: u64,
    /// Frames of reference handed out that were not in the ring (zeros).
    pub missing: u64,
}

impl RefAligner {
    /// An aligner for an input at `in_rate` Hz; the reference is handed `lead_s` early.
    pub fn new(in_rate: u32, offset_ns: i64, lead_s: f64) -> Self {
        let cap = ((f64::from(in_rate.max(1)) * RING_S) as usize).next_power_of_two();
        RefAligner {
            in_rate: in_rate.max(1),
            lead_ns: (lead_s * 1e9).round() as i64,
            offset_ns,
            ring: vec![0.0; cap * 2],
            cap,
            written: 0,
            anchors: VecDeque::with_capacity(MAX_ANCHORS),
            conv: None,
            cursor: None,
            realigned: 0,
            missing: 0,
        }
    }

    /// How many times the reference was lined up (1 after the first block).
    pub fn realignments(&self) -> u64 {
        self.realigned
    }

    /// Forget the lineup (the next block is lined up by the stamps again).
    pub fn reset_lineup(&mut self) {
        self.cursor = None;
    }

    fn push_frames(&mut self, host_ns: i64, l: &[f32], r: &[f32]) {
        if l.is_empty() {
            return;
        }
        if self.anchors.len() >= MAX_ANCHORS {
            self.anchors.pop_front();
        }
        self.anchors.push_back((self.written, host_ns));
        let mask = self.cap - 1;
        for (j, (&a, &b)) in l.iter().zip(r).enumerate() {
            let k = ((self.written as usize) + j) & mask;
            self.ring[2 * k] = a;
            self.ring[2 * k + 1] = b;
        }
        self.written += l.len() as u64;
    }

    /// One piece of the tap: `frames` frames of interleaved stereo at `rate`, the first
    /// leaving the DAC at `dac_ns` (engine clock); `lost_before` frames were lost just
    /// before it.
    pub fn push_tap(&mut self, data: &[f32], frames: usize, rate: u32, dac_ns: u64, lost_before: u64) {
        if frames == 0 || rate == 0 {
            return;
        }
        let host = dac_ns as i64 + self.offset_ns;
        if lost_before > 0 {
            // The reference has a hole: line up again by the stamps.
            self.cursor = None;
        }
        if self.conv.as_ref().is_none_or(|c| c.tap_rate != rate) {
            // First piece or a new output device rate.
            let rs = if rate == self.in_rate { None } else { jk_dsp::OutputResampler::new(rate, self.in_rate, CHUNK).ok() };
            let delay_ns = rs.as_ref().map_or(0, |r| (r.output_delay() as f64 * 1e9 / f64::from(self.in_rate)).round() as i64);
            self.conv = Some(Conv { tap_rate: rate, rs, pend: [Vec::with_capacity(CHUNK), Vec::with_capacity(CHUNK)], pend_ns: host, delay_ns });
            self.cursor = None;
        }
        let Some(mut conv) = self.conv.take() else { return };
        match conv.rs.as_mut() {
            None => {
                // Same rate: straight in (deinterleaved, then into the ring).
                conv.pend[0].clear();
                conv.pend[1].clear();
                for f in data[..2 * frames].chunks_exact(2) {
                    conv.pend[0].push(f[0]);
                    conv.pend[1].push(f[1]);
                }
                self.push_frames(host, &conv.pend[0], &conv.pend[1]);
                conv.pend[0].clear();
                conv.pend[1].clear();
            }
            Some(_) => {
                let per_frame = 1e9 / f64::from(rate);
                for (j, f) in data[..2 * frames].chunks_exact(2).enumerate() {
                    if conv.pend[0].is_empty() {
                        conv.pend_ns = host + (j as f64 * per_frame).round() as i64;
                    }
                    conv.pend[0].push(f[0]);
                    conv.pend[1].push(f[1]);
                    if conv.pend[0].len() == CHUNK {
                        let t0 = conv.pend_ns - conv.delay_ns;
                        if let Some(rs) = conv.rs.as_mut()
                            && let Ok([ol, or]) = rs.process([&conv.pend[0][..], &conv.pend[1][..]])
                        {
                            self.push_frames(t0, ol, or);
                        }
                        conv.pend[0].clear();
                        conv.pend[1].clear();
                    }
                }
            }
        }
        self.conv = Some(conv);
    }

    /// Fractional ring index of the frame that left the DAC at host time `ns`. The pieces'
    /// stamps carry the device thread's wake-up jitter (a few milliseconds on a timer
    /// driven device), so the index comes from a least-squares line through the stamps of
    /// the last [`FIT_PIECES`] pieces of one run (frames contiguous, one rate), which is
    /// accurate to a small fraction of a millisecond.
    fn index_at(&self, ns: i64) -> Option<f64> {
        let n = self.anchors.len();
        if n == 0 {
            return None;
        }
        let from = n.saturating_sub(FIT_PIECES);
        let pts: Vec<(f64, f64)> = self.anchors.iter().skip(from).map(|&(f, t)| (f as f64, t as f64)).collect();
        let rate = f64::from(self.in_rate);
        if pts.len() < 3 {
            let (f, t) = pts[pts.len() - 1];
            return Some(f + (ns as f64 - t) * rate / 1e9);
        }
        // host = a + b * frame, fitted around the means (numerically safe with big values).
        let m = pts.len() as f64;
        let (mf, mt) = (pts.iter().map(|p| p.0).sum::<f64>() / m, pts.iter().map(|p| p.1).sum::<f64>() / m);
        let (mut sff, mut sft) = (0.0f64, 0.0f64);
        for &(f, t) in &pts {
            sff += (f - mf) * (f - mf);
            sft += (f - mf) * (t - mt);
        }
        let nominal = 1e9 / rate;
        // A slope far from the nominal frame period means the pieces are not one clean run
        // (a rate change, a big hole): use the nominal period from the newest stamp.
        let b = if sff > 0.0 { sft / sff } else { nominal };
        if !(b.is_finite() && (b / nominal - 1.0).abs() < 0.01) {
            let (f, t) = pts[pts.len() - 1];
            return Some(f + (ns as f64 - t) / nominal);
        }
        Some(mf + (ns as f64 - mt) / b)
    }

    /// True when the tap holds the reference for input captured up to host time
    /// `capture_end_ns` (the frame handed for it is `lead` later), or when the tap has
    /// written nothing for a while (no playback device): then there is nothing to wait for.
    pub fn covers(&self, capture_end_ns: i64, now_ns: i64) -> bool {
        let Some(&(f, t)) = self.anchors.back() else { return true };
        let newest = t + ((self.written.saturating_sub(f)) as f64 * 1e9 / f64::from(self.in_rate)) as i64;
        newest >= capture_end_ns + self.lead_ns || now_ns - newest > STALE_TAP_NS
    }

    /// The reference for an input block of `frames` frames whose first frame was captured
    /// at host time `capture_ns` (the device's declared input latency already taken off):
    /// interleaved stereo into `out` (`2 * frames` values). Zeros where the tap has nothing
    /// (no playback device, or the reader fell behind).
    pub fn reference(&mut self, capture_ns: i64, frames: usize, out: &mut [f32]) {
        let out = &mut out[..2 * frames];
        // Handed `lead` early: the frame that leaves the DAC `lead` after the capture time, so
        // a return that comes back up to `lead` sooner than the stamps say is still found
        // (the echo stage measures lead + residual; the chain subtracts the lead).
        let want = self.index_at(capture_ns + self.lead_ns);
        match (want, self.cursor) {
            (Some(w), Some(c)) if (w - c).abs() <= REALIGN_S * f64::from(self.in_rate) => {}
            (Some(w), _) => {
                self.cursor = Some(w.round());
                self.realigned += 1;
            }
            (None, _) => {
                out.fill(0.0);
                self.missing += frames as u64;
                return;
            }
        }
        let start = self.cursor.unwrap_or(0.0) as i64;
        let oldest = self.written as i64 - self.cap as i64 + 1;
        let mask = self.cap - 1;
        for j in 0..frames {
            let k = start + j as i64;
            if k < oldest.max(0) || k >= self.written as i64 {
                out[2 * j] = 0.0;
                out[2 * j + 1] = 0.0;
                self.missing += 1;
            } else {
                let s = (k as usize) & mask;
                out[2 * j] = self.ring[2 * s];
                out[2 * j + 1] = self.ring[2 * s + 1];
            }
        }
        self.cursor = Some(start as f64 + frames as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap of 10 ms pieces whose every frame carries its index, at `rate`.
    fn feed(a: &mut RefAligner, rate: u32, pieces: usize, t0_ns: u64) {
        let n = (rate / 100) as usize;
        let mut buf = vec![0.0f32; 2 * n];
        for p in 0..pieces {
            for j in 0..n {
                let v = (p * n + j) as f32;
                buf[2 * j] = v;
                buf[2 * j + 1] = -v;
            }
            let dac = t0_ns + (p as f64 * n as f64 * 1e9 / f64::from(rate)).round() as u64;
            a.push_tap(&buf, n, rate, dac, 0);
        }
    }

    #[test]
    fn same_rate_lines_up_once_then_reads_on() {
        let rate = 48_000;
        let mut a = RefAligner::new(rate, 1_000, 0.05);
        feed(&mut a, rate, 300, 5_000_000_000);
        // Input frame captured at tap frame 4800's DAC time - lead -> reference frame 4800
        // (the reference is handed `lead` early).
        let cap = 5_000_000_000i64 + 1_000 + 100_000_000 - 50_000_000;
        let mut out = vec![0.0; 2 * 480];
        a.reference(cap, 480, &mut out);
        assert_eq!(out[0], 4800.0);
        assert_eq!(out[1], -4800.0);
        assert_eq!(out[2 * 479], 5279.0);
        // The next block continues exactly, even with a jittered stamp.
        a.reference(cap + 10_000_000 + 300_000, 480, &mut out);
        assert_eq!(out[0], 5280.0);
        assert_eq!(a.realignments(), 1);
        // A stamp far off (a device switch) lines up again.
        a.reference(cap + 500_000_000, 480, &mut out);
        assert_eq!(a.realignments(), 2);
        assert_eq!(out[0], 4800.0 + 24_000.0);
    }

    #[test]
    fn jittered_stamps_still_line_up_within_a_fraction_of_a_millisecond() {
        // A timer-driven device thread wakes up to 4 ms late: each piece's stamp carries it.
        let rate = 48_000;
        let mut a = RefAligner::new(rate, 0, 0.0);
        let n = 480usize;
        let mut buf = vec![0.0f32; 2 * n];
        let mut seed = 12345u32;
        for p in 0..300 {
            for j in 0..n {
                buf[2 * j] = (p * n + j) as f32;
                buf[2 * j + 1] = 0.0;
            }
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let jitter = (seed >> 8) as u64 % 4_000_000;
            let dac = 1_000_000_000u64 + (p as u64) * 10_000_000 + jitter;
            a.push_tap(&buf, n, rate, dac, 0);
        }
        // Mean jitter is 2 ms: a frame's true DAC time is its nominal time + 2 ms.
        let mut out = vec![0.0; 2 * 10];
        a.reference(1_000_000_000 + 2_000_000_000 + 2_000_000, 10, &mut out);
        let err_frames = f64::from(out[0]) - 96_000.0;
        assert!(err_frames.abs() <= 10.0, "lined up {err_frames} frames off ({:.2} ms)", err_frames / 48.0);
        assert!(a.covers(1_000_000_000 + 2_900_000_000, 3_000_000_000));
        assert!(!a.covers(1_000_000_000 + 3_100_000_000, 3_000_000_000));
    }

    #[test]
    fn nothing_in_the_tap_gives_zeros() {
        let mut a = RefAligner::new(48_000, 0, 0.05);
        let mut out = vec![1.0; 2 * 100];
        a.reference(1_000_000_000, 100, &mut out);
        assert!(out.iter().all(|&x| x == 0.0));
        assert_eq!(a.missing, 100);
    }

    #[test]
    fn a_different_output_rate_is_resampled_to_the_input_rate() {
        // A 1 kHz tone at 48 kHz on the tap, read at 44.1 kHz: still a 1 kHz tone, placed
        // by the stamps within a fraction of a millisecond.
        let (out_rate, in_rate) = (48_000u32, 44_100u32);
        let mut a = RefAligner::new(in_rate, 0, 0.0);
        let n = 480usize;
        let mut buf = vec![0.0f32; 2 * n];
        for p in 0..300 {
            for j in 0..n {
                let t = (p * n + j) as f64 / f64::from(out_rate);
                let v = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32 * 0.5;
                buf[2 * j] = v;
                buf[2 * j + 1] = v;
            }
            let dac = 2_000_000_000u64 + (p as f64 * n as f64 * 1e9 / f64::from(out_rate)).round() as u64;
            a.push_tap(&buf, n, out_rate, dac, 0);
        }
        // Read 0.1 s starting at song (tap) time 1.0 s.
        let frames = 4410usize;
        let mut out = vec![0.0; 2 * frames];
        a.reference(2_000_000_000 + 1_000_000_000, frames, &mut out);
        // Least-squares fit of a 1 kHz sine and cosine on the ideal time axis: the amplitude
        // must be kept, the time offset (placement by the stamps) within 0.1 ms, and
        // what the fit leaves (the resampler's error) far below the tone.
        let w = 2.0 * std::f64::consts::PI * 1000.0;
        let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        for j in 0..frames {
            let t = 1.0 + j as f64 / f64::from(in_rate);
            let (s, c) = ((w * t).sin(), (w * t).cos());
            let y = f64::from(out[2 * j]);
            ss += s * s;
            cc += c * c;
            sc += s * c;
            ys += y * s;
            yc += y * c;
        }
        let det = ss * cc - sc * sc;
        let a = (ys * cc - yc * sc) / det;
        let b = (yc * ss - ys * sc) / det;
        let amp = (a * a + b * b).sqrt();
        let dt_us = (-b).atan2(a) / w * 1e6;
        let mut res = 0.0f64;
        for j in 0..frames {
            let t = 1.0 + j as f64 / f64::from(in_rate);
            res += (f64::from(out[2 * j]) - (a * (w * t).sin() + b * (w * t).cos())).powi(2);
        }
        let snr = 10.0 * ((0.25 * 0.5 * frames as f64) / res.max(1e-30)).log10();
        assert!((amp - 0.5).abs() < 0.01, "amplitude {amp}");
        // Within 0.1 ms (about 4 input frames): the echo stage measures the delay itself, so
        // a constant offset this small only shows in its residual delay.
        assert!(dt_us.abs() < 100.0, "placed {dt_us:.1} us off");
        assert!(snr > 60.0, "resampler residual {snr:.1} dB below the tone");
    }
}

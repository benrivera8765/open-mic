// SPDX-License-Identifier: GPL-3.0-or-later
//! The output tap: a lock-free copy of what the output device plays, for practice mode's
//! "music in the input" handling (WORK_ORDER-2 4.4 and 4.10, docs/ARCHITECTURE.md
//! section 12). The echo canceller (`jk_pitch::echo`) needs the exact signal the app sent
//! to the device as its reference, and the host time at which each frame left the DAC,
//! to line it up with the microphone input (whose blocks carry host-clock stamps too).
//!
//! # Where the copy is taken
//!
//! In the device filler ([`crate::output`]), at the very end of each device callback:
//! after the graph, the key shift, the output resampler, the seek/pause fades **and the
//! master volume**, i.e. exactly the `f32` frames handed to the device. What happens
//! after the tap is outside the app: the conversion to the device's sample format (i16 /
//! i24 / i32 clamp to full scale), the operating system's endpoint volume, the device
//! itself and whatever the mixer does with the return. For the canceller those are part of
//! the echo path.
//!
//! Master volume is **before** the tap, so turning the app's volume up or down scales the
//! reference and the returned music alike: the echo path the canceller has learnt does not
//! change and nothing has to re-adapt. Changing the OS volume or the mixer's USB-return
//! level does change the path; the canceller follows that by itself (its path-gain tracker;
//! docs/benchmarks/practice-echo.md "Changes of the echo path").
//!
//! Only the first two channels are kept (one channel is duplicated), interleaved stereo,
//! at the device rate.
//!
//! # Time stamps
//!
//! Every callback block is recorded with an [`Anchor`]: its first frame's index in the
//! tap stream, its length, the device rate, the song generation and position, and
//! `dac_ns`: the host time ([`crate::now_ns`], the engine's monotonic clock: QPC on
//! Windows, `mach_absolute_time` on the Mac) at which that first frame leaves the DAC,
//! computed as the callback's time plus the frames the device still holds ahead of it
//! (the same latency the playback clock uses). [`TapReader::read`] returns each piece of
//! audio with the stamp of its first frame.
//!
//! # Real-time safety and cost
//!
//! The ring is allocated once, on the first [`crate::Engine::output_tap`] call (the
//! control thread), and lives as long as the engine. The filler writes it with plain
//! atomic stores (a sequence-checked "overwrite the oldest" ring: the writer never waits
//! and never fails; a reader that falls more than the ring's length behind is told how
//! many frames it lost). No allocation, no lock, no system call on the callback. With no
//! reader attached the callback does two atomic loads and nothing else (the `OnceLock`
//! holding the tap and the reader count; only the first before the first attach). The
//! render worker is not touched. Playback is bit-identical
//! with or without a reader: the tap only reads the device buffer.
//!
//! One writer at a time: the engine closes (joins) a device's thread before it opens the
//! next one, so two fillers never run together.

use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering::*};
use std::sync::Arc;

/// Channels kept in the tap.
pub const TAP_CHANNELS: usize = 2;
/// Frames the ring holds (power of two): 5.5 s at 48 kHz, 2.7 s at 96 kHz.
pub const TAP_FRAMES: usize = 1 << 18;
/// Callback blocks whose stamps are kept (power of two): 20 s of 10 ms callbacks, 2 s of
/// 1 ms ones. More than the frames the ring holds in every case that matters.
const ANCHORS: usize = 2048;

/// One callback block's stamp (a sequence lock per slot).
#[derive(Default)]
struct AnchorSlot {
    /// 2k+1 while anchor number k is written, 2k+2 once complete.
    seq: AtomicU64,
    first_frame: AtomicU64,
    frames: AtomicU32,
    rate: AtomicU32,
    dac_ns: AtomicU64,
    generation: AtomicU64,
    /// Song position (f64 bits) of the first frame; NaN bits when unknown.
    song_pos: AtomicU64,
}

/// The stamp of one callback block, as the reader sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// Index of the block's first frame in the tap stream (frames written since the tap
    /// was created).
    pub first_frame: u64,
    pub frames: u32,
    /// Device rate of these frames, Hz.
    pub rate: u32,
    /// Host time ([`crate::now_ns`]) at which the first frame leaves the DAC.
    pub dac_ns: u64,
    /// Song generation (segment) the first frame belongs to (0 = nothing loaded yet).
    pub generation: u64,
    /// Song time of the first frame (seconds), if the filler knew it.
    pub song_pos: Option<f64>,
}

/// The shared ring. Created by [`OutputTap::new`]; written by the device filler.
pub struct OutputTap {
    data: Box<[AtomicU32]>,
    /// Frames whose slots the writer has started to overwrite (written + the block being
    /// written). Readers check it after copying.
    claimed: AtomicU64,
    /// Frames completely written.
    written: AtomicU64,
    anchors: Box<[AnchorSlot]>,
    /// Anchors completely written.
    anchor_count: AtomicU64,
    /// Readers attached; the filler skips the tap while it is 0.
    readers: AtomicU32,
}

impl Default for OutputTap {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputTap {
    /// Allocates the ring (about 2 MiB). Never call on a real-time thread.
    pub fn new() -> Self {
        OutputTap {
            data: (0..TAP_FRAMES * TAP_CHANNELS).map(|_| AtomicU32::new(0)).collect(),
            claimed: AtomicU64::new(0),
            written: AtomicU64::new(0),
            anchors: (0..ANCHORS).map(|_| AnchorSlot::default()).collect(),
            anchor_count: AtomicU64::new(0),
            readers: AtomicU32::new(0),
        }
    }

    /// True while at least one reader is attached (the filler writes only then).
    #[inline]
    pub fn active(&self) -> bool {
        self.readers.load(Relaxed) > 0
    }

    /// Frames written so far.
    pub fn written(&self) -> u64 {
        self.written.load(Acquire)
    }

    /// Device filler (the only writer): append one callback block. `out` is the device
    /// buffer, interleaved with `channels` channels; `dac_ns` the host time its first
    /// frame leaves the DAC. Plain atomic stores only: no allocation, no lock, no wait.
    pub fn write(&self, out: &[f32], channels: usize, rate: u32, dac_ns: u64, generation: u64, song_pos: Option<f64>) {
        let ch = channels.max(1);
        let frames = out.len() / ch;
        if frames == 0 {
            return;
        }
        let w = self.written.load(Relaxed);
        // Announce the overwrite before touching the slots (seqlock pattern: a reader that
        // sees any of the new samples also sees this).
        self.claimed.store(w + frames as u64, Relaxed);
        fence(Release);
        let mask = TAP_FRAMES - 1;
        for (j, f) in out.chunks_exact(ch).enumerate() {
            let slot = ((w as usize + j) & mask) * TAP_CHANNELS;
            let l = f[0];
            let r = if ch > 1 { f[1] } else { l };
            self.data[slot].store(l.to_bits(), Relaxed);
            self.data[slot + 1].store(r.to_bits(), Relaxed);
        }
        let k = self.anchor_count.load(Relaxed);
        let a = &self.anchors[(k as usize) & (ANCHORS - 1)];
        a.seq.store(2 * k + 1, Relaxed);
        fence(Release);
        a.first_frame.store(w, Relaxed);
        a.frames.store(frames as u32, Relaxed);
        a.rate.store(rate, Relaxed);
        a.dac_ns.store(dac_ns, Relaxed);
        a.generation.store(generation, Relaxed);
        a.song_pos.store(song_pos.unwrap_or(f64::NAN).to_bits(), Relaxed);
        a.seq.store(2 * k + 2, Release);
        self.anchor_count.store(k + 1, Release);
        self.written.store(w + frames as u64, Release);
    }

    /// Anchor number `k`, if it is still in the ring and was read consistently.
    fn anchor(&self, k: u64) -> Option<Anchor> {
        let a = &self.anchors[(k as usize) & (ANCHORS - 1)];
        let s1 = a.seq.load(Acquire);
        if s1 != 2 * k + 2 {
            return None;
        }
        let first_frame = a.first_frame.load(Relaxed);
        let frames = a.frames.load(Relaxed);
        let rate = a.rate.load(Relaxed);
        let dac_ns = a.dac_ns.load(Relaxed);
        let generation = a.generation.load(Relaxed);
        let pos = f64::from_bits(a.song_pos.load(Relaxed));
        fence(Acquire);
        if a.seq.load(Relaxed) != s1 {
            return None;
        }
        Some(Anchor { first_frame, frames, rate, dac_ns, generation, song_pos: pos.is_finite().then_some(pos) })
    }

    /// Attach a reader that starts at the newest frame (nothing old is replayed).
    pub fn reader(self: &Arc<Self>) -> TapReader {
        self.readers.fetch_add(1, AcqRel);
        let next = self.written.load(Acquire);
        let anchor_next = self.anchor_count.load(Acquire);
        TapReader { tap: self.clone(), next, anchor_next, current: None, lost: 0, pending_lost: 0 }
    }
}

/// What [`TapReader::read`] returned: `frames` frames of interleaved stereo in the
/// caller's buffer, all from one callback block (one rate, one stamp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapRead {
    pub frames: usize,
    /// Index of the first frame in the tap stream. A jump from the previous read's end
    /// means frames were lost (see `lost_before`).
    pub first_frame: u64,
    pub rate: u32,
    /// Host time ([`crate::now_ns`]) at which the first frame left (or leaves) the DAC.
    pub dac_ns: u64,
    pub generation: u64,
    /// Song time of the first frame, if known.
    pub song_pos: Option<f64>,
    /// Frames skipped just before this piece because the reader fell more than the ring's
    /// length behind (0 normally). The caller should feed the canceller zeros for them or
    /// reset it.
    pub lost_before: u64,
}

/// A reader of the output tap, for one analysis thread (`Send`, not shared). Dropping it
/// detaches it; with no reader left the filler stops copying.
pub struct TapReader {
    tap: Arc<OutputTap>,
    /// Next frame to read.
    next: u64,
    /// Next anchor to look at.
    anchor_next: u64,
    /// The anchor that covers `next`, once found.
    current: Option<Anchor>,
    lost: u64,
    /// Frames skipped but not yet reported in a [`TapRead`].
    pending_lost: u64,
}

impl TapReader {
    /// Frames written but not yet read (may exceed the ring's length when the reader fell
    /// behind).
    pub fn available(&self) -> u64 {
        self.tap.written.load(Acquire).saturating_sub(self.next)
    }

    /// Frames the filler has written to the tap since it was created (all readers). It
    /// stands still while no reader is attached.
    pub fn total_written(&self) -> u64 {
        self.tap.written()
    }

    /// Frames lost so far because the reader fell behind.
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// Skip everything written so far (start afresh at the newest frame).
    pub fn skip_to_newest(&mut self) {
        self.next = self.tap.written.load(Acquire);
        self.anchor_next = self.tap.anchor_count.load(Acquire);
        self.current = None;
    }

    /// Copy the next piece of the output (interleaved stereo, at most `out.len() / 2`
    /// frames, never more than the rest of one callback block) into `out`. `None` when
    /// nothing new has been written. Never blocks; does not allocate.
    pub fn read(&mut self, out: &mut [f32]) -> Option<TapRead> {
        let max = out.len() / TAP_CHANNELS;
        if max == 0 {
            return None;
        }
        // A second handle (one atomic increment, no allocation) so `self` can be updated
        // while the ring is read.
        let tap = Arc::clone(&self.tap);
        let written = tap.written.load(Acquire);
        if written <= self.next {
            return None;
        }
        // Fallen behind by more than the ring: jump to the oldest frame still safe to read
        // (leave a quarter of the ring for the writer to keep going while we copy).
        let oldest_ok = written.saturating_sub((TAP_FRAMES - TAP_FRAMES / 4) as u64);
        if self.next < oldest_ok {
            self.skip(oldest_ok - self.next);
            self.current = None;
        }
        // The anchor covering `next`.
        let anchors = tap.anchor_count.load(Acquire);
        let covers = |a: &Anchor, f: u64| f >= a.first_frame && f < a.first_frame + u64::from(a.frames);
        if !self.current.is_some_and(|a| covers(&a, self.next)) {
            self.current = None;
            if anchors.saturating_sub(self.anchor_next) > ANCHORS as u64 {
                self.anchor_next = anchors - ANCHORS as u64 / 2;
            }
            while self.anchor_next < anchors {
                match tap.anchor(self.anchor_next) {
                    Some(a) if a.first_frame + u64::from(a.frames) <= self.next => self.anchor_next += 1,
                    Some(a) => {
                        if a.first_frame > self.next {
                            // Frames before this anchor are gone with their stamps.
                            self.skip(a.first_frame - self.next);
                        }
                        self.current = Some(a);
                        break;
                    }
                    // Overwritten while we looked (we are far behind): move on.
                    None => self.anchor_next += 1,
                }
            }
        }
        let a = self.current?;
        let end = (a.first_frame + u64::from(a.frames)).min(written);
        if end <= self.next {
            // The anchor became visible before its frames were published: next time.
            return None;
        }
        let n = ((end - self.next) as usize).min(max);
        let mask = TAP_FRAMES - 1;
        for j in 0..n {
            let slot = ((self.next as usize + j) & mask) * TAP_CHANNELS;
            out[2 * j] = f32::from_bits(tap.data[slot].load(Relaxed));
            out[2 * j + 1] = f32::from_bits(tap.data[slot + 1].load(Relaxed));
        }
        // Were any of the copied slots overwritten meanwhile? (Seqlock check: a writer
        // that changed any slot we read announced it in `claimed` first.)
        fence(Acquire);
        let claimed = tap.claimed.load(Relaxed);
        let safe_from = claimed.saturating_sub(TAP_FRAMES as u64);
        let bad = if self.next < safe_from { ((safe_from - self.next) as usize).min(n) } else { 0 };
        let first = self.next + bad as u64;
        self.next += n as u64;
        if bad > 0 {
            // Drop the frames that may be torn; the rest of the piece is good.
            self.lost += bad as u64;
            self.pending_lost += bad as u64;
            if bad == n {
                return None;
            }
            out.copy_within(2 * bad..2 * n, 0);
        }
        let off = (first - a.first_frame) as f64 / f64::from(a.rate.max(1));
        Some(TapRead {
            frames: n - bad,
            first_frame: first,
            rate: a.rate,
            dac_ns: a.dac_ns + (off * 1e9).round() as u64,
            generation: a.generation,
            song_pos: a.song_pos.map(|p| p + off),
            lost_before: std::mem::take(&mut self.pending_lost),
        })
    }

    /// Skip `n` frames that can no longer be read.
    fn skip(&mut self, n: u64) {
        self.next += n;
        self.lost += n;
        self.pending_lost += n;
    }
}

impl Drop for TapReader {
    fn drop(&mut self) {
        self.tap.readers.fetch_sub(1, AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(start: u64, frames: usize, ch: usize) -> Vec<f32> {
        (0..frames * ch).map(|i| (start as f32 + (i / ch) as f32) + 0.25 * (i % ch) as f32).collect()
    }

    #[test]
    fn nothing_written_without_a_reader_check_is_callers_job_but_reader_sees_only_new() {
        let tap = Arc::new(OutputTap::new());
        assert!(!tap.active());
        tap.write(&block(0, 100, 2), 2, 48_000, 1_000, 1, Some(0.0));
        let mut r = tap.reader();
        assert!(tap.active());
        let mut buf = vec![0.0; 512];
        assert!(r.read(&mut buf).is_none(), "a new reader starts at the newest frame");
        tap.write(&block(100, 100, 2), 2, 48_000, 2_000_000, 1, Some(1.0));
        let got = r.read(&mut buf).unwrap();
        assert_eq!((got.frames, got.first_frame, got.dac_ns, got.lost_before), (100, 100, 2_000_000, 0));
        assert_eq!(&buf[..4], &[100.0, 100.25, 101.0, 101.25]);
        drop(r);
        assert!(!tap.active());
    }

    #[test]
    fn pieces_stop_at_block_ends_and_carry_their_stamps() {
        let tap = Arc::new(OutputTap::new());
        let mut r = tap.reader();
        // Mono device (duplicated) then a 4-channel device (first two kept).
        tap.write(&[0.5, -0.5, 0.25], 1, 44_100, 10_000_000_000, 3, Some(2.0));
        tap.write(&block(0, 10, 4), 4, 48_000, 20_000_000_000, 3, None);
        let mut buf = vec![0.0; 2 * 4];
        let a = r.read(&mut buf).unwrap();
        assert_eq!((a.frames, a.rate), (3, 44_100));
        assert_eq!(&buf[..6], &[0.5, 0.5, -0.5, -0.5, 0.25, 0.25]);
        let b = r.read(&mut buf).unwrap();
        assert_eq!((b.frames, b.first_frame, b.rate, b.song_pos), (4, 3, 48_000, None));
        assert_eq!(&buf[..4], &[0.0, 0.25, 1.0, 1.25]);
        let c = r.read(&mut buf).unwrap();
        // Frame 4 of the block: its stamp is 4 frames later.
        assert_eq!(c.dac_ns, 20_000_000_000 + (4.0f64 * 1e9 / 48_000.0).round() as u64);
        assert_eq!(c.frames, 4);
        let d = r.read(&mut buf).unwrap();
        assert_eq!(d.frames, 2);
        assert!(r.read(&mut buf).is_none());
    }

    #[test]
    fn a_reader_far_behind_is_told_what_it_lost() {
        let tap = Arc::new(OutputTap::new());
        let mut r = tap.reader();
        let blk = block(0, 480, 2);
        let blocks = TAP_FRAMES / 480 + 50;
        for k in 0..blocks {
            tap.write(&blk, 2, 48_000, k as u64 * 10_000_000, 1, Some(k as f64 * 0.01));
        }
        let mut buf = vec![0.0; 2 * 4096];
        let first = r.read(&mut buf).unwrap();
        assert!(first.lost_before > 0);
        let mut total = first.frames as u64 + first.lost_before;
        while let Some(p) = r.read(&mut buf) {
            assert_eq!(p.lost_before, 0);
            // Stamps follow the frame index exactly.
            let k = p.first_frame / 480;
            let off = p.first_frame % 480;
            assert_eq!(p.dac_ns, k * 10_000_000 + (off as f64 * 1e9 / 48_000.0).round() as u64);
            total += p.frames as u64;
        }
        assert_eq!(total, (blocks * 480) as u64);
        assert_eq!(r.lost(), first.lost_before);
    }

    #[test]
    fn cost_of_one_callback_block() {
        // What the device callback pays with a reader attached: one 10 ms stereo block.
        let tap = Arc::new(OutputTap::new());
        let _r = tap.reader();
        let blk = block(0, 480, 2);
        for _ in 0..1000 {
            tap.write(&blk, 2, 48_000, 0, 1, Some(0.0));
        }
        let n = 20_000;
        let t = std::time::Instant::now();
        for k in 0..n {
            tap.write(&blk, 2, 48_000, k, 1, Some(0.0));
        }
        let per = t.elapsed().as_secs_f64() / n as f64 * 1e6;
        println!("tap write, 480 stereo frames: {per:.2} us per callback block (10 ms of audio)");
        assert!(per < 50.0, "{per} us");
    }

    #[test]
    fn concurrent_writer_and_reader_never_tear_a_frame() {
        // Every frame carries its own index in both channels; the reader checks each one.
        let tap = Arc::new(OutputTap::new());
        let mut r = tap.reader();
        let w = {
            let tap = tap.clone();
            std::thread::spawn(move || {
                let mut idx = 0u64;
                let mut buf = vec![0.0f32; 2 * 441];
                for _ in 0..20_000 {
                    for f in 0..441 {
                        let v = (idx + f as u64) as f32;
                        buf[2 * f] = v;
                        buf[2 * f + 1] = -v;
                    }
                    tap.write(&buf, 2, 44_100, idx, 1, None);
                    idx += 441;
                }
            })
        };
        let mut buf = vec![0.0f32; 2 * 300];
        let mut checked = 0u64;
        let mut done = false;
        while !done {
            done = w.is_finished();
            while let Some(p) = r.read(&mut buf) {
                for j in 0..p.frames {
                    let want = (p.first_frame + j as u64) as f32;
                    // f32 holds integers exactly up to 2^24; beyond, compare loosely.
                    assert!((buf[2 * j] - want).abs() <= want * 1e-7 + 0.5, "frame {} torn: {}", p.first_frame + j as u64, buf[2 * j]);
                    assert_eq!(buf[2 * j + 1], -buf[2 * j]);
                }
                checked += p.frames as u64;
            }
        }
        w.join().unwrap();
        assert!(checked > 0);
        assert_eq!(checked + r.lost(), 20_000 * 441);
    }
}

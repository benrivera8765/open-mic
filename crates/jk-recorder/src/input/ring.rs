// SPDX-License-Identifier: GPL-3.0-or-later
//! The lock-free hand-over from the capture callback to the analysis thread.
//!
//! WO2 section 5: "The microphone callback only copies samples and a time stamp
//! into a lock-free buffer. Analysis runs on its own thread." This module is
//! that buffer:
//!
//! * two single-producer single-consumer rings ([`rtrb`], wait-free, allocated
//!   once when the stream opens): one of interleaved `f32` samples and one of
//!   [`BlockStamp`]s, one stamp per callback block;
//! * the producer ([`CaptureProducer`]) writes a block's samples first and its
//!   stamp second, so when the consumer sees a stamp, all of that block's
//!   samples are already in the sample ring;
//! * if the analysis thread falls behind and either ring is full, the producer
//!   drops the **whole** block (never half of one), counts it, and flags the
//!   next block it does write as a discontinuity. It never blocks and never
//!   allocates.
//!
//! [`CaptureProducer::push_interleaved`] and [`CaptureProducer::push_iter`] are
//! the only calls made on the audio callback; both are real-time safe (atomics
//! and the two rings only; `tests/input_rt_alloc.rs` counts allocations).
//! Errors (device unplugged, permission refused) are reported through
//! [`StreamStatus::fail`], which takes a lock and is meant for the backend's
//! error path or error callback, never the data callback.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use super::clock::host_time_ns;

/// Flag: samples were lost before this block (the analysis thread was too slow,
/// or the OS reported a gap). The block's own samples are intact.
pub const FLAG_DISCONTINUITY: u8 = 1;
/// Flag: the OS marked the block as silence (WASAPI `AUDCLNT_BUFFERFLAGS_SILENT`);
/// its samples are exact zeros.
pub const FLAG_SILENT: u8 = 2;
/// Flag: the OS could not time-stamp this block; `host_ns` is the time it
/// arrived instead (a few milliseconds late).
pub const FLAG_TIMESTAMP_ESTIMATED: u8 = 4;

/// One block of captured sound, as the analysis thread sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BlockStamp {
    /// Index of the block's first frame since the stream started, counting
    /// every frame the device delivered (dropped blocks included), so gaps can
    /// be seen. One frame = one sample per channel.
    pub first_frame: u64,
    /// Frames in the block.
    pub frames: u32,
    /// Host-clock time ([`super::clock`]) at which the block's first frame was
    /// captured, in nanoseconds, as the OS reported it. Device latency the OS
    /// does not declare is not included (WO2 section 4.8, "Delay").
    pub host_ns: i64,
    /// `FLAG_*` bits.
    pub flags: u8,
}

impl BlockStamp {
    /// True if samples were lost just before this block.
    pub fn discontinuity(&self) -> bool {
        self.flags & FLAG_DISCONTINUITY != 0
    }
}

/// Lifecycle of a capture stream, as the producer side reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RunState {
    /// Opened, no block delivered yet.
    Starting = 0,
    /// Blocks are arriving.
    Running = 1,
    /// The source finished on its own (end of a file or generated signal).
    Ended = 2,
    /// The source failed; see [`StreamStatus::failure`].
    Failed = 3,
    /// Stopped by the app.
    Stopped = 4,
}

impl RunState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => RunState::Starting,
            1 => RunState::Running,
            2 => RunState::Ended,
            3 => RunState::Failed,
            _ => RunState::Stopped,
        }
    }
}

/// Why a stream failed, in a form the UI can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// There is no input to open (none connected, or only Bluetooth / virtual
    /// devices, which are never opened automatically).
    NoDevice,
    /// The device was unplugged, disabled or switched off mid-stream.
    Unplugged,
    /// The OS refused access to the microphone (privacy settings).
    PermissionDenied,
    /// No sound arrived for longer than [`super::STALL_TIMEOUT_NS`]: a dead
    /// driver, a device taken over by another app, or a computer that slept.
    Stalled,
    /// The device cannot be used in the format it offers.
    Format,
    /// Any other error from the audio system (the message names it).
    Device,
}

/// State shared by the producer, its error path, the consumer and the app.
/// All counters are atomics; only the failure message sits behind a lock, and
/// only the error path and readers touch it.
#[derive(Debug)]
pub struct StreamStatus {
    state: AtomicU8,
    blocks: AtomicU64,
    frames: AtomicU64,
    dropped_blocks: AtomicU64,
    dropped_frames: AtomicU64,
    /// Host time of the most recent push (whether or not the block fitted), for
    /// the stall watchdog.
    last_push_ns: AtomicI64,
    opened_ns: AtomicI64,
    failure: Mutex<Option<(FailureKind, String)>>,
}

impl StreamStatus {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(RunState::Starting as u8),
            blocks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            dropped_blocks: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            last_push_ns: AtomicI64::new(0),
            opened_ns: AtomicI64::new(host_time_ns()),
            failure: Mutex::new(None),
        }
    }

    /// The current state.
    pub fn state(&self) -> RunState {
        RunState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Report a failure. The first failure wins; later ones are ignored. Takes a
    /// lock: call it from an error path or error callback, never from the data
    /// callback.
    pub fn fail(&self, kind: FailureKind, message: impl Into<String>) {
        let mut f = self.failure.lock().unwrap_or_else(|e| e.into_inner());
        if f.is_none() && !matches!(self.state(), RunState::Stopped) {
            *f = Some((kind, message.into()));
            self.state.store(RunState::Failed as u8, Ordering::Release);
        }
    }

    /// The source finished on its own (file or generated signal).
    pub fn end(&self) {
        let _ = self.state.compare_exchange(
            RunState::Running as u8,
            RunState::Ended as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let _ = self.state.compare_exchange(
            RunState::Starting as u8,
            RunState::Ended as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Mark the stream stopped by the app (unless it already failed).
    pub fn mark_stopped(&self) {
        let s = self.state();
        if !matches!(s, RunState::Failed) {
            self.state.store(RunState::Stopped as u8, Ordering::Release);
        }
    }

    /// The failure, if the stream failed.
    pub fn failure(&self) -> Option<(FailureKind, String)> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Blocks and frames delivered by the source (dropped ones included).
    pub fn delivered(&self) -> (u64, u64) {
        (
            self.blocks.load(Ordering::Relaxed),
            self.frames.load(Ordering::Relaxed),
        )
    }

    /// Blocks and frames dropped because the analysis thread was too slow.
    pub fn dropped(&self) -> (u64, u64) {
        (
            self.dropped_blocks.load(Ordering::Relaxed),
            self.dropped_frames.load(Ordering::Relaxed),
        )
    }

    /// Host time of the most recent block pushed (0 = none yet).
    pub fn last_push_ns(&self) -> i64 {
        self.last_push_ns.load(Ordering::Relaxed)
    }

    /// Host time the stream was opened.
    pub fn opened_ns(&self) -> i64 {
        self.opened_ns.load(Ordering::Relaxed)
    }
}

/// The callback side. Owned by exactly one thread or callback.
pub struct CaptureProducer {
    samples: Producer<f32>,
    stamps: Producer<BlockStamp>,
    channels: usize,
    next_frame: u64,
    pending_flags: u8,
    status: Arc<StreamStatus>,
}

/// The analysis side. Owned by exactly one thread.
pub struct CaptureConsumer {
    samples: Consumer<f32>,
    stamps: Consumer<BlockStamp>,
    channels: usize,
    status: Arc<StreamStatus>,
}

/// Create the rings for a stream of `channels` channels.
///
/// `sample_capacity_frames` frames of samples and `block_capacity` stamps are
/// allocated now, once. Size them for the longest the analysis thread may be
/// late (the app uses 2 s, [`super::RING_SECONDS`]).
pub fn capture_ring(
    channels: u16,
    sample_capacity_frames: usize,
    block_capacity: usize,
) -> (CaptureProducer, CaptureConsumer, Arc<StreamStatus>) {
    let channels = usize::from(channels.max(1));
    let (sp, sc) = RingBuffer::<f32>::new(sample_capacity_frames.max(1) * channels);
    let (tp, tc) = RingBuffer::<BlockStamp>::new(block_capacity.max(4));
    let status = Arc::new(StreamStatus::new());
    (
        CaptureProducer {
            samples: sp,
            stamps: tp,
            channels,
            next_frame: 0,
            pending_flags: 0,
            status: status.clone(),
        },
        CaptureConsumer {
            samples: sc,
            stamps: tc,
            channels,
            status: status.clone(),
        },
        status,
    )
}

impl CaptureProducer {
    /// Channels per frame.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The shared status (for the backend's error path / error callback).
    pub fn status(&self) -> Arc<StreamStatus> {
        self.status.clone()
    }

    /// True if a block of `frames` frames would fit now.
    pub fn has_room(&self, frames: usize) -> bool {
        self.samples.slots() >= frames * self.channels && self.stamps.slots() >= 1
    }

    /// Tell the stall watchdog the source is alive although it delivered
    /// nothing (a generated source waiting for the reader). Real-time safe.
    pub fn touch(&self) {
        self.status
            .last_push_ns
            .store(host_time_ns(), Ordering::Relaxed);
    }

    /// Bookkeeping common to every push. Returns false if the block does not fit.
    #[inline]
    fn admit(&mut self, frames: usize) -> bool {
        let st = &self.status;
        st.blocks.fetch_add(1, Ordering::Relaxed);
        st.frames.fetch_add(frames as u64, Ordering::Relaxed);
        st.last_push_ns.store(host_time_ns(), Ordering::Relaxed);
        let _ = st.state.compare_exchange(
            RunState::Starting as u8,
            RunState::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let need = frames * self.channels;
        if self.samples.slots() < need || self.stamps.slots() < 1 {
            st.dropped_blocks.fetch_add(1, Ordering::Relaxed);
            st.dropped_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
            self.pending_flags |= FLAG_DISCONTINUITY;
            self.next_frame += frames as u64;
            return false;
        }
        true
    }

    #[inline]
    fn stamp(&mut self, frames: usize, host_ns: i64, flags: u8) {
        let stamp = BlockStamp {
            first_frame: self.next_frame,
            frames: frames as u32,
            host_ns,
            flags: flags | self.pending_flags,
        };
        self.pending_flags = 0;
        self.next_frame += frames as u64;
        // Cannot fail: `admit` checked for a free slot and only this side writes.
        let _ = self.stamps.push(stamp);
    }

    /// Hand over one block of interleaved `f32` samples. Real-time safe.
    /// Returns false if the block was dropped because the analysis side is
    /// behind (it is counted, and the next block is flagged).
    pub fn push_interleaved(&mut self, interleaved: &[f32], host_ns: i64, flags: u8) -> bool {
        let frames = interleaved.len() / self.channels;
        if frames == 0 {
            return true;
        }
        let used = &interleaved[..frames * self.channels];
        if !self.admit(frames) {
            return false;
        }
        // Cannot fail: `admit` checked the free space and only this side writes.
        let _ = self.samples.push_entire_slice(used);
        self.stamp(frames, host_ns, flags);
        true
    }

    /// Hand over one block of `frames` frames produced by an iterator of
    /// interleaved samples (used to convert integer formats straight into the
    /// ring without a scratch buffer). Real-time safe as long as the iterator
    /// is. Missing samples (a short iterator) are written as zeros.
    pub fn push_iter<I: Iterator<Item = f32>>(
        &mut self,
        frames: usize,
        samples: I,
        host_ns: i64,
        flags: u8,
    ) -> bool {
        if frames == 0 {
            return true;
        }
        if !self.admit(frames) {
            return false;
        }
        let n = frames * self.channels;
        if let Ok(chunk) = self.samples.write_chunk_uninit(n) {
            chunk.fill_from_iter(samples.chain(std::iter::repeat(0.0)).take(n));
        }
        self.stamp(frames, host_ns, flags);
        true
    }
}

impl CaptureConsumer {
    /// Channels per frame.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The shared status.
    pub fn status(&self) -> &Arc<StreamStatus> {
        &self.status
    }

    /// Take the next block, if one is complete. Its samples replace the
    /// contents of `out` (interleaved). `out` keeps its capacity, so a reused
    /// buffer stops allocating after the first blocks.
    pub fn pop_block(&mut self, out: &mut Vec<f32>) -> Option<BlockStamp> {
        let stamp = self.stamps.pop().ok()?;
        let n = stamp.frames as usize * self.channels;
        out.clear();
        match self.samples.read_chunk(n) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                out.extend_from_slice(a);
                out.extend_from_slice(b);
                chunk.commit_all();
            }
            Err(_) => {
                // Unreachable by construction (samples are written before the
                // stamp); deliver silence of the right length rather than panic.
                out.resize(n, 0.0);
            }
        }
        Some(stamp)
    }

    /// Blocks waiting to be read.
    pub fn pending_blocks(&self) -> usize {
        self.stamps.slots()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_keep_their_stamps_and_samples() {
        let (mut p, mut c, st) = capture_ring(2, 64, 8);
        assert_eq!(st.state(), RunState::Starting);
        assert!(p.push_interleaved(&[1.0, 2.0, 3.0, 4.0], 100, 0));
        assert!(p.push_iter(
            3,
            [5.0f32, 6.0, 7.0, 8.0, 9.0].into_iter(),
            200,
            FLAG_SILENT
        ));
        assert_eq!(st.state(), RunState::Running);
        let mut buf = Vec::new();
        let s1 = c.pop_block(&mut buf).unwrap();
        assert_eq!(
            (s1.first_frame, s1.frames, s1.host_ns, s1.flags),
            (0, 2, 100, 0)
        );
        assert_eq!(buf, vec![1.0, 2.0, 3.0, 4.0]);
        let s2 = c.pop_block(&mut buf).unwrap();
        assert_eq!((s2.first_frame, s2.frames, s2.host_ns), (2, 3, 200));
        assert_eq!(s2.flags, FLAG_SILENT);
        // A short iterator is padded with zeros.
        assert_eq!(buf, vec![5.0, 6.0, 7.0, 8.0, 9.0, 0.0]);
        assert!(c.pop_block(&mut buf).is_none());
    }

    #[test]
    fn a_full_ring_drops_whole_blocks_and_flags_the_next() {
        // Room for 4 frames of stereo.
        let (mut p, mut c, st) = capture_ring(2, 4, 8);
        assert!(p.push_interleaved(&[0.1; 6], 1, 0)); // 3 frames
        assert!(!p.push_interleaved(&[0.2; 4], 2, 0)); // 2 more do not fit
        assert_eq!(st.dropped(), (1, 2));
        let mut buf = Vec::new();
        assert_eq!(c.pop_block(&mut buf).unwrap().frames, 3);
        assert!(p.push_interleaved(&[0.3; 4], 3, 0));
        let s = c.pop_block(&mut buf).unwrap();
        assert!(s.discontinuity());
        // Frame numbering counts the lost frames.
        assert_eq!(s.first_frame, 5);
        assert_eq!(buf, vec![0.3; 4]);
        assert_eq!(st.delivered(), (3, 7));
    }

    #[test]
    fn first_failure_wins_and_stop_does_not_hide_it() {
        let (_p, _c, st) = capture_ring(1, 4, 4);
        st.fail(FailureKind::Unplugged, "gone");
        st.fail(FailureKind::Device, "later");
        st.mark_stopped();
        assert_eq!(st.state(), RunState::Failed);
        assert_eq!(st.failure().unwrap().0, FailureKind::Unplugged);
    }
}

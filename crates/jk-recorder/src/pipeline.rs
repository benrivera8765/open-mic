// SPDX-License-Identifier: GPL-3.0-or-later
//! The platform-independent half of the recorder: timeline, meter and writer.
//!
//! A platform backend (ScreenCaptureKit, WASAPI loopback) only has to:
//!
//! 1. call [`CapturePipeline::set_format`] once it knows the real format of
//!    the audio the OS delivers;
//! 2. call [`CapturePipeline::mark_start`] with the OS-clock instant it starts
//!    the capture stream, and [`CapturePipeline::mark_stop`] with the instant
//!    the user stops it (same clock as the block timestamps);
//! 3. call [`CapturePipeline::push`] with every block of interleaved `f32`
//!    samples and the OS timestamp of its first frame (or
//!    [`CapturePipeline::push_silence`] for blocks the OS flags as silent);
//! 4. call [`CapturePipeline::close_with_error`] if the OS stops the stream on
//!    its own (device removed, format changed), or [`CapturePipeline::fail`]
//!    for an error that does not end the recording.
//!
//! The pipeline then meters the audio, keeps the recording's sample count in
//! step with the OS clock, and writes a progressive float WAV/RF64 on its own
//! thread, so the capture callback never touches the disk.
//!
//! # Keeping the sample count equal to elapsed time
//!
//! WORK_ORDER section 7, trap 5: a loopback capture may deliver nothing while
//! the system is silent, and a recording built from "what arrived" comes out
//! shorter than real time. Three rules keep the length equal to the time from
//! Start to Stop:
//!
//! * **Head.** If the first block starts later than the start instant, the
//!   difference is filled with digital silence (`head_fill_frames`), so a
//!   recording started during silence still begins at Start.
//! * **Holes.** Each block's OS timestamp is compared with where the previous
//!   block ended. A jump forward larger than the gap threshold (10 ms, or half
//!   a block if that is longer) is a real hole, filled with digital silence of
//!   exactly that many frames. Smaller differences are clock jitter or slow
//!   drift between the OS host clock and the audio device clock. They are only
//!   measured, never "fixed" by inserting or dropping samples, because doing
//!   so every few minutes would put tiny dropouts into a perfectly good
//!   recording. Re-anchoring on every block keeps drift from accumulating
//!   into a false gap.
//! * **Tail.** At [`CapturePipeline::finish`], if the last block ended before
//!   the stop instant, the difference is filled with silence
//!   (`tail_fill_frames`), so a recording stopped during silence still ends at
//!   Stop.
//!
//! The head and tail rules trust that the start/stop instants and the block
//! timestamps come from the same clock. As a guard, the first timed block is
//! checked against the start instant (it cannot start more than `edge_slack`
//! before it, nor later than the wall-clock time that has passed since, plus
//! `edge_slack`), and a tail can never be longer than the wall-clock time from
//! Start to Stop. A failed check is counted in `edge_fill_refused` and nothing
//! is inserted, rather than a wrong amount of silence.
//!
//! If the writer cannot keep up (very slow USB stick) and the queue overflows,
//! the lost frames are replaced by silence of the same length (so the timeline
//! stays true) and counted in `dropped_frames`, which the UI must show as an
//! error: audio was lost.
//!
//! # Real-time behaviour
//!
//! [`CapturePipeline::push`] does not allocate: sample buffers come from a pool
//! that is filled when the format is declared, travel to the writer thread
//! and come back to be reused (measured: 0 heap allocations in the whole
//! process during 5 s of steady recording, `loopback_probe --alloc-check`),
//! never waits for the disk, and takes one mutex that only the capture thread
//! uses while recording (start, stop and errors are the only other users).
//! [`CapturePipeline::stats`] reads atomics only, so a UI polling it can never
//! hold up the capture thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::RecorderError;
use crate::format::CaptureFormat;
use crate::meter::{LevelMeter, Levels};
use crate::wav::{WavSpec, WavSummary, WavWriter};

/// A timestamp on the OS's monotonic media clock: `value / timescale` seconds.
/// macOS gives `CMTime` (value, timescale); Windows passes QPC time in 100 ns
/// units (`timescale = 10_000_000`), the unit WASAPI's `GetBuffer` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTime {
    /// Tick count.
    pub value: i64,
    /// Ticks per second (must be > 0).
    pub timescale: u32,
}

impl MediaTime {
    /// The timestamp as a frame index at `rate`, rounded to the nearest frame.
    /// Exact integer arithmetic (i128), so long recordings do not accumulate
    /// floating-point error.
    pub fn to_frames(self, rate: u32) -> i128 {
        let num = i128::from(self.value) * i128::from(rate);
        let den = i128::from(self.timescale.max(1));
        // Round half away from zero.
        if num >= 0 {
            (num + den / 2) / den
        } else {
            (num - den / 2) / den
        }
    }

    /// The same clock, `d` earlier (for an instant that happened `d` ago).
    pub fn earlier_by(self, d: Duration) -> MediaTime {
        let ticks = d.as_nanos() * u128::from(self.timescale.max(1)) / 1_000_000_000;
        MediaTime {
            value: self.value - i64::try_from(ticks).unwrap_or(i64::MAX),
            timescale: self.timescale,
        }
    }

    /// Seconds as a float (for display only).
    pub fn seconds(self) -> f64 {
        self.value as f64 / f64::from(self.timescale.max(1))
    }
}

/// Tuning knobs. The defaults are what the recorder uses.
#[derive(Clone, Debug)]
pub struct PipelineOptions {
    /// Timestamp jumps longer than this (or half a block, if longer) are filled
    /// with silence.
    pub gap_threshold: Duration,
    /// Jumps longer than this are treated as an OS clock reset, not a hole.
    pub max_gap_fill: Duration,
    /// Blocks the queue to the writer thread can hold (a block is typically
    /// 10 to 25 ms, so 4096 blocks is well over a minute of slack).
    pub queue_blocks: usize,
    /// How often the WAV header is patched so the file on disk stays valid.
    pub checkpoint_every: Duration,
    /// How often the file is flushed to the device (`fsync`).
    pub sync_every: Duration,
    /// Head/tail extension guard (see the module documentation): how much
    /// longer than the wall-clock time that really passed a head or tail fill
    /// may be before the start/stop instant is judged to be on a different
    /// clock from the block timestamps.
    pub edge_slack: Duration,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            gap_threshold: Duration::from_millis(10),
            max_gap_fill: Duration::from_secs(600),
            queue_blocks: 4096,
            checkpoint_every: Duration::from_secs(1),
            sync_every: Duration::from_secs(5),
            edge_slack: Duration::from_secs(1),
        }
    }
}

/// Everything measurable about a recording, for the UI, the self-check and CI.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingStats {
    /// The file being written.
    pub path: PathBuf,
    /// The real format, once known.
    pub format: Option<CaptureFormat>,
    /// Wall-clock time since the pipeline was created.
    pub wall_elapsed: Duration,
    /// Frames in the recording's timeline (audio received + silence inserted
    /// for the head, gaps, the tail and dropped blocks).
    pub frames: u64,
    /// `frames / sample_rate`: the recording's length so far, in seconds.
    pub media_elapsed_secs: f64,
    /// Span of the OS clock the recording covers, in seconds: from the start
    /// instant (or the first block) to the stop instant (or the end of the
    /// last block), when the OS provides timestamps.
    pub os_clock_elapsed_secs: Option<f64>,
    /// OS-clock time from the start instant to the stop instant, once both
    /// are known (the yardstick for "sample count equals elapsed time").
    pub start_to_stop_secs: Option<f64>,
    /// Frames that arrived from the OS (audio blocks and blocks the OS flagged
    /// as silent), not counting any silence the pipeline inserted.
    pub received_frames: u64,
    /// Frames the writer thread has put into the file.
    pub frames_written: u64,
    /// File length so far (header + data).
    pub file_bytes: u64,
    /// True once the file has been promoted to RF64 (> 4 GiB).
    pub rf64: bool,
    /// Blocks received from the OS.
    pub blocks: u64,
    /// Samples at or above full scale (see [`crate::meter::CLIP_LEVEL`]).
    pub clipped_samples: u64,
    /// NaN/inf samples received.
    pub non_finite_samples: u64,
    /// Silence inserted between the start instant and the first block.
    pub head_fill_frames: u64,
    /// Holes in the OS timestamps that were filled with silence.
    pub gap_events: u64,
    /// Total frames of silence inserted for those holes.
    pub gap_frames: u64,
    /// Silence inserted between the end of the last block and the stop instant.
    pub tail_fill_frames: u64,
    /// Head or tail extensions refused because the start/stop instant did not
    /// agree with the block timestamps (see `PipelineOptions::edge_slack`).
    pub edge_fill_refused: u64,
    /// Blocks whose timestamp lay before the previous block's end (by more
    /// than the threshold). Their audio is kept; this is only reported.
    pub overlap_events: u64,
    /// Total overlap in frames.
    pub overlap_frames: u64,
    /// Timestamp jumps too large to be a hole (clock reset); not filled.
    pub timestamp_resets: u64,
    /// Largest timestamp jitter below the gap threshold, in frames.
    pub max_jitter_frames: u64,
    /// Frames lost because the writer could not keep up (replaced by silence).
    /// Anything above zero is a real loss of audio.
    pub dropped_frames: u64,
    /// Buffers whose format differed from the first one. The first such
    /// buffer stops the recording: state Failed, file closed (see `error`).
    pub format_mismatches: u64,
    /// The first error, if any (writer failure, OS stopped the stream, ...).
    pub error: Option<String>,
}

impl RecordingStats {
    /// True if anything reached full scale (the UI's clip warning).
    pub fn clip_warning(&self) -> bool {
        self.clipped_samples > 0
    }

    /// Recording length divided by the OS clock span it covers: 1.0 means the
    /// sample count matches the OS timestamps exactly.
    pub fn frames_per_os_second_ratio(&self) -> Option<f64> {
        match (self.os_clock_elapsed_secs, &self.format) {
            (Some(t), Some(_)) if t > 0.0 => Some(self.media_elapsed_secs / t),
            _ => None,
        }
    }

    /// Recording length divided by the OS-clock time from Start to Stop: the
    /// WORK_ORDER section 9 measure ("sample count matches elapsed time").
    pub fn length_vs_start_stop_ratio(&self) -> Option<f64> {
        match (self.start_to_stop_secs, &self.format) {
            (Some(t), Some(_)) if t > 0.0 => Some(self.media_elapsed_secs / t),
            _ => None,
        }
    }
}

/// Lifecycle of the pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineState {
    /// Created; no audio has arrived yet.
    WaitingForAudio,
    /// Audio is being written.
    Recording,
    /// An error stopped the recording (the file up to that point is valid).
    Failed(String),
    /// `finish()` has completed.
    Finished,
}

enum Msg {
    Begin(WavSpec),
    Samples(Vec<f32>),
    Silence(u64),
}

/// "Not set" for the atomic frame positions below.
const UNSET: i64 = i64::MIN;

struct Counters {
    frames: AtomicU64,
    received_frames: AtomicU64,
    frames_written: AtomicU64,
    file_bytes: AtomicU64,
    rf64: AtomicBool,
    blocks: AtomicU64,
    head_fill_frames: AtomicU64,
    gap_events: AtomicU64,
    gap_frames: AtomicU64,
    tail_fill_frames: AtomicU64,
    edge_fill_refused: AtomicU64,
    overlap_events: AtomicU64,
    overlap_frames: AtomicU64,
    timestamp_resets: AtomicU64,
    max_jitter_frames: AtomicU64,
    dropped_frames: AtomicU64,
    format_mismatches: AtomicU64,
    /// OS-clock span covered by the timeline, as frame positions (UNSET until
    /// known). Atomics so `stats()` never takes the producer lock.
    span_first: AtomicI64,
    span_end: AtomicI64,
    /// Start/stop instants in seconds (f64 bits; NaN = not marked), display only.
    start_secs: AtomicU64,
    stop_secs: AtomicU64,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            frames: AtomicU64::new(0),
            received_frames: AtomicU64::new(0),
            frames_written: AtomicU64::new(0),
            file_bytes: AtomicU64::new(0),
            rf64: AtomicBool::new(false),
            blocks: AtomicU64::new(0),
            head_fill_frames: AtomicU64::new(0),
            gap_events: AtomicU64::new(0),
            gap_frames: AtomicU64::new(0),
            tail_fill_frames: AtomicU64::new(0),
            edge_fill_refused: AtomicU64::new(0),
            overlap_events: AtomicU64::new(0),
            overlap_frames: AtomicU64::new(0),
            timestamp_resets: AtomicU64::new(0),
            max_jitter_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            format_mismatches: AtomicU64::new(0),
            span_first: AtomicI64::new(UNSET),
            span_end: AtomicI64::new(UNSET),
            start_secs: AtomicU64::new(f64::NAN.to_bits()),
            stop_secs: AtomicU64::new(f64::NAN.to_bits()),
        }
    }
}

/// An instant on the OS media clock, with the wall-clock instant it was taken
/// (for the head/tail guard).
#[derive(Clone, Copy, Debug)]
struct Mark {
    at: MediaTime,
    wall: Instant,
}

/// Producer-side state, touched by the capture thread for every block and
/// otherwise only by start/stop/error calls (the mutex is uncontended while
/// recording).
struct Producer {
    /// `None` once the pipeline is closed (finished or failed).
    tx: Option<SyncSender<Msg>>,
    /// Empty sample buffers handed back by the writer for reuse.
    recycled: Receiver<Vec<f32>>,
    /// Where the next block should start on the OS clock, in frames.
    expected_next: Option<i128>,
    /// True once any block (timed or not) has arrived.
    seen_block: bool,
    /// Frames owed to the writer (head fill, gaps, queue overflow).
    owed_silence: u64,
    start_mark: Option<Mark>,
    stop_mark: Option<Mark>,
    /// Whether the start instant agreed with the first timed block (judged
    /// once; `None` until then). If not, neither head nor tail is filled.
    edges_trusted: Option<bool>,
}

/// See the module documentation.
pub struct CapturePipeline {
    path: PathBuf,
    opts: PipelineOptions,
    created: Instant,
    meter: LevelMeter,
    format: OnceLock<CaptureFormat>,
    channel_mask: OnceLock<u32>,
    /// Hands pre-allocated sample buffers to the pool when the format is known.
    prewarm: Mutex<Option<SyncSender<Vec<f32>>>>,
    producer: Mutex<Producer>,
    counters: Arc<Counters>,
    error: Arc<Mutex<Option<String>>>,
    writer: Mutex<Option<JoinHandle<Option<WavSummary>>>>,
    closed: AtomicBool,
    finish_called: AtomicBool,
    finished: AtomicBool,
}

impl std::fmt::Debug for CapturePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturePipeline")
            .field("path", &self.path)
            .finish()
    }
}

/// What `finish()` returns.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSummary {
    /// Final statistics.
    pub stats: RecordingStats,
    /// The finished file, or `None` if the format never became known (no file
    /// is created in that case).
    pub file: Option<WavSummary>,
}

/// Frames in `d` at `rate` (rounded down).
fn frames_in(rate: u32, d: Duration) -> i128 {
    i128::from(rate) * d.as_nanos() as i128 / 1_000_000_000
}

/// A short description of a format for messages ("48000 Hz, 2 ch, 32-bit float").
fn short_format(f: &CaptureFormat) -> String {
    format!("{} Hz, {} ch, {}", f.sample_rate, f.channels, f.source)
}

/// Pool size for recycled sample buffers.
const RECYCLE_POOL: usize = 64;
/// Buffers put into the pool when the format is declared, so the capture side
/// does not allocate even while the writer is busy with an fsync (about 320 ms
/// of 10 ms packets).
const PREWARM_BUFFERS: usize = 32;

impl CapturePipeline {
    /// Create the pipeline and start its writer thread. The file is created
    /// when the format is declared, and never overwrites an existing file.
    pub fn new(path: &Path, opts: PipelineOptions) -> Result<Arc<Self>, RecorderError> {
        let (tx, rx) = mpsc::sync_channel::<Msg>(opts.queue_blocks.max(1));
        let (recycle_tx, recycle_rx) = mpsc::sync_channel::<Vec<f32>>(RECYCLE_POOL);
        let prewarm = recycle_tx.clone();
        let counters = Arc::new(Counters::default());
        let error = Arc::new(Mutex::new(None));
        let handle = {
            let path = path.to_path_buf();
            let counters = Arc::clone(&counters);
            let error = Arc::clone(&error);
            let opts = opts.clone();
            std::thread::Builder::new()
                .name("jk-recorder-writer".into())
                .spawn(move || writer_thread(&path, &opts, rx, &recycle_tx, &counters, &error))?
        };
        Ok(Arc::new(Self {
            path: path.to_path_buf(),
            opts,
            created: Instant::now(),
            meter: LevelMeter::new(),
            format: OnceLock::new(),
            channel_mask: OnceLock::new(),
            prewarm: Mutex::new(Some(prewarm)),
            producer: Mutex::new(Producer {
                tx: Some(tx),
                recycled: recycle_rx,
                expected_next: None,
                seen_block: false,
                owed_silence: 0,
                start_mark: None,
                stop_mark: None,
                edges_trusted: None,
            }),
            counters,
            error,
            writer: Mutex::new(Some(handle)),
            closed: AtomicBool::new(false),
            finish_called: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        }))
    }

    fn lock_producer(&self) -> MutexGuard<'_, Producer> {
        self.producer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The real capture format, once known.
    pub fn format(&self) -> Option<&CaptureFormat> {
        self.format.get()
    }

    /// The speaker-position mask to store in the file (`dwChannelMask`, used
    /// for more than two channels). Call before [`CapturePipeline::set_format`];
    /// later calls are ignored.
    pub fn set_channel_mask(&self, mask: u32) {
        let _ = self.channel_mask.set(mask);
    }

    /// Declare the real format of the audio. The first call creates the file;
    /// later calls with the same format are no-ops. A different format means
    /// the OS changed it mid-recording: the recording is stopped there (state
    /// Failed with a plain message, file closed and valid) and an error is
    /// returned, so a file never mixes two formats.
    pub fn set_format(&self, format: CaptureFormat) -> Result<(), RecorderError> {
        if let Some(existing) = self.format.get() {
            if *existing == format {
                return Ok(());
            }
            self.counters
                .format_mismatches
                .fetch_add(1, Ordering::Relaxed);
            let msg = format!(
                "The audio format changed during the recording, from {} to {}. The recording \
                 stopped at that point and the file was closed, so it never mixes two formats; \
                 everything recorded before the change is saved. Start a new recording to continue.",
                short_format(existing),
                short_format(&format)
            );
            self.close_with_error(msg.clone());
            return Err(RecorderError::Format(msg));
        }
        if format.channels == 0 || format.sample_rate == 0 {
            return Err(RecorderError::Format(format!("invalid format {format}")));
        }
        let mut spec = WavSpec::new(format.sample_rate, format.channels);
        spec.channel_mask = self.channel_mask.get().copied();
        self.meter.set_channels(usize::from(format.channels));
        if self.format.set(format.clone()).is_err() {
            // Another thread declared a format at the same moment: compare.
            return self.set_format(format);
        }
        let p = self.lock_producer();
        if let Some(tx) = &p.tx {
            // Nothing else is queued before the format, so this cannot be Full.
            let _ = tx.try_send(Msg::Begin(spec));
        }
        drop(p);
        // Pre-warm the buffer pool (not on the steady-state capture path):
        // room for 100 ms per buffer, more than any OS block size.
        if let Some(tx) = self
            .prewarm
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let cap = (format.sample_rate as usize / 10).max(4096) * usize::from(format.channels);
            for _ in 0..PREWARM_BUFFERS {
                if tx.try_send(Vec::with_capacity(cap)).is_err() {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Declare the OS-clock instant the capture stream was started (same clock
    /// as the block timestamps). The recording begins at this instant: if the
    /// first block starts later, the difference is filled with silence. Can be
    /// called again (for a retried start) until the first block arrives.
    pub fn mark_start(&self, at: MediaTime) {
        self.mark_start_since(at, Instant::now());
    }

    /// Like [`CapturePipeline::mark_start`], for a backend that declares the
    /// start instant later than it happened: `wall` is the wall-clock moment
    /// of `at` (used by the head-fill guard).
    pub fn mark_start_since(&self, at: MediaTime, wall: Instant) {
        let mut p = self.lock_producer();
        if p.seen_block {
            return;
        }
        p.start_mark = Some(Mark { at, wall });
        self.counters
            .start_secs
            .store(at.seconds().to_bits(), Ordering::Relaxed);
    }

    /// Declare the OS-clock instant the user stopped the recording. At
    /// [`CapturePipeline::finish`], if the last block ended earlier, the
    /// difference is filled with silence. Blocks may still arrive after this
    /// call (audio captured before the stop instant, still in the OS buffer).
    pub fn mark_stop(&self, at: MediaTime) {
        let mut p = self.lock_producer();
        p.stop_mark = Some(Mark {
            at,
            wall: Instant::now(),
        });
        self.counters
            .stop_secs
            .store(at.seconds().to_bits(), Ordering::Relaxed);
    }

    /// Add one block of interleaved samples. `start` is the OS timestamp of
    /// its first frame, if the OS provides one.
    pub fn push(&self, start: Option<MediaTime>, interleaved: &[f32]) -> Result<(), RecorderError> {
        let Some(format) = self.format.get() else {
            return Err(RecorderError::Format("push() before set_format()".into()));
        };
        let channels = usize::from(format.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(RecorderError::Format(format!(
                "block of {} samples is not a whole number of {channels}-channel frames",
                interleaved.len()
            )));
        }
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let frames = (interleaved.len() / channels) as u64;
        self.meter.process(interleaved);
        self.enqueue(start, frames, format.sample_rate, Some(interleaved));
        Ok(())
    }

    /// Add `frames` frames that the OS flagged as silent (WASAPI's
    /// `AUDCLNT_BUFFERFLAGS_SILENT`): written as digital silence.
    pub fn push_silence(&self, start: Option<MediaTime>, frames: u64) -> Result<(), RecorderError> {
        let Some(format) = self.format.get() else {
            return Err(RecorderError::Format(
                "push_silence() before set_format()".into(),
            ));
        };
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.enqueue(start, frames, format.sample_rate, None);
        Ok(())
    }

    fn enqueue(&self, start: Option<MediaTime>, frames: u64, rate: u32, samples: Option<&[f32]>) {
        let mut p = self.lock_producer();
        if p.tx.is_none() {
            return; // closed
        }
        let c = &self.counters;
        c.blocks.fetch_add(1, Ordering::Relaxed);
        c.received_frames.fetch_add(frames, Ordering::Relaxed);
        let first = !p.seen_block;
        p.seen_block = true;

        // --- timeline -----------------------------------------------------
        let mut gap: u64 = 0;
        if let Some(t) = start {
            let at = t.to_frames(rate);
            if first {
                // Head: from the start instant to the first block.
                let mut anchor = at;
                if let Some(m) = p.start_mark {
                    let s = m.at.to_frames(rate);
                    let lead = at - s;
                    let limit = frames_in(
                        rate,
                        Instant::now().duration_since(m.wall) + self.opts.edge_slack,
                    );
                    let slack = frames_in(rate, self.opts.edge_slack);
                    if lead > limit || -lead > slack {
                        // The start instant is not on the blocks' clock.
                        p.edges_trusted = Some(false);
                        c.edge_fill_refused.fetch_add(1, Ordering::Relaxed);
                    } else {
                        p.edges_trusted = Some(true);
                        if lead > 0 {
                            gap = lead as u64;
                            anchor = s;
                            c.head_fill_frames.fetch_add(gap, Ordering::Relaxed);
                        }
                    }
                    // A first block that starts slightly before the start
                    // instant keeps all its audio (nothing to fill).
                }
                c.span_first.store(anchor as i64, Ordering::Relaxed);
            } else if let Some(expected) = p.expected_next {
                let d = at - expected;
                let threshold = (i128::from(rate) * self.opts.gap_threshold.as_micros() as i128
                    / 1_000_000)
                    .max(i128::from(frames / 2))
                    .max(1);
                let max_fill = frames_in(rate, self.opts.max_gap_fill.max(Duration::from_secs(1)));
                if d > threshold {
                    if d <= max_fill {
                        gap = d as u64;
                        c.gap_events.fetch_add(1, Ordering::Relaxed);
                        c.gap_frames.fetch_add(gap, Ordering::Relaxed);
                    } else {
                        c.timestamp_resets.fetch_add(1, Ordering::Relaxed);
                    }
                } else if d < -threshold {
                    if -d <= max_fill {
                        c.overlap_events.fetch_add(1, Ordering::Relaxed);
                        c.overlap_frames.fetch_add((-d) as u64, Ordering::Relaxed);
                    } else {
                        c.timestamp_resets.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    c.max_jitter_frames
                        .fetch_max(d.unsigned_abs() as u64, Ordering::Relaxed);
                }
            } else if c.span_first.load(Ordering::Relaxed) == UNSET {
                // Earlier blocks had no timestamps: the span starts here.
                c.span_first.store(at as i64, Ordering::Relaxed);
            }
            // Re-anchor on the OS clock every block (see module docs).
            let end = at + i128::from(frames);
            p.expected_next = Some(end);
            c.span_end.store(end as i64, Ordering::Relaxed);
        } else if let Some(expected) = p.expected_next {
            // A block without a timestamp still takes its own length of OS
            // time. Move the expectation past it, or the next timed block
            // would see that span as a hole and fill it with silence again.
            // (`span_end` stays at the last real timestamp: the OS-clock span
            // in the stats only ever uses timestamps the OS gave us.)
            p.expected_next = Some(expected + i128::from(frames));
        } else if first && let Some(m) = p.start_mark {
            // An untimed first block: assume it begins at the start instant.
            let s = m.at.to_frames(rate);
            p.expected_next = Some(s + i128::from(frames));
            c.span_first.store(s as i64, Ordering::Relaxed);
        }
        c.frames.fetch_add(gap + frames, Ordering::Relaxed);

        // --- hand over to the writer -------------------------------------
        let owed = p.owed_silence + gap;
        p.owed_silence = 0;
        let tx = p.tx.as_ref().expect("checked above").clone();
        if owed > 0
            && let Err(TrySendError::Full(_)) = tx.try_send(Msg::Silence(owed))
        {
            p.owed_silence += owed;
        }
        if p.owed_silence > 0 {
            // Keep the order: the owed silence must reach the file first. Treat
            // this block as dropped too, so the timeline stays exact.
            if samples.is_some() {
                c.dropped_frames.fetch_add(frames, Ordering::Relaxed);
            }
            p.owed_silence += frames;
            return;
        }
        let msg = match samples {
            Some(s) => {
                // Reuse a buffer the writer has finished with: no allocation
                // once the pool has warmed up.
                let mut buf = p.recycled.try_recv().unwrap_or_default();
                buf.clear();
                buf.extend_from_slice(s);
                Msg::Samples(buf)
            }
            None => Msg::Silence(frames),
        };
        if let Err(TrySendError::Full(_)) = tx.try_send(msg) {
            if samples.is_some() {
                c.dropped_frames.fetch_add(frames, Ordering::Relaxed);
            }
            p.owed_silence += frames;
        }
    }

    /// Record an error that does not end the recording by itself. The first
    /// error wins. (Use [`CapturePipeline::close_with_error`] when the OS
    /// stream has stopped.)
    pub fn fail(&self, message: impl Into<String>) {
        let mut e = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if e.is_none() {
            *e = Some(message.into());
        }
    }

    /// The recording cannot continue (device removed, format changed, OS
    /// stopped the stream): record the error, stop accepting audio and let the
    /// writer finish the file now, so it is complete and valid on disk even
    /// before `finish()` is called. The state becomes `Failed(message)`.
    pub fn close_with_error(&self, message: impl Into<String>) {
        self.fail(message);
        let mut p = self.lock_producer();
        self.close_locked(&mut p);
    }

    /// True once no more audio is accepted (after `close_with_error` or `finish`).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn close_locked(&self, p: &mut Producer) {
        if let Some(tx) = p.tx.take() {
            // Pay any silence still owed (blocking send: closing is never on
            // the steady-state capture path).
            if p.owed_silence > 0 {
                let owed = std::mem::take(&mut p.owed_silence);
                let _ = tx.send(Msg::Silence(owed));
            }
            // Dropping the sender lets the writer drain the queue and finish
            // the file.
        }
        self.closed.store(true, Ordering::Release);
    }

    /// Tail: extend the timeline from the end of the last block to the stop
    /// instant (see the module documentation).
    fn extend_to_stop(&self, p: &mut Producer) {
        let (Some(stop), Some(format)) = (p.stop_mark, self.format.get()) else {
            return;
        };
        let rate = format.sample_rate;
        let c = &self.counters;
        if p.edges_trusted == Some(false) {
            // The start instant disagreed with the blocks' clock; the stop
            // instant comes from the same clock, so it is not used either.
            c.edge_fill_refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Where the timeline ends now. With no block at all, it is empty at
        // the start instant.
        let end = match p.expected_next {
            Some(e) => e,
            None => match p.start_mark {
                Some(s) if !p.seen_block => s.at.to_frames(rate),
                _ => return,
            },
        };
        let stop_frames = stop.at.to_frames(rate);
        let tail = stop_frames - end;
        if tail <= 0 {
            return; // the last block reached (or passed) the stop instant
        }
        // Guard: the tail can never be longer than the whole recording took on
        // the wall clock (Start, or pipeline creation, to Stop).
        let since = p.start_mark.map_or(self.created, |s| s.wall);
        let limit = frames_in(
            rate,
            stop.wall.saturating_duration_since(since) + self.opts.edge_slack,
        );
        if tail > limit {
            c.edge_fill_refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let tail = tail as u64;
        p.owed_silence += tail;
        c.tail_fill_frames.fetch_add(tail, Ordering::Relaxed);
        c.frames.fetch_add(tail, Ordering::Relaxed);
        if c.span_first.load(Ordering::Relaxed) == UNSET {
            c.span_first.store(end as i64, Ordering::Relaxed);
        }
        c.span_end.store(stop_frames as i64, Ordering::Relaxed);
    }

    /// Current state.
    pub fn state(&self) -> PipelineState {
        if self.finished.load(Ordering::Acquire) {
            return PipelineState::Finished;
        }
        if let Some(e) = self.error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return PipelineState::Failed(e);
        }
        if self.format.get().is_some() {
            PipelineState::Recording
        } else {
            PipelineState::WaitingForAudio
        }
    }

    /// Meter values; resets the "recent" peaks (call this from the UI poll).
    pub fn take_levels(&self) -> Levels {
        self.meter.take_levels()
    }

    /// Meter values without resetting anything.
    pub fn peek_levels(&self) -> Levels {
        self.meter.peek_levels()
    }

    /// A snapshot of every counter. Reads atomics only.
    pub fn stats(&self) -> RecordingStats {
        let c = &self.counters;
        let format = self.format.get().cloned();
        let frames = c.frames.load(Ordering::Relaxed);
        let rate = format.as_ref().map(|f| f.sample_rate).unwrap_or(0);
        let (a, b) = (
            c.span_first.load(Ordering::Relaxed),
            c.span_end.load(Ordering::Relaxed),
        );
        let os_span = if rate > 0 && a != UNSET && b != UNSET && b > a {
            Some((b - a) as f64 / f64::from(rate))
        } else {
            None
        };
        let start = f64::from_bits(c.start_secs.load(Ordering::Relaxed));
        let stop = f64::from_bits(c.stop_secs.load(Ordering::Relaxed));
        let start_to_stop = (start.is_finite() && stop.is_finite()).then_some(stop - start);
        RecordingStats {
            path: self.path.clone(),
            format,
            wall_elapsed: self.created.elapsed(),
            frames,
            media_elapsed_secs: if rate > 0 {
                frames as f64 / f64::from(rate)
            } else {
                0.0
            },
            os_clock_elapsed_secs: os_span,
            start_to_stop_secs: start_to_stop,
            received_frames: c.received_frames.load(Ordering::Relaxed),
            frames_written: c.frames_written.load(Ordering::Relaxed),
            file_bytes: c.file_bytes.load(Ordering::Relaxed),
            rf64: c.rf64.load(Ordering::Relaxed),
            blocks: c.blocks.load(Ordering::Relaxed),
            clipped_samples: self.meter.clipped_samples(),
            non_finite_samples: self.meter.non_finite_samples(),
            head_fill_frames: c.head_fill_frames.load(Ordering::Relaxed),
            gap_events: c.gap_events.load(Ordering::Relaxed),
            gap_frames: c.gap_frames.load(Ordering::Relaxed),
            tail_fill_frames: c.tail_fill_frames.load(Ordering::Relaxed),
            edge_fill_refused: c.edge_fill_refused.load(Ordering::Relaxed),
            overlap_events: c.overlap_events.load(Ordering::Relaxed),
            overlap_frames: c.overlap_frames.load(Ordering::Relaxed),
            timestamp_resets: c.timestamp_resets.load(Ordering::Relaxed),
            max_jitter_frames: c.max_jitter_frames.load(Ordering::Relaxed),
            dropped_frames: c.dropped_frames.load(Ordering::Relaxed),
            format_mismatches: c.format_mismatches.load(Ordering::Relaxed),
            error: self.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    /// Stop accepting audio, extend the timeline to the stop instant, let the
    /// writer drain the queue, write the final header and close the file.
    /// Safe to call once; later calls return [`RecorderError::AlreadyStopped`].
    /// After `close_with_error` it just waits for the file to be finished.
    pub fn finish(&self) -> Result<RecordingSummary, RecorderError> {
        if self.finish_called.swap(true, Ordering::AcqRel) {
            return Err(RecorderError::AlreadyStopped);
        }
        {
            let mut p = self.lock_producer();
            if p.tx.is_some() {
                self.extend_to_stop(&mut p);
                self.close_locked(&mut p);
            }
        }
        let handle = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take();
        let file = match handle {
            Some(h) => h.join().unwrap_or_else(|_| {
                self.fail("the recording writer thread panicked");
                None
            }),
            None => None,
        };
        self.finished.store(true, Ordering::Release);
        Ok(RecordingSummary {
            stats: self.stats(),
            file,
        })
    }
}

fn set_error(error: &Mutex<Option<String>>, msg: String) {
    let mut e = error.lock().unwrap_or_else(|e| e.into_inner());
    if e.is_none() {
        *e = Some(msg);
    }
}

/// The writer thread: owns the file; checkpoints the header about once a
/// second; syncs to the device every few seconds; keeps draining the queue
/// even after a write error so the capture side never blocks; hands sample
/// buffers back for reuse.
fn writer_thread(
    path: &Path,
    opts: &PipelineOptions,
    rx: Receiver<Msg>,
    recycle: &SyncSender<Vec<f32>>,
    counters: &Counters,
    error: &Mutex<Option<String>>,
) -> Option<WavSummary> {
    let mut writer: Option<WavWriter> = None;
    let mut failed = false;
    let mut last_checkpoint = Instant::now();
    let mut last_sync = Instant::now();
    loop {
        let msg = match rx.recv_timeout(opts.checkpoint_every) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Some(msg) = msg {
            let result = if failed {
                Ok(())
            } else {
                match (&msg, writer.as_mut()) {
                    (Msg::Begin(spec), None) => match WavWriter::create(path, *spec) {
                        Ok(w) => {
                            writer = Some(w);
                            Ok(())
                        }
                        Err(e) => Err(format!("could not create {}: {e}", path.display())),
                    },
                    (Msg::Begin(_), Some(_)) => Ok(()),
                    (Msg::Samples(s), Some(w)) => w
                        .write_interleaved(s)
                        .map_err(|e| format!("could not write to {}: {e}", path.display())),
                    (Msg::Silence(n), Some(w)) => w
                        .write_silence(*n)
                        .map_err(|e| format!("could not write to {}: {e}", path.display())),
                    (_, None) => {
                        Err("audio arrived before the format (internal error)".to_string())
                    }
                }
            };
            if let Msg::Samples(buf) = msg {
                // Back to the capture side; if the pool is full it is freed here.
                let _ = recycle.try_send(buf);
            }
            if let Err(e) = result {
                set_error(error, e);
                failed = true;
            }
        }
        if let Some(w) = writer.as_mut() {
            counters
                .frames_written
                .store(w.frames_written(), Ordering::Relaxed);
            counters.file_bytes.store(w.file_bytes(), Ordering::Relaxed);
            if !failed && last_checkpoint.elapsed() >= opts.checkpoint_every {
                last_checkpoint = Instant::now();
                let r = if last_sync.elapsed() >= opts.sync_every {
                    last_sync = Instant::now();
                    w.sync()
                } else {
                    w.checkpoint()
                };
                if let Err(e) = r {
                    set_error(error, format!("could not update {}: {e}", path.display()));
                    failed = true;
                }
                counters.rf64.store(w.is_rf64(), Ordering::Relaxed);
            }
        }
    }
    let w = writer?;
    match w.finalize() {
        Ok(s) => {
            counters.frames_written.store(s.frames, Ordering::Relaxed);
            counters.file_bytes.store(s.file_bytes, Ordering::Relaxed);
            counters.rf64.store(s.rf64, Ordering::Relaxed);
            Some(s)
        }
        Err(e) => {
            set_error(error, format!("could not finish {}: {e}", path.display()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::SourceEncoding;
    use crate::wav::read_wav_f32;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "jk-recorder-pipeline-{}-{name}.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn fmt48() -> CaptureFormat {
        CaptureFormat {
            sample_rate: 48_000,
            channels: 2,
            source: SourceEncoding::Float32 { interleaved: false },
        }
    }

    /// CMTime-style timestamp: host time in nanoseconds.
    fn ns(frames_at_48k: i64) -> Option<MediaTime> {
        Some(MediaTime {
            value: frames_at_48k * 1_000_000_000 / 48_000,
            timescale: 1_000_000_000,
        })
    }

    /// QPC-style timestamp (100 ns units, as WASAPI reports), offset by a
    /// realistic uptime so the values are large.
    fn qpc(frames_at_48k: i64) -> MediaTime {
        MediaTime {
            value: 3_600 * 10_000_000 + frames_at_48k * 10_000_000 / 48_000,
            timescale: 10_000_000,
        }
    }

    fn block(frames: usize, value: f32) -> Vec<f32> {
        vec![value; frames * 2]
    }

    #[test]
    fn media_time_rounding_is_exact() {
        let t = MediaTime {
            value: 1_000_000_000,
            timescale: 1_000_000_000,
        };
        assert_eq!(t.to_frames(48_000), 48_000);
        let t = MediaTime {
            value: 10_416_667,
            timescale: 1_000_000_000,
        }; // 500.000016 frames
        assert_eq!(t.to_frames(48_000), 500);
        let t = MediaTime {
            value: -1,
            timescale: 3,
        };
        assert_eq!(t.to_frames(3), -1);
    }

    #[test]
    fn missing_block_is_filled_with_exact_silence() {
        let p = temp_path("gap");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        // Blocks of 1024 frames; the block at 2048 never arrives.
        pl.push(ns(0), &block(1024, 0.25)).unwrap();
        pl.push(ns(1024), &block(1024, 0.25)).unwrap();
        pl.push(ns(3072), &block(1024, 0.5)).unwrap();
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.gap_events, 1);
        assert_eq!(s.stats.gap_frames, 1024);
        assert_eq!(s.stats.frames, 4096);
        assert_eq!(s.stats.received_frames, 3072);
        assert_eq!(s.file.unwrap().frames, 4096);
        assert_eq!(s.stats.os_clock_elapsed_secs, Some(4096.0 / 48_000.0));
        assert_eq!(s.stats.frames_per_os_second_ratio(), Some(1.0));
        let (_, data) = read_wav_f32(&p).unwrap();
        assert!(data[..4096].iter().all(|&x| x == 0.25));
        assert!(data[4096..6144].iter().all(|&x| x == 0.0));
        assert!(data[6144..].iter().all(|&x| x == 0.5));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn jitter_and_slow_drift_never_insert_samples() {
        let p = temp_path("drift");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        // 2000 blocks (about 43 s). The OS clock runs 100 ppm fast relative to
        // the sample clock, plus +-3 frames of jitter: drift accumulates to
        // about 205 frames (4 ms) over the run and must never become a "gap".
        let mut t = 0.0f64;
        for i in 0..2000 {
            let jitter = [0.0, 3.0, -3.0, 1.0][i % 4];
            pl.push(ns((t + jitter).round() as i64), &block(1024, 0.1))
                .unwrap();
            t += 1024.0 * 1.0001;
        }
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.gap_events, 0);
        assert_eq!(s.stats.overlap_events, 0);
        assert_eq!(s.stats.frames, 2000 * 1024);
        assert_eq!(s.file.unwrap().frames, 2000 * 1024);
        assert!(
            s.stats.max_jitter_frames <= 7,
            "jitter {}",
            s.stats.max_jitter_frames
        );
        let ratio = s.stats.frames_per_os_second_ratio().unwrap();
        assert!((ratio - 1.0 / 1.0001).abs() < 1e-5, "ratio {ratio}");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn silent_flag_and_untimed_blocks() {
        let p = temp_path("silent");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.push(None, &block(480, 0.75)).unwrap();
        pl.push_silence(None, 480).unwrap();
        pl.push(None, &block(480, -0.75)).unwrap();
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.frames, 1440);
        assert_eq!(s.stats.os_clock_elapsed_secs, None);
        let (_, data) = read_wav_f32(&p).unwrap();
        assert_eq!(data.len(), 2880);
        assert!(data[960..1920].iter().all(|&x| x == 0.0));
        assert!(data[1920..].iter().all(|&x| x == -0.75));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn an_untimed_block_between_timed_ones_is_not_counted_twice() {
        // A block without a timestamp still occupies its own length on the OS
        // clock. The next timed block must not see that span as a "hole" and
        // fill it with silence a second time.
        let p = temp_path("untimed-mid");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.push(ns(0), &block(1024, 0.25)).unwrap();
        pl.push(None, &block(1024, 0.5)).unwrap();
        pl.push(ns(2048), &block(1024, 0.75)).unwrap();
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.gap_events, 0, "gap frames {}", s.stats.gap_frames);
        assert_eq!(s.stats.frames, 3072);
        assert_eq!(s.file.unwrap().frames, 3072);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn format_change_stops_the_recording_visibly_and_closes_the_file() {
        let p = temp_path("fmtchange");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        assert!(pl.set_format(fmt48()).is_ok());
        pl.push(ns(0), &block(1024, 0.25)).unwrap();
        assert!(pl.push(None, &[0.0; 3]).is_err()); // not whole frames
        let mut other = fmt48();
        other.sample_rate = 44_100;
        let err = pl.set_format(other).unwrap_err().to_string();
        assert!(
            err.contains("48000 Hz") && err.contains("44100 Hz"),
            "{err}"
        );
        // Visible: state Failed with the same plain message.
        match pl.state() {
            PipelineState::Failed(m) => assert!(m.contains("changed during the recording"), "{m}"),
            s => panic!("state {s:?}"),
        }
        assert!(pl.is_closed());
        // Audio after the change is not written.
        pl.push(ns(1024), &block(1024, 0.5)).unwrap();
        // The file is complete on disk BEFORE finish(): the writer closes it on
        // its own once the pipeline is closed.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut header_ok = false;
        while Instant::now() < deadline {
            if let Ok(info) = crate::wav::read_header(&p)
                && info.frames == 1024
            {
                header_ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(header_ok, "file was not finalized after the format change");
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.format_mismatches, 1);
        assert_eq!(s.file.unwrap().frames, 1024);
        assert!(s.stats.error.unwrap().contains("44100 Hz"));
        assert!(pl.finish().is_err(), "second finish must fail");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn close_with_error_finalizes_and_reports() {
        // A backend's "device removed" path.
        let p = temp_path("devlost");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.mark_start(qpc(0));
        pl.push(Some(qpc(0)), &block(480, 0.1)).unwrap();
        pl.close_with_error("the output device was removed");
        pl.push(Some(qpc(480)), &block(480, 0.1)).unwrap(); // ignored
        pl.mark_stop(qpc(48_000)); // no tail after an error: the file ends at the loss
        let s = pl.finish().unwrap();
        assert_eq!(s.file.unwrap().frames, 480);
        assert_eq!(s.stats.tail_fill_frames, 0);
        assert_eq!(
            s.stats.error.as_deref(),
            Some("the output device was removed")
        );
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn no_format_means_no_file() {
        let p = temp_path("nothing");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        assert_eq!(pl.state(), PipelineState::WaitingForAudio);
        let s = pl.finish().unwrap();
        assert!(s.file.is_none());
        assert!(!p.exists());
        assert_eq!(pl.state(), PipelineState::Finished);
    }

    #[test]
    fn queue_overflow_keeps_timeline_and_counts_loss() {
        let p = temp_path("overflow");
        // A queue of 2 blocks; the writer is slower than 10 instant pushes
        // only if it cannot drain in time, so the exact loss is racy. What
        // must always hold: frames in the file == frames in the timeline.
        let opts = PipelineOptions {
            queue_blocks: 2,
            ..PipelineOptions::default()
        };
        let pl = CapturePipeline::new(&p, opts).unwrap();
        pl.set_format(fmt48()).unwrap();
        for i in 0..200 {
            pl.push(ns(i * 4800), &block(4800, 0.2)).unwrap();
        }
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.frames, 200 * 4800);
        assert_eq!(
            s.file.unwrap().frames,
            200 * 4800,
            "dropped {}",
            s.stats.dropped_frames
        );
        let (_, data) = read_wav_f32(&p).unwrap();
        let zeros = data.iter().filter(|&&x| x == 0.0).count() as u64;
        assert_eq!(zeros, s.stats.dropped_frames * 2);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn recording_covers_start_to_first_block_and_last_block_to_stop() {
        // Stage 0 verifier finding: started and stopped during silence, a
        // loopback capture delivers its first block late and its last block
        // early. The file must still span Start..Stop exactly.
        let p = temp_path("edges");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.mark_start(qpc(0));
        // First block 0.25 s after Start (12_000 frames), then 10 blocks.
        for i in 0..10 {
            pl.push(Some(qpc(12_000 + i * 480)), &block(480, 0.5))
                .unwrap();
        }
        // Last block ends at 16_800; Stop at 0.5 s = 24_000.
        pl.mark_stop(qpc(24_000));
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.head_fill_frames, 12_000);
        assert_eq!(s.stats.tail_fill_frames, 24_000 - 16_800);
        assert_eq!(s.stats.received_frames, 4_800);
        assert_eq!(s.stats.frames, 24_000);
        assert_eq!(s.file.unwrap().frames, 24_000);
        assert_eq!(s.stats.edge_fill_refused, 0);
        let st = s.stats.start_to_stop_secs.unwrap();
        assert!((st - 0.5).abs() < 1e-6, "{st}");
        assert!((s.stats.length_vs_start_stop_ratio().unwrap() - 1.0).abs() < 1e-5);
        assert_eq!(s.stats.frames_per_os_second_ratio(), Some(1.0));
        let (_, data) = read_wav_f32(&p).unwrap();
        assert!(data[..24_000].iter().all(|&x| x == 0.0)); // head: 12_000 frames x 2 ch
        assert!(data[24_000..33_600].iter().all(|&x| x == 0.5));
        assert!(data[33_600..].iter().all(|&x| x == 0.0)); // tail
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn start_and_stop_in_silence_with_no_block_at_all_gives_full_length() {
        // Windows declares the format at start; if the system stays silent and
        // nothing arrives, the file is still Start..Stop long.
        let p = temp_path("allsilent");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.mark_start(qpc(0));
        pl.mark_stop(qpc(9_600));
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.received_frames, 0);
        assert_eq!(s.stats.tail_fill_frames, 9_600);
        assert_eq!(s.file.unwrap().frames, 9_600);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn a_long_silent_tail_is_filled_even_if_the_last_block_was_pushed_late() {
        // Measured on Windows with the keep-alive off: no packets for the last
        // 2 s before Stop. The backend holds the last packet back and pushes it
        // at Stop, so the guard must not compare the tail with the time since
        // that push. Start was 3 s ago (wall clock); blocks cover 0.5..1.0 s;
        // Stop at 3.0 s: 2 s of tail.
        let p = temp_path("longtail");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        let start_wall = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
        pl.mark_start_since(qpc(0), start_wall);
        for i in 0..50 {
            pl.push(Some(qpc(24_000 + i * 480)), &block(480, 0.25))
                .unwrap();
        }
        pl.mark_stop(qpc(144_000));
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.edge_fill_refused, 0);
        assert_eq!(s.stats.head_fill_frames, 24_000);
        assert_eq!(s.stats.tail_fill_frames, 144_000 - 48_000);
        assert_eq!(s.file.unwrap().frames, 144_000);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn a_first_block_before_the_start_instant_keeps_its_audio() {
        let p = temp_path("early");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.mark_start(qpc(100));
        pl.push(Some(qpc(0)), &block(480, 0.5)).unwrap();
        pl.mark_stop(qpc(480));
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.head_fill_frames, 0);
        assert_eq!(s.stats.tail_fill_frames, 0);
        assert_eq!(s.stats.edge_fill_refused, 0);
        assert_eq!(s.file.unwrap().frames, 480);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn edges_on_a_different_clock_are_refused_not_filled() {
        // If the start/stop instants were not on the blocks' clock, the
        // "head" would be absurd (here 1 hour). It must be refused and
        // counted, never written as an hour of silence.
        let p = temp_path("wrongclock");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        pl.mark_start(MediaTime {
            value: 0,
            timescale: 10_000_000,
        });
        pl.push(Some(qpc(0)), &block(480, 0.5)).unwrap();
        pl.mark_stop(MediaTime {
            value: 2 * 3_600 * 10_000_000,
            timescale: 10_000_000,
        });
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.head_fill_frames, 0);
        assert_eq!(s.stats.tail_fill_frames, 0);
        assert_eq!(s.stats.edge_fill_refused, 2);
        assert_eq!(s.file.unwrap().frames, 480);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn channel_mask_reaches_the_file() {
        let p = temp_path("mask");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_channel_mask(0x3F); // 5.1
        let f = CaptureFormat {
            sample_rate: 48_000,
            channels: 6,
            source: SourceEncoding::Float32 { interleaved: true },
        };
        pl.set_format(f).unwrap();
        pl.push(None, &[0.0; 12]).unwrap();
        let s = pl.finish().unwrap();
        assert_eq!(s.file.unwrap().frames, 2);
        let info = crate::wav::read_header(&p).unwrap();
        assert_eq!(info.channels, 6);
        // WAVE_FORMAT_EXTENSIBLE: dwChannelMask sits 20 bytes into the fmt body.
        let bytes = std::fs::read(&p).unwrap();
        let fmt = bytes.windows(4).position(|w| w == b"fmt ").unwrap() + 8;
        let mask = u32::from_le_bytes(bytes[fmt + 20..fmt + 24].try_into().unwrap());
        assert_eq!(mask, 0x3F);
        std::fs::remove_file(&p).unwrap();
    }
}

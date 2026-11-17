// SPDX-License-Identifier: GPL-3.0-or-later
//! The platform-independent half of the recorder: timeline, meter and writer.
//!
//! A platform backend (ScreenCaptureKit, WASAPI loopback) only has to:
//!
//! 1. call [`CapturePipeline::set_format`] once it knows the real format of
//!    the audio the OS delivers;
//! 2. call [`CapturePipeline::push`] with every block of interleaved `f32`
//!    samples and the OS timestamp of its first frame (or
//!    [`CapturePipeline::push_silence`] for blocks the OS flags as silent);
//! 3. call [`CapturePipeline::fail`] if the OS stops the stream on its own.
//!
//! The pipeline then meters the audio, keeps the recording's sample count in
//! step with the OS clock, and writes a progressive float WAV/RF64 on its own
//! thread, so the capture callback never touches the disk.
//!
//! # Keeping the sample count equal to elapsed time
//!
//! WORK_ORDER section 7, trap 5: a loopback capture may deliver nothing while
//! the system is silent, and a recording built from "what arrived" comes out
//! shorter than real time. Each block's OS timestamp is compared with where the
//! previous block ended:
//!
//! * a jump forward larger than the gap threshold (10 ms, or half a block if
//!   that is longer) is a real hole, and it is filled with digital silence of
//!   exactly that many frames;
//! * smaller differences are clock jitter or slow drift between the OS host
//!   clock and the audio device clock. They are only measured, never "fixed"
//!   by inserting or dropping samples, because doing so every few minutes
//!   would put tiny dropouts into a perfectly good recording. Re-anchoring on
//!   every block keeps drift from accumulating into a false gap.
//!
//! If the writer cannot keep up (very slow USB stick) and the queue overflows,
//! the lost frames are replaced by silence of the same length (so the timeline
//! stays true) and counted in `dropped_frames`, which the UI must show as an
//! error: audio was lost.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::RecorderError;
use crate::format::CaptureFormat;
use crate::meter::{LevelMeter, Levels};
use crate::wav::{WavSpec, WavSummary, WavWriter};

/// A timestamp on the OS's monotonic media clock: `value / timescale` seconds.
/// macOS gives `CMTime` (value, timescale); Windows can pass QPC ticks with
/// `timescale = 10_000_000` (100 ns units) or the QPC frequency.
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
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            gap_threshold: Duration::from_millis(10),
            max_gap_fill: Duration::from_secs(600),
            queue_blocks: 4096,
            checkpoint_every: Duration::from_secs(1),
            sync_every: Duration::from_secs(5),
        }
    }
}

/// Everything measurable about a recording, for the UI, the self-check and CI.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingStats {
    /// The file being written.
    pub path: PathBuf,
    /// The real format, once the first buffer arrived.
    pub format: Option<CaptureFormat>,
    /// Wall-clock time since the pipeline was created.
    pub wall_elapsed: Duration,
    /// Frames in the recording's timeline (audio received + silence inserted
    /// for gaps and for dropped blocks).
    pub frames: u64,
    /// `frames / sample_rate`: the recording's length so far, in seconds.
    pub media_elapsed_secs: f64,
    /// Span of the OS timestamps (first block start to last block end), in
    /// seconds, when the OS provides timestamps.
    pub os_clock_elapsed_secs: Option<f64>,
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
    /// Holes in the OS timestamps that were filled with silence.
    pub gap_events: u64,
    /// Total frames of silence inserted for those holes.
    pub gap_frames: u64,
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
    /// Buffers whose format differed from the first one (skipped).
    pub format_mismatches: u64,
    /// The first error, if any (writer failure, OS stopped the stream, ...).
    pub error: Option<String>,
}

impl RecordingStats {
    /// True if anything reached full scale (the UI's clip warning).
    pub fn clip_warning(&self) -> bool {
        self.clipped_samples > 0
    }

    /// Recording length divided by the OS clock span: 1.0 means the sample
    /// count matches elapsed time exactly.
    pub fn frames_per_os_second_ratio(&self) -> Option<f64> {
        match (self.os_clock_elapsed_secs, &self.format) {
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

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    frames_written: AtomicU64,
    file_bytes: AtomicU64,
    rf64: AtomicBool,
    blocks: AtomicU64,
    gap_events: AtomicU64,
    gap_frames: AtomicU64,
    overlap_events: AtomicU64,
    overlap_frames: AtomicU64,
    timestamp_resets: AtomicU64,
    max_jitter_frames: AtomicU64,
    dropped_frames: AtomicU64,
    format_mismatches: AtomicU64,
}

/// Producer-side state, touched only by the capture callback (one thread at a
/// time; the mutex is uncontended) and by `finish()`.
struct Producer {
    tx: Option<SyncSender<Msg>>,
    /// Where the next block should start on the OS clock, in frames.
    expected_next: Option<i128>,
    first_frame: Option<i128>,
    last_end: Option<i128>,
    /// Frames owed to the writer after a queue overflow.
    owed_silence: u64,
}

/// See the module documentation.
pub struct CapturePipeline {
    path: PathBuf,
    opts: PipelineOptions,
    created: Instant,
    meter: LevelMeter,
    format: OnceLock<CaptureFormat>,
    producer: Mutex<Producer>,
    counters: Arc<Counters>,
    error: Arc<Mutex<Option<String>>>,
    writer: Mutex<Option<JoinHandle<Option<WavSummary>>>>,
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
    /// The finished file, or `None` if no audio ever arrived (no file is
    /// created in that case).
    pub file: Option<WavSummary>,
}

impl CapturePipeline {
    /// Create the pipeline and start its writer thread. The file is created
    /// when the first format arrives, and never overwrites an existing file.
    pub fn new(path: &Path, opts: PipelineOptions) -> Result<Arc<Self>, RecorderError> {
        let (tx, rx) = mpsc::sync_channel::<Msg>(opts.queue_blocks.max(1));
        let counters = Arc::new(Counters::default());
        let error = Arc::new(Mutex::new(None));
        let handle = {
            let path = path.to_path_buf();
            let counters = Arc::clone(&counters);
            let error = Arc::clone(&error);
            let opts = opts.clone();
            std::thread::Builder::new()
                .name("jk-recorder-writer".into())
                .spawn(move || writer_thread(&path, &opts, rx, &counters, &error))?
        };
        Ok(Arc::new(Self {
            path: path.to_path_buf(),
            opts,
            created: Instant::now(),
            meter: LevelMeter::new(),
            format: OnceLock::new(),
            producer: Mutex::new(Producer {
                tx: Some(tx),
                expected_next: None,
                first_frame: None,
                last_end: None,
                owed_silence: 0,
            }),
            counters,
            error,
            writer: Mutex::new(Some(handle)),
            finished: AtomicBool::new(false),
        }))
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The real capture format, once known.
    pub fn format(&self) -> Option<&CaptureFormat> {
        self.format.get()
    }

    /// Declare the real format of the audio. The first call creates the file;
    /// later calls with the same format are no-ops; a different format is an
    /// error (counted in `format_mismatches`; the backend must skip that data).
    pub fn set_format(&self, format: CaptureFormat) -> Result<(), RecorderError> {
        if let Some(existing) = self.format.get() {
            if *existing == format {
                return Ok(());
            }
            self.counters
                .format_mismatches
                .fetch_add(1, Ordering::Relaxed);
            return Err(RecorderError::Format(format!(
                "the OS changed the capture format mid-recording from {existing} to {format}; \
                 those buffers were skipped"
            )));
        }
        if format.channels == 0 || format.sample_rate == 0 {
            return Err(RecorderError::Format(format!("invalid format {format}")));
        }
        let spec = WavSpec::new(format.sample_rate, format.channels);
        self.meter.set_channels(usize::from(format.channels));
        // Only one thread can win the OnceLock; it sends Begin.
        if self.format.set(format).is_ok() {
            let p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tx) = &p.tx {
                // The queue is empty at this point, so this cannot be Full.
                let _ = tx.try_send(Msg::Begin(spec));
            }
        }
        Ok(())
    }

    /// Add one block of interleaved samples. `start` is the OS timestamp of
    /// its first frame, if the OS provides one.
    pub fn push(&self, start: Option<MediaTime>, interleaved: &[f32]) -> Result<(), RecorderError> {
        let Some(format) = self.format.get() else {
            return Err(RecorderError::Format("push() before set_format()".into()));
        };
        let channels = usize::from(format.channels);
        if interleaved.len() % channels != 0 {
            return Err(RecorderError::Format(format!(
                "block of {} samples is not a whole number of {channels}-channel frames",
                interleaved.len()
            )));
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
        self.enqueue(start, frames, format.sample_rate, None);
        Ok(())
    }

    fn enqueue(&self, start: Option<MediaTime>, frames: u64, rate: u32, samples: Option<&[f32]>) {
        let mut p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
        if p.tx.is_none() {
            return; // finished
        }
        self.counters.blocks.fetch_add(1, Ordering::Relaxed);

        // --- timeline -----------------------------------------------------
        let mut gap: u64 = 0;
        if let Some(t) = start {
            let at = t.to_frames(rate);
            let threshold = (i128::from(rate) * self.opts.gap_threshold.as_micros() as i128
                / 1_000_000)
                .max(i128::from(frames / 2))
                .max(1);
            let max_fill =
                i128::from(rate) * i128::from(self.opts.max_gap_fill.as_secs().max(1) as u32);
            if let Some(expected) = p.expected_next {
                let d = at - expected;
                if d > threshold {
                    if d <= max_fill {
                        gap = d as u64;
                        self.counters.gap_events.fetch_add(1, Ordering::Relaxed);
                        self.counters.gap_frames.fetch_add(gap, Ordering::Relaxed);
                    } else {
                        self.counters
                            .timestamp_resets
                            .fetch_add(1, Ordering::Relaxed);
                    }
                } else if d < -threshold {
                    if -d <= max_fill {
                        self.counters.overlap_events.fetch_add(1, Ordering::Relaxed);
                        self.counters
                            .overlap_frames
                            .fetch_add((-d) as u64, Ordering::Relaxed);
                    } else {
                        self.counters
                            .timestamp_resets
                            .fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    self.counters
                        .max_jitter_frames
                        .fetch_max(d.unsigned_abs() as u64, Ordering::Relaxed);
                }
            } else {
                p.first_frame = Some(at);
            }
            // Re-anchor on the OS clock every block (see module docs).
            p.expected_next = Some(at + i128::from(frames));
            p.last_end = p.expected_next;
        } else if let Some(expected) = p.expected_next {
            // A block without a timestamp still takes its own length of OS
            // time. Move the expectation past it, or the next timed block
            // would see that span as a hole and fill it with silence again.
            // (`last_end` stays at the last real timestamp: the OS-clock span
            // in the stats only ever uses timestamps the OS gave us.)
            p.expected_next = Some(expected + i128::from(frames));
        }
        self.counters
            .frames
            .fetch_add(gap + frames, Ordering::Relaxed);

        // --- hand over to the writer -------------------------------------
        let owed = p.owed_silence + gap;
        p.owed_silence = 0;
        let tx = p.tx.as_ref().expect("checked above").clone();
        if owed > 0 {
            if let Err(TrySendError::Full(_)) = tx.try_send(Msg::Silence(owed)) {
                p.owed_silence += owed;
            }
        }
        if p.owed_silence > 0 {
            // Keep the order: the owed silence must reach the file first. Treat
            // this block as dropped too, so the timeline stays exact.
            if samples.is_some() {
                self.counters
                    .dropped_frames
                    .fetch_add(frames, Ordering::Relaxed);
            }
            p.owed_silence += frames;
            return;
        }
        let msg = match samples {
            Some(s) => Msg::Samples(s.to_vec()),
            None => Msg::Silence(frames),
        };
        if let Err(TrySendError::Full(_)) = tx.try_send(msg) {
            if samples.is_some() {
                self.counters
                    .dropped_frames
                    .fetch_add(frames, Ordering::Relaxed);
            }
            p.owed_silence += frames;
        }
    }

    /// Record an error reported by the OS (for example the stream stopped
    /// because the user revoked the permission). The first error wins.
    pub fn fail(&self, message: impl Into<String>) {
        let mut e = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if e.is_none() {
            *e = Some(message.into());
        }
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

    /// A snapshot of every counter.
    pub fn stats(&self) -> RecordingStats {
        let c = &self.counters;
        let format = self.format.get().cloned();
        let frames = c.frames.load(Ordering::Relaxed);
        let rate = format.as_ref().map(|f| f.sample_rate).unwrap_or(0);
        let os_span = {
            let p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
            match (p.first_frame, p.last_end, rate) {
                (Some(a), Some(b), r) if r > 0 && b > a => Some((b - a) as f64 / f64::from(r)),
                _ => None,
            }
        };
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
            frames_written: c.frames_written.load(Ordering::Relaxed),
            file_bytes: c.file_bytes.load(Ordering::Relaxed),
            rf64: c.rf64.load(Ordering::Relaxed),
            blocks: c.blocks.load(Ordering::Relaxed),
            clipped_samples: self.meter.clipped_samples(),
            non_finite_samples: self.meter.non_finite_samples(),
            gap_events: c.gap_events.load(Ordering::Relaxed),
            gap_frames: c.gap_frames.load(Ordering::Relaxed),
            overlap_events: c.overlap_events.load(Ordering::Relaxed),
            overlap_frames: c.overlap_frames.load(Ordering::Relaxed),
            timestamp_resets: c.timestamp_resets.load(Ordering::Relaxed),
            max_jitter_frames: c.max_jitter_frames.load(Ordering::Relaxed),
            dropped_frames: c.dropped_frames.load(Ordering::Relaxed),
            format_mismatches: c.format_mismatches.load(Ordering::Relaxed),
            error: self.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    /// Stop accepting audio, let the writer drain the queue, write the final
    /// header and close the file. Safe to call once; later calls return
    /// [`RecorderError::AlreadyStopped`].
    pub fn finish(&self) -> Result<RecordingSummary, RecorderError> {
        {
            let mut p = self.producer.lock().unwrap_or_else(|e| e.into_inner());
            if p.tx.is_none() {
                return Err(RecorderError::AlreadyStopped);
            }
            // Pay any silence still owed after a queue overflow (blocking send:
            // we are not on the capture thread any more).
            if p.owed_silence > 0 {
                let owed = std::mem::take(&mut p.owed_silence);
                if let Some(tx) = &p.tx {
                    let _ = tx.send(Msg::Silence(owed));
                }
            }
            p.tx = None; // disconnects the writer once the queue is drained
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
/// even after a write error so the capture side never blocks.
fn writer_thread(
    path: &Path,
    opts: &PipelineOptions,
    rx: Receiver<Msg>,
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
            if failed {
                continue;
            }
            let result = match (msg, writer.as_mut()) {
                (Msg::Begin(spec), None) => match WavWriter::create(path, spec) {
                    Ok(w) => {
                        writer = Some(w);
                        Ok(())
                    }
                    Err(e) => Err(format!("could not create {}: {e}", path.display())),
                },
                (Msg::Begin(_), Some(_)) => Ok(()),
                (Msg::Samples(s), Some(w)) => w
                    .write_interleaved(&s)
                    .map_err(|e| format!("could not write to {}: {e}", path.display())),
                (Msg::Silence(n), Some(w)) => w
                    .write_silence(n)
                    .map_err(|e| format!("could not write to {}: {e}", path.display())),
                (_, None) => Err("audio arrived before the format (internal error)".to_string()),
            };
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
    fn format_change_is_rejected_and_counted() {
        let p = temp_path("fmtchange");
        let pl = CapturePipeline::new(&p, PipelineOptions::default()).unwrap();
        pl.set_format(fmt48()).unwrap();
        assert!(pl.set_format(fmt48()).is_ok());
        let mut other = fmt48();
        other.sample_rate = 24_000;
        assert!(pl.set_format(other).is_err());
        assert!(pl.push(None, &[0.0; 3]).is_err()); // not whole frames
        let s = pl.finish().unwrap();
        assert_eq!(s.stats.format_mismatches, 1);
        assert!(pl.finish().is_err(), "second finish must fail");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn no_audio_means_no_file() {
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
}

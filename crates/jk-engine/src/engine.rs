// SPDX-License-Identifier: GPL-3.0-or-later
//! The engine behind [`crate::api::Engine`] (docs/ARCHITECTURE.md section 4, "Threads").
//!
//! ```text
//!  API calls ──► Control (Mutex, never touched by audio threads)
//!                  │ commands (SPSC)             │ seek/stop (channel)
//!                  ▼                             ▼
//!  stem reader ──► stem ring ──► render worker ──► output ring + markers ──► device filler
//!  (I/O, decode)   (4 s)        (graph, resample,  (~150 ms ahead)           (callback)
//!                                peak/clip)
//!                  ▲ spare Rubber Band instances      │ garbage (old rings, songs,
//!  house thread ───┘ (built off the render thread)    ▼ retired instances) ──► house
//! ```
//!
//! * **Control** (any thread, under a mutex): opens songs and devices, allocates rings
//!   and graphs, and hands them to the worker as boxed commands. The worker never
//!   allocates or frees: replaced objects travel back to the house thread to be dropped.
//! * **Stem reader** (one per song): decodes the stems from the song file into one SPSC
//!   ring holding about 4 s, so a slow USB stick cannot starve playback.
//! * **Render worker** (MMCSS "Pro Audio" / QoS user-interactive): runs the jk-dsp graph
//!   and the output resampler and keeps the output ring ~150 ms ahead of the device.
//!   Output peak and clipping are measured here, not in the callback.
//! * **Device filler** (callback): see [`crate::output`].
//! * **House** (low priority): builds Rubber Band instances for upcoming key changes
//!   (constructing one allocates ~2.4 ms of work, never on the render thread), drops
//!   garbage, and turns callback/reader/device flags into events (ended, device lost,
//!   errors).
//!
//! **Seek** bumps a generation counter: a new stem ring is filled from a sample-accurate
//! decoder seek starting `prime_len` frames before the target (real audio primes the
//! pitch shifter), the worker resets the graph and discards the output before the
//! target, and the filler fades the old audio out, drops it and fades the new audio in
//! (10 ms each). The device stream keeps running.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jk_dsp::{ExtraBlock, GraphConfig, OutputResampler, PitchGraph, StemBlock, Stretcher, CHANNELS};

use crate::api::*;
use crate::output::{self, Capture, DeviceFeed, Marker, MarkerKind, OpenOutput, OutputRequest, StreamInfo};
use crate::shared::{now_ns, Shared, FILLER_SEG_BITS};
use crate::source::{slot_buffers, SlotBuffers, SongInfo, SongSource, SLOT_EXTRA_PITCHED, SLOT_EXTRA_UNPITCHED, SLOT_OTHER};

/// Graph block (song frames).
const BLOCK: usize = 512;
/// How far ahead of the device the worker keeps the output ring.
const AHEAD_MS: u64 = 150;
/// Decoded audio buffered per song (stem ring).
const STEM_BUFFER_S: f64 = 4.0;
/// Frames the reader decodes per step.
const READ_CHUNK: usize = 4096;
/// Spare Rubber Band instances the worker keeps in the graph.
const GRAPH_SPARES: usize = 2;

// =====================================================================================
// Messages

/// A segment: what to play from a load / seek / device switch onward.
pub(crate) struct Segment {
    generation: u64,
    /// First song frame to play.
    start: u64,
    /// First song frame in `ring` (`start - prime_len`, clamped at 0).
    read_from: u64,
    ring: rtrb::Consumer<f32>,
    eof: Arc<AtomicBool>,
}

/// The output side of the pipeline (one per open device).
pub(crate) struct OutputSide {
    prod: rtrb::Producer<f32>,
    markers: rtrb::Producer<Marker>,
    dev_rate: u32,
    dev_ch: usize,
    ahead_frames: usize,
    total_pushed: u64,
}

/// A loaded song as the worker sees it.
pub(crate) struct WorkerSong {
    graph: PitchGraph,
    resampler: OutputResampler,
    song_rate: u32,
    song_frames: u64,
    /// Slots present in the stem ring, in ring order.
    slots: Vec<usize>,
    extra: bool,
    seg: Option<Box<Segment>>,
    // Segment progress.
    needs_reset: bool,
    started: bool,
    ended: bool,
    reached_ahead: bool,
    song_in: u64,
    pushed_dev: u64,
    expected_dev: u64,
    rs_skip: usize,
    // Scratch (allocated with the song).
    bufs: SlotBuffers,
    gl: Vec<f32>,
    gr: Vec<f32>,
    pend: [Vec<f32>; CHANNELS],
    ilv: Vec<f32>,
    zeros: Vec<f32>,
}

pub(crate) enum WorkerCmd {
    Song(Box<WorkerSong>),
    Segment(Box<Segment>),
    Output(Box<OutputSide>, Option<Box<OutputResampler>>),
    Params(jk_dsp::MixParams),
    Unload,
}

/// Things the worker must not free itself.
#[allow(dead_code)] // the payloads are only dropped (by the house thread)
pub(crate) enum Garbage {
    Song(Box<WorkerSong>),
    Segment(Box<Segment>),
    Output(Box<OutputSide>),
    Resampler(Box<OutputResampler>),
    Stretcher(Stretcher),
}

enum ReaderCmd {
    Seek { read_from: u64, ring: rtrb::Producer<f32>, eof: Arc<AtomicBool> },
    Stop,
}

// =====================================================================================
// Stem reader

fn reader_thread(
    mut src: SongSource,
    slots: Vec<usize>,
    rx: crossbeam_channel::Receiver<ReaderCmd>,
    shared: Arc<Shared>,
    err_slot: Arc<Mutex<Option<EngineError>>>,
) {
    let width = slots.len() * CHANNELS;
    let mut bufs = slot_buffers(READ_CHUNK);
    let mut ring: Option<(rtrb::Producer<f32>, Arc<AtomicBool>)> = None;
    // Nothing to read until the first seek, after the end, or after an error.
    let mut done = true;
    loop {
        // Commands. Wait for one when there is nothing to do (idle, or the ring is full
        // enough); otherwise just poll.
        let busy = !done && ring.as_ref().is_some_and(|(p, _)| p.slots() >= READ_CHUNK * width);
        let cmd = if busy {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(_) => return,
            }
        } else {
            let wait = if done { 50 } else { 3 };
            match rx.recv_timeout(Duration::from_millis(wait)) {
                Ok(c) => Some(c),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(_) => return,
            }
        };
        match cmd {
            Some(ReaderCmd::Stop) => return,
            Some(ReaderCmd::Seek { read_from, ring: r, eof }) => {
                // The old ring's producer is dropped here (not real-time).
                done = false;
                if let Err(e) = src.seek(read_from) {
                    report(&shared, &err_slot, e);
                    eof.store(true, Release);
                    done = true;
                }
                ring = Some((r, eof));
                continue;
            }
            None => {}
        }
        if !busy {
            continue;
        }
        let Some((prod, eof)) = ring.as_mut() else { continue };
        match src.read(&mut bufs, READ_CHUNK) {
            Ok(0) => {
                eof.store(true, Release);
                done = true;
            }
            Ok(n) => {
                if let Ok(mut chunk) = prod.write_chunk_uninit(n * width) {
                    let (a, b) = chunk.as_mut_slices();
                    let mut k = 0;
                    for f in 0..n {
                        for &s in &slots {
                            for c in 0..CHANNELS {
                                let v = bufs[s][c][f];
                                if k < a.len() {
                                    a[k].write(v);
                                } else {
                                    b[k - a.len()].write(v);
                                }
                                k += 1;
                            }
                        }
                    }
                    // SAFETY: all n * width slots were written above.
                    unsafe { chunk.commit_all() };
                }
                if src.position() >= src.info().frames {
                    eof.store(true, Release);
                    done = true;
                }
            }
            Err(e) => {
                report(&shared, &err_slot, e);
                eof.store(true, Release);
                done = true;
            }
        }
    }
}

fn report(shared: &Shared, slot: &Mutex<Option<EngineError>>, e: EngineError) {
    if let Ok(mut g) = slot.lock() {
        *g = Some(e);
    }
    shared.reader_failed.store(true, Release);
}

// =====================================================================================
// Render worker

struct Worker {
    shared: Arc<Shared>,
    cmd_rx: rtrb::Consumer<WorkerCmd>,
    garbage: rtrb::Producer<Garbage>,
    spares: rtrb::Consumer<Stretcher>,
    stop: Arc<AtomicBool>,
    out: Option<Box<OutputSide>>,
    song: Option<Box<WorkerSong>>,
    pending_params: Option<jk_dsp::MixParams>,
    priority: Arc<OnceLock<String>>,
    // Peak meter: 6 windows of 50 ms.
    win: [[f32; 2]; 6],
    win_idx: usize,
    win_fill: usize,
    /// Clip warning: the `Shared::clip_epoch` last seen, and the device frames still to
    /// render before the warning can latch again (audio of the previous mix that is still
    /// in the graph must not re-latch a warning the caller just cleared).
    clip_epoch: u64,
    clip_hold: u64,
}

impl Worker {
    fn trash(&mut self, g: Garbage) {
        // If the house thread is far behind, the item is dropped here (rare; counted
        // nowhere because it cannot happen with a 256-slot ring drained every 5 ms).
        let _ = self.garbage.push(g);
    }

    fn run(mut self) {
        let _ = self.priority.set(output::promote_audio_thread());
        while !self.stop.load(Acquire) {
            self.handle_cmds();
            self.exchange_stretchers();
            self.check_clip_epoch();
            let r = self.produce_block();
            self.shared.worker_state.store(r as u32, Relaxed);
            match r {
                Produced::Block => {}
                Produced::NoWork | Produced::MarkerFull | Produced::Ended | Produced::Ahead => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Produced::NoSpare | Produced::Starved => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    fn handle_cmds(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                WorkerCmd::Song(s) => {
                    if let Some(old) = self.song.replace(s) {
                        self.trash(Garbage::Song(old));
                    }
                    // The new graph was built with the song's own settings: parameters
                    // sent earlier (for the previous song, or while nothing was loaded)
                    // are stale. The meter starts afresh.
                    self.pending_params = None;
                    self.win = [[0.0; 2]; 6];
                    self.win_fill = 0;
                }
                WorkerCmd::Segment(seg) => {
                    if let Some(song) = self.song.as_mut() {
                        let old = song.seg.replace(seg);
                        song.begin_segment();
                        if let Some(old) = old {
                            let _ = self.garbage.push(Garbage::Segment(old));
                        }
                    } else {
                        self.trash(Garbage::Segment(seg));
                    }
                }
                WorkerCmd::Output(side, rs) => {
                    if let Some(old) = self.out.replace(side) {
                        self.trash(Garbage::Output(old));
                    }
                    if let (Some(mut rs), Some(song)) = (rs, self.song.as_mut()) {
                        // Swap the contents so the box (now holding the old resampler)
                        // is freed by the house thread, not here.
                        std::mem::swap(&mut song.resampler, &mut *rs);
                        let _ = self.garbage.push(Garbage::Resampler(rs));
                    }
                }
                WorkerCmd::Params(p) => self.pending_params = Some(p),
                WorkerCmd::Unload => {
                    if let Some(old) = self.song.take() {
                        self.trash(Garbage::Song(old));
                    }
                    self.pending_params = None;
                }
            }
        }
    }

    /// Device frames of audio from before a mix change that can still come out of the
    /// graph: its latency, the key-change crossfade, the 20 ms gain ramp and one block.
    fn clip_hold_frames(out: &OutputSide, song: &WorkerSong) -> u64 {
        let g = &song.graph;
        let f = (g.latency() + g.crossfade_len() + song.song_rate as usize / 50 + BLOCK) as u64;
        f * out.dev_rate as u64 / song.song_rate.max(1) as u64
    }

    /// A load, mix change or master change restarts the clip warning (see `clip_hold`).
    fn check_clip_epoch(&mut self) {
        let e = self.shared.clip_epoch.load(Acquire);
        if e != self.clip_epoch {
            self.clip_epoch = e;
            self.shared.clipped.store(false, Relaxed);
            self.clip_hold = match (self.out.as_deref(), self.song.as_deref()) {
                (Some(out), Some(song)) => Self::clip_hold_frames(out, song),
                _ => 0,
            };
        }
    }

    /// Spare Rubber Band instances in, replaced ones out.
    fn exchange_stretchers(&mut self) {
        let Some(song) = self.song.as_mut() else {
            // No song: anything arriving is for a song that is gone.
            while let Ok(s) = self.spares.pop() {
                let _ = self.garbage.push(Garbage::Stretcher(s));
            }
            return;
        };
        while song.graph.spare_count() < GRAPH_SPARES {
            let Ok(s) = self.spares.pop() else { break };
            if s.config().sample_rate != song.song_rate {
                let _ = self.garbage.push(Garbage::Stretcher(s));
                continue;
            }
            if let Err(s) = song.graph.offer_stretcher(s) {
                let _ = self.garbage.push(Garbage::Stretcher(s));
            }
        }
        while let Some(r) = song.graph.take_retired() {
            if let Err(rtrb::PushError::Full(Garbage::Stretcher(r))) = self.garbage.push(Garbage::Stretcher(r)) {
                drop(r);
            }
        }
    }

    fn produce_block(&mut self) -> Produced {
        let (Some(out), Some(song)) = (self.out.as_mut(), self.song.as_mut()) else {
            return Produced::NoWork;
        };
        if let Some(p) = self.pending_params.take() {
            song.graph.set_params(&p);
            // The new mix is heard from here on, after the graph's latency.
            self.clip_hold = Self::clip_hold_frames(out, song);
        }
        let Some(seg) = song.seg.as_mut() else { return Produced::NoWork };
        if song.needs_reset {
            // A key other than 0 restarts a shifter: wait for a pre-built instance.
            if song.graph.reset_needs_stretcher() {
                return Produced::NoSpare;
            }
            song.graph.reset();
            song.resampler.reset();
            song.rs_skip = song.resampler.output_delay();
            song.needs_reset = false;
        }
        if !song.started {
            let m = Marker { at: out.total_pushed, generation: seg.generation, kind: MarkerKind::Start { pos: seg.start as f64 / song.song_rate as f64 } };
            if out.markers.push(m).is_err() {
                return Produced::MarkerFull;
            }
            song.started = true;
            song.expected_dev = ((song.song_frames.saturating_sub(seg.start)) as u128 * out.dev_rate as u128
                / song.song_rate as u128) as u64;
            self.shared.worker_gen.store(seg.generation, Release);
        }
        if song.ended {
            return Produced::Ended;
        }
        let ch = out.dev_ch;
        let fill = (out.prod.buffer().capacity() - out.prod.slots()) / ch;
        // Lead = this segment's frames pushed minus this segment's frames the filler has
        // consumed (not the ring fill: right after a seek the ring still holds the old
        // segment's audio, which the filler drops on purpose). Counted only once the
        // filler plays this segment and the lead has reached its target once.
        let fs = self.shared.filler_seg.load(Acquire);
        let adopted = fs >> FILLER_SEG_BITS == seg.generation & ((1 << (64 - FILLER_SEG_BITS)) - 1);
        let lead = if adopted { song.pushed_dev.saturating_sub(fs & ((1 << FILLER_SEG_BITS) - 1)) } else { 0 };
        if adopted && lead >= out.ahead_frames as u64 {
            song.reached_ahead = true;
        }
        let measuring = adopted
            && song.reached_ahead
            && self.shared.want_play.load(Relaxed)
            && self.shared.target_gen.load(Acquire) == seg.generation;
        if measuring {
            self.shared.min_ahead_frames.fetch_min(lead, Relaxed);
        }
        if song.pushed_dev >= song.expected_dev {
            if out.markers.push(Marker { at: out.total_pushed, generation: seg.generation, kind: MarkerKind::End }).is_ok() {
                song.ended = true;
            }
            return Produced::Ended;
        }
        if fill >= out.ahead_frames || out.prod.slots() < (song.resampler.max_output_frames() + 8) * ch {
            return Produced::Ahead;
        }

        // One block from the stem ring (zeros after the end of the song).
        let width = song.slots.len() * CHANNELS;
        let need = BLOCK * width;
        let eof = seg.eof.load(Acquire);
        let avail = seg.ring.slots();
        if avail < need && !eof {
            if measuring {
                self.shared.reader_starved.fetch_add(1, Relaxed);
            }
            return Produced::Starved;
        }
        let t0 = now_ns();
        let frames = avail.min(need) / width;
        if let Ok(chunk) = seg.ring.read_chunk(frames * width) {
            let (x, y) = chunk.as_slices();
            let mut k = 0;
            for f in 0..frames {
                for &s in &song.slots {
                    for c in 0..CHANNELS {
                        song.bufs[s][c][f] = if k < x.len() { x[k] } else { y[k - x.len()] };
                        k += 1;
                    }
                }
            }
            chunk.commit_all();
        }
        for &s in &song.slots {
            for c in 0..CHANNELS {
                song.bufs[s][c][frames..BLOCK].iter_mut().for_each(|v| *v = 0.0);
            }
        }
        {
            let b = &song.bufs;
            let z = &song.zeros[..];
            let slot = |s: usize, c: usize| -> &[f32] {
                if song.slots.contains(&s) { &b[s][c][..] } else { z }
            };
            let blk = StemBlock { stems: std::array::from_fn(|s| [slot(s, 0), slot(s, 1)]) };
            let extra = ExtraBlock {
                pitched: [slot(SLOT_EXTRA_PITCHED, 0), slot(SLOT_EXTRA_PITCHED, 1)],
                unpitched: [slot(SLOT_EXTRA_UNPITCHED, 0), slot(SLOT_EXTRA_UNPITCHED, 1)],
            };
            let ext = if song.extra { Some(&extra) } else { None };
            song.graph.process_ext(&blk, ext, BLOCK, [&mut song.gl[..], &mut song.gr[..]]);
        }
        // Graph output frame i is song frame song_in + i - L; keep frames >= start.
        let lat = song.graph.latency() as i64;
        for i in 0..BLOCK {
            let sf = song.song_in as i64 + i as i64 - lat;
            if sf >= seg.start as i64 {
                song.pend[0].push(song.gl[i]);
                song.pend[1].push(song.gr[i]);
            }
        }
        song.song_in += BLOCK as u64;

        // Resample (or pass through) in BLOCK-frame chunks; interleave for the device.
        let master = self.shared.master.load();
        let win_len = (out.dev_rate as usize / 20).max(1);
        let mut clip = false;
        while song.pend[0].len() >= BLOCK {
            let produced = match song.resampler.process([&song.pend[0][..BLOCK], &song.pend[1][..BLOCK]]) {
                Ok(p) => p,
                Err(_) => break, // cannot happen with fixed-size input
            };
            let n_out = produced[0].len();
            let skip = song.rs_skip.min(n_out);
            song.rs_skip -= skip;
            let mut k = 0;
            for f in skip..n_out {
                if song.pushed_dev >= song.expected_dev || k + ch > song.ilv.len() {
                    break;
                }
                let (l, r) = (produced[0][f], produced[1][f]);
                // Peak meter / clip detection (pre-master value, master applied below).
                let w = &mut self.win[self.win_idx];
                w[0] = w[0].max(l.abs());
                w[1] = w[1].max(r.abs());
                self.win_fill += 1;
                if self.win_fill >= win_len {
                    self.win_fill = 0;
                    self.win_idx = (self.win_idx + 1) % self.win.len();
                    self.win[self.win_idx] = [0.0; 2];
                }
                // Clip warning: this frame at the current master volume, once the audio
                // from before the last mix / master change is out of the graph.
                if self.clip_hold > 0 {
                    self.clip_hold -= 1;
                } else if l.abs().max(r.abs()) * master > 1.0 {
                    clip = true;
                }
                match ch {
                    1 => {
                        song.ilv[k] = 0.5 * (l + r);
                        k += 1;
                    }
                    n => {
                        song.ilv[k] = l;
                        song.ilv[k + 1] = r;
                        for z in 2..n {
                            song.ilv[k + z] = 0.0;
                        }
                        k += n;
                    }
                }
                song.pushed_dev += 1;
            }
            if let Ok(mut chunk) = out.prod.write_chunk(k) {
                let (a, b) = chunk.as_mut_slices();
                let na = a.len();
                a.copy_from_slice(&song.ilv[..na]);
                b.copy_from_slice(&song.ilv[na..k]);
                chunk.commit_all();
            }
            out.total_pushed += (k / ch) as u64;
            // Drop the consumed input without shifting memory more than once per chunk.
            song.pend[0].drain(..BLOCK);
            song.pend[1].drain(..BLOCK);
        }
        let mut peak = [0.0f32; 2];
        for w in &self.win {
            peak[0] = peak[0].max(w[0]);
            peak[1] = peak[1].max(w[1]);
        }
        self.shared.peak[0].store(peak[0]);
        self.shared.peak[1].store(peak[1]);
        if clip {
            self.shared.clipped.store(true, Relaxed);
        }

        let dt = now_ns() - t0;
        let st = song.graph.stats();
        let s = &self.shared;
        s.blocks.fetch_add(1, Relaxed);
        s.busy_ns.fetch_add(dt, Relaxed);
        s.max_block_ns.fetch_max(dt, Relaxed);
        s.shortfall_frames.store(st.shift_shortfall_frames, Relaxed);
        s.deferred_blocks.store(st.deferred_blocks, Relaxed);
        s.inline_constructions.store(st.inline_constructions, Relaxed);
        s.audible_semitones.store(song.graph.audible_semitones(), Relaxed);
        Produced::Block
    }
}

/// What one worker step did (stored in `Shared::worker_state` for diagnostics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Produced {
    Block = 0,
    /// No output, song or segment.
    NoWork = 1,
    /// Waiting for a pre-built Rubber Band instance.
    NoSpare = 2,
    MarkerFull = 3,
    Ended = 4,
    /// The output ring is far enough ahead.
    Ahead = 5,
    /// Waiting for the stem reader.
    Starved = 6,
}

impl WorkerSong {
    fn begin_segment(&mut self) {
        self.needs_reset = true;
        self.started = false;
        self.ended = false;
        self.reached_ahead = false;
        self.pushed_dev = 0;
        self.expected_dev = 0;
        self.pend[0].clear();
        self.pend[1].clear();
        if let Some(seg) = &self.seg {
            self.song_in = seg.read_from;
        }
    }
}

// =====================================================================================
// Control

struct CtlSong {
    info: SongInfo,
    reader_tx: crossbeam_channel::Sender<ReaderCmd>,
    reader: Option<JoinHandle<()>>,
    prime_len: usize,
    graph_latency: usize,
    width: usize,
}

struct Control {
    device_id: Option<String>,
    exclusive: bool,
    dev: Option<OpenOutput>,
    /// The device failed; the next `play()` reopens it.
    dev_lost: bool,
    worker_tx: rtrb::Producer<WorkerCmd>,
    song: Option<CtlSong>,
    generation: u64,
    state: PlaybackState,
    error: Option<String>,
    params: MixParams,
    /// Finished reader threads to join.
    zombies: Vec<JoinHandle<()>>,
}

pub(crate) struct Core {
    shared: Arc<Shared>,
    ctl: Mutex<Control>,
    events: crossbeam_channel::Sender<EngineEvent>,
    /// Sample rate the house builds spare Rubber Band instances for (0 = none).
    spare_rate: AtomicU32,
    /// Key the next instance will most likely be used for.
    key_hint: AtomicI32,
    reader_error: Arc<Mutex<Option<EngineError>>>,
    stop: Arc<AtomicBool>,
    capture: Option<(Capture, bool)>,
    priority: Arc<OnceLock<String>>,
}

/// Counters and timings for tests and benchmarks.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    pub generation: u64,
    pub dac_gen: u64,
    pub worker_gen: u64,
    pub dac_pos: f64,
    pub dac_ns: u64,
    pub device_rate: u32,
    pub latency_frames: u32,
    pub callbacks: u64,
    pub max_callback_us: f64,
    pub underruns: u64,
    pub underrun_frames: u64,
    pub device_starved: u64,
    pub worker_blocks: u64,
    pub worker_busy_s: f64,
    pub worker_max_block_ms: f64,
    /// Lowest output-ring lead seen while playing (ms), `None` before any.
    pub min_lead_ms: Option<f64>,
    pub reader_starved: u64,
    pub shortfall_frames: u64,
    pub deferred_blocks: u64,
    pub inline_constructions: u64,
    pub audible_semitones: i32,
    /// Filler mode at the last callback (0 flush, 1 primed, 2 running, 3 fade-out,
    /// 4 paused, 5 ended) and the worker's last step (see `Produced`).
    pub filler_mode: u32,
    pub worker_state: u32,
    /// Device clock (IAudioClock) vs the OS performance counter, ppm (Windows).
    pub device_drift_ppm: Option<f64>,
    pub worker_priority: String,
    pub device: Option<String>,
    pub song_rate: u32,
    pub graph_latency_frames: usize,
}

fn lock(core: &Core) -> MutexGuard<'_, Control> {
    core.ctl.lock().unwrap_or_else(|p| p.into_inner())
}

fn no_device_error(detail: impl Into<String>) -> EngineError {
    EngineError::new(
        "no_device",
        "No audio output is available. Connect speakers or headphones, or pick another output.",
        detail,
    )
}

impl Core {
    fn emit(&self, e: EngineEvent) {
        let _ = self.events.try_send(e);
    }

    fn emit_state(&self, ctl: &Control) {
        let s = self.snapshot_locked(ctl);
        self.emit(EngineEvent::StateChanged(s));
    }

    fn current_position(&self, ctl: &Control) -> f64 {
        let Some(song) = &ctl.song else { return 0.0 };
        let dur = song.info.duration_s();
        let p = match ctl.state {
            PlaybackState::Empty => 0.0,
            PlaybackState::Ended => dur,
            _ => {
                if self.shared.dac_gen.load(Acquire) == ctl.generation {
                    self.shared.dac_pos.load()
                } else {
                    self.shared.target_pos.load()
                }
            }
        };
        p.clamp(0.0, dur)
    }

    fn snapshot_locked(&self, ctl: &Control) -> Snapshot {
        let s = &self.shared;
        let playing = ctl.state == PlaybackState::Playing;
        let master = s.master.load();
        let device = ctl.dev.as_ref().map(|d| {
            let lat = if s.callbacks.load(Relaxed) > 0 {
                s.latency_frames.load(Relaxed)
            } else {
                d.info.buffer_frames + d.info.stream_latency_frames
            };
            DeviceStatus {
                name: d.info.name.clone(),
                sample_rate: d.info.sample_rate,
                channels: d.info.channels as u16,
                exclusive: d.info.exclusive,
                latency_ms: lat as f64 * 1000.0 / d.info.sample_rate.max(1) as f64,
            }
        });
        Snapshot {
            state: ctl.state,
            position_s: self.current_position(ctl),
            duration_s: ctl.song.as_ref().map_or(0.0, |x| x.info.duration_s()),
            device,
            underruns: s.underruns.load(Relaxed),
            device_starved: s.device_starved.load(Relaxed),
            output_peak: if playing { [s.peak[0].load() * master, s.peak[1].load() * master] } else { [0.0; 2] },
            clipped: s.clipped.load(Relaxed),
            error: if ctl.state == PlaybackState::Error { ctl.error.clone() } else { None },
            position_ns: if s.dac_gen.load(Acquire) == ctl.generation { s.dac_ns.load(Relaxed) } else { 0 },
        }
    }

    fn send_worker(&self, ctl: &mut Control, mut cmd: WorkerCmd) {
        // The worker drains its queue every few ms; wait up to 2 s if it is full.
        let t = Instant::now();
        loop {
            match ctl.worker_tx.push(cmd) {
                Ok(()) => return,
                Err(rtrb::PushError::Full(c)) => {
                    if t.elapsed() > Duration::from_secs(2) {
                        return; // dropped; the worker is gone
                    }
                    cmd = c;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn close_device(&self, ctl: &mut Control) {
        if let Some(d) = ctl.dev.take() {
            d.stop.store(true, Release);
            let _ = d.thread.join();
        }
    }

    /// Open the configured device (falling back to the default when a saved device is
    /// missing) and connect it to the worker. Does not start a segment.
    fn open_device(&self, ctl: &mut Control, song_rate: Option<u32>) -> Result<(), EngineError> {
        self.close_device(ctl);
        let req = OutputRequest {
            device_id: ctl.device_id.clone(),
            exclusive: ctl.exclusive,
            preferred_rate: song_rate,
            buffer_ms: 20,
        };
        // Sound rather than nothing: the chosen device as asked; if exclusive mode is
        // refused (device busy, or an output that has no exclusive mode), the same device
        // in shared mode; if the chosen device is gone, the default (exclusive, then
        // shared). The snapshot shows which mode is really in use.
        let mut attempts = vec![req.clone()];
        if req.exclusive {
            attempts.push(OutputRequest { exclusive: false, ..req.clone() });
        }
        if req.device_id.is_some() {
            attempts.push(OutputRequest { device_id: None, ..req.clone() });
            if req.exclusive {
                attempts.push(OutputRequest { device_id: None, exclusive: false, ..req.clone() });
            }
        }
        let mut errors = Vec::new();
        for a in &attempts {
            match output::open(a, self.capture.clone()) {
                Ok(out) => {
                    self.attach_device(ctl, out);
                    return Ok(());
                }
                Err(e) => errors.push(e),
            }
        }
        Err(no_device_error(errors.join("; ")))
    }

    fn attach_device(&self, ctl: &mut Control, out: OpenOutput) {
        let info: StreamInfo = out.info.clone();
        let dev_rate = info.sample_rate;
        let dev_ch = info.channels.max(1);
        // Output ring: 0.5 s plus room for one resampler call.
        let cap = (dev_rate as usize / 2 + 8192) * dev_ch;
        let (prod, cons) = rtrb::RingBuffer::<f32>::new(cap);
        let (mprod, mcons) = rtrb::RingBuffer::<Marker>::new(64);
        self.shared.device_failed.store(false, Release);
        self.shared.simulate_device_loss.store(false, Release);
        let _ = out.feed_tx.send(DeviceFeed { ring: cons, markers: mcons, shared: self.shared.clone() });
        let side = OutputSide {
            prod,
            markers: mprod,
            dev_rate,
            dev_ch,
            ahead_frames: (AHEAD_MS * dev_rate as u64 / 1000) as usize,
            total_pushed: 0,
        };
        let rs = ctl.song.as_ref().and_then(|s| OutputResampler::new(s.info.sample_rate, dev_rate, BLOCK).ok()).map(Box::new);
        self.send_worker(ctl, WorkerCmd::Output(Box::new(side), rs));
        ctl.dev = Some(out);
        ctl.dev_lost = false;
    }

    /// Start a new segment of the loaded song at `pos_s` (in-place seek).
    fn start_segment(&self, ctl: &mut Control, pos_s: f64) {
        let Some(song) = ctl.song.as_ref() else { return };
        let rate = song.info.sample_rate as f64;
        let frame = ((pos_s.max(0.0) * rate).round() as u64).min(song.info.frames);
        let read_from = frame.saturating_sub(song.prime_len as u64);
        let cap = ((STEM_BUFFER_S * rate) as usize).max(4 * READ_CHUNK) * song.width;
        let (prod, cons) = rtrb::RingBuffer::<f32>::new(cap);
        let eof = Arc::new(AtomicBool::new(false));
        ctl.generation += 1;
        let generation = ctl.generation;
        self.shared.target_pos.store(frame as f64 / rate);
        self.shared.target_gen.store(generation, Release);
        let _ = song.reader_tx.send(ReaderCmd::Seek { read_from, ring: prod, eof: eof.clone() });
        let seg = Segment { generation, start: frame, read_from, ring: cons, eof };
        self.send_worker(ctl, WorkerCmd::Segment(Box::new(seg)));
    }

    fn unload_locked(&self, ctl: &mut Control) {
        self.shared.want_play.store(false, Release);
        ctl.generation += 1;
        self.shared.target_pos.store(0.0);
        self.shared.target_gen.store(ctl.generation, Release);
        self.send_worker(ctl, WorkerCmd::Unload);
        if let Some(mut s) = ctl.song.take() {
            let _ = s.reader_tx.send(ReaderCmd::Stop);
            if let Some(h) = s.reader.take() {
                ctl.zombies.push(h);
            }
        }
        self.spare_rate.store(0, Release);
        ctl.state = PlaybackState::Empty;
        ctl.error = None;
    }

    /// House-keeping checks (house thread, every few ms).
    fn check(&self) {
        let s = &self.shared;
        // One lock for the whole check, so an API call (e.g. set_device) cannot slip in
        // between noticing a dead device and dropping it.
        let mut ctl = lock(self);
        let device_dead = ctl.dev.as_ref().is_some_and(|d| d.thread.is_finished() || s.device_failed.load(Acquire));
        if device_dead {
            let pos = self.current_position(&ctl);
            if let Some(d) = ctl.dev.take() {
                d.stop.store(true, Release);
                let _ = d.thread.join();
            }
            ctl.dev_lost = true;
            s.want_play.store(false, Release);
            if ctl.state == PlaybackState::Playing {
                ctl.state = PlaybackState::Paused;
            }
            // Remember where we were: the next segment starts there.
            s.target_pos.store(pos);
            ctl.generation += 1;
            s.target_gen.store(ctl.generation, Release);
            self.emit(EngineEvent::DeviceLost);
            self.emit_state(&ctl);
        }
        if s.reader_failed.swap(false, AcqRel) {
            let e = self.reader_error.lock().ok().and_then(|mut g| g.take());
            let e = e.unwrap_or_else(|| EngineError::internal("stem reader failed"));
            if ctl.song.is_some() {
                s.want_play.store(false, Release);
                ctl.state = PlaybackState::Error;
                ctl.error = Some(e.message.clone());
                self.emit(EngineEvent::Error { code: e.code.clone(), message: e.message.clone() });
                self.emit_state(&ctl);
            }
        }
        let ended = s.ended_gen.load(Acquire);
        if ended != 0 && ended == ctl.generation && ctl.state == PlaybackState::Playing {
            s.want_play.store(false, Release);
            ctl.state = PlaybackState::Ended;
            self.emit(EngineEvent::Ended);
            self.emit_state(&ctl);
        }
        ctl.zombies.retain(|h| !h.is_finished());
    }
}

/// House-keeping thread: spare Rubber Band instances, garbage, status events.
fn house_thread(core: Arc<Core>, mut garbage: rtrb::Consumer<Garbage>, mut spares: rtrb::Producer<Stretcher>) {
    while !core.stop.load(Acquire) {
        while let Ok(g) = garbage.pop() {
            drop(g);
        }
        let rate = core.spare_rate.load(Acquire);
        while rate != 0 && spares.slots() > 0 {
            let cfg = jk_dsp::stretcher_config(rate);
            match Stretcher::new(cfg) {
                Ok(mut s) => {
                    let key = core.key_hint.load(Relaxed);
                    s.set_pitch_scale(jk_dsp::semitones_to_scale(key as f64));
                    let _ = spares.push(s);
                }
                Err(_) => break,
            }
        }
        core.check();
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub struct EngineInner {
    core: Arc<Core>,
    events_rx: crossbeam_channel::Receiver<EngineEvent>,
    worker: Option<JoinHandle<()>>,
    house: Option<JoinHandle<()>>,
}

impl EngineInner {
    pub fn new(cfg: EngineConfig) -> Result<Self, EngineError> {
        Self::build(cfg, None)
    }

    /// Engine on the null device with every played buffer captured (tests). `fast`:
    /// consume as fast as the worker produces instead of in real time.
    pub fn new_with_capture(rate: u32, fast: bool, master: f32) -> Result<(Self, Capture), EngineError> {
        let cap: Capture = Arc::new(Mutex::new(Vec::new()));
        let cfg = EngineConfig { device_id: Some(format!("null:{rate}")), exclusive: false, master };
        Ok((Self::build(cfg, Some((cap.clone(), fast)))?, cap))
    }

    fn build(cfg: EngineConfig, capture: Option<(Capture, bool)>) -> Result<Self, EngineError> {
        let shared = Arc::new(Shared::new(cfg.master.clamp(0.0, 1.0)));
        let (worker_tx, cmd_rx) = rtrb::RingBuffer::<WorkerCmd>::new(64);
        let (garbage_tx, garbage_rx) = rtrb::RingBuffer::<Garbage>::new(256);
        let (spare_tx, spare_rx) = rtrb::RingBuffer::<Stretcher>::new(GRAPH_SPARES);
        let (ev_tx, ev_rx) = crossbeam_channel::bounded(256);
        let stop = Arc::new(AtomicBool::new(false));
        let priority = Arc::new(OnceLock::new());
        let worker = Worker {
            shared: shared.clone(),
            cmd_rx,
            garbage: garbage_tx,
            spares: spare_rx,
            stop: stop.clone(),
            out: None,
            song: None,
            pending_params: None,
            priority: priority.clone(),
            win: [[0.0; 2]; 6],
            win_idx: 0,
            win_fill: 0,
            clip_epoch: 0,
            clip_hold: 0,
        };
        let worker = std::thread::Builder::new()
            .name("jk-render".into())
            .spawn(move || worker.run())
            .map_err(EngineError::internal)?;
        let core = Arc::new(Core {
            shared,
            ctl: Mutex::new(Control {
                device_id: cfg.device_id.clone(),
                exclusive: cfg.exclusive,
                dev: None,
                dev_lost: false,
                worker_tx,
                song: None,
                generation: 0,
                state: PlaybackState::Empty,
                error: None,
                params: MixParams::default(),
                zombies: Vec::new(),
            }),
            events: ev_tx,
            spare_rate: AtomicU32::new(0),
            key_hint: AtomicI32::new(0),
            reader_error: Arc::new(Mutex::new(None)),
            stop: stop.clone(),
            capture,
            priority,
        });
        {
            // A missing device is not an error here: playback reports it.
            let mut ctl = lock(&core);
            if core.open_device(&mut ctl, None).is_err() {
                ctl.dev_lost = true;
            }
        }
        let house = {
            let core = core.clone();
            std::thread::Builder::new()
                .name("jk-engine-house".into())
                .spawn(move || house_thread(core, garbage_rx, spare_tx))
                .map_err(EngineError::internal)?
        };
        Ok(EngineInner { core, events_rx: ev_rx, worker: Some(worker), house: Some(house) })
    }

    pub fn set_device(&self, id: Option<&str>, exclusive: bool) -> Result<(), EngineError> {
        let core = &self.core;
        let mut ctl = lock(core);
        let was_playing = core.shared.want_play.load(Acquire) && ctl.state == PlaybackState::Playing;
        let pos = core.current_position(&ctl);
        if was_playing {
            core.shared.want_play.store(false, Release);
            drop(ctl);
            std::thread::sleep(Duration::from_millis(15)); // let the 10 ms fade-out play
            ctl = lock(core);
        }
        let (old_id, old_excl) = (ctl.device_id.clone(), ctl.exclusive);
        ctl.device_id = id.map(str::to_string);
        ctl.exclusive = exclusive;
        let song_rate = ctl.song.as_ref().map(|s| s.info.sample_rate);
        core.close_device(&mut ctl);
        let req = OutputRequest { device_id: ctl.device_id.clone(), exclusive, preferred_rate: song_rate, buffer_ms: 20 };
        let mut result = Ok(());
        match output::open(&req, core.capture.clone()) {
            Ok(out) => core.attach_device(&mut ctl, out),
            Err(e) if exclusive => {
                // Exclusive refused: same device, shared mode.
                ctl.exclusive = false;
                let req = OutputRequest { exclusive: false, ..req };
                match output::open(&req, core.capture.clone()) {
                    Ok(out) => {
                        core.attach_device(&mut ctl, out);
                        result = Err(EngineError::new(
                            "exclusive_unavailable",
                            "This output does not allow exclusive mode; it is used in shared mode.",
                            e,
                        ));
                    }
                    Err(e2) => {
                        result = Err(EngineError::new("device_failed", "This audio output could not be opened.", e2));
                    }
                }
            }
            Err(e) => {
                result = Err(EngineError::new(
                    if id.is_some() { "device_not_found" } else { "no_device" },
                    "This audio output could not be opened.",
                    e,
                ));
            }
        }
        if ctl.dev.is_none() {
            // Keep sound: go back to the previous device (or the default).
            ctl.device_id = old_id;
            ctl.exclusive = old_excl;
            if core.open_device(&mut ctl, song_rate).is_err() {
                ctl.dev_lost = true;
            }
        }
        if ctl.song.is_some() && ctl.dev.is_some() {
            core.start_segment(&mut ctl, pos);
            if ctl.state == PlaybackState::Ended {
                ctl.state = PlaybackState::Paused;
            }
            if was_playing {
                core.shared.want_play.store(true, Release);
                ctl.state = PlaybackState::Playing;
            }
        } else if was_playing {
            ctl.state = PlaybackState::Paused;
        }
        core.emit_state(&ctl);
        result
    }

    pub fn load(&self, path: &Path, params: MixParams) -> Result<LoadedSong, EngineError> {
        let core = &self.core;
        // Open and check the file before touching the current song.
        let src = SongSource::open(path)?;
        let info = src.info().clone();
        let rate = info.sample_rate;
        let mut ctl = lock(core);
        core.unload_locked(&mut ctl);
        core.shared.reset_counters();
        *core.reader_error.lock().unwrap_or_else(|p| p.into_inner()) = None;
        core.shared.reader_failed.store(false, Release);
        // Exclusive mode plays at the song rate when the device allows it.
        let reopen = ctl.dev.is_none() || (ctl.exclusive && ctl.dev.as_ref().is_some_and(|d| d.info.sample_rate != rate));
        if reopen && core.open_device(&mut ctl, Some(rate)).is_err() {
            ctl.dev_lost = true;
        }
        let dev_rate = ctl.dev.as_ref().map_or(rate, |d| d.info.sample_rate);
        let dsp = params.to_dsp();
        let mut gcfg = GraphConfig::new(rate);
        gcfg.max_block = BLOCK;
        gcfg.inline_stretchers = false;
        let graph = PitchGraph::new(gcfg, dsp).map_err(EngineError::internal)?;
        let resampler = OutputResampler::new(rate, dev_rate, BLOCK).map_err(EngineError::internal)?;
        let mut slots: Vec<usize> = info.active_slots();
        if slots.is_empty() {
            slots.push(SLOT_OTHER); // nothing playable: silence of the right length
        }
        let width = slots.len() * CHANNELS;
        let prime_len = graph.prime_len();
        let graph_latency = graph.latency();
        let max_out = resampler.max_output_frames();
        let wsong = WorkerSong {
            graph,
            resampler,
            song_rate: rate,
            song_frames: info.frames,
            slots: slots.clone(),
            extra: info.has_extra(),
            seg: None,
            needs_reset: true,
            started: false,
            ended: false,
            reached_ahead: false,
            song_in: 0,
            pushed_dev: 0,
            expected_dev: 0,
            rs_skip: 0,
            bufs: slot_buffers(BLOCK),
            gl: vec![0.0; BLOCK],
            gr: vec![0.0; BLOCK],
            pend: [Vec::with_capacity(4 * BLOCK), Vec::with_capacity(4 * BLOCK)],
            // Interleave buffer: enough for one resampler call at up to 8x the song rate
            // and 8 device channels (a later device switch can raise the rate).
            ilv: vec![0.0; (max_out.max(8 * BLOCK) + 64) * 8],
            zeros: vec![0.0; BLOCK],
        };
        core.send_worker(&mut ctl, WorkerCmd::Song(Box::new(wsong)));
        let (reader_tx, reader_rx) = crossbeam_channel::unbounded();
        let reader = {
            let shared = core.shared.clone();
            let err = core.reader_error.clone();
            let slots = slots.clone();
            std::thread::Builder::new()
                .name("jk-stem-reader".into())
                .spawn(move || reader_thread(src, slots, reader_rx, shared, err))
                .map_err(EngineError::internal)?
        };
        ctl.song = Some(CtlSong { info: info.clone(), reader_tx, reader: Some(reader), prime_len, graph_latency, width });
        ctl.params = params;
        core.key_hint.store(params.semitones, Relaxed);
        core.spare_rate.store(rate, Release);
        ctl.state = PlaybackState::Paused;
        core.start_segment(&mut ctl, 0.0);
        core.emit_state(&ctl);
        Ok(LoadedSong {
            duration_s: info.duration_s(),
            sample_rate: rate,
            channels: info.channels,
            stems: info.stems.clone(),
        })
    }

    pub fn unload(&self) {
        let mut ctl = lock(&self.core);
        self.core.unload_locked(&mut ctl);
        self.core.emit_state(&ctl);
    }

    pub fn play(&self) {
        let core = &self.core;
        let mut ctl = lock(core);
        if ctl.song.is_none() {
            core.emit(EngineEvent::Error { code: "not_loaded".into(), message: "No song is loaded.".into() });
            return;
        }
        if ctl.dev.is_none() {
            let rate = ctl.song.as_ref().map(|s| s.info.sample_rate);
            let pos = core.current_position(&ctl);
            match core.open_device(&mut ctl, rate) {
                Ok(()) => core.start_segment(&mut ctl, pos),
                Err(e) => {
                    ctl.dev_lost = true;
                    ctl.state = PlaybackState::Error;
                    ctl.error = Some(e.message.clone());
                    core.emit(EngineEvent::Error { code: e.code, message: e.message });
                    core.emit_state(&ctl);
                    return;
                }
            }
        }
        match ctl.state {
            PlaybackState::Ended => core.start_segment(&mut ctl, 0.0),
            PlaybackState::Error => {
                // Retry from where it stopped (re-opens the decoders' position).
                let pos = core.current_position(&ctl);
                core.start_segment(&mut ctl, pos);
            }
            _ => {}
        }
        ctl.error = None;
        core.shared.want_play.store(true, Release);
        ctl.state = PlaybackState::Playing;
        core.emit_state(&ctl);
    }

    pub fn pause(&self) {
        let core = &self.core;
        let mut ctl = lock(core);
        core.shared.want_play.store(false, Release);
        if ctl.state == PlaybackState::Playing {
            ctl.state = PlaybackState::Paused;
            core.emit_state(&ctl);
        }
    }

    pub fn seek(&self, seconds: f64) {
        let core = &self.core;
        let mut ctl = lock(core);
        if ctl.song.is_none() || !seconds.is_finite() {
            return;
        }
        core.start_segment(&mut ctl, seconds);
        if ctl.state == PlaybackState::Ended || ctl.state == PlaybackState::Error {
            ctl.state = PlaybackState::Paused;
            core.shared.want_play.store(false, Release);
        }
        core.emit_state(&ctl);
    }

    pub fn set_params(&self, p: MixParams) {
        let core = &self.core;
        let mut ctl = lock(core);
        let old = ctl.params;
        ctl.params = p;
        core.key_hint.store(p.semitones, Relaxed);
        // Restart the clip warning (the worker ignores the old mix still in the graph).
        // Bumped before the command is queued, so the worker sees it no later.
        core.shared.clipped.store(false, Relaxed);
        core.shared.clip_epoch.fetch_add(1, Release);
        core.send_worker(&mut ctl, WorkerCmd::Params(p.to_dsp()));
        // Not playing: re-render what is buffered so play() starts with the new
        // settings instead of ~150 ms of the old ones.
        let buffered = matches!(ctl.state, PlaybackState::Paused) && ctl.song.is_some();
        if buffered && old != p {
            let pos = core.current_position(&ctl);
            core.start_segment(&mut ctl, pos);
        }
    }

    pub fn set_master(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        self.core.shared.master.store(v);
        self.core.shared.clipped.store(false, Relaxed);
        self.core.shared.clip_epoch.fetch_add(1, Release);
    }

    pub fn snapshot(&self) -> Snapshot {
        let ctl = lock(&self.core);
        self.core.snapshot_locked(&ctl)
    }

    pub fn events(&self) -> crossbeam_channel::Receiver<EngineEvent> {
        self.events_rx.clone()
    }

    pub fn diagnostics(&self) -> Diagnostics {
        let core = &self.core;
        let s = &core.shared;
        let ctl = lock(core);
        let dev_rate = ctl.dev.as_ref().map_or(0, |d| d.info.sample_rate);
        let min_ahead = s.min_ahead_frames.load(Relaxed);
        let drift = {
            let p0 = s.os_pos_first.load(Relaxed);
            let p1 = s.os_pos_last.load(Relaxed);
            let q0 = s.os_qpc_first.load(Relaxed);
            let q1 = s.os_qpc_last.load(Relaxed);
            if dev_rate == 0 || q1 <= q0 + 10_000_000 {
                None
            } else {
                let dev_s = (p1 - p0) as f64 / dev_rate as f64;
                let os_s = (q1 - q0) as f64 * 1e-7;
                Some((dev_s / os_s - 1.0) * 1e6)
            }
        };
        Diagnostics {
            generation: ctl.generation,
            dac_gen: s.dac_gen.load(Acquire),
            worker_gen: s.worker_gen.load(Acquire),
            dac_pos: s.dac_pos.load(),
            dac_ns: s.dac_ns.load(Relaxed),
            device_rate: dev_rate,
            latency_frames: s.latency_frames.load(Relaxed),
            callbacks: s.callbacks.load(Relaxed),
            max_callback_us: s.max_callback_ns.load(Relaxed) as f64 / 1000.0,
            underruns: s.underruns.load(Relaxed),
            underrun_frames: s.underrun_frames.load(Relaxed),
            device_starved: s.device_starved.load(Relaxed),
            worker_blocks: s.blocks.load(Relaxed),
            worker_busy_s: s.busy_ns.load(Relaxed) as f64 * 1e-9,
            worker_max_block_ms: s.max_block_ns.load(Relaxed) as f64 / 1e6,
            min_lead_ms: (min_ahead != u64::MAX && dev_rate > 0).then(|| min_ahead as f64 * 1000.0 / dev_rate as f64),
            reader_starved: s.reader_starved.load(Relaxed),
            shortfall_frames: s.shortfall_frames.load(Relaxed),
            deferred_blocks: s.deferred_blocks.load(Relaxed),
            inline_constructions: s.inline_constructions.load(Relaxed),
            audible_semitones: s.audible_semitones.load(Relaxed),
            filler_mode: s.filler_mode.load(Relaxed),
            worker_state: s.worker_state.load(Relaxed),
            device_drift_ppm: drift,
            worker_priority: core.priority.get().cloned().unwrap_or_default(),
            device: ctl.dev.as_ref().map(|d| {
                format!(
                    "{} [{} {}] {} Hz {} ch {}, buffer {} frames, stream latency {} frames",
                    d.info.name,
                    d.info.backend,
                    d.info.share_mode,
                    d.info.sample_rate,
                    d.info.channels,
                    d.info.sample_format,
                    d.info.buffer_frames,
                    d.info.stream_latency_frames
                )
            }),
            song_rate: ctl.song.as_ref().map_or(0, |x| x.info.sample_rate),
            graph_latency_frames: ctl.song.as_ref().map_or(0, |x| x.graph_latency),
        }
    }

    pub fn debug_simulate_device_loss(&self) {
        self.core.shared.simulate_device_loss.store(true, Release);
    }
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        {
            let mut ctl = lock(&self.core);
            self.core.unload_locked(&mut ctl);
            self.core.close_device(&mut ctl);
            for h in ctl.zombies.drain(..) {
                let _ = h.join();
            }
        }
        self.core.stop.store(true, Release);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Some(h) = self.house.take() {
            let _ = h.join();
        }
    }
}

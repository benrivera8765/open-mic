// SPDX-License-Identifier: GPL-3.0-or-later
//! The real-time player (docs/ARCHITECTURE.md section 4, "Threads"):
//!
//! 1. **Stem reader** (I/O thread): decodes the four FLAC stems ahead into one SPSC ring
//!    per stem (default 4 s each), so a slow USB stick cannot starve playback.
//! 2. **Render worker** (MMCSS "Pro Audio" / QoS user-interactive): pulls equal-length
//!    blocks from the stem rings, runs the jk-dsp graph (mix, pitch, drum delay) and the
//!    output resampler (only if device rate != song rate), and keeps the output ring about
//!    150 ms ahead of the device.
//! 3. **Device callback**: copies from the output ring, applies the master volume, advances
//!    the clock; underruns write silence and bump an atomic counter.
//!
//! Prototype limits: a live seek is done by starting a new `Player` at the seek frame
//! (the decoders seek sample-accurately and the graph is primed with real audio before the
//! seek point); a flush-in-place seek (generation counter) is not written yet.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use jk_dsp::{GraphConfig, MixParams, OutputResampler, PitchGraph, StemBlock, CHANNELS};

use crate::clock::{now_ns, Clock};
use crate::decode::FlacStem;
use crate::output::{promote_audio_thread, DeviceFeed, DeviceInfo, OutputRequest};

/// The four stem files of a song (`lead.flac`, `backing.flac`, `drums.flac`, `other.flac`).
#[derive(Clone, Debug)]
pub struct StemFiles {
    pub paths: [PathBuf; 4],
}

impl StemFiles {
    pub fn in_dir(dir: &Path) -> Self {
        StemFiles {
            paths: ["lead", "backing", "drums", "other"].map(|n| dir.join(format!("{n}.flac"))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub output: OutputRequest,
    /// Song frame to start at.
    pub start_frame: u64,
    pub params: MixParams,
    /// Linear master gain applied in the device callback.
    pub master_gain: f32,
    /// How far ahead of the device the worker keeps the output ring.
    pub ahead_ms: u32,
    /// Decoded audio buffered per stem.
    pub stem_buffer_s: f32,
    /// Graph block size (song frames).
    pub block: usize,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        PlayerConfig {
            output: OutputRequest::default(),
            start_frame: 0,
            params: MixParams::default(),
            master_gain: 1.0,
            ahead_ms: 150,
            stem_buffer_s: 4.0,
            block: 512,
        }
    }
}

/// Worker-side counters.
#[derive(Default)]
pub struct WorkerStats {
    pub blocks: AtomicU64,
    /// Longest graph+resampler block, ns.
    pub max_block_ns: AtomicU64,
    /// Total ns spent in graph+resampler.
    pub busy_ns: AtomicU64,
    /// Times (after playback started) the worker needed stem audio and the reader had not
    /// decoded it yet. Not an audible problem while the output ring still has audio.
    pub reader_starved: AtomicU64,
    /// Lowest output-ring fill seen after start (device frames).
    pub min_ahead_frames: AtomicU64,
    /// Rubber Band output not ready in time (must be 0).
    pub graph_shortfall_frames: AtomicU64,
    pub worker_priority: std::sync::OnceLock<String>,
}

pub struct Player {
    pub device: DeviceInfo,
    pub song_rate: u32,
    pub song_frames: u64,
    pub graph_latency: usize,
    pub resampler_delay: usize,
    pub resampling: bool,
    pub clock: Arc<Clock>,
    pub stats: Arc<WorkerStats>,
    master: Arc<AtomicU32>,
    params_tx: rtrb::Producer<MixParams>,
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    device_thread: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Debug)]
pub struct PlayerError(pub String);
impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PlayerError {}
fn perr(e: impl std::fmt::Display) -> PlayerError {
    PlayerError(e.to_string())
}

impl Player {
    pub fn start(files: &StemFiles, cfg: PlayerConfig) -> Result<Player, PlayerError> {
        // Open the stems and check they agree.
        let mut stems = Vec::with_capacity(4);
        for p in &files.paths {
            stems.push(FlacStem::open(p).map_err(perr)?);
        }
        let song_rate = stems[0].sample_rate();
        let song_frames = stems.iter().map(|s| s.total_frames().unwrap_or(0)).max().unwrap_or(0);
        if stems.iter().any(|s| s.sample_rate() != song_rate) {
            return Err(PlayerError("stems have different sample rates".into()));
        }

        // Device first: its rate decides whether we resample.
        let mut req = cfg.output.clone();
        req.preferred_rate = Some(song_rate);
        #[cfg(windows)]
        let out = crate::output::wasapi_out::open(req).map_err(perr)?;
        #[cfg(target_os = "macos")]
        let out = crate::output::coreaudio_out::open(req).map_err(perr)?;
        #[cfg(not(any(windows, target_os = "macos")))]
        return Err(PlayerError("no audio backend for this platform".into()));

        let device = out.info.clone();
        let dev_rate = device.sample_rate;
        let dev_ch = device.channels;

        // Graph + resampler.
        let mut gcfg = GraphConfig::new(song_rate);
        gcfg.max_block = cfg.block;
        let graph = PitchGraph::new(gcfg, cfg.params).map_err(perr)?;
        let resampler = OutputResampler::new(song_rate, dev_rate, cfg.block).map_err(perr)?;
        let graph_latency = graph.latency();
        let resampler_delay = resampler.output_delay();
        let resampling = !resampler.is_passthrough();

        // Start decoding early enough to prime the shifter with real audio.
        let preroll = graph.prime_len() as u64;
        let read_from = cfg.start_frame.saturating_sub(preroll);
        for s in stems.iter_mut() {
            s.seek(read_from).map_err(perr)?;
        }

        let clock = Arc::new(Clock::default());
        clock.device_rate.store(dev_rate, Relaxed);
        clock.song_rate.store(song_rate, Relaxed);
        clock.start_song_frame.store(cfg.start_frame, Relaxed);
        let stats = Arc::new(WorkerStats::default());
        stats.min_ahead_frames.store(u64::MAX, Relaxed);
        let running = Arc::new(AtomicBool::new(true));
        let go = Arc::new(AtomicBool::new(false));
        let master = Arc::new(AtomicU32::new(cfg.master_gain.to_bits()));

        // Rings.
        let stem_cap = ((cfg.stem_buffer_s * song_rate as f32) as usize).max(4 * cfg.block) * CHANNELS;
        let mut stem_prod = Vec::new();
        let mut stem_cons = Vec::new();
        for _ in 0..4 {
            let (p, c) = rtrb::RingBuffer::<f32>::new(stem_cap);
            stem_prod.push(p);
            stem_cons.push(c);
        }
        let out_cap = (dev_rate as usize / 2).max(4 * resampler.max_output_frames()) * dev_ch;
        let (out_prod, out_cons) = rtrb::RingBuffer::<f32>::new(out_cap);
        let (params_tx, params_rx) = rtrb::RingBuffer::<MixParams>::new(64);
        let eof = Arc::new([AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false)]);

        out.feed_tx
            .send(DeviceFeed { consumer: out_cons, clock: clock.clone(), master_gain: master.clone(), go: go.clone(), running: running.clone() })
            .map_err(|_| PlayerError("device thread gone".into()))?;

        let mut threads = Vec::new();
        {
            let running = running.clone();
            let eof = eof.clone();
            threads.push(
                std::thread::Builder::new()
                    .name("jk-stem-reader".into())
                    .spawn(move || reader_thread(stems, stem_prod, eof, running))
                    .map_err(perr)?,
            );
        }
        {
            let w = Worker {
                graph,
                resampler,
                stem_cons,
                out: out_prod,
                params_rx,
                eof,
                running: running.clone(),
                go,
                clock: clock.clone(),
                stats: stats.clone(),
                block: cfg.block,
                dev_ch,
                start: cfg.start_frame,
                song_in: read_from,
                song_frames,
                ahead_frames: (cfg.ahead_ms as u64 * dev_rate as u64 / 1000) as usize,
                dev_rate,
                song_rate,
            };
            threads.push(std::thread::Builder::new().name("jk-render".into()).spawn(move || w.run()).map_err(perr)?);
        }
        Ok(Player {
            device,
            song_rate,
            song_frames,
            graph_latency,
            resampler_delay,
            resampling,
            clock,
            stats,
            master,
            params_tx,
            running,
            threads,
            device_thread: Some(out.thread),
        })
    }

    /// Change mix / key (takes effect at the worker's next block).
    pub fn set_params(&mut self, p: MixParams) -> bool {
        self.params_tx.push(p).is_ok()
    }

    pub fn set_master_gain(&self, g: f32) {
        self.master.store(g.to_bits(), Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.clock.finished.load(Relaxed)
    }

    /// Stop all threads. Returns the device thread's result.
    pub fn stop(mut self) -> Result<(), String> {
        self.running.store(false, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        match self.device_thread.take() {
            Some(t) => t.join().unwrap_or_else(|_| Err("device thread panicked".into())),
            None => Ok(()),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.running.store(false, Relaxed);
    }
}

const READ_CHUNK: usize = 4096;

fn reader_thread(
    mut stems: Vec<FlacStem>,
    mut prod: Vec<rtrb::Producer<f32>>,
    eof: Arc<[AtomicBool; 4]>,
    running: Arc<AtomicBool>,
) {
    let mut planar = [vec![0.0f32; READ_CHUNK], vec![0.0f32; READ_CHUNK]];
    while running.load(Relaxed) {
        let mut did = false;
        for i in 0..4 {
            if eof[i].load(Relaxed) || prod[i].slots() < READ_CHUNK * CHANNELS {
                continue;
            }
            match stems[i].read(&mut planar, 0, READ_CHUNK) {
                Ok(0) => eof[i].store(true, Relaxed),
                Ok(n) => {
                    if let Ok(mut chunk) = prod[i].write_chunk_uninit(n * CHANNELS) {
                        let (a, b) = chunk.as_mut_slices();
                        let mut k = 0;
                        for f in 0..n {
                            for c in 0..CHANNELS {
                                let v = planar[c][f];
                                if k < a.len() {
                                    a[k].write(v);
                                } else {
                                    b[k - a.len()].write(v);
                                }
                                k += 1;
                            }
                        }
                        // SAFETY: all n*CHANNELS slots were written above.
                        unsafe { chunk.commit_all() };
                    }
                    did = true;
                }
                Err(e) => {
                    eprintln!("stem reader: {e}");
                    eof[i].store(true, Relaxed);
                }
            }
        }
        if !did {
            std::thread::sleep(Duration::from_millis(3));
        }
    }
}

struct Worker {
    graph: PitchGraph,
    resampler: OutputResampler,
    stem_cons: Vec<rtrb::Consumer<f32>>,
    out: rtrb::Producer<f32>,
    params_rx: rtrb::Consumer<MixParams>,
    eof: Arc<[AtomicBool; 4]>,
    running: Arc<AtomicBool>,
    go: Arc<AtomicBool>,
    clock: Arc<Clock>,
    stats: Arc<WorkerStats>,
    block: usize,
    dev_ch: usize,
    /// First song frame to play.
    start: u64,
    /// Song frame of the next graph input frame.
    song_in: u64,
    song_frames: u64,
    ahead_frames: usize,
    dev_rate: u32,
    song_rate: u32,
}

impl Worker {
    fn run(mut self) {
        let prio = promote_audio_thread();
        let _ = self.stats.worker_priority.set(prio);
        let b = self.block;
        let lat = self.graph.latency() as u64;
        let mut stems: Vec<[Vec<f32>; 2]> = (0..4).map(|_| [vec![0.0f32; b], vec![0.0f32; b]]).collect();
        let mut gl = vec![0.0f32; b];
        let mut gr = vec![0.0f32; b];
        // Song-rate frames waiting for the resampler (it takes exactly `b` per call).
        let mut pend: [Vec<f32>; 2] = [Vec::with_capacity(4 * b), Vec::with_capacity(4 * b)];
        let mut rs_skip = self.resampler.output_delay();
        let fade_len = (self.dev_rate as usize / 100).max(1); // 10 ms fade-in
        let mut pushed_dev: u64 = 0;
        let expected_dev = ((self.song_frames.saturating_sub(self.start)) as u128 * self.dev_rate as u128
            / self.song_rate as u128) as u64;
        let mut started = false;
        let mut ilv: Vec<f32> = vec![0.0; (self.resampler.max_output_frames() + 8) * self.dev_ch];

        while self.running.load(Relaxed) {
            // Parameter updates: only the newest matters.
            let mut latest = None;
            while let Ok(p) = self.params_rx.pop() {
                latest = Some(p);
            }
            if let Some(p) = latest {
                self.graph.set_params(&p);
            }
            let fill = (self.out.buffer().capacity() - self.out.slots()) / self.dev_ch;
            if started {
                let f = fill as u64;
                if f < self.stats.min_ahead_frames.load(Relaxed) {
                    self.stats.min_ahead_frames.store(f, Relaxed);
                }
            }
            if !started && fill >= self.ahead_frames {
                self.go.store(true, Relaxed);
                started = true;
            }
            if fill >= self.ahead_frames || self.out.slots() < (self.resampler.max_output_frames() + 8) * self.dev_ch {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            if pushed_dev >= expected_dev {
                self.clock.finished.store(true, Relaxed);
                if !started {
                    self.go.store(true, Relaxed);
                    started = true;
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            // Pull one block from every stem ring (zeros after the end of the song).
            let need = b * CHANNELS;
            let all_eof = self.eof.iter().all(|e| e.load(Relaxed));
            let ready = self.stem_cons.iter().all(|c| c.slots() >= need);
            if !ready && !all_eof {
                if started {
                    self.stats.reader_starved.fetch_add(1, Relaxed);
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            for (i, c) in self.stem_cons.iter_mut().enumerate() {
                let have = c.slots().min(need) / CHANNELS * CHANNELS;
                let frames = have / CHANNELS;
                if let Ok(chunk) = c.read_chunk(have) {
                    let (x, y) = chunk.as_slices();
                    for f in 0..frames {
                        for ch in 0..CHANNELS {
                            let k = f * CHANNELS + ch;
                            stems[i][ch][f] = if k < x.len() { x[k] } else { y[k - x.len()] };
                        }
                    }
                    chunk.commit_all();
                }
                for ch in 0..CHANNELS {
                    stems[i][ch][frames..].iter_mut().for_each(|v| *v = 0.0);
                }
            }
            let t0 = now_ns();
            let blk = StemBlock {
                stems: std::array::from_fn(|s| [&stems[s][0][..], &stems[s][1][..]]),
            };
            self.graph.process(&blk, b, [&mut gl[..], &mut gr[..]]);
            // Graph output frame i is song frame song_in + i - L; keep frames >= start.
            for i in 0..b {
                let sf = self.song_in as i64 + i as i64 - lat as i64;
                if sf >= self.start as i64 {
                    pend[0].push(gl[i]);
                    pend[1].push(gr[i]);
                }
            }
            self.song_in += b as u64;
            while pend[0].len() >= b {
                let produced = match self.resampler.process([&pend[0][..b], &pend[1][..b]]) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("resampler: {e}");
                        self.running.store(false, Relaxed);
                        return;
                    }
                };
                let n_out = produced[0].len();
                let skip = rs_skip.min(n_out);
                rs_skip -= skip;
                let mut k = 0;
                for f in skip..n_out {
                    if pushed_dev >= expected_dev {
                        break;
                    }
                    let (mut l, mut r) = (produced[0][f], produced[1][f]);
                    if (pushed_dev as usize) < fade_len {
                        let g = pushed_dev as f32 / fade_len as f32;
                        l *= g;
                        r *= g;
                    }
                    match self.dev_ch {
                        1 => {
                            ilv[k] = 0.5 * (l + r);
                            k += 1;
                        }
                        n => {
                            ilv[k] = l;
                            ilv[k + 1] = r;
                            for z in 2..n {
                                ilv[k + z] = 0.0;
                            }
                            k += n;
                        }
                    }
                    pushed_dev += 1;
                }
                if let Ok(mut chunk) = self.out.write_chunk(k) {
                    let (a, bb) = chunk.as_mut_slices();
                    let na = a.len();
                    a.copy_from_slice(&ilv[..na]);
                    bb.copy_from_slice(&ilv[na..k]);
                    chunk.commit_all();
                }
                pend[0].drain(..b);
                pend[1].drain(..b);
            }
            let dt = now_ns() - t0;
            self.stats.blocks.fetch_add(1, Relaxed);
            self.stats.busy_ns.fetch_add(dt, Relaxed);
            self.stats.max_block_ns.fetch_max(dt, Relaxed);
            self.stats.graph_shortfall_frames.store(self.graph.stats().shift_shortfall_frames, Relaxed);
        }
    }
}

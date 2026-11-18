// SPDX-License-Identifier: GPL-3.0-or-later
//! Key change: render ahead and cache instead of shifting in real time (WORK_ORDER 4.5,
//! last bullet: "If a computer cannot shift in real time without glitching, render ahead
//! and cache instead. Playback must never stutter.").
//!
//! * **Modes.** `Realtime` is the normal path (D-31..D-36: Rubber Band R3 in the render
//!   worker, ~150 ms ahead). `Prerender`: a key change renders the song's pitch path on a
//!   low-priority thread into a disk cache, the previous key keeps playing, and once
//!   enough is rendered ahead the render worker switches with the same 40 ms equal-power
//!   crossfade (jk-dsp's cached route). The app's "auto" setting measures Rubber Band's
//!   real-time factor once per computer and sample rate ([`benchmark_realtime_factor`])
//!   and picks prerender below [`AUTO_MIN_REALTIME_FACTOR`] ([`choose_key_mode`]).
//! * **What is cached.** The graph's pitch path alone (`GraphConfig::pitched_only`): the
//!   shifted mix of lead x guide + backing + other (+ drums with "shift drums"), for one
//!   song, key and set of pitched-stem levels. Playback adds the live drum bus, which is
//!   exactly what the real-time route computes, so the output is bit-identical for the
//!   same start point. Per-stem caches (live levels for every stem) were measured and
//!   rejected: shifting stems separately gives a different waveform from shifting the mix
//!   (residual +0.5 to +1.5 dB relative to the signal on a real song; phase-vocoder
//!   phases are not additive) and costs 3x the CPU on exactly the computers that need this
//!   mode (`examples/keycache_linearity.rs`). So the drum level stays live and a change of
//!   another level renders a new cache entry while the old mix keeps playing.
//! * **Cells and runs.** An entry is a raw f32 stereo file covering the whole song, written
//!   in cells of [`CELL`] frames, appended in the order they are rendered (a cell map
//!   gives each cell's place; writing far into an empty preallocated file would make the
//!   file system zero-fill everything before it first, seconds on a slow USB stick). A *run* starts at a cell (reset + priming exactly like a
//!   seek of the real-time engine), renders forward, and stops at the song end, at the
//!   start of another run (crossfading into that run's pre-roll over the 40 ms before
//!   it: a seam, never a jump), or when interrupted (its last cell is kept in memory as a
//!   "tail" until a run starts right after it). Covered cells are never rewritten, so a
//!   reader never sees a half-written cell. The first run starts at the play position,
//!   the next fills the rest from the start of the song.
//! * **Cache folder.** `<root>/Settings/cache/keys` (set by the app; never a per-user
//!   folder). Complete entries are listed in `index.json` and evicted least recently used
//!   beyond the size cap. Partial entries live only for the session.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jk_dsp::{EqualPowerTable, ExtraBlock, GraphConfig, PitchGraph, StemBlock, CHANNELS};
use serde::{Deserialize, Serialize};

use crate::api::EngineError;
use crate::shared::{now_ns, Shared};
use crate::source::{slot_buffers, SongSource, SLOT_EXTRA_PITCHED, SLOT_EXTRA_UNPITCHED};

/// Frames per cache cell (0.68 s at 48 kHz). The unit of coverage and of disk writes.
pub const CELL: usize = 32_768;
/// Bump when the graph's output for the same input changes (stale caches are then never
/// used: the version is part of every entry id).
pub const CACHE_FORMAT: u32 = 1;
/// "Auto" picks the real-time path at or above this Rubber Band real-time factor
/// (stereo, the song's rate, one instance). Why 3: the real-time path runs one R3
/// instance (1/3 of a core at 3x); a key change runs two for L + 40 ms (~145 ms) and
/// primes 4096 frames in one block (2/3 of a core plus a burst); a fanless MacBook Air
/// sheds roughly 30 % of its clock when hot, and the UI (bubbles, ticker) shares the
/// CPU. Below 3x those together can exceed the 150 ms render-ahead and glitch.
pub const AUTO_MIN_REALTIME_FACTOR: f64 = 3.0;
/// Rendered audio wanted ahead of the switch point before a prerendered key is heard.
pub const READY_AHEAD_S: f64 = 5.0;
/// Render speed (x real time) above which "enough ahead" means `READY_AHEAD_S`; below it
/// the cache must reach the end of the song first (a slower renderer would be caught).
pub const READY_MIN_SPEED: f64 = 1.1;
/// Audio the feed reader buffers for the render worker.
const FEED_RING_S: f64 = 2.0;
const RENDER_BLOCK: usize = 1024;
const INDEX_NAME: &str = "index.json";

/// How key changes are made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    /// Rubber Band in the render worker (the normal path).
    #[default]
    Realtime,
    /// Render ahead into the key cache, switch when enough is ready.
    Prerender,
}

/// The "auto" decision: real time when the measured factor is at least
/// [`AUTO_MIN_REALTIME_FACTOR`], prerender otherwise or when nothing was measured yet
/// (the safe side: prerender never stutters, it is only slower to answer).
pub fn choose_key_mode(realtime_factor: Option<f64>) -> KeyMode {
    match realtime_factor {
        Some(x) if x.is_finite() && x >= AUTO_MIN_REALTIME_FACTOR => KeyMode::Realtime,
        _ => KeyMode::Prerender,
    }
}

/// A key or mix being prepared (prerender mode), for the UI: "Preparing key +2... 40%".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyPrepare {
    pub semitones: i32,
    /// `"key"` (a new key) or `"mix"` (same key, a pitched-stem level changed).
    pub reason: String,
    /// 0..1 of what must be rendered before the switch.
    pub progress: f32,
}

/// Key-change status for the UI (part of `Snapshot`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyStatus {
    /// The mode in effect.
    pub mode: KeyMode,
    /// Set while a prerendered key or mix is not heard yet.
    #[serde(default)]
    pub preparing: Option<KeyPrepare>,
    /// Plain note when prerender was asked for but cannot work (no cache folder, disk
    /// full): key changes then use the real-time path.
    #[serde(default)]
    pub note: Option<String>,
}

/// Measure Rubber Band R3's real-time factor on this computer: one stereo instance at
/// `rate` through the playback graph (256-frame quanta, HighQuality pitch, +5
/// semitones), on a synthetic song. Returns audio seconds per wall second, the slowest
/// of three 1.5 s runs (conservative). Takes about 0.5 s here (10x).
pub fn benchmark_realtime_factor(rate: u32) -> Result<f64, EngineError> {
    let seconds = 1.5;
    let stems = jk_dsp::signals::synth_song(rate, seconds, 17);
    let frames = stems[0][0].len();
    let mut worst = f64::INFINITY;
    for _ in 0..3 {
        let mut cfg = GraphConfig::new(rate);
        cfg.max_block = 512;
        let p = jk_dsp::MixParams { semitones: 5, guide: 1.0, ..Default::default() };
        let mut g = PitchGraph::new(cfg, p).map_err(EngineError::internal)?;
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let t = Instant::now();
        let mut pos = 0;
        while pos + 512 <= frames {
            let blk = StemBlock { stems: std::array::from_fn(|s| [&stems[s][0][pos..pos + 512], &stems[s][1][pos..pos + 512]]) };
            g.process(&blk, 512, [&mut l[..], &mut r[..]]);
            pos += 512;
        }
        let dt = t.elapsed().as_secs_f64().max(1e-6);
        worst = worst.min(pos as f64 / rate as f64 / dt);
    }
    Ok(worst)
}

// =====================================================================================
// Entry identity

/// What one cache entry holds (besides the song): key, drum routing and the levels that
/// go through the pitch shifter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EntryKey {
    pub semitones: i32,
    pub shift_drums: bool,
    /// Lead (guide), backing (0 when off), drums (only with shift drums, else 0), other.
    pub gains: [f32; 4],
}

impl EntryKey {
    /// `None` at key 0 (the bypass needs no cache).
    pub fn from_params(p: &jk_dsp::MixParams) -> Option<EntryKey> {
        let p = p.clamped();
        if p.semitones == 0 {
            return None;
        }
        let g = p.stem_gains();
        Some(EntryKey { semitones: p.semitones, shift_drums: p.shift_drums, gains: [g[0], g[1], if p.shift_drums { g[2] } else { 0.0 }, g[3]] })
    }

    /// The graph parameters a renderer uses (same stem gains as playback).
    pub fn graph_params(&self) -> jk_dsp::MixParams {
        jk_dsp::MixParams {
            guide: self.gains[0],
            backing_on: true,
            backing: self.gains[1],
            drums: self.gains[2],
            other: self.gains[3],
            shift_drums: self.shift_drums,
            semitones: self.semitones,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A stable identity of a song's audio: rate, length and every stem's role, routing,
/// stored gain, chunk length and CRC-32 (from the file's table of contents), hashed. The
/// per-song settings slots (rewritten on every key change) are not part of it.
pub(crate) fn song_identity(path: &Path) -> Result<String, EngineError> {
    use md5::{Digest, Md5};
    let f = jk_format::SongFile::open(path).map_err(|e| EngineError::from_format(&e))?;
    let m = f.manifest();
    let mut h = Md5::new();
    h.update(format!("jksong rate {} frames {} ch {}\n", m.sample_rate, m.duration_frames, m.channels));
    for st in &m.stems {
        let (len, crc) = f.chunk(&st.chunk).map_or((0, 0), |c| (c.length, c.crc32));
        h.update(format!(
            "{} {} {} {} {} {} {len} {crc}\n",
            st.role,
            st.codec,
            st.gain.to_bits(),
            st.in_karaoke_mix(),
            st.pitched(),
            st.chunk
        ));
    }
    Ok(hex(&h.finalize()))
}

/// Entry id: the song identity, the key and levels, the cache format and the Rubber
/// Band version, hashed.
pub(crate) fn entry_id(song: &str, rate: u32, frames: u64, k: &EntryKey) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(format!(
        "jkkey v{CACHE_FORMAT} rb {} song {song} rate {rate} frames {frames} key {} drums {} gains {:08x} {:08x} {:08x} {:08x}",
        jk_dsp::RUBBERBAND_VERSION,
        k.semitones,
        k.shift_drums,
        k.gains[0].to_bits(),
        k.gains[1].to_bits(),
        k.gains[2].to_bits(),
        k.gains[3].to_bits()
    ));
    hex(&h.finalize())
}

// =====================================================================================
// Entries and coverage

struct EntryState {
    /// Cells written to the file (never rewritten).
    covered: Vec<bool>,
    /// Byte offset of each written cell in the file (cells are appended).
    offsets: Vec<u64>,
    /// Bytes appended so far.
    end: u64,
    /// Run start cell -> that run's output for the crossfade length before it
    /// (interleaved), waiting for a run that reaches it from the left.
    prerolls: HashMap<usize, Vec<f32>>,
    /// Cell -> rendered audio (interleaved) not written yet: the run stopped there and
    /// the seam into the next cell is not known yet.
    tails: HashMap<usize, Vec<f32>>,
}

/// One cache entry (one song, key and pitched mix).
pub(crate) struct Entry {
    pub id: String,
    pub key: EntryKey,
    pub file_name: String,
    pub path: PathBuf,
    pub song: PathBuf,
    pub frames: u64,
    pub rate: u32,
    pub cells: usize,
    /// Session-unique tag (the render worker reports which entry it plays by tag).
    pub tag: u64,
    st: Mutex<EntryState>,
    covered_frames: AtomicU64,
    complete: AtomicBool,
    failed: AtomicBool,
    /// Recorded in the store's index.
    pub indexed: AtomicBool,
}

impl Entry {
    /// `order`: for a complete entry, the cells in file order (runs of `[first, count]`).
    #[allow(clippy::too_many_arguments)]
    fn new(id: String, key: EntryKey, dir: &Path, file_name: String, song: &Path, frames: u64, rate: u32, tag: u64, order: Option<&[[u32; 2]]>) -> Entry {
        let cells = (frames as usize).div_ceil(CELL).max(1);
        let complete = order.is_some();
        let mut offsets = vec![u64::MAX; cells];
        let mut end = 0u64;
        for &[first, count] in order.unwrap_or(&[]) {
            for c in first as usize..(first + count) as usize {
                if c < cells {
                    offsets[c] = end;
                    let a = (c * CELL) as u64;
                    end += ((a + CELL as u64).min(frames) - a) * (CHANNELS as u64) * 4;
                }
            }
        }
        Entry {
            id,
            key,
            path: dir.join(&file_name),
            file_name,
            song: song.to_path_buf(),
            frames,
            rate,
            cells,
            tag,
            st: Mutex::new(EntryState { covered: vec![complete; cells], offsets, end, prerolls: HashMap::new(), tails: HashMap::new() }),
            covered_frames: AtomicU64::new(if complete { frames } else { 0 }),
            complete: AtomicBool::new(complete),
            failed: AtomicBool::new(false),
            indexed: AtomicBool::new(complete),
        }
    }

    fn lock(&self) -> MutexGuard<'_, EntryState> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn bytes(&self) -> u64 {
        self.frames * (CHANNELS as u64) * 4
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Acquire)
    }

    pub fn has_failed(&self) -> bool {
        self.failed.load(Acquire)
    }

    fn cell_range(&self, c: usize) -> (u64, u64) {
        let a = (c * CELL) as u64;
        (a, (a + CELL as u64).min(self.frames))
    }

    /// Fraction of the song rendered.
    pub fn progress(&self) -> f32 {
        if self.frames == 0 {
            return 1.0;
        }
        (self.covered_frames.load(Relaxed) as f64 / self.frames as f64).min(1.0) as f32
    }

    /// Frames rendered contiguously from `frame` on (to the end of the song at most).
    pub fn covered_ahead(&self, frame: u64) -> u64 {
        if frame >= self.frames {
            return 0;
        }
        if self.is_complete() {
            return self.frames - frame;
        }
        let st = self.lock();
        let mut c = frame as usize / CELL;
        if !st.covered[c] {
            return 0;
        }
        while c < self.cells && st.covered[c] {
            c += 1;
        }
        self.cell_range(c - 1).1 - frame
    }

    /// Where frame `frame` is in the file (byte offset) and how many frames of its cell
    /// follow, if it is rendered.
    pub fn locate(&self, frame: u64) -> Option<(u64, usize)> {
        if frame >= self.frames {
            return None;
        }
        let c = frame as usize / CELL;
        let st = self.lock();
        if !st.covered[c] || st.offsets[c] == u64::MAX {
            return None;
        }
        let (a, b) = self.cell_range(c);
        Some((st.offsets[c] + (frame - a) * (CHANNELS as u64) * 4, (b - frame) as usize))
    }

    /// The cells in file order, as runs of `[first, count]` (for the index).
    fn file_order(&self) -> Vec<[u32; 2]> {
        let st = self.lock();
        let mut cells: Vec<usize> = (0..self.cells).filter(|&c| st.offsets[c] != u64::MAX).collect();
        cells.sort_by_key(|&c| st.offsets[c]);
        let mut runs: Vec<[u32; 2]> = Vec::new();
        for c in cells {
            match runs.last_mut() {
                Some([f, n]) if (*f + *n) as usize == c => *n += 1,
                _ => runs.push([c as u32, 1]),
            }
        }
        runs
    }

    fn claimed(st: &EntryState, c: usize) -> bool {
        st.covered[c] || st.tails.contains_key(&c)
    }

    /// The first cell nobody has rendered, at or after `from_cell`, else from the start;
    /// `held` counts as rendered (a running run's unwritten cell).
    fn pick_start(&self, from_cell: usize, held: Option<usize>) -> Option<usize> {
        let st = self.lock();
        let free = |c: usize| !Self::claimed(&st, c) && Some(c) != held;
        let from = from_cell.min(self.cells.saturating_sub(1));
        (from..self.cells).chain(0..from).find(|&c| free(c))
    }

    fn fail(&self) {
        self.failed.store(true, Release);
    }
}

/// Append cell `c` (interleaved `data`) to the file; returns its byte offset. The
/// write itself is done without the entry lock (only the renderer thread writes, and a
/// cell is marked covered only afterwards).
fn write_cell(entry: &Entry, file: &mut File, c: usize, data: &[f32], bytes: &mut Vec<u8>) -> std::io::Result<(usize, u64)> {
    let (a, b) = entry.cell_range(c);
    let n = ((b - a) as usize) * CHANNELS;
    bytes.clear();
    for v in &data[..n] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let at = {
        let mut st = entry.lock();
        let at = st.end;
        st.end += bytes.len() as u64;
        at
    };
    file.seek(SeekFrom::Start(at))?;
    file.write_all(bytes)?;
    Ok((c, at))
}

fn mark_covered(entry: &Entry, st: &mut EntryState, (c, at): (usize, u64)) {
    if !st.covered[c] {
        st.offsets[c] = at;
        st.covered[c] = true;
        let (a, b) = entry.cell_range(c);
        entry.covered_frames.fetch_add(b - a, Relaxed);
    }
}

/// Crossfade the last `len` frames of cell data `held` into `preroll` (equal power).
fn seam(held: &mut [f32], valid_frames: usize, preroll: &[f32], t: &EqualPowerTable) {
    let len = t.len().min(valid_frames).min(preroll.len() / CHANNELS);
    let start = valid_frames - len;
    for k in 0..len {
        for c in 0..CHANNELS {
            let i = (start + k) * CHANNELS + c;
            held[i] = held[i] * t.fade_out[k] + preroll[k * CHANNELS + c] * t.fade_in[k];
        }
    }
}

// =====================================================================================
// Store (folder, index, LRU eviction)

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexEntry {
    id: String,
    file: String,
    bytes: u64,
    frames: u64,
    rate: u32,
    /// Unix milliseconds of the last use (LRU order).
    last_used_ms: u64,
    /// Cells in file order, as runs of `[first, count]`.
    order: Vec<[u32; 2]>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    format: u32,
    entries: Vec<IndexEntry>,
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The cache folder: complete entries (in `index.json`, LRU order) and the files of the
/// session's partial entries.
pub(crate) struct CacheStore {
    pub dir: PathBuf,
    pub cap: u64,
    index: Vec<IndexEntry>,
    clock: u64,
}

impl CacheStore {
    /// Opens (creates) the folder, drops index entries whose file is missing or the
    /// wrong size, and deletes cache files the index does not list (partial entries of
    /// an earlier session).
    pub fn open(dir: &Path, cap: u64) -> std::io::Result<CacheStore> {
        std::fs::create_dir_all(dir)?;
        let idx: IndexFile = std::fs::read(dir.join(INDEX_NAME))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut index: Vec<IndexEntry> = if idx.format == CACHE_FORMAT { idx.entries } else { Vec::new() };
        index.retain(|e| {
            let cells = (e.frames as usize).div_ceil(CELL).max(1) as u64;
            !e.file.contains(['/', '\\'])
                && e.order.iter().map(|r| r[1] as u64).sum::<u64>() == cells
                && std::fs::metadata(dir.join(&e.file)).is_ok_and(|m| m.len() == e.bytes)
        });
        for f in std::fs::read_dir(dir)?.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let ours = name.ends_with(".f32") || name.ends_with(".tmp");
            if ours && !index.iter().any(|e| e.file == name) {
                let _ = std::fs::remove_file(f.path());
            }
        }
        let clock = index.iter().map(|e| e.last_used_ms).max().unwrap_or(0);
        let s = CacheStore { dir: dir.to_path_buf(), cap, index, clock };
        s.save();
        Ok(s)
    }

    fn tick(&mut self) -> u64 {
        // Strictly increasing, so LRU order is exact even within one millisecond.
        self.clock = unix_ms().max(self.clock + 1);
        self.clock
    }

    fn save(&self) {
        let tmp = self.dir.join(format!("{INDEX_NAME}.tmp"));
        let body = IndexFile { format: CACHE_FORMAT, entries: self.index.clone() };
        if let Ok(text) = serde_json::to_vec_pretty(&body)
            && std::fs::write(&tmp, text).is_ok()
        {
            let _ = std::fs::rename(&tmp, self.dir.join(INDEX_NAME));
        }
    }

    /// Bytes of the complete entries.
    pub fn total_bytes(&self) -> u64 {
        self.index.iter().map(|e| e.bytes).sum()
    }

    #[cfg(test)]
    pub fn ids(&self) -> Vec<String> {
        self.index.iter().map(|e| e.id.clone()).collect()
    }

    /// A complete entry's file name and cell order, marking it used.
    pub fn lookup(&mut self, id: &str) -> Option<(String, Vec<[u32; 2]>)> {
        let now = self.tick();
        let e = self.index.iter_mut().find(|e| e.id == id)?;
        e.last_used_ms = now;
        let f = (e.file.clone(), e.order.clone());
        self.save();
        Some(f)
    }

    /// Evict least recently used complete entries (not in `protect`) until
    /// `total + extra + need <= cap`. Returns the evicted ids. The cap is soft by the
    /// entries in use: they are never evicted.
    pub fn make_room(&mut self, need: u64, extra: u64, protect: &[&str]) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.total_bytes() + extra + need > self.cap {
            let victim = self
                .index
                .iter()
                .enumerate()
                .filter(|(_, e)| !protect.contains(&e.id.as_str()))
                .min_by_key(|(_, e)| e.last_used_ms)
                .map(|(i, _)| i);
            let Some(i) = victim else { break };
            let e = self.index.remove(i);
            let _ = std::fs::remove_file(self.dir.join(&e.file));
            evicted.push(e.id);
        }
        if !evicted.is_empty() {
            self.save();
        }
        evicted
    }

    /// Record a finished entry.
    pub fn add_complete(&mut self, e: &Entry) {
        let now = self.tick();
        self.index.retain(|x| x.id != e.id);
        self.index.push(IndexEntry {
            id: e.id.clone(),
            file: e.file_name.clone(),
            bytes: e.bytes(),
            frames: e.frames,
            rate: e.rate,
            last_used_ms: now,
            order: e.file_order(),
        });
        self.save();
    }
}

/// The session's cache: the store plus the entries of the loaded song.
pub(crate) struct KeyCache {
    pub store: CacheStore,
    /// Entries of the loaded song, most recently used first (at most 4 kept).
    entries: Vec<Arc<Entry>>,
    next_tag: u64,
    pub evictions: u64,
}

impl KeyCache {
    pub fn new(store: CacheStore) -> KeyCache {
        KeyCache { store, entries: Vec::new(), next_tag: 1, evictions: 0 }
    }

    /// Forget the loaded song's entries (partial files are deleted; complete ones stay).
    pub fn clear_song(&mut self) {
        for e in self.entries.drain(..) {
            if !e.is_complete() {
                let _ = std::fs::remove_file(&e.path);
            }
        }
    }

    /// Move finished entries into the index.
    pub fn index_finished(&mut self) {
        for e in &self.entries {
            if e.is_complete() && !e.indexed.swap(true, AcqRel) {
                self.store.add_complete(e);
            }
        }
    }

    /// The entry for `key` (from this session, the index, or a new partial entry with
    /// a preallocated file). `keep`: entries that must not be dropped or evicted.
    pub fn get_or_create(&mut self, song_id: &str, song: &Path, rate: u32, frames: u64, key: EntryKey, keep: &[u64]) -> Result<Arc<Entry>, String> {
        let id = entry_id(song_id, rate, frames, &key);
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let e = self.entries.remove(i);
            if e.is_complete() {
                let _ = self.store.lookup(&id);
            }
            self.entries.insert(0, e.clone());
            return Ok(e);
        }
        let tag = self.next_tag;
        self.next_tag += 1;
        let e = if let Some((file, order)) = self.store.lookup(&id) {
            Arc::new(Entry::new(id, key, &self.store.dir, file, song, frames, rate, tag, Some(&order)))
        } else {
            let partial: u64 = self.entries.iter().filter(|e| !e.is_complete()).map(|e| e.bytes()).sum();
            let protect: Vec<String> = self.entries.iter().map(|e| e.id.clone()).collect();
            let protect: Vec<&str> = protect.iter().map(String::as_str).collect();
            let need = frames * (CHANNELS as u64) * 4;
            self.evictions += self.store.make_room(need, partial, &protect).len() as u64;
            let file = format!("{id}-{:08x}.f32", (now_ns() as u32) ^ (tag as u32).wrapping_mul(0x9e37_79b9));
            let path = self.store.dir.join(&file);
            // Created empty: cells are appended as they are rendered.
            OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Arc::new(Entry::new(id, key, &self.store.dir, file, song, frames, rate, tag, None))
        };
        self.entries.insert(0, e.clone());
        // Keep at most 4 entries in memory; drop unused partial ones (and their files).
        while self.entries.len() > 4 {
            let Some(i) = self.entries.iter().rposition(|x| !keep.contains(&x.tag) && x.tag != e.tag) else { break };
            let x = self.entries.remove(i);
            if !x.is_complete() {
                let _ = std::fs::remove_file(&x.path);
            }
        }
        Ok(e)
    }

    pub fn find_tag(&self, tag: u64) -> Option<Arc<Entry>> {
        self.entries.iter().find(|e| e.tag == tag).cloned()
    }
}

// =====================================================================================
// Renderer thread

struct Job {
    entry: Arc<Entry>,
}

/// Shared state of the key renderer thread.
pub(crate) struct Renderer {
    job: Mutex<Option<Job>>,
    cv: Condvar,
    job_gen: AtomicU64,
    priority_frame: AtomicU64,
    priority_gen: AtomicU64,
    /// Render speed, x real time x 1000 (0 = not measured yet), of the current job.
    speed_milli: AtomicU32,
    pub stop: AtomicBool,
    pub cells: AtomicU64,
    pub seams: AtomicU64,
    pub interrupts: AtomicU64,
    shared: Arc<Shared>,
}

impl Renderer {
    pub fn new(shared: Arc<Shared>) -> Arc<Renderer> {
        Arc::new(Renderer {
            job: Mutex::new(None),
            cv: Condvar::new(),
            job_gen: AtomicU64::new(0),
            priority_frame: AtomicU64::new(0),
            priority_gen: AtomicU64::new(0),
            speed_milli: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            cells: AtomicU64::new(0),
            seams: AtomicU64::new(0),
            interrupts: AtomicU64::new(0),
            shared,
        })
    }

    pub fn spawn(self: &Arc<Self>) -> std::io::Result<std::thread::JoinHandle<()>> {
        let me = self.clone();
        std::thread::Builder::new().name("jk-key-render".into()).spawn(move || me.run())
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Release);
        self.cv.notify_all();
    }

    /// The entry being rendered (tag), if any.
    pub fn job_tag(&self) -> Option<u64> {
        self.job.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|j| j.entry.tag)
    }

    /// Render `entry` (None: stop rendering), from `priority` first.
    pub fn set_job(&self, entry: Option<Arc<Entry>>, priority: u64) {
        let mut j = self.job.lock().unwrap_or_else(|p| p.into_inner());
        let same = match (&*j, &entry) {
            (Some(a), Some(b)) => a.entry.tag == b.tag,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.speed_milli.store(0, Relaxed);
        self.priority_frame.store(priority, Relaxed);
        *j = entry.map(|entry| Job { entry });
        self.job_gen.fetch_add(1, AcqRel);
        self.cv.notify_all();
    }

    /// Where coverage is needed first (a seek, a new switch point).
    pub fn set_priority(&self, frame: u64) {
        self.priority_frame.store(frame, Relaxed);
        self.priority_gen.fetch_add(1, AcqRel);
    }

    /// Measured render speed (x real time), `None` before the first cell.
    pub fn speed(&self) -> Option<f64> {
        let m = self.speed_milli.load(Relaxed);
        (m > 0).then_some(m as f64 / 1000.0)
    }

    fn run(self: Arc<Self>) {
        demote_current_thread();
        let mut src: Option<(PathBuf, SongSource)> = None;
        let mut bytes = Vec::new();
        while !self.stop.load(Acquire) {
            let job = self.job_gen.load(Acquire);
            let entry = {
                let j = self.job.lock().unwrap_or_else(|p| p.into_inner());
                let e = j.as_ref().map(|j| j.entry.clone());
                match e {
                    Some(e) if !e.is_complete() && !e.has_failed() => Some(e),
                    _ => {
                        let _ = self.cv.wait_timeout(j, Duration::from_millis(50));
                        None
                    }
                }
            };
            let Some(entry) = entry else { continue };
            if src.as_ref().is_none_or(|(p, _)| *p != entry.song) {
                src = match SongSource::open(&entry.song) {
                    Ok(s) => Some((entry.song.clone(), s)),
                    Err(_) => {
                        entry.fail();
                        None
                    }
                };
            }
            let Some((_, source)) = src.as_mut() else { continue };
            let mut file = match OpenOptions::new().read(true).write(true).open(&entry.path) {
                Ok(f) => f,
                Err(_) => {
                    entry.fail();
                    continue;
                }
            };
            // Runs until the entry is complete or the job changes.
            while !self.stop.load(Acquire) && self.job_gen.load(Acquire) == job {
                let pgen = self.priority_gen.load(Acquire);
                let from = self.priority_frame.load(Relaxed) as usize / CELL;
                let Some(start) = entry.pick_start(from, None) else {
                    let _ = file.sync_data();
                    entry.complete.store(true, Release);
                    break;
                };
                if let Err(_e) = self.render_run(&entry, source, &mut file, start, job, pgen, &mut bytes) {
                    entry.fail();
                    break;
                }
            }
        }
    }

    /// One run from cell `start` (see the module documentation).
    #[allow(clippy::too_many_arguments)]
    fn render_run(&self, entry: &Entry, src: &mut SongSource, file: &mut File, start: usize, job: u64, mut pgen: u64, bytes: &mut Vec<u8>) -> std::io::Result<()> {
        let mut cfg = GraphConfig::new(entry.rate);
        cfg.max_block = RENDER_BLOCK;
        cfg.pitched_only = true;
        let mut graph = PitchGraph::new(cfg, entry.key.graph_params()).map_err(std::io::Error::other)?;
        let lat = graph.latency() as i64;
        let xf = graph.crossfade_len();
        let table = EqualPowerTable::new(xf);
        let s = (start * CELL) as u64;
        let read_from = s.saturating_sub(graph.prime_len() as u64);
        // Exactly what the real-time engine does at a seek to `s` (graph reset, priming
        // from `s - prime_len`).
        graph.reset();
        src.seek(read_from).map_err(|e| std::io::Error::other(e.to_string()))?;
        let want_from = if start > 0 { s - xf as u64 } else { 0 };
        let mut preroll: Vec<f32> = Vec::with_capacity(xf * CHANNELS);
        let mut cur: Vec<f32> = Vec::with_capacity(CELL * CHANNELS);
        let mut held: Option<(usize, Vec<f32>)> = None;
        let mut cell = start;
        let mut song_in = read_from;
        let mut bufs = slot_buffers(RENDER_BLOCK);
        let (mut l, mut r) = (vec![0.0f32; RENDER_BLOCK], vec![0.0f32; RENDER_BLOCK]);
        let has_extra = src.info().has_extra();
        let mut cell_t = Instant::now();
        loop {
            if self.stop.load(Acquire) {
                return Ok(());
            }
            // Interruption: job change, or a seek that needs another place first.
            let new_pgen = self.priority_gen.load(Acquire);
            let changed = self.job_gen.load(Acquire) != job;
            if changed || new_pgen != pgen {
                pgen = new_pgen;
                let held_cell = held.as_ref().map(|h| h.0);
                let want = entry.pick_start(self.priority_frame.load(Relaxed) as usize / CELL, held_cell);
                if changed || want != Some(cell) {
                    self.interrupts.fetch_add(1, Relaxed);
                    if let Some((c, data)) = held.take() {
                        entry.lock().tails.insert(c, data);
                    }
                    return Ok(());
                }
            }
            let t0 = now_ns();
            src.read(&mut bufs, RENDER_BLOCK).map_err(|e| std::io::Error::other(e.to_string()))?;
            let blk = StemBlock { stems: std::array::from_fn(|k| [&bufs[k][0][..], &bufs[k][1][..]]) };
            let extra = ExtraBlock {
                pitched: [&bufs[SLOT_EXTRA_PITCHED][0][..], &bufs[SLOT_EXTRA_PITCHED][1][..]],
                unpitched: [&bufs[SLOT_EXTRA_UNPITCHED][0][..], &bufs[SLOT_EXTRA_UNPITCHED][1][..]],
            };
            graph.process_ext(&blk, has_extra.then_some(&extra), RENDER_BLOCK, [&mut l[..], &mut r[..]]);
            slow_down(&self.shared, t0);
            for i in 0..RENDER_BLOCK {
                let f = song_in as i64 + i as i64 - lat;
                if f < want_from as i64 || f as u64 >= entry.frames {
                    continue;
                }
                let f = f as u64;
                if f < s {
                    preroll.push(l[i]);
                    preroll.push(r[i]);
                    continue;
                }
                cur.push(l[i]);
                cur.push(r[i]);
                let (_, end) = entry.cell_range(cell);
                if f + 1 < end {
                    continue;
                }
                // Cell `cell` is complete.
                self.cells.fetch_add(1, Relaxed);
                let dt = cell_t.elapsed().as_secs_f64().max(1e-6);
                cell_t = Instant::now();
                let speed = ((end - (cell * CELL) as u64) as f64 / entry.rate as f64) / dt;
                let old = self.speed_milli.load(Relaxed) as f64 / 1000.0;
                let ema = if old > 0.0 { 0.7 * old + 0.3 * speed } else { speed };
                self.speed_milli.store(((ema * 1000.0).round() as u32).max(1), Relaxed);
                // Plan under the lock (only this thread changes the state), write
                // without it, then mark the written cells covered.
                let next = cell + 1;
                let (left_tail, next_pre, next_taken) = {
                    let mut st = entry.lock();
                    let left_tail = if cell == start && start > 0 {
                        // Left seam: an interrupted run's tail just before us, or wait for one.
                        let t = st.tails.remove(&(start - 1));
                        if t.is_none() {
                            st.prerolls.insert(start, std::mem::take(&mut preroll));
                        }
                        t
                    } else {
                        None
                    };
                    let next_pre = if next < entry.cells { st.prerolls.remove(&next) } else { None };
                    let next_taken = next < entry.cells && (st.covered[next] || st.tails.contains_key(&next));
                    (left_tail, next_pre, next_taken)
                };
                let mut written: [Option<(usize, u64)>; 3] = [None; 3];
                if let Some(mut tail) = left_tail {
                    let n = (entry.cell_range(start - 1).1 - entry.cell_range(start - 1).0) as usize;
                    seam(&mut tail, n, &preroll, &table);
                    written[0] = Some(write_cell(entry, file, start - 1, &tail, bytes)?);
                    self.seams.fetch_add(1, Relaxed);
                }
                if let Some((c, data)) = held.take() {
                    written[1] = Some(write_cell(entry, file, c, &data, bytes)?);
                }
                let mut done = std::mem::replace(&mut cur, Vec::with_capacity(CELL * CHANNELS));
                let finished = if next >= entry.cells {
                    true
                } else if let Some(pre) = next_pre {
                    // Seam into the run that starts at the next cell.
                    let n = (end - (cell * CELL) as u64) as usize;
                    seam(&mut done, n, &pre, &table);
                    self.seams.fetch_add(1, Relaxed);
                    true
                } else {
                    // `next_taken` cannot happen (runs start only at free cells); a hard
                    // joint is still better than rendering a cell twice.
                    next_taken
                };
                if finished {
                    written[2] = Some(write_cell(entry, file, cell, &done, bytes)?);
                }
                {
                    let mut st = entry.lock();
                    for w in written.into_iter().flatten() {
                        mark_covered(entry, &mut st, w);
                    }
                }
                if finished {
                    return Ok(());
                }
                held = Some((cell, done));
                cell = next;
            }
            song_in += RENDER_BLOCK as u64;
        }
    }
}

/// Test hook (`Engine::debug_set_slowdown`): make the calling thread `factor` times
/// slower for the work since `t0` by spinning, as a slower CPU would be.
pub(crate) fn slow_down(shared: &Shared, t0: u64) {
    let f = shared.debug_slowdown_x100.load(Relaxed);
    if f > 100 {
        let dt = now_ns().saturating_sub(t0);
        let until = t0 + dt * f as u64 / 100;
        while now_ns() < until {
            std::hint::spin_loop();
        }
    }
}

fn demote_current_thread() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        // SAFETY: plain Win32 call on the current thread's pseudo handle.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }
}

// =====================================================================================
// Feeds: cached frames for the render worker

/// Cached pitch-path frames for the render worker, from song frame `from` on. Built by
/// control, read by the worker (lock-free ring), filled by a reader thread.
pub(crate) struct CacheFeed {
    pub generation: u64,
    pub from: u64,
    /// Next song frame the ring will deliver.
    pub next: u64,
    /// Song length: frames from here on are silence (no ring data).
    pub frames: u64,
    pub ring: rtrb::Consumer<f32>,
    pub semitones: i32,
    pub drums_in: bool,
    pub tag: u64,
}

/// Start a reader thread that streams `entry` from `from` into a new ring (2 s). It
/// waits for frames that are not rendered yet, and ends when the worker drops the ring
/// (through the garbage ring), at the end of the song, on `stop`, or on a read error
/// (the worker then sees a missing block and fades to the bypass).
pub(crate) fn spawn_feed(entry: Arc<Entry>, from: u64, generation: u64, stop: Arc<AtomicBool>) -> Result<Box<CacheFeed>, EngineError> {
    let cap = ((FEED_RING_S * entry.rate as f64) as usize).max(4 * RENDER_BLOCK);
    let (mut prod, cons) = rtrb::RingBuffer::<f32>::new(cap * CHANNELS);
    let mut file = File::open(&entry.path).map_err(EngineError::internal)?;
    let feed = Box::new(CacheFeed {
        generation,
        from,
        next: from,
        frames: entry.frames,
        ring: cons,
        semitones: entry.key.semitones,
        drums_in: entry.key.shift_drums,
        tag: entry.tag,
    });
    std::thread::Builder::new()
        .name("jk-key-feed".into())
        .spawn(move || {
            const CHUNK: usize = 8192;
            let mut pos = from.min(entry.frames);
            let mut bytes = vec![0u8; CHUNK * CHANNELS * 4];
            loop {
                if prod.is_abandoned() || stop.load(Acquire) || pos >= entry.frames {
                    return;
                }
                let space = prod.slots() / CHANNELS;
                // Wait for room in the ring, or for the renderer to write this cell.
                let Some((at, in_cell)) = entry.locate(pos).filter(|_| space > 0) else {
                    std::thread::sleep(Duration::from_millis(4));
                    continue;
                };
                let n = space.min(in_cell).min(CHUNK);
                let b = &mut bytes[..n * CHANNELS * 4];
                if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(b).is_err() {
                    return;
                }
                let Ok(mut chunk) = prod.write_chunk_uninit(n * CHANNELS) else { continue };
                let (x, y) = chunk.as_mut_slices();
                for (k, w) in b.as_chunks::<4>().0.iter().enumerate() {
                    let v = f32::from_le_bytes(*w);
                    if k < x.len() {
                        x[k].write(v);
                    } else {
                        y[k - x.len()].write(v);
                    }
                }
                // SAFETY: all n * CHANNELS slots were written above.
                unsafe { chunk.commit_all() };
                pos += n as u64;
            }
        })
        .map_err(EngineError::internal)?;
    Ok(feed)
}

/// What the worker got from a feed for one block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pull {
    /// The block ends before the feed starts.
    NotYet,
    /// `buf` holds the block (zeros outside the feed's range).
    Data,
    /// Frames the feed should have are not in the ring.
    Missing,
}

/// Real-time side: frames `[song_in, song_in + n)` of `feed` into `buf` (planar).
/// No allocation, no locks: ring reads only.
pub(crate) fn pull(feed: &mut CacheFeed, song_in: u64, n: usize, buf: &mut [Vec<f32>; CHANNELS]) -> Pull {
    let end = song_in + n as u64;
    if end <= feed.from {
        return Pull::NotYet;
    }
    // Drop frames the worker has already passed (a switch that started late).
    if feed.next < song_in && feed.next < feed.frames {
        let want = (song_in.min(feed.frames) - feed.next) as usize;
        let k = (feed.ring.slots() / CHANNELS).min(want);
        if let Ok(c) = feed.ring.read_chunk(k * CHANNELS) {
            c.commit_all();
        }
        feed.next += k as u64;
        if feed.next < song_in.min(feed.frames) {
            return Pull::Missing;
        }
    }
    for b in buf.iter_mut() {
        b[..n].iter_mut().for_each(|v| *v = 0.0);
    }
    let lo = song_in.max(feed.from).max(feed.next.min(feed.frames));
    let hi = end.min(feed.frames);
    if lo < hi {
        let need = (hi - lo) as usize;
        if feed.ring.slots() < need * CHANNELS {
            return Pull::Missing;
        }
        if let Ok(c) = feed.ring.read_chunk(need * CHANNELS) {
            let (x, y) = c.as_slices();
            let off = (lo - song_in) as usize;
            for i in 0..need {
                for ch in 0..CHANNELS {
                    let k = i * CHANNELS + ch;
                    buf[ch][off + i] = if k < x.len() { x[k] } else { y[k - x.len()] };
                }
            }
            c.commit_all();
        }
        feed.next = hi;
    }
    Pull::Data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jk-keycache-unit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn auto_mode_threshold() {
        assert_eq!(choose_key_mode(None), KeyMode::Prerender);
        assert_eq!(choose_key_mode(Some(f64::NAN)), KeyMode::Prerender);
        assert_eq!(choose_key_mode(Some(0.8)), KeyMode::Prerender);
        assert_eq!(choose_key_mode(Some(2.99)), KeyMode::Prerender);
        assert_eq!(choose_key_mode(Some(3.0)), KeyMode::Realtime);
        assert_eq!(choose_key_mode(Some(11.0)), KeyMode::Realtime);
    }

    #[test]
    fn entry_key_ignores_what_stays_live() {
        let p = jk_dsp::MixParams { semitones: 2, ..Default::default() };
        assert!(EntryKey::from_params(&jk_dsp::MixParams { semitones: 0, ..p }).is_none());
        let a = EntryKey::from_params(&p).unwrap();
        // The drum level is live unless the drums are shifted.
        assert_eq!(a, EntryKey::from_params(&jk_dsp::MixParams { drums: 0.3, ..p }).unwrap());
        assert_ne!(a, EntryKey::from_params(&jk_dsp::MixParams { drums: 0.3, shift_drums: true, ..p }).unwrap());
        // Backing off == backing at 0, whatever its level.
        let off = EntryKey::from_params(&jk_dsp::MixParams { backing_on: false, backing: 0.7, ..p }).unwrap();
        assert_eq!(off.gains[1], 0.0);
        assert_ne!(a, EntryKey::from_params(&jk_dsp::MixParams { guide: 0.5, ..p }).unwrap());
        let id = entry_id("song", 44_100, 1000, &a);
        assert_eq!(id.len(), 32);
        assert_ne!(id, entry_id("song", 44_100, 1000, &EntryKey { semitones: 3, ..a }));
    }

    #[test]
    fn lru_eviction_keeps_the_cap_and_protected_entries() {
        let dir = scratch("lru");
        let key = EntryKey { semitones: 1, shift_drums: false, gains: [0.0, 1.0, 0.0, 1.0] };
        let frames = 10_000u64; // 80 000 bytes each
        let bytes = frames * 8;
        let mut kc = KeyCache::new(CacheStore::open(&dir, 3 * bytes).unwrap());
        let mut ids = Vec::new();
        for k in 1..=3 {
            let e = kc.get_or_create("s", Path::new("x"), 44_100, frames, EntryKey { semitones: k, ..key }, &[]).unwrap();
            // As if rendered: its one cell written.
            std::fs::write(&e.path, vec![0u8; bytes as usize]).unwrap();
            mark_covered(&e, &mut e.lock(), (0, 0));
            e.complete.store(true, Release);
            ids.push(e.id.clone());
            kc.index_finished();
        }
        assert_eq!(kc.store.total_bytes(), 3 * bytes);
        // Use entry 1 again: entry 2 becomes the least recently used.
        kc.clear_song();
        assert!(kc.get_or_create("s", Path::new("x"), 44_100, frames, EntryKey { semitones: 1, ..key }, &[]).unwrap().is_complete());
        kc.clear_song();
        let e4 = kc.get_or_create("s", Path::new("x"), 44_100, frames, EntryKey { semitones: 4, ..key }, &[]).unwrap();
        assert!(!e4.is_complete());
        assert_eq!(kc.evictions, 1);
        let left = kc.store.ids();
        assert!(!left.contains(&ids[1]), "least recently used entry evicted");
        assert!(left.contains(&ids[0]) && left.contains(&ids[2]));
        assert!(kc.store.total_bytes() + e4.bytes() <= 3 * bytes);
        let files = std::fs::read_dir(&dir).unwrap().filter(|f| f.as_ref().unwrap().file_name().to_string_lossy().ends_with(".f32")).count();
        assert_eq!(files, 3, "two complete + one partial file on disk");
        // A new session: the partial file is deleted, the index survives.
        drop(kc);
        let store = CacheStore::open(&dir, 3 * bytes).unwrap();
        assert_eq!(store.ids().len(), 2);
        let files = std::fs::read_dir(&dir).unwrap().filter(|f| f.as_ref().unwrap().file_name().to_string_lossy().ends_with(".f32")).count();
        assert_eq!(files, 2);
        // A cap below one entry never evicts what is protected.
        let mut store = store;
        store.cap = 1;
        let protect = vec![ids[0].as_str()];
        store.make_room(bytes, 0, &protect);
        assert_eq!(store.ids(), vec![ids[0].clone()]);
    }

    #[test]
    fn pick_start_prefers_the_play_position_then_wraps() {
        let dir = scratch("pick");
        let key = EntryKey { semitones: 1, shift_drums: false, gains: [0.0; 4] };
        let e = Entry::new("x".into(), key, &dir, "x.f32".into(), Path::new("s"), (CELL * 10) as u64, 44_100, 1, None);
        assert_eq!(e.pick_start(4, None), Some(4));
        {
            let mut st = e.lock();
            for c in 4..10 {
                st.covered[c] = true;
            }
            st.tails.insert(1, vec![]);
        }
        assert_eq!(e.pick_start(4, None), Some(0));
        assert_eq!(e.pick_start(4, Some(0)), Some(2));
        assert_eq!(e.covered_ahead((CELL * 5 + 7) as u64), (CELL * 5 - 7) as u64);
        assert_eq!(e.covered_ahead(0), 0);
    }

    /// Reference: the pitch path of a run that starts at song frame `start` (reset and
    /// priming as at a real-time seek), for song frames `[lo, frames)`, interleaved.
    fn pitched_run(song: &Path, key: &EntryKey, start: u64, lo: u64) -> Vec<f32> {
        let mut src = SongSource::open(song).unwrap();
        let frames = src.info().frames;
        let mut cfg = GraphConfig::new(src.info().sample_rate);
        cfg.max_block = 300; // a different block size: the output must not depend on it
        cfg.pitched_only = true;
        let mut g = PitchGraph::new(cfg, key.graph_params()).unwrap();
        g.reset();
        let read_from = start.saturating_sub(g.prime_len() as u64);
        src.seek(read_from).unwrap();
        let mut bufs = slot_buffers(300);
        let (mut l, mut r) = (vec![0.0f32; 300], vec![0.0f32; 300]);
        let mut out = Vec::new();
        let mut pos = read_from;
        while out.len() < ((frames - lo) * 2) as usize {
            src.read(&mut bufs, 300).unwrap();
            let blk = StemBlock { stems: std::array::from_fn(|k| [&bufs[k][0][..], &bufs[k][1][..]]) };
            g.process(&blk, 300, [&mut l[..], &mut r[..]]);
            for i in 0..300 {
                let f = pos as i64 + i as i64 - g.latency() as i64;
                if f >= lo as i64 && (f as u64) < frames {
                    out.push(l[i]);
                    out.push(r[i]);
                }
            }
            pos += 300;
        }
        out
    }

    #[test]
    fn two_runs_meet_with_a_crossfade_and_every_cell_reads_back() {
        let dir = scratch("runs");
        let song = dir.join("s.jksong");
        crate::demo::make_synth_song(&song, 6.0, 44_100, 5).unwrap();
        let frames = SongSource::open(&song).unwrap().info().frames;
        let key = EntryKey { semitones: 3, shift_drums: false, gains: [0.5, 1.0, 0.0, 1.0] };
        let mut kc = KeyCache::new(CacheStore::open(&dir.join("keys"), 1 << 30).unwrap());
        let e = kc.get_or_create("s", &song, 44_100, frames, key, &[]).unwrap();
        let r = Renderer::new(Arc::new(Shared::new(1.0)));
        let h = r.spawn().unwrap();
        // As after a key change at 2.2 s: the run from cell 3 first, then the rest.
        let s = (3 * CELL) as u64;
        r.set_job(Some(e.clone()), s);
        let t = Instant::now();
        while !e.is_complete() {
            assert!(t.elapsed() < Duration::from_secs(60), "render did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        r.shutdown();
        h.join().unwrap();
        assert_eq!((r.seams.load(Relaxed), r.interrupts.load(Relaxed)), (1, 0));
        // The file holds cells 3.. first, then 0..3 (appended in render order).
        assert_eq!(e.file_order(), vec![[3, e.cells as u32 - 3], [0, 3]]);
        assert_eq!(std::fs::metadata(&e.path).unwrap().len(), e.bytes());
        // Read everything back through the cell map.
        let mut f = File::open(&e.path).unwrap();
        let mut got = Vec::new();
        let mut pos = 0u64;
        while pos < frames {
            let (at, n) = e.locate(pos).expect("covered");
            let mut b = vec![0u8; n * 8];
            f.seek(SeekFrom::Start(at)).unwrap();
            f.read_exact(&mut b).unwrap();
            got.extend(b.as_chunks::<4>().0.iter().map(|w| f32::from_le_bytes(*w)));
            pos += n as u64;
        }
        // Expected: run A (from 0) up to the seam, the equal-power crossfade into run
        // B's pre-roll over the 40 ms before cell 3, run B (from cell 3) after it.
        let xf = (40.0f64 * 44.1).round() as u64;
        let a = pitched_run(&song, &key, 0, 0);
        let b = pitched_run(&song, &key, s, s - xf);
        let tab = EqualPowerTable::new(xf as usize);
        let mut bad = 0;
        for fr in 0..frames {
            for c in 0..2 {
                let i = (fr * 2) as usize + c;
                let want = if fr < s - xf {
                    a[i]
                } else if fr < s {
                    let k = (fr - (s - xf)) as usize;
                    a[i] * tab.fade_out[k] + b[((fr - (s - xf)) * 2) as usize + c] * tab.fade_in[k]
                } else {
                    b[((fr - (s - xf)) * 2) as usize + c]
                };
                if got[i].to_bits() != want.to_bits() {
                    bad += 1;
                }
            }
        }
        assert_eq!(bad, 0, "cache content differs from run A | seam | run B");
    }
}

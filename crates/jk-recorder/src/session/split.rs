// SPDX-License-Identifier: GPL-3.0-or-later
//! Splitting a session's master into songs (see the module above for the layout).
//!
//! 1. **Boundaries** from the markers: every track change, plus "playback ended"
//!    (the first pause / stop / close after the last time playback started, when it
//!    never resumed) and, with a song limit, the start of the song after the last
//!    one. Track changes closer together than 1.5 s are one change (players often
//!    announce a track in two steps); the first instant is kept with the last
//!    metadata.
//! 2. **Cuts**: each boundary is moved to the quietest point within 2.5 s of the
//!    event (and no further than half-way to the neighbouring boundaries, so two
//!    close track changes never share one gap): a run of frames whose samples are
//!    all below -70 dBFS on every channel, at least 100 ms long. The run nearest the
//!    event wins (one containing it is at
//!    distance 0; a tie goes to the longer run); a track change is cut in the middle
//!    of the run, "playback ended" 1 s into it at most (the rest of the silence is
//!    not part of the song). No run: the cut stays at the event, "uncertain".
//! 3. **Parts** between the cuts: songs, and excluded parts (advertisements, what
//!    came before the first song or after playback ended, skips shorter than 5 s).
//! 4. **Files**: each part is copied sample for sample into its own float WAV
//!    (temporary file, flushed, renamed), read back and compared with the master
//!    (length and a 64-bit hash of the bytes). Songs get a sidecar and their cover.
//!    The session file is saved after every part, so an interrupted split resumes:
//!    files that are already there and match are kept.
//! 5. Only when every part matched and the parts cover the master exactly is the
//!    master deleted (and the cover folder); otherwise it is kept and the session
//!    says why.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::naming::{safe_stem, song_stem};
use super::{
    BoundaryInfo, COVERS_DIR, Confidence, EXCLUDED_DIR, Marker, MarkerKind, PartKind, PartRecord, SONG_FORMAT,
    SessionFile, SessionState, SongSidecar, SongStatus, copy_atomic, looks_like_ad, remove_temp_files, temp_path,
    write_atomic,
};
use crate::wav::{WavInfo, WavSpec, WavWriter, read_header};

/// The splitter's tuning. The defaults are what the app uses.
#[derive(Clone, Debug)]
pub struct SplitSettings {
    /// A frame is "quiet" when every channel's sample is below this (dBFS).
    pub silence_dbfs: f64,
    /// Shortest quiet run that counts as a gap between songs (s).
    pub min_gap_s: f64,
    /// How far from the player's event a gap is looked for (s, each side).
    pub search_s: f64,
    /// After playback ended, at most this much of the silence stays at the end of
    /// the song (s).
    pub end_keep_s: f64,
    /// Track changes closer together than this are one change (s).
    pub merge_s: f64,
    /// A song part shorter than this is a skip, not a song (s).
    pub min_song_s: f64,
    /// Delete the master after a fully verified split.
    pub delete_master: bool,
}

impl Default for SplitSettings {
    fn default() -> Self {
        Self {
            silence_dbfs: -70.0,
            min_gap_s: 0.1,
            search_s: 2.5,
            end_keep_s: 1.0,
            merge_s: 1.5,
            min_song_s: 5.0,
            delete_master: true,
        }
    }
}

/// What [`split`] did.
#[derive(Clone, Debug)]
pub struct SplitOutcome {
    /// The session file as saved at the end.
    pub session: SessionFile,
    /// The song files with their sidecars, in order.
    pub songs: Vec<(PathBuf, SongSidecar)>,
    /// True if the master was deleted (every part verified).
    pub master_deleted: bool,
}

// ---------------------------------------------------------------------------
// Reading the master
// ---------------------------------------------------------------------------

/// Random access to interleaved frames.
pub trait FrameSource {
    /// Channels per frame.
    fn channels(&self) -> usize;
    /// Frames available.
    fn frames(&self) -> u64;
    /// The raw little-endian bytes of frames `start..start+len` (clamped to the end).
    fn read_bytes(&mut self, start: u64, len: u64) -> io::Result<Vec<u8>>;
    /// The samples of frames `start..start+len` (clamped to the end).
    fn read(&mut self, start: u64, len: u64) -> io::Result<Vec<f32>> {
        let b = self.read_bytes(start, len)?;
        Ok(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
    }
}

/// A 32-bit float WAV / RF64 file read in place.
pub struct WavSource {
    file: File,
    info: WavInfo,
    frames: u64,
}

impl WavSource {
    /// Open a float WAV; the length is what is really on disk (a header that
    /// claims more is not trusted).
    pub fn open(path: &Path) -> io::Result<WavSource> {
        let info = read_header(path)?;
        let file = File::open(path)?;
        let on_disk = file.metadata()?.len().saturating_sub(info.data_offset) / (4 * u64::from(info.channels));
        Ok(WavSource {
            file,
            info,
            frames: info.frames.min(on_disk),
        })
    }

    /// The header.
    pub fn info(&self) -> WavInfo {
        self.info
    }
}

impl FrameSource for WavSource {
    fn channels(&self) -> usize {
        usize::from(self.info.channels)
    }
    fn frames(&self) -> u64 {
        self.frames
    }
    fn read_bytes(&mut self, start: u64, len: u64) -> io::Result<Vec<u8>> {
        let start = start.min(self.frames);
        let len = len.min(self.frames - start);
        let block = 4 * u64::from(self.info.channels);
        self.file.seek(SeekFrom::Start(self.info.data_offset + start * block))?;
        let mut buf = vec![0u8; usize::try_from(len * block).map_err(io::Error::other)?];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Interleaved samples in memory (tests, and anything already loaded).
pub struct MemSource {
    /// The samples.
    pub samples: Vec<f32>,
    /// Channels per frame.
    pub channels: usize,
}

impl FrameSource for MemSource {
    fn channels(&self) -> usize {
        self.channels
    }
    fn frames(&self) -> u64 {
        (self.samples.len() / self.channels.max(1)) as u64
    }
    fn read_bytes(&mut self, start: u64, len: u64) -> io::Result<Vec<u8>> {
        let n = self.frames();
        let start = start.min(n);
        let end = (start + len).min(n);
        let ch = self.channels;
        Ok(self.samples[start as usize * ch..end as usize * ch]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect())
    }
}

/// 64-bit hash of bytes (FNV-1a over 8-byte words, the tail byte-wise). Detects
/// any difference a disk or a stick could introduce; not cryptographic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hash64(u64);

impl Default for Hash64 {
    fn default() -> Self {
        Hash64(0xcbf2_9ce4_8422_2325)
    }
}

impl Hash64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    /// Add bytes.
    pub fn update(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        let mut h = self.0;
        for w in words {
            h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(Self::PRIME);
        }
        for &b in tail {
            h = (h ^ u64::from(b)).wrapping_mul(Self::PRIME);
        }
        self.0 = h;
    }
    /// The value.
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Frames per copy step (about 1.4 s at 48 kHz stereo, 512 KiB).
const CHUNK: u64 = 1 << 16;

// ---------------------------------------------------------------------------
// Silence
// ---------------------------------------------------------------------------

/// Runs of quiet frames (every channel below `threshold`) at least `min_len` long,
/// as absolute `[start, end)` frame ranges; `base` is the first frame's index.
pub fn quiet_runs(samples: &[f32], channels: usize, base: u64, threshold: f32, min_len: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut run_start: Option<u64> = None;
    let frames = samples.len() / channels.max(1);
    for i in 0..=frames {
        let quiet = i < frames && samples[i * channels..(i + 1) * channels].iter().all(|x| x.abs() < threshold);
        match (quiet, run_start) {
            (true, None) => run_start = Some(i as u64),
            (false, Some(s)) => {
                if i as u64 - s >= min_len.max(1) {
                    out.push((base + s, base + i as u64));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    out
}

/// Where one boundary was cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// The first frame of the part after the cut.
    pub frame: u64,
    /// How sure.
    pub confidence: Confidence,
    /// The quiet run the cut is in.
    pub gap: Option<(u64, u64)>,
    /// Where the player's event was noticed (for the report).
    pub event: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    /// A track change: cut in the middle of the gap.
    Track,
    /// Playback ended: keep at most `end_keep_s` of the silence.
    End,
}

/// Grow a run that touches the edge of what was read, so its middle is its real
/// middle (at most 60 s each way, never past `lo` / `hi`).
fn extend_run(
    src: &mut dyn FrameSource,
    mut run: (u64, u64),
    read: (u64, u64),
    lo: u64,
    hi: u64,
    thr: f32,
) -> io::Result<(u64, u64)> {
    let ch = src.channels();
    let step = 4800u64;
    let limit = 60 * 48_000u64;
    if run.0 == read.0 {
        let floor = run.0.saturating_sub(limit).max(lo);
        'back: while run.0 > floor {
            let a = run.0.saturating_sub(step).max(floor);
            let s = src.read(a, run.0 - a)?;
            for i in (0..(run.0 - a) as usize).rev() {
                if !s[i * ch..(i + 1) * ch].iter().all(|x| x.abs() < thr) {
                    run.0 = a + i as u64 + 1;
                    break 'back;
                }
            }
            run.0 = a;
        }
    }
    if run.1 == read.1 {
        let ceil = (run.1 + limit).min(hi);
        'fwd: while run.1 < ceil {
            let b = (run.1 + step).min(ceil);
            let s = src.read(run.1, b - run.1)?;
            for i in 0..(b - run.1) as usize {
                if !s[i * ch..(i + 1) * ch].iter().all(|x| x.abs() < thr) {
                    run.1 += i as u64;
                    break 'fwd;
                }
            }
            run.1 = b;
        }
    }
    Ok(run)
}

#[allow(clippy::too_many_arguments)]
/// Cut one boundary near `center` (the event, or the player's better estimate),
/// strictly inside `(lo, hi)`. Only quiet runs that reach into `(win_lo, win_hi)`
/// count: the half-way points to the neighbouring boundaries' centres, so two track
/// changes less than 2 x 2.5 s apart never both claim the same gap (verification
/// finding, 2026-11-18: a skip 1.8 s after a gap put the skipped song into the next
/// song's file, its start marked "certain").
fn cut_near(
    src: &mut dyn FrameSource,
    rate: u32,
    center: u64,
    event: u64,
    edge: Edge,
    (lo, hi): (u64, u64),
    (win_lo, win_hi): (u64, u64),
    st: &SplitSettings,
) -> io::Result<Cut> {
    let sec = |s: f64| (s * f64::from(rate)).round() as u64;
    let thr = 10f64.powf(st.silence_dbfs / 20.0) as f32;
    let search = sec(st.search_s);
    let lo_c = lo + 1;
    let hi_c = hi.saturating_sub(1).max(lo_c);
    let center = center.clamp(lo_c, hi_c);
    // Read the search window plus as much again on each side, to see each run whole.
    let a = center.saturating_sub(2 * search).max(lo);
    let b = (center + 2 * search).min(hi);
    let samples = src.read(a, b - a)?;
    let runs = quiet_runs(&samples, src.channels(), a, thr, sec(st.min_gap_s).max(1));
    let (s_lo, s_hi) = (center.saturating_sub(search).max(win_lo), (center + search).min(win_hi));
    let mut best: Option<((u64, u64), u64)> = None;
    for r in runs.into_iter().filter(|r| r.1 > s_lo && r.0 < s_hi) {
        let dist = if r.0 <= center && center < r.1 {
            0
        } else if center < r.0 {
            r.0 - center
        } else {
            center + 1 - r.1
        };
        let better = match best {
            None => true,
            Some((br, bd)) => dist < bd || (dist == bd && r.1 - r.0 > br.1 - br.0),
        };
        if better {
            best = Some((r, dist));
        }
    }
    let Some((run, _)) = best else {
        return Ok(Cut {
            frame: center,
            confidence: Confidence::Uncertain,
            gap: None,
            event,
        });
    };
    let run = extend_run(src, run, (a, b), lo, hi, thr)?;
    let len = run.1 - run.0;
    let frame = match edge {
        Edge::Track => run.0 + len / 2,
        Edge::End => run.0 + (len / 2).min(sec(st.end_keep_s)),
    }
    .clamp(lo_c, hi_c);
    Ok(Cut {
        frame,
        confidence: Confidence::Certain,
        gap: Some(run),
        event,
    })
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// Who a part belongs to.
#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    /// The song of this track marker (index into `markers`); `None` when the
    /// player never reported a track.
    Song(Option<usize>),
    /// Not a song, with the reason and the track marker it came from, if any.
    Excluded(String, Option<usize>),
}

/// One part of the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPart {
    /// First frame.
    pub start: u64,
    /// One past the last frame.
    pub end: u64,
    /// Song or excluded.
    pub owner: Owner,
    /// The cut at the start (`None` = the start of the recording).
    pub start_cut: Option<Cut>,
    /// The cut at the end (`None` = the end of the recording).
    pub end_cut: Option<Cut>,
}

#[derive(Clone, Debug)]
struct Boundary {
    center: u64,
    event: u64,
    edge: Edge,
    owner_after: Owner,
}

/// Where the parts go. Pure apart from reading `src`.
pub fn plan(session: &SessionFile, src: &mut dyn FrameSource, st: &SplitSettings) -> io::Result<Vec<PlannedPart>> {
    let total = src.frames();
    let rate = session.sample_rate.max(1);
    let sec = |s: f64| (s * f64::from(rate)).round() as u64;
    let m = &session.markers;
    let clampf = |f: u64| f.min(total);

    // Playback ended: the first pause / stop / close after the last "playing", if
    // playback never resumed.
    let last_playing = m.iter().rposition(|x| x.kind() == MarkerKind::Playing);
    let ended = last_playing.and_then(|p| {
        m.iter().enumerate().skip(p + 1).find(|(_, x)| {
            matches!(x.kind(), MarkerKind::Paused | MarkerKind::Stopped | MarkerKind::Gone)
        })
    });
    let ended_frame = ended.map(|(_, x)| clampf(x.frame));

    // Track boundaries (the initial marker owns the start of the recording).
    let mut first_owner = Owner::Excluded("before the first song".into(), None);
    let mut bounds: Vec<Boundary> = Vec::new();
    let mut songs_seen = 0u32;
    for (i, x) in m.iter().enumerate() {
        if x.kind() != MarkerKind::Track {
            continue;
        }
        let center = clampf(x.center());
        if ended_frame.is_some_and(|e| center >= e) {
            break; // the player moved on after playback ended: part of the tail
        }
        let is_ad = looks_like_ad(&x.title, &x.artist, &x.album, x.duration_s);
        if !is_ad {
            songs_seen += 1;
        }
        let over_limit = !is_ad
            && session.stop_reason.as_deref() == Some("max_songs")
            && session.options.max_songs.is_some_and(|n| songs_seen > n);
        if over_limit {
            bounds.push(Boundary {
                center,
                event: clampf(x.frame),
                edge: Edge::Track,
                owner_after: Owner::Excluded("after the last song".into(), Some(i)),
            });
            break;
        }
        let at_start = x.initial || center < sec(st.merge_s);
        if bounds.is_empty() && at_start {
            first_owner = Owner::Song(Some(i));
            continue;
        }
        if let Some(prev) = bounds.last_mut()
            && center < prev.center + sec(st.merge_s)
        {
            prev.owner_after = Owner::Song(Some(i)); // one change in two steps
            continue;
        }
        bounds.push(Boundary {
            center,
            event: clampf(x.frame),
            edge: Edge::Track,
            owner_after: Owner::Song(Some(i)),
        });
    }
    if !bounds.iter().any(|b| matches!(b.owner_after, Owner::Excluded(..)))
        && let (Some(e), Some((_, ex))) = (ended_frame, ended)
        && e < total
    {
        bounds.push(Boundary {
            center: clampf(ex.center()),
            event: e,
            edge: Edge::End,
            owner_after: Owner::Excluded("after playback stopped".into(), None),
        });
    }
    if m.iter().all(|x| x.kind() != MarkerKind::Track) {
        first_owner = Owner::Song(None);
    }

    // Cuts, in order, each strictly between its neighbours.
    let mut cuts: Vec<(Cut, Owner)> = Vec::new();
    for (k, b) in bounds.iter().enumerate() {
        let lo = cuts.last().map_or(0, |(c, _)| c.frame);
        let hi = bounds.get(k + 1).map_or(total, |n| n.center.min(total));
        if hi <= lo + 1 {
            // No room (events at the very end): the previous part continues.
            if let Some(last) = cuts.last_mut() {
                last.1 = b.owner_after.clone();
            } else {
                first_owner = b.owner_after.clone();
            }
            continue;
        }
        // The part of the timeline this boundary owns: half-way to its neighbours.
        let win_lo = k.checked_sub(1).map_or(0, |p| bounds[p].center.midpoint(b.center));
        let win_hi = bounds.get(k + 1).map_or(u64::MAX, |n| b.center.midpoint(n.center));
        let c = cut_near(src, rate, b.center, b.event, b.edge, (lo, hi), (win_lo, win_hi), st)?;
        cuts.push((c, b.owner_after.clone()));
    }

    let mut parts = Vec::with_capacity(cuts.len() + 1);
    let mut start = 0u64;
    let mut start_cut: Option<Cut> = None;
    let mut owner = first_owner;
    for (c, next_owner) in cuts {
        parts.push(PlannedPart {
            start,
            end: c.frame,
            owner,
            start_cut,
            end_cut: Some(c.clone()),
        });
        start = c.frame;
        start_cut = Some(c);
        owner = next_owner;
    }
    parts.push(PlannedPart {
        start,
        end: total,
        owner,
        start_cut,
        end_cut: None,
    });
    parts.retain(|p| p.end > p.start);

    // Advertisements and quick skips are not songs.
    for p in &mut parts {
        if let Owner::Song(Some(i)) = p.owner {
            let x = &m[i];
            if looks_like_ad(&x.title, &x.artist, &x.album, x.duration_s) {
                p.owner = Owner::Excluded("advertisement".into(), Some(i));
            } else if p.end - p.start < sec(st.min_song_s) {
                p.owner = Owner::Excluded("skipped within a few seconds".into(), Some(i));
            }
        }
    }
    Ok(parts)
}

/// Seconds the player was paused or stopped inside `[start, end)` and then played on
/// (a pause that never ends is the end of playback, not a pause in the song).
fn paused_inside(markers: &[Marker], start: u64, end: u64, rate: u32) -> f64 {
    let mut total = 0u64;
    let mut since: Option<u64> = None;
    for x in markers {
        match x.kind() {
            MarkerKind::Paused | MarkerKind::Stopped | MarkerKind::Gone if since.is_none() => since = Some(x.frame),
            MarkerKind::Playing => {
                if let Some(s) = since.take() {
                    let (a, b) = (s.max(start), x.frame.min(end));
                    total += b.saturating_sub(a);
                }
            }
            _ => {}
        }
    }
    total as f64 / f64::from(rate.max(1))
}

/// Round to `places` decimals, so the JSON files hold short numbers that read back
/// exactly.
fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

fn boundary_info(c: Option<&Cut>, rate: u32) -> BoundaryInfo {
    let ms = |f: f64| round(f / f64::from(rate.max(1)) * 1000.0, 2);
    match c {
        None => BoundaryInfo {
            confidence: Confidence::Edge,
            gap_ms: None,
            moved_ms: None,
        },
        Some(c) => BoundaryInfo {
            confidence: c.confidence,
            gap_ms: c.gap.map(|g| ms((g.1 - g.0) as f64)),
            moved_ms: Some(ms(c.frame as f64 - c.event as f64)),
        },
    }
}

/// Complete / partial / unknown, with the reason for "partial".
fn song_status(session: &SessionFile, p: &PlannedPart, x: Option<&Marker>, paused_s: f64, total: u64) -> (SongStatus, Option<String>) {
    let rate = f64::from(session.sample_rate.max(1));
    let recorded = (p.end - p.start) as f64 / rate - paused_s;
    // The player was part-way into the first song when the recording started: playing
    // (position unknown or past 1 s), or paused past 1 s and then resumed.
    let started_mid = x.is_some_and(|x| {
        x.initial
            && match x.position_s {
                Some(pos) => pos > 1.0,
                None => x.initial_status.as_deref() == Some("playing"),
            }
    });
    if started_mid {
        return (
            SongStatus::Partial,
            Some("The player was already part-way into it when the recording started, so its beginning is missing.".into()),
        );
    }
    let cut_by_stop = p.end >= total
        && p.end_cut.is_none()
        && session.markers.last().is_some_and(|m| m.kind() == MarkerKind::Playing || m.kind() == MarkerKind::Track);
    match x.and_then(|x| x.duration_s).filter(|d| *d > 0.0) {
        Some(d) => {
            let tol = (0.03 * d).max(2.0);
            if recorded + tol < d {
                let why = if cut_by_stop {
                    "The recording stopped during the song."
                } else {
                    "It is shorter than the track: it was skipped or interrupted."
                };
                (SongStatus::Partial, Some(why.into()))
            } else {
                (SongStatus::Complete, None)
            }
        }
        None if cut_by_stop => (SongStatus::Partial, Some("The recording stopped during the song.".into())),
        None => (SongStatus::Unknown, None),
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Hash of the master's bytes for frames `start..end`.
fn hash_range(src: &mut dyn FrameSource, start: u64, end: u64) -> io::Result<Hash64> {
    let mut h = Hash64::default();
    let mut f = start;
    while f < end {
        let n = CHUNK.min(end - f);
        h.update(&src.read_bytes(f, n)?);
        f += n;
    }
    Ok(h)
}

/// True if `path` holds exactly `frames` frames whose bytes hash to `want`.
fn file_matches(path: &Path, frames: u64, want: Hash64) -> bool {
    let Ok(mut f) = WavSource::open(path) else { return false };
    if f.frames() != frames || f.info().frames != frames {
        return false;
    }
    hash_range(&mut f, 0, frames).is_ok_and(|h| h == want)
}

/// Copy master frames `start..end` into a new float WAV at `dest` (temporary file,
/// flushed, renamed). Returns the hash of what was copied.
fn write_part(
    src: &mut dyn FrameSource,
    spec: WavSpec,
    start: u64,
    end: u64,
    dest: &Path,
    progress: &mut dyn FnMut(u64),
) -> io::Result<Hash64> {
    let tmp = temp_path(dest);
    let result = (|| {
        let mut w = WavWriter::create(&tmp, spec)?;
        let mut h = Hash64::default();
        let mut f = start;
        while f < end {
            let n = CHUNK.min(end - f);
            let bytes = src.read_bytes(f, n)?;
            h.update(&bytes);
            let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
            w.write_interleaved(&samples)?;
            f += n;
            progress(n);
        }
        w.finalize()?;
        std::fs::rename(&tmp, dest)?;
        Ok(h)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn rel(dir: &Path, p: &Path) -> String {
    p.strip_prefix(dir)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Split the session in `dir` (see the module documentation). Safe to run again on
/// a session whose split was interrupted.
pub fn split(dir: &Path, st: &SplitSettings) -> io::Result<SplitOutcome> {
    split_with_progress(dir, st, &mut |_| {})
}

/// [`split`], reporting the fraction of the master copied so far (0..1).
pub fn split_with_progress(dir: &Path, st: &SplitSettings, progress: &mut dyn FnMut(f64)) -> io::Result<SplitOutcome> {
    let mut s = SessionFile::load(dir)?;
    let master = dir.join(&s.master);
    if s.state == SessionState::Split {
        return if master.is_file() { finish_cleanup(dir, &mut s) } else { Ok(outcome(dir, s, true)) };
    }
    let mut src = match WavSource::open(&master) {
        Ok(src) => src,
        Err(e) => {
            s.state = SessionState::Failed;
            s.message = Some(format!(
                "The session's recording ({}) could not be read, so it was not split: {e}",
                s.master
            ));
            s.save(dir)?;
            return Err(e);
        }
    };
    let info = src.info();
    s.sample_rate = info.sample_rate;
    s.channels = info.channels;
    let total = src.frames();
    s.stop_frame = Some(total);
    if total == 0 {
        s.state = SessionState::Failed;
        s.message = Some("Nothing was recorded (no sound arrived), so there is nothing to split.".into());
        s.save(dir)?;
        return Ok(outcome(dir, s, false));
    }
    s.state = SessionState::Splitting;
    s.parts.clear();
    s.message = None;
    s.save(dir)?;
    remove_temp_files(dir);

    let planned = plan(&s, &mut src, st)?;
    let spec = WavSpec::new(info.sample_rate, info.channels);
    let mut done_frames = 0u64;
    let mut number = 0u32;
    let mut songs = Vec::new();
    for (pi, p) in planned.iter().enumerate() {
        let marker = match p.owner {
            Owner::Song(Some(i)) => Some(&s.markers[i]),
            _ => None,
        };
        let (dest, kind, reason) = match &p.owner {
            Owner::Song(_) => {
                number += 1;
                let (artist, title) = marker.map_or(("", ""), |x| (x.artist.as_str(), x.title.as_str()));
                let stem = song_stem(number as usize, artist, title);
                (dir.join(format!("{stem}.wav")), PartKind::Song, None)
            }
            Owner::Excluded(why, from) => {
                // The track it came from (an advertisement, a quick skip) is named too,
                // so the excluded parts are easy to check by ear.
                let title = from.map(|i| s.markers[i].title.as_str()).unwrap_or("");
                let name = if title.trim().is_empty() {
                    format!("Part {:02} - {why}", pi + 1)
                } else {
                    format!("Part {:02} - {why} - {title}", pi + 1)
                };
                std::fs::create_dir_all(dir.join(EXCLUDED_DIR))?;
                (
                    dir.join(EXCLUDED_DIR).join(format!("{}.wav", safe_stem(&name))),
                    PartKind::Excluded,
                    Some(why.clone()),
                )
            }
        };
        let want = hash_range(&mut src, p.start, p.end)?;
        let frames = p.end - p.start;
        if !file_matches(&dest, frames, want) {
            let base = done_frames;
            let mut copied = 0u64;
            write_part(&mut src, spec, p.start, p.end, &dest, &mut |n| {
                copied += n;
                progress(((base + copied) as f64 / total as f64).min(1.0));
            })?;
        }
        done_frames += frames;
        progress((done_frames as f64 / total as f64).min(1.0));
        let verified = file_matches(&dest, frames, want);
        if kind == PartKind::Song {
            let stem = dest.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let mut cover = None;
            if let Some(c) = marker.and_then(|x| x.cover.as_deref()) {
                let from = dir.join(c);
                if let Some(ext) = from.extension().map(|e| e.to_string_lossy().into_owned())
                    && from.is_file()
                {
                    let name = format!("{stem}.{ext}");
                    let to = dir.join(&name);
                    if !to.is_file() || std::fs::read(&to).ok() != std::fs::read(&from).ok() {
                        copy_atomic(&from, &to)?;
                    }
                    cover = Some(name);
                }
            }
            let paused_s = paused_inside(&s.markers, p.start, p.end, info.sample_rate);
            let (status, partial_reason) = song_status(&s, p, marker, paused_s, total);
            let side = SongSidecar {
                format: SONG_FORMAT.into(),
                version: 1,
                title: marker.map(|x| x.title.clone()).unwrap_or_default(),
                artist: marker.map(|x| x.artist.clone()).unwrap_or_default(),
                album: marker.map(|x| x.album.clone()).unwrap_or_default(),
                cover,
                recorded_duration_s: round(frames as f64 / f64::from(info.sample_rate), 6),
                track_duration_s: marker.and_then(|x| x.duration_s),
                status,
                partial_reason,
                start: boundary_info(p.start_cut.as_ref(), info.sample_rate),
                end: boundary_info(p.end_cut.as_ref(), info.sample_rate),
                paused_s: round(paused_s, 3),
                source_player: s.player.clone(),
                session: dir.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
                number,
                master_start_frame: p.start,
                master_end_frame: p.end,
                sample_rate: info.sample_rate,
                channels: info.channels,
            };
            let text = serde_json::to_string_pretty(&side).map_err(io::Error::other)?;
            write_atomic(&dest.with_extension("json"), text.as_bytes())?;
            songs.push((dest.clone(), side));
        }
        s.parts.push(PartRecord {
            kind,
            file: rel(dir, &dest),
            start_frame: p.start,
            end_frame: p.end,
            reason,
            verified,
        });
        s.save(dir)?;
    }

    let contiguous = s.parts.windows(2).all(|w| w[0].end_frame == w[1].start_frame)
        && s.parts.first().is_some_and(|p| p.start_frame == 0)
        && s.parts.last().is_some_and(|p| p.end_frame == total);
    let sum: u64 = s.parts.iter().map(|p| p.end_frame - p.start_frame).sum();
    let all_ok = contiguous && sum == total && s.parts.iter().all(|p| p.verified);
    if all_ok {
        s.state = SessionState::Split;
        s.save(dir)?;
        let deleted = st.delete_master && delete_master(dir, &s);
        return Ok(SplitOutcome {
            session: s,
            songs,
            master_deleted: deleted,
        });
    }
    let bad: Vec<&str> = s.parts.iter().filter(|p| !p.verified).map(|p| p.file.as_str()).collect();
    s.state = SessionState::KeptMaster;
    s.message = Some(if bad.is_empty() {
        format!("The song files do not add up to the whole recording ({sum} of {total} samples), so the full recording ({}) was kept.", s.master)
    } else {
        format!(
            "{} file(s) did not read back exactly as recorded ({}), so the full recording ({}) was kept. Split it again from the Recorder screen.",
            bad.len(),
            bad.join(", "),
            s.master
        )
    });
    s.save(dir)?;
    Ok(SplitOutcome {
        session: s,
        songs,
        master_deleted: false,
    })
}

fn delete_master(dir: &Path, s: &SessionFile) -> bool {
    let ok = std::fs::remove_file(dir.join(&s.master)).is_ok();
    if ok {
        let _ = std::fs::remove_dir_all(dir.join(COVERS_DIR));
    }
    ok
}

/// A session marked "split" whose master is still there: check every part against
/// it again, then delete it (or mark the session "kept_master").
pub(crate) fn finish_cleanup(dir: &Path, s: &mut SessionFile) -> io::Result<SplitOutcome> {
    let mut src = WavSource::open(&dir.join(&s.master))?;
    let mut all = true;
    for p in &mut s.parts {
        let want = hash_range(&mut src, p.start_frame, p.end_frame)?;
        p.verified = file_matches(&dir.join(&p.file), p.end_frame - p.start_frame, want);
        all &= p.verified;
    }
    let sum: u64 = s.parts.iter().map(|p| p.end_frame - p.start_frame).sum();
    drop(src);
    if all && sum == src_frames(dir, s) {
        s.save(dir)?;
        let deleted = delete_master(dir, s);
        return Ok(outcome(dir, s.clone(), deleted));
    }
    s.state = SessionState::KeptMaster;
    s.message = Some(format!("Some song files no longer match the full recording, so it ({}) was kept.", s.master));
    s.save(dir)?;
    Ok(outcome(dir, s.clone(), false))
}

fn src_frames(dir: &Path, s: &SessionFile) -> u64 {
    WavSource::open(&dir.join(&s.master)).map_or(0, |w| w.frames())
}

/// The outcome of a finished session, from its files.
fn outcome(dir: &Path, session: SessionFile, master_deleted: bool) -> SplitOutcome {
    let songs = session
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Song)
        .filter_map(|p| {
            let path = dir.join(&p.file);
            SongSidecar::for_audio(&path).map(|sc| (path, sc))
        })
        .collect();
    SplitOutcome {
        session,
        songs,
        master_deleted,
    }
}

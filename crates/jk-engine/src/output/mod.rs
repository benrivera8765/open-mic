// SPDX-License-Identifier: GPL-3.0-or-later
//! Device output and the device-side "filler".
//!
//! One thread (or OS callback) per open stream. It only runs [`Filler::fill`]: copy
//! from the output ring, apply fades and the master volume, keep the playback clock.
//! No allocation, no locks, no logging, no syscalls besides the audio API's own buffer
//! calls. Backends:
//!
//! * Windows: WASAPI via the `wasapi` crate, shared mode at the device mix format, or
//!   exclusive mode (option) at the song rate when the device accepts it.
//! * macOS: Core Audio via `cpal` 0.16, at the device's nominal rate.
//! * Everywhere: a "null" device that plays nothing but consumes audio in real time
//!   (tests, headless self-check, computers without an output device).
//!
//! ## The output ring and segment markers
//!
//! The render worker writes device-rate interleaved frames into one SPSC ring and, into a
//! second small SPSC ring, *markers*: `Start{generation, pos}` before the first frame of a
//! segment (a load, a seek, a device switch) and `End{generation}` after the last frame of the
//! song. A marker's `at` is the total number of frames the worker had pushed when it was
//! written, so the filler knows exactly where segments begin and end in the frame ring.
//!
//! A seek is "in place": control stores the target position, bumps `target_gen`, and
//! hands the worker the new segment. The filler fades the old audio out over 10 ms,
//! discards the rest of the old generation from the ring (the frames before the new
//! `Start` marker), waits until the new segment has ~50 ms buffered and fades it in over
//! 10 ms. The device stream itself never stops.
//!
//! ## The playback clock
//!
//! The filler logs what it wrote as spans (song audio from position p, or silence while
//! paused at p). After each write it walks back by the latency the device reports (the
//! frames queued in the device plus the stream latency) and publishes the song time of
//! the sample leaving the DAC now, with the callback time. This is the clock the lyrics
//! follow (WORK_ORDER trap 7); nothing runs on a timer.

use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::shared::{now_ns, Shared};

#[cfg(target_os = "macos")]
pub mod coreaudio_out;
pub mod null_out;
#[cfg(windows)]
pub mod wasapi_out;

/// What the caller asks for.
#[derive(Clone, Debug)]
pub struct OutputRequest {
    /// Device id (`None` = default; `"null"` / `"null:<rate>"` = the null device).
    pub device_id: Option<String>,
    /// WASAPI exclusive mode (Windows only).
    pub exclusive: bool,
    /// Rate to try first in exclusive mode (normally the song rate, so no resampling).
    pub preferred_rate: Option<u32>,
    /// Device buffer length to ask for (shared mode), in milliseconds.
    pub buffer_ms: u32,
}

impl Default for OutputRequest {
    fn default() -> Self {
        OutputRequest { device_id: None, exclusive: false, preferred_rate: None, buffer_ms: 20 }
    }
}

/// The stream that was opened.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    /// Device id (as listed by `list_devices`).
    pub id: String,
    pub name: String,
    pub backend: &'static str,
    pub share_mode: &'static str,
    pub exclusive: bool,
    pub sample_rate: u32,
    pub channels: usize,
    /// e.g. "f32", "i32 (24 valid bits)", "i16".
    pub sample_format: String,
    /// Endpoint buffer size in frames (0 if unknown).
    pub buffer_frames: u32,
    /// The stream latency the OS reports (frames), excluding queued buffer contents.
    pub stream_latency_frames: u32,
}

/// Where a segment begins or the song ends, in the output ring's frame count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    /// Frames pushed into the output ring before this marker.
    pub at: u64,
    pub generation: u64,
    pub kind: MarkerKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerKind {
    /// First frame of a segment; `pos` is its song time in seconds.
    Start { pos: f64 },
    /// The song ends here.
    End,
}

/// Everything the device thread needs once the pipeline exists.
pub struct DeviceFeed {
    pub ring: rtrb::Consumer<f32>,
    pub markers: rtrb::Consumer<Marker>,
    pub shared: Arc<Shared>,
}

/// An open output stream (its own thread).
pub struct OpenOutput {
    pub info: StreamInfo,
    pub feed_tx: mpsc::Sender<DeviceFeed>,
    pub thread: JoinHandle<Result<(), String>>,
    /// Set to stop the stream and end the thread.
    pub stop: Arc<AtomicBool>,
}

/// Test capture for the null device: every buffer it "plays" is appended here.
pub type Capture = Arc<Mutex<Vec<f32>>>;

/// Open an output stream. Returns once the device format is known; the stream starts
/// (playing silence) as soon as a [`DeviceFeed`] is sent.
pub fn open(req: &OutputRequest, capture: Option<(Capture, bool)>) -> Result<OpenOutput, String> {
    if let Some(id) = req.device_id.as_deref()
        && let Some(rate) = null_out::parse_null_id(id)
    {
        return null_out::open(rate, capture);
    }
    #[cfg(windows)]
    {
        wasapi_out::open(req.clone())
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio_out::open(req.clone())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err("no audio backend for this platform".into())
    }
}

// -------------------------------------------------------------------------------------
// Span log: what was written to the device, for the playback clock.

#[derive(Clone, Copy, Debug, Default)]
struct Span {
    frames: u64,
    /// Song time at the span's first frame (song spans advance; silence spans hold).
    pos: f64,
    song: bool,
    generation: u64,
}

const SPANS: usize = 64;

/// Fixed-size history of the most recent spans (no allocation after construction).
struct SpanLog {
    spans: [Span; SPANS],
    /// Index of the oldest span.
    head: usize,
    len: usize,
    total: u64,
    rate: f64,
    /// Frames to keep (comfortably more than any device latency).
    keep: u64,
}

impl SpanLog {
    fn new(rate: u32) -> Self {
        SpanLog { spans: [Span::default(); SPANS], head: 0, len: 0, total: 0, rate: rate as f64, keep: 4 * rate as u64 }
    }

    fn idx(&self, k: usize) -> usize {
        (self.head + k) % SPANS
    }

    fn push(&mut self, frames: u64, pos: f64, song: bool, generation: u64) {
        if frames == 0 {
            return;
        }
        if self.len > 0 {
            let last = self.idx(self.len - 1);
            let l = &mut self.spans[last];
            let contiguous = if song { l.song && (l.pos + l.frames as f64 / self.rate - pos).abs() < 1e-9 } else { !l.song && l.pos == pos };
            if contiguous && l.generation == generation {
                l.frames += frames;
                self.total += frames;
                self.prune();
                return;
            }
        }
        if self.len == SPANS {
            self.total -= self.spans[self.head].frames;
            self.head = (self.head + 1) % SPANS;
            self.len -= 1;
        }
        let i = self.idx(self.len);
        self.spans[i] = Span { frames, pos, song, generation };
        self.len += 1;
        self.total += frames;
        self.prune();
    }

    fn prune(&mut self) {
        while self.len > 1 && self.total - self.spans[self.head].frames >= self.keep {
            self.total -= self.spans[self.head].frames;
            self.head = (self.head + 1) % SPANS;
            self.len -= 1;
        }
    }

    /// Song time and generation of the frame written `back` frames before the write head
    /// (0 = just after the newest frame).
    fn position_back(&self, back: u64) -> Option<(f64, u64)> {
        if self.len == 0 {
            return None;
        }
        let mut rem = back;
        for k in (0..self.len).rev() {
            let s = &self.spans[self.idx(k)];
            if rem <= s.frames {
                let j = s.frames - rem;
                let pos = if s.song { s.pos + j as f64 / self.rate } else { s.pos };
                return Some((pos, s.generation));
            }
            rem -= s.frames;
        }
        let s = &self.spans[self.head];
        Some((s.pos, s.generation))
    }
}

// -------------------------------------------------------------------------------------
// The filler.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Pause,
    Flush,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Discarding old-generation audio, waiting for the target segment's start marker.
    Flush,
    /// At the start of a segment, waiting for play and enough buffered audio.
    Primed,
    Running,
    /// Fading out (10 ms), then pause or flush.
    FadeOut(After),
    Paused,
    /// The song's end marker was reached.
    Ended,
}

/// Copies from the ring into the device buffer: fades, master volume, clock, counters.
/// Shared by all backends. Real-time safe.
pub(crate) struct Filler {
    ring: rtrb::Consumer<f32>,
    markers: rtrb::Consumer<Marker>,
    shared: Arc<Shared>,
    channels: usize,
    mode: Mode,
    /// Generation being played (0 = none).
    generation: u64,
    /// Ring frames consumed in total (comparable with `Marker::at`).
    consumed: u64,
    /// Song time at the current segment's start, and device frames consumed in it.
    seg_pos: f64,
    seg_k: u64,
    rate: f64,
    /// Fade envelope, counted in frames: gain = `fade_pos / fade_len` (0..=1). An
    /// integer counter, so a fade always ends exactly at 0 or 1.
    fade_pos: u32,
    fade_len: u32,
    /// Frames buffered before a new segment starts playing.
    start_threshold: usize,
    spans: SpanLog,
    gain: f32,
}

impl Filler {
    pub fn new(feed: DeviceFeed, channels: usize, rate: u32) -> Self {
        let fade = (rate as usize / 100).max(1); // 10 ms
        let gain = feed.shared.master.load();
        Filler {
            ring: feed.ring,
            markers: feed.markers,
            shared: feed.shared,
            channels,
            mode: Mode::Flush,
            generation: 0,
            consumed: 0,
            seg_pos: 0.0,
            seg_k: 0,
            rate: rate as f64,
            fade_pos: 0,
            fade_len: fade as u32,
            start_threshold: rate as usize / 20, // 50 ms
            spans: SpanLog::new(rate),
            gain,
        }
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// Current song time (end of what has been consumed in this segment).
    fn cur_pos(&self) -> f64 {
        self.seg_pos + self.seg_k as f64 / self.rate
    }

    /// Discard `frames` frames from the ring (old generation).
    fn discard(&mut self, frames: u64) {
        let n = frames as usize * self.channels;
        if n > 0 {
            if let Ok(c) = self.ring.read_chunk(n) {
                c.commit_all();
            }
            self.consumed += frames;
        }
    }

    /// Flush mode: drop old audio until the target segment's start marker; adopt it.
    fn try_adopt(&mut self, target: u64) -> bool {
        // Read the ring fill BEFORE looking at the markers: any frame of the target
        // generation that is visible here was pushed after its start marker, so the
        // marker is visible too (release/acquire through the rings).
        let mut avail = (self.ring.slots() / self.channels) as u64;
        loop {
            let m = match self.markers.peek() {
                Ok(m) => *m,
                Err(_) => {
                    // No marker: everything visible belongs to old generations.
                    self.discard(avail);
                    return false;
                }
            };
            // The frames before a marker were pushed before it, so once the marker is
            // visible they are too; re-read the fill if it was taken too early.
            if m.at > self.consumed + avail {
                avail = (self.ring.slots() / self.channels) as u64;
            }
            let skip = m.at.saturating_sub(self.consumed).min(avail);
            if m.generation < target || (m.generation == target && m.kind == MarkerKind::End) {
                self.discard(skip);
                avail -= skip;
                let _ = self.markers.pop();
                continue;
            }
            if m.generation > target {
                // Cannot happen (control bumps the target before the worker gets the
                // segment); be safe and wait.
                self.discard(skip);
                return false;
            }
            // Start of the target generation.
            self.discard(skip);
            let _ = self.markers.pop();
            if let MarkerKind::Start { pos } = m.kind {
                self.generation = target;
                self.seg_pos = pos;
                self.seg_k = 0;
                self.mode = Mode::Primed;
                self.fade_pos = 0;
            }
            return true;
        }
    }

    /// Enough audio of the current (just adopted) segment to start playing?
    fn primed_ready(&self) -> bool {
        if let Ok(m) = self.markers.peek()
            && m.generation == self.generation
        {
            return true; // the whole (short) segment is in the ring
        }
        self.ring.slots() / self.channels >= self.start_threshold
    }

    /// True when a fill of `frames` would play audio without an underrun, or play
    /// silence by design. The null device in "fast" mode waits on this instead of
    /// underrunning.
    pub fn can_fill(&self, frames: usize) -> bool {
        let new_target = self.shared.target_gen.load(Acquire) != self.generation;
        let want_play = self.shared.want_play.load(Acquire);
        match self.mode {
            Mode::Running | Mode::FadeOut(_) => {
                new_target || !want_play || self.ring.slots() / self.channels >= frames || self.markers.peek().is_ok()
            }
            Mode::Primed => new_target || (want_play && self.primed_ready()),
            Mode::Flush => self.markers.peek().is_ok(),
            Mode::Paused => new_target || want_play,
            Mode::Ended => new_target,
        }
    }

    /// Fill `out` (interleaved f32, whole frames). `latency_frames`: the frames queued in
    /// the device once this write is done (including `out`) plus the stream latency the
    /// device reports; the clock walks back that far from the end of `out`.
    pub fn fill(&mut self, out: &mut [f32], latency_frames: u32) {
        let t0 = now_ns();
        let ch = self.channels;
        let frames = out.len() / ch;
        let target = self.shared.target_gen.load(Acquire);
        let want_play = self.shared.want_play.load(Acquire);

        // Transitions decided once per callback.
        if target != self.generation {
            self.mode = match self.mode {
                Mode::Running | Mode::FadeOut(_) => Mode::FadeOut(After::Flush),
                _ => Mode::Flush,
            };
        }
        match self.mode {
            Mode::Running if !want_play => self.mode = Mode::FadeOut(After::Pause),
            Mode::Paused if want_play => self.mode = Mode::Running,
            _ => {}
        }

        let mut i = 0usize;
        while i < frames {
            match self.mode {
                Mode::Flush => {
                    if self.try_adopt(target) {
                        continue;
                    }
                    let pos = self.shared.target_pos.load();
                    out[i * ch..frames * ch].iter_mut().for_each(|v| *v = 0.0);
                    self.spans.push((frames - i) as u64, pos, false, target);
                    i = frames;
                }
                Mode::Primed => {
                    if want_play && self.primed_ready() {
                        self.mode = Mode::Running;
                        continue;
                    }
                    out[i * ch..frames * ch].iter_mut().for_each(|v| *v = 0.0);
                    self.spans.push((frames - i) as u64, self.seg_pos, false, self.generation);
                    i = frames;
                }
                Mode::Paused | Mode::Ended => {
                    out[i * ch..frames * ch].iter_mut().for_each(|v| *v = 0.0);
                    self.spans.push((frames - i) as u64, self.cur_pos(), false, self.generation);
                    i = frames;
                }
                Mode::Running | Mode::FadeOut(_) => {
                    let avail = (self.ring.slots() / ch) as u64;
                    let next_marker = self.markers.peek().ok().copied();
                    let limit = next_marker.map_or(avail, |m| m.at.saturating_sub(self.consumed).min(avail));
                    let mut can = limit.min((frames - i) as u64) as usize;
                    if let Mode::FadeOut(_) = self.mode {
                        can = can.min(self.fade_pos.max(1) as usize);
                    }
                    if can == 0 {
                        if let Some(m) = next_marker
                            && m.at <= self.consumed
                        {
                            match m.kind {
                                MarkerKind::End if m.generation == self.generation => {
                                    let _ = self.markers.pop();
                                    self.mode = Mode::Ended;
                                    self.shared.ended_gen.store(self.generation, Release);
                                    continue;
                                }
                                MarkerKind::Start { .. } if m.generation != self.generation => {
                                    // The next segment starts here: the current one is
                                    // over. Flush mode adopts it (the marker stays).
                                    self.fade_pos = 0;
                                    self.mode = Mode::Flush;
                                    continue;
                                }
                                _ => {
                                    // A stale marker of the current generation.
                                    let _ = self.markers.pop();
                                    continue;
                                }
                            }
                        }
                        if let Mode::FadeOut(then) = self.mode {
                            // Out of audio while fading out: the fade is over.
                            self.fade_pos = 0;
                            self.mode = if then == After::Pause { Mode::Paused } else { Mode::Flush };
                            continue;
                        }
                        // Underrun: the worker did not keep up.
                        out[i * ch..frames * ch].iter_mut().for_each(|v| *v = 0.0);
                        self.shared.underruns.fetch_add(1, Relaxed);
                        self.shared.underrun_frames.fetch_add((frames - i) as u64, Relaxed);
                        self.spans.push((frames - i) as u64, self.cur_pos(), false, self.generation);
                        self.fade_pos = 0; // fade back in when audio returns
                        i = frames;
                        continue;
                    }
                    let pos0 = self.cur_pos();
                    let n = can * ch;
                    if let Ok(chunk) = self.ring.read_chunk(n) {
                        let (a, b) = chunk.as_slices();
                        let dst = &mut out[i * ch..i * ch + n];
                        dst[..a.len()].copy_from_slice(a);
                        dst[a.len()..].copy_from_slice(b);
                        chunk.commit_all();
                    }
                    // Fade envelope, per frame (skipped once fully faded in). Fade-in frame
                    // k gets (k+1)/len; fade-out frame k gets (len-k-1)/len, ending at 0.
                    let fading_out = matches!(self.mode, Mode::FadeOut(_));
                    if fading_out || self.fade_pos < self.fade_len {
                        let dst = &mut out[i * ch..i * ch + n];
                        let len = self.fade_len as f32;
                        for f in dst.chunks_exact_mut(ch) {
                            self.fade_pos = if fading_out {
                                self.fade_pos.saturating_sub(1)
                            } else {
                                (self.fade_pos + 1).min(self.fade_len)
                            };
                            let g = self.fade_pos as f32 / len;
                            f.iter_mut().for_each(|v| *v *= g);
                        }
                    }
                    self.consumed += can as u64;
                    self.seg_k += can as u64;
                    self.spans.push(can as u64, pos0, true, self.generation);
                    i += can;
                    if let Mode::FadeOut(then) = self.mode
                        && self.fade_pos == 0
                    {
                        self.mode = if then == After::Pause { Mode::Paused } else { Mode::Flush };
                    }
                }
            }
        }

        // Master volume: linear ramp from the previous value to the target over this buffer.
        let target_gain = self.shared.master.load();
        if target_gain == self.gain {
            if target_gain != 1.0 {
                out.iter_mut().for_each(|v| *v *= target_gain);
            }
        } else {
            let step = (target_gain - self.gain) / frames.max(1) as f32;
            let mut g = self.gain;
            for f in out.chunks_exact_mut(ch) {
                g += step;
                f.iter_mut().for_each(|v| *v *= g);
            }
            self.gain = target_gain;
        }

        // Clock: the sample leaving the DAC now was written `latency_frames` ago.
        let s = &self.shared;
        if let Some((pos, generation)) = self.spans.position_back(latency_frames as u64) {
            s.dac_pos.store(pos);
            s.dac_gen.store(generation, Release);
        }
        s.latency_frames.store(latency_frames, Relaxed);
        let mask = (1u64 << crate::shared::FILLER_SEG_BITS) - 1;
        s.filler_seg.store((self.generation << crate::shared::FILLER_SEG_BITS) | (self.seg_k & mask), Release);
        s.filler_mode.store(
            match self.mode {
                Mode::Flush => 0,
                Mode::Primed => 1,
                Mode::Running => 2,
                Mode::FadeOut(_) => 3,
                Mode::Paused => 4,
                Mode::Ended => 5,
            },
            Relaxed,
        );
        s.callbacks.fetch_add(1, Relaxed);
        let t1 = now_ns();
        s.dac_ns.store(t1, Relaxed);
        s.max_callback_ns.fetch_max(t1 - t0, Relaxed);
    }
}

/// Raise the calling thread's priority for audio work (MMCSS "Pro Audio" on Windows,
/// QoS user-interactive on macOS). Returns a description of what happened.
pub fn promote_audio_thread() -> String {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let mut task = 0u32;
        // SAFETY: plain Win32 call with a static string and a valid out pointer.
        match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) } {
            Ok(_) => "MMCSS Pro Audio".to_string(),
            Err(e) => format!("MMCSS failed: {e}"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        coreaudio_out::promote_current_thread()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "default priority".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_log_walks_back() {
        let mut s = SpanLog::new(1000);
        s.push(500, 10.0, true, 1); // song 10.0 .. 10.5
        s.push(500, 10.5, true, 1); // merged: 10.0 .. 11.0
        s.push(200, 11.0, false, 1); // paused at 11.0
        assert_eq!(s.len, 2);
        assert_eq!(s.position_back(0), Some((11.0, 1)));
        assert_eq!(s.position_back(200), Some((11.0, 1)));
        assert_eq!(s.position_back(300), Some((10.9, 1)));
        assert_eq!(s.position_back(1200), Some((10.0, 1)));
        assert_eq!(s.position_back(5000), Some((10.0, 1)));
        s.push(100, 42.0, true, 2); // after a seek
        assert_eq!(s.position_back(50), Some((42.05, 2)));
        assert_eq!(s.position_back(150), Some((11.0, 1)));
    }
}

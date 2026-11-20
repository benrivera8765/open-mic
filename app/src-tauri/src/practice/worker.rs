// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice analysis thread "jk-practice" (docs/ARCHITECTURE.md 12.2 steps 1-8, 12.4).
//!
//! One per session, normal priority, never an audio thread. It owns the open input (the
//! microphone, or a generated test source through the very same `InputStream` path), the
//! engine's output tap reader and the analysis chain (`jk_pitch::practice::Chain`, the
//! code the offline harness runs too). Each turn (every few milliseconds) it:
//!
//! 1. takes the commands from the core (finish, stop, settings, suspend, reopen...);
//! 2. reads the playback clock (`Engine::snapshot`: song time at the DAC with its host
//!    time and segment) as an anchor for the song-time mapping, forwards the engine's key
//!    timeline (the key HEARD, D-239) to the scorer, and starts or ends the take;
//! 3. reads the output tap into the reference aligner (src/practice/align.rs);
//! 4. reads every waiting input block: meters (in the stream), the voice channel alone
//!    (never a sum), the lined-up reference, the chain;
//! 5. every 16 ms sends `practice://live`, every 50 ms `mic://levels`, and every 250 ms
//!    hands the session state to the core (which sends `practice://state` on a change);
//! 6. at the end of a take, grades it, saves the result in the song's slot (through the
//!    song-file lock, never during a guide swap) and hands it to the core.
//!
//! Microphone sound lives only in the input rings and this thread's buffers: it is never
//! written, sent or played. Nothing here feeds the engine's output.

use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use jk_engine::PlaybackState;
use jk_pitch::echo::{DelayStability, Presence, RemovalQuality};
use jk_pitch::practice::{Chain, ChainConfig, ClockAnchor, LatencyConfig, LatencySource, MusicGrading, PracticeLive};
use jk_pitch::score::{KeyTimeline, OctaveMode, ScoreConfig, Strictness};
use jk_recorder::input::{InputDeviceInfo, InputState, InputStream, list_input_devices};
use serde::Serialize;
use serde_json::Value;

use super::align::{RefAligner, clock_offset_ns};
use super::guide_view::SessionGuide;
use super::mic::{self, MicInput, SourceSpec};
use super::result;
use super::{LatencyView, MusicView, PracticeInputView, PracticeService, TakeView};
use crate::engine::AudioEngine;
use crate::events::{EventSink, emit};
use crate::guide::GuideService;
use crate::playback::Player;
use crate::settings::SettingsStore;

/// `practice://live`, `practice://result`.
pub mod names {
    pub const PRACTICE_STATE: &str = "practice://state";
    pub const PRACTICE_LIVE: &str = "practice://live";
    pub const PRACTICE_RESULT: &str = "practice://result";
}

/// How early the reference is handed (jk-pitch `ChainConfig::reference_lead_s`): a
/// return that arrives up to this much sooner than the stamps predict (a device that
/// over-declares its latency) is still found. Kept small, because an input block waits
/// until the output tap holds its reference (about this long minus the output latency).
const REFERENCE_LEAD_S: f64 = 0.02;
/// The longest an input block waits for its echo reference (then zeros are used).
const MAX_REFERENCE_WAIT_NS: i64 = 120_000_000;
/// Turn period of the loop when nothing arrives.
const TURN: Duration = Duration::from_millis(4);
/// Live batches at most this often (APP_API 12.5: every 16 ms).
const LIVE_EVERY: Duration = Duration::from_millis(12);
const LEVELS_EVERY: Duration = Duration::from_millis(50);
const STATUS_EVERY: Duration = Duration::from_millis(250);
/// After the song ended, input this long is still read before the take is graded (the
/// last notes reach the analysis through the input latency).
const END_GRACE: Duration = Duration::from_millis(400);
/// A lost device is looked for again this often.
const REOPEN_EVERY: Duration = Duration::from_secs(2);

/// What a finished take produced.
#[derive(Clone, Debug)]
pub struct TakeOutcome {
    /// The result screen's view (APP_API 12.6).
    pub view: Value,
    pub saved: bool,
}

/// Commands from the core.
pub enum Cmd {
    /// Close the input and end the session; a running take is graded and saved when
    /// enough was sung (and `keep_result`).
    Stop { keep_result: bool, reply: Sender<Option<TakeOutcome>> },
    /// "Finish": grade the take now (also a short one; it is shown, not saved).
    Finish { reply: Sender<Option<TakeOutcome>> },
    /// "Sing again": drop a running take and start afresh (the core then seeks and plays).
    Again { reply: Sender<()> },
    /// The song's octave and strictness changed (notes still to come use them).
    Settings { octave: Option<(OctaveMode, i32)>, strictness: Option<Strictness> },
    /// The excluded spans changed.
    Excluded(Vec<(f64, f64)>),
    /// The manual latency offset (ms) changed.
    Manual(f64),
    /// Close (true) or reopen (false) the microphone without leaving practice.
    Suspend(bool),
    /// Another input or channel was chosen.
    Reopen { spec: SourceSpec, channel: Option<usize> },
}

/// What the analysis thread starts with.
pub struct WorkerInit {
    pub svc: Weak<PracticeService>,
    pub engine: Arc<dyn AudioEngine>,
    pub sink: Arc<dyn EventSink>,
    pub player: Arc<Player>,
    pub guide_service: Arc<GuideService>,
    pub settings: Arc<SettingsStore>,
    pub song_id: String,
    pub path: PathBuf,
    pub guide: Arc<SessionGuide>,
    /// The open input (moved into the worker when it starts).
    pub stream: Option<InputStream>,
    pub device: MicInput,
    pub spec: SourceSpec,
    pub channel_choice: Option<usize>,
    pub octave: (OctaveMode, i32),
    pub strictness: Strictness,
    pub excluded: Vec<(f64, f64)>,
    pub auto_latency: bool,
    pub stored_latency_s: Option<f64>,
    pub manual_ms: f64,
    /// `practice.measured_latency_ms` key for this computer, input and output.
    pub measured_key: String,
    pub app_version: String,
    pub machine: String,
    pub rx: Receiver<Cmd>,
}

/// The running take.
#[derive(Default)]
struct Take {
    running: bool,
    started_s: f64,
    ended_at: Option<Instant>,
    input_lost: bool,
    clipped0: u64,
    dropped0: u64,
    key_changes: u32,
    last_heard: Option<i32>,
}

/// One `practice://live` batch.
#[derive(Serialize)]
struct LiveEvent<'a> {
    seq: u64,
    segment: u64,
    pitch: &'a [(f64, Option<f64>, f64)],
    hits: &'a [(usize, f64, f64, i8)],
    done: &'a [(usize, Option<f64>, i32, Option<f64>)],
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub struct Worker {
    w: WorkerInit,
    stream: Option<InputStream>,
    device: MicInput,
    channels: usize,
    rate: u32,
    declared_s: f64,
    channel: usize,
    suggested_streak: (Option<usize>, u32),
    chain: Chain,
    aligner: RefAligner,
    tap: Option<jk_engine::TapReader>,
    offset_ns: i64,
    take: Take,
    live: PracticeLive,
    live_seq: u64,
    pitch_out: Vec<(f64, Option<f64>, f64)>,
    done_out: Vec<(usize, Option<f64>, i32, Option<f64>)>,
    hits_out: Vec<(usize, f64, f64, i8)>,
    block: Vec<f32>,
    mono: Vec<f32>,
    reference: Vec<f32>,
    tap_buf: Vec<f32>,
    last_anchor_ns: u64,
    key_sig: Option<(u64, i32, Option<(i32, Option<i64>)>)>,
    key_forwarded_at: Instant,
    suspended: bool,
    lost: Option<String>,
    lost_state: &'static str,
    next_reopen: Instant,
    /// Silence fed to the chain while the input is closed or lost: host time of the next
    /// synthetic frame.
    silent_ns: Option<i64>,
    /// An input block (in `block`) waiting for its echo reference: (host ns, discontinuity).
    held: Option<(i64, bool)>,
    last_measured_saved: Option<(f64, Instant)>,
    stop: bool,
}

impl Worker {
    pub fn new(mut w: WorkerInit) -> Worker {
        let stream = w.stream.take().expect("the session opens the input before starting the worker");
        let f = stream.format().clone();
        let rate = f.sample_rate.max(1);
        let channels = usize::from(f.channels.max(1));
        let declared_s = f.declared_latency_ns.unwrap_or(0) as f64 / 1e9;
        let channel = w.channel_choice.unwrap_or(0).min(channels - 1);
        let tap = w.engine.output_tap();
        let offset_ns = clock_offset_ns();
        let keys = timeline_now(&*w.engine);
        let chain = new_chain(&w, rate, declared_s, tap.is_some(), keys);
        let max_block = f.max_block_frames.max(4096);
        Worker {
            device: w.device.clone(),
            stream: Some(stream),
            channels,
            rate,
            declared_s,
            channel,
            suggested_streak: (None, 0),
            chain,
            aligner: RefAligner::new(rate, offset_ns, REFERENCE_LEAD_S),
            tap,
            offset_ns,
            take: Take::default(),
            live: PracticeLive::default(),
            live_seq: 0,
            pitch_out: Vec::with_capacity(256),
            done_out: Vec::with_capacity(64),
            hits_out: Vec::with_capacity(256),
            block: Vec::with_capacity(max_block * channels),
            mono: vec![0.0; max_block],
            reference: vec![0.0; 2 * max_block],
            tap_buf: vec![0.0; 2 * 4096],
            last_anchor_ns: 0,
            key_sig: None,
            key_forwarded_at: Instant::now(),
            suspended: false,
            lost: None,
            lost_state: "unplugged",
            next_reopen: Instant::now() + REOPEN_EVERY,
            silent_ns: None,
            held: None,
            last_measured_saved: None,
            stop: false,
            w,
        }
    }

    /// The loop. Returns when told to stop or when the core is gone.
    pub fn run(mut self) {
        let mut last_live = Instant::now();
        let mut last_levels = Instant::now();
        let mut last_status = Instant::now() - STATUS_EVERY;
        self.push_status();
        while !self.stop {
            // Paced by a plain sleep: std's sleep uses a high-resolution timer on Windows,
            // while a channel's timed wait rounds up to the 15.6 ms system tick, which would
            // halve the rate of the live batches (APP_API 12.5: every 16 ms).
            std::thread::sleep(TURN);
            loop {
                match self.w.rx.try_recv() {
                    Ok(cmd) => self.command(cmd),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.stop = true;
                        break;
                    }
                }
                if self.stop {
                    break;
                }
            }
            if self.stop {
                break;
            }
            if self.w.svc.upgrade().is_none() {
                break;
            }
            self.turn();
            if last_live.elapsed() >= LIVE_EVERY {
                last_live = Instant::now();
                self.send_live();
            }
            if last_levels.elapsed() >= LEVELS_EVERY {
                last_levels = Instant::now();
                self.send_levels();
            }
            if last_status.elapsed() >= STATUS_EVERY {
                last_status = Instant::now();
                self.push_status();
                self.save_measured_latency();
                if debug_log() {
                    let st = self.chain.stats();
                    crate::applog::info(&format!(
                        "practice debug: echo {:?}; latency {:?}; aligner realigned {} missing {}; stats shown {} dropped music {} unknown {} unmapped {}",
                        self.chain.echo_report(),
                        self.chain.latency(),
                        self.aligner.realignments(),
                        self.aligner.missing,
                        st.shown,
                        st.dropped_music,
                        st.dropped_unknown,
                        st.unmapped
                    ));
                }
            }
        }
        if let Some(mut s) = self.stream.take() {
            s.stop();
        }
        if self.take.running {
            self.w.guide_service.set_take_active(None);
        }
    }

    // ------------------------------------------------------------------ one turn

    fn turn(&mut self) {
        // 2. The playback clock, the key heard, the take.
        let snap = self.w.engine.snapshot();
        if snap.position_ns != 0 && snap.position_ns != self.last_anchor_ns {
            self.last_anchor_ns = snap.position_ns;
            // APP_API 12.7: the manual offset applies only while nothing was measured
            // (the chain's own choice says "none"); the chain itself is given 0 for it.
            let manual = if self.chain.latency().source == LatencySource::None { self.w.manual_ms / 1000.0 } else { 0.0 };
            self.chain.playback_anchor(ClockAnchor {
                host_ns: snap.position_ns as i64 + self.offset_ns,
                song_s: snap.position_s - manual,
                segment: snap.key.segment,
                playing: snap.state == PlaybackState::Playing,
            });
        }
        let sig = (snap.key.segment, snap.key.heard, snap.key.pending.map(|p| (p.semitones, p.at_s.map(|t| (t * 1000.0) as i64))));
        if self.key_sig != Some(sig) || self.key_forwarded_at.elapsed() >= Duration::from_millis(500) {
            self.key_sig = Some(sig);
            self.key_forwarded_at = Instant::now();
            forward_keys(&mut self.chain, &self.w.engine.key_timeline());
        }
        let playing = snap.state == PlaybackState::Playing;
        if playing && !self.take.running {
            self.start_take(snap.position_s);
        }
        if self.take.running {
            if self.take.last_heard.is_some_and(|k| k != snap.key.heard) {
                self.take.key_changes += 1;
            }
            self.take.last_heard = Some(snap.key.heard);
            if snap.state == PlaybackState::Ended {
                let t = *self.take.ended_at.get_or_insert_with(Instant::now);
                if t.elapsed() >= END_GRACE {
                    self.read_input();
                    if let Some(o) = self.finish_take(true, true, false) {
                        self.hand_over(o);
                    }
                    return;
                }
            } else {
                self.take.ended_at = None;
            }
        }
        // 3. The output tap.
        if let Some(tap) = self.tap.as_mut() {
            while let Some(p) = tap.read(&mut self.tap_buf) {
                self.aligner.push_tap(&self.tap_buf, p.frames, p.rate, p.dac_ns, p.lost_before);
            }
        }
        // 4. The input.
        self.read_input();
        self.watch_input();
    }

    fn start_take(&mut self, at_s: f64) {
        self.take = Take { running: true, started_s: at_s, ..Take::default() };
        if let Some(s) = &self.stream {
            self.take.clipped0 = s.levels().channels.get(self.channel).map_or(0, |c| c.clipped_samples);
            self.take.dropped0 = s.health().dropped_blocks;
        }
        if self.lost.is_some() {
            self.take.input_lost = true;
        }
        // A guide swap of this song waits until the take ends (APP_API 12.3).
        self.w.guide_service.set_take_active(Some(self.w.song_id.clone()));
        if let Some(svc) = self.w.svc.upgrade() {
            svc.take_started(&self.w.song_id);
        }
        self.push_status();
    }

    /// Every waiting input block through the chain (or digital silence while the input is
    /// closed or lost, so the take keeps its time: nothing sung counts as not sung).
    fn read_input(&mut self) {
        let has_input = self.stream.is_some() && !(self.suspended && !self.w.spec.is_test());
        if has_input {
            self.silent_ns = None;
            loop {
                // A block held back for its reference first, else the next one waiting.
                let stamp = match self.held.take() {
                    Some(h) => h,
                    None => {
                        let Some(stream) = self.stream.as_mut() else { return };
                        match stream.read(&mut self.block) {
                            Some(st) => (st.host_ns, st.discontinuity()),
                            None => break,
                        }
                    }
                };
                if self.suspended {
                    // A test source keeps running while "suspended"; its sound is dropped.
                    continue;
                }
                let (host_ns, discontinuity) = stamp;
                let frames = self.block.len() / self.channels;
                let capture = host_ns - (self.declared_s * 1e9).round() as i64;
                if self.chain.config().reference_channels > 0 {
                    // The echo reference is handed a little early (align.rs): wait until the
                    // output tap holds it, at most MAX_REFERENCE_WAIT after the capture.
                    let end = capture + (frames as f64 * 1e9 / f64::from(self.rate)) as i64;
                    let now = jk_recorder::input::clock::host_time_ns();
                    if !self.aligner.covers(end, now) && now - end < MAX_REFERENCE_WAIT_NS {
                        self.held = Some(stamp);
                        break;
                    }
                }
                if discontinuity {
                    self.aligner.reset_lineup();
                }
                if self.mono.len() < frames {
                    self.mono.resize(frames, 0.0);
                    self.reference.resize(2 * frames, 0.0);
                }
                // The voice channel alone, never a sum (trap 14).
                for (i, f) in self.block.chunks_exact(self.channels).enumerate() {
                    self.mono[i] = f[self.channel];
                }
                let refs: &[f32] = if self.chain.config().reference_channels > 0 {
                    self.aligner.reference(capture, frames, &mut self.reference);
                    &self.reference[..2 * frames]
                } else {
                    &[]
                };
                self.chain.process(host_ns, &self.mono[..frames], refs);
            }
        } else {
            // Synthetic silence at the input rate, stamped with the host clock.
            let now = jk_recorder::input::clock::host_time_ns();
            let start = *self.silent_ns.get_or_insert(now);
            let frames = ((now - start) as f64 * f64::from(self.rate) / 1e9) as usize;
            if frames >= (self.rate / 100) as usize {
                let frames = frames.min(self.mono.len());
                self.mono[..frames].fill(0.0);
                let refs: &[f32] = if self.chain.config().reference_channels > 0 {
                    let capture = start - (self.declared_s * 1e9).round() as i64;
                    self.aligner.reference(capture, frames, &mut self.reference);
                    &self.reference[..2 * frames]
                } else {
                    &[]
                };
                self.chain.process(start, &self.mono[..frames], refs);
                self.silent_ns = Some(start + (frames as f64 * 1e9 / f64::from(self.rate)).round() as i64);
            }
        }
    }

    /// Input failures (unplugged, stalled): watch only with the reason, the take flagged,
    /// the device looked for again; nothing crashes.
    fn watch_input(&mut self) {
        if let Some(stream) = &self.stream
            && let InputState::Failed { kind, message } = stream.state()
        {
            let e = jk_recorder::input::InputError::new(kind, message);
            crate::applog::warn(&format!("practice: input lost: {}", e.message));
            self.lost = Some(mic::plain_input_error(&e));
            self.lost_state = match kind {
                jk_recorder::input::FailureKind::Stalled => "stalled",
                jk_recorder::input::FailureKind::Unplugged => "unplugged",
                _ => "failed",
            };
            if self.take.running {
                self.take.input_lost = true;
            }
            if let Some(mut s) = self.stream.take() {
                s.stop();
            }
            self.next_reopen = Instant::now() + REOPEN_EVERY;
            self.push_status();
        }
        // A lost microphone is opened again when it comes back (test sources are not).
        if self.lost.is_some() && !self.suspended && Instant::now() >= self.next_reopen {
            self.next_reopen = Instant::now() + REOPEN_EVERY;
            if let SourceSpec::Device { id } = &self.w.spec {
                let present = list_input_devices().map(|l| l.iter().any(|d| &d.id == id)).unwrap_or(false);
                if present {
                    let spec = self.w.spec.clone();
                    self.reopen(spec, self.w.channel_choice);
                }
            }
        }
    }

    // ------------------------------------------------------------------ commands

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Stop { keep_result, reply } => {
                self.read_input();
                let out = if self.take.running { self.finish_take(false, keep_result, true) } else { None };
                let _ = reply.send(out);
                self.stop = true;
            }
            Cmd::Finish { reply } => {
                self.read_input();
                let out = if self.take.running { self.finish_take(false, true, false) } else { None };
                if let Some(o) = &out {
                    self.hand_over(o.clone());
                }
                let _ = reply.send(out);
            }
            Cmd::Again { reply } => {
                if self.take.running {
                    self.w.guide_service.set_take_active(None);
                }
                self.take = Take::default();
                self.renew_chain();
                let _ = reply.send(());
            }
            Cmd::Settings { octave, strictness } => {
                if let Some((m, s)) = octave {
                    self.w.octave = (m, s);
                    self.chain.scorer_mut().set_octave(m, s);
                }
                if let Some(s) = strictness {
                    self.w.strictness = s;
                    self.chain.scorer_mut().set_strictness(s);
                }
                self.push_status();
            }
            Cmd::Excluded(spans) => {
                self.w.excluded = spans.clone();
                self.chain.scorer_mut().set_excluded(spans);
            }
            Cmd::Manual(ms) => {
                self.w.manual_ms = ms;
                self.push_status();
            }
            Cmd::Suspend(on) => {
                if on == self.suspended {
                    return;
                }
                self.suspended = on;
                if !self.w.spec.is_test() {
                    if on {
                        // The microphone closes while the window is hidden (WO2 4.4).
                        if let Some(mut s) = self.stream.take() {
                            s.stop();
                        }
                    } else {
                        let spec = self.w.spec.clone();
                        self.reopen(spec, self.w.channel_choice);
                    }
                }
                self.push_status();
            }
            Cmd::Reopen { spec, channel } => {
                self.w.channel_choice = channel;
                self.reopen(spec, channel);
            }
        }
    }

    /// Opens `spec` in place of the current input. A different rate or channel count
    /// needs a new chain (the running take is dropped: the input changed under it).
    fn reopen(&mut self, spec: SourceSpec, channel: Option<usize>) {
        if let Some(mut s) = self.stream.take() {
            s.stop();
        }
        let list: Vec<InputDeviceInfo> = list_input_devices().unwrap_or_default();
        let out_name = self.w.engine.snapshot().device.map(|d| d.name);
        match mic::open_source(&spec, &list, out_name.as_deref(), &self.w.engine) {
            Ok((stream, view)) => {
                let f = stream.format().clone();
                let rate = f.sample_rate.max(1);
                let channels = usize::from(f.channels.max(1));
                let changed = rate != self.rate || channels != self.channels;
                self.rate = rate;
                self.channels = channels;
                self.declared_s = f.declared_latency_ns.unwrap_or(0) as f64 / 1e9;
                self.channel = channel.unwrap_or(0).min(channels - 1);
                self.block = Vec::with_capacity(f.max_block_frames.max(4096) * channels);
                self.device = view;
                self.w.spec = spec;
                self.stream = Some(stream);
                self.lost = None;
                self.silent_ns = None;
                self.held = None;
                if changed {
                    self.aligner = RefAligner::new(rate, self.offset_ns, REFERENCE_LEAD_S);
                    if self.take.running {
                        self.w.guide_service.set_take_active(None);
                        self.take = Take::default();
                    }
                    self.renew_chain();
                } else {
                    self.aligner.reset_lineup();
                }
            }
            Err(e) => {
                self.lost = Some(mic::plain_input_error(&e));
                self.lost_state = "failed";
                if self.take.running {
                    self.take.input_lost = true;
                }
            }
        }
        self.push_status();
    }

    /// A fresh chain for the next take (same input and settings).
    fn renew_chain(&mut self) {
        let keys = timeline_now(&*self.w.engine);
        self.chain = new_chain(&self.w, self.rate, self.declared_s, self.tap.is_some(), keys);
        self.live = PracticeLive::default();
    }

    // ------------------------------------------------------------------ the end of a take

    /// Grades the running take. `save`: keep it in the song's history when enough was sung
    /// (APP_API 12.2); `drop_short`: a take too short for the history gives no result at
    /// all (practice_stop, a song change), else it is still shown (Finish).
    fn finish_take(&mut self, completed: bool, save: bool, drop_short: bool) -> Option<TakeOutcome> {
        let mut flags: Vec<&str> = Vec::new();
        if let Some(s) = &self.stream {
            let clipped = s.levels().channels.get(self.channel).map_or(0, |c| c.clipped_samples);
            if clipped > self.take.clipped0 {
                flags.push("clipping");
            }
            if s.health().dropped_blocks > self.take.dropped0 {
                flags.push("dropouts");
            }
        }
        if self.take.input_lost {
            flags.push("input_lost");
        }
        if self.device.bluetooth {
            flags.push("bluetooth_input");
        }
        let covered = self.chain.progress().covered;
        let mut r = self.chain.finish(completed, &flags);
        // The take's own count of heard key changes covers what the scorer's per-run
        // count can miss (a change between two segments).
        r.key_changes = r.key_changes.max(self.take.key_changes);
        // In `auto` the octave found is where the next take starts (and where the lane stays),
        // as it is saved for the next session (SONG_FORMAT 11 `octave.shift`).
        if self.w.octave.0 == OctaveMode::Auto {
            self.w.octave.1 = r.octave.clamp(-2, 2);
        }
        self.w.guide_service.set_take_active(None);
        self.take = Take::default();
        let long_enough = covered >= result::MIN_SAVED_SHARE;
        let out = if !long_enough && drop_short {
            None
        } else {
            let stored = result::stored(&r, &self.w.app_version);
            let (saved, best) = if save && long_enough { self.save(&stored) } else { (false, false) };
            Some(TakeOutcome { view: result::screen_view(&stored, best, saved, self.w.guide.song_key), saved })
        };
        self.renew_chain();
        self.push_status();
        out
    }

    /// The result into the song's slot (best + 20 recent, SONG_FORMAT 11), through the
    /// song-file lock so no guide swap or other slot write runs meanwhile. In `auto` the
    /// octave found becomes the song's starting point next time. Returns (saved, best).
    fn save(&self, r: &jk_format::practice::PracticeResult) -> (bool, bool) {
        let _files = self.w.guide_service.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.w.player.saver.flush();
        let mode = self.w.octave.0;
        let mut best = false;
        let res = jk_format::practice::save_practice(&self.w.path, |p| {
            p.add_result(r.clone());
            best = p.best.as_ref().is_some_and(|b| b.date == r.date && b.overall == r.overall);
            if mode == OctaveMode::Auto {
                p.octave.mode = "auto".into();
                p.octave.shift = r.octave.clamp(-2, 2);
            }
        });
        match res {
            Ok(_) => {
                crate::applog::info(&format!(
                    "practice: {}: result saved ({:?}, graded {} of {})",
                    self.w.song_id, r.overall, r.graded, r.total
                ));
                (true, best)
            }
            Err(e) => {
                crate::applog::warn(&format!("practice: {}: result not saved: {e}", self.w.song_id));
                (false, false)
            }
        }
    }

    fn hand_over(&self, o: TakeOutcome) {
        emit(&*self.w.sink, names::PRACTICE_RESULT, &o.view);
        if let Some(svc) = self.w.svc.upgrade() {
            svc.take_finished(&self.w.song_id, o.view);
        }
    }

    // ------------------------------------------------------------------ events and state

    fn send_live(&mut self) {
        self.chain.drain_live(&mut self.live);
        if self.live.pitch.is_empty() && self.live.hits.is_empty() && self.live.done.is_empty() {
            return;
        }
        self.pitch_out.clear();
        self.pitch_out.extend(
            self.live
                .pitch
                .iter()
                .map(|&(t, m, l)| (r3(t), m.map(|m| (m * 1000.0).round() / 1000.0), (f64::from(l) * 10.0).round() / 10.0)),
        );
        self.hits_out.clear();
        self.hits_out.extend(self.live.hits.iter().map(|&(n, a, b, w)| (n, r3(a), r3(b), w)));
        self.done_out.clear();
        self.done_out.extend(
            self.live
                .done
                .iter()
                .map(|&(n, c, k, e)| (n, c.map(r3), k, e.map(|e| (e * 10.0).round() / 10.0))),
        );
        self.live_seq += 1;
        emit(
            &*self.w.sink,
            names::PRACTICE_LIVE,
            &LiveEvent { seq: self.live_seq, segment: self.live.segment, pitch: &self.pitch_out, hits: &self.hits_out, done: &self.done_out },
        );
    }

    fn send_levels(&mut self) {
        let Some(stream) = &self.stream else { return };
        let lv = stream.levels();
        emit(&*self.w.sink, mic::names::MIC_LEVELS, &mic::levels_event(&self.device.id, &lv, Some(self.channel)));
        // The suggested voice channel, when the singer did not pick one: followed once it
        // has been the suggestion for half a second.
        if self.w.channel_choice.is_none()
            && let Some(c) = jk_recorder::input::suggest_voice_channel(&lv)
        {
            if self.suggested_streak.0 == Some(c) {
                self.suggested_streak.1 += 1;
            } else {
                self.suggested_streak = (Some(c), 1);
            }
            if c != self.channel && self.suggested_streak.1 >= 10 {
                self.channel = c;
            }
        }
    }

    fn input_view(&self) -> Option<PracticeInputView> {
        let meter = self
            .stream
            .as_ref()
            .and_then(|s| s.levels().channels.get(self.channel).map(|c| mic::channel_meter(self.channel, c)));
        let state = if self.suspended {
            "suspended"
        } else if self.lost.is_some() {
            self.lost_state
        } else {
            match self.stream.as_ref().map(InputStream::state) {
                Some(InputState::Running) => "running",
                Some(InputState::Starting) => "starting",
                _ => "failed",
            }
        };
        let message = if self.suspended {
            Some("The microphone is closed while the window is hidden.".to_string())
        } else {
            self.lost.clone().or_else(|| meter.as_ref().and_then(|m| m.message.clone()))
        };
        Some(PracticeInputView {
            device: self.device.clone(),
            channel: self.channel,
            sample_rate: self.rate,
            state,
            message,
            meter,
            dropped_blocks: self.stream.as_ref().map_or(0, |s| s.health().dropped_blocks),
        })
    }

    fn music_view(&self) -> MusicView {
        let m = self.chain.music();
        let presence = match m.presence {
            Presence::Searching => "searching",
            Presence::Absent => "absent",
            Presence::Present => "present",
        };
        let removal = match m.removal {
            RemovalQuality::NotNeeded => "not_needed",
            RemovalQuality::Checking => "checking",
            RemovalQuality::Good => "good",
            RemovalQuality::Marginal => "marginal",
            RemovalQuality::Unusable => "unusable",
        };
        let message = (m.presence == Presence::Present).then(|| music_message(m.delay_ms, m.removal, m.stability));
        MusicView {
            presence,
            level_db: m.level_db.map(|x| (f64::from(x) * 10.0).round() / 10.0),
            delay_ms: m.delay_ms.map(|x| (x * 10.0).round() / 10.0),
            stability: match m.stability {
                DelayStability::Unknown => "unknown",
                DelayStability::Stable => "stable",
                DelayStability::Wandering => "wandering",
            },
            drift_ppm: m.drift_ppm.map(|x| (x * 10.0).round() / 10.0),
            removal,
            suppression_db: m.suppression_db.map(|x| (f64::from(x) * 10.0).round() / 10.0),
            grading: match m.grading {
                MusicGrading::Normal => "normal",
                MusicGrading::LessCertain => "less_certain",
                MusicGrading::NotGraded => "not_graded",
            },
            message,
        }
    }

    fn latency_view(&self) -> LatencyView {
        let l = self.chain.latency();
        let (applied, source) = match l.source {
            LatencySource::ReturnedMusic => (l.applied_ms, "returned_music"),
            LatencySource::RigCheck => (l.applied_ms, "rig_check"),
            _ if self.w.manual_ms != 0.0 => (self.w.manual_ms, "manual"),
            _ => (0.0, "none"),
        };
        LatencyView {
            applied_ms: (applied * 10.0).round() / 10.0,
            source,
            measured_ms: l.measured_ms.or(self.w.stored_latency_s.map(|s| s * 1000.0)).map(|x| (x * 10.0).round() / 10.0),
            manual_ms: self.w.manual_ms,
        }
    }

    fn take_view(&self) -> Option<TakeView> {
        let p = self.chain.progress();
        Some(TakeView {
            running: self.take.running,
            started_s: r3(self.take.started_s),
            covered: (p.covered * 1000.0).round() / 1000.0,
            graded: p.graded,
            total: p.total,
            overall: p.overall,
            seeks: p.seeks,
            key_changes: self.take.key_changes,
        })
    }

    fn push_status(&self) {
        let Some(svc) = self.w.svc.upgrade() else { return };
        let (shift, settled) = self.chain.scorer().octave();
        svc.worker_status(
            &self.w.song_id,
            super::WorkerStatus {
                input: self.input_view(),
                lost: self.lost.clone(),
                music: self.music_view(),
                octave_shift: shift,
                octave_settled: settled,
                latency: self.latency_view(),
                take: self.take_view(),
                take_running: self.take.running,
            },
        );
    }

    /// A delay measured from the returned music (stable: a cable or USB) is kept for this
    /// computer, input and output (APP_API 12.7), at most every 10 s and only when it moved.
    fn save_measured_latency(&mut self) {
        let l = self.chain.latency();
        if l.source != LatencySource::ReturnedMusic {
            return;
        }
        let Some(ms) = l.measured_ms else { return };
        if let Some((prev, at)) = self.last_measured_saved
            && ((prev - ms).abs() < 1.0 || at.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        self.last_measured_saved = Some((ms, Instant::now()));
        let key = self.w.measured_key.clone();
        self.w.settings.update(|s| {
            s.practice.measured_latency_ms.insert(
                key.clone(),
                crate::settings::MeasuredLatency {
                    ms: (ms * 10.0).round() / 10.0,
                    how: "returned_music".into(),
                    utc: jk_format::time::utc_now_iso8601(),
                    extra: Default::default(),
                },
            );
        });
    }
}

/// Extra diagnostics in the log (`JK_PRACTICE_DEBUG` set): the echo stage's report and the
/// aligner's counters every 250 ms. Never any sound.
fn debug_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("JK_PRACTICE_DEBUG").is_some())
}

/// The plain words of `MusicInInput.message` (APP_API 12.5).
pub fn music_message(delay_ms: Option<f64>, removal: RemovalQuality, stability: DelayStability) -> String {
    let late = delay_ms.map(|d| format!(" (about {:.0} ms late)", d.max(0.0))).unwrap_or_default();
    let effect = match (removal, stability) {
        (_, DelayStability::Wandering) | (RemovalQuality::Unusable, _) => {
            "Its delay keeps changing (as it does over Bluetooth), so it cannot be removed: the marker may show the music, \
             and this take will not be graded. Practice needs the music on a cable or over USB."
        }
        (RemovalQuality::Good, _) => "The app removes it, so the marker and the grade still follow your voice.",
        (RemovalQuality::Marginal, _) => "The app removes most of it; the grade will be marked as less certain.",
        _ => "The app is learning to remove it; for a moment the marker may stay off.",
    };
    format!(
        "The mixer is sending the app's own music back into your microphone input{late}. {effect} To stop it, switch off \
         the mixer's loopback, or send the music to the mixer over USB instead of AUX-IN or Bluetooth."
    )
}

/// The engine's key timeline as the scorer's (constant from the key heard when the engine
/// has none yet).
fn timeline_now(engine: &dyn AudioEngine) -> KeyTimeline {
    let mut tl = KeyTimeline::default();
    let segs = engine.key_timeline();
    if segs.is_empty() {
        let s = engine.snapshot();
        tl.start_segment(s.key.segment, 0.0, s.key.heard);
    } else {
        forward_into(&mut tl, &segs);
    }
    tl
}

fn forward_into(tl: &mut KeyTimeline, segs: &[jk_engine::KeySegment]) {
    for s in segs {
        let Some(&(t0, k0)) = s.keys.first() else { continue };
        tl.start_segment(s.segment, t0.min(s.start_s), k0);
        for &(t, k) in &s.keys[1..] {
            tl.change(s.segment, t, k);
        }
    }
}

/// The engine's key marks to the chain's scorer (D-239: the key heard per segment).
fn forward_keys(chain: &mut Chain, segs: &[jk_engine::KeySegment]) {
    for s in segs {
        let Some(&(t0, k0)) = s.keys.first() else { continue };
        chain.key_segment(s.segment, t0.min(s.start_s), k0);
        for &(t, k) in &s.keys[1..] {
            chain.key_change(s.segment, t, k);
        }
    }
}

fn new_chain(w: &WorkerInit, rate: u32, declared_s: f64, with_reference: bool, keys: KeyTimeline) -> Chain {
    // The voice channel is taken out before the chain (one channel in).
    let mut cfg = ChainConfig::new(rate, 1, 0);
    cfg.reference_channels = if with_reference { 2 } else { 0 };
    cfg.declared_input_latency_s = declared_s;
    cfg.reference_lead_s = REFERENCE_LEAD_S;
    // The manual offset is applied to the playback anchors while nothing is measured
    // (see `turn`), so it can change during a take; the chain gets 0 for it.
    cfg.latency = LatencyConfig { auto: w.auto_latency, stored_s: w.stored_latency_s, manual_s: 0.0 };
    cfg.score = ScoreConfig {
        strictness: w.strictness,
        octave_mode: w.octave.0,
        octave_shift: w.octave.1,
        ..ScoreConfig::default()
    };
    Chain::new(cfg, w.guide.score.clone(), keys, w.excluded.clone())
}

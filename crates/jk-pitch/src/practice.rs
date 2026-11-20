// SPDX-License-Identifier: GPL-3.0-or-later
//! The practice analysis chain (docs/ARCHITECTURE.md 12.2 steps 2-8): one pure object that
//! turns microphone blocks into the marker's pitch values, the lit parts of the bars, the
//! "music in the input" status and the take's grade. The app core's analysis thread and
//! the offline harness (`jukeoke-cli practice run`) run exactly this code, so a
//! performance file graded offline gets the grade the app would give it (WO2 4.7 "Same
//! everywhere"). No threads, no I/O, no clock reads: the caller hands in every time stamp.
//!
//! Per input block ([`Chain::process`]):
//!
//! 1. **Channel.** The chosen channel of the interleaved block is taken alone; channels
//!    are never added together (WO2 4.4, trap 14).
//! 2. **Echo.** [`crate::echo::EchoCanceller`] compares it with the reference (what the app
//!    played, handed in by the caller sample-aligned to the input "to its best knowledge":
//!    the reference frame that left the DAC when each input frame was captured, at the
//!    input's rate), detects and removes the returned playback and judges each stretch
//!    (Clean / Singer / Music / Unknown). No reference: no echo stage (everything Clean).
//! 3. **Tracker.** [`crate::tracker::Tracker`] on the cleaned channel, never told the
//!    expected note (trap 6).
//! 4. **Gate.** A pitch in a stretch judged Music or Unknown is dropped: left-over music
//!    can never move the marker or earn credit. Unknown stretches are marked so the
//!    scorer does not charge the singer for them.
//! 5. **Song time.** Each value's input sample becomes a host time through the block time
//!    stamps (host clock, never sample counting: trap 4), minus the input latency the
//!    device declares, then a song time and segment through the playback clock anchors
//!    (interpolated between the anchors either side, never across a segment), minus the
//!    applied latency (APP_API 12.7 "Timing": the delay of the returned music when it is
//!    measured, else a stored measurement, else the manual offsets, else 0).
//! 6. **Scorer.** [`crate::score::Scorer`] with the key heard at each moment
//!    ([`Chain::key_segment`], [`Chain::key_change`]: the engine's key marks, D-239).
//! 7. **Outputs.** [`Chain::drain_live`] (pitch values, hits, finished notes),
//!    [`Chain::music`] (music in the input), [`Chain::progress`], and [`Chain::finish`]
//!    (the result with its trust flags).
//!
//! Everything is allocated in [`Chain::new`]; `process` only appends to buffers reserved
//! there (the score's sample store, the live batch) as long as the caller drains them.

use serde::{Deserialize, Serialize};

use crate::echo::{DelayStability, EchoCanceller, EchoConfig, EchoReport, Presence, RemovalQuality, Trust};
use crate::score::{
    FinishContext, KeyTimeline, ScoreConfig, ScoreGuide, ScoreLive, ScoreResult, Scorer, SingerSample, TakeProgress,
};
use crate::tracker::{PitchEvent, Tracker, TrackerConfig};

/// Where the applied latency comes from (APP_API 12.5 `latency.source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencySource {
    /// Measured live from the app's own music coming back in the input.
    ReturnedMusic,
    /// The practice rig check's stored measurement.
    RigCheck,
    /// The manual offsets (the input's plus the output device's).
    Manual,
    /// Nothing: the scorer's timing tolerance absorbs what is left.
    None,
}

/// How the singer's timing is corrected (APP_API 12.7 "Timing").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LatencyConfig {
    /// Use a measured delay (`practice.auto_latency`).
    pub auto: bool,
    /// The last stored measurement for this computer, input and output (s), if any.
    pub stored_s: Option<f64>,
    /// The manual offset of the input plus the output device's offset (s).
    pub manual_s: f64,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self { auto: true, stored_s: None, manual_s: 0.0 }
    }
}

/// Chain settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Input sample rate (Hz) and channel count, and the channel that carries the voice.
    pub input_rate: u32,
    pub input_channels: usize,
    pub channel: usize,
    /// Channels of the reference handed to [`Chain::process`] (1 or 2), or 0 when there is
    /// none (no echo stage).
    pub reference_channels: usize,
    /// Input latency the device declares (s), taken off each capture time.
    pub declared_input_latency_s: f64,
    /// How much earlier than its best estimate the caller hands the reference (s), so a
    /// return arriving slightly before the estimate is still found; subtracted from the
    /// echo stage's measured delay.
    pub reference_lead_s: f64,
    pub latency: LatencyConfig,
    pub tracker: TrackerConfig,
    pub echo: EchoConfig,
    pub score: ScoreConfig,
    /// Largest block handed to the inner stages at once (longer input blocks are split).
    pub max_block: usize,
}

impl ChainConfig {
    /// Defaults for an input at `rate` Hz with `channels` channels, the voice on `channel`,
    /// and a stereo reference.
    pub fn new(rate: u32, channels: usize, channel: usize) -> Self {
        Self {
            input_rate: rate,
            input_channels: channels.max(1),
            channel,
            reference_channels: 2,
            declared_input_latency_s: 0.0,
            reference_lead_s: 0.0,
            latency: LatencyConfig::default(),
            tracker: TrackerConfig::default(),
            echo: EchoConfig::default(),
            score: ScoreConfig::default(),
            max_block: 4096,
        }
    }
}

/// One reading of the playback clock: the song time at the DAC (declared output latency
/// already removed) at a host time, its segment, and whether it is playing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockAnchor {
    pub host_ns: i64,
    pub song_s: f64,
    pub segment: u64,
    pub playing: bool,
}

/// How the grade treats the music in the input (APP_API 12.5 `MusicInInput.grading`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicGrading {
    Normal,
    LessCertain,
    NotGraded,
}

/// The app's own playback in the input (APP_API 12.5 `MusicInInput`, without the message,
/// which is the app's wording).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicStatus {
    pub presence: Presence,
    pub level_db: Option<f32>,
    /// Residual delay beyond the declared latencies (ms).
    pub delay_ms: Option<f64>,
    pub stability: DelayStability,
    pub drift_ppm: Option<f64>,
    pub removal: RemovalQuality,
    pub suppression_db: Option<f32>,
    pub grading: MusicGrading,
}

/// The applied latency now (APP_API 12.5 `latency`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatencyStatus {
    pub applied_ms: f64,
    pub source: LatencySource,
    pub measured_ms: Option<f64>,
    pub manual_ms: f64,
}

/// What the UI gets about every 16 ms (APP_API 12.5 `practice://live`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PracticeLive {
    /// Segment of the newest value.
    pub segment: u64,
    /// `[song_time_s, midi | null, level_db]`, one per tracker hop: not snapped, not octave
    /// shifted. `null`: no pitch (silence, breath, noise, or only returned music).
    pub pitch: Vec<(f64, Option<f64>, f32)>,
    /// `[note, from_s, to_s, where]` and `[note, credit, key, mean error]` (see
    /// [`ScoreLive`]).
    pub hits: Vec<(usize, f64, f64, i8)>,
    pub done: Vec<(usize, Option<f64>, i32, Option<f64>)>,
}

/// Counters over the whole session (diagnostics and the harness's evidence).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChainStats {
    /// Tracker values produced.
    pub values: u64,
    /// Values with a pitch shown to the singer (after the gate).
    pub shown: u64,
    /// Pitches dropped because the stretch held only returned music.
    pub dropped_music: u64,
    /// Pitches dropped because the stretch could not be judged yet.
    pub dropped_unknown: u64,
    /// Values with no song time (no playback clock yet).
    pub unmapped: u64,
    /// Values while the returned music was present and its removal unusable (shown, not
    /// graded).
    pub unusable_values: u64,
    /// Input seconds analysed.
    pub input_s: f64,
    /// Values given to the scorer (while playing).
    pub scored: u64,
    /// Input seconds with the returned music present, and per removal verdict.
    pub music_present_s: f64,
    pub removal_good_s: f64,
    pub removal_marginal_s: f64,
    pub removal_unusable_s: f64,
    /// Input seconds before the return was first declared present (detection time).
    pub first_present_at_s: Option<f64>,
}

/// Block time stamps kept (enough for several seconds of any block size).
const STAMPS: usize = 512;
/// Playback clock anchors kept (2.5 s at one every 5 ms).
const ANCHORS: usize = 512;
/// The echo report is refreshed this often (s of input).
const REPORT_EVERY_S: f64 = 0.05;
/// The result says the music came back (`music_in_input` and a removal verdict) only when
/// it was present for at least this long, or this share of the take if shorter (a take of
/// a short song): a second or two of a false detection (a singer in unison with the
/// song's backing vocals can briefly look like returned music) must not mark a result.
const FLAG_MIN_PRESENT_S: f64 = 5.0;
const FLAG_MIN_PRESENT_SHARE: f64 = 0.05;

/// The practice analysis chain. See the module documentation.
pub struct Chain {
    cfg: ChainConfig,
    tracker: Tracker,
    echo: Option<EchoCanceller>,
    scorer: Scorer,
    /// Input frames consumed.
    n_in: u64,
    /// (first input frame, host ns) per block, circular.
    stamps: [(u64, i64); STAMPS],
    n_stamps: usize,
    anchors: [ClockAnchor; ANCHORS],
    n_anchors: usize,
    mono: Vec<f32>,
    clean: Vec<f32>,
    events: Vec<PitchEvent>,
    live: PracticeLive,
    score_live: ScoreLive,
    report: Option<EchoReport>,
    report_at: u64,
    report_every: u64,
    latency: LatencyStatus,
    /// How late the returned music arrives now (ms, display), while it is present.
    music_delay_ms: Option<f64>,
    /// The last delay measured while the returned music was present with a `Stable`
    /// delay (ms): the singer's timing correction from then on, also after the music
    /// stops coming back (the devices, hence the delay, are the same).
    last_measured_ms: Option<f64>,
    measured_ever: bool,
    stats: ChainStats,
}

impl Chain {
    /// A chain for one take of one song: the guide, the key timeline known so far and the
    /// excluded spans. Everything is allocated here.
    ///
    /// Panics if the channel is not among the input's channels.
    pub fn new(cfg: ChainConfig, guide: ScoreGuide, keys: KeyTimeline, excluded: Vec<(f64, f64)>) -> Self {
        assert!(cfg.channel < cfg.input_channels, "channel {} of {}", cfg.channel, cfg.input_channels);
        let rate = cfg.input_rate;
        let tracker = Tracker::with_config(rate, cfg.tracker.clone());
        let echo = (cfg.reference_channels > 0).then(|| {
            let mut e = cfg.echo.clone();
            e.reference_channels = cfg.reference_channels;
            EchoCanceller::with_config(rate, e)
        });
        let hop = tracker.hop_len().max(1);
        let block = cfg.max_block.max(64);
        let expected = guide.duration_s + 30.0;
        let scorer = Scorer::new(cfg.score.clone(), guide, keys, excluded, expected);
        let manual_ms = 1000.0 * cfg.latency.manual_s;
        Chain {
            tracker,
            echo,
            scorer,
            n_in: 0,
            stamps: [(0, 0); STAMPS],
            n_stamps: 0,
            anchors: [ClockAnchor { host_ns: 0, song_s: 0.0, segment: 0, playing: false }; ANCHORS],
            n_anchors: 0,
            mono: vec![0.0; block],
            clean: vec![0.0; block],
            events: Vec::with_capacity(block / hop + 8),
            live: PracticeLive {
                segment: 0,
                pitch: Vec::with_capacity(1024),
                hits: Vec::with_capacity(256),
                done: Vec::with_capacity(64),
            },
            score_live: ScoreLive { hits: Vec::with_capacity(256), done: Vec::with_capacity(64) },
            report: None,
            report_at: 0,
            report_every: ((REPORT_EVERY_S * f64::from(rate)) as u64).max(1),
            latency: LatencyStatus { applied_ms: 0.0, source: LatencySource::None, measured_ms: None, manual_ms },
            music_delay_ms: None,
            last_measured_ms: None,
            measured_ever: false,
            stats: ChainStats::default(),
            cfg,
        }
    }

    pub fn config(&self) -> &ChainConfig {
        &self.cfg
    }

    /// A reading of the playback clock (the app samples it every 5 ms and on every engine
    /// snapshot). Anchors arrive in host-time order.
    pub fn playback_anchor(&mut self, a: ClockAnchor) {
        self.anchors[self.n_anchors % ANCHORS] = a;
        self.n_anchors += 1;
    }

    /// The engine started a segment at `start_s` in `key` (forwarded key marks, D-239).
    pub fn key_segment(&mut self, segment: u64, start_s: f64, key: i32) {
        self.scorer.keys_mut().start_segment(segment, start_s, key);
    }

    /// From `at_s` on, `key` is heard in `segment`.
    pub fn key_change(&mut self, segment: u64, at_s: f64, key: i32) {
        self.scorer.keys_mut().change(segment, at_s, key);
    }

    /// The scorer (settings changes during a take, the target at a time, the samples).
    pub fn scorer(&self) -> &Scorer {
        &self.scorer
    }

    pub fn scorer_mut(&mut self) -> &mut Scorer {
        &mut self.scorer
    }

    /// One input block: `input` interleaved with `input_channels` channels, its first frame
    /// captured at host time `host_ns` (the capture thread's stamp, not counted from
    /// samples), and `reference` with `reference_channels` samples per input frame (empty
    /// when there is no reference). Any length.
    ///
    /// Panics if the reference length does not match.
    pub fn process(&mut self, host_ns: i64, input: &[f32], reference: &[f32]) {
        let ch = self.cfg.input_channels;
        let rc = self.cfg.reference_channels;
        let frames = input.len() / ch;
        if rc > 0 {
            assert_eq!(reference.len(), frames * rc, "reference frames");
        }
        if frames == 0 {
            return;
        }
        self.stamps[self.n_stamps % STAMPS] = (self.n_in, host_ns);
        self.n_stamps += 1;
        let mut off = 0usize;
        while off < frames {
            let m = (frames - off).min(self.mono.len());
            for i in 0..m {
                let x = input[(off + i) * ch + self.cfg.channel];
                self.mono[i] = if x.is_finite() { x } else { 0.0 };
            }
            match self.echo.as_mut() {
                Some(e) => e.process(&reference[off * rc..(off + m) * rc], &self.mono[..m], &mut self.clean[..m]),
                None => self.clean[..m].copy_from_slice(&self.mono[..m]),
            }
            self.events.clear();
            let events = &mut self.events;
            self.tracker.process(&self.clean[..m], |ev| events.push(ev));
            self.n_in += m as u64;
            for k in 0..self.events.len() {
                let ev = self.events[k];
                self.handle(ev);
            }
            off += m;
        }
        if self.echo.is_some() && self.n_in >= self.report_at + self.report_every {
            let dt = (self.n_in - self.report_at) as f64 / f64::from(self.cfg.input_rate);
            self.report_at = self.n_in;
            self.refresh_report(dt);
        }
    }

    /// Refreshes the echo report, the applied latency and the music accounting.
    fn refresh_report(&mut self, dt: f64) {
        let Some(e) = self.echo.as_ref() else { return };
        let r = e.report();
        let lead_ms = 1000.0 * self.cfg.reference_lead_s;
        if r.presence == Presence::Present {
            self.stats.music_present_s += dt;
            if self.stats.first_present_at_s.is_none() {
                self.stats.first_present_at_s = Some(self.n_in as f64 / f64::from(self.cfg.input_rate));
            }
            match r.quality {
                RemovalQuality::Good => self.stats.removal_good_s += dt,
                RemovalQuality::Marginal => self.stats.removal_marginal_s += dt,
                RemovalQuality::Unusable => self.stats.removal_unusable_s += dt,
                _ => {}
            }
        }
        let now = (r.presence == Presence::Present).then(|| e.measured_delay_s().map(|d| 1000.0 * d - lead_ms)).flatten();
        self.music_delay_ms = now;
        // Only a delay the echo stage calls stable (measured over about 5 s of music, fixed
        // or drifting in a straight line: a cable or USB) corrects the singer's timing; a
        // wandering one (Bluetooth) or a brief detection never does.
        if r.stability == DelayStability::Stable && now.is_some() {
            self.last_measured_ms = now;
        }
        self.latency.measured_ms = self.last_measured_ms;
        self.report = Some(r);
        self.choose_latency();
    }

    /// APP_API 12.7: measured (live, then stored) when `auto`, else manual, else none.
    fn choose_latency(&mut self) {
        let l = &self.cfg.latency;
        let (ms, src) = match (l.auto, self.latency.measured_ms, l.stored_s) {
            (true, Some(m), _) => (m, LatencySource::ReturnedMusic),
            (true, None, Some(s)) => (1000.0 * s, LatencySource::RigCheck),
            _ if l.manual_s != 0.0 => (1000.0 * l.manual_s, LatencySource::Manual),
            _ => (0.0, LatencySource::None),
        };
        if matches!(src, LatencySource::ReturnedMusic | LatencySource::RigCheck) {
            self.measured_ever = true;
        }
        self.latency.applied_ms = ms;
        self.latency.source = src;
    }

    /// One tracker value through the gate, the clocks and the scorer.
    fn handle(&mut self, ev: PitchEvent) {
        self.stats.values += 1;
        let lat = self.echo.as_ref().map(|e| e.latency() as i64).unwrap_or(0);
        // The input sample the value describes (the tracker runs on the canceller's output).
        let k = ev.time - lat;
        let half = (self.tracker.window_len() / 2) as i64;
        let trust = match self.echo.as_ref() {
            Some(e) => e.trust_window(k - half, k + half),
            None => Trust::Clean,
        };
        let mut midi = ev.hz.filter(|h| h.is_finite() && *h > 0.0).map(|h| crate::score::hz_to_midi(f64::from(h)));
        // While the returned music is present but its removal is unusable (a wandering
        // Bluetooth delay), the marker still shows what the gate lets through, but nothing
        // there is graded (APP_API 12.5 `not_graded`).
        let unusable = self.report.is_some_and(|r| r.presence == Presence::Present && r.quality == RemovalQuality::Unusable);
        let music = trust == Trust::Music;
        // A pitch in a stretch judged to hold only returned music is dropped (never shown,
        // never credited), and the moment is not judged either: the left-overs might be
        // the music, or the canceller's gate might have taken the singer for it (it is
        // cautious), so it neither earns nor costs. Stretches with no pitch at all still
        // count as not sung, so silence keeps the lowest grade with the music coming back.
        let unknown = trust == Trust::Unknown || unusable || (music && midi.is_some());
        if unusable {
            self.stats.unusable_values += 1;
        }
        if midi.is_some() && (trust == Trust::Unknown || music) {
            if music {
                self.stats.dropped_music += 1;
            } else {
                self.stats.dropped_unknown += 1;
            }
            midi = None;
        }
        if midi.is_some() {
            self.stats.shown += 1;
        }
        let capture_ns = self.host_of(k) - (self.cfg.declared_input_latency_s * 1e9).round() as i64;
        let Some((song, segment, playing)) = self.song_of(capture_ns) else {
            self.stats.unmapped += 1;
            return;
        };
        let t = song - self.latency.applied_ms / 1000.0;
        self.live.segment = segment;
        self.live.pitch.push((t, midi, ev.level_dbfs));
        if playing {
            self.stats.scored += 1;
            self.scorer.push(SingerSample { t, segment, midi, level_db: ev.level_dbfs, unknown, music });
        }
    }

    /// Host time (ns) of input frame `k`: the stamp of the block it arrived in plus its
    /// offset in that block at the nominal rate.
    fn host_of(&self, k: i64) -> i64 {
        let rate = f64::from(self.cfg.input_rate);
        let n = self.n_stamps.min(STAMPS);
        let newest = self.n_stamps;
        let mut chosen = None;
        for back in 1..=n {
            let s = self.stamps[(newest - back) % STAMPS];
            chosen = Some(s);
            if (s.0 as i64) <= k {
                break;
            }
        }
        let (start, host) = chosen.unwrap_or((0, 0));
        host + (((k - start as i64) as f64) * 1e9 / rate).round() as i64
    }

    /// Song time, segment and play state at host time `ns`, from the anchors either side
    /// (interpolated within a segment, extrapolated at the real-time rate from the newest
    /// anchor before when playing). `None` before the first anchor.
    fn song_of(&self, ns: i64) -> Option<(f64, u64, bool)> {
        let n = self.n_anchors.min(ANCHORS);
        if n == 0 {
            return None;
        }
        let at = |back: usize| self.anchors[(self.n_anchors - back) % ANCHORS];
        // Newest anchor at or before ns (back = 1 is the newest).
        let mut found = None;
        for back in 1..=n {
            if at(back).host_ns <= ns {
                found = Some(back);
                break;
            }
        }
        let extrapolate = |a: ClockAnchor| {
            let s = if a.playing { a.song_s + (ns - a.host_ns) as f64 / 1e9 } else { a.song_s };
            (s, a.segment, a.playing)
        };
        match found {
            None => Some(extrapolate(at(n))),
            Some(back) => {
                let a = at(back);
                if back > 1 {
                    let b = at(back - 1);
                    if b.segment == a.segment && b.host_ns > a.host_ns && a.playing && b.playing {
                        let f = (ns - a.host_ns) as f64 / (b.host_ns - a.host_ns) as f64;
                        return Some((a.song_s + f * (b.song_s - a.song_s), a.segment, true));
                    }
                }
                Some(extrapolate(a))
            }
        }
    }

    /// Takes everything new for the UI since the last call. Hand the same `out` back each
    /// time: its vectors are swapped in, so nothing is allocated.
    pub fn drain_live(&mut self, out: &mut PracticeLive) {
        self.scorer.drain_live(&mut self.score_live);
        out.pitch.clear();
        out.hits.clear();
        out.done.clear();
        out.segment = self.live.segment;
        std::mem::swap(&mut out.pitch, &mut self.live.pitch);
        out.hits.extend_from_slice(&self.score_live.hits);
        out.done.extend_from_slice(&self.score_live.done);
    }

    /// The app's own playback in the input now.
    pub fn music(&self) -> MusicStatus {
        match &self.report {
            None => MusicStatus {
                presence: if self.echo.is_some() { Presence::Searching } else { Presence::Absent },
                level_db: None,
                delay_ms: None,
                stability: DelayStability::Unknown,
                drift_ppm: None,
                removal: RemovalQuality::NotNeeded,
                suppression_db: None,
                grading: MusicGrading::Normal,
            },
            Some(r) => {
                let present = r.presence == Presence::Present;
                let grading = match (present, r.quality) {
                    (false, _) | (true, RemovalQuality::Good | RemovalQuality::NotNeeded) => MusicGrading::Normal,
                    (true, RemovalQuality::Unusable) => MusicGrading::NotGraded,
                    (true, _) => MusicGrading::LessCertain,
                };
                MusicStatus {
                    presence: r.presence,
                    level_db: r.level_db,
                    delay_ms: present.then_some(self.music_delay_ms).flatten(),
                    stability: r.stability,
                    drift_ppm: r.drift_ppm,
                    removal: r.quality,
                    suppression_db: r.suppression_db,
                    grading,
                }
            }
        }
    }

    /// The echo stage's full report (diagnostics), when there is an echo stage.
    pub fn echo_report(&self) -> Option<EchoReport> {
        self.echo.as_ref().map(|e| e.report())
    }

    /// The applied latency now.
    pub fn latency(&self) -> LatencyStatus {
        self.latency
    }

    /// The running state of the take.
    pub fn progress(&self) -> TakeProgress {
        self.scorer.progress()
    }

    pub fn stats(&self) -> ChainStats {
        self.stats
    }

    /// Ends the take and grades it. `completed`: the take ran to the end of the song;
    /// `flags`: trust flags only the caller knows (`clipping`, `input_lost`, `dropouts`,
    /// `bluetooth_input`). The chain adds `music_in_input` with `removal_active`,
    /// `removal_marginal` or `untrusted` (removal unusable for half of the take or more:
    /// untrusted, no grade; marginal or worse for a quarter or more, or at the end: marginal;
    /// else active) when the music was present for at least 5 s (or 5 % of a shorter take;
    /// moments it made unjudgeable are never graded either way), and `latency_unmeasured`
    /// (no measured delay was ever applied).
    pub fn finish(&mut self, completed: bool, flags: &[&str]) -> ScoreResult {
        if self.echo.is_some() {
            let dt = (self.n_in - self.report_at) as f64 / f64::from(self.cfg.input_rate);
            self.report_at = self.n_in;
            self.refresh_report(dt);
        }
        self.stats.input_s = self.n_in as f64 / f64::from(self.cfg.input_rate);
        let mut f: Vec<String> = Vec::new();
        let s = self.stats;
        let flag_min = FLAG_MIN_PRESENT_S.min(FLAG_MIN_PRESENT_SHARE * s.input_s);
        if s.music_present_s > 0.0 && s.music_present_s >= flag_min {
            f.push("music_in_input".into());
            // Moments while removal was unusable were not graded (see `handle`); the flag
            // says how much that was: half of the take or more is untrusted (no grade), a
            // quarter or more of marginal-or-worse removal, or ending in such a state, is
            // marginal.
            let take = s.input_s.max(1e-9);
            let last = self.report.map(|r| if r.presence == Presence::Present { r.quality } else { RemovalQuality::NotNeeded });
            let last_bad = matches!(last, Some(RemovalQuality::Marginal | RemovalQuality::Unusable));
            let verdict = if s.removal_unusable_s >= 0.5 * take {
                "untrusted"
            } else if last_bad || s.removal_marginal_s + s.removal_unusable_s >= 0.25 * take {
                "removal_marginal"
            } else {
                "removal_active"
            };
            f.push(verdict.into());
        }
        if !self.measured_ever {
            f.push("latency_unmeasured".into());
        }
        for x in flags {
            f.push((*x).to_string());
        }
        self.scorer.finish(&FinishContext { completed, flags: f })
    }
}

#[cfg(test)]
mod tests;

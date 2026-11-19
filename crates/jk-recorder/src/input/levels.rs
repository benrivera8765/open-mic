// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-channel input levels for practice mode (WO2 section 4.4, "Choosing the
//! channel" and "Levels").
//!
//! Computed on the analysis thread from the blocks it reads, **never on the
//! capture callback**. For every channel separately:
//!
//! * peak and RMS over the last 300 ms (the meter);
//! * the peak over the last 3 s (what the advice is based on);
//! * the noise floor: the 10th percentile of 50 ms RMS windows over the last
//!   10 s (the quiet gaps between phrases), once 1 s has been heard;
//! * the "active" level: the 90th percentile of the same windows (how loud the
//!   channel is when something is on it), used to suggest the voice channel;
//! * clipping: samples at or above [`INPUT_CLIP_LEVEL`], in the last 2 s and in
//!   total.
//!
//! Channels are never added together. The app never changes the input gain;
//! the advice tells the singer what to change on the mixer.

use serde::Serialize;

/// A sample at or above this magnitude counts as clipped (-0.009 dBFS, the top
/// code of a 16- or 24-bit converter after scaling to float).
pub const INPUT_CLIP_LEVEL: f32 = 0.999;
/// Below this peak over 3 s a channel that has *some* signal is "too quiet".
/// A close-miked dynamic microphone on a mixer set for it peaks far above this
/// when sung into (about -20 to -6 dBFS).
pub const TOO_QUIET_PEAK_DBFS: f32 = -30.0;
/// How far above its noise floor a channel must peak to count as carrying
/// something (a voice, or music).
pub const SIGNAL_MARGIN_DB: f32 = 12.0;
/// A channel whose 3-second peak stays below this carries no sound at all
/// (digital silence: nothing plugged in, or a muted channel).
pub const SILENT_CHANNEL_DBFS: f32 = -90.0;
/// A channel whose active level (90th percentile of 50 ms RMS) reaches this
/// carries something even if it never pauses (used by [`suggest_voice_channel`]).
pub const ACTIVE_CHANNEL_DBFS: f32 = -50.0;
/// The lowest level reported (instead of minus infinity, so values serialise).
pub const FLOOR_DBFS: f32 = -160.0;

/// Length of one analysis window.
pub const WINDOW_SECONDS: f64 = 0.05;
/// Windows kept for the noise floor and the active level (10 s).
pub const HISTORY_WINDOWS: usize = 200;
/// Windows for the meter's peak and RMS (300 ms).
const METER_WINDOWS: usize = 6;
/// Windows for the advice peak (3 s).
const ADVICE_WINDOWS: usize = 60;
/// Windows for "clipped recently" (2 s).
const CLIP_WINDOWS: usize = 40;
/// Windows needed before a noise floor is reported (1 s).
const FLOOR_MIN_WINDOWS: usize = 20;

/// Linear magnitude to dBFS, floored at [`FLOOR_DBFS`].
pub fn dbfs(x: f64) -> f32 {
    if x > 0.0 {
        ((20.0 * x.log10()) as f32).max(FLOOR_DBFS)
    } else {
        FLOOR_DBFS
    }
}

/// What the singer should do about a channel's level, in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelAdvice {
    /// The level is fine.
    Good,
    /// Nothing above the noise floor yet (nobody singing): no advice.
    Waiting,
    /// Something is coming in, but too quietly for reliable pitch tracking.
    TooQuiet,
    /// The input reached full scale in the last 2 seconds.
    Clipping,
    /// The channel carries no sound at all.
    Silent,
}

impl LevelAdvice {
    /// The sentence the practice screen shows (empty for `Good` / `Waiting`).
    pub fn message(self) -> &'static str {
        match self {
            LevelAdvice::Good | LevelAdvice::Waiting => "",
            LevelAdvice::TooQuiet => {
                "Your voice is coming in too quietly. Turn up the microphone's gain on the mixer \
                 (Jukeoke never changes it)."
            }
            LevelAdvice::Clipping => {
                "Your voice is too loud and is clipping. Turn down the microphone's gain on the \
                 mixer, or sing a little further from the microphone."
            }
            LevelAdvice::Silent => {
                "No sound at all on this channel. Pick the channel your microphone is on, or check \
                 that the mixer is sending to the computer."
            }
        }
    }
}

/// One channel's levels at a moment.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelLevel {
    /// Peak over the last 300 ms, dBFS.
    pub peak_dbfs: f32,
    /// RMS over the last 300 ms, dBFS.
    pub rms_dbfs: f32,
    /// Peak over the last 3 s, dBFS.
    pub peak_3s_dbfs: f32,
    /// Noise floor (10th percentile of 50 ms RMS over up to 10 s), dBFS; `None`
    /// until 1 s has been heard.
    pub noise_floor_dbfs: Option<f32>,
    /// Active level (90th percentile of 50 ms RMS over up to 10 s), dBFS.
    pub active_rms_dbfs: Option<f32>,
    /// A clipped sample in the last 2 s.
    pub clipped_recently: bool,
    /// Clipped samples since the stream opened.
    pub clipped_samples: u64,
    /// What to tell the singer.
    pub advice: LevelAdvice,
}

/// All channels' levels.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct InputLevels {
    /// One entry per channel, in device order.
    pub channels: Vec<ChannelLevel>,
    /// Seconds of sound analysed so far.
    pub seconds: f64,
}

#[derive(Clone, Copy, Default)]
struct Window {
    rms: f32,
    peak: f32,
    clipped: bool,
}

/// The meter. Owned by the analysis thread.
pub struct ChannelLevels {
    channels: usize,
    window_frames: usize,
    filled: usize,
    cur_sum_sq: Vec<f64>,
    cur_peak: Vec<f32>,
    cur_clipped: Vec<bool>,
    /// `history[ch]` is a ring of closed windows; `hist_len` of them are valid,
    /// the newest at `hist_pos - 1`.
    history: Vec<Vec<Window>>,
    hist_pos: usize,
    hist_len: usize,
    clipped_total: Vec<u64>,
    frames_total: u64,
    sample_rate: u32,
}

impl ChannelLevels {
    /// A meter for `channels` channels at `sample_rate`. Allocates everything now.
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        let c = usize::from(channels.max(1));
        Self {
            channels: c,
            window_frames: ((f64::from(sample_rate.max(1)) * WINDOW_SECONDS).round() as usize)
                .max(1),
            filled: 0,
            cur_sum_sq: vec![0.0; c],
            cur_peak: vec![0.0; c],
            cur_clipped: vec![false; c],
            history: vec![vec![Window::default(); HISTORY_WINDOWS]; c],
            hist_pos: 0,
            hist_len: 0,
            clipped_total: vec![0; c],
            frames_total: 0,
            sample_rate: sample_rate.max(1),
        }
    }

    /// Channels metered.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Meter one block of interleaved samples. Does not allocate.
    pub fn process(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.channels) {
            for (ch, &s) in frame.iter().enumerate() {
                let s = if s.is_finite() { s } else { 0.0 };
                let a = s.abs();
                if a > self.cur_peak[ch] {
                    self.cur_peak[ch] = a;
                }
                if a >= INPUT_CLIP_LEVEL {
                    self.cur_clipped[ch] = true;
                    self.clipped_total[ch] += 1;
                }
                self.cur_sum_sq[ch] += f64::from(s) * f64::from(s);
            }
            self.filled += 1;
            self.frames_total += 1;
            if self.filled == self.window_frames {
                self.close_window();
            }
        }
    }

    fn close_window(&mut self) {
        for ch in 0..self.channels {
            self.history[ch][self.hist_pos] = Window {
                rms: (self.cur_sum_sq[ch] / self.filled as f64).sqrt() as f32,
                peak: self.cur_peak[ch],
                clipped: self.cur_clipped[ch],
            };
            self.cur_sum_sq[ch] = 0.0;
            self.cur_peak[ch] = 0.0;
            self.cur_clipped[ch] = false;
        }
        self.filled = 0;
        self.hist_pos = (self.hist_pos + 1) % HISTORY_WINDOWS;
        self.hist_len = (self.hist_len + 1).min(HISTORY_WINDOWS);
    }

    /// The newest `n` closed windows of channel `ch`, newest first.
    fn recent(&self, ch: usize, n: usize) -> impl Iterator<Item = &Window> {
        let n = n.min(self.hist_len);
        (0..n).map(move |i| {
            let idx = (self.hist_pos + HISTORY_WINDOWS - 1 - i) % HISTORY_WINDOWS;
            &self.history[ch][idx]
        })
    }

    /// The levels now. Allocates (call it at UI rate, not per sample).
    pub fn snapshot(&self) -> InputLevels {
        let mut out = InputLevels {
            channels: Vec::with_capacity(self.channels),
            seconds: self.frames_total as f64 / f64::from(self.sample_rate),
        };
        let mut sorted: Vec<f32> = Vec::with_capacity(HISTORY_WINDOWS);
        for ch in 0..self.channels {
            // Meter: closed windows of the last 300 ms plus the open one.
            let mut peak = self.cur_peak[ch];
            let mut sum_sq = self.cur_sum_sq[ch];
            let mut frames = self.filled;
            for w in self.recent(ch, METER_WINDOWS) {
                peak = peak.max(w.peak);
                sum_sq += f64::from(w.rms) * f64::from(w.rms) * self.window_frames as f64;
                frames += self.window_frames;
            }
            let rms = if frames > 0 {
                (sum_sq / frames as f64).sqrt()
            } else {
                0.0
            };
            let peak_3s = self
                .recent(ch, ADVICE_WINDOWS)
                .fold(self.cur_peak[ch], |m, w| m.max(w.peak));
            let clipped_recently =
                self.cur_clipped[ch] || self.recent(ch, CLIP_WINDOWS).any(|w| w.clipped);
            sorted.clear();
            sorted.extend(self.recent(ch, HISTORY_WINDOWS).map(|w| w.rms));
            sorted.sort_by(f32::total_cmp);
            let pct = |p: f64| -> Option<f32> {
                if sorted.len() < FLOOR_MIN_WINDOWS {
                    return None;
                }
                let i = ((sorted.len() - 1) as f64 * p).round() as usize;
                Some(dbfs(f64::from(sorted[i])))
            };
            let noise_floor = pct(0.10);
            let active = pct(0.90);
            let peak_3s_dbfs = dbfs(f64::from(peak_3s));
            let advice = advise(peak_3s_dbfs, noise_floor, clipped_recently);
            out.channels.push(ChannelLevel {
                peak_dbfs: dbfs(f64::from(peak)),
                rms_dbfs: dbfs(rms),
                peak_3s_dbfs,
                noise_floor_dbfs: noise_floor,
                active_rms_dbfs: active,
                clipped_recently,
                clipped_samples: self.clipped_total[ch],
                advice,
            });
        }
        out
    }
}

/// The advice for one channel from its 3-second peak, noise floor and clip flag.
pub fn advise(
    peak_3s_dbfs: f32,
    noise_floor_dbfs: Option<f32>,
    clipped_recently: bool,
) -> LevelAdvice {
    if clipped_recently {
        return LevelAdvice::Clipping;
    }
    if peak_3s_dbfs < SILENT_CHANNEL_DBFS {
        return LevelAdvice::Silent;
    }
    if peak_3s_dbfs >= TOO_QUIET_PEAK_DBFS {
        return LevelAdvice::Good;
    }
    // Weak: "too quiet" only if it stands out from the room noise (someone is
    // singing, just not loudly enough); otherwise nobody is singing yet.
    // Without a floor yet, assume a quiet room (-70 dBFS).
    let floor = noise_floor_dbfs.unwrap_or(-70.0);
    if peak_3s_dbfs >= floor + SIGNAL_MARGIN_DB {
        LevelAdvice::TooQuiet
    } else {
        LevelAdvice::Waiting
    }
}

/// Suggest the channel that carries the voice: the channel whose active level
/// is highest, among channels that carry something: an active level at least
/// [`SIGNAL_MARGIN_DB`] above their own noise floor (singing with pauses), or at
/// least [`ACTIVE_CHANNEL_DBFS`] (a sound held for the whole 10 s window). Ties
/// within 1 dB go to the lower channel number (a mixer that copies one
/// microphone to both sides).
/// `None` while no channel carries anything.
///
/// This is only the default until the singer picks: if the mixer sends music
/// on another channel louder than the voice, the music channel wins, which is
/// why the practice screen shows a meter per channel and lets the singer choose
/// (WO2 section 4.4).
pub fn suggest_voice_channel(levels: &InputLevels) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (ch, l) in levels.channels.iter().enumerate() {
        let (Some(active), Some(floor)) = (l.active_rms_dbfs, l.noise_floor_dbfs) else {
            continue;
        };
        let carries = active >= floor + SIGNAL_MARGIN_DB || active >= ACTIVE_CHANNEL_DBFS;
        if !carries || l.peak_3s_dbfs < SILENT_CHANNEL_DBFS {
            continue;
        }
        match best {
            Some((_, b)) if active <= b + 1.0 => {}
            _ => best = Some((ch, active)),
        }
    }
    best.map(|(ch, _)| ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f64, amp: f32, f: f64) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n)
            .map(|i| amp * (std::f64::consts::TAU * f * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    fn stereo(l: &[f32], r: &[f32]) -> Vec<f32> {
        l.iter().zip(r).flat_map(|(&a, &b)| [a, b]).collect()
    }

    #[test]
    fn per_channel_levels_are_independent() {
        let rate = 48_000;
        // Left: a sine peaking at -6 dBFS (RMS -9 dBFS). Right: silence.
        let l = tone(rate, 2.0, 0.5, 440.0);
        let r = vec![0.0; l.len()];
        let mut m = ChannelLevels::new(2, rate);
        m.process(&stereo(&l, &r));
        let s = m.snapshot();
        assert!(
            (s.channels[0].peak_dbfs + 6.02).abs() < 0.05,
            "{:?}",
            s.channels[0]
        );
        assert!((s.channels[0].rms_dbfs + 9.03).abs() < 0.05);
        assert_eq!(s.channels[1].peak_dbfs, FLOOR_DBFS);
        assert_eq!(s.channels[1].advice, LevelAdvice::Silent);
        assert_eq!(s.channels[0].advice, LevelAdvice::Good);
        assert!((s.seconds - 2.0).abs() < 1e-9);
    }

    #[test]
    fn noise_floor_comes_from_the_gaps() {
        let rate = 16_000;
        // 1 s of noise-like low tone at -60 dBFS peak, then 1 s of voice at -12.
        let mut ch = tone(rate, 1.0, 0.001, 97.0);
        ch.extend(tone(rate, 1.0, 0.25, 220.0));
        let mut m = ChannelLevels::new(1, rate);
        m.process(&ch);
        let s = &m.snapshot().channels[0];
        let floor = s.noise_floor_dbfs.unwrap();
        assert!((floor + 63.0).abs() < 0.5, "floor {floor}");
        assert!((s.active_rms_dbfs.unwrap() + 15.05).abs() < 0.5);
        assert_eq!(s.advice, LevelAdvice::Good);
        assert_eq!(suggest_voice_channel(&m.snapshot()), Some(0));
    }

    #[test]
    fn advice_thresholds() {
        assert_eq!(advise(-3.0, Some(-70.0), true), LevelAdvice::Clipping);
        assert_eq!(advise(-120.0, None, false), LevelAdvice::Silent);
        assert_eq!(advise(-65.0, Some(-70.0), false), LevelAdvice::Waiting);
        assert_eq!(advise(-40.0, Some(-70.0), false), LevelAdvice::TooQuiet);
        assert_eq!(advise(-12.0, Some(-70.0), false), LevelAdvice::Good);
        assert!(!LevelAdvice::TooQuiet.message().is_empty());
    }

    #[test]
    fn clipping_is_seen_and_forgotten_after_two_seconds() {
        let rate = 8_000;
        let mut m = ChannelLevels::new(1, rate);
        m.process(&[1.0, -1.0, 0.5]);
        assert!(m.snapshot().channels[0].clipped_recently);
        assert_eq!(m.snapshot().channels[0].clipped_samples, 2);
        m.process(&tone(rate, 2.2, 0.1, 300.0));
        let s = &m.snapshot().channels[0];
        assert!(!s.clipped_recently);
        assert_eq!(s.clipped_samples, 2);
    }

    #[test]
    fn voice_channel_suggestion() {
        let rate = 48_000;
        let n = rate as usize * 2;
        // Voice on the right only, with gaps; left is a constant faint hum.
        let mut voice = tone(rate, 1.0, 0.0, 1.0);
        voice.extend(tone(rate, 1.0, 0.3, 330.0));
        let hum = tone(rate, 2.0, 0.0005, 50.0);
        let mut m = ChannelLevels::new(2, rate);
        m.process(&stereo(&hum[..n], &voice[..n]));
        assert_eq!(suggest_voice_channel(&m.snapshot()), Some(1));
        // Nothing anywhere: no suggestion.
        let mut q = ChannelLevels::new(2, rate);
        q.process(&vec![0.0; n * 2]);
        assert_eq!(suggest_voice_channel(&q.snapshot()), None);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming pitch tracker for the singer's microphone (WORK_ORDER-2 4.4 and 6, "Live tracker").
//!
//! The method is the McLeod Pitch Method (P. McLeod and G. Wyvill, "A smarter way to find
//! pitch", ICMC 2005), written directly on `realfft`:
//!
//! 1. The input (one mono channel, chosen by the caller: channels are never summed) goes
//!    through a 4th-order Butterworth high-pass at 55 Hz (below C2 = 65.4 Hz) that takes the
//!    bulk of handling thumps, popped consonants, DC and 50 Hz hum away before analysis.
//! 2. Every `hop` (10 ms) the last `window` of filtered samples (42.7 ms: 2048 samples at
//!    48 kHz, sized in *time*, so 16, 44.1, 48 and 96 kHz give the same answer) is analysed:
//!    the normalised square difference function (NSDF)
//!    `n(t) = 2 r(t) / m(t)`, with `r` the autocorrelation (one forward and one inverse real
//!    FFT of the zero-padded window) and `m(t) = sum x[j]^2 + x[j+t]^2` (updated
//!    incrementally per lag).
//! 3. Peak picking as in the paper: the "key maximum" (highest local maximum) of every
//!    positive region of the NSDF after the zero-lag lobe; the first key maximum that reaches
//!    `peak_threshold` (k = 0.9) times the highest one is the period. Parabolic interpolation
//!    refines its lag and gives the clarity (the NSDF value at the peak, 0..1).
//! 4. Singing / not singing: a clarity threshold with hysteresis (`clarity_on` = 0.93 to
//!    start, `clarity_off` = 0.75 to keep going, the latter only while the pitch continues
//!    the line within `max_step_cents`, so a breath after a note cannot drag the pitch
//!    around) and a level gate with hysteresis on the window's RMS (-55 / -60 dBFS).
//! 5. A minimum run of voiced analysis frames (`min_voiced_frames`, default 2, each
//!    continuing the previous one's pitch) and a three-point median of the voiced
//!    estimates: a thump, a pop or a burst of noise cannot become a note, and a single-frame
//!    octave slip never reaches the output.
//!
//! Output: one [`PitchEvent`] per hop (time in input samples, Hz or `None`, clarity). After
//! [`Tracker::new`] nothing is allocated: the FFT plans, buffers and scratch are owned by the
//! tracker and events are handed to a caller closure.
//!
//! Delay (measured in docs/benchmarks/practice-tracker.md): an event is ready at the end of
//! its newest analysis window. From a note's onset that is the part of the window the new
//! note must fill before its clarity passes `clarity_on` (about 20 ms in the middle of the
//! range, the whole 42.7 ms window for the lowest notes), plus the wait for the next hop
//! (0..10 ms), plus one hop for the minimum run / median.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub mod testvoice;

#[cfg(test)]
mod tests;

/// Tracker settings. Every duration is in seconds and every frequency in Hz, so the same
/// settings mean the same thing at any sample rate.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TrackerConfig {
    /// Length of the analysis window. 2048 samples at 48 kHz: long enough that the NSDF of
    /// a C2 (period 15.3 ms) is computed over 1.8 periods (WO2 trap 5).
    pub window_s: f64,
    /// Time between two estimates.
    pub hop_s: f64,
    /// Lowest pitch reported. 62 Hz: just below C2 (65.4 Hz) and above 60 Hz mains hum.
    pub min_hz: f64,
    /// Highest pitch reported. 1400 Hz: a little above C6 (1046.5 Hz) plus vibrato.
    pub max_hz: f64,
    /// MPM's relative peak threshold k: the first key maximum reaching k times the highest
    /// key maximum is the period.
    pub peak_threshold: f32,
    /// Clarity needed to start reporting a pitch. 0.93: the highest clarity measured on
    /// generated breath, whisper, consonants, room noise, thumps and pops was 0.897, and a
    /// held voice reaches it within about 25 ms (docs/benchmarks/practice-tracker.md).
    pub clarity_on: f32,
    /// Clarity needed to keep reporting a pitch once started (hysteresis), as long as the
    /// pitch continues the line (`max_step_cents`). Keeps breathy and vibrato notes whole.
    pub clarity_off: f32,
    /// Window RMS (after the high-pass), in dBFS, needed to start reporting a pitch.
    pub level_on_dbfs: f32,
    /// Window RMS needed to keep reporting a pitch once started.
    pub level_off_dbfs: f32,
    /// Corner of the 4th-order Butterworth high-pass applied before analysis.
    pub highpass_hz: f64,
    /// Voiced analysis frames in a row needed before a pitch is reported (minimum duration).
    /// While a run is starting, every frame must continue the previous one's pitch.
    pub min_voiced_frames: u32,
    /// Largest pitch change between two analysis frames (10 ms apart) that counts as the
    /// same sung line: needed for a run to start, and for a frame below `clarity_on` to
    /// keep one going. 150 cents is several times the fastest vibrato (+-50 cents at 6 Hz
    /// moves at most 19 cents per 10 ms) and glide (an octave in 0.1 s is 120).
    pub max_step_cents: f32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            window_s: 2048.0 / 48_000.0,
            hop_s: 0.010,
            min_hz: 62.0,
            max_hz: 1400.0,
            peak_threshold: 0.9,
            clarity_on: 0.93,
            clarity_off: 0.75,
            level_on_dbfs: -55.0,
            level_off_dbfs: -60.0,
            highpass_hz: 55.0,
            min_voiced_frames: 2,
            max_step_cents: 150.0,
        }
    }
}

/// One estimate, produced every hop.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PitchEvent {
    /// The moment this estimate describes, in input samples counted from the first sample
    /// given to the tracker (at the input's own rate): the centre of the analysed audio
    /// (the middle analysis window of the three the median looks at). Compare this, not
    /// `ready_at`, with the song: it is where the sound was sung. Negative for the first
    /// few events of a stream (their windows reach back before the first sample).
    pub time: i64,
    /// The input sample count at which this event became available: the end of the newest
    /// analysis window. `ready_at - time` is constant (half a window plus one hop).
    pub ready_at: u64,
    /// The pitch sung, or `None` when the input is not a sung (pitched) sound.
    pub hz: Option<f32>,
    /// MPM clarity of the newest analysis frame, 0..1 (1 = perfectly periodic). Given also
    /// when `hz` is `None`, for diagnostics.
    pub clarity: f32,
    /// RMS level of the newest analysis window after the high-pass, in dBFS (-200 for
    /// digital silence).
    pub level_dbfs: f32,
}

/// Raw result of analysing one window, before the voicing logic.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    hz: Option<f32>,
    clarity: f32,
    /// Mean square of the window (linear power).
    power: f64,
}

/// Streaming McLeod pitch tracker for one mono channel. See the module documentation.
pub struct Tracker {
    cfg: TrackerConfig,
    rate: u32,
    /// Analysis window length in samples.
    win: usize,
    /// Samples between two analyses.
    hop: usize,
    /// Lag range searched for the period (samples), inclusive.
    lag_lo: usize,
    lag_hi: usize,
    /// High-pass (two biquad sections).
    hp: [Biquad; 2],
    /// The last `win` filtered samples, circular; `ring_pos` is the oldest.
    ring: Vec<f32>,
    ring_pos: usize,
    since_hop: usize,
    consumed: u64,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    /// Zero-padded window (FFT input), then the autocorrelation (inverse FFT output).
    fft_buf: Vec<f32>,
    acf: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// The window in time order (kept: the FFT input buffer is destroyed by the transform).
    frame: Vec<f32>,
    /// NSDF for lags `0..=lag_hi + 1`.
    nsdf: Vec<f32>,
    /// Linear power thresholds of the level gate.
    power_on: f64,
    power_off: f64,
    voiced: bool,
    run: u32,
    /// The last three raw voiced estimates (`None` = that frame was not voiced), oldest first.
    recent: [Option<f32>; 3],
}

impl Tracker {
    /// A tracker for a mono stream at `rate` Hz with the default settings.
    pub fn new(rate: u32) -> Self {
        Self::with_config(rate, TrackerConfig::default())
    }

    /// A tracker for a mono stream at `rate` Hz. Everything the tracker needs is allocated
    /// here; [`Tracker::process`] never allocates.
    ///
    /// Panics if `rate` is below 8 kHz or the settings are inconsistent (a window shorter
    /// than two periods of `min_hz`, `max_hz` above 0.45 x `rate`, a zero hop).
    pub fn with_config(rate: u32, cfg: TrackerConfig) -> Self {
        assert!(
            rate >= 8_000,
            "sample rate {rate} Hz is too low for the tracker"
        );
        let fs = f64::from(rate);
        let win = (cfg.window_s * fs).round() as usize;
        let hop = (cfg.hop_s * fs).round() as usize;
        assert!(hop >= 1, "hop must be at least one sample");
        assert!(
            cfg.max_hz <= 0.45 * fs,
            "max_hz must stay below 0.45 x the sample rate"
        );
        assert!(
            cfg.min_hz > 0.0 && cfg.min_hz < cfg.max_hz,
            "bad pitch range"
        );
        // Lags searched: from just below the period of max_hz to just above that of min_hz.
        let lag_lo = ((fs / cfg.max_hz).floor() as usize).max(2);
        let lag_hi = (fs / cfg.min_hz).ceil() as usize + 1;
        assert!(
            2 * lag_hi <= win + 2,
            "window too short for min_hz: needs about two periods"
        );
        // Linear (not circular) autocorrelation for lags up to win - 1 needs N >= 2 win - 1.
        let fft_len = (2 * win).next_power_of_two();
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(fft_len);
        let c2r = planner.plan_fft_inverse(fft_len);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        let db_to_power = |db: f32| 10f64.powf(f64::from(db) / 10.0);
        let mut t = Self {
            rate,
            win,
            hop,
            lag_lo,
            lag_hi,
            hp: Biquad::butterworth_highpass4(fs, cfg.highpass_hz),
            ring: vec![0.0; win],
            ring_pos: 0,
            since_hop: 0,
            consumed: 0,
            fft_buf: r2c.make_input_vec(),
            acf: c2r.make_output_vec(),
            spectrum: r2c.make_output_vec(),
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            r2c,
            c2r,
            frame: vec![0.0; win],
            nsdf: vec![0.0; lag_hi + 2],
            power_on: db_to_power(cfg.level_on_dbfs),
            power_off: db_to_power(cfg.level_off_dbfs),
            voiced: false,
            run: 0,
            recent: [None; 3],
            cfg,
        };
        t.reset();
        t
    }

    /// The settings in use.
    pub fn config(&self) -> &TrackerConfig {
        &self.cfg
    }

    /// Input sample rate in Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Analysis window length in samples at this rate.
    pub fn window_len(&self) -> usize {
        self.win
    }

    /// Samples between two events at this rate.
    pub fn hop_len(&self) -> usize {
        self.hop
    }

    /// `ready_at - time` of every event, in samples: how far the moment an event describes
    /// lies behind the moment it is available (half a window plus one hop for the median).
    pub fn describe_delay(&self) -> u64 {
        (self.win / 2 + self.hop) as u64
    }

    /// Samples consumed since construction or the last [`Tracker::reset`].
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Forget all history (for example after the input device changed). Sample counting
    /// restarts at zero. Does not allocate.
    pub fn reset(&mut self) {
        for s in &mut self.hp {
            s.clear();
        }
        self.ring.fill(0.0);
        self.ring_pos = 0;
        self.since_hop = 0;
        self.consumed = 0;
        self.voiced = false;
        self.run = 0;
        self.recent = [None; 3];
    }

    /// Feed a block of mono samples (any length, including empty). `on_event` is called once
    /// for every hop boundary crossed inside the block, in time order. Never allocates.
    /// Non-finite samples (a broken driver) are treated as silence.
    pub fn process(&mut self, block: &[f32], mut on_event: impl FnMut(PitchEvent)) {
        for &x in block {
            self.push(x, &mut on_event);
        }
    }

    /// Feed one channel of an interleaved block (`channels` samples per frame; the voice on
    /// `channel`). The other channels are ignored, never summed in (WO2 trap 14).
    ///
    /// Panics if `channel >= channels`.
    pub fn process_interleaved(
        &mut self,
        block: &[f32],
        channels: usize,
        channel: usize,
        mut on_event: impl FnMut(PitchEvent),
    ) {
        assert!(channel < channels, "channel {channel} of {channels}");
        for &x in block.iter().skip(channel).step_by(channels) {
            self.push(x, &mut on_event);
        }
    }

    #[inline]
    fn push(&mut self, x: f32, on_event: &mut impl FnMut(PitchEvent)) {
        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        let first = self.hp[0].tick(x);
        let y = self.hp[1].tick(first);
        self.ring[self.ring_pos] = y as f32;
        self.ring_pos += 1;
        if self.ring_pos == self.win {
            self.ring_pos = 0;
        }
        self.consumed += 1;
        self.since_hop += 1;
        if self.since_hop == self.hop {
            self.since_hop = 0;
            let ev = self.step();
            on_event(ev);
        }
    }

    /// Analyse the current window and run the voicing logic: one event.
    fn step(&mut self) -> PitchEvent {
        let f = self.analyse();
        let min_run = self.cfg.min_voiced_frames.max(1);
        // Is this frame's pitch within max_step_cents of the previous voiced frame's?
        let prev = self.recent[2];
        let continuous = match (f.hz, prev) {
            (Some(a), Some(b)) => (1200.0 * (a / b).log2()).abs() <= self.cfg.max_step_cents,
            _ => false,
        };
        let confident = f.clarity >= self.cfg.clarity_on;
        self.voiced = f.hz.is_some()
            && if self.voiced {
                // Keep going: loud enough, and either confident (a new note may start
                // anywhere) or at least clarity_off and continuing the same line (so a
                // breath right after a note cannot drag the pitch around).
                f.power >= self.power_off
                    && (confident || (f.clarity >= self.cfg.clarity_off && continuous))
            } else {
                confident && f.power >= self.power_on
            };
        self.run = if !self.voiced {
            0
        } else if self.run > 0 && self.run < min_run && !continuous {
            // Still starting up and the pitch jumped: the run restarts here. Noise gives
            // a pitch that wanders from frame to frame; a voice does not.
            1
        } else {
            self.run.saturating_add(1)
        };
        self.recent = [
            self.recent[1],
            self.recent[2],
            if self.voiced { f.hz } else { None },
        ];
        let hz = if self.voiced && self.run >= min_run {
            Some(self.median_recent())
        } else {
            None
        };
        let ready_at = self.consumed;
        PitchEvent {
            time: ready_at as i64 - self.describe_delay() as i64,
            ready_at,
            hz,
            clarity: f.clarity,
            level_dbfs: if f.power > 1e-20 {
                (10.0 * f.power.log10()) as f32
            } else {
                -200.0
            },
        }
    }

    /// Median of the voiced values among the last three frames: the true median of three,
    /// or the newest of two (the start of a run, where the newest window holds the most of
    /// the new note), or the only one.
    fn median_recent(&self) -> f32 {
        match self.recent {
            [Some(a), Some(b), Some(c)] => a.max(b).min(a.min(b).max(c)),
            [_, _, Some(c)] => c,
            // The newest frame is voiced whenever this is called.
            [_, Some(b), None] => b,
            _ => 0.0,
        }
    }

    /// McLeod Pitch Method on the current window.
    fn analyse(&mut self) -> Frame {
        let win = self.win;
        // Window in time order: the ring's oldest sample is at ring_pos.
        let (newer, older) = self.ring.split_at(self.ring_pos);
        self.frame[..older.len()].copy_from_slice(older);
        self.frame[older.len()..].copy_from_slice(newer);

        let energy: f64 = self
            .frame
            .iter()
            .map(|&v| f64::from(v) * f64::from(v))
            .sum();
        let power = energy / win as f64;
        // Digital silence or near it: nothing to analyse (and no division by ~0 below).
        if energy < 1e-12 {
            return Frame {
                hz: None,
                clarity: 0.0,
                power,
            };
        }

        // Autocorrelation r(t) for every lag, via |FFT|^2 of the zero-padded window.
        self.fft_buf[..win].copy_from_slice(&self.frame);
        self.fft_buf[win..].fill(0.0);
        if self
            .r2c
            .process_with_scratch(&mut self.fft_buf, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            return Frame {
                hz: None,
                clarity: 0.0,
                power,
            };
        }
        for c in &mut self.spectrum {
            *c = Complex::new(c.re * c.re + c.im * c.im, 0.0);
        }
        // The inverse transform reports (harmless) non-zero imaginary DC/Nyquist parts as an
        // error; they were zeroed above, so any error here is a real one.
        if self
            .c2r
            .process_with_scratch(&mut self.spectrum, &mut self.acf, &mut self.scratch)
            .is_err()
        {
            return Frame {
                hz: None,
                clarity: 0.0,
                power,
            };
        }
        // realfft's inverse is unnormalised: r(t) = acf[t] / N. The NSDF is a ratio of r to
        // m, so scale m by N instead of r.
        let n_fft = self.fft_buf.len() as f64;

        // NSDF n(t) = 2 r(t) / m(t), m(t) = sum_{j < win - t} x[j]^2 + x[j + t]^2.
        let mut m = 2.0 * energy;
        self.nsdf[0] = 1.0;
        for t in 1..self.nsdf.len() {
            let a = f64::from(self.frame[t - 1]);
            let b = f64::from(self.frame[win - t]);
            m -= a * a + b * b;
            self.nsdf[t] = if m > 1e-12 * energy {
                (2.0 * f64::from(self.acf[t]) / (m * n_fft)) as f32
            } else {
                0.0
            };
        }

        match self.pick_peak() {
            Some((lag, clarity)) => {
                let hz = f64::from(self.rate) / lag;
                let hz = (hz >= self.cfg.min_hz && hz <= self.cfg.max_hz).then_some(hz as f32);
                Frame { hz, clarity, power }
            }
            None => Frame {
                hz: None,
                clarity: 0.0,
                power,
            },
        }
    }

    /// The paper's peak picking: key maxima of the positive regions after the zero-lag
    /// lobe, restricted to the searched lag range; the first reaching
    /// `peak_threshold` x the highest. Returns (interpolated lag, clarity).
    fn pick_peak(&self) -> Option<(f64, f32)> {
        let n = &self.nsdf;
        // Pass 1 finds the highest key maximum; pass 2 takes the first one reaching
        // k x that height.
        let mut top = f32::NEG_INFINITY;
        self.key_maxima(|t| {
            top = top.max(n[t]);
            false
        });
        if !top.is_finite() {
            return None;
        }
        let threshold = self.cfg.peak_threshold * top;
        let mut chosen = None;
        self.key_maxima(|t| {
            if n[t] >= threshold {
                chosen = Some(t);
                true
            } else {
                false
            }
        });
        let chosen = chosen?;
        // Parabolic interpolation through the peak and its neighbours.
        let (a, b, c) = (n[chosen - 1], n[chosen], n[chosen + 1]);
        let den = a - 2.0 * b + c;
        let delta = if den.abs() > 1e-12 {
            (0.5 * (a - c) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let peak = b - 0.25 * (a - c) * delta;
        Some((chosen as f64 + f64::from(delta), peak.clamp(0.0, 1.0)))
    }

    /// Calls `visit` with the lag of every key maximum in the searched lag range, in order
    /// of lag, until `visit` returns true. A key maximum is the highest local maximum of a
    /// positive region of the NSDF; the zero-lag lobe and the first negative region are
    /// skipped (McLeod and Wyvill, section 5).
    fn key_maxima(&self, mut visit: impl FnMut(usize) -> bool) {
        let n = &self.nsdf;
        let last = n.len() - 1; // n[last] exists only to test a local maximum at last - 1
        let mut t = 1;
        while t < last && n[t] > 0.0 {
            t += 1;
        }
        while t < last && n[t] <= 0.0 {
            t += 1;
        }
        let mut region_max: Option<usize> = None;
        while t < last {
            if n[t] > 0.0 {
                if n[t] > n[t - 1]
                    && n[t] >= n[t + 1]
                    && (self.lag_lo..=self.lag_hi).contains(&t)
                    && region_max.is_none_or(|r| n[t] > n[r])
                {
                    region_max = Some(t);
                }
            } else if let Some(r) = region_max.take() {
                // End of a positive region: r is its key maximum.
                if visit(r) {
                    return;
                }
            }
            t += 1;
        }
        // A region still open at the end of the lag range.
        if let Some(r) = region_max {
            visit(r);
        }
    }
}

/// Direct-form-I biquad in f64 (a 55 Hz corner at 96 kHz needs the precision).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// High-pass section with quality `q` at `fc` Hz (bilinear transform, pre-warped, so
    /// the response is the same at every sample rate).
    fn highpass(fs: f64, fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    /// 4th-order Butterworth high-pass as two sections (Q = 0.5412 and 1.3066).
    fn butterworth_highpass4(fs: f64, fc: f64) -> [Self; 2] {
        [
            Self::highpass(fs, fc, 0.541_196_100_146_197),
            Self::highpass(fs, fc, 1.306_562_964_876_376_5),
        ]
    }

    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Flush denormals so a long silence never slows the filter down.
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        self.y1
    }
}

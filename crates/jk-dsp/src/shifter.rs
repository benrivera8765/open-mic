// SPDX-License-Identifier: GPL-3.0-or-later
//! One pitch-shift path: a Rubber Band R3 real-time stretcher held at a **fixed** pitch,
//! plus a FIFO of its output addressed on the graph's absolute timeline.
//!
//! Why a fixed pitch per instance: Rubber Band's start delay depends on the pitch scale
//! (`delay = W/2 / scale` when shifting up, see `probe` example), so changing the pitch of
//! a running stretcher changes its latency and would move the band against the unshifted
//! drums. Instead the graph keeps two paths and, on a key change, primes the idle one at
//! the new pitch with the real audio just before the switch point, then crossfades.
//!
//! Timing: after `reset()` and `set_pitch_scale()`, raw output sample `o` corresponds to
//! raw input sample `o - start_delay + preferred_start_pad` (Rubber Band's documented
//! contract). On top of that, R3 places an isolated transient a pitch-dependent number of
//! samples early or late (measured: up to about -130 samples at +12, see
//! docs/benchmarks/dsp.md). [`transient_trim`] removes that bias, so drum hits and band
//! transients line up on the same sample on average.
//!
//! Chunking: R3 in real-time mode is *not* invariant to how its input is split into
//! `process()` calls when the pitch ratio is not a power of two (measured with the
//! `probe_chunking` example: up to 0.5 difference in sample values for 64- vs 512-frame
//! calls at +3 semitones). The path therefore always feeds Rubber Band exactly
//! [`RB_QUANTUM`] frames at a time, counted from its restart point, whatever block sizes
//! the caller uses. Playback, export and the cross-platform test then see identical
//! chunking.
//!
//! Fresh instances: Rubber Band 4.0.0's `reset()` does not clear all R3 state (the
//! accumulated output phase, bin classification and guidance survive; measured with the
//! `probe_reset` example: a used-then-reset stretcher differs from a fresh one by up to 0.25
//! in sample value). A restarted path therefore gets a never-used stretcher (measured
//! 2.4 ms to create + drop at 44.1 kHz on a Ryzen 7 9700X). The real-time engine builds
//! those on a helper thread and hands them over lock-free (see `PitchGraph::offer_stretcher`),
//! so the render worker never constructs or frees one; the offline renderer builds them
//! inline.

use crate::params::CHANNELS;
use crate::ring::AbsRing;
use jk_rubberband::{semitones_to_scale, PitchMode, Stretcher, StretcherConfig};

/// Rubber Band R3's longest FFT for a sample rate (mirrors `Guide.h`:
/// classification FFT = next power of two of `ceil(rate / 32)`, at least 1024; longest = 2x).
pub fn r3_longest_fft(sample_rate: u32) -> usize {
    let class = (sample_rate as usize).div_ceil(32).next_power_of_two().max(1024);
    class * 2
}

/// Measured transient bias of R3 (HighQuality pitch mode), in samples at the sample rate
/// it was measured at, indexed by `semitones + 12`. Positive = the shifted path is late.
///
/// Measured through the full graph with `cargo run --release -p jk-dsp --example
/// calibrate_trim <rate> <stride> 256` (mean impulse-peak offset of 256 isolated impulses
/// spread over Rubber Band's hop grid), averaging two independent position sets
/// (strides 17 and 23). The `drum_alignment_impulse` test validates the result on a third
/// set (stride 29). Raw numbers: docs/benchmarks/dsp.md.
///
/// R3 longest FFT 4096 (sample rates 32.8 to 65.5 kHz); measured at 44.1 kHz.
const TRIM_FFT_4096: [i32; 25] = [
    // -12  -11  -10   -9   -8   -7   -6   -5   -4   -3   -2   -1    0
        29,  27,  35,  24,  30,  38,  34,  31,  23,  22,  17,   6,   0,
    //  +1   +2   +3   +4   +5   +6   +7   +8   +9  +10  +11  +12
       -10, -19, -32, -44, -60, -69, -84, -91,-106,-116,-121,-134,
];

/// R3 longest FFT 8192 (sample rates 65.5 to 131 kHz); measured at 96 kHz. Note the
/// different shape: for +8..+12 R3 places transients 7 to 9 ms early at this FFT size.
const TRIM_FFT_8192: [i32; 25] = [
    // -12  -11  -10   -9   -8   -7   -6   -5   -4   -3   -2   -1    0
        56,  84, 117, 150, 123, 102,  74,  44,  66,  27,  39,   5,   0,
    //  +1   +2   +3   +4   +5   +6   +7   +8   +9  +10  +11  +12
        -8,   2,   1,   6,  -2,  -1,  -2,-664,-728,-792,-850,-902,
];

/// Transient trim in samples for this sample rate and key shift. FFT sizes 4096 and 8192
/// use their measured tables; other sizes (sample rates up to 32.8 kHz or above 131 kHz)
/// scale the nearest table by the FFT size ratio, which has NOT been verified.
pub fn transient_trim(sample_rate: u32, semitones: i32) -> i32 {
    let idx = (semitones.clamp(-12, 12) + 12) as usize;
    let fft = r3_longest_fft(sample_rate);
    let (table, base) = if fft <= 4096 { (&TRIM_FFT_4096, 4096) } else { (&TRIM_FFT_8192, 8192) };
    let t = table[idx] as i64;
    ((t * fft as i64) as f64 / base as f64).round() as i32
}

/// Frames per Rubber Band `process()` call (see "Chunking" above). Adds up to
/// `RB_QUANTUM - 1` frames of internal buffering, covered by the graph latency.
pub const RB_QUANTUM: usize = 256;

/// The Rubber Band configuration the graph's shifters use at `sample_rate` (stereo, R3,
/// real-time, high-quality pitch, [`RB_QUANTUM`]-frame process calls). A helper thread
/// uses it to pre-build instances off the render thread.
pub fn stretcher_config(sample_rate: u32) -> StretcherConfig {
    let mut cfg = StretcherConfig::new(sample_rate, CHANNELS);
    cfg.pitch_mode = PitchMode::HighQuality;
    cfg.max_process_size = RB_QUANTUM;
    cfg
}

pub(crate) struct ShiftPath {
    rb: Stretcher,
    rb_cfg: StretcherConfig,
    sample_rate: u32,
    pub semitones: i32,
    /// Drums are summed into this path's input (the "shift drums" option).
    pub drums_in: bool,
    /// Rubber Band output, on the graph's absolute timeline.
    fifo: AbsRing,
    /// Absolute index of the next raw output sample Rubber Band will hand us.
    next_out_abs: i64,
    /// Input collected until a full quantum is ready (allocated once).
    pending: [Vec<f32>; CHANNELS],
    pending_len: usize,
    /// Retrieve buffer (allocated once).
    out_scratch: [Vec<f32>; CHANNELS],
    /// `rb` has been fed audio since it was constructed. A stretcher that has never
    /// been fed is interchangeable with a newly constructed one (construction leaves
    /// R3 in its `JustCreated` mode, where `set_pitch_scale` recomputes everything that
    /// depends on the pitch), so a restart can reuse it instead of building another.
    fed: bool,
}

impl ShiftPath {
    pub fn new(sample_rate: u32, fifo_capacity: usize) -> Result<Self, jk_rubberband::Error> {
        let cfg = Self::stretcher_config(sample_rate);
        let rb = Stretcher::new(cfg)?;
        Ok(ShiftPath {
            rb,
            rb_cfg: cfg,
            sample_rate,
            semitones: 0,
            drums_in: false,
            fifo: AbsRing::new(CHANNELS, fifo_capacity, 0),
            next_out_abs: 0,
            pending: [vec![0.0; RB_QUANTUM], vec![0.0; RB_QUANTUM]],
            pending_len: 0,
            out_scratch: [vec![0.0; 4096], vec![0.0; 4096]],
            fed: false,
        })
    }

    /// The configuration every stretcher of a path at this rate is built with.
    pub fn stretcher_config(sample_rate: u32) -> StretcherConfig {
        stretcher_config(sample_rate)
    }

    /// True when a restart needs a new stretcher (the current one has processed audio).
    pub fn needs_fresh(&self) -> bool {
        self.fed
    }

    /// Restart this path at `semitones`, fed from absolute frame `from_abs` onward.
    /// After this, call [`feed`](Self::feed) with consecutive input starting at `from_abs`.
    ///
    /// Rubber Band instance (see "Fresh instances" above): if the current one has never
    /// been fed it is reused; otherwise `fresh` is used (a never-fed instance built with
    /// [`stretcher_config`](Self::stretcher_config), e.g. by a helper thread), and only
    /// when `fresh` is `None` is one constructed here. Returns the replaced instance, if
    /// any, so the caller can free it off the real-time thread, and whether this call
    /// constructed an instance itself.
    pub fn restart(
        &mut self,
        semitones: i32,
        drums_in: bool,
        from_abs: u64,
        fresh: Option<Stretcher>,
    ) -> (Option<Stretcher>, Option<Stretcher>, bool) {
        self.semitones = semitones;
        self.drums_in = drums_in;
        let mut retired = None;
        let mut unused = None;
        let mut constructed = false;
        if self.fed {
            let new = match fresh {
                Some(f) => Some(f),
                None => {
                    constructed = true;
                    Stretcher::new(self.rb_cfg).ok()
                }
            };
            match new {
                Some(n) => retired = Some(std::mem::replace(&mut self.rb, n)),
                // Construction cannot fail with the validated configuration; keep going
                // with reset() rather than stopping playback.
                None => self.rb.reset(),
            }
        } else {
            unused = fresh;
        }
        self.fed = false;
        self.rb.set_pitch_scale(semitones_to_scale(semitones as f64));
        let pad = self.rb.preferred_start_pad() as i64;
        let delay = self.rb.start_delay() as i64;
        let trim = transient_trim(self.sample_rate, semitones) as i64;
        // raw output 0 <-> raw input (pad - delay), shifted by the measured transient bias.
        self.next_out_abs = from_abs as i64 + pad - delay - trim;
        let start = self.next_out_abs.max(0) as u64;
        self.fifo.clear(start);
        self.pending_len = 0;
        (retired, unused, constructed)
    }

    /// Feed `frames` frames: `bus` (+ `drums` when this path carries the drums).
    /// Rubber Band is called once per complete [`RB_QUANTUM`].
    pub fn feed(&mut self, bus: &[&[f32]; CHANNELS], drums: &[&[f32]; CHANNELS], frames: usize) {
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(RB_QUANTUM - self.pending_len);
            let at = self.pending_len;
            for c in 0..CHANNELS {
                let dst = &mut self.pending[c][at..at + n];
                dst.copy_from_slice(&bus[c][done..done + n]);
                if self.drums_in {
                    for (d, s) in dst.iter_mut().zip(&drums[c][done..done + n]) {
                        *d += *s;
                    }
                }
            }
            self.pending_len += n;
            done += n;
            if self.pending_len == RB_QUANTUM {
                {
                    let [l, r] = &self.pending;
                    self.rb.process(&[&l[..], &r[..]], RB_QUANTUM, false);
                }
                self.fed = true;
                self.pending_len = 0;
                self.drain();
            }
        }
    }

    /// Move everything Rubber Band has ready into the FIFO.
    fn drain(&mut self) {
        loop {
            let avail = self.rb.available().unwrap_or(0);
            if avail == 0 {
                break;
            }
            let n = avail.min(self.out_scratch[0].len());
            let got = {
                let [l, r] = &mut self.out_scratch;
                let mut outs: [&mut [f32]; CHANNELS] = [&mut l[..n], &mut r[..n]];
                self.rb.retrieve(&mut outs)
            };
            if got == 0 {
                break;
            }
            // Drop output that maps before the timeline start (only right after a restart
            // with a negative mapping; never in steady state).
            let mut skip = 0usize;
            if self.next_out_abs < self.fifo.end() as i64 {
                skip = ((self.fifo.end() as i64 - self.next_out_abs) as usize).min(got);
            }
            if got > skip {
                let [l, r] = &self.out_scratch;
                self.fifo.push(&[&l[skip..got], &r[skip..got]], got - skip);
            }
            self.next_out_abs += got as i64;
        }
    }

    /// True if output for `[abs, abs + frames)` is ready.
    pub fn has(&self, abs: u64, frames: usize) -> bool {
        self.fifo.has(abs, frames)
    }

    /// Absolute index one past the newest output frame available.
    pub fn available_end(&self) -> u64 {
        self.fifo.end()
    }

    pub fn read_channel(&self, ch: usize, abs: u64, out: &mut [f32]) {
        self.fifo.read_channel(ch, abs, out)
    }

    pub fn start_delay(&self) -> usize {
        self.rb.start_delay()
    }

    pub fn preferred_start_pad(&self) -> usize {
        self.rb.preferred_start_pad()
    }
}

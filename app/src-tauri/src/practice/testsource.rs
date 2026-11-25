// SPDX-License-Identifier: GPL-3.0-or-later
//! A generated "mixer return" for tests: an [`InputSource`] that behaves like a mixer whose
//! loopback sends the computer's own playback back into its USB input (WO2 4.4 "Music in
//! the input", 4.10), at a fixed level and delay, optionally with a generated singer on top.
//!
//! It reads the engine's output tap (exactly what the device plays, with the host time each
//! frame leaves the DAC) and delivers it `delay_ms` later at `level_db`, stamped with the
//! host clock exactly as a capture device stamps its blocks, through the same rings and
//! meters as a microphone. Nothing is recorded: the tap is the app's own playback, and the
//! optional singer is a WAV of generated sound. Used by `practice_test_input({ kind:
//! "return" })` (test modes only) and by the rig check's tests, so the whole echo path
//! (detection, the delay, removal, the gate) runs in the real app with no microphone and
//! without a sound being made (the null output device).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use jk_recorder::input::clock::host_time_ns;
use jk_recorder::input::{CaptureProducer, FailureKind, InputError, InputFormat, InputSource};

use crate::engine::AudioEngine;

/// Pieces of the tap whose stamps the generated return's sample clock is averaged over
/// (10 ms pieces: about 2 s, as the practice aligner's line fit).
const CLOCK_PIECES: usize = 200;

/// The generated return's sample clock: like a capture device's, it counts frames from a
/// start; that start is the mean of the host times the tap's pieces imply for it, never one
/// piece's stamp. Each stamp carries the output thread's wake-up delay, so a clock started
/// from a single one (the first, taken while the computer was busy) put the whole return
/// several milliseconds late: the rig check measured 25.4 ms for a 17 ms return on a CI
/// runner (DECISIONS D-279).
struct SampleClock {
    rate: u32,
    /// Tap frame index the clock counts from.
    k0: u64,
    /// Host time of frame `k0` implied by each recent piece, and their sum.
    implied: VecDeque<i64>,
    sum: i128,
}

impl SampleClock {
    fn new(rate: u32, k0: u64, stamp: i64) -> SampleClock {
        let mut c = SampleClock { rate, k0, implied: VecDeque::with_capacity(CLOCK_PIECES + 1), sum: 0 };
        c.push(k0, stamp);
        c
    }

    /// Nanoseconds from frame `k0` to frame `k` (None before `k0`).
    fn since(&self, k: u64) -> Option<i64> {
        k.checked_sub(self.k0).map(|d| (d as f64 * 1e9 / f64::from(self.rate)) as i64)
    }

    /// The host time of frame `k` by this clock.
    fn at(&self, k: u64) -> Option<i64> {
        let t0 = (self.sum / self.implied.len().max(1) as i128) as i64;
        self.since(k).map(|d| t0 + d)
    }

    /// One more piece: its first frame `k` left at host time `stamp`.
    fn push(&mut self, k: u64, stamp: i64) {
        let Some(d) = self.since(k) else { return };
        self.implied.push_back(stamp - d);
        self.sum += i128::from(stamp - d);
        if self.implied.len() > CLOCK_PIECES
            && let Some(old) = self.implied.pop_front()
        {
            self.sum -= i128::from(old);
        }
    }
}

/// The returned playback (and a singer) as an input.
pub struct ReturnSource {
    engine: Arc<dyn AudioEngine>,
    rate: u32,
    delay_ns: i64,
    gain: f32,
    /// A generated singer (mono, at `rate`), placed by song time (sample k = song time k / rate).
    voice: Arc<[f32]>,
    offset_ns: i64,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ReturnSource {
    /// The engine's playback returned `delay_ms` late at `level_db` relative to what is
    /// played, plus `voice` (a generated singer WAV, mono or the first channel), at the
    /// output device's rate.
    pub fn new(engine: Arc<dyn AudioEngine>, delay_ms: f64, level_db: f64, voice: Option<&Path>) -> Result<ReturnSource, InputError> {
        let rate = engine
            .snapshot()
            .device
            .map(|d| d.sample_rate)
            .filter(|&r| r > 0)
            .ok_or_else(|| InputError::new(FailureKind::NoDevice, "There is no output device to return the music from."))?;
        let voice: Arc<[f32]> = match voice {
            None => Arc::from(Vec::new()),
            Some(p) => {
                let (info, samples) = jk_recorder::wav::read_wav_f32(p).map_err(|e| InputError::new(FailureKind::Format, e.to_string()))?;
                if info.sample_rate != rate {
                    return Err(InputError::new(
                        FailureKind::Format,
                        format!("the singer WAV is at {} Hz but the output runs at {rate} Hz", info.sample_rate),
                    ));
                }
                let ch = usize::from(info.channels.max(1));
                Arc::from(samples.chunks_exact(ch).map(|f| f[0]).collect::<Vec<f32>>())
            }
        };
        Ok(ReturnSource {
            engine,
            rate,
            delay_ns: (delay_ms.max(0.0) * 1e6).round() as i64,
            gain: 10f64.powf(level_db / 20.0) as f32,
            voice,
            offset_ns: super::align::clock_offset_ns(),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }
}

impl InputSource for ReturnSource {
    fn open(&mut self) -> Result<InputFormat, InputError> {
        Ok(InputFormat {
            source: "generated mixer return (the app's own playback, delayed)".into(),
            device_name: "mixer return".into(),
            sample_rate: self.rate,
            channels: 1,
            sample_format: "f32".into(),
            max_block_frames: 4096,
            declared_latency_ns: Some(0),
            notes: Vec::new(),
        })
    }

    fn start(&mut self, mut producer: CaptureProducer) -> Result<(), InputError> {
        let mut tap = self
            .engine
            .output_tap()
            .ok_or_else(|| InputError::new(FailureKind::Device, "this engine has no output tap"))?;
        let (stop, rate, delay, gain, off) = (Arc::clone(&self.stop), self.rate, self.delay_ns, self.gain, self.offset_ns);
        let voice = Arc::clone(&self.voice);
        let handle = std::thread::Builder::new()
            .name("jk-input-return".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; 2 * 4096];
                // (capture host ns of the first frame, mono frames), waiting until "captured".
                let mut queue: VecDeque<(i64, Vec<f32>)> = VecDeque::new();
                let mut clock: Option<SampleClock> = None;
                while !stop.load(Ordering::Acquire) {
                    while let Some(p) = tap.read(&mut buf) {
                        if p.rate != rate {
                            continue; // another output rate: not this source's
                        }
                        let mut m: Vec<f32> = buf[..2 * p.frames].chunks_exact(2).map(|f| 0.5 * (f[0] + f[1]) * gain).collect();
                        // The singer hears the music through the mixer and sings along: the
                        // voice for song time `pos` enters the mixer together with that music,
                        // so both come back in the same block (the return delay covers the
                        // mixer's path to the headphones and back to the computer alike).
                        if let Some(pos) = p.song_pos {
                            let v0 = (pos * f64::from(rate)).round() as i64;
                            for (j, x) in m.iter_mut().enumerate() {
                                let k = v0 + j as i64;
                                if k >= 0
                                    && let Some(v) = voice.get(k as usize)
                                {
                                    *x += *v;
                                }
                            }
                        }
                        // Stamped like a real capture device: by its sample clock (the frames
                        // counted from a start), not by each piece's wake-up time, which
                        // carries the device thread's jitter; started again after a gap.
                        let stamp = p.dac_ns as i64 + off + delay;
                        let t = match clock.as_mut().and_then(|c| c.at(p.first_frame).map(|pred| (c, pred))) {
                            Some((c, pred)) if (pred - stamp).abs() <= 20_000_000 && p.lost_before == 0 => {
                                c.push(p.first_frame, stamp);
                                c.at(p.first_frame).unwrap_or(pred)
                            }
                            _ => {
                                clock = Some(SampleClock::new(rate, p.first_frame, stamp));
                                stamp
                            }
                        };
                        queue.push_back((t, m));
                    }
                    let now = host_time_ns();
                    while let Some((t, m)) = queue.front() {
                        // A device hands a block over once its last frame was captured.
                        let end = *t + (m.len() as i64 * 1_000_000_000) / i64::from(rate);
                        if end > now {
                            break;
                        }
                        let (t, m) = queue.pop_front().expect("front exists");
                        producer.push_interleaved(&m, t, 0);
                    }
                    producer.touch();
                    std::thread::sleep(Duration::from_millis(3));
                }
            })
            .map_err(|e| InputError::new(FailureKind::Device, format!("could not start a thread: {e}")))?;
        self.thread = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ReturnSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A WAV of generated sound delivered as a singer who sings along: each block carries the
/// part of the WAV at the song time that was leaving the DAC when the block was "captured"
/// (sample k of the WAV = song time k / rate), so a generated singer made from the guide
/// lines up with the song however long the engine took to start, follows seeks, and is
/// silent while the song is paused. `late_ms` makes the singer late (positive) or early.
/// After `unplug_after_s` seconds of delivery the source reports that it was unplugged
/// (WO2 section 9 "the input unplugged during a song"). Blocks are stamped with the host
/// clock and go through the same rings and meters as a microphone.
pub struct SyncWavSource {
    engine: Arc<dyn AudioEngine>,
    rate: u32,
    channels: u16,
    samples: Arc<[f32]>,
    name: String,
    late_ns: i64,
    unplug_after_ns: Option<i64>,
    offset_ns: i64,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SyncWavSource {
    pub fn new(engine: Arc<dyn AudioEngine>, path: &Path, late_ms: f64, unplug_after_s: Option<f64>) -> Result<SyncWavSource, InputError> {
        let (info, samples) = jk_recorder::wav::read_wav_f32(path).map_err(|e| InputError::new(FailureKind::Format, e.to_string()))?;
        Ok(SyncWavSource {
            engine,
            rate: info.sample_rate.max(1),
            channels: info.channels.max(1),
            samples: Arc::from(samples),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "WAV".into()),
            late_ns: (late_ms * 1e6).round() as i64,
            unplug_after_ns: unplug_after_s.filter(|s| *s > 0.0).map(|s| (s * 1e9) as i64),
            offset_ns: super::align::clock_offset_ns(),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }
}

impl InputSource for SyncWavSource {
    fn open(&mut self) -> Result<InputFormat, InputError> {
        Ok(InputFormat {
            source: "generated singer (WAV, sung along to the song)".into(),
            device_name: self.name.clone(),
            sample_rate: self.rate,
            channels: self.channels,
            sample_format: "f32".into(),
            max_block_frames: (self.rate as usize / 100).max(1),
            declared_latency_ns: Some(0),
            notes: Vec::new(),
        })
    }

    fn start(&mut self, mut producer: CaptureProducer) -> Result<(), InputError> {
        let engine = Arc::clone(&self.engine);
        let samples = Arc::clone(&self.samples);
        let (stop, rate, ch, late, off, unplug) =
            (Arc::clone(&self.stop), self.rate, usize::from(self.channels), self.late_ns, self.offset_ns, self.unplug_after_ns);
        let handle = std::thread::Builder::new()
            .name("jk-input-sync-wav".into())
            .spawn(move || {
                let status = producer.status();
                let n = (rate as usize / 100).max(1);
                let block_ns = (n as i64 * 1_000_000_000) / i64::from(rate);
                let total = samples.len() / ch;
                let t_open = host_time_ns();
                let mut block_start = t_open;
                let mut next_idx: Option<i64> = None;
                let mut out = vec![0.0f32; n * ch];
                while !stop.load(Ordering::Acquire) {
                    let now = host_time_ns();
                    if let Some(u) = unplug
                        && now - t_open >= u
                    {
                        status.fail(FailureKind::Unplugged, "the input device was unplugged (simulated)");
                        return;
                    }
                    if now - block_start > 200_000_000 {
                        // Far behind (the computer was busy): start again from now.
                        block_start = now - block_ns;
                    }
                    let snap = engine.snapshot();
                    while block_start + block_ns <= now {
                        let playing = snap.state == jk_engine::PlaybackState::Playing && snap.position_ns != 0;
                        if playing {
                            // Song time at the DAC when this block was captured.
                            let dac_ns = snap.position_ns as i64 + off;
                            let song_t = snap.position_s + (block_start - dac_ns - late) as f64 / 1e9;
                            let want = (song_t * f64::from(rate)).round() as i64;
                            let idx = match next_idx {
                                Some(i) if (i - want).abs() <= i64::from(rate) / 20 => i,
                                _ => want,
                            };
                            for j in 0..n {
                                let k = idx + j as i64;
                                for c in 0..ch {
                                    out[j * ch + c] = if k >= 0 && (k as usize) < total { samples[k as usize * ch + c] } else { 0.0 };
                                }
                            }
                            next_idx = Some(idx + n as i64);
                        } else {
                            out.fill(0.0);
                            next_idx = None;
                        }
                        producer.push_interleaved(&out, block_start, 0);
                        block_start += block_ns;
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            })
            .map_err(|e| InputError::new(FailureKind::Device, format!("could not start a thread: {e}")))?;
        self.thread = Some(handle);
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SyncWavSource {
    fn drop(&mut self) {
        self.stop();
    }
}

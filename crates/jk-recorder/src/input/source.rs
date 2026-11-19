// SPDX-License-Identifier: GPL-3.0-or-later
//! Sources that are not a microphone but go through exactly the same path:
//! a generated signal or a WAV file, delivered in blocks by a thread that
//! behaves like a capture callback (it only copies samples and a time stamp
//! into the ring). Tests and the command-line probe use them (WO2 section 6:
//! "feed a file or a generated signal through the whole practice path as if it
//! were a microphone").

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::InputError;
use super::clock::{NS_PER_SECOND, host_time_ns};
use super::ring::{CaptureProducer, FailureKind};
use super::{InputFormat, InputSource};

/// How fast a generated source delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// One block per block-length of real time, stamped with the host clock at
    /// the moment its first frame "was captured", like a device.
    RealTime,
    /// As fast as the reader takes them (it waits when the ring is full instead
    /// of dropping); stamps are synthetic: open time + frames / rate.
    AsFastAsPossible,
}

/// What happens when a generated source reaches the end of its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalEnd {
    /// The stream reports that it ended.
    Ends,
    /// Start again from the beginning.
    Loops,
    /// Behave like a device that was unplugged: report `Unplugged`.
    Unplugs,
    /// Stop delivering without a word, like a dead driver (the stall watchdog
    /// must notice).
    GoesQuiet,
}

/// A generated (or file) signal delivered block by block.
pub struct SignalSource {
    name: String,
    rate: u32,
    channels: u16,
    samples: Arc<[f32]>,
    block_frames: usize,
    pacing: Pacing,
    end: SignalEnd,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SignalSource {
    /// A source of `interleaved` samples with `channels` channels at `rate`.
    /// Defaults: 10 ms blocks, real-time pacing, ends at the end.
    pub fn new(name: impl Into<String>, rate: u32, channels: u16, interleaved: Vec<f32>) -> Self {
        Self {
            name: name.into(),
            rate: rate.max(1),
            channels: channels.max(1),
            samples: interleaved.into(),
            block_frames: (rate as usize / 100).max(1),
            pacing: Pacing::RealTime,
            end: SignalEnd::Ends,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// A 32-bit float WAV file (the crate's own reader), delivered the same way.
    pub fn from_wav(path: &Path) -> Result<Self, InputError> {
        let (info, samples) = crate::wav::read_wav_f32(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "WAV file".into());
        Ok(Self::new(name, info.sample_rate, info.channels, samples))
    }

    /// Set the block length in frames.
    pub fn block_frames(mut self, frames: usize) -> Self {
        self.block_frames = frames.max(1);
        self
    }

    /// Set the pacing.
    pub fn pacing(mut self, pacing: Pacing) -> Self {
        self.pacing = pacing;
        self
    }

    /// Set what happens at the end.
    pub fn at_end(mut self, end: SignalEnd) -> Self {
        self.end = end;
        self
    }
}

impl InputSource for SignalSource {
    fn open(&mut self) -> Result<InputFormat, InputError> {
        Ok(InputFormat {
            source: match self.pacing {
                Pacing::RealTime => "generated signal (real-time pacing)".into(),
                Pacing::AsFastAsPossible => "generated signal (as fast as read)".into(),
            },
            device_name: self.name.clone(),
            sample_rate: self.rate,
            channels: self.channels,
            sample_format: "f32".into(),
            max_block_frames: self.block_frames,
            declared_latency_ns: Some(0),
            notes: Vec::new(),
        })
    }

    fn start(&mut self, mut producer: CaptureProducer) -> Result<(), InputError> {
        let samples = self.samples.clone();
        let channels = usize::from(self.channels);
        let block = self.block_frames;
        let rate = i64::from(self.rate);
        let pacing = self.pacing;
        let end = self.end;
        let stop = self.stop.clone();
        let total_frames = samples.len() / channels;
        let handle = std::thread::Builder::new()
            .name("jk-input-signal".into())
            .spawn(move || {
                let status = producer.status();
                let t0 = host_time_ns();
                // Frames delivered since t0 (drives the time stamps), and the
                // read position in the samples.
                let mut delivered: u64 = 0;
                let mut pos: usize = 0;
                while !stop.load(Ordering::Acquire) {
                    if pos >= total_frames {
                        match end {
                            SignalEnd::Loops if total_frames > 0 => pos = 0,
                            SignalEnd::Unplugs => {
                                status.fail(
                                    FailureKind::Unplugged,
                                    "the input device was unplugged (simulated)",
                                );
                                return;
                            }
                            SignalEnd::GoesQuiet => return,
                            _ => {
                                status.end();
                                return;
                            }
                        }
                    }
                    let frames = block.min(total_frames - pos);
                    let host_ns = t0
                        + (delivered as i128 * i128::from(NS_PER_SECOND) / i128::from(rate)) as i64;
                    match pacing {
                        Pacing::RealTime => {
                            // A device hands over a block once its last frame
                            // has been captured.
                            let due = host_ns + (frames as i64 * NS_PER_SECOND) / rate;
                            let wait = due - host_time_ns();
                            if wait > 0 {
                                std::thread::sleep(Duration::from_nanos(wait as u64));
                            }
                        }
                        Pacing::AsFastAsPossible => {
                            while !producer.has_room(frames) {
                                if stop.load(Ordering::Acquire) {
                                    return;
                                }
                                producer.touch();
                                std::thread::sleep(Duration::from_millis(1));
                            }
                        }
                    }
                    let s = &samples[pos * channels..(pos + frames) * channels];
                    producer.push_interleaved(s, host_ns, 0);
                    delivered += frames as u64;
                    pos += frames;
                }
            })
            .map_err(|e| {
                InputError::new(
                    FailureKind::Device,
                    format!("could not start a thread: {e}"),
                )
            })?;
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

impl Drop for SignalSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::signal::{OtherChannel, VoiceTone, place_voice};
    use crate::input::{InputState, InputStream, extract_channel};
    use crate::wav::{WavSpec, WavWriter};

    /// A generated (not recorded) WAV goes through the same path as a
    /// microphone: format from the file, samples bit-exact, voice channel intact.
    #[test]
    fn a_wav_file_is_fed_through_the_input_path() {
        let rate = 44_100;
        let voice = VoiceTone::default_a3().render(rate, 0.5);
        let x = place_voice(&voice, rate, 2, 1, OtherChannel::Noise { rms_dbfs: -60.0 });
        let path =
            std::env::temp_dir().join(format!("jk-input-wav-test-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path, WavSpec::new(rate, 2)).unwrap();
        w.write_interleaved(&x).unwrap();
        w.finalize().unwrap();
        let src = SignalSource::from_wav(&path)
            .unwrap()
            .pacing(Pacing::AsFastAsPossible);
        let mut s = InputStream::open(Box::new(src)).unwrap();
        assert_eq!((s.format().sample_rate, s.format().channels), (rate, 2));
        let mut all = Vec::new();
        let mut buf = Vec::new();
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            match s.read(&mut buf) {
                Some(_) => all.extend_from_slice(&buf),
                None if s.state() == InputState::Ended => break,
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        let _ = std::fs::remove_file(&path);
        assert_eq!(all, x);
        let mut v = Vec::new();
        extract_channel(&all, 2, 1, &mut v);
        assert_eq!(v, voice);
    }
}

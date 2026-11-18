// SPDX-License-Identifier: GPL-3.0-or-later
//! Listening to a recorded song before making a karaoke song from it: plays part
//! of a 32-bit float WAV through the default output device, natively (the app's
//! web view never plays audio). Windows: a WASAPI shared-mode render stream at the
//! file's own format (Windows converts only if the device runs at another rate).
//! The samples are played as recorded: no gain, no fades.
//!
//! One preview at a time per [`Preview`]; dropping it stops the sound.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::RecorderError;

/// A preview that is playing (or has finished). Dropping it stops it.
#[derive(Debug)]
pub struct Preview {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Preview {
    /// Play `path` from `start_s` for at most `max_s` seconds.
    pub fn start(path: &Path, start_s: f64, max_s: f64) -> Result<Preview, RecorderError> {
        imp::start(path, start_s, max_s)
    }

    /// True once the sound has ended (or failed).
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Stop now.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.halt();
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::session::split::{FrameSource, WavSource};
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    pub(super) fn start(path: &Path, start_s: f64, max_s: f64) -> Result<Preview, RecorderError> {
        let mut src = WavSource::open(path)?;
        let info = src.info();
        let rate = info.sample_rate;
        let first = ((start_s.max(0.0) * f64::from(rate)) as u64).min(src.frames());
        let last = (first + (max_s.max(0.0) * f64::from(rate)) as u64).min(src.frames());
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let thread = {
            let (stop, done) = (Arc::clone(&stop), Arc::clone(&done));
            std::thread::Builder::new().name("jk-preview".into()).spawn(move || {
                let com = wasapi::initialize_mta();
                let r = (|| -> Result<(), String> {
                    let fail = |e: wasapi::WasapiError| e.to_string();
                    let dev = DeviceEnumerator::new()
                        .and_then(|en| en.get_default_device(Direction::Render))
                        .map_err(fail)?;
                    let mut client = dev.get_iaudioclient().map_err(fail)?;
                    let fmt = WaveFormat::new(32, 32, SampleType::Float, rate as usize, usize::from(info.channels), None);
                    client
                        .initialize_client(
                            &fmt,
                            Direction::Render,
                            StreamMode::PollingShared {
                                autoconvert: true,
                                buffer_duration_hns: 1_000_000,
                            },
                            None,
                        )
                        .map_err(fail)?;
                    let render = client.get_audiorenderclient().map_err(fail)?;
                    let block = 4 * usize::from(info.channels);
                    let mut pos = first;
                    let mut started = false;
                    let _ = ready_tx.send(Ok(()));
                    while !stop.load(Ordering::SeqCst) && pos < last {
                        let avail = u64::from(client.get_available_space_in_frames().map_err(fail)?);
                        let n = avail.min(last - pos);
                        if n > 0 {
                            let bytes = src.read_bytes(pos, n).map_err(|e| e.to_string())?;
                            render.write_to_device(&bytes[..n as usize * block], None).map_err(fail)?;
                            pos += n;
                        }
                        if !started {
                            client.start_stream().map_err(fail)?;
                            started = true;
                        }
                        std::thread::sleep(Duration::from_millis(15));
                    }
                    // Let what is queued play out, unless stopped.
                    while started && !stop.load(Ordering::SeqCst) && client.get_current_padding().unwrap_or(0) > 0 {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                    if started {
                        let _ = client.stop_stream();
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    let _ = ready_tx.send(Err(e));
                }
                done.store(true, Ordering::SeqCst);
                if com.is_ok() {
                    wasapi::deinitialize();
                }
            })?
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Preview {
                stop,
                done,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(RecorderError::Backend {
                    domain: "WASAPI".into(),
                    code: 0,
                    message: format!("could not play the preview: {e}"),
                })
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err(RecorderError::Timeout("starting the preview"))
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub(super) fn start(_path: &Path, _start_s: f64, _max_s: f64) -> Result<Preview, RecorderError> {
        Err(RecorderError::Unsupported(
            "listening to a recording inside the app works on Windows only for now".into(),
        ))
    }
}

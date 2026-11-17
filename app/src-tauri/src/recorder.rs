// SPDX-License-Identifier: GPL-3.0-or-later
//! The recorder (docs/APP_API.md section 8, WORK_ORDER 4.6), wrapping jk-recorder.
//!
//! - One recording at a time, into `<root>/Recordings/Recording <local time>.wav`
//!   (32-bit float WAV, RF64 above 4 GiB, exactly what jk-recorder writes: no
//!   resampling, no processing).
//! - A level thread polls the recorder about 20 times per second (it is the only
//!   poller, because polling resets the meter's peaks) and emits `recorder://level`;
//!   `recorder_status` returns its latest reading.
//! - macOS: the "Screen Recording" permission is asked only by
//!   `recorder_request_permission`, after the UI has shown
//!   `jk_recorder::PERMISSION_EXPLANATION` (exposed in `AppInfo`).
//! - What the list cannot read from the WAV header (whether it clipped) is kept in
//!   `<root>/Settings/recordings-index.json`.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use jk_recorder::{PermissionState, Recorder, RecorderConfig, RecorderError, SourceEncoding};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};

/// docs/APP_API.md `RecorderStatus.format`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FormatView {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: &'static str,
    pub note: String,
}

/// docs/APP_API.md `RecorderStatus`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecorderStatusView {
    pub state: &'static str,
    pub elapsed_s: f64,
    pub format: Option<FormatView>,
    pub peak_db: Vec<f32>,
    pub clipped: bool,
    pub dropped_s: f64,
    pub error: Option<String>,
}

impl RecorderStatusView {
    fn idle(error: Option<String>) -> Self {
        RecorderStatusView {
            state: if error.is_some() { "error" } else { "idle" },
            elapsed_s: 0.0,
            format: None,
            peak_db: Vec::new(),
            clipped: false,
            dropped_s: 0.0,
            error,
        }
    }
}

/// docs/APP_API.md `Recording`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recording {
    pub path: String,
    pub name: String,
    pub duration_s: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bytes: u64,
    pub clipped: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct IndexEntry {
    clipped: bool,
    #[serde(default)]
    peak_dbfs: Vec<f32>,
    #[serde(default)]
    format: String,
}

const INDEX_FILE: &str = "recordings-index.json";

fn sample_format(src: &SourceEncoding) -> &'static str {
    match src {
        SourceEncoding::Float32 { .. } => "f32",
        SourceEncoding::Int16 { .. } => "i16",
        SourceEncoding::Int32 { .. } => "i32",
    }
}

/// Plain-language message for a recorder error (WORK_ORDER 4.10: specific enough to
/// act on without a second round trip).
pub fn user_message(e: &RecorderError) -> (String, String) {
    let (code, msg) = match e {
        RecorderError::NotImplemented(what) => (
            "not_available",
            format!("Recording is not available in this build yet ({what})."),
        ),
        RecorderError::Unsupported(why) => (
            "not_available",
            format!("This computer cannot record the sound it plays: {why}"),
        ),
        RecorderError::PermissionDenied(_) => (
            "permission_denied",
            "Jukeoke does not have the macOS \"Screen Recording\" permission yet. Open System Settings > \
             Privacy & Security > Screen Recording, switch Jukeoke on, then quit and reopen Jukeoke."
                .to_string(),
        ),
        RecorderError::Backend { message, .. } => ("capture_failed", format!("Recording failed: {message}")),
        RecorderError::Timeout(what) => (
            "timeout",
            format!("The system did not start the audio capture in time ({what}). Try again; if it keeps happening, restart the computer's audio (or the computer)."),
        ),
        RecorderError::Format(why) => (
            "capture_failed",
            format!("The system delivered audio in a format Jukeoke cannot store without converting it: {why}"),
        ),
        RecorderError::Io(io) => ("io", format!("The recording could not be written: {io}")),
        RecorderError::AlreadyStopped => ("not_recording", "The recording has already stopped.".to_string()),
    };
    (code.to_string(), msg)
}

struct Active {
    recorder: Arc<Mutex<Option<Recorder>>>,
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

enum Phase {
    Idle,
    Starting,
    Recording(Active),
    Stopping,
}

pub struct RecorderCtl {
    dir: PathBuf,
    index_path: PathBuf,
    sink: Arc<dyn EventSink>,
    phase: Mutex<Phase>,
    last: Arc<Mutex<RecorderStatusView>>,
    asked_permission: AtomicBool,
}

impl RecorderCtl {
    pub fn new(recordings_dir: PathBuf, settings_dir: &Path, sink: Arc<dyn EventSink>) -> RecorderCtl {
        RecorderCtl {
            dir: recordings_dir,
            index_path: settings_dir.join(INDEX_FILE),
            sink,
            phase: Mutex::new(Phase::Idle),
            last: Arc::new(Mutex::new(RecorderStatusView::idle(None))),
            asked_permission: AtomicBool::new(false),
        }
    }

    /// `"granted" | "denied" | "not_determined" | "not_required"`.
    pub fn permission(&self) -> &'static str {
        match jk_recorder::permission_preflight() {
            PermissionState::Granted => "granted",
            PermissionState::NotRequired => "not_required",
            PermissionState::NotGranted if self.asked_permission.load(Ordering::SeqCst) => "denied",
            PermissionState::NotGranted => "not_determined",
        }
    }

    /// `recorder_request_permission`: shows the macOS prompt (the first time only;
    /// afterwards macOS just answers with the current state).
    pub fn request_permission(&self) -> &'static str {
        self.asked_permission.store(true, Ordering::SeqCst);
        match jk_recorder::permission_request() {
            PermissionState::Granted => "granted",
            PermissionState::NotRequired => "not_required",
            PermissionState::NotGranted => "denied",
        }
    }

    pub fn status(&self) -> RecorderStatusView {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_last(&self, v: RecorderStatusView) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = v.clone();
        emit(&*self.sink, names::RECORDER_LEVEL, &v);
    }

    /// `recorder_start`. Blocks until the OS has started the stream (at most 20 s).
    pub fn start(&self) -> ApiResult<RecorderStatusView> {
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*phase, Phase::Idle) {
                return Err(ApiError::new("busy", "A recording is already running."));
            }
            *phase = Phase::Starting;
        }
        let mut starting = RecorderStatusView::idle(None);
        starting.state = "starting";
        self.set_last(starting);

        let path = match self.new_path() {
            Ok(p) => p,
            Err(e) => {
                *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
                let err = ApiError::io("the Recordings folder", &e);
                self.set_last(RecorderStatusView::idle(Some(err.message.clone())));
                return Err(err);
            }
        };
        crate::applog::info(&format!("recorder: starting into {}", path.display()));
        match Recorder::start(RecorderConfig::new(&path)) {
            Ok(rec) => {
                // The first reading, so the answer already says "recording".
                self.set_last(view_of(&rec.status()));
                let recorder = Arc::new(Mutex::new(Some(rec)));
                let stop = Arc::new(AtomicBool::new(false));
                let spawned = {
                    let (recorder, stop, last, sink) =
                        (Arc::clone(&recorder), Arc::clone(&stop), Arc::clone(&self.last), Arc::clone(&self.sink));
                    std::thread::Builder::new()
                        .name("recorder-level".into())
                        .spawn(move || level_loop(&recorder, &stop, &last, &*sink))
                };
                let thread = match spawned {
                    Ok(t) => t,
                    Err(e) => {
                        // Close the file properly before giving up.
                        if let Some(r) = recorder.lock().unwrap_or_else(|e| e.into_inner()).take() {
                            let _ = r.stop();
                        }
                        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
                        return Err(ApiError::internal(e));
                    }
                };
                *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Recording(Active {
                    recorder,
                    path,
                    stop,
                    thread,
                });
                Ok(self.status())
            }
            Err(e) => {
                crate::applog::warn(&format!("recorder: start failed: {e}"));
                *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
                let (code, message) = user_message(&e);
                self.set_last(RecorderStatusView::idle(Some(message.clone())));
                Err(ApiError::new(code, message))
            }
        }
    }

    fn new_path(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let stem = format!("Recording {}", crate::sysinfo::local_stamp());
        Ok(crate::export::unique_path(&self.dir, &stem, "wav", &Default::default()))
    }

    /// `recorder_stop`: closes the file and returns what was recorded.
    pub fn stop(&self) -> ApiResult<Recording> {
        let active = {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            match std::mem::replace(&mut *phase, Phase::Stopping) {
                Phase::Recording(a) => a,
                other => {
                    *phase = other;
                    return Err(ApiError::new("not_recording", "Nothing is being recorded."));
                }
            }
        };
        active.stop.store(true, Ordering::SeqCst);
        let _ = active.thread.join();
        let rec = active.recorder.lock().unwrap_or_else(|e| e.into_inner()).take();
        let result = match rec {
            Some(r) => r.stop(),
            None => Err(RecorderError::AlreadyStopped),
        };
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
        let finished = match result {
            Ok(f) => f,
            Err(e) => {
                let (code, message) = user_message(&e);
                self.set_last(RecorderStatusView::idle(Some(message.clone())));
                return Err(ApiError::new(code, message));
            }
        };
        crate::applog::info(&format!("recorder: stopped\n{}", finished.report()));
        let Some(file) = &finished.file else {
            let msg = "Nothing was recorded: no sound arrived from the system. Play something, then record again.";
            self.set_last(RecorderStatusView::idle(Some(msg.into())));
            return Err(ApiError::new("no_audio", msg));
        };
        let s = &finished.stats;
        let fmt = s.format.clone();
        let rate = fmt.as_ref().map_or(0, |f| f.sample_rate);
        let recording = Recording {
            path: active.path.display().to_string(),
            name: file_name(&active.path),
            duration_s: if rate > 0 { file.frames as f64 / rate as f64 } else { 0.0 },
            sample_rate: rate,
            channels: fmt.as_ref().map_or(0, |f| f.channels),
            bytes: file.file_bytes,
            clipped: s.clip_warning(),
        };
        self.remember(
            &recording.name,
            IndexEntry {
                clipped: recording.clipped,
                peak_dbfs: finished.levels.peak_total_dbfs(),
                format: fmt.map(|f| f.to_string()).unwrap_or_default(),
            },
        );
        self.set_last(RecorderStatusView::idle(None));
        Ok(recording)
    }

    fn load_index(&self) -> BTreeMap<String, IndexEntry> {
        std::fs::read_to_string(&self.index_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn remember(&self, name: &str, entry: IndexEntry) {
        let mut idx = self.load_index();
        idx.insert(name.to_string(), entry);
        // Forget files that are gone, so the index never grows without bound.
        idx.retain(|n, _| self.dir.join(n).exists());
        if let Ok(text) = serde_json::to_string_pretty(&idx)
            && let Err(e) = crate::fs_atomic::write_atomic(&self.index_path, text.as_bytes())
        {
            crate::applog::warn(&format!("recordings index: {e}"));
        }
    }

    /// `recordings_list`: the WAV files in the Recordings folder, newest first.
    pub fn list(&self) -> ApiResult<Vec<Recording>> {
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ApiError::io("the Recordings folder", &e)),
        };
        let recording_now = match &*self.phase.lock().unwrap_or_else(|e| e.into_inner()) {
            Phase::Recording(a) => Some(a.path.clone()),
            _ => None,
        };
        let idx = self.load_index();
        let mut out: Vec<(u64, Recording)> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            let name = file_name(&path);
            if name.starts_with('.') || name.contains(".tmp-") || Some(&path) == recording_now.as_ref() {
                continue;
            }
            if !name.to_ascii_lowercase().ends_with(".wav") {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Some(h) = read_wav_header(&path) else { continue };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            out.push((
                modified,
                Recording {
                    path: path.display().to_string(),
                    clipped: idx.get(&name).is_some_and(|i| i.clipped),
                    name,
                    duration_s: h.duration_s(),
                    sample_rate: h.sample_rate,
                    channels: h.channels,
                    bytes: meta.len(),
                },
            ));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        Ok(out.into_iter().map(|(_, r)| r).collect())
    }

    /// Stops a running recording (on exit) so the file is closed properly.
    pub fn shutdown(&self) {
        if matches!(*self.phase.lock().unwrap_or_else(|e| e.into_inner()), Phase::Recording(_)) {
            let _ = self.stop();
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn level_loop(
    recorder: &Mutex<Option<Recorder>>,
    stop: &AtomicBool,
    last: &Mutex<RecorderStatusView>,
    sink: &dyn EventSink,
) {
    while !stop.load(Ordering::SeqCst) {
        let view = {
            let guard = recorder.lock().unwrap_or_else(|e| e.into_inner());
            let Some(rec) = guard.as_ref() else { break };
            view_of(&rec.status())
        };
        *last.lock().unwrap_or_else(|e| e.into_inner()) = view.clone();
        emit(sink, names::RECORDER_LEVEL, &view);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn view_of(st: &jk_recorder::RecorderStatus) -> RecorderStatusView {
    let s = &st.stats;
    let (state, error) = match &st.state {
        jk_recorder::RecorderState::WaitingForAudio | jk_recorder::RecorderState::Recording => ("recording", None),
        jk_recorder::RecorderState::Failed(m) => ("error", Some(format!("Recording stopped: {m}"))),
    };
    let rate = s.format.as_ref().map_or(0, |f| f.sample_rate);
    let mut notes = vec![format!("Captured with {}.", st.setup.backend)];
    if let Some(f) = &s.format {
        notes.push(format!("{f}."));
    } else {
        notes.push("Waiting for the system to report its audio format; silence before the first sound is kept in the recording.".into());
    }
    notes.extend(st.setup.notes.iter().cloned());
    RecorderStatusView {
        state,
        // Wall time since Start, not the media clock: the recorder fills silence from
        // the start()/stop() instants so the file ends up exactly this long (recorder
        // stream: measured equal within 1 ms), while the media clock can stand still
        // during silence when Windows delivers no packets (keep-alive not running).
        elapsed_s: s.wall_elapsed.as_secs_f64(),
        format: s.format.as_ref().map(|f| FormatView {
            sample_rate: f.sample_rate,
            channels: f.channels,
            sample_format: sample_format(&f.source),
            note: notes.join(" "),
        }),
        peak_db: st.levels.peak_dbfs(),
        clipped: s.clip_warning(),
        dropped_s: if rate > 0 { s.dropped_frames as f64 / rate as f64 } else { 0.0 },
        error: error.or_else(|| s.error.clone()),
    }
}

/// What the list needs from a WAV / RF64 header.
#[derive(Clone, Debug, PartialEq)]
pub struct WavHeader {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub data_bytes: u64,
}

impl WavHeader {
    pub fn duration_s(&self) -> f64 {
        let frame = self.channels as u64 * (self.bits as u64 / 8);
        if frame == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        (self.data_bytes / frame) as f64 / self.sample_rate as f64
    }
}

/// Reads `fmt ` and the data size of a RIFF/WAVE or RF64 file. For RF64 the 64-bit
/// data size comes from the `ds64` chunk. A data size larger than the file (a
/// recording cut short by a pulled stick) is clamped to what is there.
pub fn read_wav_header(path: &Path) -> Option<WavHeader> {
    let mut f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if &head[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut ds64_data: Option<u64> = None;
    let mut pos = 12u64;
    for _ in 0..64 {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut ch = [0u8; 8];
        if f.read_exact(&mut ch).is_err() {
            break;
        }
        let id = [ch[0], ch[1], ch[2], ch[3]];
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        let body = pos + 8;
        match &id {
            b"ds64" => {
                let mut b = [0u8; 24];
                f.read_exact(&mut b).ok()?;
                ds64_data = Some(u64::from_le_bytes(b[8..16].try_into().ok()?));
            }
            b"fmt " => {
                let mut b = [0u8; 16];
                f.read_exact(&mut b).ok()?;
                channels = u16::from_le_bytes([b[2], b[3]]);
                rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                bits = u16::from_le_bytes([b[14], b[15]]);
            }
            b"data" => {
                let declared = if rf64 && size == 0xFFFF_FFFF {
                    ds64_data.unwrap_or(0)
                } else {
                    size
                };
                let available = file_len.saturating_sub(body);
                if rate == 0 || channels == 0 {
                    return None;
                }
                return Some(WavHeader {
                    sample_rate: rate,
                    channels,
                    bits,
                    data_bytes: declared.min(available),
                });
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, ch: u16, frames: u32, rf64: bool) -> Vec<u8> {
        let data = frames * ch as u32 * 4;
        let mut v = Vec::new();
        v.extend_from_slice(if rf64 { b"RF64" } else { b"RIFF" });
        v.extend_from_slice(&(if rf64 { u32::MAX } else { 4 + 24 + 8 + data }).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        if rf64 {
            v.extend_from_slice(b"ds64");
            v.extend_from_slice(&28u32.to_le_bytes());
            v.extend_from_slice(&0u64.to_le_bytes());
            v.extend_from_slice(&(data as u64).to_le_bytes());
            v.extend_from_slice(&(frames as u64).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
        }
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes());
        v.extend_from_slice(&ch.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * ch as u32 * 4).to_le_bytes());
        v.extend_from_slice(&(ch * 4).to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(if rf64 { u32::MAX } else { data }).to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data as usize));
        v
    }

    #[test]
    fn wav_and_rf64_headers() {
        let dir = std::env::temp_dir().join(format!(
            "jukeoke-wav-test-{}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        std::fs::write(&a, wav(48000, 2, 24000, false)).unwrap();
        let h = read_wav_header(&a).unwrap();
        assert_eq!((h.sample_rate, h.channels, h.bits), (48000, 2, 32));
        assert!((h.duration_s() - 0.5).abs() < 1e-9);
        let b = dir.join("b.wav");
        std::fs::write(&b, wav(44100, 1, 44100, true)).unwrap();
        assert!((read_wav_header(&b).unwrap().duration_s() - 1.0).abs() < 1e-9);
        // Cut short: the length is what is really there.
        let mut cut = wav(48000, 2, 48000, false);
        cut.truncate(cut.len() - 48000 * 4);
        std::fs::write(&a, cut).unwrap();
        assert!((read_wav_header(&a).unwrap().duration_s() - 0.5).abs() < 1e-9);
        std::fs::write(&a, b"not a wav file at all").unwrap();
        assert!(read_wav_header(&a).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

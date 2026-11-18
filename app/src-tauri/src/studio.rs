// SPDX-License-Identifier: GPL-3.0-or-later
//! The Studio supervisor (docs/STUDIO_PROTOCOL.md, docs/APP_API.md section 9).
//!
//! The Studio is a separate Python process (Windows only) that turns any song into a
//! `.jksong`. This module starts it on demand, speaks the JSON-lines protocol over
//! its stdin/stdout, appends its stderr to `<root>/Settings/logs/studio.log`, and
//! keeps the app's own copy of the job queue so that:
//!
//! - every change of a job is mirrored to the UI as a `studio://job` event;
//! - a crash never loses the queue: the running job fails with `studio_crashed`, the
//!   Studio is started again and the jobs that were still waiting are sent again
//!   (at most [`MAX_CRASHES`] crashes in [`CRASH_WINDOW`], then the waiting jobs fail
//!   with a plain explanation instead of looping);
//! - a Studio that does not answer is a timeout with a plain message, never a hang
//!   (60 s for `hello` and the first `probe`, which import PyTorch).
//!
//! Capability (APP_API section 1): unavailable on macOS, when the Studio runtime is
//! missing next to the app, or when the probe says this computer cannot run it. The
//! probe runs once in the background after start-up; while it runs the status says
//! `checking: true` (an addition to the contract). When the Studio has been idle for
//! [`IDLE_SHUTDOWN`] it is stopped to give its memory back; the next request starts
//! it again.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};

/// Protocol version this app speaks (docs/STUDIO_PROTOCOL.md).
pub const PROTOCOL: u64 = 1;
/// The Studio's default quality preset (STUDIO_PROTOCOL `hello.ok` `default_preset`):
/// `StudioStatus.models_present` is about this preset's models.
pub const DEFAULT_PRESET: &str = "full";
/// Crashes tolerated within [`CRASH_WINDOW`] before automatic restarts stop.
pub const MAX_CRASHES: usize = 3;
pub const CRASH_WINDOW: Duration = Duration::from_secs(600);
/// Stop the Studio after this long with nothing to do.
pub const IDLE_SHUTDOWN: Duration = Duration::from_secs(600);
/// studio.log is rotated (one old copy kept) above this size.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// How to start the Studio.
#[derive(Clone, Debug)]
pub struct StudioConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub root: PathBuf,
    pub models_dir: PathBuf,
    pub library_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub log_path: PathBuf,
    pub app_version: String,
    /// Time allowed for `hello` and for the first `probe` (PyTorch import).
    pub start_timeout: Duration,
    /// Default inactivity timeout for requests (reset by every message for the
    /// request, such as download progress).
    pub request_timeout: Duration,
    pub idle_shutdown: Duration,
}

impl StudioConfig {
    /// The production / developer layout (STUDIO_PROTOCOL section 1). `None` when no
    /// Studio runtime is present, with the plain-language reason.
    ///
    /// Every variant gets the same portable environment ([`studio_env`]) and the
    /// arguments `serve --root <root> --models-dir <models> --temp-dir <tmp>`, so the
    /// Studio's caches (Hugging Face, PyTorch, CUDA, numba, matplotlib, temporary files)
    /// stay under `<root>/Settings` and its weights under `<root>/Models`; nothing goes
    /// to per-user folders (WORKING ORDER 4.8). The Studio sets the same values itself
    /// from `--root` before it imports any library (studio/jukeoke_studio/env.py); the
    /// environment here also covers anything it starts.
    pub fn locate(
        exe_dir: Option<&Path>,
        dev_mode: bool,
        paths: &crate::portable::Paths,
        root: &Path,
        app_version: &str,
    ) -> Result<StudioConfig, String> {
        if cfg!(not(windows)) {
            return Err("Songs are created in the Studio on a Windows PC with an NVIDIA graphics card. This computer plays them.".into());
        }
        let mut env: Vec<(OsString, OsString)> = Vec::new();
        let mut models_dir = paths.models.clone();
        let (program, mut args, cwd) = if let Some(py) = std::env::var_os("JK_STUDIO_PYTHON") {
            // Developer / test override, e.g. the fake Studio:
            // JK_STUDIO_PYTHON=python JK_STUDIO_SCRIPT=app/src-tauri/tests/fake_studio.py
            let mut args: Vec<OsString> = vec!["-X".into(), "utf8".into()];
            match std::env::var_os("JK_STUDIO_SCRIPT") {
                Some(script) => args.push(script),
                None => args.extend(["-m".into(), "jukeoke_studio".into()]),
            }
            args.push("serve".into());
            (PathBuf::from(py), args, paths.tmp.clone())
        } else {
            let packaged = exe_dir.map(|d| d.join("Studio"));
            let packaged_py = packaged.as_ref().map(|d| d.join("python").join("python.exe"));
            match packaged_py.filter(|p| p.is_file()) {
                Some(py) => (
                    py,
                    // -I: isolated from any Python on the computer; -X utf8: stdin
                    // and stdout are UTF-8 whatever the Windows code page (paths and
                    // song titles with non-ASCII letters).
                    vec!["-I".into(), "-X".into(), "utf8".into(), "-m".into(), "jukeoke_studio".into(), "serve".into()],
                    packaged.expect("packaged dir"),
                ),
                None if dev_mode => {
                    // Developer machine: the Studio venv and the source tree. No -I
                    // here: it would ignore PYTHONPATH. The weights already downloaded
                    // for development live in $JK_BUILD/models-cache (Hugging Face
                    // models in its hf/ folder): use them instead of an empty
                    // dev-data/Models, so a dev run never downloads gigabytes again.
                    let jk_build = std::env::var_os(crate::portable::DEV_ENV_VAR).map(PathBuf::from);
                    let Some(source_dir) = crate::portable::SOURCE_DIR else {
                        return Err(missing_runtime());
                    };
                    let repo_studio = Path::new(source_dir).join("../../studio");
                    if let Some(cache) = jk_build.as_ref().map(|b| b.join("models-cache")).filter(|d| d.is_dir()) {
                        models_dir = cache;
                    }
                    let py = jk_build.map(|b| b.join("studio-venv").join("Scripts").join("python.exe"));
                    match py.filter(|p| p.is_file()) {
                        Some(py) if repo_studio.is_dir() => {
                            env.push(("PYTHONPATH".into(), repo_studio.clone().into_os_string()));
                            env.push(("PYTHONUTF8".into(), "1".into()));
                            (py, vec!["-m".into(), "jukeoke_studio".into(), "serve".into()], repo_studio)
                        }
                        _ => return Err(missing_runtime()),
                    }
                }
                None => return Err(missing_runtime()),
            }
        };
        let temp_dir = paths.tmp.join("studio");
        args.extend([
            "--root".into(),
            root.as_os_str().to_owned(),
            "--models-dir".into(),
            models_dir.as_os_str().to_owned(),
            "--temp-dir".into(),
            temp_dir.as_os_str().to_owned(),
        ]);
        env.extend(studio_env(root, &paths.settings, &models_dir, &temp_dir));
        Ok(StudioConfig {
            program,
            args,
            cwd,
            env,
            root: root.to_path_buf(),
            models_dir,
            library_dir: paths.library.clone(),
            temp_dir,
            log_path: paths.logs.join("studio.log"),
            app_version: app_version.to_string(),
            start_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(120),
            idle_shutdown: IDLE_SHUTDOWN,
        })
    }
}

/// The Studio's environment (WORKING ORDER 4.8: in portable mode nothing is written to
/// per-user folders). Caches go under `<settings>/cache/<name>`, temporary files to
/// `temp_dir` (`<settings>/tmp/studio`), Hugging Face weights to `<models>/hf`; the
/// Hugging Face libraries are kept offline (the Studio downloads only on the
/// `models.download` request, with its own verified downloader). Same names and
/// folders as studio/jukeoke_studio/env.py.
pub fn studio_env(root: &Path, settings: &Path, models_dir: &Path, temp_dir: &Path) -> Vec<(OsString, OsString)> {
    let cache = settings.join("cache");
    let mut env: Vec<(OsString, OsString)> = vec![
        ("JUKEOKE_ROOT".into(), root.as_os_str().to_owned()),
        ("JUKEOKE_MODELS_DIR".into(), models_dir.as_os_str().to_owned()),
    ];
    for (var, sub) in [
        ("HF_HOME", "huggingface"),
        ("TORCH_HOME", "torch"),
        ("XDG_CACHE_HOME", "xdg"),
        ("CUDA_CACHE_PATH", "cuda"),
        ("NUMBA_CACHE_DIR", "numba"),
        ("MPLCONFIGDIR", "matplotlib"),
        ("PYTORCH_KERNEL_CACHE_PATH", "torch/kernels"),
        ("TRITON_CACHE_DIR", "triton"),
        ("TORCHINDUCTOR_CACHE_DIR", "inductor"),
    ] {
        env.push((var.into(), cache.join(sub).into_os_string()));
    }
    let hf = models_dir.join("hf");
    for var in ["HF_HUB_CACHE", "TRANSFORMERS_CACHE", "HUGGINGFACE_HUB_CACHE"] {
        env.push((var.into(), hf.as_os_str().to_owned()));
    }
    for var in ["TMP", "TEMP", "TMPDIR"] {
        env.push((var.into(), temp_dir.as_os_str().to_owned()));
    }
    for (var, val) in [
        ("HF_HUB_OFFLINE", "1"),
        ("TRANSFORMERS_OFFLINE", "1"),
        ("HF_HUB_DISABLE_TELEMETRY", "1"),
        ("DO_NOT_TRACK", "1"),
        ("PYTHONDONTWRITEBYTECODE", "1"),
    ] {
        env.push((var.into(), val.into()));
    }
    env
}

fn missing_runtime() -> String {
    "The Studio engine is not on this drive (the Windows/Studio folder is missing), so songs cannot be created here. Songs are created on a stronger computer and can be played anywhere.".into()
}

/// docs/APP_API.md `StudioJob`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StudioJob {
    pub job_id: String,
    pub input_name: String,
    pub state: &'static str,
    pub stage: Option<String>,
    pub overall: f64,
    /// Addition: progress of the current stage, 0..1 (STUDIO_PROTOCOL `stage_progress`).
    pub stage_progress: Option<f64>,
    pub message: Option<String>,
    pub output_song_id: Option<String>,
    pub error: Option<String>,
    /// Addition: the error code (`studio_crashed`, `decode_failed`, ...).
    pub error_code: Option<String>,
}

/// docs/APP_API.md `studio_status()` (plus `checking`, an addition).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StudioStatus {
    pub available: bool,
    pub reasons: Vec<String>,
    pub running: bool,
    pub models_present: bool,
    pub queue: Vec<StudioJob>,
    pub checking: bool,
}

/// `AppInfo.capabilities.studio` (plus `checking`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StudioCapability {
    pub available: bool,
    pub reasons: Vec<String>,
    pub gpu: Option<String>,
    pub vram_mb: Option<u64>,
    pub checking: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Avail {
    /// Not probed yet (the background probe has not finished).
    Checking,
    Yes { gpu: Option<String>, vram_mb: Option<u64> },
    No(Vec<String>),
}

struct Job {
    view: StudioJob,
    /// The `job` object for `job.submit` (resent after a crash).
    spec: Value,
    /// Sent to the current Studio process.
    sent: bool,
    /// The user cancelled it; if the Studio did not know it yet, the dispatcher
    /// cancels it as soon as the Studio has accepted it.
    cancel_requested: bool,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    generation: u64,
}

struct State {
    proc: Option<Proc>,
    generation: u64,
    avail: Avail,
    models_present: bool,
    jobs: Vec<Job>,
    /// Requests waiting for an answer: id -> (process generation, channel).
    pending: HashMap<String, (u64, crossbeam_channel::Sender<Value>)>,
    crashes: Vec<Instant>,
    last_activity: Instant,
    /// We asked this process generation to stop: its exit is not a crash.
    stopping: Option<u64>,
    /// The app is closing: never start the Studio again.
    closing: bool,
    /// Plain reason for the last start failure (shown to the user).
    start_error: Option<String>,
}

pub struct Studio {
    cfg: Option<StudioConfig>,
    sink: Arc<dyn EventSink>,
    state: Mutex<State>,
    /// Wakes the dispatcher (new job, crash, shutdown).
    wake: Condvar,
    /// Held while a process is being started (spawn + hello), never while waiting for
    /// anything else, so the reader thread is never blocked by it.
    start_lock: Mutex<()>,
    ids: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unavailable(reasons: &[String]) -> ApiError {
    let msg = if reasons.is_empty() {
        "The Studio is not available on this computer.".to_string()
    } else {
        reasons.join(" ")
    };
    ApiError::new("studio_unavailable", msg)
}

fn file_name(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// The sidecar of a song recorded with "Record songs separately" (`<stem>.json`
/// next to the WAV, src/recsession.rs), with the audio path.
fn recorded_song(path: &Path) -> Option<(jk_recorder::session::SongSidecar, PathBuf)> {
    jk_recorder::session::SongSidecar::for_audio(path).map(|sc| (sc, path.to_path_buf()))
}

/// `studio_inspect` of a recorded song: a WAV has no tags, so the title, artist,
/// album and cover the player reported (its sidecar) fill the fields the file left
/// empty, and `recorded` (an addition) says how complete the recording is.
pub fn merge_recorded_song(result: &mut Value, path: &Path) {
    let Some((sc, audio)) = recorded_song(path) else { return };
    let Some(o) = result.as_object_mut() else { return };
    let empty = |v: Option<&Value>| v.is_none_or(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()));
    // The Studio's inspect falls back to the file name for the title of an untagged
    // file (`title_from_tags: false`, docs/STUDIO_PROTOCOL.md section 5): for a recorded
    // song that is "01 Artist - Title", so the sidecar's title replaces it.
    let title_from_file_name = o.get("title_from_tags").and_then(Value::as_bool) == Some(false);
    for (key, value) in [("title", &sc.title), ("artist", &sc.artist), ("album", &sc.album)] {
        let replaceable = empty(o.get(key)) || (key == "title" && title_from_file_name);
        if replaceable && !value.trim().is_empty() {
            o.insert(key.into(), json!(value));
        }
    }
    if empty(o.get("cover_path"))
        && let Some(c) = sc.cover_path(&audio)
    {
        o.insert("cover_path".into(), json!(c.display().to_string()));
    }
    o.insert(
        "recorded".into(),
        json!({
            "status": match sc.status {
                jk_recorder::session::SongStatus::Complete => "complete",
                jk_recorder::session::SongStatus::Partial => "partial",
                jk_recorder::session::SongStatus::Unknown => "unknown",
            },
            "partial_reason": sc.partial_reason,
            "track_duration_s": sc.track_duration_s,
            "source_player": sc.source_player.app_name,
        }),
    );
}

impl Studio {
    /// `cfg: Err(reason)` = no Studio on this computer (macOS, runtime missing).
    pub fn new(cfg: Result<StudioConfig, String>, sink: Arc<dyn EventSink>) -> Arc<Studio> {
        let (cfg, avail) = match cfg {
            Ok(c) => (Some(c), Avail::Checking),
            Err(reason) => (None, Avail::No(vec![reason])),
        };
        let s = Arc::new(Studio {
            cfg,
            sink,
            state: Mutex::new(State {
                proc: None,
                generation: 0,
                avail,
                models_present: false,
                jobs: Vec::new(),
                pending: HashMap::new(),
                crashes: Vec::new(),
                last_activity: Instant::now(),
                stopping: None,
                closing: false,
                start_error: None,
            }),
            wake: Condvar::new(),
            start_lock: Mutex::new(()),
            ids: AtomicU64::new(0),
        });
        if s.cfg.is_some() {
            let me = Arc::clone(&s);
            let _ = std::thread::Builder::new()
                .name("studio-dispatch".into())
                .spawn(move || me.dispatch_loop());
        }
        s
    }

    /// Runs the capability probe (blocking; the app calls it on a background thread).
    pub fn probe_now(self: &Arc<Self>) {
        let Some(cfg) = self.cfg.clone() else { return };
        let result = self
            .ensure_started()
            .and_then(|()| self.request(json!({"type": "probe"}), cfg.start_timeout));
        let avail = match result {
            Ok(r) => {
                let ok = r.get("ok_for_studio").and_then(Value::as_bool).unwrap_or(false);
                let reasons: Vec<String> = r
                    .get("reasons")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if ok {
                    Avail::Yes {
                        gpu: r.get("gpu").and_then(Value::as_str).map(str::to_string),
                        vram_mb: r.get("vram_mb").and_then(Value::as_u64),
                    }
                } else if reasons.is_empty() {
                    Avail::No(vec!["This computer cannot run the Studio. Songs are created on a stronger computer and can be played anywhere.".into()])
                } else {
                    Avail::No(reasons)
                }
            }
            Err(e) => {
                crate::applog::warn(&format!("studio probe failed: {e}"));
                Avail::No(vec![e.message])
            }
        };
        crate::applog::info(&format!("studio capability: {avail:?}"));
        let ok = matches!(avail, Avail::Yes { .. });
        lock(&self.state).avail = avail;
        if ok {
            match self.request(json!({"type": "models.status", "preset": DEFAULT_PRESET}), cfg.request_timeout) {
                Ok(r) => {
                    lock(&self.state).models_present =
                        r.get("all_present").and_then(Value::as_bool).unwrap_or(false);
                }
                Err(e) => crate::applog::warn(&format!("studio models.status: {e}")),
            }
        } else {
            self.stop_process_locked(Duration::from_secs(5));
        }
        self.emit_status();
        self.wake.notify_all();
    }

    pub fn capability(&self) -> StudioCapability {
        let st = lock(&self.state);
        match &st.avail {
            Avail::Checking => StudioCapability {
                available: false,
                reasons: vec!["Still checking whether this computer can run the Studio...".into()],
                gpu: None,
                vram_mb: None,
                checking: true,
            },
            Avail::Yes { gpu, vram_mb } => StudioCapability {
                available: true,
                reasons: Vec::new(),
                gpu: gpu.clone(),
                vram_mb: *vram_mb,
                checking: false,
            },
            Avail::No(r) => StudioCapability {
                available: false,
                reasons: r.clone(),
                gpu: None,
                vram_mb: None,
                checking: false,
            },
        }
    }

    pub fn status(&self) -> StudioStatus {
        let cap = self.capability();
        let st = lock(&self.state);
        StudioStatus {
            available: cap.available,
            reasons: cap.reasons,
            running: st.proc.is_some(),
            models_present: st.models_present,
            queue: st.jobs.iter().map(|j| j.view.clone()).collect(),
            checking: cap.checking,
        }
    }

    fn emit_status(&self) {
        emit(&*self.sink, names::STUDIO_STATUS, &self.status());
    }

    fn emit_job(&self, j: &StudioJob) {
        emit(&*self.sink, names::STUDIO_JOB, j);
    }

    fn require_available(&self) -> ApiResult<&StudioConfig> {
        let cfg = self
            .cfg
            .as_ref()
            .ok_or_else(|| unavailable(&self.capability().reasons))?;
        if let Avail::No(r) = &lock(&self.state).avail {
            return Err(unavailable(r));
        }
        Ok(cfg)
    }

    // ------------------------------------------------------------------ process

    /// Starts the Studio if it is not running: spawn, then `hello` (with the start
    /// timeout). Errors are plain language.
    fn ensure_started(self: &Arc<Self>) -> ApiResult<()> {
        let cfg = self
            .cfg
            .as_ref()
            .ok_or_else(|| unavailable(&self.capability().reasons))?;
        let _start = lock(&self.start_lock);
        if lock(&self.state).proc.is_some() {
            return Ok(());
        }
        let generation = {
            let mut st = lock(&self.state);
            if st.closing {
                return Err(ApiError::new("studio_unavailable", "The app is closing."));
            }
            st.generation += 1;
            st.generation
        };
        let _ = std::fs::create_dir_all(&cfg.temp_dir);
        if let Some(dir) = cfg.log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut cmd = Command::new(&cfg.program);
        cmd.args(&cfg.args)
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: the app is a GUI program; without this flag Windows
            // would open a console window for the Python process.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| {
            crate::applog::error(&format!("studio: cannot start {}: {e}", cfg.program.display()));
            ApiError::new(
                "studio_unavailable",
                "The Studio engine could not be started. Details are in Settings/logs/app.log.",
            )
        })?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        crate::applog::info(&format!("studio: started pid {pid} ({})", cfg.program.display()));
        {
            let log_path = cfg.log_path.clone();
            let _ = std::thread::Builder::new()
                .name("studio-stderr".into())
                .spawn(move || copy_stderr(stderr, &log_path, pid));
        }
        {
            let me = Arc::clone(self);
            let _ = std::thread::Builder::new()
                .name("studio-stdout".into())
                .spawn(move || me.read_loop(stdout, generation));
        }
        {
            let mut st = lock(&self.state);
            st.proc = Some(Proc {
                child,
                stdin,
                generation,
            });
            st.last_activity = Instant::now();
        }
        let hello = json!({
            "type": "hello",
            "protocol": PROTOCOL,
            "app_version": cfg.app_version,
            "root": cfg.root,
            "models_dir": cfg.models_dir,
            "library_dir": cfg.library_dir,
            "temp_dir": cfg.temp_dir,
        });
        match self.request(hello, cfg.start_timeout) {
            Ok(r) => {
                let proto = r.get("protocol").and_then(Value::as_u64);
                if proto != Some(PROTOCOL) {
                    self.stop_process(Duration::from_secs(2));
                    let reason = format!(
                        "The Studio engine on this drive speaks a different protocol version ({}) from this app ({PROTOCOL}). Copy matching versions of the app and the Studio.",
                        proto.map_or("unknown".into(), |p| p.to_string())
                    );
                    lock(&self.state).avail = Avail::No(vec![reason.clone()]);
                    return Err(ApiError::new("protocol_mismatch", reason));
                }
                crate::applog::info(&format!("studio: hello.ok {r}"));
                lock(&self.state).start_error = None;
                self.emit_status();
                Ok(())
            }
            Err(e) => {
                self.stop_process(Duration::from_secs(2));
                let e = if e.code == "protocol_mismatch" {
                    lock(&self.state).avail = Avail::No(vec![e.message.clone()]);
                    e
                } else if e.code == "timeout" {
                    ApiError::new(
                        "studio_unavailable",
                        format!(
                            "The Studio engine did not answer within {} seconds after starting. Details are in Settings/logs/studio.log.",
                            cfg.start_timeout.as_secs()
                        ),
                    )
                } else {
                    ApiError::new(
                        "studio_unavailable",
                        "The Studio engine stopped while starting. Details are in Settings/logs/studio.log.",
                    )
                };
                lock(&self.state).start_error = Some(e.message.clone());
                Err(e)
            }
        }
    }

    fn write_line(&self, msg: &Value) -> ApiResult<()> {
        let mut st = lock(&self.state);
        let proc = st.proc.as_mut().ok_or_else(|| {
            ApiError::new("studio_crashed", "The Studio engine is not running.")
        })?;
        let mut line = serde_json::to_string(msg).map_err(ApiError::internal)?;
        line.push('\n');
        proc.stdin
            .write_all(line.as_bytes())
            .and_then(|()| proc.stdin.flush())
            .map_err(|e| {
                crate::applog::warn(&format!("studio: write failed: {e}"));
                ApiError::new("studio_crashed", "The Studio engine stopped unexpectedly.")
            })?;
        st.last_activity = Instant::now();
        Ok(())
    }

    /// Sends a request and waits for its final answer. `inactivity` is the longest
    /// silence allowed: every message about this request (progress) resets it.
    pub fn request(&self, mut msg: Value, inactivity: Duration) -> ApiResult<Value> {
        let id = format!("{}", self.ids.fetch_add(1, Ordering::SeqCst) + 1);
        msg["id"] = Value::String(id.clone());
        let (tx, rx) = crossbeam_channel::unbounded();
        {
            let mut st = lock(&self.state);
            let generation = st.proc.as_ref().map_or(0, |p| p.generation);
            st.pending.insert(id.clone(), (generation, tx));
        }
        let kind = msg["type"].as_str().unwrap_or("").to_string();
        if let Err(e) = self.write_line(&msg) {
            lock(&self.state).pending.remove(&id);
            return Err(e);
        }
        let result = loop {
            match rx.recv_timeout(inactivity) {
                Ok(m) => {
                    let t = m.get("type").and_then(Value::as_str).unwrap_or("");
                    if t == "error" {
                        let err = &m["error"];
                        if let Some(d) = err.get("detail").and_then(Value::as_str) {
                            crate::applog::warn(&format!("studio {kind} error detail: {d}"));
                        }
                        break Err(ApiError::new(
                            err.get("code").and_then(Value::as_str).unwrap_or("internal"),
                            err.get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("The Studio reported an error."),
                        ));
                    }
                    if t.ends_with(".result") || t.ends_with(".ok") || t == "job.accepted" {
                        break Ok(m);
                    }
                    // Progress for this request: keep waiting.
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    crate::applog::warn(&format!("studio: {kind} timed out after {inactivity:?}"));
                    break Err(ApiError::new(
                        "timeout",
                        format!(
                            "The Studio did not answer within {} seconds.",
                            inactivity.as_secs()
                        ),
                    ));
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    break Err(ApiError::new(
                        "studio_crashed",
                        "The Studio engine stopped unexpectedly. Details are in Settings/logs/studio.log.",
                    ));
                }
            }
        };
        let mut st = lock(&self.state);
        st.pending.remove(&id);
        st.last_activity = Instant::now();
        result
    }

    fn read_loop(self: Arc<Self>, stdout: std::process::ChildStdout, generation: u64) {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(msg) => self.dispatch(msg),
                Err(_) => crate::applog::warn(&format!(
                    "studio: ignored a stdout line that is not JSON: {}",
                    line.chars().take(200).collect::<String>()
                )),
            }
        }
        self.on_exit(generation);
    }

    /// Routes one message from the Studio.
    fn dispatch(&self, msg: Value) {
        let t = msg.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        if t == "models.progress" {
            emit(
                &*self.sink,
                names::STUDIO_MODELS,
                &json!({
                    "name": msg.get("name").cloned().unwrap_or(Value::Null),
                    "done_bytes": msg.get("done_bytes").cloned().unwrap_or(json!(0)),
                    "total_bytes": msg.get("total_bytes").cloned().unwrap_or(json!(0)),
                }),
            );
        }
        if let Some(id) = msg.get("id").and_then(Value::as_str) {
            let tx = lock(&self.state).pending.get(id).map(|(_, tx)| tx.clone());
            if let Some(tx) = tx {
                let _ = tx.send(msg.clone());
            }
        }
        if t.starts_with("job.")
            && let Some(job_id) = msg.get("job_id").and_then(Value::as_str)
            && t != "job.accepted"
        {
            self.on_job_event(&t, job_id, &msg);
        } else if t == "error" && msg.get("id").is_none() {
            crate::applog::warn(&format!("studio: unsolicited error {msg}"));
        }
    }

    fn on_job_event(&self, t: &str, job_id: &str, msg: &Value) {
        let view = {
            let mut st = lock(&self.state);
            st.last_activity = Instant::now();
            let library = self.cfg.as_ref().map(|c| c.library_dir.clone());
            let Some(job) = st.jobs.iter_mut().find(|j| j.view.job_id == job_id) else {
                crate::applog::warn(&format!("studio: event {t} for unknown job {job_id}"));
                return;
            };
            let v = &mut job.view;
            match t {
                "job.started" => {
                    v.state = "running";
                    v.message = None;
                }
                "job.progress" => {
                    v.state = "running";
                    v.stage = msg.get("stage").and_then(Value::as_str).map(str::to_string);
                    if let Some(o) = msg.get("overall").and_then(Value::as_f64) {
                        v.overall = o.clamp(0.0, 1.0);
                    }
                    v.stage_progress = msg
                        .get("stage_progress")
                        .and_then(Value::as_f64)
                        .map(|p| p.clamp(0.0, 1.0));
                    v.message = msg.get("message").and_then(Value::as_str).map(str::to_string);
                }
                "job.done" => {
                    v.state = "done";
                    v.overall = 1.0;
                    v.stage = None;
                    v.stage_progress = None;
                    let out = msg.get("output").and_then(Value::as_str).map(PathBuf::from);
                    v.output_song_id = match (out, library) {
                        (Some(o), Some(lib)) => crate::library::Library::new(lib).song_id_of(&o),
                        _ => None,
                    };
                    v.message = msg
                        .pointer("/report/warnings")
                        .and_then(Value::as_array)
                        .filter(|w| !w.is_empty())
                        .map(|w| w.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
                    if let Some(r) = msg.get("report") {
                        crate::applog::info(&format!("studio: job {job_id} done: {r}"));
                    }
                }
                "job.failed" => {
                    let code = msg
                        .pointer("/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("internal")
                        .to_string();
                    v.state = if code == "cancelled" { "cancelled" } else { "failed" };
                    v.error = Some(
                        msg.pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("This song could not be made.")
                            .to_string(),
                    );
                    if let Some(d) = msg.pointer("/error/detail").and_then(Value::as_str) {
                        crate::applog::warn(&format!("studio: job {job_id} failed ({code}): {d}"));
                    }
                    v.error_code = Some(code);
                }
                _ => return,
            }
            v.clone()
        };
        self.emit_job(&view);
        if view.state == "done" {
            self.sink.emit_value(names::LIBRARY_CHANGED, Value::Null);
        }
    }

    /// The process `generation` has exited (or at least closed stdout).
    fn on_exit(&self, generation: u64) {
        let (proc, crashed_jobs, expected) = {
            let mut st = lock(&self.state);
            if !st.proc.as_ref().is_some_and(|p| p.generation == generation) {
                return; // an old process we already replaced or stopped
            }
            let proc = st.proc.take();
            // This process's waiters get "disconnected" (studio_crashed).
            st.pending.retain(|_, (g, _)| *g != generation);
            let expected = st.stopping == Some(generation) || st.closing;
            let mut crashed = Vec::new();
            if !expected {
                st.crashes.push(Instant::now());
                st.crashes.retain(|t| t.elapsed() < CRASH_WINDOW);
                for j in st.jobs.iter_mut() {
                    if j.view.state == "running" {
                        j.view.state = "failed";
                        j.view.error_code = Some("studio_crashed".into());
                        j.view.error = Some(
                            "The Studio engine stopped unexpectedly while making this song. The details are in Settings/logs/studio.log."
                                .into(),
                        );
                        crashed.push(j.view.clone());
                    }
                }
            }
            // Jobs that were waiting in the dead process's queue go out again (unless
            // the user had cancelled them meanwhile).
            for j in st.jobs.iter_mut() {
                if j.view.state == "queued" {
                    j.sent = false;
                    if j.cancel_requested {
                        j.view.state = "cancelled";
                        j.view.error = Some("Cancelled.".into());
                        j.view.error_code = Some("cancelled".into());
                        crashed.push(j.view.clone());
                    }
                }
            }
            (proc, crashed, expected)
        };
        // Reap the process outside the lock (it may take a moment to really exit).
        if let Some(mut p) = proc {
            drop(p.stdin);
            let code = wait_or_kill(&mut p.child, Duration::from_secs(5));
            crate::applog::info(&format!("studio: process exited (code {code:?}, expected {expected})"));
        }
        for j in &crashed_jobs {
            self.emit_job(j);
        }
        if !expected {
            self.emit_status();
            self.wake.notify_all();
        }
    }

    /// [`Self::stop_process`] for callers that do not hold `start_lock`: no new
    /// process can start while the old one is being stopped.
    fn stop_process_locked(&self, grace: Duration) {
        let _start = lock(&self.start_lock);
        self.stop_process(grace);
    }

    /// Asks the Studio to shut down, then closes stdin, then kills it if needed.
    /// The caller holds `start_lock`.
    fn stop_process(&self, grace: Duration) {
        let generation = {
            let mut st = lock(&self.state);
            let Some(g) = st.proc.as_ref().map(|p| p.generation) else { return };
            st.stopping = Some(g);
            g
        };
        let _ = self.request(json!({"type": "shutdown"}), grace.min(Duration::from_secs(5)));
        let taken = {
            let mut st = lock(&self.state);
            if st.proc.as_ref().is_some_and(|p| p.generation == generation) { st.proc.take() } else { None }
        };
        if let Some(mut p) = taken {
            drop(p.stdin); // the Studio exits when stdin closes
            let code = wait_or_kill(&mut p.child, grace);
            crate::applog::info(&format!("studio: stopped (exit code {code:?})"));
        }
        let mut st = lock(&self.state);
        st.pending.retain(|_, (g, _)| *g != generation);
        drop(st);
        self.emit_status();
    }

    // ------------------------------------------------------------------ jobs

    /// `studio_submit`: queues a job and returns its id at once. The dispatcher
    /// starts the Studio (if needed) and sends it.
    pub fn submit(&self, input: &Value) -> ApiResult<String> {
        let cfg = self.require_available()?;
        let obj = input
            .as_object()
            .ok_or_else(|| ApiError::bad_request("The job must be an object."))?;
        let path = obj
            .get("input")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::bad_request("Choose a file to make a song from."))?;
        if !Path::new(path).is_file() {
            return Err(ApiError::new("not_found", "That file could not be found."));
        }
        let preset = obj.get("preset").and_then(Value::as_str).unwrap_or(DEFAULT_PRESET);
        // A song recorded with "Record songs separately": its sidecar names the cover
        // the player showed. "No cover_path" means "the file's own cover", and for a
        // recording that is the sidecar's (a WAV holds none); null still removes it.
        let mut metadata = obj.get("metadata").cloned().unwrap_or_else(|| json!({}));
        if let Some(m) = metadata.as_object_mut()
            && !m.contains_key("cover_path")
            && let Some(cover) = recorded_song(Path::new(path)).and_then(|(sc, p)| sc.cover_path(&p))
        {
            m.insert("cover_path".into(), json!(cover.display().to_string()));
        }
        if !matches!(preset, "full" | "clean" | "permissive") {
            return Err(ApiError::bad_request(
                "The quality preset must be \"full\", \"clean\" or \"permissive\".",
            ));
        }
        let job_id = format!(
            "job-{}-{:08x}",
            self.ids.fetch_add(1, Ordering::SeqCst) + 1,
            crate::fs_atomic::random_u32()
        );
        let spec = json!({
            "job_id": job_id,
            "input": path,
            "output_dir": cfg.library_dir,
            "preset": preset,
            "language": obj.get("language").and_then(Value::as_str).unwrap_or("en"),
            "metadata": metadata,
            "lyrics": obj.get("lyrics").cloned().unwrap_or_else(|| json!({"mode": "auto", "text": null, "lrc": null})),
        });
        let view = StudioJob {
            job_id: job_id.clone(),
            input_name: file_name(path),
            state: "queued",
            stage: None,
            overall: 0.0,
            stage_progress: None,
            message: None,
            output_song_id: None,
            error: None,
            error_code: None,
        };
        lock(&self.state).jobs.push(Job {
            view: view.clone(),
            spec,
            sent: false,
            cancel_requested: false,
        });
        self.emit_job(&view);
        self.wake.notify_all();
        Ok(job_id)
    }

    /// `studio_cancel`.
    pub fn cancel(&self, job_id: &str) -> ApiResult<()> {
        let (sent, view) = {
            let mut st = lock(&self.state);
            let job = st
                .jobs
                .iter_mut()
                .find(|j| j.view.job_id == job_id)
                .ok_or_else(|| ApiError::new("not_found", "This job is not in the queue."))?;
            if !matches!(job.view.state, "queued" | "running") {
                return Ok(());
            }
            job.cancel_requested = true;
            if job.sent {
                (true, None)
            } else {
                // Not handed to the Studio yet: the dispatcher claims jobs under the
                // same lock, so it will never send this one now.
                job.view.state = "cancelled";
                job.view.error = Some("Cancelled.".into());
                job.view.error_code = Some("cancelled".into());
                (false, Some(job.view.clone()))
            }
        };
        if let Some(v) = view {
            self.emit_job(&v);
        }
        if sent {
            let timeout = self.cfg.as_ref().map_or(Duration::from_secs(30), |c| c.request_timeout);
            match self.request(json!({"type": "job.cancel", "job_id": job_id}), timeout) {
                Ok(_) => {}
                // Sent but not yet accepted: the dispatcher cancels it on acceptance.
                Err(e) if e.code == "not_found" => {}
                // The Studio died meanwhile: on_exit turns it into "cancelled".
                Err(e) if e.code == "studio_crashed" => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Removes finished jobs (done / failed / cancelled) from the queue view.
    pub fn clear_finished(&self) {
        lock(&self.state)
            .jobs
            .retain(|j| matches!(j.view.state, "queued" | "running"));
        self.emit_status();
    }

    fn dispatch_loop(self: Arc<Self>) {
        loop {
            // Wait until there is an unsent queued job (or the Studio has been idle).
            let has_work = {
                let mut st = lock(&self.state);
                loop {
                    let waiting = st.jobs.iter().any(|j| !j.sent && j.view.state == "queued");
                    if waiting && !matches!(st.avail, Avail::Checking) {
                        break true;
                    }
                    let idle_after = self.cfg.as_ref().map_or(IDLE_SHUTDOWN, |c| c.idle_shutdown);
                    let idle = st.proc.is_some()
                        && st.pending.is_empty()
                        && !st.jobs.iter().any(|j| matches!(j.view.state, "queued" | "running"))
                        && st.last_activity.elapsed() >= idle_after;
                    if idle {
                        break false;
                    }
                    st = self
                        .wake
                        .wait_timeout(st, Duration::from_secs(30).min(idle_after))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            };
            if !has_work {
                crate::applog::info("studio: idle; stopping it to free memory");
                self.stop_process_locked(Duration::from_secs(10));
                continue;
            }
            // Not available (probe said no): fail what is waiting, with the reason.
            if let Avail::No(reasons) = lock(&self.state).avail.clone() {
                self.fail_waiting("studio_unavailable", &reasons.join(" "));
                continue;
            }
            // Too many crashes: stop restarting.
            let crashes = {
                let mut st = lock(&self.state);
                st.crashes.retain(|t| t.elapsed() < CRASH_WINDOW);
                st.crashes.len()
            };
            if crashes >= MAX_CRASHES && lock(&self.state).proc.is_none() {
                self.fail_waiting(
                    "studio_crashed",
                    "The Studio engine stopped unexpectedly several times, so the waiting songs were not started. The details are in Settings/logs/studio.log. Try again later, or restart the app.",
                );
                lock(&self.state).crashes.clear();
                continue;
            }
            if let Err(e) = self.ensure_started() {
                self.fail_waiting(&e.code, &e.message);
                continue;
            }
            // Claim the next job under the lock (a cancel in between wins), marking it
            // sent before sending: its events may arrive before job.accepted.
            let claimed = {
                let mut st = lock(&self.state);
                st.jobs
                    .iter_mut()
                    .find(|j| !j.sent && j.view.state == "queued")
                    .map(|j| {
                        j.sent = true;
                        (j.view.job_id.clone(), j.spec.clone())
                    })
            };
            let Some((job_id, spec)) = claimed else { continue };
            let timeout = self.cfg.as_ref().map_or(Duration::from_secs(60), |c| c.request_timeout);
            match self.request(json!({"type": "job.submit", "job": spec}), timeout) {
                Ok(_) => {
                    let cancel = lock(&self.state).jobs.iter().any(|j| {
                        j.view.job_id == job_id && j.cancel_requested && matches!(j.view.state, "queued" | "running")
                    });
                    if cancel {
                        let _ = self.request(json!({"type": "job.cancel", "job_id": job_id}), timeout);
                    }
                }
                Err(e) if e.code == "studio_crashed" => {
                    // on_exit already put it back in the queue (sent = false).
                }
                Err(e) => self.fail_job(&job_id, &e.code, &e.message),
            }
        }
    }

    fn fail_job(&self, job_id: &str, code: &str, message: &str) {
        let view = {
            let mut st = lock(&self.state);
            let Some(j) = st.jobs.iter_mut().find(|j| j.view.job_id == job_id) else { return };
            j.view.state = "failed";
            j.view.error = Some(message.to_string());
            j.view.error_code = Some(code.to_string());
            j.view.clone()
        };
        self.emit_job(&view);
    }

    fn fail_waiting(&self, code: &str, message: &str) {
        let views: Vec<StudioJob> = {
            let mut st = lock(&self.state);
            st.jobs
                .iter_mut()
                .filter(|j| !j.sent && j.view.state == "queued")
                .map(|j| {
                    j.view.state = "failed";
                    j.view.error = Some(message.to_string());
                    j.view.error_code = Some(code.to_string());
                    j.view.clone()
                })
                .collect()
        };
        for v in &views {
            self.emit_job(v);
        }
    }

    // ------------------------------------------------------------------ requests

    /// Starts the Studio if needed and sends one request.
    fn call(self: &Arc<Self>, msg: Value, inactivity: Option<Duration>) -> ApiResult<Value> {
        let cfg = self.require_available()?.clone();
        self.ensure_started()?;
        let mut r = self.request(msg, inactivity.unwrap_or(cfg.request_timeout))?;
        if let Some(o) = r.as_object_mut() {
            o.remove("type");
            o.remove("id");
        }
        Ok(r)
    }

    /// `studio_inspect`.
    pub fn inspect(self: &Arc<Self>, path: &str) -> ApiResult<Value> {
        if !Path::new(path).is_file() {
            return Err(ApiError::new("not_found", "That file could not be found."));
        }
        let mut r = self.call(json!({"type": "inspect", "input": path}), None)?;
        merge_recorded_song(&mut r, Path::new(path));
        Ok(r)
    }

    /// `studio_lyrics_lookup` (only ever when the user pressed the button).
    pub fn lyrics_lookup(self: &Arc<Self>, q: &Value) -> ApiResult<Value> {
        let msg = json!({
            "type": "lyrics.lookup",
            "artist": q.get("artist").cloned().unwrap_or(Value::Null),
            "title": q.get("title").cloned().unwrap_or(Value::Null),
            "album": q.get("album").cloned().unwrap_or(Value::Null),
            "duration_seconds": q.get("duration_s").or_else(|| q.get("duration_seconds")).cloned().unwrap_or(Value::Null),
        });
        self.call(msg, None)
    }

    /// `studio_models_status(preset)`: the models of that preset (the one chosen in the
    /// Studio form) plus the lyrics models. `StudioStatus.models_present` stays the
    /// answer for the default preset only, so asking about "clean" or "permissive" never
    /// changes it.
    pub fn models_status(self: &Arc<Self>, preset: &str) -> ApiResult<Value> {
        let r = self.call(json!({"type": "models.status", "preset": preset}), None)?;
        if let Some(all) = r.get("all_present").and_then(Value::as_bool).filter(|_| preset == DEFAULT_PRESET) {
            lock(&self.state).models_present = all;
        }
        Ok(r)
    }

    /// `studio_models_download(preset)`: downloads that preset's missing models (and the
    /// lyrics models); progress arrives as `studio://models` events; this waits until the
    /// download has finished (10 minutes without progress = timeout). Then
    /// `models_present` is asked again for the default preset: a "permissive" download
    /// does not bring the default preset's models.
    pub fn models_download(self: &Arc<Self>, preset: &str) -> ApiResult<Value> {
        let r = self.call(
            json!({"type": "models.download", "preset": preset}),
            Some(Duration::from_secs(600)),
        )?;
        if let Err(e) = self.models_status(DEFAULT_PRESET) {
            crate::applog::warn(&format!("studio models.status after a download: {e}"));
        }
        self.emit_status();
        Ok(r)
    }

    /// `studio_realign`: aligns corrected text against the song's lead vocal.
    pub fn realign(self: &Arc<Self>, song: &Path, text: &str, anchors: &Value) -> ApiResult<Value> {
        let r = self.call(
            json!({"type": "realign", "song": song, "lyrics": {"mode": "text", "text": text}, "anchors": anchors}),
            Some(Duration::from_secs(900)),
        )?;
        if r.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(ApiError::new("alignment_failed", "The lyrics could not be aligned to the song."));
        }
        r.get("lyrics")
            .cloned()
            .ok_or_else(|| ApiError::internal("realign.result without lyrics"))
    }

    pub fn running(&self) -> bool {
        lock(&self.state).proc.is_some()
    }

    /// On exit: ask the Studio to shut down (it cancels a running job and deletes its
    /// temporary file), then make sure it is gone. If a start is in progress (the
    /// Studio is still answering `hello`, which may take up to the start timeout),
    /// the half-started process is killed instead, so closing the app never waits
    /// for it.
    pub fn shutdown(&self) {
        lock(&self.state).closing = true;
        match self.start_lock.try_lock() {
            Ok(_start) => self.stop_process(Duration::from_secs(5)),
            Err(_) => {
                let taken = lock(&self.state).proc.take();
                if let Some(mut p) = taken {
                    crate::applog::warn("studio: closing while it was starting; killing it");
                    drop(p.stdin);
                    let _ = p.child.kill();
                    let _ = p.child.wait();
                    // The waiting `hello` gets "disconnected" and the start gives up.
                    lock(&self.state).pending.retain(|_, (g, _)| *g != p.generation);
                }
            }
        }
        self.wake.notify_all();
    }
}

/// Waits up to `grace` for the child to exit, then kills it. Returns the exit code.
fn wait_or_kill(child: &mut Child, grace: Duration) -> Option<i32> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                crate::applog::warn("studio: did not exit in time; killing it");
                let _ = child.kill();
                return child.wait().ok().and_then(|s| s.code());
            }
        }
    }
}

/// Appends the Studio's stderr to studio.log (rotated above 8 MiB).
fn copy_stderr(stderr: std::process::ChildStderr, log_path: &Path, pid: u32) {
    use std::fs::OpenOptions;
    if std::fs::metadata(log_path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(log_path, log_path.with_extension("log.1"));
    }
    let mut out = OpenOptions::new().create(true).append(true).open(log_path).ok();
    if let Some(f) = out.as_mut() {
        let _ = writeln!(f, "==== {} Studio started, pid {pid} ====", jk_format::time::utc_now_iso8601());
    }
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if let Some(f) = out.as_mut() {
                    let _ = f.write_all(&buf);
                }
            }
        }
    }
    if let Some(f) = out.as_mut() {
        let _ = writeln!(f, "==== {} Studio stderr closed, pid {pid} ====", jk_format::time::utc_now_iso8601());
    }
}

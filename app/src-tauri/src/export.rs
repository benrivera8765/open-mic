// SPDX-License-Identifier: GPL-3.0-or-later
//! "Export backing track" (docs/APP_API.md section 7, WORK_ORDER 4.3).
//!
//! Renders the song's current key and mix offline with the same DSP graph as
//! playback (`jk_engine::render_offline`), at the song's sample rate, on a worker
//! thread, to `<root>/Exports/<artist> - <title> (<key>).flac|wav`. Events:
//! `export://progress` (throttled to about 10 per second), `export://done`,
//! `export://failed`. `export_cancel` (an addition to APP_API v1) stops a render.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::engine::{ExportFormat, MixParams, Renderer};
use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};

#[derive(Clone, Debug, Serialize)]
pub struct ExportStarted {
    pub job_id: String,
}

#[derive(Clone, Debug, Serialize)]
struct Progress<'a> {
    job_id: &'a str,
    progress: f32,
}

#[derive(Clone, Debug, Serialize)]
struct Done<'a> {
    job_id: &'a str,
    path: String,
    /// Addition: gain the FLAC export applied to avoid clipping (dB, <= 0).
    applied_gain_db: f64,
    peak: f32,
}

#[derive(Clone, Debug, Serialize)]
struct Failed<'a> {
    job_id: &'a str,
    code: String,
    message: String,
}

/// Characters that are not allowed in file names on Windows, macOS or exFAT.
fn bad_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
}

/// A file name that works on Windows, macOS and exFAT: invalid characters become
/// `_`, no trailing dots or spaces, no reserved Windows device names, at most 150
/// characters (long paths on a stick are fragile).
pub fn safe_file_stem(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if bad_char(c) { '_' } else { c }).collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.chars().count() > 150 {
        out = out.chars().take(150).collect::<String>().trim_end_matches(['.', ' ']).to_string();
    }
    let upper = out.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base)
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    if out.is_empty() { "Export".into() } else { out }
}

/// `<dir>/<stem>.<ext>`, or `<stem> (2).<ext>`, `(3)`... when taken (on disk or by a
/// render still running).
pub fn unique_path(dir: &Path, stem: &str, ext: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() && !reserved.contains(&first) {
        return first;
    }
    for n in 2.. {
        let p = dir.join(format!("{stem} ({n}).{ext}"));
        if !p.exists() && !reserved.contains(&p) {
            return p;
        }
    }
    unreachable!()
}

/// What to render.
pub struct ExportRequest {
    pub song: PathBuf,
    pub artist: String,
    pub title: String,
    /// "B major, +2" (see keys::export_key_text).
    pub key_text: String,
    pub params: MixParams,
    pub format: ExportFormat,
}

pub struct Exports {
    dir: PathBuf,
    sink: Arc<dyn EventSink>,
    renderer: Arc<dyn Renderer>,
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    reserved: Arc<Mutex<HashSet<PathBuf>>>,
    counter: AtomicU64,
}

impl Exports {
    pub fn new(dir: PathBuf, sink: Arc<dyn EventSink>, renderer: Arc<dyn Renderer>) -> Exports {
        Exports {
            dir,
            sink,
            renderer,
            jobs: Mutex::new(HashMap::new()),
            reserved: Arc::new(Mutex::new(HashSet::new())),
            counter: AtomicU64::new(0),
        }
    }

    /// Starts a render on a worker thread and returns its job id at once.
    pub fn start(self: &Arc<Self>, req: ExportRequest) -> ApiResult<ExportStarted> {
        std::fs::create_dir_all(&self.dir).map_err(|e| ApiError::io("the Exports folder", &e))?;
        let ext = match req.format {
            ExportFormat::Flac24 => "flac",
            ExportFormat::Wav32f => "wav",
        };
        let name = if req.artist.trim().is_empty() {
            format!("{} ({})", req.title, req.key_text)
        } else {
            format!("{} - {} ({})", req.artist, req.title, req.key_text)
        };
        let out = {
            let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
            let p = unique_path(&self.dir, &safe_file_stem(&name), ext, &reserved);
            reserved.insert(p.clone());
            p
        };
        let job_id = format!(
            "export-{}-{}",
            self.counter.fetch_add(1, Ordering::SeqCst) + 1,
            crate::fs_atomic::random_u32()
        );
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job_id.clone(), Arc::clone(&cancel));
        let me = Arc::clone(self);
        let id = job_id.clone();
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || me.run(&id, req, &out, &cancel))
            .map_err(ApiError::internal)?;
        Ok(ExportStarted { job_id })
    }

    fn run(&self, job_id: &str, req: ExportRequest, out: &Path, cancel: &AtomicBool) {
        crate::applog::info(&format!(
            "export {job_id}: {} -> {} ({:?}, {:?})",
            req.song.display(),
            out.display(),
            req.format,
            req.params
        ));
        let started = Instant::now();
        let mut last = Instant::now() - Duration::from_secs(1);
        let sink = Arc::clone(&self.sink);
        let mut progress = |p: f32| {
            // ~10 events per second is plenty for a progress bar.
            if last.elapsed() >= Duration::from_millis(100) || p >= 1.0 {
                last = Instant::now();
                emit(&*sink, names::EXPORT_PROGRESS, &Progress { job_id, progress: p.clamp(0.0, 1.0) });
            }
        };
        let result = self
            .renderer
            .render(&req.song, &req.params, out, req.format, &mut progress, cancel);
        match result {
            Ok(report) => {
                crate::applog::info(&format!(
                    "export {job_id} done in {:.1} s: {} frames at {} Hz, peak {:.4}, gain {:.2} dB",
                    started.elapsed().as_secs_f64(),
                    report.frames,
                    report.sample_rate,
                    report.peak,
                    report.applied_gain_db
                ));
                emit(
                    &*self.sink,
                    names::EXPORT_DONE,
                    &Done {
                        job_id,
                        path: out.display().to_string(),
                        applied_gain_db: report.applied_gain_db,
                        peak: report.peak,
                    },
                );
            }
            Err(e) => {
                crate::applog::warn(&format!("export {job_id} failed: {e}"));
                emit(
                    &*self.sink,
                    names::EXPORT_FAILED,
                    &Failed {
                        job_id,
                        code: e.code.clone(),
                        message: e.message.clone(),
                    },
                );
            }
        }
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(job_id);
        self.reserved.lock().unwrap_or_else(|e| e.into_inner()).remove(out);
    }

    /// `export_cancel` (addition): stops a running render; it then fails with
    /// code `cancelled` and leaves no file.
    pub fn cancel(&self, job_id: &str) -> ApiResult<()> {
        match self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(job_id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err(ApiError::new("not_found", "This export is not running.")),
        }
    }

    /// Cancels every running render (on exit).
    pub fn cancel_all(&self) {
        for flag in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn running(&self) -> usize {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_everywhere() {
        assert_eq!(safe_file_stem("AC/DC - Back: In? (A major)"), "AC_DC - Back_ In_ (A major)");
        assert_eq!(safe_file_stem("  name. . "), "name");
        assert_eq!(safe_file_stem("CON"), "_CON");
        assert_eq!(safe_file_stem("com1.x"), "_com1.x");
        assert_eq!(safe_file_stem("Comet"), "Comet");
        assert_eq!(safe_file_stem("\u{0}\u{7}"), "__");
        assert_eq!(safe_file_stem("..."), "Export");
        assert_eq!(safe_file_stem(&"x".repeat(400)).len(), 150);
    }

    #[test]
    fn unique_paths_skip_existing_and_reserved_names() {
        let dir = std::env::temp_dir().join(format!(
            "jukeoke-export-test-{}-{:08x}",
            std::process::id(),
            crate::fs_atomic::random_u32()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.flac"), b"").unwrap();
        let mut reserved = HashSet::new();
        reserved.insert(dir.join("A (2).flac"));
        assert_eq!(unique_path(&dir, "A", "flac", &reserved), dir.join("A (3).flac"));
        assert_eq!(unique_path(&dir, "B", "flac", &reserved), dir.join("B.flac"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

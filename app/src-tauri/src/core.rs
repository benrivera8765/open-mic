// SPDX-License-Identifier: GPL-3.0-or-later
//! The app core: everything the commands use, independent of Tauri so it can be
//! built and tested without a window (tests pass a fake engine, a fake renderer and a
//! recording event sink).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::deleted::{DeletedList, DeletedSong, DeletedSongs, Failure, Purged, Restored, ToDelete};
use crate::engine::{AudioEngine, ExportFormat, MixParams, Renderer};
use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};
use crate::export::{ExportRequest, ExportStarted, Exports};
use crate::guide::{BatchQueued, GuideQueue, GuideService, GuideStatus};
use crate::library::{Library, LibraryList, Watcher};
use crate::playback::{Player, SongDetail};
use crate::portable::{DataLayout, DataMode, Paths};
use crate::recorder::RecorderCtl;
use crate::settings::{AppSettings, SettingsStore};
use crate::studio::{Studio, StudioConfig};

/// What the core is built from.
pub struct CoreOptions {
    pub layout: DataLayout,
    pub exe: Option<PathBuf>,
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub sink: Arc<dyn EventSink>,
    pub engine: Arc<dyn AudioEngine>,
    pub renderer: Arc<dyn Renderer>,
    pub studio: Result<StudioConfig, String>,
    /// The bundled demo song (Tauri resource), if found.
    pub demo_song: Option<PathBuf>,
    /// Debounce of the per-song settings save (1 s in the app).
    pub slot_save_delay: Duration,
    /// Value of WEBVIEW2_USER_DATA_FOLDER that was ignored, if any (diagnostics).
    pub webview_env_ignored: Option<String>,
}

pub struct Core {
    pub layout: DataLayout,
    pub paths: Paths,
    pub exe: Option<PathBuf>,
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub sink: Arc<dyn EventSink>,
    pub settings: SettingsStore,
    pub library: Arc<Library>,
    /// Deleted songs, `<root>/Deleted` (src/deleted.rs).
    pub deleted: DeletedSongs,
    pub player: Arc<Player>,
    pub exports: Arc<Exports>,
    pub recorder: RecorderCtl,
    pub studio: Arc<Studio>,
    /// (WO2) The melody guide queue (src/guide.rs).
    pub guide: Arc<GuideService>,
    pub renderer: Arc<dyn Renderer>,
    pub demo_song: Option<PathBuf>,
    pub webview_env_ignored: Option<String>,
    watcher: Mutex<Option<Watcher>>,
    /// navigator.userAgent as the UI reported it (self-check).
    pub user_agent: Mutex<Option<String>>,
}

/// docs/APP_API.md `AppInfo` (snake_case), plus additions marked below.
#[derive(Clone, Debug, Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: &'static str,
    pub os_version: String,
    pub portable: bool,
    pub root: Option<String>,
    pub translocated: bool,
    pub paths: PathsView,
    pub capabilities: Capabilities,
    // ---- additions to APP_API v1 ----
    pub identifier: String,
    /// "portable" | "dev" | "user" | "translocated".
    pub mode: DataMode,
    pub mode_reason: String,
    /// WebView2 runtime version (Windows) or WebKit version (macOS).
    pub webview_version: String,
    /// What the UI shows before asking for a stick folder when translocated.
    pub translocation_help: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PathsView {
    pub library: String,
    pub recordings: String,
    pub exports: String,
    pub settings: String,
    pub models: Option<String>,
    /// Addition (2026-11-18): the Deleted songs folder (created on the first delete).
    pub deleted: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Capabilities {
    pub studio: crate::studio::StudioCapability,
    pub recorder: RecorderCapability,
    pub exclusive_mode: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecorderCapability {
    pub available: bool,
    pub reason: Option<String>,
    pub permission: &'static str,
    /// Addition: the text to show before asking for "Screen Recording" (macOS).
    pub permission_explanation: Option<String>,
    /// Addition: where the user switches the permission on (macOS).
    pub settings_url: Option<String>,
    /// Addition: "WASAPI loopback" / "ScreenCaptureKit".
    pub backend: &'static str,
}

/// `library_delete` result: the songs now in Deleted songs, the ones that could not be
/// moved (with the reason), and the loaded song's id when it was stopped and closed.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteResult {
    pub deleted: Vec<DeletedSong>,
    pub failed: Vec<Failure>,
    pub unloaded: Option<String>,
}

/// `library_restore` result.
#[derive(Clone, Debug, Serialize)]
pub struct RestoreResult {
    pub restored: Vec<Restored>,
    pub failed: Vec<Failure>,
}

/// Shown when macOS runs the app from a translocated copy.
pub const TRANSLOCATION_HELP: &str = "macOS opened Jukeoke from a temporary, read-only copy (this happens to apps \
copied from a download or another computer), so it cannot see the songs next to it on the stick. Choose the \
stick's main folder (the one that holds Library and Mac) and Jukeoke will remember it on this Mac. To stop this \
happening, move Jukeoke.app out of the Mac folder and back in once with Finder, or open it once with Control-click > Open.";

fn s(p: &Path) -> String {
    p.display().to_string()
}

impl Core {
    pub fn new(o: CoreOptions) -> Arc<Core> {
        let paths = o.layout.paths();
        for d in [&paths.settings, &paths.logs, &paths.library] {
            if let Err(e) = std::fs::create_dir_all(d) {
                crate::applog::warn(&format!("cannot create {}: {e}", d.display()));
            }
        }
        let settings = SettingsStore::load(&paths.settings);
        let library = Arc::new(Library::new(paths.library.clone()));
        let deleted = DeletedSongs::new(paths.deleted.clone(), paths.library.clone());
        let player = Arc::new(Player::new(
            o.engine,
            Arc::clone(&o.sink),
            Arc::clone(&library),
            o.slot_save_delay,
        ));
        player.attach();
        {
            // Key change: the cache folder (never a per-user folder in portable mode)
            // and the mode inputs (src/keymode.rs).
            let a = settings.get().audio;
            player.configure_key_cache(crate::keymode::cache_dir(&o.layout).as_deref(), crate::keymode::cache_bytes(&a));
            let me = crate::keymode::machine_id();
            player.set_key_config(a.key_mode, crate::keymode::factors_for(&me, &a.key_benchmarks));
        }
        let exports = Arc::new(Exports::new(
            paths.exports.clone(),
            Arc::clone(&o.sink),
            Arc::clone(&o.renderer),
        ));
        let recorder = RecorderCtl::new(paths.recordings.clone(), &paths.settings, Arc::clone(&o.sink));
        let studio = Studio::new(o.studio, Arc::clone(&o.sink));
        // (WO2) The guide queue, wired both ways to the Studio: a finished song gets its
        // guide as the job's last stage (STUDIO_PROTOCOL section 12). Weak references, so
        // neither keeps the other alive.
        let guide = GuideService::new(
            Arc::clone(&library),
            Arc::clone(&player),
            Arc::clone(&o.sink),
            &paths.settings,
            &o.name,
            &o.version,
        );
        {
            let (g1, g2) = (Arc::downgrade(&guide), Arc::downgrade(&guide));
            studio.set_guide_hooks(crate::studio::GuideHooks {
                on_done: Arc::new(move |job_id: &str, song_id: &str| {
                    if let Some(g) = g1.upgrade() {
                        g.enqueue_new_song(song_id, job_id);
                    }
                }),
                on_cancel: Arc::new(move |song_id: &str| {
                    if let Some(g) = g2.upgrade() {
                        g.cancel(Some(song_id));
                    }
                }),
            });
            let st = Arc::downgrade(&studio);
            guide.set_job_hook(Arc::new(move |job_id: &str, status: &GuideStatus, finished: bool| {
                if let Some(s) = st.upgrade() {
                    s.guide_stage_update(job_id, status, finished);
                }
            }));
        }
        Arc::new(Core {
            layout: o.layout,
            paths,
            exe: o.exe,
            name: o.name,
            version: o.version,
            identifier: o.identifier,
            sink: o.sink,
            settings,
            library,
            deleted,
            player,
            exports,
            recorder,
            studio,
            guide,
            renderer: o.renderer,
            demo_song: o.demo_song,
            webview_env_ignored: o.webview_env_ignored,
            watcher: Mutex::new(None),
            user_agent: Mutex::new(None),
        })
    }

    /// Background work after the window is up: clock thread, library watcher, stale
    /// temp file clean-up, Studio probe.
    pub fn start_background(self: &Arc<Self>) {
        // "Record songs separately" sessions a crash or a closed app left behind.
        self.recorder.recover_sessions();
        self.player.start_clock();
        // "auto" key mode: measurements are saved per computer and rate; the common
        // rates are measured now, in the background, before a song needs them.
        let weak = Arc::downgrade(self);
        self.player.set_bench_hook(Arc::new(move |rate, factor| {
            if let Some(core) = weak.upgrade() {
                let key = crate::keymode::bench_key(&crate::keymode::machine_id(), rate);
                core.settings.update(|s| {
                    s.audio.key_benchmarks.insert(key.clone(), (factor * 100.0).round() / 100.0);
                });
            }
        }));
        self.player.measure_rates(&[44_100, 48_000]);
        // (WO2) Guides left in the queue by the last session, then new ones.
        self.guide.start();
        *self.watcher.lock().unwrap_or_else(|e| e.into_inner()) =
            Watcher::start(&self.paths.library, Arc::clone(&self.sink));
        let me = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("startup-background".into())
            .spawn(move || {
                let n = me.library.clean_stale_temp_files();
                if n > 0 {
                    crate::applog::info(&format!("removed {n} unfinished song writes"));
                }
                // Finishes or undoes a delete / restore an interruption cut off.
                let d = me.deleted.list();
                if d.count > 0 {
                    crate::applog::info(&format!("{} deleted songs ({} bytes) in {}", d.count, d.total_bytes, d.folder));
                }
                crate::portable::remove_discarded_profiles(&me.paths.settings);
                me.studio.probe_now();
            });
    }

    /// Writes everything pending and stops background work (on exit).
    pub fn shutdown(&self) {
        crate::applog::info("shutting down");
        self.player.shutdown();
        // (WO2) After the song is closed: guides that waited for it to stop are written.
        self.guide.shutdown();
        self.settings.flush();
        self.recorder.shutdown();
        self.exports.cancel_all();
        self.studio.shutdown();
        *self.watcher.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn app_info(&self, webview_version: String) -> AppInfo {
        let l = &self.layout;
        let studio_possible = cfg!(windows);
        let permission = self.recorder.permission();
        let backend = jk_recorder::backend();
        let mac = cfg!(target_os = "macos");
        AppInfo {
            name: self.name.clone(),
            version: self.version.clone(),
            platform: if mac { "macos" } else { "windows" },
            os_version: crate::sysinfo::os_version(),
            portable: l.mode == DataMode::Portable,
            root: l.portable_root.as_deref().map(s),
            translocated: l.translocated,
            paths: PathsView {
                library: s(&self.paths.library),
                recordings: s(&self.paths.recordings),
                exports: s(&self.paths.exports),
                settings: s(&self.paths.settings),
                models: studio_possible.then(|| s(&self.paths.models)),
                deleted: s(&self.paths.deleted),
            },
            capabilities: Capabilities {
                studio: self.studio.capability(),
                recorder: RecorderCapability {
                    available: backend != jk_recorder::Backend::None,
                    reason: (backend == jk_recorder::Backend::None)
                        .then(|| "Recording the computer's sound is not supported on this system.".to_string()),
                    permission,
                    permission_explanation: mac.then(|| jk_recorder::PERMISSION_EXPLANATION.to_string()),
                    settings_url: mac.then(|| jk_recorder::MACOS_SCREEN_RECORDING_SETTINGS_URL.to_string()),
                    backend: backend.name(),
                },
                exclusive_mode: cfg!(windows),
            },
            identifier: self.identifier.clone(),
            mode: l.mode,
            mode_reason: l.reason.clone(),
            webview_version,
            translocation_help: (l.mode == DataMode::Translocated).then(|| TRANSLOCATION_HELP.to_string()),
        }
    }

    // ---------------------------------------------------------------- settings

    /// `settings_set`, with the audio side effects applied live.
    pub fn settings_set(&self, changes: &Value) -> ApiResult<AppSettings> {
        let before = self.settings.get();
        let after = self.settings.set(changes)?;
        if (after.audio.master_volume - before.audio.master_volume).abs() > f64::EPSILON {
            self.player.set_master(after.audio.master_volume)?;
        }
        if after.audio.device_id != before.audio.device_id || after.audio.exclusive != before.audio.exclusive {
            self.player
                .set_device(after.audio.device_id.as_deref(), after.audio.exclusive)?;
        }
        if after.audio.key_cache_mb != before.audio.key_cache_mb {
            self.player.configure_key_cache(
                crate::keymode::cache_dir(&self.layout).as_deref(),
                crate::keymode::cache_bytes(&after.audio),
            );
        }
        if after.audio.key_mode != before.audio.key_mode || after.audio.key_benchmarks != before.audio.key_benchmarks {
            let me = crate::keymode::machine_id();
            self.player.set_key_config(after.audio.key_mode, crate::keymode::factors_for(&me, &after.audio.key_benchmarks));
        }
        Ok(after)
    }

    /// `player_set_master`: live, and remembered in the app settings.
    pub fn set_master(&self, volume: f64) -> ApiResult<()> {
        let v = self.player.set_master(volume)?;
        self.settings.update(|s| s.audio.master_volume = v as f64);
        Ok(())
    }

    /// `device_set`: switch and remember.
    pub fn device_set(&self, id: Option<String>, exclusive: bool) -> ApiResult<()> {
        let id = id.filter(|s| !s.is_empty());
        let r = self.player.set_device(id.as_deref(), exclusive);
        // Remember the choice even when exclusive mode was refused (the engine then
        // plays in shared mode on that device and reports why).
        if r.is_ok() || r.as_ref().is_err_and(|e| e.code == "exclusive_unavailable") {
            self.settings.update(|s| {
                s.audio.device_id = id.clone();
                s.audio.exclusive = exclusive;
            });
        }
        r
    }

    // ---------------------------------------------------------------- library, deleted songs

    /// `library_list`: the songs, free space, and what Deleted songs still holds.
    pub fn library_list(&self) -> ApiResult<LibraryList> {
        let mut list = self.library.list()?;
        // (WO2) The guide queue's live state over what each file says.
        for s in list.songs.iter_mut() {
            s.guide = self.guide.overlay(&s.song_id, s.guide.clone());
        }
        let d = self.deleted.list();
        list.deleted_count = d.count;
        list.deleted_bytes = d.total_bytes;
        Ok(list)
    }

    /// `library_deleted_list`.
    pub fn library_deleted_list(&self) -> DeletedList {
        self.deleted.list()
    }

    fn deleted_changed(&self) {
        emit(&*self.sink, names::LIBRARY_DELETED, &self.deleted.list());
        // Free space, the song count and Deleted songs' size in the Library's header.
        self.sink.emit_value(names::LIBRARY_CHANGED, Value::Null);
    }

    /// `library_delete(song_ids)`: moves songs into Deleted songs. The loaded song, if
    /// it is one of them, is stopped and closed first (its pending settings written).
    pub fn library_delete(&self, song_ids: &[String]) -> ApiResult<DeleteResult> {
        if song_ids.is_empty() {
            return Err(ApiError::bad_request("Choose at least one song to delete."));
        }
        let mut failed = Vec::new();
        let mut targets: Vec<(String, PathBuf)> = Vec::new();
        for id in song_ids {
            if targets.iter().any(|(t, _)| t == id) {
                continue;
            }
            match self.library.resolve(id) {
                Ok(p) => targets.push((id.clone(), p)),
                Err(e) => failed.push(Failure::new(id, &e)),
            }
        }
        let paths: Vec<PathBuf> = targets.iter().map(|(_, p)| p.clone()).collect();
        // (WO2) A guide being made for a song that moves away is cancelled first.
        for (id, _) in &targets {
            self.guide.forget(id);
        }
        let _files = self.guide.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        // The files move while no song can be loaded (a `song_load` racing with this
        // waits, then finds the file gone).
        let (unloaded, (deleted, more)) = self.player.release_during(&paths, || {
            // Any other pending settings save goes to its file before files move.
            self.player.saver.flush();
            let items = targets
                .into_iter()
                .map(|(song_id, path)| ToDelete {
                    summary: self.library.summary(&path),
                    song_id,
                    path,
                })
                .collect();
            self.deleted.delete(items)
        });
        failed.extend(more);
        if !deleted.is_empty() {
            self.deleted_changed();
        }
        Ok(DeleteResult { deleted, failed, unloaded })
    }

    /// `library_restore(deleted_ids)`: back to their places in the Library.
    pub fn library_restore(&self, deleted_ids: &[String]) -> ApiResult<RestoreResult> {
        if deleted_ids.is_empty() {
            return Err(ApiError::bad_request("Choose at least one song to restore."));
        }
        let (restored, failed) = self.deleted.restore(deleted_ids);
        if !restored.is_empty() {
            self.deleted_changed();
        }
        Ok(RestoreResult { restored, failed })
    }

    /// `library_purge(deleted_ids | all)`: deletes for good (cannot be undone).
    pub fn library_purge(&self, deleted_ids: Option<&[String]>) -> ApiResult<Purged> {
        if deleted_ids.is_some_and(<[String]>::is_empty) {
            return Err(ApiError::bad_request("Choose at least one song to delete for good."));
        }
        let p = self.deleted.purge(deleted_ids);
        if !p.purged.is_empty() {
            self.deleted_changed();
        }
        Ok(p)
    }

    // ---------------------------------------------------------------- song file edits

    /// Writes a change into a song's settings slot; on a payload too large for the
    /// slot, rewrites the whole file (with the song released by the engine).
    fn edit_user_state(
        &self,
        song_id: &str,
        edit: impl Fn(&mut jk_format::UserState) + Clone,
    ) -> ApiResult<()> {
        let path = self.library.resolve(song_id)?;
        // (WO2) Never during a guide swap of this or any song (src/guide.rs).
        let _files = self.guide.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        // A pending mix change for this song goes first (both are read-merge-write).
        self.player.saver.flush();
        match jk_format::update_user_state(&path, edit.clone()) {
            Ok(_) => {}
            Err(e) if e.kind() == jk_format::ErrorKind::SlotPayloadTooLarge => {
                crate::applog::info(&format!("{song_id}: settings too large for the slot; rewriting the file"));
                self.player.with_song_released(&path, || {
                    let song = jk_format::SongFile::open(&path)?;
                    let mut state = song.user_state().clone();
                    drop(song);
                    edit(&mut state);
                    state.updated_utc = Some(jk_format::time::utc_now_iso8601());
                    jk_format::rewrite_song(
                        &path,
                        jk_format::Rewrite {
                            user_state: Some(state),
                            ..Default::default()
                        },
                    )?;
                    Ok(())
                })?;
            }
            Err(e) => return Err(e.into()),
        }
        self.player.refresh_current_from_disk(&path);
        Ok(())
    }

    /// `lyrics_save`: stores `lyrics_override` after checking the timing rules.
    pub fn lyrics_save(&self, song_id: &str, lyrics: Value) -> ApiResult<()> {
        let lyrics: jk_format::Lyrics = serde_json::from_value(lyrics)
            .map_err(|e| ApiError::bad_request(format!("These lyrics are not in the song file format: {e}")))?;
        let path = self.library.resolve(song_id)?;
        let duration = jk_format::SongFile::open(&path)?.manifest().duration();
        let problems = lyrics.validate(duration, "lyrics");
        if let Some(first) = problems.first() {
            return Err(ApiError::new(
                "invalid_lyrics",
                format!(
                    "The lyrics timing is not valid ({first}{}). Nothing was saved.",
                    if problems.len() > 1 { format!(", and {} more problems", problems.len() - 1) } else { String::new() }
                ),
            ));
        }
        self.edit_user_state(song_id, move |s| s.lyrics_override = Some(lyrics.clone()))
    }

    /// `lyrics_reset`: removes the override (the Studio's lyrics are used again).
    pub fn lyrics_reset(&self, song_id: &str) -> ApiResult<()> {
        self.edit_user_state(song_id, |s| s.lyrics_override = None)
    }

    /// `metadata_save`: `{ title?, artist?, album? }`; `album: null` = no album.
    pub fn metadata_save(&self, song_id: &str, changes: &Value) -> ApiResult<()> {
        let obj = changes
            .as_object()
            .ok_or_else(|| ApiError::bad_request("Metadata changes must be an object."))?;
        let text = |k: &str| -> ApiResult<Option<String>> {
            match obj.get(k) {
                None => Ok(None),
                Some(Value::String(v)) if !v.trim().is_empty() => Ok(Some(v.trim().to_string())),
                Some(_) => Err(ApiError::bad_request(format!("The {k} must be some text."))),
            }
        };
        let (title, artist) = (text("title")?, text("artist")?);
        let album: Option<Option<String>> = match obj.get("album") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(a)) => Some(Some(a.trim().to_string()).filter(|a| !a.is_empty())),
            Some(_) => return Err(ApiError::bad_request("The album must be some text or empty.")),
        };
        if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "title" | "artist" | "album")) {
            return Err(ApiError::bad_request(format!("Unknown metadata field '{k}'.")));
        }
        self.edit_user_state(song_id, move |s| {
            if let Some(t) = &title {
                s.metadata_override.title = Some(t.clone());
            }
            if let Some(a) = &artist {
                s.metadata_override.artist = Some(a.clone());
            }
            if let Some(al) = &album {
                s.metadata_override.album = Some(al.clone());
            }
        })
    }

    /// `studio_realign`: aligns corrected text with the Studio and stores the result as
    /// the song's `lyrics_override` (STUDIO_PROTOCOL section 8). Returns the lyrics.
    pub fn studio_realign(&self, song_id: &str, text: &str, anchors: &Value) -> ApiResult<Value> {
        let path = self.library.resolve(song_id)?;
        if text.trim().is_empty() {
            return Err(ApiError::bad_request("There is no text to align."));
        }
        let anchors = if anchors.is_null() { Value::Array(Vec::new()) } else { anchors.clone() };
        if !anchors.is_array() {
            return Err(ApiError::bad_request("anchors must be a list of { line, start }."));
        }
        let lyrics = self.studio.realign(&path, text, &anchors)?;
        self.lyrics_save(song_id, lyrics.clone())?;
        Ok(lyrics)
    }

    // ---------------------------------------------------------------- playback, guides (WO2)

    /// `song_load`: the player's load, with the guide's live state in the summary.
    pub fn song_load(&self, song_id: &str) -> ApiResult<SongDetail> {
        let mut d = self.player.load(song_id)?;
        d.summary.guide = self.guide.overlay(song_id, d.summary.guide.clone());
        Ok(d)
    }

    /// `guide_status(song_id)`.
    pub fn guide_status(&self, song_id: &str) -> ApiResult<GuideStatus> {
        self.guide.status(song_id)
    }

    /// `guide_build(song_id, force?)`.
    pub fn guide_build(&self, song_id: &str, force: bool) -> ApiResult<GuideStatus> {
        self.guide.build(song_id, force)
    }

    /// `guide_build_all({ include_outdated })`.
    pub fn guide_build_all(&self, include_outdated: bool) -> ApiResult<BatchQueued> {
        self.guide.build_all(include_outdated)
    }

    /// `guide_cancel(song_id?)`.
    pub fn guide_cancel(&self, song_id: Option<&str>) -> ApiResult<()> {
        if let Some(id) = song_id {
            self.library.resolve(id)?;
        }
        self.guide.cancel(song_id);
        Ok(())
    }

    /// `guide_queue()`.
    pub fn guide_queue(&self) -> GuideQueue {
        self.guide.queue()
    }

    // ---------------------------------------------------------------- export

    /// `export_backing`.
    pub fn export_backing(&self, song_id: &str, format: &str) -> ApiResult<ExportStarted> {
        let format = match format {
            "flac24" => ExportFormat::Flac24,
            "wav32f" => ExportFormat::Wav32f,
            _ => return Err(ApiError::bad_request("The export format must be \"flac24\" or \"wav32f\".")),
        };
        let path = self.library.resolve(song_id)?;
        let song = jk_format::SongFile::open(&path)?;
        let meta = song.effective_metadata();
        let key = crate::library::song_key(&song);
        // The loaded song's live settings win (they may not be saved yet).
        let settings = match self.player.current().filter(|c| c.path == path) {
            Some(c) => c.settings,
            None => song.effective_settings().clone(),
        };
        drop(song);
        let semitones = settings.semitones_clamped();
        self.exports.start(ExportRequest {
            song: path,
            artist: meta.artist,
            title: meta.title,
            key_text: crate::keys::export_key_text(key, semitones),
            params: MixParams::from_settings(&settings),
            format,
        })
    }
}

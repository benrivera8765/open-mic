// SPDX-License-Identifier: GPL-3.0-or-later
//! The melody guide queue (Work Order 2; docs/APP_API.md section 12.3,
//! docs/STUDIO_PROTOCOL.md section 12, docs/SONG_FORMAT.md section 7.1).
//!
//! Every song gets practice data with no step for the singer:
//!
//! - **New songs**: after the Studio's `job.done` the song enters this queue and the
//!   Studio job shows a last `guide` stage (src/studio.rs calls [`GuideService::enqueue_new_song`]
//!   and gets the stage's progress back through the job hook).
//! - **Existing songs**: `guide_build` (one song; Practice on a song without a guide) and
//!   `guide_build_all` ("Add practice data to all songs").
//!
//! One build at a time, on one below-normal-priority thread, never an audio thread. A
//! build reads the song's own lead vocal and lyrics (jk-guide, the same code as
//! `jukeoke-cli guide add`), writes the temporary file next to the song (the song may
//! keep playing: the file is only read), then swaps it in:
//!
//! - a song nobody has loaded: at once, under the song-file lock (no settings save, lyrics
//!   save or delete runs meanwhile) and the player's load lock (no `song_load` opens it);
//! - **the loaded song, not playing**: released exactly as the delete feature does
//!   (pending settings flushed, the engine closes every handle: D-231/D-233), renamed,
//!   loaded again at the same position (`Player::with_song_released`);
//! - **the loaded song, playing** (or during a practice take): the swap waits until it is
//!   paused, stopped, replaced or the app quits, so playback never stutters for a guide;
//!   meanwhile the guide is used from memory (`in_memory`, `pending_write`).
//!
//! A failure never touches the song: the file stays as it was and plays normally; the
//! status says `failed` with a plain reason and the code of STUDIO_PROTOCOL 12.2, and a
//! guide that was built but could not be written stays usable from memory for the session.
//!
//! The queue survives a restart: `<root>/Settings/guide-queue.json` (written atomically)
//! lists every song waiting for a build or a swap; at the next start they are built again
//! (the file is either the old one or the finished new one, SONG_FORMAT 7.1).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jk_format::guide::{GuideDoc, GuideFileState};
use jk_format::{PreparedGuide, RewriteControl};
use jk_guide::{Built, GuideError};
use jk_pitch::guide::GuideConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::events::{EventSink, emit, names};
use crate::library::Library;
use crate::playback::Player;

/// The queue file, inside the Settings folder.
pub const QUEUE_FILE: &str = "guide-queue.json";
/// `guide://status` progress events at most this often (5 a second).
const PROGRESS_EVERY: Duration = Duration::from_millis(200);
/// How often a waiting swap looks whether the song stopped playing.
const SWAP_POLL: Duration = Duration::from_millis(250);
/// A song file another program holds open is tried again this often, this many times.
const IN_USE_RETRY: Duration = Duration::from_secs(3);
const IN_USE_TRIES: u32 = 20;
/// Leftover temporary files of interrupted guide writes older than this are deleted.
const STALE_TEMP_AGE: Duration = Duration::from_secs(3600);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `GuideStatus.counts`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GuideCounts {
    pub notes: usize,
    pub pitched: usize,
    pub unpitched: usize,
    pub low_confidence: usize,
    pub octave_flags: usize,
    pub graded: usize,
}

/// docs/APP_API.md 12.3 `GuideStatus`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GuideStatus {
    /// none | queued | building | ready | outdated | stale | newer | failed
    pub state: &'static str,
    pub reason: Option<String>,
    pub error_code: Option<String>,
    pub progress: Option<f64>,
    pub in_memory: bool,
    pub pending_write: bool,
    pub method: Option<String>,
    pub version: Option<String>,
    pub created_utc: Option<String>,
    pub counts: Option<GuideCounts>,
    pub tuning_cents: Option<f64>,
}

/// Plain words for the states that need them (APP_API 12.3).
const REASON_NONE: &str = "Practice data has not been added to this song yet.";
const REASON_NEWER: &str = "This song's practice data was made by a newer version of Jukeoke. It is kept as it is; practice in this version makes its own copy for the session.";

impl GuideStatus {
    fn bare(state: &'static str) -> GuideStatus {
        GuideStatus {
            state,
            reason: None,
            error_code: None,
            progress: None,
            in_memory: false,
            pending_write: false,
            method: None,
            version: None,
            created_utc: None,
            counts: None,
            tuning_cents: None,
        }
    }

    /// `none`, as for a song that could not be read.
    pub fn none() -> GuideStatus {
        GuideStatus { reason: Some(REASON_NONE.into()), ..GuideStatus::bare("none") }
    }

    /// Fills provenance, counts and tuning from a guide document.
    fn with_doc(mut self, d: &GuideDoc) -> GuideStatus {
        let c = d.compute_counts();
        self.method = Some(d.provenance.method.clone());
        self.version = Some(d.provenance.version.clone());
        self.created_utc = d.provenance.created_utc.clone();
        self.counts = Some(GuideCounts {
            notes: c.notes,
            pitched: c.pitched,
            unpitched: c.unpitched,
            low_confidence: c.low_confidence,
            octave_flags: c.octave_flags,
            graded: c.graded,
        });
        self.tuning_cents = Some(d.tuning.use_cents);
        self
    }

    /// The guide's status as the file says (no queue): none / ready / outdated / stale /
    /// newer, or failed / `guide_no_lead` for a song with no lead vocal stem.
    pub fn from_file(song: &jk_format::SongFile) -> GuideStatus {
        if song.manifest().stem("lead_vocal").is_none() {
            let e = GuideError::no_lead();
            return GuideStatus {
                reason: Some(e.message),
                error_code: Some(e.code.into()),
                ..GuideStatus::bare("failed")
            };
        }
        // The library lists many songs: the curve is checked when it is read for practice
        // (and by `verify`), not on every scan.
        let (state, doc) = jk_format::guide::guide_state_opts(song, jk_guide::METHOD, jk_guide::VERSION, false);
        let base = match &state {
            GuideFileState::None => GuideStatus::none(),
            GuideFileState::Ready => GuideStatus::bare("ready"),
            GuideFileState::Outdated => GuideStatus::bare("outdated"),
            GuideFileState::Stale(r) => GuideStatus { reason: Some(r.clone()), ..GuideStatus::bare("stale") },
            GuideFileState::Newer => GuideStatus { reason: Some(REASON_NEWER.into()), ..GuideStatus::bare("newer") },
        };
        match &doc {
            Some(d) => base.with_doc(d),
            None => base,
        }
    }

    /// Usable for practice from the file as it is (ready, outdated, newer).
    fn usable_in_file(&self) -> bool {
        matches!(self.state, "ready" | "outdated" | "newer")
    }
}

/// `guide_queue()` result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GuideQueue {
    pub items: Vec<GuideQueueItem>,
    pub running: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GuideQueueItem {
    pub song_id: String,
    pub state: &'static str,
    pub progress: Option<f64>,
}

/// `guide_build_all` result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BatchQueued {
    pub queued: usize,
    pub skipped: usize,
}

/// `guide://batch` payload.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BatchEvent {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub current: Option<String>,
    pub running: bool,
}

/// Where a queue entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A new song after the Studio's `job.done`.
    Studio,
    /// `guide_build` (one song).
    User,
    /// `guide_build_all`.
    Batch,
}

/// One song waiting for a build or a swap (also the entry in guide-queue.json).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    song_id: String,
    #[serde(default)]
    force: bool,
    origin: Origin,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    added_utc: Option<String>,
    /// Builds tried for this entry in this session (a song that changed on disk while it
    /// was written is built once more).
    #[serde(skip)]
    attempts: u32,
}

#[derive(Serialize, Deserialize)]
struct QueueFile {
    queue_version: u32,
    items: Vec<Entry>,
}

/// What the Studio job of a new song is told (its `guide` stage).
pub type JobHook = Arc<dyn Fn(&str, &GuideStatus, bool) + Send + Sync>;
/// Whether a practice take holds the swap of a song (Stage 3 sets it).
type TakeHold = Option<String>;

struct Running {
    entry: Entry,
    cancel: Arc<AtomicBool>,
    progress: f64,
    last_emit: Instant,
}

struct Pending {
    entry: Entry,
    prepared: PreparedGuide,
    built: Arc<Built>,
    in_use_tries: u32,
    next_try: Instant,
}

#[derive(Default)]
struct Live {
    /// The last attempt failed: (code, plain message).
    failed: Option<(String, String)>,
    /// Built but not in the file (could not be written): used for this session.
    memory: Option<Arc<Built>>,
}

#[derive(Default)]
struct Batch {
    total: usize,
    done: usize,
    failed: usize,
    ids: HashSet<String>,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Entry>,
    running: Option<Running>,
    pending: HashMap<String, Pending>,
    /// A waiting swap being done right now (its status stays "in memory" until it ends).
    swapping: HashMap<String, Arc<Built>>,
    live: HashMap<String, Live>,
    batch: Option<Batch>,
    take: TakeHold,
    closing: bool,
}

/// The guide queue and its builder thread.
pub struct GuideService {
    library: Arc<Library>,
    player: Arc<Player>,
    sink: Arc<dyn EventSink>,
    queue_path: PathBuf,
    app: jk_format::guide::GuideApp,
    cfg: GuideConfig,
    /// Held while a song file is swapped; also by every other whole-file or slot write the
    /// core makes outside the player (lyrics, metadata), so none runs during a swap.
    pub file_lock: Arc<Mutex<()>>,
    state: Mutex<State>,
    wake: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
    job_hook: Mutex<Option<JobHook>>,
}

impl GuideService {
    pub fn new(
        library: Arc<Library>,
        player: Arc<Player>,
        sink: Arc<dyn EventSink>,
        settings_dir: &Path,
        app_name: &str,
        app_version: &str,
    ) -> Arc<GuideService> {
        let queue_path = settings_dir.join(QUEUE_FILE);
        let mut st = State::default();
        for mut e in load_queue(&queue_path) {
            if st.waiting.iter().all(|w| w.song_id != e.song_id) {
                e.attempts = 0;
                st.waiting.push_back(e);
            }
        }
        if !st.waiting.is_empty() {
            crate::applog::info(&format!("guide queue: {} songs left from the last session", st.waiting.len()));
        }
        Arc::new(GuideService {
            library,
            player,
            sink,
            queue_path,
            app: jk_guide::app_stamp(app_name, app_version),
            cfg: GuideConfig::default(),
            file_lock: Arc::new(Mutex::new(())),
            state: Mutex::new(st),
            wake: Condvar::new(),
            worker: Mutex::new(None),
            job_hook: Mutex::new(None),
        })
    }

    /// The Studio's `guide` stage is reported through this.
    pub fn set_job_hook(&self, hook: JobHook) {
        *lock(&self.job_hook) = Some(hook);
    }

    /// Starts the builder thread (after the window is up).
    pub fn start(self: &Arc<Self>) {
        let mut w = lock(&self.worker);
        if w.is_some() {
            return;
        }
        let me = Arc::clone(self);
        *w = std::thread::Builder::new()
            .name("guide-builder".into())
            .spawn(move || {
                below_normal_priority();
                me.run();
            })
            .ok();
    }

    /// On exit: cancels a running build, then writes the guides that waited for the song
    /// to stop (the player has closed the song by now). A guide that cannot be written now
    /// stays in guide-queue.json and is made again at the next start.
    pub fn shutdown(&self) {
        let handle = {
            let mut st = lock(&self.state);
            st.closing = true;
            if let Some(r) = &st.running {
                r.cancel.store(true, Ordering::SeqCst);
            }
            lock(&self.worker).take()
        };
        self.wake.notify_all();
        if let Some(h) = handle {
            let _ = h.join();
        }
        let pending: Vec<(String, Pending)> = lock(&self.state).pending.drain().collect();
        for (song_id, p) in pending {
            let Ok(path) = self.library.resolve(&song_id) else { continue };
            let _files = lock(&self.file_lock);
            match jk_guide::finish_write(p.prepared, &mut RewriteControl::default()) {
                Ok(()) => {
                    crate::applog::info(&format!("guide: {song_id}: written on exit"));
                    self.persist_remove(&song_id);
                }
                Err(e) => crate::applog::warn(&format!("guide: {song_id}: not written on exit ({e}); it is made again next time ({})", path.display())),
            }
        }
    }

    // ------------------------------------------------------------------ status

    /// `guide_status(song_id)`.
    pub fn status(&self, song_id: &str) -> ApiResult<GuideStatus> {
        let path = self.library.resolve(song_id)?;
        let file = self.library.summary(&path).guide;
        Ok(self.overlay(song_id, file))
    }

    /// The live state (queue, build, waiting swap, last failure) over the file's status.
    pub fn overlay(&self, song_id: &str, file: GuideStatus) -> GuideStatus {
        let st = lock(&self.state);
        if let Some(r) = st.running.as_ref().filter(|r| r.entry.song_id == song_id) {
            return GuideStatus { state: "building", progress: Some(r.progress), reason: None, error_code: None, ..file };
        }
        if st.waiting.iter().any(|e| e.song_id == song_id) {
            return GuideStatus { state: "queued", progress: None, reason: None, error_code: None, ..file };
        }
        if let Some(b) = st.pending.get(song_id).map(|p| &p.built).or_else(|| st.swapping.get(song_id)) {
            return GuideStatus { in_memory: true, pending_write: true, ..GuideStatus::bare("ready").with_doc(&b.doc) };
        }
        if let Some(l) = st.live.get(song_id)
            && let Some((code, msg)) = &l.failed
            && !file.usable_in_file()
        {
            let base = GuideStatus {
                reason: Some(msg.clone()),
                error_code: Some(code.clone()),
                in_memory: l.memory.is_some(),
                ..GuideStatus::bare("failed")
            };
            return match &l.memory {
                Some(b) => base.with_doc(&b.doc),
                None => base,
            };
        }
        file
    }

    /// The guide built for a song but not (yet) in its file, if any (practice uses it).
    pub fn memory_guide(&self, song_id: &str) -> Option<Arc<Built>> {
        let st = lock(&self.state);
        st.pending
            .get(song_id)
            .map(|p| Arc::clone(&p.built))
            .or_else(|| st.live.get(song_id).and_then(|l| l.memory.clone()))
    }

    /// A practice take on `song_id` holds its swap until the take ends (APP_API 12.3);
    /// `None` releases it. (Practice sessions arrive in Stage 3.)
    pub fn set_take_active(&self, song_id: Option<String>) {
        lock(&self.state).take = song_id;
        self.wake.notify_all();
    }

    /// `guide_queue()`.
    pub fn queue(&self) -> GuideQueue {
        let st = lock(&self.state);
        let mut items = Vec::new();
        if let Some(r) = &st.running {
            items.push(GuideQueueItem { song_id: r.entry.song_id.clone(), state: "building", progress: Some(r.progress) });
        }
        for e in &st.waiting {
            items.push(GuideQueueItem { song_id: e.song_id.clone(), state: "queued", progress: None });
        }
        GuideQueue { items, running: st.running.is_some() }
    }

    fn emit_status(&self, song_id: &str) {
        let status = match self.library.resolve(song_id) {
            Ok(path) => self.overlay(song_id, self.library.summary(&path).guide),
            Err(_) => self.overlay(song_id, GuideStatus::none()),
        };
        emit(&*self.sink, names::GUIDE_STATUS, &json!({"song_id": song_id, "status": status}));
        let job = {
            let st = lock(&self.state);
            st.running
                .as_ref()
                .filter(|r| r.entry.song_id == song_id)
                .and_then(|r| r.entry.job_id.clone())
                .or_else(|| st.waiting.iter().find(|e| e.song_id == song_id).and_then(|e| e.job_id.clone()))
        };
        if let Some(job_id) = job
            && let Some(h) = lock(&self.job_hook).clone()
        {
            h(&job_id, &status, false);
        }
    }

    fn emit_batch(&self) {
        let ev = {
            let st = lock(&self.state);
            let Some(b) = &st.batch else { return };
            let running = st.running.as_ref().is_some_and(|r| b.ids.contains(&r.entry.song_id))
                || st.waiting.iter().any(|e| b.ids.contains(&e.song_id));
            BatchEvent {
                total: b.total,
                done: b.done,
                failed: b.failed,
                current: st.running.as_ref().filter(|r| b.ids.contains(&r.entry.song_id)).map(|r| r.entry.song_id.clone()),
                running,
            }
        };
        emit(&*self.sink, names::GUIDE_BATCH, &ev);
        if !ev.running {
            lock(&self.state).batch = None;
        }
    }

    // ------------------------------------------------------------------ requests

    /// `guide_build(song_id, force?)`: queues a build (first in line: someone is waiting
    /// for this song) and returns at once. A song already queued or building gets its
    /// status, not a second build; a current guide is not rebuilt unless `force`; a guide
    /// made by a newer Jukeoke is never overwritten.
    pub fn build(&self, song_id: &str, force: bool) -> ApiResult<GuideStatus> {
        let path = self.library.resolve(song_id)?;
        {
            let mut st = lock(&self.state);
            if st.running.as_ref().is_some_and(|r| r.entry.song_id == song_id)
                || st.waiting.iter().any(|e| e.song_id == song_id)
            {
                drop(st);
                return self.status(song_id);
            }
            if st.pending.contains_key(song_id) {
                if !force {
                    drop(st);
                    return self.status(song_id);
                }
                st.pending.remove(song_id); // the temporary file is deleted with it
            }
        }
        let file = self.library.summary(&path).guide;
        let wanted = match file.state {
            "none" | "stale" | "outdated" => true,
            "failed" => file.error_code.as_deref() != Some("guide_no_lead"),
            "ready" => force && !newer_builder(&file),
            _ => false, // newer: kept
        };
        if !wanted {
            lock(&self.state).live.remove(song_id);
            return self.status(song_id);
        }
        {
            let mut st = lock(&self.state);
            st.live.remove(song_id);
            st.waiting.push_front(Entry {
                song_id: song_id.into(),
                force,
                origin: Origin::User,
                job_id: None,
                added_utc: Some(jk_format::time::utc_now_iso8601()),
                attempts: 0,
            });
        }
        self.persist();
        self.wake.notify_all();
        self.emit_status(song_id);
        self.status(song_id)
    }

    /// `guide_build_all({ include_outdated })`: queues every song whose guide is none, stale
    /// or failed (and outdated when asked). Damaged songs, songs with no lead vocal and
    /// guides made by a newer Jukeoke are skipped.
    pub fn build_all(&self, include_outdated: bool) -> ApiResult<BatchQueued> {
        let list = self.library.list()?;
        let mut queued = Vec::new();
        let mut skipped = 0;
        {
            let st = lock(&self.state);
            for s in &list.songs {
                let busy = st.running.as_ref().is_some_and(|r| r.entry.song_id == s.song_id)
                    || st.waiting.iter().any(|e| e.song_id == s.song_id)
                    || st.pending.contains_key(&s.song_id);
                let live = self.overlay_locked(&st, &s.song_id, s.guide.clone());
                let wanted = s.error.is_none()
                    && !busy
                    && match live.state {
                        "none" | "stale" => true,
                        "failed" => live.error_code.as_deref() != Some("guide_no_lead"),
                        "outdated" => include_outdated,
                        _ => false,
                    };
                if wanted {
                    queued.push(s.song_id.clone());
                } else {
                    skipped += 1;
                }
            }
        }
        if queued.is_empty() {
            return Ok(BatchQueued { queued: 0, skipped });
        }
        {
            let mut st = lock(&self.state);
            let now = jk_format::time::utc_now_iso8601();
            for id in &queued {
                st.live.remove(id);
                st.waiting.push_back(Entry {
                    song_id: id.clone(),
                    force: false,
                    origin: Origin::Batch,
                    job_id: None,
                    added_utc: Some(now.clone()),
                    attempts: 0,
                });
            }
            let b = st.batch.get_or_insert_with(Batch::default);
            b.total += queued.len();
            b.ids.extend(queued.iter().cloned());
        }
        self.persist();
        self.wake.notify_all();
        for id in &queued {
            self.emit_status(id);
        }
        self.emit_batch();
        Ok(BatchQueued { queued: queued.len(), skipped })
    }

    fn overlay_locked(&self, st: &State, song_id: &str, file: GuideStatus) -> GuideStatus {
        if let Some(l) = st.live.get(song_id)
            && let Some((code, msg)) = &l.failed
            && !file.usable_in_file()
        {
            return GuideStatus { reason: Some(msg.clone()), error_code: Some(code.clone()), ..GuideStatus::bare("failed") };
        }
        file
    }

    /// A new song from the Studio (STUDIO_PROTOCOL 12.2 step 1).
    pub fn enqueue_new_song(&self, song_id: &str, job_id: &str) {
        {
            let mut st = lock(&self.state);
            st.live.remove(song_id);
            st.waiting.retain(|e| e.song_id != song_id);
            st.waiting.push_back(Entry {
                song_id: song_id.into(),
                force: true, // a new file: whatever it holds, the guide is made from it
                origin: Origin::Studio,
                job_id: Some(job_id.into()),
                added_utc: Some(jk_format::time::utc_now_iso8601()),
                attempts: 0,
            });
        }
        self.persist();
        self.wake.notify_all();
        self.emit_status(song_id);
    }

    /// `guide_cancel(song_id?)`: one song, or everything. A cancelled build leaves the file
    /// as it was.
    pub fn cancel(&self, song_id: Option<&str>) {
        let mut touched = Vec::new();
        let mut jobs = Vec::new();
        {
            let mut st = lock(&self.state);
            let hit = |id: &str| song_id.is_none_or(|s| s == id);
            let removed: Vec<Entry> = {
                let (out, keep): (Vec<Entry>, Vec<Entry>) = st.waiting.drain(..).partition(|e| hit(&e.song_id));
                st.waiting = keep.into();
                out
            };
            for e in removed {
                if let Some(j) = &e.job_id {
                    jobs.push((j.clone(), e.song_id.clone()));
                }
                touched.push(e.song_id);
            }
            if let Some(r) = st.running.as_ref().filter(|r| hit(&r.entry.song_id)) {
                r.cancel.store(true, Ordering::SeqCst);
            }
            let pend: Vec<String> = st.pending.keys().filter(|k| hit(k)).cloned().collect();
            for k in pend {
                if let Some(p) = st.pending.remove(&k)
                    && let Some(j) = p.entry.job_id
                {
                    jobs.push((j, k.clone()));
                }
                touched.push(k);
            }
            for id in &touched {
                st.live.insert(id.clone(), Live { failed: Some(("cancelled".into(), "Cancelled.".into())), memory: None });
                if let Some(b) = st.batch.as_mut()
                    && b.ids.remove(id)
                {
                    b.total = b.total.saturating_sub(1);
                }
            }
        }
        if !touched.is_empty() {
            self.persist();
        }
        for id in &touched {
            self.emit_status(id);
        }
        let hook = lock(&self.job_hook).clone();
        for (job, id) in jobs {
            if let (Some(h), Ok(st)) = (&hook, self.status(&id)) {
                h(&job, &st, true);
            }
        }
        self.emit_batch();
    }

    /// Before a song file is moved away (deleted): its build is cancelled and a waiting
    /// swap dropped (with its temporary file).
    pub fn forget(&self, song_id: &str) {
        let busy = {
            let st = lock(&self.state);
            st.running.as_ref().is_some_and(|r| r.entry.song_id == song_id)
                || st.waiting.iter().any(|e| e.song_id == song_id)
                || st.pending.contains_key(song_id)
        };
        if busy {
            self.cancel(Some(song_id));
        }
        lock(&self.state).live.remove(song_id);
    }

    // ------------------------------------------------------------------ the queue file

    /// Every song waiting for a build or a swap, in order.
    fn persist(&self) {
        let items: Vec<Entry> = {
            let st = lock(&self.state);
            let mut v: Vec<Entry> = Vec::new();
            if let Some(r) = &st.running {
                v.push(r.entry.clone());
            }
            v.extend(st.pending.values().map(|p| p.entry.clone()));
            v.extend(st.waiting.iter().cloned());
            v
        };
        let file = QueueFile { queue_version: 1, items };
        match serde_json::to_vec_pretty(&file) {
            Ok(b) => {
                if let Err(e) = crate::fs_atomic::write_atomic(&self.queue_path, &b) {
                    crate::applog::warn(&format!("guide queue: cannot write {}: {e}", self.queue_path.display()));
                }
            }
            Err(e) => crate::applog::error(&format!("guide queue: {e}")),
        }
    }

    fn persist_remove(&self, _song_id: &str) {
        self.persist();
    }

    // ------------------------------------------------------------------ the builder thread

    fn run(self: Arc<Self>) {
        loop {
            let next = {
                let mut st = lock(&self.state);
                loop {
                    if st.closing {
                        return;
                    }
                    if let Some(e) = st.waiting.pop_front() {
                        st.running = Some(Running {
                            entry: e.clone(),
                            cancel: Arc::new(AtomicBool::new(false)),
                            progress: 0.0,
                            last_emit: Instant::now() - PROGRESS_EVERY,
                        });
                        break Some(e);
                    }
                    let wait = if st.pending.is_empty() { Duration::from_secs(2) } else { SWAP_POLL };
                    st = self.wake.wait_timeout(st, wait).unwrap_or_else(|e| e.into_inner()).0;
                    if !st.pending.is_empty() && st.waiting.is_empty() {
                        break None;
                    }
                }
            };
            self.try_pending_swaps();
            if let Some(e) = next {
                self.build_one(e);
                self.try_pending_swaps();
            }
        }
    }

    fn set_progress(&self, song_id: &str, p: f64) {
        let due = {
            let mut st = lock(&self.state);
            match st.running.as_mut().filter(|r| r.entry.song_id == song_id) {
                Some(r) => {
                    r.progress = (p.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
                    if r.last_emit.elapsed() >= PROGRESS_EVERY {
                        r.last_emit = Instant::now();
                        true
                    } else {
                        false
                    }
                }
                None => false,
            }
        };
        if due {
            self.emit_status(song_id);
        }
    }

    fn build_one(&self, entry: Entry) {
        let song_id = entry.song_id.clone();
        let t0 = Instant::now();
        self.emit_status(&song_id);
        self.emit_batch();
        let cancel = lock(&self.state).running.as_ref().map(|r| Arc::clone(&r.cancel)).unwrap_or_default();
        let path = match self.library.resolve(&song_id) {
            Ok(p) => p,
            Err(_) => {
                // The song is gone (deleted or moved): drop it.
                crate::applog::info(&format!("guide: {song_id} is no longer in the library; dropped from the queue"));
                self.finish(&entry, Outcome::Gone);
                return;
            }
        };
        if let Some(dir) = path.parent() {
            clean_stale_temps(dir);
        }
        // Nothing to do for a current guide (unless forced).
        let file = self.library.summary(&path).guide;
        let wanted = match file.state {
            "none" | "stale" | "outdated" => true,
            "failed" => file.error_code.as_deref() != Some("guide_no_lead"),
            "ready" => entry.force && !newer_builder(&file),
            _ => false,
        };
        if file.error_code.as_deref() == Some("guide_no_lead") {
            self.finish(&entry, Outcome::Failed(GuideError::no_lead(), None));
            return;
        }
        if !wanted {
            self.finish(&entry, Outcome::Unchanged);
            return;
        }
        // 1. Build from the file (it is only read: the song may play meanwhile).
        let built = {
            let mut progress = |p: f64| self.set_progress(&song_id, 0.4 * p);
            let mut ctl = jk_guide::BuildControl { cancel: Some(&cancel), progress: Some(&mut progress) };
            jk_guide::build_for_song(&path, &self.cfg, &self.app, &mut ctl)
        };
        let built = match built {
            Ok(b) => Arc::new(b),
            Err(e) => {
                self.finish(&entry, Outcome::Failed(e, None));
                return;
            }
        };
        // 2. The temporary file next to the song.
        let prepared = {
            let mut progress = |done: u64, total: u64| {
                self.set_progress(&song_id, 0.4 + 0.55 * done as f64 / total.max(1) as f64);
            };
            let mut ctl = RewriteControl { cancel: Some(&cancel), progress: Some(&mut progress), fail_at: None };
            jk_guide::prepare_write(&path, &built, &mut ctl)
        };
        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                // Built but not written: usable from memory for this session.
                let mem = (e.code != "cancelled").then(|| Arc::clone(&built));
                self.finish(&entry, Outcome::Failed(e, mem));
                return;
            }
        };
        crate::applog::info(&format!(
            "guide: {song_id}: built in {:.2} s ({} notes), temporary file written after {:.2} s",
            built.seconds,
            built.doc.notes.len(),
            t0.elapsed().as_secs_f64()
        ));
        // 3. The swap, now or when the song stops playing.
        let p = Pending { entry: entry.clone(), prepared, built, in_use_tries: 0, next_try: Instant::now() };
        match self.swap(&song_id, &path, p) {
            SwapResult::Done => self.finish(&entry, Outcome::Written),
            SwapResult::Waiting(p) => {
                let mut st = lock(&self.state);
                st.running = None;
                st.pending.insert(song_id.clone(), p);
                if let Some(b) = st.batch.as_mut()
                    && b.ids.contains(&song_id)
                {
                    b.done += 1;
                }
                drop(st);
                crate::applog::info(&format!("guide: {song_id}: in memory; written when the song stops playing"));
                self.persist();
                self.emit_status(&song_id);
                self.emit_batch();
            }
            SwapResult::Rebuild => {
                // The song changed on disk while it was written: once more, from the new file.
                let mut again = entry.clone();
                again.attempts += 1;
                let mut st = lock(&self.state);
                st.running = None;
                if again.attempts < 2 {
                    st.waiting.push_front(again);
                    drop(st);
                    self.persist();
                } else {
                    drop(st);
                    let e = GuideError {
                        code: "guide_internal",
                        message: "The song file kept changing while its practice data was being made. Press Try again.".into(),
                        detail: "changed on disk twice".into(),
                    };
                    self.finish(&entry, Outcome::Failed(e, None));
                }
            }
            SwapResult::Failed(e, mem) => self.finish(&entry, Outcome::Failed(e, mem)),
        }
    }

    /// Whether the song is loaded and playing, or a take holds it.
    fn swap_blocked(&self, song_id: &str, path: &Path) -> bool {
        if lock(&self.state).take.as_deref() == Some(song_id) {
            return true;
        }
        match self.player.current() {
            Some(c) if c.path == path => self.player.state().state == "playing",
            _ => false,
        }
    }

    /// Steps 3-5 of SONG_FORMAT 7.1 for one song, when it may be swapped now.
    fn swap(&self, song_id: &str, path: &Path, p: Pending) -> SwapResult {
        if self.swap_blocked(song_id, path) {
            return SwapResult::Waiting(p);
        }
        let file_length = p.prepared.file_length;
        let _files = lock(&self.file_lock);
        // With the song released by the player (when it is the loaded one: the engine
        // closes its handles first), the file must be free of every other program too
        // (Windows cannot replace an open file); if not, it is tried again shortly.
        let mut slot = Some(p);
        let r = self.player.with_song_released(path, || {
            // The engine closes its handles just after the unload (D-231): wait for that,
            // at most 2 s, as the delete feature does.
            if !crate::deleted::wait_until_closed(path, Duration::from_secs(2)) || !file_is_free(path) {
                return Ok(Inner::Busy);
            }
            let p = slot.take().expect("taken once");
            let built = Arc::clone(&p.built);
            Ok(match jk_guide::finish_write(p.prepared, &mut RewriteControl::default()) {
                Ok(()) => Inner::Done,
                Err(e) => Inner::Failed(e, built),
            })
        });
        match r {
            Ok(Inner::Done) => SwapResult::Done,
            Ok(Inner::Busy) => {
                let p = slot.take().expect("not consumed when busy");
                if p.in_use_tries + 1 >= IN_USE_TRIES {
                    // "In use" on every system (a raw Windows code would read as EPIPE on
                    // macOS): access denied at the swap is jk_guide's guide_in_use.
                    let e = jk_guide::write_error(
                        &jk_format::FormatError::Io(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "the song file is still open in another program",
                        )),
                        jk_guide::WriteStage::Swap,
                        file_length,
                    );
                    let built = Arc::clone(&p.built);
                    return SwapResult::Failed(e, Some(built));
                }
                SwapResult::Waiting(Pending { in_use_tries: p.in_use_tries + 1, next_try: Instant::now() + IN_USE_RETRY, ..p })
            }
            Ok(Inner::Failed(e, _)) if e.detail.contains("changed on disk") => SwapResult::Rebuild,
            Ok(Inner::Failed(e, built)) => SwapResult::Failed(e, Some(built)),
            Err(e) => {
                let built = slot.map(|p| p.built);
                SwapResult::Failed(GuideError::internal(e.message), built)
            }
        }
    }

    /// Swaps the guides that waited, when their song stopped playing.
    fn try_pending_swaps(&self) {
        let ready: Vec<String> = {
            let st = lock(&self.state);
            st.pending.iter().filter(|(_, p)| p.next_try <= Instant::now()).map(|(k, _)| k.clone()).collect()
        };
        for song_id in ready {
            let Ok(path) = self.library.resolve(&song_id) else {
                lock(&self.state).pending.remove(&song_id);
                self.persist();
                continue;
            };
            if self.swap_blocked(&song_id, &path) {
                continue;
            }
            let Some(p) = ({
                let mut st = lock(&self.state);
                let p = st.pending.remove(&song_id);
                if let Some(p) = &p {
                    st.swapping.insert(song_id.clone(), Arc::clone(&p.built));
                }
                p
            }) else {
                continue;
            };
            let entry = p.entry.clone();
            let result = self.swap(&song_id, &path, p);
            {
                // Back to the waiting list or settled below, in one step with the
                // end of the swap, so the status never falls back in between.
                let mut st = lock(&self.state);
                st.swapping.remove(&song_id);
                if let SwapResult::Waiting(p) = result {
                    st.pending.insert(song_id, p);
                    continue;
                }
                if let SwapResult::Failed(e, mem) = &result {
                    st.live.insert(song_id.clone(), Live { failed: Some((e.code.into(), e.message.clone())), memory: mem.clone() });
                }
            }
            match result {
                SwapResult::Done => {
                    crate::applog::info(&format!("guide: {song_id}: written now that the song stopped"));
                    self.finish_pending(&entry, Outcome::Written);
                }
                SwapResult::Waiting(_) => unreachable!("handled above"),
                SwapResult::Rebuild => {
                    let mut again = entry.clone();
                    again.attempts += 1;
                    lock(&self.state).waiting.push_front(again);
                    self.persist();
                    self.wake.notify_all();
                }
                SwapResult::Failed(e, mem) => self.finish_pending(&entry, Outcome::Failed(e, mem)),
            }
        }
    }

    fn finish_pending(&self, entry: &Entry, o: Outcome) {
        self.settle(entry, o, false);
    }

    fn finish(&self, entry: &Entry, o: Outcome) {
        self.settle(entry, o, true);
    }

    /// Records how a build ended, updates the queue file and tells everyone.
    fn settle(&self, entry: &Entry, o: Outcome, was_running: bool) {
        let song_id = &entry.song_id;
        let failed = matches!(o, Outcome::Failed(..));
        {
            let mut st = lock(&self.state);
            if was_running {
                st.running = None;
            }
            match &o {
                Outcome::Failed(e, mem) => {
                    crate::applog::warn(&format!("guide: {song_id} failed: {e}"));
                    st.live.insert(song_id.clone(), Live { failed: Some((e.code.into(), e.message.clone())), memory: mem.clone() });
                }
                Outcome::Written | Outcome::Unchanged => {
                    st.live.remove(song_id);
                }
                Outcome::Gone => {
                    st.live.remove(song_id);
                }
            }
            if was_running
                && let Some(b) = st.batch.as_mut()
                && b.ids.contains(song_id)
            {
                b.done += 1;
                if failed {
                    b.failed += 1;
                }
            }
        }
        self.persist();
        if !matches!(o, Outcome::Gone) {
            self.emit_status(song_id);
        }
        if matches!(o, Outcome::Written) {
            self.sink.emit_value(names::LIBRARY_CHANGED, Value::Null);
        }
        if let Some(job_id) = &entry.job_id
            && let Some(h) = lock(&self.job_hook).clone()
        {
            let status = match o {
                Outcome::Gone => GuideStatus {
                    reason: Some("The song file was moved or deleted before its practice data was made.".into()),
                    error_code: Some("guide_read_failed".into()),
                    ..GuideStatus::bare("failed")
                },
                _ => self.status(song_id).unwrap_or_else(|_| GuideStatus::none()),
            };
            h(job_id, &status, true);
        }
        if was_running {
            self.emit_batch();
        }
    }
}

enum Outcome {
    Written,
    Unchanged,
    Gone,
    Failed(GuideError, Option<Arc<Built>>),
}

/// What happened inside the release window of a swap.
enum Inner {
    Done,
    Busy,
    Failed(GuideError, Arc<Built>),
}

enum SwapResult {
    Done,
    Waiting(Pending),
    Rebuild,
    Failed(GuideError, Option<Arc<Built>>),
}

/// A ready guide from a newer version of this builder is never downgraded.
fn newer_builder(s: &GuideStatus) -> bool {
    s.method.as_deref() == Some(jk_guide::METHOD)
        && s.version.as_deref().is_some_and(|v| {
            jk_format::guide::compare_versions(v, jk_guide::VERSION) == std::cmp::Ordering::Greater
        })
}

fn load_queue(path: &Path) -> Vec<Entry> {
    match std::fs::read(path) {
        Ok(b) => match serde_json::from_slice::<QueueFile>(&b) {
            Ok(q) => q.items,
            Err(e) => {
                crate::applog::warn(&format!("guide queue: {} is not readable ({e}); kept as .bad", path.display()));
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

/// Deletes `*.jksong.tmp-*` leftovers older than an hour in a song's folder (an
/// interrupted write; SONG_FORMAT 7.1 step 6).
fn clean_stale_temps(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.contains(".jksong.tmp-") || name.contains(".JKSONG.tmp-")) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP_AGE);
        if old && std::fs::remove_file(e.path()).is_ok() {
            crate::applog::info(&format!("guide: removed an unfinished write {}", e.path().display()));
        }
    }
}

/// Whether no other program holds the file open (Windows cannot replace an open file).
fn file_is_free(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new().read(true).share_mode(0).open(path) {
            Ok(_) => true,
            Err(e) => e.raw_os_error() != Some(32) && e.raw_os_error() != Some(33),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

/// The builder never competes with playback for the processor.
fn below_normal_priority() {
    #[cfg(windows)]
    // SAFETY: GetCurrentThread returns a pseudo handle for this thread; changing its own
    // priority has no other effect.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// `guide_build` with an unknown song id and similar: plain errors.
pub fn not_found() -> ApiError {
    ApiError::new("not_found", "This song is not in the library.")
}

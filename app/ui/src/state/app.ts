// SPDX-License-Identifier: GPL-3.0-or-later
// Application state and the actions that change it. Screens read slices with
// `useApp(selector)` and call the functions exported here; only this module talks
// to the API, so every command's error handling and optimistic update lives in one
// place.
import type { Api } from "../api/client";
import { errorMessage } from "../api/client";
import type {
  AppInfo,
  AppSettings,
  AppSettingsChanges,
  DeletedList,
  Device,
  GuideBatchEvent,
  GuideStatus,
  LibraryList,
  Lyrics,
  MediaPlayers,
  PlayerState,
  ProposedExtensions,
  RecordedSong,
  Recording,
  RecorderStatus,
  RecordingSession,
  SongDetail,
  SongSettings,
  StudioJob,
  StudioStatus,
} from "../api/types";
import { clampKey } from "../lib/keys";
import { exportGainNotice, formatBytes } from "../lib/format";
import { DEFAULT_PRACTICE_SETTINGS, lookOf } from "../api/defaults";
import { PlaybackClock } from "../lib/clock";
import { smokeProgress } from "../lib/smoke";
import { Store, useSelector } from "./store";
import { initPractice } from "./practice";

export type Screen = "library" | "stage" | "sync" | "recorder" | "studio" | "settings";
export type Overlay = null | "help" | "selfcheck";

/** One "Export backing track" job as the UI shows it. */
export interface ExportJob {
  progress: number;
  path: string | null;
  error: string | null;
  /** Set when the core scaled the file down to avoid clipping (`applied_gain_db` < 0). */
  notice: string | null;
}

export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
}

export interface AppState {
  ready: boolean;
  fatal: string | null;
  info: AppInfo | null;
  settings: AppSettings | null;
  library: LibraryList | null;
  libraryLoading: boolean;
  /** Deleted songs (the Library's "Deleted songs" view); null until asked. */
  deleted: DeletedList | null;
  /** A confirmation dialog is open: the app-wide shortcuts stand aside. */
  modal: boolean;
  song: SongDetail | null;
  /** Live song settings (optimistic: updated before the core confirms). */
  songSettings: SongSettings | null;
  player: (PlayerState & ProposedExtensions) | null;
  devices: Device[];
  recorder: RecorderStatus | null;
  recordings: Recording[];
  /** Media players the recorder can start and follow (Windows; null until asked). */
  players: MediaPlayers | null;
  /** "Record songs separately" sessions, newest first. */
  recordingSessions: RecordingSession[];
  /** The recording being previewed (played natively), if any. */
  previewPath: string | null;
  studio: StudioStatus | null;
  /** Files waiting in the Studio panel (dropped, picked, or sent from the recorder). */
  studioInbox: string[];
  screen: Screen;
  /** Stage shown alone (full screen). */
  fullscreen: boolean;
  overlay: Overlay;
  toasts: Toast[];
  /** Bumped when the library search box should take focus (Ctrl/Cmd+F). */
  searchFocus: number;
  /** Export jobs by id. */
  exports: Record<string, ExportJob>;
  /** The sync editor's tap-along owns Space/Enter/Esc while true. */
  tapMode: boolean;
  /** WO2: "Add practice data to all songs" while it runs (and its last report). */
  guideBatch: GuideBatchEvent | null;
}

export const store = new Store<AppState>({
  ready: false,
  fatal: null,
  info: null,
  settings: null,
  library: null,
  libraryLoading: false,
  deleted: null,
  modal: false,
  song: null,
  songSettings: null,
  player: null,
  devices: [],
  recorder: null,
  recordings: [],
  players: null,
  recordingSessions: [],
  previewPath: null,
  studio: null,
  studioInbox: [],
  screen: "library",
  fullscreen: false,
  overlay: null,
  toasts: [],
  searchFocus: 0,
  exports: {},
  tapMode: false,
  guideBatch: null,
});

export const useApp = <T>(selector: (s: AppState) => T): T => useSelector(store, selector);

/** The one playback clock (lib/clock.ts), fed by `playback://clock` events. */
export const clock = new PlaybackClock();

/** Diagnostics hook for scripts/real-app-check.mjs and screenshots.mjs (read-only use). */
declare global {
  interface Window {
    __jkDebug?: { clock: PlaybackClock; store: Store<AppState>; clockEvents: { at: number; position_s: number; playing: boolean }[] };
  }
}

/** Listeners that want a nudge whenever a clock event arrives (the ticker's frame loop). */
const clockKicks = new Set<() => void>();
export function onClockKick(f: () => void): () => void {
  clockKicks.add(f);
  return () => clockKicks.delete(f);
}
function kick(): void {
  for (const f of [...clockKicks]) f();
}

let api: Api | null = null;
export function getApiSync(): Api {
  if (!api) throw new Error("API not initialised");
  return api;
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

let toastId = 1;
export function toast(text: string, kind: Toast["kind"] = "info", durationMs?: number): void {
  const id = toastId++;
  store.set((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, text }] }));
  setTimeout(() => dismissToast(id), durationMs ?? (kind === "error" ? 9000 : 4000));
}
export function dismissToast(id: number): void {
  store.set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}
/** Runs a command that returns data; shows its plain-language error as a toast. Undefined on failure. */
async function attempt<T>(p: Promise<T>, context?: string): Promise<T | undefined> {
  try {
    return await p;
  } catch (err) {
    toast(context ? `${context}: ${errorMessage(err)}` : errorMessage(err), "error");
    return undefined;
  }
}

/** Runs a command that returns nothing; true when it succeeded (errors shown as a toast). */
async function succeeded(p: Promise<unknown>, context?: string): Promise<boolean> {
  try {
    await p;
    return true;
  } catch (err) {
    toast(context ? `${context}: ${errorMessage(err)}` : errorMessage(err), "error");
    return false;
  }
}

// ---------------------------------------------------------------------------
// Start-up
// ---------------------------------------------------------------------------

export async function init(theApi: Api): Promise<void> {
  api = theApi;
  // Events first, so nothing that happens during start-up is missed.
  const debug = { clock, store, clockEvents: [] as { at: number; position_s: number; playing: boolean }[] };
  if (typeof window !== "undefined") window.__jkDebug = debug;
  api.on("playback://clock", (ev) => {
    const now = performance.now();
    // Keep the last few hundred events for diagnostics (bounded memory).
    if (debug.clockEvents.length > 600) debug.clockEvents.splice(0, 300);
    debug.clockEvents.push({ at: now, position_s: ev.position_s, playing: ev.playing });
    clock.onEvent(ev, now);
    kick();
  });
  api.on("playback://state", (ps) => {
    store.set({ player: ps });
    if (ps.state === "ended" || ps.state === "error" || ps.state === "paused") kick();
  });
  api.on("playback://error", (e) => toast(e.message, "error"));
  api.on("library://changed", () => void refreshLibrary());
  api.on("library://deleted", (d) => store.set({ deleted: d }));
  api.on("guide://status", ({ song_id, status }) => applyGuideStatus(song_id, status));
  api.on("guide://batch", (b) => store.set({ guideBatch: b.running ? b : null }));
  api.on("recorder://level", (r) => store.set({ recorder: r }));
  api.on("recorder://session", (sess) => upsertRecordingSession(sess));
  api.on("recorder://preview", ({ path, playing }) =>
    store.set((s) => ({ previewPath: playing ? path : s.previewPath === path ? null : s.previewPath })),
  );
  api.on("studio://job", (job) => updateJob(job));
  api.on("studio://dropped", ({ paths }) => addToStudio(paths));
  api.on("studio://status", (st) => applyStudioStatus(st));
  api.on("export://progress", ({ job_id, progress }) =>
    store.set((s) => ({ exports: { ...s.exports, [job_id]: { progress, path: null, error: null, notice: null } } })),
  );
  api.on("export://done", ({ job_id, path, applied_gain_db }) => {
    // A FLAC export that had to be lowered to avoid clipping says so, plainly, both in a
    // toast and next to the saved path in the mixer (WORK_ORDER 5: no hidden processing).
    const notice = exportGainNotice(applied_gain_db, path);
    store.set((s) => ({ exports: { ...s.exports, [job_id]: { progress: 1, path, error: null, notice } } }));
    if (notice) toast(`Exported: ${path}. ${notice}`, "info", 15000);
    else toast(`Backing track saved: ${path}`);
  });
  api.on("export://failed", ({ job_id, message }) => {
    store.set((s) => ({ exports: { ...s.exports, [job_id]: { progress: 0, path: null, error: message, notice: null } } }));
    toast(message, "error");
  });
  // WO2: practice mode's events (state/practice.ts).
  initPractice(api);

  try {
    const [info, settings] = await Promise.all([api.appInfo(), api.settingsGet()]);
    store.set({ info, settings, fullscreen: false });
  } catch (err) {
    store.set({ fatal: errorMessage(err) });
    return;
  }
  store.set({ ready: true });
  smokeProgress("ui: app_info and settings_get answered; first screen ready");
  // Core addition: the user agent goes into `--selfcheck` reports.
  api.selfcheckUserAgent?.(navigator.userAgent).catch(() => undefined);
  // `--smoke-test` progress (D-279): each step says when it is done, and a start-up still
  // waiting after 15 s says on what.
  const pending = new Set<string>();
  const step = (name: string, p: Promise<unknown>): Promise<unknown> => {
    pending.add(name);
    return p.finally(() => {
      pending.delete(name);
      smokeProgress(`ui: ${name} done`);
    });
  };
  const slow = setTimeout(() => smokeProgress(`ui: start-up still waiting after 15 s for: ${[...pending].join(", ")}`), 15000);
  await Promise.all([
    step("library_list", refreshLibrary()),
    step("devices_list", attempt(api.devicesList()).then((r) => r && store.set({ devices: r.devices }))),
    step("player_state", attempt(api.playerState()).then((p) => p && store.set({ player: p }))),
    step("recorder_status", attempt(api.recorderStatus()).then((r) => r && store.set({ recorder: r }))),
    step("recordings_list", refreshRecordings()),
    step("recorder_sessions", refreshRecordingSessions()),
    step("studio_status", refreshStudio()),
  ]);
  clearTimeout(slow);
}

export async function refreshLibrary(): Promise<void> {
  if (!api) return;
  store.set({ libraryLoading: true });
  const lib = await attempt(api.libraryList(), "Could not read the library");
  store.set({ libraryLoading: false, ...(lib ? { library: lib } : {}) });
}

// ---------------------------------------------------------------------------
// Practice data (the melody guide; WO2, APP_API 12.3). The core makes it; the Library
// shows whether a song has it and offers to add it (one song, or every song).
// ---------------------------------------------------------------------------

/** States in which practice is not ready and adding practice data can help. */
export function guideNeedsAdding(g: GuideStatus | undefined): boolean {
  if (!g) return false;
  if (g.state === "failed") return g.error_code !== "guide_no_lead";
  return g.state === "none" || g.state === "stale";
}

/** Practice can start from what the song holds (or from memory). */
export function guideUsable(g: GuideStatus | undefined): boolean {
  return !!g && (g.state === "ready" || g.state === "outdated" || g.state === "newer" || g.in_memory);
}

/** A `guide://status` arrives: patch the song in the library (no rescan) and the loaded one. */
export function applyGuideStatus(songId: string, status: GuideStatus): void {
  store.set((s) => {
    const lib = s.library;
    const songs = lib?.songs.map((x) => (x.song_id === songId ? { ...x, guide: status } : x));
    const song = s.song?.summary.song_id === songId ? { ...s.song, summary: { ...s.song.summary, guide: status } } : s.song;
    return { ...(lib && songs ? { library: { ...lib, songs } } : {}), song };
  });
}

/** "Add practice data" for one song (`force`: make it again). */
export async function buildGuide(songId: string, force = false): Promise<void> {
  if (!api?.guideBuild) return;
  const st = await attempt(api.guideBuild(songId, force), "Practice data could not be added");
  if (st) applyGuideStatus(songId, st);
}

/** "Add practice data to all songs". */
export async function buildAllGuides(): Promise<void> {
  if (!api?.guideBuildAll) return;
  const r = await attempt(api.guideBuildAll(false), "Practice data could not be added");
  if (r && r.queued === 0) toast("Every song already has its practice data.");
}

/** Stops adding practice data (every song still waiting, and the one being made). */
export async function cancelGuides(): Promise<void> {
  if (!api?.guideCancel) return;
  await attempt(api.guideCancel(), "Could not stop");
}

// ---------------------------------------------------------------------------
// Deleting songs (the owner's request of 2026-11-18; APP_API section 2). A delete moves
// the song file to Deleted songs, from where it is restored or deleted for good. The
// screens ask first (Library.tsx); these only carry out what was confirmed.
// ---------------------------------------------------------------------------

/** "Artist - Title" (or the title alone) for messages. */
export function songLabel(s: { title: string; artist: string }): string {
  const title = s.title || "Untitled";
  return s.artist ? `${s.artist} - ${title}` : title;
}

export async function refreshDeleted(): Promise<void> {
  if (!api) return;
  const d = await attempt(api.libraryDeletedList(), "Could not read Deleted songs");
  if (d) store.set({ deleted: d });
}

/** Moves songs to Deleted songs; true when at least one went. */
export async function deleteSongs(songIds: string[]): Promise<boolean> {
  if (!api || songIds.length === 0) return false;
  const before = store.get().library?.songs ?? [];
  const r = await attempt(api.libraryDelete(songIds), "Nothing was deleted");
  if (!r) return false;
  if (r.unloaded && store.get().song?.summary.song_id === r.unloaded) {
    // The core stopped and closed it before moving the file.
    clock.onEvent({ position_s: 0, playing: false }, performance.now());
    const screen = store.get().screen;
    store.set({ song: null, songSettings: null, ...(screen === "stage" || screen === "sync" ? { screen: "library" as Screen } : {}) });
    kick();
  }
  for (const f of r.failed) {
    const s = before.find((x) => x.song_id === f.id);
    toast(`${s ? songLabel(s) : f.id}: ${f.message}`, "error");
  }
  if (r.deleted.length === 1) toast(`“${songLabel(r.deleted[0])}” moved to Deleted songs.`);
  else if (r.deleted.length > 1) toast(`${r.deleted.length} songs moved to Deleted songs.`);
  await Promise.all([refreshLibrary(), refreshDeleted()]);
  return r.deleted.length > 0;
}

/** Puts deleted songs back into the Library. */
export async function restoreSongs(deletedIds: string[]): Promise<boolean> {
  if (!api || deletedIds.length === 0) return false;
  const before = store.get().deleted?.songs ?? [];
  const r = await attempt(api.libraryRestore(deletedIds), "Nothing was restored");
  if (!r) return false;
  for (const f of r.failed) {
    const s = before.find((x) => x.deleted_id === f.id);
    toast(`${s ? songLabel(s) : f.id}: ${f.message}`, "error");
  }
  if (r.restored.length === 1) {
    const s = before.find((x) => x.deleted_id === r.restored[0].deleted_id);
    toast(`“${s ? songLabel(s) : r.restored[0].song_id}” is back in the library.`);
  } else if (r.restored.length > 1) toast(`${r.restored.length} songs are back in the library.`);
  await Promise.all([refreshLibrary(), refreshDeleted()]);
  return r.restored.length > 0;
}

/** Deletes for good (cannot be undone): some deleted songs, or "all". */
export async function purgeSongs(which: string[] | "all"): Promise<boolean> {
  if (!api || (which !== "all" && which.length === 0)) return false;
  const before = store.get().deleted?.songs ?? [];
  const r = await attempt(api.libraryPurge(which), "Nothing was deleted");
  if (!r) return false;
  for (const f of r.failed) {
    const s = before.find((x) => x.deleted_id === f.id);
    toast(`${s ? songLabel(s) : f.id}: ${f.message}`, "error");
  }
  if (r.purged.length > 0) {
    const what = r.purged.length === 1 ? (() => {
      const s = before.find((x) => x.deleted_id === r.purged[0]);
      return s ? `“${songLabel(s)}”` : "1 song";
    })() : `${r.purged.length} songs`;
    toast(`${what} deleted for good (${formatBytes(r.freed_bytes)} freed).`);
  }
  await Promise.all([refreshLibrary(), refreshDeleted()]);
  return r.purged.length > 0;
}

export function setModal(on: boolean): void {
  store.set({ modal: on });
}

export async function refreshRecordings(): Promise<void> {
  if (!api) return;
  const r = await attempt(api.recordingsList());
  if (r) store.set({ recordings: r.recordings });
}

/** Media players for "Record & play" and "Record songs separately" (quietly: polled). */
export async function refreshPlayers(): Promise<void> {
  if (!api?.recorderMediaSessions) return;
  try {
    store.set({ players: await api.recorderMediaSessions() });
  } catch {
    // The list is only advice; the buttons report real errors.
  }
}

export async function refreshRecordingSessions(): Promise<void> {
  if (!api?.recorderSessionsList) return;
  const r = await attempt(api.recorderSessionsList());
  if (r) store.set({ recordingSessions: r.sessions });
}

function upsertRecordingSession(sess: RecordingSession): void {
  store.set((s) => {
    const rest = s.recordingSessions.filter((x) => x.folder !== sess.folder);
    const all = [sess, ...rest].sort((a, b) => (a.folder < b.folder ? 1 : a.folder > b.folder ? -1 : 0));
    return { recordingSessions: all };
  });
  // A finished split may have produced new files for the Studio list.
  if (sess.state === "split" || sess.state === "kept_master") void refreshRecordings();
}

/**
 * Studio hand-off for recorded songs: one Studio job per song, with the title, artist,
 * album and cover the player reported (the sidecar) - the same `studio_submit` the
 * Studio form uses, with the cover given explicitly (`metadata.cover_path`).
 * Returns how many were queued.
 */
export async function makeKaraokeSongs(songs: RecordedSong[], lyrics?: ({ mode: "auto" | "text"; text: string | null; lrc: null } | undefined)[]): Promise<number> {
  if (!api || songs.length === 0) return 0;
  let queued = 0;
  for (const [k, song] of songs.entries()) {
    try {
      await api.studioSubmit({
        input: song.path,
        preset: "full",
        language: "en",
        metadata: {
          title: song.title.trim() || song.name.replace(/\.[^.]+$/, ""),
          artist: song.artist.trim(),
          album: song.album.trim() || null,
          ...(song.cover_path ? { cover_path: song.cover_path } : {}),
        },
        lyrics: lyrics?.[k] ?? { mode: "auto", text: null, lrc: null },
      });
      queued += 1;
    } catch (err) {
      toast(`${song.title || song.name}: ${errorMessage(err)}`, "error");
    }
  }
  if (queued > 0) {
    toast(`Added ${queued} song${queued === 1 ? "" : "s"} to the Studio queue.`);
    void refreshStudio();
  }
  return queued;
}

/** Plays (or stops) a preview of a recording, natively. */
export async function togglePreview(path: string): Promise<void> {
  if (!api?.recorderPreview) return;
  if (store.get().previewPath === path) {
    await succeeded(api.recorderPreviewStop?.() ?? Promise.resolve());
    store.set({ previewPath: null });
    return;
  }
  if (await succeeded(api.recorderPreview(path, 0, 30), "Preview")) store.set({ previewPath: path });
}

export async function refreshStudio(): Promise<void> {
  if (!api) return;
  const st = await attempt(api.studioStatus());
  if (st) applyStudioStatus(st);
}

/**
 * A Studio status from the core (`studio://status` event or `studio_status()`).
 *
 * `info.capabilities.studio` comes from `app_info()` at start-up, which is before the
 * background probe has answered (about 10 s on the Studio PC). It used to be kept as it
 * was, so on a capable PC it said "checking" for good: the Studio screen showed "Still
 * checking whether this computer can run the Studio..." while the queue ran, and the
 * recorder's "make a karaoke song" and the sync editor's realign stayed off (found by
 * the end-to-end run in the real app, docs/benchmarks/end-to-end.md). Availability,
 * reasons and `checking` are now copied over at once; the GPU name and memory exist only
 * in `app_info()`, which is fetched again when the probe's answer changes them.
 */
function applyStudioStatus(st: StudioStatus): void {
  const prev = store.get().info?.capabilities.studio;
  store.set((s) => ({
    studio: st,
    ...(s.info
      ? { info: { ...s.info, capabilities: { ...s.info.capabilities, studio: { ...s.info.capabilities.studio, available: st.available, reasons: st.reasons, checking: st.checking ?? false } } } }
      : {}),
  }));
  if (api && prev && !st.checking && (prev.checking === true || prev.available !== st.available)) {
    void attempt(api.appInfo()).then((info) => info && store.set({ info }));
  }
}

function updateJob(job: StudioJob): void {
  store.set((s) => {
    if (!s.studio) return {};
    const queue = s.studio.queue.some((j) => j.job_id === job.job_id)
      ? s.studio.queue.map((j) => (j.job_id === job.job_id ? job : j))
      : [...s.studio.queue, job];
    return { studio: { ...s.studio, queue, running: queue.some((j) => j.state === "running") } };
  });
  if (job.state === "done") void refreshLibrary();
}

// ---------------------------------------------------------------------------
// Navigation and overlays
// ---------------------------------------------------------------------------

export function go(screen: Screen): void {
  store.set({ screen });
}
export function setOverlay(overlay: Overlay): void {
  store.set({ overlay });
}
export function focusSearch(): void {
  store.set((s) => ({ screen: "library", fullscreen: false, searchFocus: s.searchFocus + 1 }));
}

export async function setFullscreen(on: boolean): Promise<void> {
  if (on) store.set({ screen: store.get().screen === "sync" ? "sync" : "stage" });
  store.set({ fullscreen: on });
  if (api) await api.setWindowFullscreen(on);
}

// ---------------------------------------------------------------------------
// Settings (optimistic, deep-merged locally, saved by the core)
// ---------------------------------------------------------------------------

function mergeSettings(base: AppSettings, ch: AppSettingsChanges): AppSettings {
  const { look: chLook, ...chUi } = ch.ui ?? {};
  return {
    ...base,
    ui: {
      ...base.ui,
      ...chUi,
      colors: { ...base.ui.colors, ...(ch.ui?.colors ?? {}) },
      ...(base.ui.look || chLook ? { look: { ...lookOf(base.ui), ...(chLook ?? {}) } } : {}),
    },
    audio: { ...base.audio, ...(ch.audio ?? {}), device_offsets_ms: { ...base.audio.device_offsets_ms, ...(ch.audio?.device_offsets_ms ?? {}) } },
    // WO2 (APP_API 12.7): practice settings, one level deep (the maps are written by the core).
    ...(base.practice || ch.practice ? { practice: { ...DEFAULT_PRACTICE_SETTINGS, ...(base.practice ?? {}), ...(ch.practice ?? {}) } } : {}),
  };
}

let pendingSettings: AppSettingsChanges | null = null;
let settingsTimer: ReturnType<typeof setTimeout> | null = null;

/** Applies a settings change now; sends it to the core (debounced for sliders). */
export function changeSettings(changes: AppSettingsChanges, debounceMs = 0): void {
  const cur = store.get().settings;
  if (!cur) return;
  store.set({ settings: mergeSettings(cur, changes) });
  pendingSettings = pendingSettings ? mergeChanges(pendingSettings, changes) : changes;
  if (settingsTimer) clearTimeout(settingsTimer);
  settingsTimer = setTimeout(flushSettings, debounceMs);
}

function mergeChanges(a: AppSettingsChanges, b: AppSettingsChanges): AppSettingsChanges {
  return {
    ui:
      a.ui || b.ui
        ? {
            ...(a.ui ?? {}),
            ...(b.ui ?? {}),
            ...(a.ui?.colors || b.ui?.colors ? { colors: { ...(a.ui?.colors ?? {}), ...(b.ui?.colors ?? {}) } } : {}),
            ...(a.ui?.look || b.ui?.look ? { look: { ...(a.ui?.look ?? {}), ...(b.ui?.look ?? {}) } } : {}),
          }
        : undefined,
    audio:
      a.audio || b.audio
        ? { ...(a.audio ?? {}), ...(b.audio ?? {}), ...(a.audio?.device_offsets_ms || b.audio?.device_offsets_ms ? { device_offsets_ms: { ...(a.audio?.device_offsets_ms ?? {}), ...(b.audio?.device_offsets_ms ?? {}) } } : {}) }
        : undefined,
    practice: a.practice || b.practice ? { ...(a.practice ?? {}), ...(b.practice ?? {}) } : undefined,
  };
}

async function flushSettings(): Promise<void> {
  settingsTimer = null;
  const ch = pendingSettings;
  pendingSettings = null;
  if (!ch || !api) return;
  const saved = await attempt(api.settingsSet(ch), "Settings were not saved");
  // Keep local edits made while the request was in flight.
  if (saved) store.set({ settings: pendingSettings ? mergeSettings(saved, pendingSettings) : saved });
}

/** Key under which the per-device Bluetooth offset is stored: the device id, or "default". */
export function deviceOffsetKey(s: AppSettings | null): string {
  return s?.audio.device_id ?? "default";
}
export function deviceOffsetMs(s: AppSettings | null): number {
  if (!s) return 0;
  return s.audio.device_offsets_ms[deviceOffsetKey(s)] ?? 0;
}

// ---------------------------------------------------------------------------
// Songs and playback
// ---------------------------------------------------------------------------

export async function loadSong(songId: string, opts: { play?: boolean; screen?: Screen } = {}): Promise<boolean> {
  if (!api) return false;
  // Changes to the current song go to the current song, before it is replaced.
  await flushSongNow();
  store.set({ player: store.get().player ? { ...store.get().player!, state: "loading" } : null });
  const detail = await attempt(api.songLoad(songId), "This song could not be opened");
  if (!detail) {
    void attempt(api.playerState()).then((p) => p && store.set({ player: p }));
    return false;
  }
  clock.onEvent({ position_s: 0, playing: false }, performance.now());
  store.set({ song: detail, songSettings: detail.settings, screen: opts.screen ?? "stage" });
  kick();
  if (opts.play) await play();
  return true;
}

export async function play(): Promise<void> {
  if (!api) return;
  clock.release();
  await attempt(api.playerPlay());
}
export async function pause(): Promise<void> {
  if (!api) return;
  clock.hold(performance.now());
  kick();
  const ok = await succeeded(api.playerPause());
  if (!ok) clock.release();
}
export async function togglePlay(): Promise<void> {
  const st = store.get().player?.state;
  if (st === "playing") await pause();
  else if (store.get().song) await play();
}
let lastSeekAt = Number.NEGATIVE_INFINITY;
/** Milliseconds since the last seek this UI asked for (the key-preparation note uses it, D-215). */
export function msSinceSeek(): number {
  return performance.now() - lastSeekAt;
}
export async function seek(positionS: number): Promise<void> {
  if (!api || !store.get().song) return;
  const dur = store.get().song!.summary.duration_s;
  const p = Math.max(0, Math.min(dur, positionS));
  lastSeekAt = performance.now();
  clock.jump(p, performance.now());
  kick();
  await attempt(api.playerSeek(p));
}
export function seekBy(deltaS: number): Promise<void> {
  return seek(clock.position(performance.now()) + deltaS);
}
export async function restart(): Promise<void> {
  await seek(0);
}

// Song settings: applied locally at once, sent to the core coalesced (the core applies
// them live and saves them into the song file about a second later).
let pendingSong: Partial<SongSettings> | null = null;
let songInFlight = false;
let songTimer: ReturnType<typeof setTimeout> | null = null;

export function changeSongSettings(changes: Partial<SongSettings>, throttleMs = 0): void {
  const cur = store.get().songSettings;
  if (!cur) return;
  const next: SongSettings = { ...cur, ...changes, levels: { ...cur.levels, ...(changes.levels ?? {}) } };
  store.set({ songSettings: next });
  pendingSong = { ...(pendingSong ?? {}), ...changes, ...(changes.levels ? { levels: next.levels } : {}) };
  if (songTimer) return;
  songTimer = setTimeout(() => {
    songTimer = null;
    void flushSong();
  }, throttleMs);
}

let songFlight: Promise<unknown> | null = null;

async function flushSong(): Promise<void> {
  if (songInFlight || !pendingSong || !api) return;
  const ch = pendingSong;
  pendingSong = null;
  songInFlight = true;
  songFlight = attempt(api.playerSet(ch), "The change was not applied");
  await songFlight;
  songFlight = null;
  songInFlight = false;
  if (pendingSong) void flushSong();
}

/**
 * Sends a throttled song-settings change at once and waits until the core has it.
 * Called before another song is loaded: otherwise a fader moved just before opening
 * the next song could reach the core after song_load and be applied to (and saved
 * into) the wrong song.
 */
async function flushSongNow(): Promise<void> {
  if (songTimer) {
    clearTimeout(songTimer);
    songTimer = null;
  }
  for (let guard = 0; guard < 10 && (songFlight || pendingSong); guard++) {
    if (songFlight) await songFlight;
    else await flushSong();
  }
}

/** Key control: move by `delta` semitones (clamped to -12..+12). */
export function shiftKey(delta: number): void {
  const s = store.get().songSettings;
  if (!s) return;
  const n = clampKey(s.semitones + delta);
  if (n !== s.semitones) changeSongSettings({ semitones: n });
}
export function setKey(semitones: number): void {
  const s = store.get().songSettings;
  if (!s) return;
  const n = clampKey(semitones);
  if (n !== s.semitones) changeSongSettings({ semitones: n });
}

export async function setMaster(volume: number): Promise<void> {
  if (!api) return;
  const v = Math.max(0, Math.min(1, volume));
  const cur = store.get().settings;
  if (cur) store.set({ settings: { ...cur, audio: { ...cur.audio, master_volume: v } } });
  await attempt(api.playerSetMaster(v));
}

export async function chooseDevice(id: string | null, exclusive: boolean): Promise<void> {
  if (!api) return;
  const ok = await succeeded(api.deviceSet(id, exclusive), "Could not switch the output");
  if (!ok) return;
  const cur = store.get().settings;
  if (cur) store.set({ settings: { ...cur, audio: { ...cur.audio, device_id: id, exclusive } } });
  const p = await attempt(api.playerState());
  if (p) store.set({ player: p });
}

export async function refreshDevices(): Promise<void> {
  if (!api) return;
  const r = await attempt(api.devicesList());
  if (r) store.set({ devices: r.devices });
}

// ---------------------------------------------------------------------------
// Sync editor and metadata
// ---------------------------------------------------------------------------

export async function saveLyrics(lyrics: Lyrics): Promise<boolean> {
  const song = store.get().song;
  if (!api || !song) return false;
  const ok = await succeeded(api.lyricsSave(song.summary.song_id, lyrics), "The lyrics were not saved");
  if (!ok) return false;
  store.set({ song: { ...song, lyrics, lyrics_is_override: true, summary: { ...song.summary, lyrics_checked: lyrics.checked, has_lyrics: true } } });
  toast("Lyrics saved in the song file.");
  return true;
}

/**
 * Removes the corrected lyrics. The API has no "original lyrics" query, so the song
 * is reloaded (that pauses it) and returned to the same position.
 */
export async function resetLyrics(): Promise<boolean> {
  const song = store.get().song;
  if (!api || !song) return false;
  const ok = await succeeded(api.lyricsReset(song.summary.song_id), "The lyrics were not reset");
  if (!ok) return false;
  const pos = clock.position(performance.now());
  if (await loadSong(song.summary.song_id, { screen: store.get().screen })) await seek(pos);
  toast("The song's original lyrics are back.");
  return true;
}

export async function saveMetadata(changes: { title?: string; artist?: string; album?: string | null }): Promise<boolean> {
  const song = store.get().song;
  if (!api || !song) return false;
  const ok = await succeeded(api.metadataSave(song.summary.song_id, changes), "The details were not saved");
  if (!ok) return false;
  store.set({ song: { ...song, summary: { ...song.summary, ...changes } } });
  void refreshLibrary();
  return true;
}

export async function exportBacking(format: "flac24" | "wav32f"): Promise<void> {
  const song = store.get().song;
  if (!api || !song) return;
  const r = await attempt(api.exportBacking(song.summary.song_id, format), "Export failed");
  if (r) store.set((s) => ({ exports: { ...s.exports, [r.job_id]: { progress: 0, path: null, error: null, notice: null } } }));
}

// ---------------------------------------------------------------------------
// Studio inbox
// ---------------------------------------------------------------------------

export function addToStudio(paths: string[]): void {
  if (paths.length === 0) return;
  store.set((s) => ({ studioInbox: [...s.studioInbox, ...paths.filter((p) => !s.studioInbox.includes(p))], screen: "studio", fullscreen: false }));
}
export function removeFromStudio(path: string): void {
  store.set((s) => ({ studioInbox: s.studioInbox.filter((p) => p !== path) }));
}

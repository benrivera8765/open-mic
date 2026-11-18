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
  Device,
  LibraryList,
  Lyrics,
  PlayerState,
  ProposedExtensions,
  Recording,
  RecorderStatus,
  SongDetail,
  SongSettings,
  StudioJob,
  StudioStatus,
} from "../api/types";
import { clampKey } from "../lib/keys";
import { exportGainNotice } from "../lib/format";
import { PlaybackClock } from "../lib/clock";
import { Store, useSelector } from "./store";

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
  song: SongDetail | null;
  /** Live song settings (optimistic: updated before the core confirms). */
  songSettings: SongSettings | null;
  player: (PlayerState & ProposedExtensions) | null;
  devices: Device[];
  recorder: RecorderStatus | null;
  recordings: Recording[];
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
}

export const store = new Store<AppState>({
  ready: false,
  fatal: null,
  info: null,
  settings: null,
  library: null,
  libraryLoading: false,
  song: null,
  songSettings: null,
  player: null,
  devices: [],
  recorder: null,
  recordings: [],
  studio: null,
  studioInbox: [],
  screen: "library",
  fullscreen: false,
  overlay: null,
  toasts: [],
  searchFocus: 0,
  exports: {},
  tapMode: false,
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
  api.on("recorder://level", (r) => store.set({ recorder: r }));
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

  try {
    const [info, settings] = await Promise.all([api.appInfo(), api.settingsGet()]);
    store.set({ info, settings, fullscreen: false });
  } catch (err) {
    store.set({ fatal: errorMessage(err) });
    return;
  }
  store.set({ ready: true });
  // Core addition: the user agent goes into `--selfcheck` reports.
  api.selfcheckUserAgent?.(navigator.userAgent).catch(() => undefined);
  await Promise.all([
    refreshLibrary(),
    attempt(api.devicesList()).then((r) => r && store.set({ devices: r.devices })),
    attempt(api.playerState()).then((p) => p && store.set({ player: p })),
    attempt(api.recorderStatus()).then((r) => r && store.set({ recorder: r })),
    refreshRecordings(),
    refreshStudio(),
  ]);
}

export async function refreshLibrary(): Promise<void> {
  if (!api) return;
  store.set({ libraryLoading: true });
  const lib = await attempt(api.libraryList(), "Could not read the library");
  store.set({ libraryLoading: false, ...(lib ? { library: lib } : {}) });
}

export async function refreshRecordings(): Promise<void> {
  if (!api) return;
  const r = await attempt(api.recordingsList());
  if (r) store.set({ recordings: r.recordings });
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
  return {
    ui: { ...base.ui, ...(ch.ui ?? {}), colors: { ...base.ui.colors, ...(ch.ui?.colors ?? {}) } },
    audio: { ...base.audio, ...(ch.audio ?? {}), device_offsets_ms: { ...base.audio.device_offsets_ms, ...(ch.audio?.device_offsets_ms ?? {}) } },
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
    ui: a.ui || b.ui ? { ...(a.ui ?? {}), ...(b.ui ?? {}), ...(a.ui?.colors || b.ui?.colors ? { colors: { ...(a.ui?.colors ?? {}), ...(b.ui?.colors ?? {}) } } : {}) } : undefined,
    audio:
      a.audio || b.audio
        ? { ...(a.audio ?? {}), ...(b.audio ?? {}), ...(a.audio?.device_offsets_ms || b.audio?.device_offsets_ms ? { device_offsets_ms: { ...(a.audio?.device_offsets_ms ?? {}), ...(b.audio?.device_offsets_ms ?? {}) } } : {}) }
        : undefined,
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
export async function seek(positionS: number): Promise<void> {
  if (!api || !store.get().song) return;
  const dur = store.get().song!.summary.duration_s;
  const p = Math.max(0, Math.min(dur, positionS));
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

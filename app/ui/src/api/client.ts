// SPDX-License-Identifier: GPL-3.0-or-later
// The one door between the UI and the Rust core (docs/APP_API.md).
//
// `Api` is the interface every screen uses. Two implementations exist:
//   - `TauriApi` (this file): Tauri `invoke` for commands, `listen` for events;
//   - `MockApi` (mock.ts): a browser-only fake with a demo library and a simulated
//     clock, so `npm run dev` works in a normal browser and tests need no Rust.
// `getApi()` picks the Tauri one inside the app and the mock everywhere else.
//
// Argument names: APP_API says arguments are snake_case JSON objects. Tauri 2
// renames Rust snake_case parameters to camelCase unless the command is declared
// with `#[tauri::command(rename_all = "snake_case")]`; the core must do that.
// Where APP_API shows an object literal as a positional parameter, the name used for
// it is in ARG_NAMES below (one place to change if the core picks other names).
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApiError,
  ApiEventName,
  ApiEvents,
  AppInfo,
  AppSettings,
  AppSettingsChanges,
  CoverResult,
  Device,
  ExportFormat,
  InspectResult,
  LibraryList,
  LookupResult,
  LyricsLookupQuery,
  Lyrics,
  MetadataChanges,
  PlayerState,
  RealignAnchor,
  Recording,
  RecorderStatus,
  SelfCheckResult,
  SongDetail,
  SongSettings,
  StudioModelsStatus,
  StudioStatus,
  StudioSubmitRequest,
} from "./types";

/** Names of the object parameters APP_API does not name explicitly. */
export const ARG_NAMES = {
  metadata_save: "metadata",
  export_backing: "options",
  studio_lyrics_lookup: "query",
  studio_submit: "job",
} as const;

export type Unsubscribe = () => void;

/** Everything the UI can ask of the core. Mirrors docs/APP_API.md section by section. */
export interface Api {
  /** "tauri" inside the app, "mock" in a plain browser or a test. */
  readonly kind: "tauri" | "mock";

  // 1. App
  appInfo(): Promise<AppInfo>;
  // 2. Library
  libraryList(): Promise<LibraryList>;
  songCover(songId: string, maxPx: number): Promise<CoverResult>;
  // 3. Playback
  songLoad(songId: string): Promise<SongDetail>;
  playerPlay(): Promise<void>;
  playerPause(): Promise<void>;
  playerToggle(): Promise<void>;
  playerStop(): Promise<void>;
  playerSeek(positionS: number): Promise<void>;
  playerSet(changes: Partial<SongSettings>): Promise<void>;
  playerSetMaster(volume: number): Promise<void>;
  playerState(): Promise<PlayerState>;
  // 4. Devices
  devicesList(): Promise<{ devices: Device[] }>;
  deviceSet(id: string | null, exclusive: boolean): Promise<void>;
  // 5. Settings
  settingsGet(): Promise<AppSettings>;
  settingsSet(changes: AppSettingsChanges): Promise<AppSettings>;
  // 6. Sync editor and metadata
  lyricsSave(songId: string, lyrics: Lyrics): Promise<void>;
  lyricsReset(songId: string): Promise<void>;
  metadataSave(songId: string, changes: MetadataChanges): Promise<void>;
  studioRealign(songId: string, text: string, anchors: RealignAnchor[]): Promise<Lyrics>;
  // 7. Export
  exportBacking(songId: string, format: ExportFormat): Promise<{ job_id: string }>;
  // 8. Recorder
  recorderStatus(): Promise<RecorderStatus>;
  recorderStart(): Promise<void>;
  recorderStop(): Promise<Recording>;
  recorderRequestPermission(): Promise<void>;
  recordingsList(): Promise<{ recordings: Recording[] }>;
  // 9. Studio
  studioStatus(): Promise<StudioStatus>;
  studioInspect(path: string): Promise<InspectResult>;
  studioLyricsLookup(query: LyricsLookupQuery): Promise<{ results: LookupResult[] }>;
  studioSubmit(request: StudioSubmitRequest): Promise<{ job_id: string }>;
  studioCancel(jobId: string): Promise<void>;
  studioModelsStatus(): Promise<StudioModelsStatus>;
  studioModelsDownload(): Promise<void>;
  pickFiles(): Promise<string[]>;
  // 10. Self-check (`silent`: digital silence instead of the -60 dBFS tone; a core addition)
  selfcheckRun(userAgent: string, silent?: boolean): Promise<SelfCheckResult>;

  // Additions the app core implements beyond APP_API v1 (optional here).
  /** macOS translocation: native folder picker for the stick's main folder, then restart. */
  rootChoose?(): Promise<{ ok: boolean; root: string | null; message: string }>;
  /** Tells the core the web engine's user agent (used by `--selfcheck` runs). */
  selfcheckUserAgent?(userAgent: string): Promise<void>;
  /** Stage 0 `--smoke-test` report; the core rejects it when not in that mode. */
  sendSmokeReport?(report: unknown): Promise<void>;

  /** Subscribe to a core event. */
  on<E extends ApiEventName>(event: E, handler: (payload: ApiEvents[E]) => void): Unsubscribe;

  /**
   * Window full screen (Tauri window API inside the app, the Fullscreen API in a
   * browser). Resolves false when the platform refused; the UI then still shows the
   * stage-only layout inside the window.
   */
  setWindowFullscreen(on: boolean): Promise<boolean>;

  /**
   * Calls `cb(true)` while the native window is minimised and `cb(false)` when it
   * is back (Tauri only). The jukebox skin stops its light drift and bubbles then:
   * a web view does not always report a minimised window as a hidden page.
   */
  onWindowMinimised?(cb: (minimised: boolean) => void): Unsubscribe;

  /**
   * True when files dropped on the page arrive as HTML5 drop events with no real
   * path (browser mock). Inside Tauri, drops arrive as `studio://dropped` instead.
   */
  readonly htmlDropGivesPaths: boolean;
  /** Mock only: turn browser `File`s into queue entries. */
  acceptBrowserFiles?(names: string[]): string[];
}

/** Turns whatever a rejected invoke carried into the contract's `{ code, message }`. */
export function toApiError(err: unknown): ApiError {
  if (err && typeof err === "object") {
    const o = err as Record<string, unknown>;
    if (typeof o.message === "string") {
      return { code: typeof o.code === "string" ? o.code : "error", message: o.message };
    }
  }
  if (typeof err === "string") return { code: "error", message: err };
  return { code: "error", message: "Something went wrong (no details were given)." };
}

/** Plain message for showing an error to the user. */
export function errorMessage(err: unknown): string {
  return toApiError(err).message;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    throw toApiError(err);
  }
}

/** The real thing: Tauri commands and events. */
export class TauriApi implements Api {
  readonly kind = "tauri" as const;
  readonly htmlDropGivesPaths = false;

  appInfo = () => call<AppInfo>("app_info");
  libraryList = () => call<LibraryList>("library_list");
  songCover = (song_id: string, max_px: number) => call<CoverResult>("song_cover", { song_id, max_px });
  songLoad = (song_id: string) => call<SongDetail>("song_load", { song_id });
  playerPlay = () => call<void>("player_play");
  playerPause = () => call<void>("player_pause");
  playerToggle = () => call<void>("player_toggle");
  playerStop = () => call<void>("player_stop");
  playerSeek = (position_s: number) => call<void>("player_seek", { position_s });
  playerSet = (changes: Partial<SongSettings>) => call<void>("player_set", { changes });
  playerSetMaster = (volume: number) => call<void>("player_set_master", { volume });
  playerState = () => call<PlayerState>("player_state");
  devicesList = () => call<{ devices: Device[] }>("devices_list");
  deviceSet = (id: string | null, exclusive: boolean) => call<void>("device_set", { id, exclusive });
  settingsGet = () => call<AppSettings>("settings_get");
  settingsSet = (changes: AppSettingsChanges) => call<AppSettings>("settings_set", { changes });
  lyricsSave = (song_id: string, lyrics: Lyrics) => call<void>("lyrics_save", { song_id, lyrics });
  lyricsReset = (song_id: string) => call<void>("lyrics_reset", { song_id });
  metadataSave = (song_id: string, changes: MetadataChanges) =>
    call<void>("metadata_save", { song_id, [ARG_NAMES.metadata_save]: changes });
  studioRealign = (song_id: string, text: string, anchors: RealignAnchor[]) =>
    call<Lyrics>("studio_realign", { song_id, text, anchors });
  exportBacking = (song_id: string, format: ExportFormat) =>
    call<{ job_id: string }>("export_backing", { song_id, [ARG_NAMES.export_backing]: { format } });
  recorderStatus = () => call<RecorderStatus>("recorder_status");
  recorderStart = () => call<void>("recorder_start");
  recorderStop = () => call<Recording>("recorder_stop");
  recorderRequestPermission = () => call<void>("recorder_request_permission");
  recordingsList = () => call<{ recordings: Recording[] }>("recordings_list");
  studioStatus = () => call<StudioStatus>("studio_status");
  studioInspect = (path: string) => call<InspectResult>("studio_inspect", { path });
  studioLyricsLookup = (query: LyricsLookupQuery) =>
    call<{ results: LookupResult[] }>("studio_lyrics_lookup", { [ARG_NAMES.studio_lyrics_lookup]: query });
  studioSubmit = (request: StudioSubmitRequest) =>
    call<{ job_id: string }>("studio_submit", { [ARG_NAMES.studio_submit]: request });
  studioCancel = (job_id: string) => call<void>("studio_cancel", { job_id });
  studioModelsStatus = () => call<StudioModelsStatus>("studio_models_status");
  studioModelsDownload = () => call<void>("studio_models_download");
  pickFiles = () => call<string[]>("pick_files");
  selfcheckRun = (user_agent: string, silent = false) => call<SelfCheckResult>("selfcheck_run", { user_agent, silent });
  rootChoose = () => call<{ ok: boolean; root: string | null; message: string }>("root_choose");
  selfcheckUserAgent = (user_agent: string) => call<void>("selfcheck_user_agent", { user_agent });

  on<E extends ApiEventName>(event: E, handler: (payload: ApiEvents[E]) => void): Unsubscribe {
    // `listen` is asynchronous; remember a request to stop that arrives before it resolves.
    let unlisten: UnlistenFn | null = null;
    let stopped = false;
    listen<ApiEvents[E]>(event, (e) => handler(e.payload))
      .then((fn) => {
        if (stopped) fn();
        else unlisten = fn;
      })
      .catch((err: unknown) => console.error(`listen(${event}) failed`, err));
    return () => {
      stopped = true;
      unlisten?.();
    };
  }

  async setWindowFullscreen(on: boolean): Promise<boolean> {
    try {
      // Needs the `core:window:allow-set-fullscreen` permission in the app's capabilities.
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setFullscreen(on);
      return true;
    } catch (err) {
      console.warn("window full screen refused; using the in-window stage layout", err);
      return false;
    }
  }

  onWindowMinimised(cb: (minimised: boolean) => void): Unsubscribe {
    let stopped = false;
    let unlisten: UnlistenFn | null = null;
    void (async () => {
      try {
        // Window getters and the resize event are in core:default (no extra permission).
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const check = () => {
          win
            .isMinimized()
            .then((m) => !stopped && cb(m))
            .catch(() => undefined);
        };
        // Minimising and restoring both resize the window (to 0x0 and back on Windows).
        const un = await win.onResized(check);
        if (stopped) un();
        else unlisten = un;
        check();
      } catch (err) {
        console.warn("cannot follow the window's minimised state", err);
      }
    })();
    return () => {
      stopped = true;
      unlisten?.();
    };
  }

  /** Stage 0 `--smoke-test` hook (app/src-tauri/src/smoke.rs); rejected outside that mode. */
  sendSmokeReport = (report: unknown) => call<void>("smoke_report", { report });
}

let chosen: Api | null = null;

/** The API for this run: Tauri inside the app, the mock in a browser. Created once. */
export async function getApi(): Promise<Api> {
  if (chosen) return chosen;
  if (isTauri()) {
    chosen = new TauriApi();
  } else {
    // Loaded lazily so the shipped app never runs mock code.
    const { MockApi } = await import("./mock");
    chosen = new MockApi(MockApi.optionsFromUrl(globalThis.location?.search ?? ""));
  }
  return chosen;
}

/** For tests: install a specific implementation. */
export function setApiForTests(api: Api | null): void {
  chosen = api;
}

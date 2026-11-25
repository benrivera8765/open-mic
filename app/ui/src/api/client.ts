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
  DeletedList,
  DeleteResult,
  Device,
  ExportFormat,
  GuideQueue,
  GuideStatus,
  ExcludedSpan,
  InputChoice,
  InputError,
  LaneCost,
  MicInput,
  MicList,
  MicPermission,
  OctaveMode,
  PracticeGuide,
  PracticeHistory,
  PracticeResult,
  PracticeStartOptions,
  PracticeState,
  PracticeTarget,
  RigCheckOptions,
  RigCheckReport,
  Strictness,
  InspectResult,
  LibraryList,
  MediaPlayers,
  LookupResult,
  LyricsLookupQuery,
  Lyrics,
  MetadataChanges,
  PlayerState,
  PurgeResult,
  RealignAnchor,
  Recording,
  RecorderStatus,
  RecordingSession,
  RestoreResult,
  SelfCheckResult,
  SessionStartOptions,
  SongDetail,
  SongSettings,
  StudioModelsStatus,
  StudioPreset,
  StudioJob,
  StudioStatus,
  StudioSubmitRequest,
} from "./types";

/** Names of the object parameters APP_API does not name explicitly. */
export const ARG_NAMES = {
  metadata_save: "metadata",
  export_backing: "options",
  studio_lyrics_lookup: "query",
  studio_submit: "job",
  recorder_session_start: "options",
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
  /** Moves songs to Deleted songs (the loaded one is stopped and closed first). */
  libraryDelete(songIds: string[]): Promise<DeleteResult>;
  libraryDeletedList(): Promise<DeletedList>;
  /** Back to their places in the Library (a free name next to it when taken). */
  libraryRestore(deletedIds: string[]): Promise<RestoreResult>;
  /** Deletes for good: some songs, or "all" (empties Deleted songs). Cannot be undone. */
  libraryPurge(which: string[] | "all"): Promise<PurgeResult>;
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
  /** `pausePlayer` (addition): after "Record & play", also pause that player. */
  recorderStop(pausePlayer?: boolean): Promise<Recording>;
  recorderRequestPermission(): Promise<void>;
  recordingsList(): Promise<{ recordings: Recording[] }>;
  // 9. Studio
  studioStatus(): Promise<StudioStatus>;
  studioInspect(path: string): Promise<InspectResult>;
  studioLyricsLookup(query: LyricsLookupQuery): Promise<{ results: LookupResult[] }>;
  studioSubmit(request: StudioSubmitRequest): Promise<{ job_id: string }>;
  studioCancel(jobId: string): Promise<void>;
  /** Lyrics of a song still waiting in the queue (addition). */
  studioSetLyrics?(jobId: string, lyrics: { mode: "auto" | "text"; text: string | null }): Promise<StudioJob>;
  /** The models of one quality preset (the one chosen in the form), plus the lyrics models. */
  studioModelsStatus(preset: StudioPreset): Promise<StudioModelsStatus>;
  /** Downloads that preset's missing models; resolves when the download has finished. */
  studioModelsDownload(preset: StudioPreset): Promise<void>;
  pickFiles(): Promise<string[]>;
  // 10. Self-check (`silent`: digital silence instead of the -60 dBFS tone; a core addition)
  /** `lane` (added WO2, APP_API 10.1): the practice lane's drawing cost measured by this web view. */
  selfcheckRun(userAgent: string, silent?: boolean, lane?: LaneCost | null): Promise<SelfCheckResult>;

  // Additions the app core implements beyond APP_API v1 (optional here).
  /** macOS translocation: native folder picker for the stick's main folder, then restart. */
  rootChoose?(): Promise<{ ok: boolean; root: string | null; message: string }>;
  // 8. Recorder additions (the owner's request of 2026-11-18): the media player link.
  /** The media players Windows knows about (read only). */
  recorderMediaSessions?(): Promise<MediaPlayers>;
  /** Starts the capture, waits for its first buffer, then tells the player to play. */
  recorderRecordAndPlay?(playerId: string): Promise<RecorderStatus>;
  /** "Record songs separately": one capture, split into one file per song on stop. */
  recorderSessionStart?(options: SessionStartOptions): Promise<RecorderStatus>;
  recorderSessionStop?(pausePlayer: boolean): Promise<RecordingSession>;
  recorderSessionsList?(): Promise<{ sessions: RecordingSession[] }>;
  recorderSessionSplit?(folder: string): Promise<RecordingSession>;
  recorderSessionDelete?(folder: string): Promise<void>;
  /** Plays part of a recording natively (never in the web view). */
  recorderPreview?(path: string, startS: number, maxS: number): Promise<void>;
  recorderPreviewStop?(): Promise<void>;
  /** Studio form: native dialog for one JPEG / PNG cover picture; null when cancelled. */
  pickCoverImage?(): Promise<string | null>;
  /** Studio form: a JPEG / PNG picture file (the inspected cover or a picked one) as a thumbnail. */
  studioCoverPreview?(path: string, maxPx: number): Promise<CoverResult>;
  /** Tells the core the web engine's user agent (used by `--selfcheck` runs). */
  selfcheckUserAgent?(userAgent: string): Promise<void>;
  /** Stage 0 `--smoke-test` report; the core rejects it when not in that mode. */
  sendSmokeReport?(report: unknown): Promise<void>;
  /** `--smoke-test` only: one start-up step or script error, kept in the report so a UI that
   *  never reports says how far it got (D-279); the core rejects it outside that mode. */
  sendSmokeProgress?(step: string): Promise<void>;
  // 12.3 Practice data (the melody guide; added WO2). Optional so older fakes still type-check.
  guideStatus?(songId: string): Promise<GuideStatus>;
  /** Queues a build and returns at once; progress comes as `guide://status`. */
  guideBuild?(songId: string, force?: boolean): Promise<GuideStatus>;
  /** "Add practice data to all songs". */
  guideBuildAll?(includeOutdated?: boolean): Promise<{ queued: number; skipped: number }>;
  /** One song's build, or every one (no id). */
  guideCancel?(songId?: string): Promise<void>;
  guideQueue?(): Promise<GuideQueue>;
  // 3 (added WO2): the key heard at a song time (screen checks and tests).
  playerKeyAt?(positionS: number, segment?: number): Promise<{ semitones: number | null }>;
  // 12.4 Microphone (added WO2). Optional so older fakes still type-check; the UI degrades without them.
  micPermission?(): Promise<MicPermission>;
  /** macOS asks the system (only after the app's own explanation); the answer comes as `mic://permission`. */
  micRequestPermission?(): Promise<MicPermission>;
  micOpenSettings?(): Promise<void>;
  micList?(): Promise<MicList>;
  micChoose?(choice: InputChoice): Promise<InputChoice>;
  /** Opens a device for the input picker's per-channel meters only; null closes it. */
  micMeter?(deviceId: string | null): Promise<{ device: MicInput | null; error: InputError | null }>;
  // 12.5 / 12.6 Practice session (added WO2).
  practiceStart?(options?: PracticeStartOptions): Promise<PracticeState>;
  practiceStop?(keepResult?: boolean): Promise<{ result: PracticeResult | null }>;
  practiceState?(): Promise<PracticeState>;
  practiceGuide?(songId?: string): Promise<PracticeGuide>;
  practiceSuspendInput?(suspended: boolean): Promise<void>;
  practiceFinish?(): Promise<{ result: PracticeResult | null }>;
  practiceAgain?(): Promise<PracticeState>;
  practiceSongSet?(songId: string, changes: { octave?: { mode: OctaveMode; shift?: number }; strictness?: Strictness | null }): Promise<PracticeState>;
  practiceExclude?(songId: string, span: ExcludedSpan, excluded: boolean): Promise<{ excluded: ExcludedSpan[] }>;
  practiceHistory?(songId: string): Promise<PracticeHistory>;
  practiceLatencySet?(manualMs: number, deviceId?: string | null): Promise<{ manual_ms: number }>;
  /** Internal (screen checks): the scorer's target at a song time. */
  practiceTargetAt?(positionS: number, segment?: number): Promise<PracticeTarget>;
  /** Self-check: the lane's drawing cost measured by this web view (APP_API 10.1). */
  selfcheckLane?(lane: LaneCost): Promise<void>;
  /** 10.2 The practice rig check: MAKES A SOUND; only after the owner confirmed (audible_confirmed). */
  practiceRigCheck?(options: RigCheckOptions): Promise<RigCheckReport>;
  /** Internal (test modes only): generated sound instead of the microphone for the next practice_start. */
  practiceTestInput?(source: Record<string, unknown> | null): Promise<{ ok: boolean }>;

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
  libraryDelete = (song_ids: string[]) => call<DeleteResult>("library_delete", { song_ids });
  libraryDeletedList = () => call<DeletedList>("library_deleted_list");
  libraryRestore = (deleted_ids: string[]) => call<RestoreResult>("library_restore", { deleted_ids });
  libraryPurge = (which: string[] | "all") =>
    call<PurgeResult>("library_purge", which === "all" ? { all: true } : { deleted_ids: which });
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
  recorderStop = (pause_player = false) => call<Recording>("recorder_stop", { pause_player });
  recorderMediaSessions = () => call<MediaPlayers>("recorder_media_sessions");
  recorderRecordAndPlay = (player_id: string) => call<RecorderStatus>("recorder_record_and_play", { player_id });
  recorderSessionStart = (options: SessionStartOptions) =>
    call<RecorderStatus>("recorder_session_start", { [ARG_NAMES.recorder_session_start]: options });
  recorderSessionStop = (pause_player: boolean) => call<RecordingSession>("recorder_session_stop", { pause_player });
  recorderSessionsList = () => call<{ sessions: RecordingSession[] }>("recorder_sessions_list");
  recorderSessionSplit = (folder: string) => call<RecordingSession>("recorder_session_split", { folder });
  recorderSessionDelete = (folder: string) => call<void>("recorder_session_delete", { folder });
  recorderPreview = (path: string, start_s: number, max_s: number) => call<void>("recorder_preview", { path, start_s, max_s });
  recorderPreviewStop = () => call<void>("recorder_preview_stop");
  recorderRequestPermission = () => call<void>("recorder_request_permission");
  recordingsList = () => call<{ recordings: Recording[] }>("recordings_list");
  studioStatus = () => call<StudioStatus>("studio_status");
  studioInspect = (path: string) => call<InspectResult>("studio_inspect", { path });
  studioLyricsLookup = (query: LyricsLookupQuery) =>
    call<{ results: LookupResult[] }>("studio_lyrics_lookup", { [ARG_NAMES.studio_lyrics_lookup]: query });
  studioSubmit = (request: StudioSubmitRequest) =>
    call<{ job_id: string }>("studio_submit", { [ARG_NAMES.studio_submit]: request });
  studioCancel = (job_id: string) => call<void>("studio_cancel", { job_id });
  studioSetLyrics = (job_id: string, lyrics: { mode: "auto" | "text"; text: string | null }) => call<StudioJob>("studio_set_lyrics", { job_id, lyrics });
  studioModelsStatus = (preset: StudioPreset) => call<StudioModelsStatus>("studio_models_status", { preset });
  studioModelsDownload = (preset: StudioPreset) => call<void>("studio_models_download", { preset });
  pickFiles = () => call<string[]>("pick_files");
  pickCoverImage = () => call<string | null>("pick_cover_image");
  studioCoverPreview = (path: string, max_px: number) => call<CoverResult>("studio_cover_preview", { path, max_px });
  selfcheckRun = (user_agent: string, silent = false, lane: LaneCost | null = null) =>
    call<SelfCheckResult>("selfcheck_run", lane ? { user_agent, silent, lane } : { user_agent, silent });
  rootChoose = () => call<{ ok: boolean; root: string | null; message: string }>("root_choose");
  guideStatus = (song_id: string) => call<GuideStatus>("guide_status", { song_id });
  guideBuild = (song_id: string, force = false) => call<GuideStatus>("guide_build", { song_id, force });
  guideBuildAll = (include_outdated = false) =>
    call<{ queued: number; skipped: number }>("guide_build_all", { options: { include_outdated } });
  guideCancel = (song_id?: string) => call<void>("guide_cancel", song_id === undefined ? {} : { song_id });
  guideQueue = () => call<GuideQueue>("guide_queue");
  // WO2 (APP_API 3, 12): argument names as in the contract.
  playerKeyAt = (position_s: number, segment?: number) =>
    call<{ semitones: number | null }>("player_key_at", segment === undefined ? { position_s } : { position_s, segment });
  micPermission = () => call<MicPermission>("mic_permission");
  micRequestPermission = () => call<MicPermission>("mic_request_permission");
  micOpenSettings = () => call<void>("mic_open_settings");
  micList = () => call<MicList>("mic_list");
  micChoose = (choice: InputChoice) => call<InputChoice>("mic_choose", { choice });
  micMeter = (device_id: string | null) => call<{ device: MicInput | null; error: InputError | null }>("mic_meter", { device_id });
  practiceStart = (options: PracticeStartOptions = {}) => call<PracticeState>("practice_start", { options });
  practiceStop = (keep_result = true) => call<{ result: PracticeResult | null }>("practice_stop", { options: { keep_result } });
  practiceState = () => call<PracticeState>("practice_state");
  practiceGuide = (song_id?: string) => call<PracticeGuide>("practice_guide", song_id === undefined ? {} : { song_id });
  practiceSuspendInput = (suspended: boolean) => call<void>("practice_suspend_input", { suspended });
  practiceFinish = () => call<{ result: PracticeResult | null }>("practice_finish");
  practiceAgain = () => call<PracticeState>("practice_again");
  practiceSongSet = (song_id: string, changes: { octave?: { mode: OctaveMode; shift?: number }; strictness?: Strictness | null }) =>
    call<PracticeState>("practice_song_set", { song_id, changes });
  practiceExclude = (song_id: string, span: ExcludedSpan, excluded: boolean) =>
    call<{ excluded: ExcludedSpan[] }>("practice_exclude", { song_id, span, excluded });
  practiceHistory = (song_id: string) => call<PracticeHistory>("practice_history", { song_id });
  practiceLatencySet = (manual_ms: number, device_id?: string | null) =>
    call<{ manual_ms: number }>("practice_latency_set", device_id === undefined ? { manual_ms } : { manual_ms, device_id });
  practiceTargetAt = (position_s: number, segment?: number) =>
    call<PracticeTarget>("practice_target_at", segment === undefined ? { position_s } : { position_s, segment });
  selfcheckLane = (lane: LaneCost) => call<void>("selfcheck_lane", { lane });
  practiceRigCheck = (options: RigCheckOptions) => call<RigCheckReport>("practice_rig_check", { options });
  practiceTestInput = (source: Record<string, unknown> | null) => call<{ ok: boolean }>("practice_test_input", { source });
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
  sendSmokeProgress = (step: string) => call<void>("smoke_progress", { step });
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

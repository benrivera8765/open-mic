// SPDX-License-Identifier: GPL-3.0-or-later
// TypeScript mirror of docs/APP_API.md (app API version 1) and of the lyrics schema
// in docs/SONG_FORMAT.md section 5. The contract documents win; if they change,
// change this file to match, field for field. Keys are snake_case as on the wire.
//
// Where the contract leaves a type open (Studio inspect / lookup / models results),
// the shape is taken from docs/STUDIO_PROTOCOL.md, which the app core forwards.
// Optional fields the UI reads but the contract does not define yet are kept out
// of these interfaces and live in `ProposedExtensions` at the bottom, so it stays
// obvious what is contract and what is a proposal.

// ---------------------------------------------------------------------------
// Errors (APP_API conventions)
// ---------------------------------------------------------------------------

/** Every command rejects with this; `message` is plain language for the user. */
export interface ApiError {
  code: string;
  message: string;
}

// ---------------------------------------------------------------------------
// 1. App and capabilities
// ---------------------------------------------------------------------------

export type Platform = "windows" | "macos";
export type RecorderPermission = "granted" | "denied" | "not_determined" | "not_required";

export interface AppInfo {
  name: string;
  version: string;
  platform: Platform;
  /** "Windows 11 26200", "macOS 13.6.9" */
  os_version: string;
  portable: boolean;
  /** Root shown for information only. */
  root: string | null;
  /** macOS App Translocation detected. */
  translocated: boolean;
  paths: { library: string; recordings: string; exports: string; settings: string; models: string | null };
  capabilities: {
    /** `checking` (APP_API addition): true until the background Studio probe has answered. */
    studio: { available: boolean; reasons: string[]; gpu: string | null; vram_mb: number | null; checking?: boolean };
    recorder: { available: boolean; reason: string | null; permission: RecorderPermission };
    /** Windows only. */
    exclusive_mode: boolean;
  };
}

// ---------------------------------------------------------------------------
// 2. Library
// ---------------------------------------------------------------------------

export type KeyMode = "major" | "minor";

export interface SongKey {
  /** One of C C# D D# E F F# G G# A A# B (sharps only on the wire). */
  tonic: string;
  mode: KeyMode;
  name: string;
}

export interface SongSummary {
  song_id: string;
  title: string;
  artist: string;
  album: string | null;
  duration_s: number;
  key: SongKey | null;
  tempo_bpm: number | null;
  has_lyrics: boolean;
  lyrics_checked: boolean;
  /** Saved key shift. */
  semitones: number;
  file_bytes: number;
  modified_ms: number;
  preset: string;
  /** Non-null: listed but unplayable (damaged, newer version...). */
  error: string | null;
}

export interface LibraryList {
  songs: SongSummary[];
  free_bytes: number;
  total_bytes: number;
}

export interface CoverResult {
  data_url: string | null;
}

// ---------------------------------------------------------------------------
// Lyrics (SONG_FORMAT section 5)
// ---------------------------------------------------------------------------

/** `[grapheme, start, end]`: one Unicode extended grapheme cluster and its times (s). */
export type LyricChar = [string, number, number];

export interface LyricWord {
  start: number;
  end: number;
  confidence?: number;
  chars: LyricChar[];
  /** Unknown keys are preserved (forward compatibility). */
  [extra: string]: unknown;
}

export interface LyricLine {
  start: number;
  end: number;
  words: LyricWord[];
  /** Later versions may add `"kind": "backing"`; readers ignore unknown keys. */
  kind?: string;
  [extra: string]: unknown;
}

export interface Lyrics {
  lyrics_version: number;
  language: string;
  /** user_text | lrc | embedded | online:<service> | transcribed */
  source: string;
  checked: boolean;
  alignment?: { method: string; model: string; anchored_lines: boolean; [extra: string]: unknown } | null;
  lines: LyricLine[];
  [extra: string]: unknown;
}

// ---------------------------------------------------------------------------
// 3. Loading and playback
// ---------------------------------------------------------------------------

export interface SongSettings {
  semitones: number;
  shift_drums: boolean;
  /** 0..1 */
  guide_vocal: number;
  backing_on: boolean;
  backing_level: number;
  levels: { drums: number; other: number };
  lyrics_offset_ms: number;
}

export interface StemInfo {
  role: string;
  in_karaoke_mix: boolean;
  pitched: boolean;
}

export interface SongDetail {
  summary: SongSummary;
  /** Effective lyrics (override wins). */
  lyrics: Lyrics | null;
  lyrics_is_override: boolean;
  /** Effective saved settings. */
  settings: SongSettings;
  stems: StemInfo[];
  sample_rate: number;
  channels: number;
}

export type PlayerStateName = "empty" | "loading" | "paused" | "playing" | "ended" | "error";

export interface DeviceState {
  name: string;
  sample_rate: number;
  channels: number;
  exclusive: boolean;
  latency_ms: number;
}

export interface PlayerState {
  state: PlayerStateName;
  song_id: string | null;
  position_s: number;
  duration_s: number;
  device: DeviceState | null;
  underruns: number;
  error: string | null;
  /** "A major -> B major (+2)" */
  key_label: string | null;
  /** Addition (WORK_ORDER 4.5): how key changes are made now. */
  key_mode?: "realtime" | "prerender";
  /** Addition: prerender mode, a key or mix being rendered before it is heard. */
  key_preparing?: KeyPreparing | null;
  /** Addition: why prerender was asked for but real time is used. */
  key_note?: string | null;
}

/** "Preparing key +2... 40%" (render ahead and cache, WORK_ORDER 4.5). */
export interface KeyPreparing {
  semitones: number;
  /** "key": a new key; "mix": same key, a level of a pitched stem changed. */
  reason: "key" | "mix";
  /** 0..1 of what must be rendered before the switch. */
  progress: number;
}

/** `settings.audio.key_mode`. */
export type KeyModeSetting = "auto" | "realtime" | "prerender";

/** Payload of `playback://clock`. */
export interface ClockEvent {
  /** Song time of the sample now leaving the DAC (device latency already subtracted). */
  position_s: number;
  playing: boolean;
  seq: number;
}

// ---------------------------------------------------------------------------
// 4. Output devices
// ---------------------------------------------------------------------------

export interface Device {
  id: string;
  name: string;
  is_default: boolean;
  sample_rate: number;
  channels: number;
  exclusive_capable: boolean;
}

// ---------------------------------------------------------------------------
// 5. App settings
// ---------------------------------------------------------------------------

export type MotionSetting = "system" | "reduce" | "full";
export type LibrarySort = "title" | "artist" | "recent" | "key";

export interface AppSettings {
  ui: {
    skin: boolean;
    bubbles: boolean;
    motion: MotionSetting;
    /** CSS px at 1440x900, scaled with the window. */
    lyrics_text_size: number;
    /** 0..1 from the left, default 0.33. */
    marker_position: number;
    colors: { sung: string; upcoming: string; marker: string };
    library_sort: LibrarySort;
    fullscreen_stage: boolean;
  };
  audio: {
    device_id: string | null;
    exclusive: boolean;
    master_volume: number;
    /** Bluetooth latency per device id. */
    device_offsets_ms: Record<string, number>;
    /** Key change: "auto" (default) renders ahead on computers too slow for real time. */
    key_mode?: KeyModeSetting;
    /** Size cap of the key cache in MB (64..65536, default 1024). */
    key_cache_mb?: number;
    /** Measured Rubber Band speed per computer and sample rate (written by the core). */
    key_benchmarks?: Record<string, number>;
  };
}

/** `settings_set` takes a deep partial (deep-merged by the core). */
export type AppSettingsChanges = {
  ui?: Partial<Omit<AppSettings["ui"], "colors">> & { colors?: Partial<AppSettings["ui"]["colors"]> };
  audio?: Partial<AppSettings["audio"]>;
};

// ---------------------------------------------------------------------------
// 6. Sync editor and metadata
// ---------------------------------------------------------------------------

export interface MetadataChanges {
  title?: string;
  artist?: string;
  album?: string | null;
}

export interface RealignAnchor {
  /** 0-based index into the `lines` array of the lyrics produced from `text`. */
  line: number;
  start: number;
}

// ---------------------------------------------------------------------------
// 7. Export backing track
// ---------------------------------------------------------------------------

export type ExportFormat = "flac24" | "wav32f";

export interface ExportProgressEvent {
  job_id: string;
  progress: number;
}
export interface ExportDoneEvent {
  job_id: string;
  path: string;
  /**
   * Gain the export applied to the whole file, in dB (0 or negative). Non-zero only for
   * `flac24` when the rendered mix peaked above full scale: a 24-bit file cannot hold
   * that, so it was lowered by this much to avoid clipping (WORK_ORDER 5: never hidden,
   * the UI must say so). `wav32f` is never scaled. Missing from an older core = 0.
   */
  applied_gain_db?: number;
  /** Largest |sample| of the rendered mix before any gain (linear, 1.0 = full scale). */
  peak?: number;
}
export interface ExportFailedEvent {
  job_id: string;
  code: string;
  message: string;
}

// ---------------------------------------------------------------------------
// 8. Recorder
// ---------------------------------------------------------------------------

export type SampleFormat = "f32" | "i16" | "i24" | "i32";

export interface RecorderFormat {
  sample_rate: number;
  channels: number;
  sample_format: SampleFormat;
  note: string;
}

export interface RecorderStatus {
  state: "idle" | "starting" | "recording" | "stopping" | "error";
  elapsed_s: number;
  format: RecorderFormat | null;
  peak_db: number[];
  clipped: boolean;
  dropped_s: number;
  error: string | null;
}

export interface Recording {
  path: string;
  name: string;
  duration_s: number;
  sample_rate: number;
  channels: number;
  bytes: number;
  clipped: boolean;
}

// ---------------------------------------------------------------------------
// 9. Studio
// ---------------------------------------------------------------------------

export type StudioPreset = "full" | "clean" | "permissive";
export type StudioJobState = "queued" | "running" | "done" | "failed" | "cancelled";

/** Stage names in order (STUDIO_PROTOCOL section 7). */
export const STUDIO_STAGES = ["decode", "separate", "lyrics", "align", "analyse", "write"] as const;
export type StudioStage = (typeof STUDIO_STAGES)[number];

export interface StudioJob {
  job_id: string;
  input_name: string;
  state: StudioJobState;
  stage: string | null;
  overall: number;
  message: string | null;
  output_song_id: string | null;
  error: string | null;
}

export interface StudioStatus {
  available: boolean;
  reasons: string[];
  running: boolean;
  models_present: boolean;
  queue: StudioJob[];
  /** APP_API addition: as `capabilities.studio.checking`. */
  checking?: boolean;
}

/** STUDIO_PROTOCOL section 5 (`inspect.result`, minus the protocol envelope). */
export interface InspectResult {
  ok: boolean;
  title: string | null;
  artist: string | null;
  album: string | null;
  duration_seconds: number | null;
  container: string | null;
  codec: string | null;
  sample_rate: number | null;
  channels: number | null;
  bits_per_sample: number | null;
  lossy: boolean | null;
  has_video: boolean;
  embedded_lyrics: { present: boolean; synced: boolean; text: string | null } | null;
  cover_path: string | null;
}

export interface LyricsLookupQuery {
  artist: string;
  title: string;
  album: string | null;
  duration_s: number | null;
}

/** STUDIO_PROTOCOL section 6, one entry of `results`. */
export interface LookupResult {
  service: string;
  synced_lrc: string | null;
  plain: string | null;
  duration_seconds: number | null;
}

export type StudioLyricsMode = "auto" | "text" | "lrc" | "none";

export interface StudioSubmitRequest {
  input: string;
  preset: StudioPreset;
  language: string;
  /** `cover_path` (STUDIO_PROTOCOL 7, added): absent = the file's own cover, null = no
   * cover, a path = that JPEG / PNG picture instead. */
  metadata: { title: string; artist: string; album: string | null; cover_path?: string | null };
  /** `source`: STUDIO_PROTOCOL 7 `lyrics.source`, e.g. "online:lrclib" for a lookup
   * result; null or absent = the Studio's default (`user_text` / `lrc`). */
  lyrics: { mode: StudioLyricsMode; text: string | null; lrc: string | null; source?: string | null };
}

/** STUDIO_PROTOCOL section 4 (`models.status.result`, minus the envelope). */
export interface StudioModelInfo {
  key: string;
  name: string;
  /** Plain model name (the real Studio sends it; older cores and the mock may not). */
  title?: string;
  file: string;
  size_bytes: number;
  present: boolean;
  /** "GPL-3.0", "MIT", "Apache-2.0", or "none published". */
  licence: string;
  /** "yes", "no" or "unclear" (MODELS.md). */
  commercial: string;
  /** "separation", "alignment" or "transcription". */
  task?: string;
  needed_for?: string;
}
export interface StudioModelsStatus {
  /** The preset the answer is for (the Studio echoes it). */
  preset?: StudioPreset;
  models: StudioModelInfo[];
  all_present: boolean;
  download_bytes?: number;
}

export interface StudioModelsEvent {
  name: string;
  done_bytes: number;
  total_bytes: number;
}

// ---------------------------------------------------------------------------
// 10. Self-check
// ---------------------------------------------------------------------------

export interface SelfCheckItem {
  name: string;
  ok: boolean;
  detail: string;
}
export interface SelfCheckResult {
  report_path: string;
  passed: boolean;
  items: SelfCheckItem[];
}

// ---------------------------------------------------------------------------
// Events (name -> payload)
// ---------------------------------------------------------------------------

export interface ApiEvents {
  "library://changed": null;
  "playback://clock": ClockEvent;
  "playback://state": PlayerState;
  "playback://error": ApiError;
  "export://progress": ExportProgressEvent;
  "export://done": ExportDoneEvent;
  "export://failed": ExportFailedEvent;
  "recorder://level": RecorderStatus;
  "studio://job": StudioJob;
  "studio://models": StudioModelsEvent;
  "studio://dropped": { paths: string[] };
  /** Addition by the app core (not in APP_API v1): a full studio_status() when it changes. */
  "studio://status": StudioStatus;
}
export type ApiEventName = keyof ApiEvents;

// ---------------------------------------------------------------------------
// Proposed extensions (NOT in APP_API.md v1). The UI reads them when present and
// degrades gracefully when absent. Reported to the contract owner.
// ---------------------------------------------------------------------------

export interface ProposedExtensions {
  /** PlayerState (sent by the app core): the output went above full scale (mixer clip light). */
  clipped?: boolean;
  /** PlayerState (sent by the app core): WASAPI device-side starvation events. */
  device_starved?: number;
  /** PlayerState (proposed, not sent yet): output peak per channel in dBFS since the last event. */
  output_peak_db?: number[];
  /** StudioJob (proposed): progress of the current stage 0..1 (STUDIO_PROTOCOL has `stage_progress`). */
  stage_progress?: number;
}

/** AppInfo fields the app core adds to APP_API v1 (read when present). */
export interface AppInfoAdditions {
  identifier?: string;
  mode?: string;
  mode_reason?: string;
  webview_version?: string;
  /** What to tell the user before asking for the stick's folder (macOS translocation). */
  translocation_help?: string | null;
}

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
  /** `deleted` (addition, 2026-11-18): the Deleted songs folder, created on the first delete. */
  paths: { library: string; recordings: string; exports: string; settings: string; models: string | null; deleted?: string };
  capabilities: {
    /** `checking` (APP_API addition): true until the background Studio probe has answered. */
    studio: { available: boolean; reasons: string[]; gpu: string | null; vram_mb: number | null; checking?: boolean };
    recorder: { available: boolean; reason: string | null; permission: RecorderPermission };
    /** Windows only. */
    exclusive_mode: boolean;
    /** (added WO2) Practice mode's input (APP_API 12.4); absent from an older core. */
    microphone?: MicCapability;
    /** (added WO2) This app's guide builder. */
    practice?: { guide_method: string; guide_version: string };
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
  /** (added WO2, APP_API 12.3) The melody guide ("practice data"); absent from an older core. */
  guide?: GuideStatus;
  /** (added WO2) The best practice result kept in the song's slot. */
  practice_best?: { overall: number | null; grade: string | null; date: string } | null;
  /**
   * (added 2026-11-25, D-281) The file the Studio made the song from: its name and
   * length. The Recorder marks a recording with the same name and length as made.
   */
  source?: { file_name: string; seconds: number } | null;
}

/**
 * The melody guide of a song ("practice data" in the UI): docs/APP_API.md 12.3.
 * none / stale / failed: practice is not ready (Practice adds it on the spot).
 */
export type GuideState = "none" | "queued" | "building" | "ready" | "outdated" | "stale" | "newer" | "failed";

export interface GuideCounts {
  notes: number;
  pitched: number;
  unpitched: number;
  low_confidence: number;
  octave_flags: number;
  graded: number;
}

export interface GuideStatus {
  state: GuideState;
  /** Plain words for none / stale / newer / failed. */
  reason: string | null;
  /** failed: guide_no_lead | guide_read_failed | guide_damaged | guide_disk_full | guide_read_only | guide_in_use | guide_internal | cancelled */
  error_code: string | null;
  /** building: 0..1 */
  progress: number | null;
  /** Usable now, but not (yet) in the file. */
  in_memory: boolean;
  /** The swap into the file waits until the song stops playing. */
  pending_write: boolean;
  method: string | null;
  version: string | null;
  created_utc: string | null;
  counts: GuideCounts | null;
  tuning_cents: number | null;
}

/** `guide://batch` ("Add practice data to all songs"). */
export interface GuideBatchEvent {
  total: number;
  done: number;
  failed: number;
  current: string | null;
  running: boolean;
}

export interface GuideQueue {
  items: { song_id: string; state: "queued" | "building"; progress: number | null }[];
  running: boolean;
}

export interface LibraryList {
  songs: SongSummary[];
  free_bytes: number;
  total_bytes: number;
  /** Addition (2026-11-18): songs in Deleted songs and the bytes they still take on the drive. */
  deleted_count?: number;
  deleted_bytes?: number;
}

/**
 * Deleted songs (the owner's request of 2026-11-18, APP_API section 2): a deleted
 * song's file is moved to `<root>/Deleted/`, from where it is restored or deleted for good.
 */
export interface DeletedSong {
  /** Its file name in Deleted/: what restore and purge take. */
  deleted_id: string;
  /** Its song_id before it was deleted. */
  original_song_id: string;
  /** The song_id a restore gives it now (the original, or a free name next to it). */
  restore_to: string;
  title: string;
  artist: string;
  album: string | null;
  duration_s: number;
  file_bytes: number;
  /** Milliseconds since 1970 (UTC). */
  deleted_ms: number;
}

export interface DeletedList {
  /** Newest first. */
  songs: DeletedSong[];
  count: number;
  total_bytes: number;
  folder: string;
}

/** A song an operation could not handle (`id` is a song_id or a deleted_id). */
export interface DeleteFailure {
  id: string;
  code: string;
  message: string;
}

export interface DeleteResult {
  deleted: DeletedSong[];
  failed: DeleteFailure[];
  /** The loaded song's id when it was one of them (it was stopped and closed first). */
  unloaded: string | null;
}

export interface RestoreResult {
  restored: { deleted_id: string; song_id: string }[];
  failed: DeleteFailure[];
}

export interface PurgeResult {
  purged: string[];
  freed_bytes: number;
  failed: DeleteFailure[];
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
  /** (added WO2, APP_API 3) As in the summary; absent from an older core. */
  guide?: GuideStatus;
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
  // ---- (added WO2, D-239) the key HEARD: practice mode draws and grades with it, never with the request.
  // Optional here so an older core (and older fakes) still type-check; absent = the requested key.
  /** Semitones sounding at the DAC now, i.e. at position_s (-12..12). */
  key_heard?: number;
  /** The key last asked for (= settings.semitones). */
  key_requested?: number;
  /** The next change of the key heard: from song time at_s on (null: not scheduled yet). */
  key_pending?: KeyPending | null;
  /** Playback segment position_s and key_heard belong to (new at every load, seek, device switch). */
  key_segment?: number;
  /** As key_label, for the key heard. */
  key_heard_label?: string | null;
  /** How to name notes in the key heard (agrees with key_heard_label). */
  key_spelling?: NoteSpelling;
  /** The same for key_pending. */
  key_pending_spelling?: NoteSpelling | null;
}

/** `PlayerState.key_pending` / `playback://clock` `key_pending` (APP_API 3, WO2). */
export interface KeyPending {
  semitones: number;
  /** Song time from which the key is heard; null = not scheduled yet (render ahead still rendering). */
  at_s: number | null;
}

/** Note spelling for the key heard (APP_API 3): flats in flat-signature keys, sharps otherwise. */
export type NoteSpelling = "sharps" | "flats";

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
  /** (added WO2) The key heard at position_s, the next change and the playback segment. */
  key_heard?: number;
  key_pending?: KeyPending | null;
  segment?: number;
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
/** How the lyrics move (the wipe or the ticker): "word" = smooth, paced by word (default); "letter" = exact, letter by letter. */
export type LyricsMotion = "word" | "letter";
/** How the lyrics are shown (D-218): "page" = static lines with a colour wipe (default); "scroll" = the scrolling ticker. */
export type LyricsDisplay = "page" | "scroll";
export type LibrarySort = "title" | "artist" | "recent" | "key";

/** Jukebox customisation (owner, 2026-11-22). Stored by the core as unknown `ui.look` keys (UiSettings.extra), so an older core keeps them as they are. */
export type CabinetFinish = "pearl" | "walnut" | "maple" | "aquarium" | "forge" | "ocean" | "brass" | "cyber";
export type FrameStyle = "neon" | "pilaster" | "lava" | "light" | "ocean" | "fire" | "water" | "steam" | "data";
export type AccentFinish = "silver" | "red" | "iron" | "copper";
export type BadgeField = "lapis" | "red" | "silver" | "magma" | "copper" | "carbon";
export type BadgeLettering = "gold" | "cream" | "ember" | "neon" | "white";
export type BadgeRim = "lapis" | "gold";
export type LatticeFinish = "silver" | "gold" | "iron";
/** The warm metal of rims, rings, bezels and plates: gold chrome or bronze. */
export type MetalFinish = "gold" | "bronze" | "chrome" | "brass";
/** The colour lit plates, faders, focus rings and the screen's top glow take. */
export type GlowColor = "pink" | "ember" | "aqua" | "violet" | "amber";
/** The colour of the practice lane's note bars. */
export type LaneBars = "pink" | "ember" | "aqua" | "violet" | "amber";
/** A theme: a preset of every look option (and the lyric colours); "custom" = none. */
export type ThemeId = "deluxe" | "classic" | "ocean" | "electric" | "molten" | "water" | "steampunk" | "cyberpunk" | "custom";
export type CelebrationKind = "mix" | "starburst" | "fireworks" | "rings" | "bloom" | "inferno";
/** How often perfect phrases are celebrated: nearly perfect ones too, every one, every 2nd / 3rd, at most every 30 s. */
export type CelebrateFreq = "often" | "every" | "second" | "third" | "rare";
/** What comes off the pitch cursor while the singer is on pitch (owner, 2026-11-23). */
export type PitchFx = "off" | "sparks" | "rings" | "stars" | "flames" | "notes";
export interface LookSettings {
  /** The cabinet face: animated pearlescent glow (default), dark walnut, light maple, or an aquarium. */
  cabinet: CabinetFinish;
  /** The border round the screen: neon bubble tubes (default), pearl pilasters with two small bubble tubes, a lava lamp, a lightning tube of blue water ("light"), or an ocean tube with fish (goes with the aquarium cabinet). */
  frame: FrameStyle;
  /** The bands between the chrome rings (columns and crest clamps): silver chrome (default) or red enamel. */
  accents: AccentFinish;
  /** The badge's field inside its medium gold border. */
  badge_field: BadgeField;
  /** The badge's lettering. */
  badge_text: BadgeLettering;
  /** The badge's frame: lapis lazuli with a thin gold edge (default) or gold chrome. */
  badge_rim: BadgeRim;
  /** The bars of the speaker lattices (their surround stays gold). */
  lattice: LatticeFinish;
  /** A thin silver chrome line inside the gold of the plates, the key dial, the grilles and the medallions. */
  inner_trim: boolean;
  /** Watch only: a perfect voice line rides the melody (for filming). */
  watch_follow: boolean;
  /** Windows: the forge's flowing lava and the fire pipe drawn on the GPU (WebGL); off = the lighter 2D versions. */
  gpu_fx: boolean;
  /** Windows: the animation frame rate, "standard" (as designed: effects 8-30 fps, lyrics and lane 60) or up to 60 / 120 / 240 fps. */
  fx_fps: "standard" | "60" | "120" | "240";
  /** A burst of colour when a phrase is sung perfectly (also in watch only with watch_follow). */
  celebrate: boolean;
  celebration: CelebrationKind;
  /** How often a perfect phrase is celebrated. */
  celebrate_freq: CelebrateFreq;
  /** Sparks (or another effect) coming off the pitch cursor while on pitch. */
  pitch_fx: PitchFx;
  /** Gold chrome or bronze for every warm metal part. */
  metal: MetalFinish;
  /** Glow colour of lit plates and faders, focus rings and the top of the screen. */
  glow: GlowColor;
  /** Colour of the practice lane's note bars. */
  lane_bars: LaneBars;
  /** The theme last applied (settings > Themes); any option changed after it is "modified". */
  theme: ThemeId;
}

export interface AppSettings {
  ui: {
    skin: boolean;
    bubbles: boolean;
    motion: MotionSetting;
    /** CSS px at 1440x900, scaled with the window. */
    lyrics_text_size: number;
    /** 0..1 from the left, default 0.33 (scrolling ticker only). */
    marker_position: number;
    /** Pace of the wipe or the ticker (D-216): "word" (default) or "letter" (the letter-exact motion of D-94/D-95). */
    lyrics_motion: LyricsMotion;
    /** "page" (static lines with a colour wipe, default since D-218) or "scroll" (the ticker). */
    lyrics_display: LyricsDisplay;
    /** Page display: preview the next line under the current one (default true). */
    show_next_line: boolean;
    /** Page display: the progress needle on the wipe edge, in colors.marker (default true; the ticker always has its marker). */
    show_marker: boolean;
    /** marker: the ticker's marker and the page display's progress needle. */
    colors: { sung: string; upcoming: string; marker: string };
    library_sort: LibrarySort;
    fullscreen_stage: boolean;
    /** Jukebox customisation; absent from a settings file written before it existed (read with lookOf()). */
    look?: LookSettings;
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
  /** (added WO2, APP_API 12.7) Practice mode's per-computer settings; absent from an older core. */
  practice?: PracticeAppSettings;
}

/** `settings_set` takes a deep partial (deep-merged by the core). */
export type AppSettingsChanges = {
  ui?: Partial<Omit<AppSettings["ui"], "colors" | "look">> & { colors?: Partial<AppSettings["ui"]["colors"]>; look?: Partial<LookSettings> };
  audio?: Partial<AppSettings["audio"]>;
  practice?: Partial<PracticeAppSettings>;
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
  // ---- additions (the owner's request of 2026-11-18; absent in older cores) ----
  /** Which button started the recording ("plain" while idle). */
  mode?: RecorderMode;
  /** The media player "Record & play" started, or a session follows. */
  player?: { id: string; app_name: string } | null;
  /** "Record songs separately": what is being recorded now. */
  session?: LiveSession | null;
  /** Not an error, but worth telling (the player did not accept "play"). */
  note?: string | null;
}

export type RecorderMode = "plain" | "record_and_play" | "session";

export interface Recording {
  path: string;
  name: string;
  duration_s: number;
  sample_rate: number;
  channels: number;
  bytes: number;
  clipped: boolean;
  /** Addition: what happened to the player on "Stop & pause". */
  note?: string | null;
}

/** A media player Windows knows about (`recorder_media_sessions`, addition). */
export interface MediaPlayer {
  id: string;
  source_app_id: string;
  app_name: string;
  is_current: boolean;
  status: "closed" | "opened" | "changing" | "stopped" | "playing" | "paused" | "unknown";
  title: string;
  artist: string;
  album: string;
  duration_s: number | null;
  can_play: boolean;
  can_pause: boolean;
}

export interface MediaPlayers {
  available: boolean;
  /** Why the player link is unavailable (macOS, an old Windows). */
  reason: string | null;
  players: MediaPlayer[];
}

/** `RecorderStatus.session` while songs are recorded separately. */
export interface LiveSession {
  folder: string;
  path: string;
  current: { title: string; artist: string; album: string } | null;
  playback: string;
  /** Songs started so far (advertisements not counted). */
  songs: number;
  auto_stop_idle_s: number;
  max_songs: number | null;
  /** Seconds until the session stops by itself (the player is paused). */
  auto_stop_in_s: number | null;
}

export interface SessionStartOptions {
  player_id: string;
  auto_stop_idle_s?: number;
  max_songs?: number | null;
  start_player?: boolean;
  pause_player_at_end?: boolean;
}

export type CutConfidence = "certain" | "uncertain" | "edge";

/** One song of a session (its sidecar). */
export interface RecordedSong {
  path: string;
  name: string;
  number: number;
  title: string;
  artist: string;
  album: string;
  cover_path: string | null;
  duration_s: number;
  track_duration_s: number | null;
  status: "complete" | "partial" | "unknown";
  partial_reason: string | null;
  start_confidence: CutConfidence;
  end_confidence: CutConfidence;
  paused_s: number;
}

export interface ExcludedPart {
  path: string;
  name: string;
  reason: string;
  duration_s: number;
}

/** A "record songs separately" session (`recorder_sessions_list`, addition). */
export interface RecordingSession {
  folder: string;
  path: string;
  created: string;
  player: string;
  player_id: string;
  state: "recording" | "stopped" | "splitting" | "split" | "kept_master" | "failed" | "interrupted";
  progress: number | null;
  stop_reason: string | null;
  message: string | null;
  master_present: boolean;
  song_count: number;
  songs: RecordedSong[];
  excluded: ExcludedPart[];
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
  /** (added WO2) The app's last `guide` stage: the new song's practice data. */
  guide?: GuideStatus | null;
  /** Addition: the job's lyrics, "auto" (found automatically) or "text" (the singer's own, aligned). */
  lyrics?: StudioLyricsMode;
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
  /** Addition: a song recorded with "Record songs separately" (from its sidecar). */
  recorded?: { status: "complete" | "partial" | "unknown"; partial_reason: string | null; track_duration_s: number | null; source_player: string } | null;
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

/** (added WO2, APP_API 10.1) The practice lane's drawing cost, measured by the UI for the self-check. */
export interface LaneCost {
  frames: number;
  avg_ms: number;
  p95_ms: number;
  simple_avg_ms: number;
  width: number;
  height: number;
  dpr: number;
}

// ---------------------------------------------------------------------------
// 12. Practice mode (added WO2, docs/APP_API.md section 12)
// ---------------------------------------------------------------------------

/** 12.4 */
export interface MicPermission {
  state: "granted" | "denied" | "not_determined" | "restricted";
  /** The app's own words, shown BEFORE the OS asks. */
  explanation: string;
  /** denied / restricted: the exact steps to allow it on this system. */
  steps: string | null;
  settings_url: string | null;
  /** Windows privacy switches (read only, never changed); null on macOS. */
  windows: { microphone_access: "allow" | "deny" | "unset"; user_access: "allow" | "deny" | "unset"; desktop_apps: "allow" | "deny" | "unset" } | null;
}

export interface MicCapability {
  available: boolean;
  reason: string | null;
  permission: MicPermission;
  backend: "WASAPI" | "Core Audio";
}

export type MicKind = "usb" | "built_in" | "wired_external" | "bluetooth" | "display" | "virtual" | "unknown";

export interface MicInput {
  id: string;
  name: string;
  adapter: string | null;
  kind: MicKind;
  channels: number;
  sample_rate: number;
  sample_format: string | null;
  is_default: boolean;
  bluetooth: boolean;
  /** The automatic choice (never Bluetooth or virtual). */
  preferred: boolean;
  declared_latency_ms: number | null;
  same_device_as_output: boolean;
  warning: string | null;
}

/** channel 0-based; null = the suggested voice channel. */
export interface InputChoice {
  device_id: string | null;
  device_name: string | null;
  channel: number | null;
}

export interface ChannelMeter {
  index: number;
  peak_db: number;
  rms_db: number;
  noise_floor_db: number | null;
  active_db: number | null;
  clipping: boolean;
  clipped_samples: number;
  advice: "good" | "waiting" | "too_quiet" | "clipping" | "silent";
  message: string | null;
}

export interface InputError {
  kind: "no_device" | "unplugged" | "permission_denied" | "stalled" | "format" | "device";
  message: string;
}

export interface MicList {
  inputs: MicInput[];
  chosen: InputChoice;
  automatic: string | null;
}

/** `mic://levels` */
export interface MicLevelsEvent {
  device_id: string;
  seconds: number;
  channels: ChannelMeter[];
  suggested_channel: number | null;
  chosen_channel: number | null;
}

/** 12.5 */
export type PracticePhase = "off" | "preparing" | "not_ready" | "watch_only" | "listening" | "result";
export type OctaveMode = "auto" | "fixed" | "strict";
export type Strictness = "relaxed" | "standard" | "strict";

export interface PracticeInput {
  device: MicInput;
  channel: number;
  sample_rate: number;
  state: "starting" | "running" | "unplugged" | "stalled" | "failed" | "suspended";
  message: string | null;
  meter: ChannelMeter | null;
  dropped_blocks: number;
}

export interface MusicInInput {
  presence: "searching" | "absent" | "present";
  level_db: number | null;
  delay_ms: number | null;
  stability: "unknown" | "stable" | "wandering";
  drift_ppm: number | null;
  removal: "not_needed" | "checking" | "good" | "marginal" | "unusable";
  suppression_db: number | null;
  grading: "normal" | "less_certain" | "not_graded";
  message: string | null;
}

export interface ExcludedSpan {
  line: number | null;
  start: number;
  end: number;
}

export interface PracticeState {
  active: boolean;
  song_id: string | null;
  seq: number;
  phase: PracticePhase;
  guide: GuideStatus;
  not_ready: { code: string; message: string; can_retry: boolean } | null;
  watch_only: { code: "chosen" | "no_device" | "permission_denied" | "not_determined" | "restricted" | "input_lost"; message: string } | null;
  input: PracticeInput | null;
  music: MusicInInput;
  /** settled: auto has decided. */
  octave: { mode: OctaveMode; shift: number; settled: boolean };
  strictness: Strictness;
  latency: { applied_ms: number; source: "returned_music" | "rig_check" | "manual" | "none"; measured_ms: number | null; manual_ms: number };
  take: { running: boolean; started_s: number; covered: number; graded: number; total: number; overall: number | null; seeks: number; key_changes: number } | null;
  excluded: ExcludedSpan[];
  last_result: PracticeResult | null;
}

export interface PracticeStartOptions {
  song_id?: string;
  watch_only?: boolean;
}

/** One note of `practice_guide` (the guide as the lane draws it). */
export interface PracticeGuideNote {
  start: number;
  end: number;
  midi: number;
  kind: "pitched" | "unpitched";
  confidence: number;
  /** Low confidence or octave-flagged: drawn dimmer. */
  dim: boolean;
  /** SONG_FORMAT 10.1's rule, exclusions included. */
  graded: boolean;
  glide_in: number;
  /** [line, word] of the effective lyrics. */
  word: [number, number] | null;
}

export interface PracticeGuide {
  song_id: string;
  status: GuideStatus;
  tuning_cents: number;
  /** Pitched notes, original key, no octave shift. */
  range: { low: number; high: number } | null;
  notes: PracticeGuideNote[];
  /** spoken / breath: "speak" bars, never graded. */
  spans: { start: number; end: number; kind: string }[];
  /** 0 = unvoiced; two decimals. */
  curve: { hop_s: number; t0_s: number; midi: number[] } | null;
  /** Rows of the breakdown (lyric lines, else phrases). */
  lines: { line: number; start: number; end: number }[];
}

/** `practice://live` (about 60 per second while the input runs). */
export interface PracticeLive {
  seq: number;
  segment: number;
  /** [song_time_s, midi | null, level_db], one per 10 ms. Not snapped, not octave-shifted. */
  pitch: [number, number | null, number][];
  /** [note, from_s, to_s, where]: 0 on pitch (lit), 1 above, -1 below. */
  hits: [number, number, number, -1 | 0 | 1][];
  /** [note, credit 0..1 (null: not graded), key heard, mean error in cents]. */
  done: [number, number | null, number, number | null][];
}

/** 12.6 */
export interface PracticeResult {
  date: string;
  key: number;
  key_changes: number;
  seeks: number;
  octave: number;
  octave_mode: OctaveMode;
  strictness: Strictness;
  overall: number | null;
  grade: string | null;
  graded: number;
  total: number;
  tendency_cents: number | null;
  sung_share: number;
  voiced_share: number;
  completed: boolean;
  trust: string[];
  guide: string;
  scorer: string;
  app: string;
  /** Rows: [line, start, score, graded, tendency]. */
  lines: { basis: "lyrics" | "phrases"; rows: [number, number, number | null, number, number | null][] };
  // added for the screen, not stored:
  trust_messages: string[];
  best: boolean;
  saved: boolean;
  key_label: string | null;
}

export interface PracticeHistory {
  best: PracticeResult | null;
  recent: PracticeResult[];
}

/** 12.7 */
export interface PracticeAppSettings {
  piano_side: "left" | "right";
  simple_lane: boolean;
  words_under_notes: boolean;
  lyrics_below: boolean;
  strictness: Strictness;
  octave_mode: OctaveMode;
  auto_latency: boolean;
  inputs: Record<string, InputChoice>;
  latency_offsets_ms: Record<string, number>;
  measured_latency_ms: Record<string, { ms: number; how: "returned_music" | "rig_check"; utc: string }>;
}

/** 10.2 The practice rig check (AUDIBLE, started by hand only). */
export interface RigCheckOptions {
  audible_confirmed: boolean;
  output_device_id?: string | null;
  input_device_id?: string | null;
  label?: string;
  voice_step?: boolean;
}
export interface RigCheckReport {
  report_path: string;
  ok: boolean;
  label: string | null;
  output: { name: string; kind: string; sample_rate: number; channels: number; format: string; latency_ms: number | null };
  input: { name: string; kind: string; sample_rate: number; channels: number; format: string; declared_latency_ms: number | null; same_device_as_output: boolean };
  channels: { index: number; noise_floor_db: number; voice_rms_db: number | null; test_rms_db: number | null; carries: "voice" | "test_sound" | "both" | "nothing" }[];
  returned: { detected: boolean; channel: number | null; level_db: number | null; delay_ms: number | null; delay_spread_ms: number | null };
  findings: string[];
}
/** `practice://rigcheck` */
export interface RigCheckStep {
  step: "silence" | "voice" | "test_sound" | "analysing" | "done";
  progress: number;
  message: string;
}

/** `practice_target_at` (internal, screen checks). */
export interface PracticeTarget {
  note: number | null;
  midi: number | null;
  lane: number | null;
  name: string | null;
  key: number | null;
  octave: number;
  graded: boolean;
}

// ---------------------------------------------------------------------------
// Events (name -> payload)
// ---------------------------------------------------------------------------

export interface ApiEvents {
  "library://changed": null;
  /** Addition (2026-11-18): the full deleted-songs list after a delete, restore or purge. */
  "library://deleted": DeletedList;
  "playback://clock": ClockEvent;
  "playback://state": PlayerState;
  "playback://error": ApiError;
  "export://progress": ExportProgressEvent;
  "export://done": ExportDoneEvent;
  "export://failed": ExportFailedEvent;
  "recorder://level": RecorderStatus;
  /** Addition: a session started, stopped, is splitting or finished splitting. */
  "recorder://session": RecordingSession;
  /** Addition: a recording preview started or ended. */
  "recorder://preview": { path: string; playing: boolean };
  "studio://job": StudioJob;
  "studio://models": StudioModelsEvent;
  "studio://dropped": { paths: string[] };
  /** Addition by the app core (not in APP_API v1): a full studio_status() when it changes. */
  "studio://status": StudioStatus;
  /** (added WO2, APP_API 12.3) A song's practice data changed (progress at most 5/s). */
  "guide://status": { song_id: string; status: GuideStatus };
  /** (added WO2) "Add practice data to all songs" progress. */
  "guide://batch": GuideBatchEvent;
  /** (added WO2, APP_API 12.4) Per-channel input meters, about 20 per second while a meter or practice input is open. */
  "mic://levels": MicLevelsEvent;
  "mic://permission": MicPermission;
  "mic://devices": null;
  /** (added WO2, APP_API 12.5) */
  "practice://state": PracticeState;
  "practice://live": PracticeLive;
  "practice://result": PracticeResult;
  /** (added WO2, APP_API 10.2) One per step of the rig check. */
  "practice://rigcheck": RigCheckStep;
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

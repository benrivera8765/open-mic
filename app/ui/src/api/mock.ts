// SPDX-License-Identifier: GPL-3.0-or-later
// Browser mock of the whole app API (docs/APP_API.md), used automatically when the UI
// does not run inside Tauri (`npm run dev` in a normal browser, the UI tests and the
// headless screenshots). It plays no audio: playback is a simulated clock that emits
// `playback://clock` about 30 times per second, like the real engine.
//
// URL switches (for trying states by hand and for screenshots):
//   ?platform=macos        Mac app: no Studio, recorder asks for "Screen Recording"
//   ?studio=off            Studio unavailable on Windows (reasons shown)
//   ?models=missing        Studio available but none of its models are downloaded (by
//                          default the "full" preset and the lyrics models are present and
//                          the "clean" / "permissive" ones are not, as on the owner's stick)
//   ?recorder=denied|not_determined|unavailable
//   ?players=0             no media player open (the recorder's "Record & play" and
//                          "Record songs separately" then explain; default: 1 on Windows)
//   ?empty=1               empty library
//   ?many=200              add generated filler songs (library performance)
//   ?demo=1                leave out the broken-file examples (clean screenshots)
//   ?skin=0                start with the plain look (settings.ui.skin off)
//   ?lyricsmotion=letter   start with the letter-exact pace (settings.ui.lyrics_motion)
//   ?lyricsdisplay=scroll  start with the scrolling ticker instead of the page display
//                          (settings.ui.lyrics_display, D-218)
//   ?nextline=0            start without the next-line preview (settings.ui.show_next_line)
//   ?marker=0              start without the page display's progress needle (settings.ui.show_marker)
//   ?exportpeak=1.95       peak of the simulated export render (linear; above full scale
//                          a FLAC export reports the gain it applied, like the real core)
//   ?practice=none         start with no practice data on any song (WO2: by default two
//                          demo songs have it and the others not)
//   ?mic=granted|denied|not_determined|restricted|none, ?micprivacy=off, ?music=present|
//   marginal|unusable, ?singeroctave=-1: practice mode's input side (mock-practice.ts)
//   ?keymode=prerender     key changes are heard only after a simulated render ahead
//                          (key_pending.at_s null for 1.5 s), as on a slow computer
//   ?speed=12              the simulated clock runs 12 times faster (a whole take in seconds)
//   ?guidefail=1           adding practice data to Frère Jacques fails (its lead vocal cannot be read)
// Handled by the UI (App.tsx `mockStartup`), not here:
//   ?song=N&at=SECONDS     load playable song N (0-based) at a position, paused
//   &play=1                ...and start playing
//   &screen=NAME           open that screen (library, stage, sync, recorder, studio, settings)
//   &overlay=help|selfcheck, &fullscreen=1
//   &practice=1 (practice mode on the loaded song), &practice=watch, &practice=result
//   (sing to the end at 8x and show the result screen), &picker=1 (the input picker)
import type { Api, Unsubscribe } from "./client";
import { coverDataUrl, DEFAULT_SONG_SETTINGS, demoLibrary, type DemoSong, fillerSongs, promoSong, promoSongData } from "./demo";
import type {
  ApiError,
  ApiEventName,
  ApiEvents,
  AppInfo,
  AppSettings,
  AppSettingsChanges,
  DeletedList,
  DeletedSong,
  DeleteFailure,
  DeleteResult,
  GuideQueue,
  GuideStatus,
  LyricsDisplay,
  LyricsMotion,
  Device,
  ExportFormat,
  InspectResult,
  LookupResult,
  Lyrics,
  LyricsLookupQuery,
  MediaPlayer,
  MediaPlayers,
  MetadataChanges,
  PlayerState,
  ProposedExtensions,
  PurgeResult,
  RealignAnchor,
  Recording,
  RecorderMode,
  RecorderPermission,
  RecorderStatus,
  RecordedSong,
  RecordingSession,
  RestoreResult,
  SelfCheckResult,
  SessionStartOptions,
  SongDetail,
  SongSettings,
  SongSummary,
  StudioJob,
  StudioModelInfo,
  StudioModelsStatus,
  StudioPreset,
  StudioStatus,
  StudioSubmitRequest,
} from "./types";
import { STUDIO_STAGES } from "./types";
import type {
  ExcludedSpan,
  InputChoice,
  KeyPending,
  MicList,
  MicPermission,
  OctaveMode,
  PracticeGuide,
  PracticeHistory,
  PracticeResult,
  PracticeStartOptions,
  PracticeState,
  PracticeTarget,
  Strictness,
  MicInput,
  InputError,
  LaneCost,
} from "./types";
import { MockPractice, type MockPracticeOptions } from "./mock-practice";
import { spellingFor } from "../practice/pitch";
import { DEFAULT_APP_SETTINGS } from "./defaults";
import { cloneLyrics, editLineText, lyricsText } from "../lib/lyrics";
import { keyLabel } from "../lib/keys";

const NEEDED_FOR: Record<string, string> = { separation: "every song", alignment: "every song with lyrics", transcription: "songs without lyrics (transcription)" };
/** The model list the mock's Studio reports (names, sizes and licences of the real registry). */
const MOCK_MODELS: Record<string, StudioModelInfo> = Object.fromEntries(
  (
    [
      ["vocals", "bs_roformer_anvuew_ft1", "BS-RoFormer vocals ft1 (anvuew)", "bs_roformer_ft1_anvuew_sdr_12.55.ckpt", 204_493_312, "GPL-3.0", "yes", "separation"],
      ["karaoke", "bs_karaoke_anvuew", "BS-RoFormer Karaoke (anvuew)", "karaoke_bs_roformer_anvuew.ckpt", 204_486_925, "GPL-3.0", "yes", "separation"],
      ["drums", "bs_roformer_4stems_syh", "BS-RoFormer 4 stems FT (SYH99999)", "bs_roformer_4stems_ft_syh.pth", 527_245_586, "Apache-2.0", "yes", "separation"],
      ["instrumental", "bs_inst_resurrection_unwa", "BS-RoFormer Resurrection Inst (unwa)", "bs_roformer_resurrection_inst.ckpt", 204_483_033, "none published", "unclear", "separation"],
      ["vocals", "mel_roformer_kim", "Mel-Band RoFormer vocals (Kimberley Jensen)", "MelBandRoformer_kim.ckpt", 913_106_900, "MIT", "yes", "separation"],
      ["alignment", "facebook/wav2vec2-large-960h-lv60-self", "wav2vec2 large 960h lv60 self", "pytorch_model.bin", 1_262_055_246, "Apache-2.0", "yes", "alignment"],
      ["transcription", "mobiuslabsgmbh/faster-whisper-large-v3-turbo", "Whisper large-v3-turbo", "model.bin", 1_617_884_929, "MIT", "yes", "transcription"],
    ] as const
  ).map(([key, name, title, file, size_bytes, licence, commercial, task]) => [
    name,
    { key, name, title, file, size_bytes, present: false, licence, commercial, task, needed_for: NEEDED_FOR[task] },
  ]),
);
/** studio/jukeoke_studio/presets.py PRESETS (separation models per preset). */
const PRESET_MODELS: Record<StudioPreset, string[]> = {
  full: ["bs_roformer_anvuew_ft1", "bs_karaoke_anvuew", "bs_roformer_4stems_syh"],
  clean: ["bs_inst_resurrection_unwa", "bs_roformer_anvuew_ft1", "bs_karaoke_anvuew", "bs_roformer_4stems_syh"],
  permissive: ["mel_roformer_kim", "bs_roformer_4stems_syh"],
};
const LYRICS_MODELS = ["facebook/wav2vec2-large-960h-lv60-self", "mobiuslabsgmbh/faster-whisper-large-v3-turbo"];

export interface MockOptions {
  platform: "windows" | "macos";
  studio: "on" | "off";
  models: "present" | "missing";
  recorder: RecorderPermission | "unavailable";
  /** Media players open (Windows; the Mac has no player link). */
  players: number;
  empty: boolean;
  /** Leave the broken-file examples out of the library (README / LinkedIn screenshots). */
  demo: boolean;
  many: number;
  /** settings.ui.skin at start (the jukebox look); the tests run both looks. */
  skin: boolean;
  /** settings.ui.lyrics_motion at start (default "word", D-216). */
  lyricsMotion: LyricsMotion;
  /** settings.ui.lyrics_display at start (default "page", D-218). */
  lyricsDisplay: LyricsDisplay;
  /** settings.ui.show_next_line at start (default true). */
  showNextLine: boolean;
  /** settings.ui.show_marker at start (default true). */
  showMarker: boolean;
  /** Simulated IPC latency for commands, ms. */
  latencyMs: number;
  /**
   * Peak (linear) of the simulated export render. Like jk-engine's export, a `flac24`
   * export above 24-bit full scale is scaled down and reports `applied_gain_db`;
   * `wav32f` is never scaled.
   */
  exportPeak: number;
  /** WO2: songs start with no practice data at all (default: two demo songs have it). */
  noPractice: boolean;
  /** WO2: song ids whose practice data fails to save (the stick is full), for tests. */
  guideFailIds: string[];
  /** WO2: song ids whose practice data cannot be made at all (the lead vocal cannot be read). */
  guideHardFailIds: string[];
  /** WO2 practice input side (mock-practice.ts). */
  mic: MockPracticeOptions["mic"];
  micPrivacyOff: boolean;
  music: MockPracticeOptions["music"];
  singerOctave: number;
  /** WO2: how a key change becomes heard: about 0.28 s later, or after a simulated render ahead. */
  keyMode: "realtime" | "prerender";
  /** Playback speed of the simulated clock (screenshots of a whole take: ?speed=12). */
  speed: number;
  /** Clock source (ms); injectable for tests. */
  now: () => number;
  /** Timer functions; injectable for tests. */
  setInterval: (f: () => void, ms: number) => unknown;
  clearInterval: (h: unknown) => void;
}

const DEFAULT_OPTIONS: MockOptions = {
  platform: "windows",
  studio: "on",
  models: "present",
  recorder: "not_required",
  players: 1,
  empty: false,
  demo: false,
  many: 0,
  skin: true,
  lyricsMotion: "word",
  lyricsDisplay: "page",
  showNextLine: true,
  showMarker: true,
  latencyMs: 3,
  exportPeak: 0.89,
  noPractice: false,
  guideFailIds: [],
  guideHardFailIds: [],
  mic: "granted",
  micPrivacyOff: false,
  music: "off",
  singerOctave: 0,
  keyMode: "realtime",
  speed: 1,
  now: () => performance.now(),
  setInterval: (f, ms) => setInterval(f, ms),
  clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
};


const STAGE_MESSAGES: Record<(typeof STUDIO_STAGES)[number], string> = {
  decode: "Reading the file",
  separate: "Separating lead vocal",
  lyrics: "Getting the lyrics",
  align: "Timing every letter",
  analyse: "Finding the key and tempo",
  write: "Writing the song file",
};

const DEVICES: Device[] = [
  { id: "wasapi:speakers", name: "Speakers (Realtek High Definition Audio)", is_default: true, sample_rate: 48000, channels: 2, exclusive_capable: true },
  { id: "wasapi:bt-headphones", name: "Headphones (Bluetooth)", is_default: false, sample_rate: 48000, channels: 2, exclusive_capable: false },
  { id: "wasapi:usb-dac", name: "USB Audio DAC", is_default: false, sample_rate: 96000, channels: 2, exclusive_capable: true },
];

const fail = (code: string, message: string): ApiError => ({ code, message });

const MOCK_PLAYERS: MediaPlayer[] = [
  { id: "MusicPlayer.exe", source_app_id: "MusicPlayer.exe", app_name: "MusicPlayer", is_current: true, status: "paused", title: "Morning Light", artist: "The Demo Band", album: "Demo Album", duration_s: 214, can_play: true, can_pause: true },
  { id: "MSEdge", source_app_id: "MSEdge", app_name: "Microsoft Edge", is_current: false, status: "paused", title: "A video", artist: "", album: "", duration_s: 600, can_play: true, can_pause: true },
  { id: "Radio.exe", source_app_id: "Radio.exe", app_name: "Radio", is_current: false, status: "stopped", title: "", artist: "", album: "", duration_s: null, can_play: true, can_pause: false },
];

interface MockTrack {
  title: string;
  artist: string;
  album: string;
  duration_s: number;
  ad?: boolean;
}

const MOCK_PLAYLIST: MockTrack[] = [
  { title: "Morning Light", artist: "The Demo Band", album: "Demo Album", duration_s: 214 },
  { title: "Paper Boats", artist: "The Demo Band", album: "Demo Album", duration_s: 187 },
  { title: "Advertisement", artist: "", album: "", duration_s: 15, ad: true },
  { title: "Slow River", artist: "Example Trio", album: "Samples", duration_s: 251 },
];

function mockSong(folder: string, n: number, t: MockTrack, status: RecordedSong["status"], reason: string | null): RecordedSong {
  const stem = `${String(n).padStart(2, "0")} ${t.artist} - ${t.title}`;
  const dir = `E:\\Recordings\\${folder}`;
  return {
    path: `${dir}\\${stem}.wav`,
    name: `${stem}.wav`,
    number: n,
    title: t.title,
    artist: t.artist,
    album: t.album,
    cover_path: `${dir}\\${stem}.jpg`,
    duration_s: status === "partial" ? 61.2 : t.duration_s + 1.1,
    track_duration_s: t.duration_s,
    status,
    partial_reason: reason,
    start_confidence: n === 1 ? "edge" : "certain",
    end_confidence: status === "partial" ? "edge" : n === 2 ? "uncertain" : "certain",
    paused_s: 0,
  };
}

function demoSession(): RecordingSession {
  const folder = "2026-11-17 20-15-03";
  const songs = [
    mockSong(folder, 1, MOCK_PLAYLIST[0], "complete", null),
    mockSong(folder, 2, MOCK_PLAYLIST[1], "complete", null),
    mockSong(folder, 3, MOCK_PLAYLIST[3], "partial", "It is shorter than the track: it was skipped or interrupted."),
  ];
  return {
    folder,
    path: `E:\\Recordings\\${folder}`,
    created: folder,
    player: "MusicPlayer",
    player_id: "MusicPlayer.exe",
    state: "split",
    progress: null,
    stop_reason: "idle",
    message: null,
    master_present: false,
    song_count: 3,
    songs,
    excluded: [
      { path: `E:\\Recordings\\${folder}\\_excluded\\Part 03 - advertisement - Advertisement.wav`, name: "Part 03 - advertisement - Advertisement.wav", reason: "advertisement", duration_s: 15.9 },
      { path: `E:\\Recordings\\${folder}\\_excluded\\Part 05 - after playback stopped.wav`, name: "Part 05 - after playback stopped.wav", reason: "after playback stopped", duration_s: 29.0 },
    ],
  };
}

function deepMerge<T>(base: T, changes: unknown): T {
  if (changes === null || typeof changes !== "object" || Array.isArray(changes)) return changes as T;
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(changes as Record<string, unknown>)) {
    const cur = out[k];
    out[k] = v !== null && typeof v === "object" && !Array.isArray(v) && cur && typeof cur === "object" ? deepMerge(cur, v) : v;
  }
  return out as T;
}

interface MockSongState {
  demo: DemoSong;
  settings: SongSettings;
  lyricsOverride: Lyrics | null;
  metadata: MetadataChanges;
  /** WO2: the song's practice data (melody guide). */
  guide: GuideStatus;
}

/** A guide status with no guide (APP_API 12.3). */
export function noGuide(reason = "Practice data has not been added to this song yet."): GuideStatus {
  return { state: "none", reason, error_code: null, progress: null, in_memory: false, pending_write: false, method: null, version: null, created_utc: null, counts: null, tuning_cents: null };
}

/** A ready guide as the real core reports one. */
export function readyGuide(notes = 120): GuideStatus {
  return {
    ...noGuide(),
    state: "ready",
    reason: null,
    method: "jk-pitch pyin+notes",
    version: "0.1.0",
    created_utc: "2026-11-19T12:00:00Z",
    counts: { notes, pitched: notes - 10, unpitched: 10, low_confidence: 12, octave_flags: 2, graded: notes - 24 },
    tuning_cents: 0,
  };
}

export class MockApi implements Api {
  readonly kind = "mock" as const;
  readonly htmlDropGivesPaths = true;
  private o: MockOptions;
  private handlers = new Map<string, Set<(p: unknown) => void>>();
  private songs: MockSongState[];
  private settings: AppSettings = JSON.parse(JSON.stringify(DEFAULT_APP_SETTINGS)) as AppSettings;
  private current: MockSongState | null = null;
  private playing = false;
  private anchorPos = 0;
  private anchorT = 0;
  private seq = 0;
  private ended = false;
  private ticker: unknown = null;
  private recorder: RecorderStatus = { state: "idle", elapsed_s: 0, format: null, peak_db: [], clipped: false, dropped_s: 0, error: null };
  private recStart = 0;
  private recTicker: unknown = null;
  private recordings: Recording[] = [];
  private recMode: RecorderMode = "plain";
  private mediaPlayers: MediaPlayer[] = [];
  private live: { player: MediaPlayer; folder: string; track: number; played: number[]; opts: SessionStartOptions; lastSwitch: number; idleSince: number | null } | null = null;
  private sessions: RecordingSession[] = [];
  private previewTimer: ReturnType<typeof setTimeout> | null = null;
  private permission: RecorderPermission;
  private jobs: StudioJob[] = [];
  private jobTicker: unknown = null;
  /** Model names on the simulated drive (MOCK_MODELS keys). */
  private modelsOnDisk: Set<string>;
  private nextId = 1;
  /** Deleted songs (newest last), each with the song state a restore brings back. */
  private deletedSongs: { entry: DeletedSong; song: MockSongState }[] = [];
  /** Bytes freed by "delete for good" (the free space grows by them). */
  private freedBytes = 0;
  /** WO2 practice data: songs waiting, the one being made, the batch, its timer. */
  private guideWaiting: string[] = [];
  private guideRunning: string | null = null;
  private guideBatchState: { total: number; done: number; failed: number; ids: Set<string> } | null = null;
  private guideTicker: unknown = null;
  /** WO2 (D-239): the key HEARD, the next change and the playback segment, like the engine. */
  private keyHeard = 0;
  private keyPending: KeyPending | null = null;
  private keySegment = 1;
  private pendingTimer: ReturnType<typeof setTimeout> | null = null;
  /** WO2 practice mode (mock-practice.ts). */
  private practice: MockPractice;

  constructor(options: Partial<MockOptions> = {}) {
    this.o = { ...DEFAULT_OPTIONS, ...options };
    this.permission =
      this.o.recorder === "unavailable" ? "not_required" : this.o.platform === "macos" && this.o.recorder === "not_required" ? "not_determined" : this.o.recorder;
    this.modelsOnDisk = new Set(this.o.models === "present" ? [...PRESET_MODELS.full, ...LYRICS_MODELS] : []);
    this.settings.ui.skin = this.o.skin;
    this.settings.ui.lyrics_motion = this.o.lyricsMotion;
    this.settings.ui.lyrics_display = this.o.lyricsDisplay;
    this.settings.ui.show_next_line = this.o.showNextLine;
    this.settings.ui.show_marker = this.o.showMarker;
    // A real song injected by a recording script comes first (demo.ts promoSong).
    const promo = this.o.empty ? null : promoSong();
    const lib = this.o.empty ? [] : [...(promo ? [promo] : []), ...demoLibrary().filter((d) => !this.o.demo || !d.summary.error), ...fillerSongs(this.o.many)];
    const shift = promo ? 1 : 0;
    this.songs = lib.map((demo, i) => ({
      demo,
      settings: { ...demo.settings, levels: { ...demo.settings.levels } },
      lyricsOverride: null,
      metadata: {},
      // The first and third demo songs have practice data; the others not yet.
      guide: !this.o.noPractice && !demo.summary.error && (demo.storedGuide || i - shift === 0 || i - shift === 2) ? readyGuide(100 + i * 20) : noGuide(),
    }));
    if (this.o.platform === "windows") {
      this.mediaPlayers = MOCK_PLAYERS.slice(0, this.o.players).map((p) => ({ ...p }));
    }
    if (!this.o.empty && this.o.platform === "windows") this.sessions = [demoSession()];
    this.recordings = [
      { path: "Recordings/2026-11-16 21-04-12.wav", name: "2026-11-16 21-04-12.wav", duration_s: 187.4, sample_rate: 48000, channels: 2, bytes: 71_965_000, clipped: false },
      { path: "Recordings/2026-11-15 19-30-55.wav", name: "2026-11-15 19-30-55.wav", duration_s: 64.0, sample_rate: 48000, channels: 2, bytes: 24_576_000, clipped: true },
    ];
    const mic = this.o.mic === "granted" && this.o.platform === "macos" && !options.mic ? "not_determined" : this.o.mic;
    this.practice = new MockPractice(
      {
        now: () => this.o.now(),
        position: () => this.position(),
        playing: () => this.playing,
        ended: () => this.ended,
        song: () => this.current?.demo ?? null,
        semitones: () => this.current?.settings.semitones ?? 0,
        keyAt: (t) => this.keyAtSong(t),
        segment: () => this.keySegment,
        settings: () => this.settings,
        guideStatus: (id) => {
          const s = this.find(id);
          return s ? { ...s.guide } : null;
        },
        buildGuide: (id) => void this.guideBuild(id),
        emit: (event, payload) => this.emit(event as ApiEventName, payload as never),
        setInterval: (f, ms) => this.o.setInterval(f, ms),
        clearInterval: (h) => this.o.clearInterval(h),
        pause: () => void this.playerPause(),
        seek: (t) => void this.playerSeek(t),
        play: () => void this.playerPlay(),
      },
      { platform: this.o.platform, mic, micPrivacyOff: this.o.micPrivacyOff, music: this.o.music, singerOctave: this.o.singerOctave },
    );
  }

  /** Reads the URL switches documented at the top of this file. */
  static optionsFromUrl(search: string): Partial<MockOptions> {
    const q = new URLSearchParams(search);
    const o: Partial<MockOptions> = {};
    if (q.get("platform") === "macos") o.platform = "macos";
    if (q.get("studio") === "off") o.studio = "off";
    if (q.get("models") === "missing") o.models = "missing";
    const r = q.get("recorder");
    if (r === "denied" || r === "not_determined" || r === "granted" || r === "unavailable") o.recorder = r;
    if (q.get("empty") === "1") o.empty = true;
    if (q.get("demo") === "1") o.demo = true;
    const players = Number(q.get("players"));
    if (q.has("players") && players >= 0) o.players = Math.min(3, Math.floor(players));
    if (q.get("skin") === "0") o.skin = false;
    const lm = q.get("lyricsmotion");
    if (lm === "letter" || lm === "word") o.lyricsMotion = lm;
    const ld = q.get("lyricsdisplay");
    if (ld === "page" || ld === "scroll") o.lyricsDisplay = ld;
    if (q.get("nextline") === "0") o.showNextLine = false;
    if (q.get("marker") === "0") o.showMarker = false;
    const many = Number(q.get("many"));
    if (many > 0) o.many = Math.min(2000, Math.floor(many));
    if (q.get("practice") === "none") o.noPractice = true;
    const mic = q.get("mic");
    if (mic === "granted" || mic === "denied" || mic === "not_determined" || mic === "restricted" || mic === "none") o.mic = mic;
    if (q.get("micprivacy") === "off") o.micPrivacyOff = true;
    const music = q.get("music");
    if (music === "present" || music === "marginal" || music === "unusable") o.music = music;
    const so = Number(q.get("singeroctave"));
    if (q.has("singeroctave") && Number.isInteger(so) && Math.abs(so) <= 2) o.singerOctave = so;
    if (q.get("keymode") === "prerender") o.keyMode = "prerender";
    if (q.get("guidefail") === "1") o.guideHardFailIds = ["Traditional - Frère Jacques.jksong"];
    const speed = Number(q.get("speed"));
    if (speed > 0 && speed <= 32) o.speed = speed;
    const peak = Number(q.get("exportpeak"));
    if (peak > 0 && Number.isFinite(peak)) o.exportPeak = peak;
    return o;
  }

  /** Tests: stop every timer and forget every listener (the next test starts clean). */
  dispose(): void {
    for (const t of [this.ticker, this.recTicker, this.jobTicker, this.guideTicker]) if (t !== null) this.o.clearInterval(t);
    this.ticker = this.recTicker = this.jobTicker = this.guideTicker = null;
    if (this.previewTimer) clearTimeout(this.previewTimer);
    this.previewTimer = null;
    if (this.pendingTimer) clearTimeout(this.pendingTimer);
    this.pendingTimer = null;
    this.practice.dispose();
    this.handlers.clear();
  }

  // --- plumbing -------------------------------------------------------------

  private reply<T>(value: T): Promise<T> {
    return new Promise((resolve) => setTimeout(() => resolve(value), this.o.latencyMs));
  }
  private reject<T>(err: ApiError): Promise<T> {
    return new Promise((_, reject) => setTimeout(() => reject(err), this.o.latencyMs));
  }

  on<E extends ApiEventName>(event: E, handler: (payload: ApiEvents[E]) => void): Unsubscribe {
    let set = this.handlers.get(event);
    if (!set) this.handlers.set(event, (set = new Set()));
    const h = handler as (p: unknown) => void;
    set.add(h);
    return () => set.delete(h);
  }

  private emit<E extends ApiEventName>(event: E, payload: ApiEvents[E]): void {
    const set = this.handlers.get(event);
    if (!set) return;
    for (const h of [...set]) {
      try {
        h(payload);
      } catch (err) {
        console.error(`mock: handler for ${event} threw`, err);
      }
    }
  }

  async setWindowFullscreen(on: boolean): Promise<boolean> {
    const doc = globalThis.document;
    if (!doc?.documentElement) return false;
    try {
      if (on && !doc.fullscreenElement && doc.documentElement.requestFullscreen) await doc.documentElement.requestFullscreen();
      if (!on && doc.fullscreenElement) await doc.exitFullscreen();
      return true;
    } catch {
      return false;
    }
  }

  // --- 1. app ---------------------------------------------------------------

  appInfo(): Promise<AppInfo> {
    const mac = this.o.platform === "macos";
    const studioReasons = mac
      ? ["The Studio runs on Windows computers with an NVIDIA graphics card. Songs are created there and play anywhere, including this Mac."]
      : this.o.studio === "off"
        ? [
            "No NVIDIA graphics card with CUDA was found. Songs are created on a stronger computer and can be played anywhere.",
            "This computer has 8 GB of memory; the Studio needs at least 12 GB.",
          ]
        : [];
    const root = mac ? "/Volumes/OPEN MIC" : "E:\\";
    const p = (name: string) => (mac ? `${root}/${name}` : `${root}${name}`);
    return this.reply<AppInfo>({
      name: "Open Mic",
      version: "0.1.0",
      platform: this.o.platform,
      os_version: mac ? "macOS 13.6.9" : "Windows 11 26200",
      portable: true,
      root,
      translocated: false,
      paths: { library: p("Library"), recordings: p("Recordings"), exports: p("Exports"), settings: p("Settings"), models: mac ? null : p("Models"), deleted: p("Deleted") },
      capabilities: {
        studio: { available: studioReasons.length === 0, reasons: studioReasons, gpu: studioReasons.length === 0 ? "NVIDIA GeForce RTX 5070 Ti" : null, vram_mb: studioReasons.length === 0 ? 16303 : null },
        recorder: {
          available: this.o.recorder !== "unavailable",
          reason: this.o.recorder === "unavailable" ? "No output device supports capture on this computer." : null,
          permission: this.permission,
        },
        exclusive_mode: !mac,
        microphone: {
          available: this.o.mic !== "none",
          reason: this.o.mic === "none" ? "No microphone or audio input was found on this computer." : null,
          permission: this.practice.micPermission(),
          backend: mac ? "Core Audio" : "WASAPI",
        },
        practice: { guide_method: "jk-pitch pyin+notes", guide_version: "0.1.0" },
      },
    });
  }

  // --- 2. library -----------------------------------------------------------

  private summaryOf(s: MockSongState): SongSummary {
    const lyr = s.lyricsOverride ?? s.demo.lyrics;
    return {
      ...s.demo.summary,
      ...(s.metadata.title !== undefined ? { title: s.metadata.title } : {}),
      ...(s.metadata.artist !== undefined ? { artist: s.metadata.artist } : {}),
      ...(s.metadata.album !== undefined ? { album: s.metadata.album } : {}),
      semitones: s.settings.semitones,
      has_lyrics: s.demo.summary.error ? false : lyr !== null,
      lyrics_checked: lyr?.checked ?? false,
      guide: { ...s.guide },
    };
  }

  // --- 12.3 practice data (WO2) ------------------------------------------------
  // Like the core's guide queue: one build at a time, progress events, a batch for
  // "Add practice data to all songs", cancel. A build takes about a second here.

  private guideEmit(s: MockSongState): void {
    this.emit("guide://status", { song_id: s.demo.summary.song_id, status: { ...s.guide } });
    this.practice?.onGuideStatus(s.demo.summary.song_id, { ...s.guide });
  }

  private guideBatchEmit(): void {
    const b = this.guideBatchState;
    if (!b) return;
    const running = this.guideWaiting.some((id) => b.ids.has(id)) || (this.guideRunning !== null && b.ids.has(this.guideRunning));
    this.emit("guide://batch", { total: b.total, done: b.done, failed: b.failed, current: running ? this.guideRunning : null, running });
    if (!running) this.guideBatchState = null;
  }

  private guideTick(): void {
    if (this.guideRunning === null) {
      const next = this.guideWaiting.shift();
      if (next === undefined) {
        if (this.guideTicker !== null) this.o.clearInterval(this.guideTicker);
        this.guideTicker = null;
        return;
      }
      this.guideRunning = next;
      const s = this.find(next);
      if (s) {
        s.guide = { ...s.guide, state: "building", progress: 0, reason: null, error_code: null };
        this.guideEmit(s);
        this.guideBatchEmit();
      }
      return;
    }
    const s = this.find(this.guideRunning);
    if (!s) {
      this.guideRunning = null;
      return;
    }
    const p = Math.min(1, (s.guide.progress ?? 0) + 0.25);
    if (p < 1) {
      s.guide = { ...s.guide, progress: p };
      this.guideEmit(s);
      return;
    }
    const failed = this.o.guideFailIds.includes(s.demo.summary.song_id);
    const hard = this.o.guideHardFailIds.includes(s.demo.summary.song_id);
    s.guide = hard
      ? { ...noGuide("The lead vocal in this song file could not be read, so no practice data could be made. Copy the song again from the original and press Try again."), state: "failed", error_code: "guide_read_failed" }
      : failed
        ? { ...noGuide("Practice data could not be saved: the drive is full. Free 150 MB and press Try again."), state: "failed", error_code: "guide_disk_full", in_memory: true }
        : readyGuide();
    const b = this.guideBatchState;
    if (b && b.ids.has(s.demo.summary.song_id)) {
      b.done += 1;
      if (failed) b.failed += 1;
    }
    this.guideRunning = null;
    this.guideEmit(s);
    this.guideBatchEmit();
  }

  private guideStart(): void {
    if (this.guideTicker === null) this.guideTicker = this.o.setInterval(() => this.guideTick(), 120);
  }

  guideStatus(songId: string): Promise<GuideStatus> {
    const s = this.find(songId);
    return s ? this.reply({ ...s.guide }) : this.reject(fail("not_found", "This song is not in the library."));
  }

  guideBuild(songId: string, force = false): Promise<GuideStatus> {
    const s = this.find(songId);
    if (!s) return this.reject(fail("not_found", "This song is not in the library."));
    if (s.demo.summary.error) return this.reply({ ...noGuide(s.demo.summary.error), state: "failed", error_code: "guide_read_failed" });
    const busy = this.guideRunning === songId || this.guideWaiting.includes(songId);
    const wanted = s.guide.state === "ready" || s.guide.state === "newer" ? force && s.guide.state === "ready" : true;
    if (!busy && wanted) {
      this.guideWaiting.unshift(songId);
      s.guide = { ...s.guide, state: "queued", progress: null, reason: null, error_code: null };
      this.guideEmit(s);
      this.guideStart();
    }
    return this.reply({ ...s.guide });
  }

  guideBuildAll(includeOutdated = false): Promise<{ queued: number; skipped: number }> {
    let queued = 0;
    let skipped = 0;
    const ids: string[] = [];
    for (const s of this.songs) {
      const id = s.demo.summary.song_id;
      const busy = this.guideRunning === id || this.guideWaiting.includes(id);
      const st = s.guide.state;
      const want = !s.demo.summary.error && !busy && (st === "none" || st === "stale" || (st === "failed" && s.guide.error_code !== "guide_no_lead") || (includeOutdated && st === "outdated"));
      if (!want) {
        skipped += 1;
        continue;
      }
      queued += 1;
      ids.push(id);
      this.guideWaiting.push(id);
      s.guide = { ...s.guide, state: "queued", progress: null, reason: null, error_code: null };
      this.guideEmit(s);
    }
    if (queued > 0) {
      const b = (this.guideBatchState ??= { total: 0, done: 0, failed: 0, ids: new Set() });
      b.total += queued;
      for (const id of ids) b.ids.add(id);
      this.guideBatchEmit();
      this.guideStart();
    }
    return this.reply({ queued, skipped });
  }

  guideCancel(songId?: string): Promise<void> {
    const hit = (id: string) => songId === undefined || songId === id;
    const touched = [...this.guideWaiting.filter(hit), ...(this.guideRunning !== null && hit(this.guideRunning) ? [this.guideRunning] : [])];
    this.guideWaiting = this.guideWaiting.filter((id) => !hit(id));
    if (this.guideRunning !== null && hit(this.guideRunning)) this.guideRunning = null;
    for (const id of touched) {
      const s = this.find(id);
      if (!s) continue;
      s.guide = { ...noGuide("Cancelled."), state: "failed", error_code: "cancelled" };
      this.guideEmit(s);
      const b = this.guideBatchState;
      if (b && b.ids.delete(id)) b.total = Math.max(0, b.total - 1);
    }
    this.guideBatchEmit();
    return this.reply(undefined);
  }

  guideQueue(): Promise<GuideQueue> {
    const items: GuideQueue["items"] = [];
    if (this.guideRunning !== null) items.push({ song_id: this.guideRunning, state: "building", progress: this.find(this.guideRunning)?.guide.progress ?? null });
    for (const id of this.guideWaiting) items.push({ song_id: id, state: "queued", progress: null });
    return this.reply({ items, running: this.guideRunning !== null });
  }

  libraryList() {
    const d = this.deletedListNow();
    return this.reply({
      songs: this.songs.map((s) => this.summaryOf(s)),
      free_bytes: 18_734_000_000 + this.freedBytes,
      total_bytes: 30_920_000_000,
      deleted_count: d.count,
      deleted_bytes: d.total_bytes,
    });
  }

  // Deleted songs (APP_API section 2, the owner's request of 2026-11-18): like the core,
  // a delete moves the song aside, a restore puts it back (a free name when its place is
  // taken), a purge deletes it for good and frees its bytes.

  /** `stem (n).ext`: the first name `free` accepts. */
  private static numbered(name: string, free: (n: string) => boolean): string {
    if (free(name)) return name;
    const dot = name.lastIndexOf(".");
    const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
    for (let n = 2; ; n++) if (free(`${stem} (${n})${ext}`)) return `${stem} (${n})${ext}`;
  }

  private restoreTarget(original: string, taken: Set<string> = new Set()): string {
    const slash = original.lastIndexOf("/");
    const folder = slash >= 0 ? original.slice(0, slash + 1) : "";
    const inLibrary = new Set(this.songs.map((s) => s.demo.summary.song_id.toLowerCase()));
    return folder + MockApi.numbered(original.slice(slash + 1), (n) => !inLibrary.has((folder + n).toLowerCase()) && !taken.has((folder + n).toLowerCase()));
  }

  private deletedListNow(): DeletedList {
    const songs = [...this.deletedSongs]
      .reverse()
      .map(({ entry }) => ({ ...entry, restore_to: this.restoreTarget(entry.original_song_id) }));
    return { songs, count: songs.length, total_bytes: songs.reduce((t, s) => t + s.file_bytes, 0), folder: this.o.platform === "macos" ? "/Volumes/OPEN MIC/Deleted" : "E:\\Deleted" };
  }

  private deletedChanged(): void {
    this.emit("library://deleted", this.deletedListNow());
    this.emit("library://changed", null);
  }

  libraryDelete(songIds: string[]): Promise<DeleteResult> {
    if (songIds.length === 0) return this.reject(fail("bad_request", "Choose at least one song to delete."));
    const deleted: DeletedSong[] = [];
    const failed: DeleteFailure[] = [];
    let unloaded: string | null = null;
    for (const id of new Set(songIds)) {
      const s = this.find(id);
      if (!s) {
        failed.push({ id, code: "not_found", message: "This song is not in the library." });
        continue;
      }
      if (this.current === s) {
        // Stopped and closed first, as the core does.
        this.stopTicker();
        this.playing = false;
        this.ended = false;
        this.anchorPos = 0;
        this.current = null;
        unloaded = id;
        this.emitState();
      }
      const taken = new Set(this.deletedSongs.map((d) => d.entry.deleted_id.toLowerCase()));
      const name = MockApi.numbered(id.slice(id.lastIndexOf("/") + 1), (n) => !taken.has(n.toLowerCase()));
      const sum = this.summaryOf(s);
      const entry: DeletedSong = {
        deleted_id: name,
        original_song_id: id,
        restore_to: id,
        title: sum.title,
        artist: sum.artist,
        album: sum.album,
        duration_s: sum.duration_s,
        file_bytes: sum.file_bytes,
        deleted_ms: Date.now(),
      };
      this.songs = this.songs.filter((x) => x !== s);
      this.deletedSongs.push({ entry, song: s });
      deleted.push(entry);
    }
    if (deleted.length > 0) this.deletedChanged();
    return this.reply({ deleted, failed, unloaded });
  }

  libraryDeletedList(): Promise<DeletedList> {
    return this.reply(this.deletedListNow());
  }

  libraryRestore(deletedIds: string[]): Promise<RestoreResult> {
    if (deletedIds.length === 0) return this.reject(fail("bad_request", "Choose at least one song to restore."));
    const restored: RestoreResult["restored"] = [];
    const failed: DeleteFailure[] = [];
    for (const id of deletedIds) {
      const i = this.deletedSongs.findIndex((d) => d.entry.deleted_id.toLowerCase() === id.toLowerCase());
      if (i < 0) {
        failed.push({ id, code: "not_found", message: "This song is no longer in Deleted songs." });
        continue;
      }
      const [{ entry, song }] = this.deletedSongs.splice(i, 1);
      const to = this.restoreTarget(entry.original_song_id);
      song.demo = { ...song.demo, summary: { ...song.demo.summary, song_id: to } };
      this.songs.push(song);
      restored.push({ deleted_id: entry.deleted_id, song_id: to });
    }
    if (restored.length > 0) this.deletedChanged();
    return this.reply({ restored, failed });
  }

  libraryPurge(which: string[] | "all"): Promise<PurgeResult> {
    if (which !== "all" && which.length === 0) return this.reject(fail("bad_request", "Choose at least one song to delete for good."));
    const purged: string[] = [];
    const failed: DeleteFailure[] = [];
    let freed = 0;
    const ids = which === "all" ? this.deletedSongs.map((d) => d.entry.deleted_id) : which;
    for (const id of ids) {
      const i = this.deletedSongs.findIndex((d) => d.entry.deleted_id.toLowerCase() === id.toLowerCase());
      if (i < 0) {
        failed.push({ id, code: "not_found", message: "This song is no longer in Deleted songs." });
        continue;
      }
      const [{ entry }] = this.deletedSongs.splice(i, 1);
      freed += entry.file_bytes;
      purged.push(entry.deleted_id);
    }
    this.freedBytes += freed;
    if (purged.length > 0) this.deletedChanged();
    return this.reply({ purged, freed_bytes: freed, failed });
  }

  songCover(songId: string, maxPx: number) {
    const s = this.find(songId);
    if (!s) return this.reject<{ data_url: string | null }>(fail("not_found", "That song is no longer in the library."));
    return this.reply({ data_url: s.demo.cover ? coverDataUrl(s.demo.summary.title, s.demo.cover, maxPx) : null });
  }

  private find(songId: string): MockSongState | undefined {
    return this.songs.find((s) => s.demo.summary.song_id === songId);
  }

  // --- 3. playback ----------------------------------------------------------

  private position(): number {
    if (!this.current) return 0;
    if (!this.playing) return this.anchorPos;
    return Math.min(this.current.demo.summary.duration_s, this.anchorPos + ((this.o.now() - this.anchorT) / 1000) * this.o.speed);
  }

  private stateName(): PlayerState["state"] {
    if (!this.current) return "empty";
    if (this.ended) return "ended";
    return this.playing ? "playing" : "paused";
  }

  private device(): Device {
    return DEVICES.find((d) => d.id === this.settings.audio.device_id) ?? DEVICES[0];
  }

  private fullState(): PlayerState & ProposedExtensions {
    const dev = this.device();
    const cur = this.current;
    return {
      state: this.stateName(),
      song_id: cur?.demo.summary.song_id ?? null,
      position_s: this.position(),
      duration_s: cur?.demo.summary.duration_s ?? 0,
      device: { name: dev.name, sample_rate: dev.sample_rate, channels: dev.channels, exclusive: this.settings.audio.exclusive && dev.exclusive_capable, latency_ms: dev.id.includes("bt") ? 180 : 12 },
      underruns: 0,
      error: null,
      key_label: cur ? keyLabel(cur.demo.summary.key, cur.settings.semitones, "->") : null,
      output_peak_db: this.playing ? [-9.5, -10.2] : [-120, -120],
      clipped: false,
      device_starved: 0,
      key_mode: this.o.keyMode,
      key_heard: this.keyHeard,
      key_requested: cur?.settings.semitones ?? 0,
      key_pending: this.keyPending ? { ...this.keyPending } : null,
      key_segment: this.keySegment,
      key_heard_label: cur ? keyLabel(cur.demo.summary.key, this.keyHeard, "->") : null,
      key_spelling: spellingFor(cur?.demo.summary.key ?? null, this.keyHeard),
      key_pending_spelling: this.keyPending ? spellingFor(cur?.demo.summary.key ?? null, this.keyPending.semitones) : null,
    };
  }

  // --- the key heard (D-239), simulated -----------------------------------------
  // Real time: a change is heard about 0.28 s after the request (the engine's render
  // lead), from a song time given in key_pending.at_s. Render ahead (?keymode=prerender):
  // at_s stays null while the key is "rendered" (1.5 s), then it is scheduled. While
  // paused a change is heard at once, in a new segment (the engine re-renders).

  /** The key heard at song time t (the engine's key_at for the current segment). */
  private keyAtSong(t: number): number {
    const p = this.keyPending;
    return p && p.at_s !== null && t >= p.at_s ? p.semitones : this.keyHeard;
  }

  /** A pending change whose song time has come is now heard. */
  private settleKey(): boolean {
    const p = this.keyPending;
    if (!p || p.at_s === null || this.position() < p.at_s) return false;
    this.keyHeard = p.semitones;
    this.keyPending = null;
    return true;
  }

  private requestKey(semitones: number): void {
    if (this.pendingTimer) clearTimeout(this.pendingTimer);
    this.pendingTimer = null;
    if (semitones === (this.keyPending?.semitones ?? this.keyHeard)) return;
    if (!this.playing) {
      this.keyHeard = semitones;
      this.keyPending = null;
      this.keySegment += 1;
      return;
    }
    if (this.o.keyMode === "prerender") {
      this.keyPending = { semitones, at_s: null };
      this.pendingTimer = setTimeout(() => {
        this.pendingTimer = null;
        if (this.keyPending?.semitones !== semitones) return;
        if (!this.playing) {
          this.keyHeard = semitones;
          this.keyPending = null;
          this.keySegment += 1;
        } else this.keyPending = { semitones, at_s: Math.round((this.position() + 0.25) * 1000) / 1000 };
        this.emitState();
      }, 1500);
    } else {
      this.keyPending = { semitones, at_s: Math.round((this.position() + 0.28) * 1000) / 1000 };
    }
  }

  private emitClock(): void {
    this.seq += 1;
    if (this.settleKey()) this.emit("playback://state", this.fullState());
    this.emit("playback://clock", {
      position_s: this.position(),
      playing: this.playing,
      seq: this.seq,
      key_heard: this.keyHeard,
      key_pending: this.keyPending ? { ...this.keyPending } : null,
      segment: this.keySegment,
    });
  }
  private emitState(): void {
    this.emit("playback://state", this.fullState());
    this.emitClock();
    this.practice?.onPlayer();
  }

  private startTicker(): void {
    if (this.ticker !== null) return;
    this.ticker = this.o.setInterval(() => {
      if (!this.current || !this.playing) return;
      if (this.position() >= this.current.demo.summary.duration_s) {
        this.anchorPos = this.current.demo.summary.duration_s;
        this.playing = false;
        this.ended = true;
        this.stopTicker();
        this.emitState();
        return;
      }
      this.emitClock();
    }, 33);
  }
  private stopTicker(): void {
    if (this.ticker !== null) this.o.clearInterval(this.ticker);
    this.ticker = null;
  }

  songLoad(songId: string): Promise<SongDetail> {
    const s = this.find(songId);
    if (!s) return this.reject(fail("not_found", "That song is no longer in the library."));
    if (s.demo.summary.error) return this.reject(fail("unplayable", s.demo.summary.error));
    this.stopTicker();
    this.current = s;
    this.playing = false;
    this.ended = false;
    this.anchorPos = 0;
    this.anchorT = this.o.now();
    if (this.practice.state().active && this.practice.state().song_id !== songId) this.practice.stop(true);
    this.keyHeard = s.settings.semitones;
    this.keyPending = null;
    this.keySegment += 1;
    const lyrics = s.lyricsOverride ?? s.demo.lyrics;
    const detail: SongDetail = {
      summary: this.summaryOf(s),
      lyrics: lyrics ? cloneLyrics(lyrics) : null,
      lyrics_is_override: s.lyricsOverride !== null,
      settings: { ...s.settings, levels: { ...s.settings.levels } },
      stems: s.demo.stems,
      sample_rate: s.demo.sample_rate,
      channels: s.demo.channels,
      guide: { ...s.guide },
    };
    setTimeout(() => this.emitState(), 0);
    return this.reply(detail);
  }

  playerPlay() {
    if (!this.current) return this.reject<void>(fail("no_song", "Choose a song first."));
    if (this.ended) {
      this.anchorPos = 0;
      this.ended = false;
    }
    if (!this.playing) {
      this.anchorT = this.o.now();
      this.playing = true;
      this.startTicker();
    }
    this.emitState();
    return this.reply(undefined);
  }
  playerPause() {
    if (this.playing) {
      this.anchorPos = this.position();
      this.playing = false;
      this.stopTicker();
    }
    this.emitState();
    return this.reply(undefined);
  }
  playerToggle() {
    return this.playing ? this.playerPause() : this.playerPlay();
  }
  playerStop() {
    this.playing = false;
    this.stopTicker();
    this.anchorPos = 0;
    this.ended = false;
    this.emitState();
    return this.reply(undefined);
  }
  playerSeek(positionS: number) {
    if (!this.current) return this.reject<void>(fail("no_song", "Choose a song first."));
    this.anchorPos = Math.max(0, Math.min(this.current.demo.summary.duration_s, positionS));
    this.anchorT = this.o.now();
    this.ended = false;
    // A seek starts a new segment in the key last asked for (a change still rendering keeps waiting).
    if (this.keyPending?.at_s !== null && this.keyPending) {
      this.keyHeard = this.keyPending.semitones;
      this.keyPending = null;
    }
    this.keySegment += 1;
    this.practice.onSeek();
    this.emitState();
    return this.reply(undefined);
  }
  playerSet(changes: Partial<SongSettings>) {
    if (!this.current) return this.reject<void>(fail("no_song", "Choose a song first."));
    if (changes.semitones !== undefined && (!Number.isInteger(changes.semitones) || Math.abs(changes.semitones) > 12)) {
      return this.reject<void>(fail("bad_request", "The key can move at most 12 semitones up or down."));
    }
    this.current.settings = deepMerge(this.current.settings, changes);
    if (changes.semitones !== undefined) this.requestKey(changes.semitones);
    this.emit("playback://state", this.fullState());
    this.emitClock();
    return this.reply(undefined);
  }
  playerSetMaster(volume: number) {
    this.settings.audio.master_volume = Math.max(0, Math.min(1, volume));
    return this.reply(undefined);
  }
  playerState() {
    return this.reply(this.fullState() as PlayerState);
  }

  // --- 4. devices -----------------------------------------------------------

  devicesList() {
    return this.reply({ devices: DEVICES.map((d) => ({ ...d })) });
  }
  deviceSet(id: string | null, exclusive: boolean) {
    if (id !== null && !DEVICES.some((d) => d.id === id)) return this.reject<void>(fail("not_found", "That output device is not connected any more."));
    this.settings.audio.device_id = id;
    this.settings.audio.exclusive = exclusive;
    this.emit("playback://state", this.fullState());
    return this.reply(undefined);
  }

  // --- 5. settings ----------------------------------------------------------

  settingsGet() {
    return this.reply(JSON.parse(JSON.stringify(this.settings)) as AppSettings);
  }
  settingsSet(changes: AppSettingsChanges) {
    this.settings = deepMerge(this.settings, changes);
    return this.reply(JSON.parse(JSON.stringify(this.settings)) as AppSettings);
  }

  // --- 6. sync editor and metadata -----------------------------------------

  lyricsSave(songId: string, lyrics: Lyrics) {
    const s = this.find(songId);
    if (!s) return this.reject<void>(fail("not_found", "That song is no longer in the library."));
    s.lyricsOverride = cloneLyrics(lyrics);
    this.emit("library://changed", null);
    return this.reply(undefined);
  }
  lyricsReset(songId: string) {
    const s = this.find(songId);
    if (!s) return this.reject<void>(fail("not_found", "That song is no longer in the library."));
    s.lyricsOverride = null;
    this.emit("library://changed", null);
    return this.reply(undefined);
  }
  metadataSave(songId: string, changes: MetadataChanges) {
    const s = this.find(songId);
    if (!s) return this.reject<void>(fail("not_found", "That song is no longer in the library."));
    s.metadata = { ...s.metadata, ...changes };
    this.emit("library://changed", null);
    return this.reply(undefined);
  }
  studioRealign(songId: string, text: string, anchors: RealignAnchor[]): Promise<Lyrics> {
    const s = this.find(songId);
    if (!s) return this.reject(fail("not_found", "That song is no longer in the library."));
    if (this.o.platform === "macos" || this.o.studio === "off") return this.reject(fail("no_gpu", "Re-aligning needs the Studio, which is not available on this computer."));
    // The mock cannot align: it keeps the current timings, applies the corrected text
    // line by line and moves anchored lines to their anchors.
    let l = cloneLyrics(s.lyricsOverride ?? s.demo.lyrics ?? { lyrics_version: 1, language: "en", source: "user_text", checked: true, lines: [] });
    text.split("\n").forEach((t, i) => {
      if (i < l.lines.length && t !== lyricsText({ ...l, lines: [l.lines[i]] })) l = editLineText(l, i, t);
    });
    void anchors;
    return new Promise((resolve) => setTimeout(() => resolve(l), 900));
  }

  // --- 7. export ------------------------------------------------------------

  exportBacking(songId: string, format: ExportFormat) {
    const s = this.find(songId);
    if (!s) return this.reject<{ job_id: string }>(fail("not_found", "That song is no longer in the library."));
    const job_id = `export-${this.nextId++}`;
    let p = 0;
    const h = this.o.setInterval(() => {
      p = Math.min(1, p + 0.08);
      this.emit("export://progress", { job_id, progress: p });
      if (p >= 1) {
        this.o.clearInterval(h);
        const sum = this.summaryOf(s);
        const key = keyLabel(sum.key, s.settings.semitones, "->", true);
        // Same rule as jk-engine's export: a 24-bit FLAC cannot hold a mix above full
        // scale, so it is scaled by one constant just enough to fit, and says so.
        const peak = this.o.exportPeak;
        const fullScale24 = 8_388_607 / 8_388_608;
        const applied_gain_db = format === "flac24" && peak > fullScale24 ? 20 * Math.log10(fullScale24 / peak) : 0;
        this.emit("export://done", {
          job_id,
          path: `Exports/${sum.artist} - ${sum.title} (${key}).${format === "flac24" ? "flac" : "wav"}`,
          applied_gain_db,
          peak,
        });
      }
    }, 120);
    return this.reply({ job_id });
  }

  // --- 8. recorder ----------------------------------------------------------

  recorderStatus() {
    return this.reply({ ...this.recorder });
  }
  recorderStart() {
    return this.startCapture("plain", null);
  }
  private startCapture(mode: RecorderMode, player: MediaPlayer | null): Promise<void> {
    if (this.o.recorder === "unavailable") return this.reject<void>(fail("recorder_unavailable", "No output device supports capture on this computer."));
    if (this.permission === "denied" || this.permission === "not_determined") {
      return this.reject<void>(fail("permission", "macOS has not allowed this app to record the screen's sound yet (\"Screen Recording\" permission)."));
    }
    if (this.recorder.state === "recording") return this.reject(fail("busy", "A recording is already running."));
    const mac = this.o.platform === "macos";
    this.recMode = mode;
    this.recStart = this.o.now();
    this.recorder = {
      state: "recording",
      elapsed_s: 0,
      format: {
        sample_rate: 48000,
        channels: 2,
        sample_format: "f32",
        note: mac
          ? "ScreenCaptureKit delivers 48 kHz. Set the Mac's output to 48 kHz in Audio MIDI Setup to avoid a conversion inside macOS."
          : "This is the format Windows mixes all sound in (the output device's shared-mode format). Saved as 32-bit float WAV, bit for bit.",
      },
      peak_db: [-60, -60],
      clipped: false,
      dropped_s: 0,
      error: null,
      mode,
      player: player ? { id: player.id, app_name: player.app_name } : null,
      session: null,
      note: null,
    };
    this.recTicker = this.o.setInterval(() => {
      const el = (this.o.now() - this.recStart) / 1000;
      const l = -14 + 8 * Math.sin(el * 2.1) + 3 * Math.sin(el * 7.3);
      const r = -15 + 8 * Math.sin(el * 1.9 + 1) + 3 * Math.sin(el * 6.1);
      this.recorder = { ...this.recorder, elapsed_s: el, peak_db: [Math.min(0, l), Math.min(0, r)], clipped: this.recorder.clipped || l > -0.1 };
      this.tickSession();
      this.emit("recorder://level", { ...this.recorder });
    }, 50);
    this.emit("recorder://level", { ...this.recorder });
    return this.reply(undefined);
  }
  recorderStop(pausePlayer = false): Promise<Recording> {
    if (this.recorder.state !== "recording") return this.reject(fail("not_recording", "The recorder is not running."));
    if (this.recMode === "session") return this.reject(fail("session_running", "Songs are being recorded separately; stop them with the Stop button of that panel."));
    this.o.clearInterval(this.recTicker);
    const player = this.recorder.player ? this.mediaPlayers.find((p) => p.id === this.recorder.player?.id) : undefined;
    if (pausePlayer && player) player.status = "paused";
    this.recMode = "plain";
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const name = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}.wav`;
    const dur = (this.o.now() - this.recStart) / 1000;
    const rec: Recording = { path: `Recordings/${name}`, name, duration_s: dur, sample_rate: 48000, channels: 2, bytes: Math.round(dur * 48000 * 8), clipped: this.recorder.clipped };
    this.recordings.unshift(rec);
    this.recorder = { ...this.recorder, state: "idle", mode: "plain", player: null, session: null, note: null };
    this.emit("recorder://level", { ...this.recorder });
    return this.reply(rec);
  }

  // Media-player link and "Record songs separately" (additions, 2026-11-18).
  recorderMediaSessions(): Promise<MediaPlayers> {
    if (this.o.platform === "macos") {
      return this.reply({ available: false, reason: "Following and controlling the music player works on Windows only for now. Start the song in your player yourself, then record.", players: [] });
    }
    return this.reply({ available: true, reason: null, players: this.mediaPlayers.map((p) => ({ ...p })) });
  }
  private findPlayer(id: string): MediaPlayer | null {
    return this.mediaPlayers.find((p) => p.id === id) ?? null;
  }
  private notFound<T>(): Promise<T> {
    return this.reject(
      fail("player_not_found", "That music player is not open any more (or has nothing loaded). Open it and pick a song, or use the plain Record button and press play yourself."),
    );
  }
  async recorderRecordAndPlay(playerId: string): Promise<RecorderStatus> {
    const p = this.findPlayer(playerId);
    if (!p) return this.notFound();
    const started = this.startCapture("record_and_play", p);
    if (this.recMode === "record_and_play") p.status = "playing";
    await started;
    return { ...this.recorder };
  }
  async recorderSessionStart(options: SessionStartOptions): Promise<RecorderStatus> {
    const p = this.findPlayer(options.player_id);
    if (!p) return this.notFound();
    await this.startCapture("session", p);
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const folder = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}`;
    this.live = { player: p, folder, track: 0, played: [0], opts: options, lastSwitch: this.o.now(), idleSince: null };
    if (options.start_player !== false) p.status = "playing";
    this.recorder = { ...this.recorder, session: this.liveView() };
    const s = this.sessionFromLive("recording");
    this.sessions.unshift(s);
    this.emit("recorder://session", { ...s });
    this.emit("recorder://level", { ...this.recorder });
    return { ...this.recorder };
  }
  private liveView() {
    const l = this.live;
    if (!l) return null;
    const t = MOCK_PLAYLIST[l.track % MOCK_PLAYLIST.length];
    const idle = l.opts.auto_stop_idle_s ?? 30;
    return {
      folder: l.folder,
      path: `E:\\Recordings\\${l.folder}`,
      current: { title: t.title, artist: t.artist, album: t.album },
      playback: l.player.status,
      songs: l.played.filter((i) => !MOCK_PLAYLIST[i % MOCK_PLAYLIST.length].ad).length,
      auto_stop_idle_s: idle,
      max_songs: l.opts.max_songs ?? null,
      auto_stop_in_s: l.idleSince === null || idle <= 0 ? null : Math.max(0, idle - (this.o.now() - l.idleSince) / 1000),
    };
  }
  /** The simulated player moves to the next track every 12 s. */
  private tickSession(): void {
    const l = this.live;
    if (!l || this.recMode !== "session") return;
    if (l.player.status === "playing" && this.o.now() - l.lastSwitch > 12000) {
      l.track += 1;
      l.played.push(l.track);
      l.lastSwitch = this.o.now();
    }
    this.recorder = { ...this.recorder, session: this.liveView() };
  }
  private sessionFromLive(state: RecordingSession["state"]): RecordingSession {
    const l = this.live!;
    const songs: RecordedSong[] = [];
    let n = 0;
    l.played.forEach((i, k) => {
      const t = MOCK_PLAYLIST[i % MOCK_PLAYLIST.length];
      if (t.ad) return;
      n += 1;
      const last = k === l.played.length - 1;
      songs.push(mockSong(l.folder, n, t, last ? "partial" : "complete", last ? "The recording stopped during the song." : null));
    });
    return {
      folder: l.folder,
      path: `E:\\Recordings\\${l.folder}`,
      created: l.folder,
      player: l.player.app_name,
      player_id: l.player.source_app_id,
      state,
      progress: state === "splitting" ? 0 : null,
      stop_reason: state === "recording" ? null : "user",
      message: null,
      master_present: state !== "split",
      song_count: songs.length,
      songs: state === "split" ? songs : [],
      excluded: [],
    };
  }
  private putSession(s: RecordingSession): void {
    this.sessions = [s, ...this.sessions.filter((x) => x.folder !== s.folder)];
    this.emit("recorder://session", { ...s });
  }
  recorderSessionStop(pausePlayer: boolean): Promise<RecordingSession> {
    if (this.recorder.state !== "recording" || this.recMode !== "session" || !this.live) {
      return this.reject(fail("not_recording", "No songs are being recorded separately."));
    }
    this.o.clearInterval(this.recTicker);
    if (pausePlayer) this.live.player.status = "paused";
    const splitting = this.sessionFromLive("splitting");
    const done = this.sessionFromLive("split");
    this.live = null;
    this.recMode = "plain";
    this.recorder = { ...this.recorder, state: "idle", mode: "plain", player: null, session: null, note: null };
    this.emit("recorder://level", { ...this.recorder });
    this.putSession(splitting);
    setTimeout(() => this.putSession(done), Math.max(20, this.o.latencyMs * 10));
    return this.reply({ ...splitting });
  }
  recorderSessionsList(): Promise<{ sessions: RecordingSession[] }> {
    return this.reply({ sessions: this.sessions.map((s) => ({ ...s })) });
  }
  recorderSessionSplit(folder: string): Promise<RecordingSession> {
    const s = this.sessions.find((x) => x.folder === folder);
    if (!s) return this.reject(fail("not_found", "That session could not be found."));
    if (!s.master_present) return this.reject(fail("nothing_to_split", "This session's full recording is no longer there, so there is nothing to split again."));
    const splitting = { ...s, state: "splitting" as const, progress: 0, message: null };
    this.putSession(splitting);
    setTimeout(() => this.putSession({ ...s, state: "split", progress: null, message: null, master_present: false }), 30);
    return this.reply(splitting);
  }
  recorderSessionDelete(folder: string): Promise<void> {
    const i = this.sessions.findIndex((x) => x.folder === folder);
    if (i < 0) return this.reject(fail("not_found", "That session could not be found."));
    if (this.sessions[i].state === "recording" || this.sessions[i].state === "splitting") return this.reject(fail("busy", "That session is still being recorded or split."));
    this.sessions.splice(i, 1);
    return this.reply(undefined);
  }
  recorderPreview(path: string): Promise<void> {
    void this.recorderPreviewStop();
    this.emit("recorder://preview", { path, playing: true });
    this.previewTimer = setTimeout(() => {
      this.previewTimer = null;
      this.emit("recorder://preview", { path, playing: false });
    }, 3000);
    this.previewPath = path;
    return this.reply(undefined);
  }
  private previewPath: string | null = null;
  recorderPreviewStop(): Promise<void> {
    if (this.previewTimer) {
      clearTimeout(this.previewTimer);
      this.previewTimer = null;
      if (this.previewPath) this.emit("recorder://preview", { path: this.previewPath, playing: false });
    }
    return this.reply(undefined);
  }
  recorderRequestPermission() {
    // The real app shows the macOS prompt; the mock pretends the user allowed it.
    if (this.permission === "not_determined") this.permission = "granted";
    return this.reply(undefined);
  }
  recordingsList() {
    return this.reply({ recordings: this.recordings.map((r) => ({ ...r })) });
  }

  // --- 9. studio ------------------------------------------------------------

  private studioAvailable(): boolean {
    return this.o.platform === "windows" && this.o.studio === "on";
  }

  studioStatus(): Promise<StudioStatus> {
    const available = this.studioAvailable();
    return this.reply({
      available,
      reasons: available ? [] : ["The Studio is not available on this computer. Songs are created on a stronger computer and played anywhere."],
      running: this.jobs.some((j) => j.state === "running"),
      models_present: this.presetPresent("full"),
      queue: this.jobs.map((j) => ({ ...j })),
    });
  }

  studioInspect(path: string): Promise<InspectResult> {
    if (!this.studioAvailable()) return this.reject(fail("no_gpu", "The Studio is not available on this computer."));
    const name = path.split(/[\\/]/).pop() ?? path;
    const ext = (name.split(".").pop() ?? "").toLowerCase();
    if (!["mp3", "m4a", "flac", "wav", "ogg", "opus", "aiff", "wma", "mp4", "mkv", "mov", "webm", "avi"].includes(ext)) {
      return this.reject(fail("decode_failed", `"${name}" could not be read as audio or video.`));
    }
    const stem = name.replace(/\.[^.]+$/, "");
    const [artist, title] = stem.includes(" - ") ? stem.split(" - ", 2) : ["", stem];
    const video = ["mp4", "mkv", "mov", "webm", "avi"].includes(ext);
    const lossy = !["flac", "wav", "aiff"].includes(ext);
    return this.reply({
      ok: true,
      title,
      artist: artist || null,
      album: null,
      duration_seconds: 150 + (name.length % 90),
      container: ext,
      codec: ext === "m4a" ? "aac" : video ? "aac" : ext,
      sample_rate: ext === "flac" ? 96000 : 44100,
      channels: 2,
      bits_per_sample: lossy ? null : 24,
      lossy,
      has_video: video,
      embedded_lyrics: ext === "flac" ? { present: true, synced: false, text: "Twinkle, twinkle, little star,\nHow I wonder what you are!" } : { present: false, synced: false, text: null },
      // The demo FLAC carries a cover (served by studioCoverPreview), the others none.
      cover_path: ext === "flac" ? "E:\\Settings\\tmp\\studio\\covers\\cover-demo.png" : null,
    });
  }

  studioLyricsLookup(query: LyricsLookupQuery): Promise<{ results: LookupResult[] }> {
    if (!this.studioAvailable()) return this.reject(fail("no_gpu", "The Studio is not available on this computer."));
    const results: LookupResult[] = /twinkle/i.test(query.title)
      ? [
          {
            service: "lrclib",
            synced_lrc: "[00:07.80] Twinkle, twinkle, little star,\n[00:12.60] How I wonder what you are!",
            plain: "Twinkle, twinkle, little star,\nHow I wonder what you are!",
            duration_seconds: query.duration_s,
          },
        ]
      : [];
    return new Promise((resolve) => setTimeout(() => resolve({ results }), 400));
  }

  studioSubmit(request: StudioSubmitRequest): Promise<{ job_id: string }> {
    if (!this.studioAvailable()) return this.reject(fail("no_gpu", "The Studio is not available on this computer."));
    if (!this.presetPresent(request.preset ?? "full")) return this.reject(fail("models_missing", "The models for this quality preset are not downloaded yet. Download them first."));
    if ((request.lyrics.mode === "text" && !request.lyrics.text?.trim()) || (request.lyrics.mode === "lrc" && !request.lyrics.lrc?.trim())) {
      return this.reject(fail("lyrics_missing", "No lyrics were given. Paste them, choose an .lrc file, or pick Automatic."));
    }
    const job: StudioJob = {
      job_id: `job-${this.nextId++}`,
      input_name: request.input.split(/[\\/]/).pop() ?? request.input,
      state: "queued",
      stage: null,
      overall: 0,
      message: null,
      output_song_id: null,
      error: null,
      lyrics: request.lyrics.mode,
    };
    this.jobs.push(job);
    this.emit("studio://job", { ...job });
    this.runJobs();
    return this.reply({ job_id: job.job_id });
  }

  /** Advances the queue: one job at a time, a failing file never stops the queue. */
  private runJobs(): void {
    if (this.jobTicker !== null) return;
    this.jobTicker = this.o.setInterval(() => {
      const job = this.jobs.find((j) => j.state === "running") ?? this.jobs.find((j) => j.state === "queued");
      if (!job) {
        this.o.clearInterval(this.jobTicker);
        this.jobTicker = null;
        return;
      }
      const weights = [0.03, 0.55, 0.15, 0.15, 0.05, 0.07];
      if (job.state === "queued") {
        job.state = "running";
        job.stage = "decode";
        job.overall = 0;
      } else {
        job.overall = Math.min(1, job.overall + 0.035);
        let acc = 0;
        let idx = 0;
        for (; idx < weights.length; idx++) {
          acc += weights[idx];
          if (job.overall < acc) break;
        }
        const stage = STUDIO_STAGES[Math.min(idx, STUDIO_STAGES.length - 1)];
        job.stage = stage;
        job.message = STAGE_MESSAGES[stage];
        if (/broken|corrupt/i.test(job.input_name) && job.stage === "separate") {
          job.state = "failed";
          job.error = "This file could not be read as audio past 0:41. It may be damaged.";
          job.message = null;
        } else if (job.overall >= 1) {
          job.state = "done";
          job.stage = null;
          job.message = null;
          job.output_song_id = `${job.input_name.replace(/\.[^.]+$/, "")}.jksong`;
        }
      }
      this.emit("studio://job", { ...job });
    }, 150);
  }

  studioSetLyrics(jobId: string, lyrics: { mode: "auto" | "text"; text: string | null }): Promise<StudioJob> {
    const job = this.jobs.find((j) => j.job_id === jobId);
    if (!job) return this.reject(fail("not_found", "This job is not in the queue."));
    if (job.state !== "queued") return this.reject(fail("busy", "The Studio has already started this song, so its lyrics can no longer be changed here."));
    if (lyrics.mode === "text" && !lyrics.text?.trim()) return this.reject(fail("lyrics_missing", "No lyrics were given. Paste them, or pick Automatic."));
    job.lyrics = lyrics.mode;
    this.emit("studio://job", { ...job });
    return this.reply({ ...job });
  }
  studioCancel(jobId: string) {
    const job = this.jobs.find((j) => j.job_id === jobId);
    if (job && (job.state === "queued" || job.state === "running")) {
      job.state = "cancelled";
      job.message = null;
      job.error = "Cancelled.";
      this.emit("studio://job", { ...job });
    }
    return this.reply(undefined);
  }

  /** Every separation model of `preset` is on the simulated drive. */
  private presetPresent(preset: StudioPreset): boolean {
    return PRESET_MODELS[preset].every((m) => this.modelsOnDisk.has(m));
  }

  /** Like the real Studio: the preset's separation models plus the two lyrics models. */
  private modelsFor(preset: StudioPreset): StudioModelInfo[] {
    return [...PRESET_MODELS[preset], ...LYRICS_MODELS].map((name) => ({ ...MOCK_MODELS[name], present: this.modelsOnDisk.has(name) }));
  }

  studioModelsStatus(preset: StudioPreset): Promise<StudioModelsStatus> {
    const models = this.modelsFor(preset);
    return this.reply({
      preset,
      models,
      all_present: models.every((m) => m.present),
      download_bytes: models.filter((m) => !m.present).reduce((a, m) => a + m.size_bytes, 0),
    });
  }

  /** Resolves when the download has finished, like the real core. */
  studioModelsDownload(preset: StudioPreset): Promise<void> {
    const missing = this.modelsFor(preset).filter((m) => !m.present);
    return new Promise((resolve) => {
      if (missing.length === 0) return resolve();
      let k = 0;
      let done = 0;
      const h = this.o.setInterval(() => {
        const m = missing[k];
        done = Math.min(m.size_bytes, done + Math.max(1, Math.ceil(m.size_bytes / 4)));
        this.emit("studio://models", { name: m.file, done_bytes: done, total_bytes: m.size_bytes });
        if (done >= m.size_bytes) {
          this.modelsOnDisk.add(m.name);
          k += 1;
          done = 0;
          if (k >= missing.length) {
            this.o.clearInterval(h);
            resolve();
          }
        }
      }, 25);
    });
  }

  pickCoverImage() {
    return this.reply<string | null>("C:\\Users\\you\\Pictures\\My cover.jpg");
  }

  studioCoverPreview(path: string, maxPx: number) {
    if (!/\.(jpe?g|png)$/i.test(path)) return this.reject<{ data_url: string | null }>(fail("bad_request", "The cover picture must be a JPEG or PNG image."));
    const name = (path.split(/[\\/]/).pop() ?? path).replace(/\.[^.]+$/, "");
    const colors: [string, string] = /demo/.test(name) ? ["#f6c453", "#c2410c"] : ["#60a5fa", "#7c3aed"];
    return this.reply({ data_url: coverDataUrl(name, colors, maxPx) });
  }

  pickFiles() {
    // No native dialog in a browser; the Studio panel also offers an <input type=file>.
    const promo = promoSongData();
    return this.reply([promo ? `C:\\Music\\${promo.file}` : "C:\\Music\\Jane Taylor - Twinkle Twinkle (piano demo).flac"]);
  }

  acceptBrowserFiles(names: string[]): string[] {
    return names.map((n) => `C:\\Users\\you\\Music\\${n}`);
  }

  // --- 12. practice mode (WO2; mock-practice.ts) ------------------------------------

  private practiceCall<T>(f: () => T): Promise<T> {
    try {
      return this.reply(f());
    } catch (e) {
      return this.reject(e as ApiError);
    }
  }
  micPermission(): Promise<MicPermission> {
    return this.reply(this.practice.micPermission());
  }
  micRequestPermission(): Promise<MicPermission> {
    return this.reply(this.practice.micRequestPermission());
  }
  micOpenSettings(): Promise<void> {
    return this.reply(undefined);
  }
  micList(): Promise<MicList> {
    return this.reply(this.practice.micList());
  }
  micChoose(choice: InputChoice): Promise<InputChoice> {
    const c = this.practice.micChoose(choice);
    const pr = this.settings.practice;
    if (pr) pr.inputs = { ...pr.inputs, "mock-computer": { ...c } };
    return this.reply(c);
  }
  micMeter(deviceId: string | null): Promise<{ device: MicInput | null; error: InputError | null }> {
    return this.reply(this.practice.micMeter(deviceId));
  }
  practiceStart(options: PracticeStartOptions = {}): Promise<PracticeState> {
    return this.practiceCall(() => this.practice.start(options));
  }
  practiceStop(keepResult = true): Promise<{ result: PracticeResult | null }> {
    return this.practiceCall(() => this.practice.stop(keepResult));
  }
  practiceState(): Promise<PracticeState> {
    return this.reply(this.practice.state());
  }
  practiceGuide(songId?: string): Promise<PracticeGuide> {
    return this.practiceCall(() => this.practice.guide(songId));
  }
  practiceSuspendInput(suspended: boolean): Promise<void> {
    return this.practiceCall(() => this.practice.suspend(suspended));
  }
  practiceFinish(): Promise<{ result: PracticeResult | null }> {
    return this.practiceCall(() => this.practice.finish());
  }
  practiceAgain(): Promise<PracticeState> {
    return this.practiceCall(() => this.practice.again());
  }
  practiceSongSet(songId: string, changes: { octave?: { mode: OctaveMode; shift?: number }; strictness?: Strictness | null }): Promise<PracticeState> {
    return this.practiceCall(() => this.practice.songSet(songId, changes));
  }
  practiceExclude(songId: string, span: ExcludedSpan, excluded: boolean): Promise<{ excluded: ExcludedSpan[] }> {
    return this.practiceCall(() => this.practice.exclude(songId, span, excluded));
  }
  practiceHistory(songId: string): Promise<PracticeHistory> {
    return this.reply(this.practice.history(songId));
  }
  practiceLatencySet(manualMs: number): Promise<{ manual_ms: number }> {
    return this.reply(this.practice.latencySet(manualMs));
  }
  practiceTargetAt(positionS: number): Promise<PracticeTarget> {
    return this.reply(this.practice.targetAt(positionS));
  }
  playerKeyAt(positionS: number): Promise<{ semitones: number | null }> {
    return this.reply({ semitones: this.current ? this.keyAtSong(positionS) : null });
  }

  // --- 10. self-check -------------------------------------------------------

  selfcheckRun(userAgent: string, silent = false, lane: LaneCost | null = null): Promise<SelfCheckResult> {
    const mac = this.o.platform === "macos";
    const items = [
      { name: "Test tone", ok: true, detail: silent ? "Played 1 s of digital silence on Speakers (48000 Hz, 2 ch, f32); 0 underruns." : "Played 1 s of 440 Hz at -60 dBFS on Speakers (48000 Hz, 2 ch, f32); 0 underruns." },
      { name: "Bundled test song", ok: true, detail: "Loaded tiny.jksong and rendered 4.0 s offline; output matches the reference (max diff 0)." },
      { name: "Key change", ok: true, detail: "Rendered +2 semitones; measured pitch error 0.001 cents; length unchanged." },
      {
        name: "Recorder",
        ok: this.permission === "granted" || this.permission === "not_required",
        detail:
          this.permission === "granted" || this.permission === "not_required"
            ? "Captured 3.00 s at 48000 Hz, 2 ch, f32 (144000 frames, 0 dropped)."
            : 'macOS has not allowed "Screen Recording" yet. Open System Settings > Privacy & Security > Screen Recording, turn the app on, then run the self-check again.',
      },
      { name: "System", ok: true, detail: `${mac ? "macOS 13.6.9, 8 GB memory" : "Windows 11 26200, 32 GB memory"}; web engine: ${userAgent}` },
      // WO2 (APP_API 10.1): the lane's drawing cost as this web view measured it.
      {
        name: "Practice lane",
        ok: !lane || lane.avg_ms <= 4,
        detail: lane
          ? `${lane.frames} frames at ${lane.width}x${lane.height} (x${lane.dpr}): ${lane.avg_ms.toFixed(2)} ms average, ${lane.p95_ms.toFixed(2)} ms 95th percentile; simple lane ${lane.simple_avg_ms.toFixed(2)} ms (budget 4 ms with the lyrics).`
          : "not measured",
      },
    ];
    return new Promise((resolve) =>
      setTimeout(
        () => resolve({ report_path: mac ? "/Volumes/OPEN MIC/Settings/selfcheck-2026-11-17.txt" : "E:\\Settings\\selfcheck-2026-11-17.txt", passed: items.every((i) => i.ok), items }),
        700,
      ),
    );
  }

  // --- helpers for tests ------------------------------------------------------

  /** Current simulated playback position (tests). */
  debugPosition(): number {
    return this.position();
  }

  /** Default song settings, for tests that reset a song. */
  static readonly defaultSongSettings = DEFAULT_SONG_SETTINGS;
}

// SPDX-License-Identifier: GPL-3.0-or-later
// Browser mock of the whole app API (docs/APP_API.md), used automatically when the UI
// does not run inside Tauri (`npm run dev` in a normal browser, the UI tests and the
// headless screenshots). It plays no audio: playback is a simulated clock that emits
// `playback://clock` about 30 times per second, like the real engine.
//
// URL switches (for trying states by hand and for screenshots):
//   ?platform=macos        Mac app: no Studio, recorder asks for "Screen Recording"
//   ?studio=off            Studio unavailable on Windows (reasons shown)
//   ?models=missing        Studio available but its models are not downloaded
//   ?recorder=denied|not_determined|unavailable
//   ?empty=1               empty library
//   ?many=200              add generated filler songs (library performance)
//   ?skin=0                start with the plain look (settings.ui.skin off)
// Handled by the UI (App.tsx `mockStartup`), not here:
//   ?song=N&at=SECONDS     load playable song N (0-based) at a position, paused
//   &play=1                ...and start playing
//   &screen=NAME           open that screen (library, stage, sync, recorder, studio, settings)
//   &overlay=help|selfcheck, &fullscreen=1
import type { Api, Unsubscribe } from "./client";
import { coverDataUrl, DEFAULT_SONG_SETTINGS, demoLibrary, type DemoSong, fillerSongs } from "./demo";
import type {
  ApiError,
  ApiEventName,
  ApiEvents,
  AppInfo,
  AppSettings,
  AppSettingsChanges,
  Device,
  ExportFormat,
  InspectResult,
  LookupResult,
  Lyrics,
  LyricsLookupQuery,
  MetadataChanges,
  PlayerState,
  ProposedExtensions,
  RealignAnchor,
  Recording,
  RecorderPermission,
  RecorderStatus,
  SelfCheckResult,
  SongDetail,
  SongSettings,
  SongSummary,
  StudioJob,
  StudioModelsStatus,
  StudioStatus,
  StudioSubmitRequest,
} from "./types";
import { STUDIO_STAGES } from "./types";
import { DEFAULT_APP_SETTINGS } from "./defaults";
import { cloneLyrics, editLineText, lyricsText } from "../lib/lyrics";
import { keyLabel } from "../lib/keys";

export interface MockOptions {
  platform: "windows" | "macos";
  studio: "on" | "off";
  models: "present" | "missing";
  recorder: RecorderPermission | "unavailable";
  empty: boolean;
  many: number;
  /** settings.ui.skin at start (the jukebox look); the tests run both looks. */
  skin: boolean;
  /** Simulated IPC latency for commands, ms. */
  latencyMs: number;
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
  empty: false,
  many: 0,
  skin: true,
  latencyMs: 3,
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
  private permission: RecorderPermission;
  private jobs: StudioJob[] = [];
  private jobTicker: unknown = null;
  private modelsPresent: boolean;
  private nextId = 1;

  constructor(options: Partial<MockOptions> = {}) {
    this.o = { ...DEFAULT_OPTIONS, ...options };
    this.permission =
      this.o.recorder === "unavailable" ? "not_required" : this.o.platform === "macos" && this.o.recorder === "not_required" ? "not_determined" : this.o.recorder;
    this.modelsPresent = this.o.models === "present";
    this.settings.ui.skin = this.o.skin;
    const lib = this.o.empty ? [] : [...demoLibrary(), ...fillerSongs(this.o.many)];
    this.songs = lib.map((demo) => ({ demo, settings: { ...demo.settings, levels: { ...demo.settings.levels } }, lyricsOverride: null, metadata: {} }));
    this.recordings = [
      { path: "Recordings/2026-11-16 21-04-12.wav", name: "2026-11-16 21-04-12.wav", duration_s: 187.4, sample_rate: 48000, channels: 2, bytes: 71_965_000, clipped: false },
      { path: "Recordings/2026-11-15 19-30-55.wav", name: "2026-11-15 19-30-55.wav", duration_s: 64.0, sample_rate: 48000, channels: 2, bytes: 24_576_000, clipped: true },
    ];
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
    if (q.get("skin") === "0") o.skin = false;
    const many = Number(q.get("many"));
    if (many > 0) o.many = Math.min(2000, Math.floor(many));
    return o;
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
    const root = mac ? "/Volumes/JUKEOKE" : "E:\\";
    const p = (name: string) => (mac ? `${root}/${name}` : `${root}${name}`);
    return this.reply<AppInfo>({
      name: "Jukeoke",
      version: "0.1.0",
      platform: this.o.platform,
      os_version: mac ? "macOS 13.6.9" : "Windows 11 26200",
      portable: true,
      root,
      translocated: false,
      paths: { library: p("Library"), recordings: p("Recordings"), exports: p("Exports"), settings: p("Settings"), models: mac ? null : p("Models") },
      capabilities: {
        studio: { available: studioReasons.length === 0, reasons: studioReasons, gpu: studioReasons.length === 0 ? "NVIDIA GeForce RTX 5070 Ti" : null, vram_mb: studioReasons.length === 0 ? 16303 : null },
        recorder: {
          available: this.o.recorder !== "unavailable",
          reason: this.o.recorder === "unavailable" ? "No output device supports capture on this computer." : null,
          permission: this.permission,
        },
        exclusive_mode: !mac,
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
    };
  }

  libraryList() {
    return this.reply({ songs: this.songs.map((s) => this.summaryOf(s)), free_bytes: 18_734_000_000, total_bytes: 30_920_000_000 });
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
    return Math.min(this.current.demo.summary.duration_s, this.anchorPos + (this.o.now() - this.anchorT) / 1000);
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
    };
  }

  private emitClock(): void {
    this.seq += 1;
    this.emit("playback://clock", { position_s: this.position(), playing: this.playing, seq: this.seq });
  }
  private emitState(): void {
    this.emit("playback://state", this.fullState());
    this.emitClock();
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
    const lyrics = s.lyricsOverride ?? s.demo.lyrics;
    const detail: SongDetail = {
      summary: this.summaryOf(s),
      lyrics: lyrics ? cloneLyrics(lyrics) : null,
      lyrics_is_override: s.lyricsOverride !== null,
      settings: { ...s.settings, levels: { ...s.settings.levels } },
      stems: s.demo.stems,
      sample_rate: s.demo.sample_rate,
      channels: s.demo.channels,
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
    this.emitState();
    return this.reply(undefined);
  }
  playerSet(changes: Partial<SongSettings>) {
    if (!this.current) return this.reject<void>(fail("no_song", "Choose a song first."));
    if (changes.semitones !== undefined && (!Number.isInteger(changes.semitones) || Math.abs(changes.semitones) > 12)) {
      return this.reject<void>(fail("bad_request", "The key can move at most 12 semitones up or down."));
    }
    this.current.settings = deepMerge(this.current.settings, changes);
    this.emit("playback://state", this.fullState());
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
        this.emit("export://done", { job_id, path: `Exports/${sum.artist} - ${sum.title} (${key}).${format === "flac24" ? "flac" : "wav"}` });
      }
    }, 120);
    return this.reply({ job_id });
  }

  // --- 8. recorder ----------------------------------------------------------

  recorderStatus() {
    return this.reply({ ...this.recorder });
  }
  recorderStart() {
    if (this.o.recorder === "unavailable") return this.reject<void>(fail("recorder_unavailable", "No output device supports capture on this computer."));
    if (this.permission === "denied" || this.permission === "not_determined") {
      return this.reject<void>(fail("permission", "macOS has not allowed this app to record the screen's sound yet (\"Screen Recording\" permission)."));
    }
    if (this.recorder.state === "recording") return this.reply(undefined);
    const mac = this.o.platform === "macos";
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
    };
    this.recTicker = this.o.setInterval(() => {
      const el = (this.o.now() - this.recStart) / 1000;
      const l = -14 + 8 * Math.sin(el * 2.1) + 3 * Math.sin(el * 7.3);
      const r = -15 + 8 * Math.sin(el * 1.9 + 1) + 3 * Math.sin(el * 6.1);
      this.recorder = { ...this.recorder, elapsed_s: el, peak_db: [Math.min(0, l), Math.min(0, r)], clipped: this.recorder.clipped || l > -0.1 };
      this.emit("recorder://level", { ...this.recorder });
    }, 50);
    this.emit("recorder://level", { ...this.recorder });
    return this.reply(undefined);
  }
  recorderStop(): Promise<Recording> {
    if (this.recorder.state !== "recording") return this.reject(fail("not_recording", "The recorder is not running."));
    this.o.clearInterval(this.recTicker);
    const d = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const name = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}.wav`;
    const dur = (this.o.now() - this.recStart) / 1000;
    const rec: Recording = { path: `Recordings/${name}`, name, duration_s: dur, sample_rate: 48000, channels: 2, bytes: Math.round(dur * 48000 * 8), clipped: this.recorder.clipped };
    this.recordings.unshift(rec);
    this.recorder = { ...this.recorder, state: "idle" };
    this.emit("recorder://level", { ...this.recorder });
    return this.reply(rec);
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
      models_present: this.modelsPresent,
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
      cover_path: null,
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
    if (!this.modelsPresent) return this.reject(fail("models_missing", "The separation models are not downloaded yet. Download them first (about 1.6 GB)."));
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

  studioModelsStatus(): Promise<StudioModelsStatus> {
    return this.reply({
      models: [
        { key: "vocals", name: "Vocal separation (RoFormer)", file: "vocals.ckpt", size_bytes: 639_000_000, present: this.modelsPresent, licence: "MIT", commercial: "yes" },
        { key: "karaoke", name: "Lead / backing vocal split", file: "karaoke.ckpt", size_bytes: 428_000_000, present: this.modelsPresent, licence: "MIT", commercial: "yes" },
        { key: "drums", name: "Drum separation", file: "drums.ckpt", size_bytes: 310_000_000, present: this.modelsPresent, licence: "MIT", commercial: "yes" },
        { key: "align", name: "Letter alignment (wav2vec2)", file: "align.bin", size_bytes: 1_260_000_000, present: this.modelsPresent, licence: "Apache-2.0", commercial: "yes" },
      ],
      all_present: this.modelsPresent,
    });
  }

  studioModelsDownload() {
    let done = 0;
    const total = 2_637_000_000;
    const h = this.o.setInterval(() => {
      done = Math.min(total, done + 160_000_000);
      this.emit("studio://models", { name: "Vocal separation (RoFormer)", done_bytes: done, total_bytes: total });
      if (done >= total) {
        this.o.clearInterval(h);
        this.modelsPresent = true;
      }
    }, 100);
    return this.reply(undefined);
  }

  pickFiles() {
    // No native dialog in a browser; the Studio panel also offers an <input type=file>.
    return this.reply(["C:\\Music\\Jane Taylor - Twinkle Twinkle (piano demo).flac"]);
  }

  acceptBrowserFiles(names: string[]): string[] {
    return names.map((n) => `C:\\Users\\you\\Music\\${n}`);
  }

  // --- 10. self-check -------------------------------------------------------

  selfcheckRun(userAgent: string, silent = false): Promise<SelfCheckResult> {
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
    ];
    return new Promise((resolve) =>
      setTimeout(
        () => resolve({ report_path: mac ? "/Volumes/JUKEOKE/Settings/selfcheck-2026-11-17.txt" : "E:\\Settings\\selfcheck-2026-11-17.txt", passed: items.every((i) => i.ok), items }),
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

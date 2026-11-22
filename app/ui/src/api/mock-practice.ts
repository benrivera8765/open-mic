// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode in the browser mock (docs/APP_API.md section 12), so `npm run dev`, the UI
// tests and the headless screenshots exercise the practice screen with no Rust core:
//
//   - guides generated from the demo songs' real (public-domain) melodies, on the same beat
//     grid as their generated lyrics: Twinkle (A major, saved +2) and Row (G major, recorded
//     14 cents flat, with a spoken last line and two low-confidence notes); any other song
//     gets a scale-walk melody when "Add practice data" is pressed;
//   - a microphone side: three inputs (a USB mixer with two channels, the built-in
//     microphone, a Bluetooth headset that is never chosen automatically), per-channel
//     meters, the permission (macOS asks after the app's explanation; Windows' privacy
//     switches), "watch only";
//   - a simulated singer through the "core": tracker values every 10 ms about 45 ms late,
//     mostly on pitch with vibrato and a scoop, some notes sharp, some flat, some missed,
//     sung in the key HEARD (so a key change is followed only once it is heard), scored by
//     the standard band into hits, finished notes and a result with history;
//   - music in the input on request (?music=present|marginal|unusable).
//
// URL switches (MockApi.optionsFromUrl): ?mic=granted|denied|not_determined|restricted|none,
// ?micprivacy=off (Windows: the microphone privacy switch is off), ?music=present|marginal|
// unusable, ?singeroctave=-1 (the singer sings an octave down: auto moves the lane there).
import type { DemoSong } from "./demo";
import { rowScore, twinkleScore, type ScoreLine, prng } from "./demo";
import type {
  ChannelMeter,
  GuideStatus,
  InputChoice,
  Lyrics,
  MicInput,
  MicList,
  MicPermission,
  MusicInInput,
  PracticeGuide,
  PracticeHistory,
  PracticeLive,
  PracticeResult,
  PracticeStartOptions,
  PracticeState,
  PracticeTarget,
  SongKey,
  Strictness,
  OctaveMode,
  ExcludedSpan,
  AppSettings,
} from "./types";
import { practiceGuideOf, type StoredGuide, type StoredGuideNote } from "../practice/guide";
import { foldCents, noteName, spellingFor } from "../practice/pitch";
import { keyLabel } from "../lib/keys";

// ---------------------------------------------------------------------------
// Guides from the melodies
// ---------------------------------------------------------------------------

/** One melody line: [scale offset in semitones from the tonic, beats] per note. */
type MelodyLine = [number, number][];

const TW1: MelodyLine = [[0, 1], [0, 1], [7, 1], [7, 1], [9, 1], [9, 1], [7, 2]];
const TW2: MelodyLine = [[5, 1], [5, 1], [4, 1], [4, 1], [2, 1], [2, 1], [0, 2]];
const TW3: MelodyLine = [[7, 1], [7, 1], [5, 1], [5, 1], [4, 1], [4, 1], [2, 2]];
/** Every verse of Twinkle sings the same six-line tune. */
const TWINKLE_TUNE = [TW1, TW2, TW3, TW3, TW1, TW2];

const ROW_TUNE: MelodyLine[] = [
  [[0, 1.5], [0, 1.5], [0, 1], [2, 0.5], [4, 1.5]],
  [[4, 0.5], [2, 0.5], [4, 0.5], [5, 1], [7, 3.5]],
  [[12, 1 / 3], [12, 1 / 3], [12, 1 / 3], [7, 1 / 3], [7, 1 / 3], [7, 1 / 3], [4, 1 / 3], [4, 1 / 3], [4, 1 / 3], [0, 1 / 3], [0, 1 / 3], [0, 1 / 3]],
  [[7, 1], [5, 0.5], [4, 1], [2, 0.5], [0, 3]],
];

interface MelodySpec {
  score: ScoreLine[];
  tune: MelodyLine[];
  bpm: number;
  offset: number;
  /** MIDI of the tonic the singer sings. */
  tonic: number;
  /** The recording's tuning, cents (the notes are measured with it). */
  tuning: number;
  /** Lines (score index) sung as speech: "speak" bars, never graded. */
  spoken?: number[];
  /** Notes (index) the guide is unsure of. */
  unsure?: number[];
}

const SPECS: Record<string, () => MelodySpec> = {
  "Jane Taylor - Twinkle, Twinkle, Little Star.jksong": () => ({ score: twinkleScore(), tune: TWINKLE_TUNE, bpm: 100, offset: 0, tonic: 57, tuning: 0, unsure: [52] }),
  "Traditional - Row, Row, Row Your Boat.jksong": () => ({ score: rowScore(), tune: ROW_TUNE, bpm: 104, offset: 0.1, tonic: 55, tuning: -14, spoken: [11], unsure: [17, 60] }),
};

/** A stored guide from a melody on the lyrics' beat grid (the curve: notes with a little vibrato on long ones). */
function guideFromMelody(spec: MelodySpec, duration: number): { guide: StoredGuide; curve: PracticeGuide["curve"] } {
  const spb = 60 / spec.bpm;
  const notes: StoredGuideNote[] = [];
  spec.score.forEach((line, li) => {
    const tune = spec.tune[li % spec.tune.length];
    let beat = line.beat;
    tune.forEach(([deg, beats], k) => {
      const start = spec.offset + beat * spb;
      const last = k === tune.length - 1;
      const end = start + beats * spb * (last ? 0.82 : 0.9);
      const spoken = spec.spoken?.includes(li) ?? false;
      notes.push({
        start: Math.round(start * 1000) / 1000,
        end: Math.round(end * 1000) / 1000,
        midi: Math.round((spec.tonic + deg + spec.tuning / 100) * 1000) / 1000,
        confidence: spoken ? 0.8 : 0.93,
        kind: spoken ? "unpitched" : "pitched",
        octave: spoken ? "unchecked" : "ok",
        glide_in: k > 0 && !spoken && Math.abs(deg - tune[k - 1][0]) >= 5 ? 0.06 : 0,
      });
      beat += beats;
    });
  });
  for (const i of spec.unsure ?? []) if (notes[i]) notes[i].confidence = 0.42;
  const hop = 0.01;
  const midi: number[] = new Array(Math.floor(duration / hop) + 1).fill(0);
  for (const n of notes) {
    if (n.kind !== "pitched") continue;
    for (let k = Math.ceil(n.start / hop); k * hop < n.end && k < midi.length; k++) {
      const tt = k * hop - n.start;
      const vib = n.end - n.start > 0.6 && tt > 0.25 ? 0.18 * Math.sin(2 * Math.PI * 5.4 * tt) : 0;
      const glide = (n.glide_in ?? 0) > 0 && tt < n.glide_in! ? -1.2 * (1 - tt / n.glide_in!) : 0;
      midi[k] = Math.round((n.midi + vib + glide) * 100) / 100;
    }
  }
  // Spoken lines are one "spoken" span each (SONG_FORMAT 10.1).
  const spans: { start: number; end: number; kind: string }[] = [];
  let open: { start: number; end: number; kind: string } | null = null;
  for (const n of notes) {
    if (n.kind === "pitched") {
      open = null;
      continue;
    }
    if (open && n.start - open.end < 0.6) open.end = n.end;
    else spans.push((open = { start: n.start, end: n.end, kind: "spoken" }));
  }
  const guide: StoredGuide = {
    low_confidence: 0.5,
    tuning: { cents: spec.tuning, strength: spec.tuning === 0 ? 0.95 : 0.9, use_cents: spec.tuning },
    notes,
    spans,
    provenance: { method: "jk-pitch pyin+notes", version: "0.1.0", created_utc: "2026-11-19T12:00:00Z" },
  };
  return { guide, curve: { hop_s: hop, t0_s: 0, midi } };
}

/** Any other song with lyrics: one note per word, walking the major scale (the mock's "Add practice data"). */
function scaleWalkGuide(lyrics: Lyrics, key: SongKey | null): StoredGuide {
  const tonics = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  const tonic = 48 + Math.max(0, tonics.indexOf(key?.tonic ?? "C")) + (key && tonics.indexOf(key.tonic) < 5 ? 12 : 0);
  const scale = [0, 2, 4, 5, 7, 9, 7, 5, 4, 2];
  const notes: StoredGuideNote[] = [];
  let k = 0;
  for (const line of lyrics.lines) {
    for (const w of line.words) {
      notes.push({ start: w.start, end: Math.max(w.start + 0.12, w.end), midi: tonic + scale[k % scale.length], confidence: 0.9, kind: "pitched", octave: "ok", glide_in: 0 });
      k++;
    }
  }
  return { low_confidence: 0.5, tuning: { cents: 0, strength: 0.9, use_cents: 0 }, notes, spans: [], provenance: { method: "jk-pitch pyin+notes", version: "0.1.0" } };
}

/** The guide the mock serves for a song (null: no lyrics and no melody known). */
export function mockGuideFor(song: DemoSong, excluded: { start: number; end: number }[] = []): PracticeGuide | null {
  const spec = SPECS[song.summary.song_id];
  const lyrics = song.lyrics;
  if (spec) {
    // The mock's lyrics are generated from the same score (demo.ts), so notes and words share the beat grid.
    const { guide, curve } = guideFromMelody(spec(), song.summary.duration_s);
    return practiceGuideOf(song.summary.song_id, guide, lyrics, curve, { excluded });
  }
  if (!lyrics) return null;
  return practiceGuideOf(song.summary.song_id, scaleWalkGuide(lyrics, song.summary.key), lyrics, null, { excluded });
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

export interface MockPracticeOptions {
  platform: "windows" | "macos";
  mic: "granted" | "denied" | "not_determined" | "restricted" | "none";
  micPrivacyOff: boolean;
  music: "off" | "present" | "marginal" | "unusable";
  singerOctave: number;
}

/** What the session needs of the mock's player. */
export interface PracticeHost {
  now(): number;
  position(): number;
  playing(): boolean;
  ended(): boolean;
  song(): DemoSong | null;
  semitones(): number;
  keyAt(t: number): number;
  segment(): number;
  settings(): AppSettings;
  guideStatus(songId: string): GuideStatus | null;
  buildGuide(songId: string): void;
  emit(event: string, payload: unknown): void;
  setInterval(f: () => void, ms: number): unknown;
  clearInterval(h: unknown): void;
  pause(): void;
  seek(t: number): void;
  play(): void;
}

const EXPLANATION =
  "Practice mode listens to your microphone so it can show the note you are singing and grade it. It listens only while the practice screen is open, works out the pitch in memory on this computer, and never records, saves or sends your voice. Your computer will now ask whether Open Mic may use the microphone: choose Allow (OK on a Mac).";
const STEPS_MAC =
  "macOS is not letting Open Mic use the microphone. Open System Settings > Privacy & Security > Microphone and switch on Open Mic, then open practice mode again (if it still says no microphone, quit and reopen Open Mic). Each new build of the app may have to be allowed again, because it is signed ad hoc.";
const STEPS_WIN =
  'Windows is not letting apps use the microphone. Open Settings > Privacy & security > Microphone and switch on both "Microphone access" and "Let desktop apps access your microphone", then open practice mode again.';

const TRUST_TEXT: Record<string, string> = {
  music_in_input: "The app's own music came back through the input during this take.",
  removal_active: "It was removed well, so the grade is normal.",
  removal_marginal: "It could only partly be removed, so this grade is less certain.",
  untrusted: "It could not be removed (the delay kept changing, as over Bluetooth), so this take was not graded.",
  low_level: "Your voice was too quiet for a reliable pitch for part of the take: turn up the microphone gain on the mixer.",
  clipping: "The input clipped (too loud): turn the gain down a little on the mixer.",
  uncertain_guide: "The practice data for this song is unsure in many places, so the grade is less certain.",
  latency_unmeasured: "The timing between the music and your voice was not measured; small delays were forgiven.",
  input_lost: "The input stopped during the take (unplugged?).",
  dropouts: "Some of the input was lost.",
  bluetooth_input: "A Bluetooth microphone was used: its delay and sound make the grade less reliable.",
  stopped_early: "The take ended before the song did.",
};

/** Grade letters (APP_API 12.6, owner decision 3). */
export function gradeOf(overall: number): string {
  return overall >= 95 ? "A+" : overall >= 90 ? "A" : overall >= 85 ? "B+" : overall >= 80 ? "B" : overall >= 75 ? "C+" : overall >= 70 ? "C" : overall >= 60 ? "D" : "F";
}

const BANDS: Record<Strictness, [number, number]> = { relaxed: [75, 150], standard: [50, 100], strict: [30, 60] };

interface NoteStat {
  frames: number;
  credit: number;
  voiced: number;
  errSum: number;
  errN: number;
  key: number;
  done: boolean;
}

interface SongSlot {
  octave: { mode: OctaveMode; shift: number };
  strictness: Strictness | null;
  excluded: ExcludedSpan[];
  history: PracticeHistory;
}

function historyResult(date: string, overall: number, key: number, label: string | null, lines: number): PracticeResult {
  return {
    date,
    key,
    key_changes: 0,
    seeks: 0,
    octave: 0,
    octave_mode: "auto",
    strictness: "standard",
    overall,
    grade: gradeOf(overall),
    graded: 76,
    total: 84,
    tendency_cents: -9,
    sung_share: 1,
    voiced_share: 0.9,
    completed: true,
    trust: [],
    guide: "jk-pitch pyin+notes 0.1.0",
    scorer: "jk-pitch score 0.1.0",
    app: "0.2.0",
    lines: { basis: "lyrics", rows: Array.from({ length: lines }, (_, i) => [i, i * 4.8, Math.round(overall + ((i * 37) % 21) - 10), 7, -9] as [number, number, number, number, number]) },
    trust_messages: [],
    best: false,
    saved: true,
    key_label: label,
  };
}

export class MockPractice {
  private st: PracticeState;
  private guides = new Map<string, PracticeGuide>();
  private slots = new Map<string, SongSlot>();
  private permission: MicPermission;
  private inputs: MicInput[];
  private chosen: InputChoice = { device_id: null, device_name: null, channel: null };
  private meterTimer: unknown = null;
  private liveTimer: unknown = null;
  private lastT = Number.NaN;
  private seq = 0;
  private stats = new Map<number, NoteStat>();
  private ticks = 0;
  private listenSince = 0;
  private lastStateEmit = 0;
  private dirty = false;
  private takeKeys = new Map<number, number>();
  private lastKey = 0;
  private covered = new Set<number>();

  constructor(
    private host: PracticeHost,
    private o: MockPracticeOptions,
  ) {
    this.permission = this.permissionNow();
    const usb: MicInput = {
      id: "wasapi:in:usb-mixer",
      name: "USB Audio Mixer (USB Audio CODEC)",
      adapter: "USB Audio CODEC",
      kind: "usb",
      channels: 2,
      sample_rate: 48000,
      sample_format: "i16",
      is_default: false,
      bluetooth: false,
      preferred: true,
      declared_latency_ms: 10,
      same_device_as_output: false,
      warning: null,
    };
    const builtIn: MicInput = {
      id: "wasapi:in:builtin",
      name: o.platform === "macos" ? "MacBook Air Microphone" : "Microphone Array (Realtek Audio)",
      adapter: o.platform === "macos" ? null : "Realtek Audio",
      kind: "built_in",
      channels: 1,
      sample_rate: 48000,
      sample_format: "f32",
      is_default: true,
      bluetooth: false,
      preferred: false,
      declared_latency_ms: 12,
      same_device_as_output: false,
      warning: null,
    };
    const bt: MicInput = {
      id: "wasapi:in:bt-headset",
      name: "Headset (Bluetooth Hands-Free)",
      adapter: "Bluetooth",
      kind: "bluetooth",
      channels: 1,
      sample_rate: 16000,
      sample_format: "i16",
      is_default: false,
      bluetooth: true,
      preferred: false,
      declared_latency_ms: 120,
      same_device_as_output: false,
      warning: "Opening a Bluetooth headset's microphone turns its sound into low-quality mono. Use a cable or the mixer's USB connection for practice.",
    };
    this.inputs = o.mic === "none" ? [] : [usb, builtIn, bt];
    this.st = this.offState();
    // Twinkle already has a history (best and three recent takes) on the stick.
    this.slots.set("Jane Taylor - Twinkle, Twinkle, Little Star.jksong", {
      octave: { mode: "auto", shift: 0 },
      strictness: null,
      excluded: [],
      history: (() => {
        const best = historyResult("2026-11-18T20:41:00Z", 86.2, 2, "A major → B major (+2)", 18);
        best.best = true;
        return {
          best,
          recent: [
            historyResult("2026-11-19T09:12:00Z", 78.4, 0, "A major (original key)", 18),
            best,
            historyResult("2026-11-17T19:03:00Z", 71.9, 2, "A major → B major (+2)", 18),
          ],
        };
      })(),
    });
  }

  // --- helpers -----------------------------------------------------------------

  private permissionNow(): MicPermission {
    const mac = this.o.platform === "macos";
    const privacyOff = !mac && this.o.micPrivacyOff;
    const state: MicPermission["state"] =
      privacyOff ? "denied" : this.o.mic === "none" ? "granted" : this.o.mic === "not_determined" && !mac ? "granted" : this.o.mic;
    return {
      state,
      explanation: EXPLANATION,
      steps: state === "denied" || state === "restricted" ? (mac ? STEPS_MAC : STEPS_WIN) : null,
      settings_url: mac ? "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone" : "ms-settings:privacy-microphone",
      windows: mac ? null : { microphone_access: privacyOff ? "deny" : "allow", user_access: "allow", desktop_apps: "allow" },
    };
  }

  private slot(songId: string): SongSlot {
    let s = this.slots.get(songId);
    if (!s) {
      const pr = this.host.settings().practice;
      s = { octave: { mode: pr?.octave_mode ?? "auto", shift: 0 }, strictness: null, excluded: [], history: { best: null, recent: [] } };
      this.slots.set(songId, s);
    }
    return s;
  }

  private music(): MusicInInput {
    return { presence: "absent", level_db: null, delay_ms: null, stability: "unknown", drift_ppm: null, removal: "not_needed", suppression_db: null, grading: "normal", message: null };
  }

  private offState(): PracticeState {
    return {
      active: false,
      song_id: null,
      seq: ++this.seq,
      phase: "off",
      guide: { state: "none", reason: null, error_code: null, progress: null, in_memory: false, pending_write: false, method: null, version: null, created_utc: null, counts: null, tuning_cents: null },
      not_ready: null,
      watch_only: null,
      input: null,
      music: this.music(),
      octave: { mode: "auto", shift: 0, settled: false },
      strictness: "standard",
      latency: { applied_ms: 0, source: "none", measured_ms: null, manual_ms: 0 },
      take: null,
      excluded: [],
      last_result: null,
    };
  }

  private snapshot(): PracticeState {
    return JSON.parse(JSON.stringify(this.st)) as PracticeState;
  }

  private emitState(): void {
    this.st.seq = ++this.seq;
    this.lastStateEmit = this.host.now();
    this.dirty = false;
    this.host.emit("practice://state", this.snapshot());
  }

  private guideOf(songId: string): PracticeGuide | null {
    let g = this.guides.get(songId) ?? null;
    if (!g) {
      const song = this.host.song();
      if (!song || song.summary.song_id !== songId) return null;
      g = mockGuideFor(song, this.slot(songId).excluded);
      if (g) this.guides.set(songId, g);
    }
    return g;
  }

  private chosenInput(): MicInput | null {
    const byId = this.inputs.find((i) => i.id === this.chosen.device_id);
    return byId ?? this.inputs.find((i) => i.preferred) ?? null;
  }

  // --- 12.4 microphone ------------------------------------------------------------

  micPermission(): MicPermission {
    return { ...this.permission };
  }

  micRequestPermission(): MicPermission {
    if (this.permission.state === "not_determined") {
      // macOS shows its prompt; the mock "allows" after a moment.
      setTimeout(() => {
        this.permission = { ...this.permission, state: "granted", steps: null };
        this.host.emit("mic://permission", { ...this.permission });
      }, 500);
    }
    return { ...this.permission };
  }

  micList(): MicList {
    const auto = this.inputs.find((i) => i.preferred)?.id ?? null;
    return { inputs: this.inputs.map((i) => ({ ...i })), chosen: { ...this.chosen }, automatic: auto };
  }

  micChoose(choice: InputChoice): InputChoice {
    const dev = this.inputs.find((i) => i.id === choice.device_id) ?? null;
    this.chosen = { device_id: dev?.id ?? null, device_name: dev?.name ?? null, channel: choice.channel };
    if (this.st.phase === "listening" || (this.st.phase === "watch_only" && this.st.watch_only?.code === "chosen")) {
      this.openInput();
      this.emitState();
    }
    return { ...this.chosen };
  }

  micMeter(deviceId: string | null): { device: MicInput | null; error: null | { kind: "no_device"; message: string } } {
    if (this.meterTimer !== null) this.host.clearInterval(this.meterTimer);
    this.meterTimer = null;
    if (deviceId === null) return { device: null, error: null };
    const dev = this.inputs.find((i) => i.id === deviceId) ?? null;
    if (!dev) return { device: null, error: { kind: "no_device", message: "That input is not connected any more." } };
    this.meterTimer = this.host.setInterval(() => this.host.emit("mic://levels", this.levels(dev)), 50);
    return { device: { ...dev }, error: null };
  }

  /** Simulated per-channel meters: the voice on channel 1 of the mixer, nothing on channel 2. */
  private levels(dev: MicInput) {
    const t = this.host.now() / 1000;
    const voice = -19 + 6 * Math.sin(t * 2.1) * Math.sin(t * 0.7);
    const meter = (index: number): ChannelMeter => {
      if (dev.kind === "built_in") {
        const rms = -41 + 4 * Math.sin(t * 1.7);
        return { index, peak_db: rms + 9, rms_db: rms, noise_floor_db: -62, active_db: rms, clipping: false, clipped_samples: 0, advice: "too_quiet", message: "Too quiet: move closer to the microphone or raise its level in the system's sound settings." };
      }
      if (index === 0) return { index, peak_db: voice + 9, rms_db: voice, noise_floor_db: -68, active_db: voice, clipping: false, clipped_samples: 0, advice: "good", message: null };
      return { index, peak_db: -76, rms_db: -82, noise_floor_db: -84, active_db: null, clipping: false, clipped_samples: 0, advice: "silent", message: "Nothing on this channel." };
    };
    const channels = Array.from({ length: dev.channels }, (_, i) => meter(i));
    return { device_id: dev.id, seconds: t, channels, suggested_channel: 0, chosen_channel: this.chosen.device_id === dev.id ? this.chosen.channel : null };
  }

  // --- 12.5 session ---------------------------------------------------------------

  state(): PracticeState {
    return this.snapshot();
  }

  start(opts: PracticeStartOptions = {}): PracticeState {
    const song = this.host.song();
    if (!song) throw { code: "not_loaded", message: "Choose a song first." };
    const id = song.summary.song_id;
    if (this.st.active && this.st.song_id === id && this.st.phase !== "not_ready") return this.snapshot();
    const slot = this.slot(id);
    const status = this.host.guideStatus(id);
    this.st = {
      ...this.offState(),
      active: true,
      song_id: id,
      guide: status ? { ...status } : this.offState().guide,
      octave: { mode: slot.octave.mode, shift: slot.octave.mode === "strict" ? 0 : slot.octave.shift, settled: slot.octave.mode !== "auto" },
      strictness: slot.strictness ?? this.host.settings().practice?.strictness ?? "standard",
      excluded: slot.excluded.map((x) => ({ ...x })),
      last_result: null,
    };
    const usable = status && (status.state === "ready" || status.state === "outdated" || status.state === "newer" || status.in_memory);
    if (!usable) {
      if (status?.state === "failed" && status.error_code === "guide_no_lead") {
        this.st.phase = "not_ready";
        this.st.not_ready = { code: "guide_no_lead", message: status.reason ?? "This song has no separate lead vocal, so there is no melody to practise.", can_retry: false };
      } else if (!song.lyrics && !SPECS[id]) {
        this.st.phase = "not_ready";
        this.st.not_ready = { code: "no_guide", message: "This song has no lead vocal melody the app could follow, so it cannot be practised.", can_retry: false };
      } else {
        // Built on the spot (APP_API 12.2): progress comes as guide://status.
        this.st.phase = "preparing";
        this.host.buildGuide(id);
      }
      this.emitState();
      return this.snapshot();
    }
    this.afterGuide(opts.watch_only ?? false);
    return this.snapshot();
  }

  /** A guide://status for the practised song (the mock forwards every one). */
  onGuideStatus(songId: string, status: GuideStatus): void {
    if (!this.st.active || this.st.song_id !== songId) return;
    this.st.guide = { ...status };
    if (this.st.phase !== "preparing") return;
    if (status.state === "ready" || status.in_memory) {
      this.guides.delete(songId);
      this.afterGuide(false);
    } else if (status.state === "failed") {
      this.st.phase = "not_ready";
      this.st.not_ready = { code: status.error_code ?? "no_guide", message: status.reason ?? "Practice data could not be added.", can_retry: status.error_code !== "guide_no_lead" };
      this.emitState();
    } else this.emitState();
  }

  private afterGuide(watchOnly: boolean): void {
    const p = this.permission.state;
    if (watchOnly) this.watchOnly("chosen", "Watch only: the notes and keys show the melody; nothing listens and nothing is graded.");
    else if (this.inputs.length === 0) this.watchOnly("no_device", "No microphone or audio input was found. Plug in your mixer or microphone, then choose it with Input.");
    else if (p === "denied") this.watchOnly("permission_denied", this.permission.steps ?? "The microphone is not allowed.");
    else if (p === "restricted") this.watchOnly("restricted", this.permission.steps ?? "A device policy does not allow the microphone.");
    else if (p === "not_determined") this.watchOnly("not_determined", "The app may not use the microphone yet.");
    else {
      this.st.phase = "listening";
      this.st.watch_only = null;
      this.openInput();
    }
    this.emitState();
  }

  private watchOnly(code: NonNullable<PracticeState["watch_only"]>["code"], message: string): void {
    this.stopInput();
    this.st.phase = "watch_only";
    this.st.watch_only = { code, message };
    this.st.input = null;
  }

  private openInput(): void {
    const dev = this.chosenInput();
    if (!dev) return;
    this.st.input = {
      device: { ...dev },
      channel: this.chosen.channel ?? 0,
      sample_rate: dev.sample_rate,
      state: "running",
      message: dev.bluetooth ? dev.warning : null,
      meter: this.levels(dev).channels[this.chosen.channel ?? 0] ?? null,
      dropped_blocks: 0,
    };
    this.st.latency = { applied_ms: 0, source: "none", measured_ms: null, manual_ms: this.st.latency.manual_ms };
    this.st.music = { ...this.music(), presence: "searching" };
    this.listenSince = this.host.now();
    this.lastT = Number.NaN;
    if (this.liveTimer === null) this.liveTimer = this.host.setInterval(() => this.tick(), 16);
    if (this.host.playing() && !this.st.take) this.beginTake();
  }

  private stopInput(): void {
    if (this.liveTimer !== null) this.host.clearInterval(this.liveTimer);
    this.liveTimer = null;
  }

  stop(keepResult = true): { result: PracticeResult | null } {
    let result: PracticeResult | null = null;
    if (this.st.take?.running) {
      const covered = this.coveredShare();
      if (covered >= 0.1 && keepResult) result = this.endTake(false);
    }
    this.stopInput();
    this.micMeter(null);
    this.st = this.offState();
    this.emitState();
    return { result };
  }

  suspend(suspended: boolean): void {
    if (!this.st.input) return;
    this.st.input.state = suspended ? "suspended" : "running";
    this.emitState();
  }

  finish(): { result: PracticeResult | null } {
    if (!this.st.take?.running) return { result: this.st.last_result };
    const result = this.endTake(false);
    this.host.pause();
    return { result };
  }

  again(): PracticeState {
    if (!this.st.active) throw { code: "bad_request", message: "Practice is not on." };
    this.host.seek(0);
    this.st.phase = this.st.watch_only ? "watch_only" : "listening";
    this.st.take = null;
    this.st.last_result = null;
    this.stats.clear();
    this.covered.clear();
    this.lastT = Number.NaN;
    if (this.st.phase === "listening") this.beginTake();
    this.host.play();
    this.emitState();
    return this.snapshot();
  }

  guide(songId?: string): PracticeGuide {
    const id = songId ?? this.host.song()?.summary.song_id ?? "";
    const g = this.guideOf(id);
    if (!g) throw { code: "no_guide", message: "This song has no practice data yet." };
    return JSON.parse(JSON.stringify(g)) as PracticeGuide;
  }

  songSet(songId: string, changes: { octave?: { mode: OctaveMode; shift?: number }; strictness?: Strictness | null }): PracticeState {
    const slot = this.slot(songId);
    if (changes.octave) {
      slot.octave = { mode: changes.octave.mode, shift: changes.octave.mode === "strict" ? 0 : changes.octave.shift ?? slot.octave.shift };
      if (this.st.song_id === songId) this.st.octave = { mode: slot.octave.mode, shift: slot.octave.shift, settled: slot.octave.mode !== "auto" };
    }
    if (changes.strictness !== undefined) {
      slot.strictness = changes.strictness;
      if (this.st.song_id === songId) this.st.strictness = changes.strictness ?? this.host.settings().practice?.strictness ?? "standard";
    }
    this.emitState();
    return this.snapshot();
  }

  exclude(songId: string, span: ExcludedSpan, excluded: boolean): { excluded: ExcludedSpan[] } {
    const slot = this.slot(songId);
    slot.excluded = slot.excluded.filter((x) => !(Math.abs(x.start - span.start) < 1e-3 && Math.abs(x.end - span.end) < 1e-3));
    if (excluded) slot.excluded.push({ ...span });
    slot.excluded.sort((a, b) => a.start - b.start);
    this.guides.delete(songId);
    if (this.st.song_id === songId) {
      this.st.excluded = slot.excluded.map((x) => ({ ...x }));
      this.emitState();
    }
    return { excluded: slot.excluded.map((x) => ({ ...x })) };
  }

  history(songId: string): PracticeHistory {
    const h = this.slot(songId).history;
    return JSON.parse(JSON.stringify(h)) as PracticeHistory;
  }

  latencySet(ms: number): { manual_ms: number } {
    const v = Math.max(-500, Math.min(500, Math.round(ms)));
    this.st.latency = { ...this.st.latency, manual_ms: v, applied_ms: this.st.latency.source === "none" || this.st.latency.source === "manual" ? v : this.st.latency.applied_ms, source: v !== 0 && this.st.latency.source === "none" ? "manual" : this.st.latency.source };
    this.emitState();
    return { manual_ms: v };
  }

  targetAt(t: number): PracticeTarget {
    const id = this.host.song()?.summary.song_id ?? "";
    const g = this.guideOf(id);
    const key = this.host.keyAt(t);
    const octave = this.st.octave.shift;
    const i = g ? g.notes.findIndex((n) => n.start <= t && t < n.end) : -1;
    if (!g || i < 0) return { note: null, midi: null, lane: null, name: null, key, octave, graded: false };
    const n = g.notes[i];
    const midi = n.midi + key + 12 * octave;
    const lane = midi - g.tuning_cents / 100;
    const song = this.host.song();
    return { note: i, midi, lane, name: noteName(Math.round(lane), spellingFor(song?.summary.key ?? null, key)), key, octave, graded: n.graded };
  }

  /** The player changed state (play, pause, seek, end): takes start and end with it. */
  onPlayer(): void {
    if (this.st.phase !== "listening") return;
    if (this.host.playing() && !this.st.take) this.beginTake();
    if (this.host.ended() && this.st.take?.running) this.endTake(true);
  }

  /** A seek: counted, values start afresh. */
  onSeek(): void {
    if (this.st.take?.running) this.st.take.seeks++;
    this.lastT = Number.NaN;
  }

  dispose(): void {
    this.stopInput();
    if (this.meterTimer !== null) this.host.clearInterval(this.meterTimer);
    this.meterTimer = null;
  }

  // --- the simulated singer and scorer ---------------------------------------------

  private beginTake(): void {
    const pos = this.host.position();
    this.st.take = { running: true, started_s: pos, covered: 0, graded: 0, total: 0, overall: null, seeks: 0, key_changes: 0 };
    this.stats.clear();
    this.covered.clear();
    this.takeKeys.clear();
    this.lastKey = this.host.keyAt(pos);
    this.st.last_result = null;
    this.dirty = true;
  }

  /** Per note: what the simulated singer does (deterministic). */
  private behaviour(i: number): "on" | "sharp" | "flat" | "half" | "silent" {
    const r = prng(i * 7919 + 13)();
    return r < 0.64 ? "on" : r < 0.76 ? "sharp" : r < 0.87 ? "flat" : r < 0.95 ? "half" : "silent";
  }

  /** The singer's error from the note (cents) at time tt into the note, or null for no voice. */
  private singerCents(i: number, tt: number, len: number): number | null {
    if (tt < 0.04 || tt > len - 0.03) return null;
    const b = this.behaviour(i);
    if (b === "silent") return null;
    const vib = len > 0.45 && tt > 0.2 ? 17 * Math.sin(2 * Math.PI * 5.3 * tt) : 0;
    const scoop = tt < 0.09 ? -60 * (1 - (tt - 0.04) / 0.05) : 0;
    const base = b === "sharp" ? 78 : b === "flat" ? -88 : b === "half" && tt > len / 2 ? -82 : 0;
    return base + vib + Math.max(scoop, -60) * (scoop < 0 ? 1 : 0);
  }

  private coveredShare(): number {
    const g = this.st.song_id ? this.guideOf(this.st.song_id) : null;
    if (!g) return 0;
    let all = 0;
    let cov = 0;
    g.notes.forEach((n, i) => {
      if (!n.graded) return;
      all += n.end - n.start;
      if (this.covered.has(i)) cov += n.end - n.start;
    });
    return all > 0 ? cov / all : 0;
  }

  private tick(): void {
    const id = this.st.song_id;
    if (!id || this.st.phase !== "listening" || !this.st.input || this.st.input.state !== "running") return;
    const g = this.guideOf(id);
    if (!g) return;
    this.ticks++;
    const now = this.host.now();
    const dev = this.st.input.device;
    if (this.ticks % 3 === 0) {
      const lv = this.levels(dev);
      this.host.emit("mic://levels", lv);
      this.st.input.meter = lv.channels[this.st.input.channel] ?? null;
    }
    // Music in the input (on request): found a few seconds after listening starts.
    if (this.o.music !== "off" && this.st.music.presence !== "present" && now - this.listenSince > 4000) {
      const removal = this.o.music === "present" ? "good" : this.o.music === "marginal" ? "marginal" : "unusable";
      this.st.music = {
        presence: "present",
        level_db: this.o.music === "unusable" ? -9 : -14,
        delay_ms: this.o.music === "unusable" ? 182 : 23,
        stability: this.o.music === "unusable" ? "wandering" : "stable",
        drift_ppm: this.o.music === "unusable" ? null : 3,
        removal,
        suppression_db: removal === "good" ? 41 : removal === "marginal" ? 17 : null,
        grading: removal === "good" ? "normal" : removal === "marginal" ? "less_certain" : "not_graded",
        message:
          removal === "unusable"
            ? "The app's music is coming back through your input, with a delay that keeps changing (as over Bluetooth), so it cannot be removed. The marker still shows, but this take will not be graded. Play the music to the mixer over its USB connection or a cable instead."
            : `The mixer is sending the app's music back to it on channel 1, ${this.o.music === "present" ? "23" : "23"} ms late. ${removal === "good" ? "The app is removing it well, so the marker and the grade are not affected." : "The app can only partly remove it, so the marker may jump and the grade is less certain."} To stop it, switch off LOOPBACK on the mixer, or keep the music out of the mixer's input.`,
      };
      if (this.o.music !== "unusable") this.st.latency = { ...this.st.latency, applied_ms: 23, source: "returned_music", measured_ms: 23 };
      this.dirty = true;
    }
    // Auto octave: decided from the first sung phrase (the mock's singer may sing an octave down).
    if (this.st.take?.running && this.st.octave.mode === "auto" && !this.st.octave.settled && this.host.position() - this.st.take.started_s > 6) {
      this.st.octave = { mode: "auto", shift: this.o.singerOctave, settled: true };
      this.slot(id).octave = { mode: "auto", shift: this.o.singerOctave };
      this.dirty = true;
    }
    const pos = this.host.position();
    const pitch: PracticeLive["pitch"] = [];
    const hits: PracticeLive["hits"] = [];
    const done: PracticeLive["done"] = [];
    const [full, none] = BANDS[this.st.strictness];
    if (!this.host.playing()) {
      // Paused: the singer waits (no pitch at the paused position).
      pitch.push([Math.round(pos * 1000) / 1000, null, -62]);
    } else {
      const upTo = pos - 0.045;
      if (Number.isNaN(this.lastT) || upTo - this.lastT > 2 || upTo < this.lastT - 0.05) this.lastT = upTo - 0.01;
      let j = g.notes.findIndex((n) => n.end > this.lastT);
      for (let tau = this.lastT + 0.01; tau <= upTo + 1e-9; tau += 0.01) {
        this.lastT = tau;
        while (j >= 0 && j < g.notes.length && g.notes[j].end <= tau) {
          this.finishNote(j, done);
          j++;
        }
        if (j < 0) j = g.notes.length;
        const n = j < g.notes.length && g.notes[j].start <= tau ? g.notes[j] : null;
        const key = this.host.keyAt(tau);
        if (this.st.take?.running) {
          if (key !== this.lastKey) {
            this.st.take.key_changes++;
            this.lastKey = key;
          }
          this.takeKeys.set(key, (this.takeKeys.get(key) ?? 0) + 0.01);
        }
        let midi: number | null = null;
        if (n && n.kind === "pitched") {
          const c = this.singerCents(j, tau - n.start, n.end - n.start);
          if (c !== null) midi = n.midi + key + 12 * this.o.singerOctave + c / 100;
          if (n.graded && this.st.take?.running) {
            this.covered.add(j);
            const s = this.stats.get(j) ?? { frames: 0, credit: 0, voiced: 0, errSum: 0, errN: 0, key, done: false };
            s.frames++;
            if (c !== null && this.st.music.grading !== "not_graded") {
              let e = c + (this.o.singerOctave - this.st.octave.shift) * 1200;
              if (this.st.octave.mode !== "strict") e = foldCents(e);
              const a = Math.abs(e);
              s.voiced++;
              s.credit += a <= full ? 1 : a >= none ? 0 : (none - a) / (none - full);
              s.errSum += e;
              s.errN++;
              const where: -1 | 0 | 1 = a <= full ? 0 : e > 0 ? 1 : -1;
              const last = hits[hits.length - 1];
              if (last && last[0] === j && last[3] === where && Math.abs(last[2] - tau) < 0.015) last[2] = Math.round((tau + 0.01) * 1000) / 1000;
              else hits.push([j, Math.round(tau * 1000) / 1000, Math.round((tau + 0.01) * 1000) / 1000, where]);
            }
            this.stats.set(j, s);
          }
        }
        pitch.push([Math.round(tau * 1000) / 1000, midi === null ? null : Math.round(midi * 1000) / 1000, midi === null ? -58 : -19]);
      }
    }
    if (pitch.length || hits.length || done.length) {
      this.host.emit("practice://live", { seq: ++this.seq, segment: this.host.segment(), pitch, hits, done } satisfies PracticeLive);
    }
    if (this.st.take?.running && done.length) {
      this.updateTake();
      this.dirty = true;
    }
    if (this.dirty && now - this.lastStateEmit > 250) this.emitState();
  }

  private finishNote(j: number, done: PracticeLive["done"]): void {
    const s = this.stats.get(j);
    const g = this.st.song_id ? this.guideOf(this.st.song_id) : null;
    if (!g) return;
    const n = g.notes[j];
    if (!s || s.done) {
      if (!s && n && this.st.take?.running && this.covered.has(j)) done.push([j, null, this.host.keyAt(n.start), null]);
      return;
    }
    s.done = true;
    done.push([j, n.graded ? Math.round((s.credit / Math.max(1, s.frames)) * 1000) / 1000 : null, s.key, s.errN ? Math.round(s.errSum / s.errN) : null]);
  }

  private updateTake(): void {
    const g = this.st.song_id ? this.guideOf(this.st.song_id) : null;
    const take = this.st.take;
    if (!g || !take) return;
    let w = 0;
    let c = 0;
    let graded = 0;
    for (const [j, s] of this.stats) {
      if (!s.done) continue;
      const n = g.notes[j];
      const len = n.end - n.start;
      w += len;
      c += len * (s.credit / Math.max(1, s.frames));
      graded++;
    }
    take.graded = graded;
    take.total = g.notes.filter((n, i) => this.covered.has(i) || (n.start >= take.started_s && n.end <= this.host.position())).length;
    take.overall = w > 0 && this.st.music.grading !== "not_graded" ? Math.round((1000 * c) / w) / 10 : null;
    take.covered = Math.round(this.coveredShare() * 100) / 100;
  }

  private endTake(completed: boolean): PracticeResult {
    const id = this.st.song_id!;
    const g = this.guideOf(id)!;
    const song = this.host.song()!;
    this.updateTake();
    const take = this.st.take!;
    // Finish notes still open at the end.
    for (const [j] of this.stats) this.finishNote(j, []);
    this.updateTake();
    let errSum = 0;
    let errN = 0;
    let voiced = 0;
    let frames = 0;
    for (const s of this.stats.values()) {
      errSum += s.errSum;
      errN += s.errN;
      voiced += s.voiced;
      frames += s.frames;
    }
    const lyrics = song.lyrics;
    const rows: PracticeResult["lines"]["rows"] = g.lines.map((ln) => {
      let w = 0;
      let c = 0;
      let k = 0;
      let es = 0;
      let en = 0;
      g.notes.forEach((n, j) => {
        const s = this.stats.get(j);
        if (!s || !n.graded || n.start < ln.start - 0.05 || n.start >= ln.end + 0.05) return;
        w += n.end - n.start;
        c += (n.end - n.start) * (s.credit / Math.max(1, s.frames));
        k++;
        es += s.errSum;
        en += s.errN;
      });
      return [ln.line, Math.round(ln.start * 10) / 10, k > 0 && this.st.music.grading !== "not_graded" ? Math.round((100 * c) / Math.max(1e-9, w)) : null, k, en ? Math.round(es / en) : null];
    });
    const trust: string[] = [];
    if (this.st.music.presence === "present") {
      trust.push("music_in_input");
      trust.push(this.st.music.removal === "good" ? "removal_active" : this.st.music.removal === "marginal" ? "removal_marginal" : "untrusted");
    }
    if (this.st.latency.source === "none") trust.push("latency_unmeasured");
    if (this.st.input?.device.bluetooth) trust.push("bluetooth_input");
    const sung = this.coveredShare();
    if (!completed) trust.push("stopped_early");
    let mainKey = this.host.keyAt(take.started_s);
    let most = -1;
    for (const [k, s] of this.takeKeys) if (s > most) [most, mainKey] = [s, k];
    const overall = take.overall;
    const slot = this.slot(id);
    const result: PracticeResult = {
      date: new Date(Date.UTC(2026, 10, 19, 20, 15, 0)).toISOString().replace(".000", ""),
      key: mainKey,
      key_changes: take.key_changes,
      seeks: take.seeks,
      octave: this.st.octave.shift,
      octave_mode: this.st.octave.mode,
      strictness: this.st.strictness,
      overall,
      grade: overall === null ? null : gradeOf(overall),
      graded: take.graded,
      total: Math.max(take.total, take.graded),
      tendency_cents: errN > 20 ? Math.round(errSum / errN) : null,
      sung_share: Math.round(sung * 100) / 100,
      voiced_share: frames ? Math.round((voiced / frames) * 100) / 100 : 0,
      completed,
      trust,
      guide: `${g.status.method ?? "jk-pitch pyin+notes"} ${g.status.version ?? "0.1.0"}`,
      scorer: "jk-pitch score 0.1.0",
      app: "0.2.0",
      lines: { basis: lyrics ? "lyrics" : "phrases", rows },
      trust_messages: trust.map((f) => TRUST_TEXT[f] ?? f),
      best: false,
      saved: sung >= 0.1,
      key_label: keyLabel(song.summary.key, mainKey, "→"),
    };
    if (result.saved) {
      const h = slot.history;
      const eligible = result.overall !== null && result.sung_share >= 0.9 && !trust.includes("untrusted");
      if (eligible && (!h.best || (h.best.overall ?? 0) < (result.overall ?? 0))) {
        result.best = true;
        h.best = { ...result };
      }
      h.recent = [{ ...result }, ...h.recent].slice(0, 20);
    }
    this.st.take = { ...take, running: false };
    this.st.last_result = result;
    this.st.phase = "result";
    this.host.emit("practice://result", JSON.parse(JSON.stringify(result)));
    this.emitState();
    return result;
  }
}

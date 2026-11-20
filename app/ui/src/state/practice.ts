// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode's state and actions in the UI (WO2 Stage 4; docs/APP_API.md section 12).
// Practice is a mode of the singing screen, not a second player: the transport, the key
// dial and the mixer keep working through state/app.ts, and with practice off nothing here
// runs or draws. The core does the listening, the octave decision and the grading; the UI
// asks, draws and shows the result.
//
// Two kinds of state:
//   - `practiceStore` (a Store, re-renders the screens): the session (PracticeState), the
//     guide as the lane draws it, the result screen, the history, the input picker;
//   - module objects read by the lane's frame loop only: `live` (practice://live batches,
//     about 60 a second, kept out of the store), `keys` (the key HEARD over song time, from
//     playback://clock and playback://state; the bars follow it, never the dial).
import type { Api } from "../api/client";
import { errorMessage } from "../api/client";
import type { MicLevelsEvent, MicList, MicPermission, OctaveMode, PracticeGuide, PracticeHistory, PracticeLive, PracticeResult, PracticeState, Strictness } from "../api/types";
import { measureLaneCost } from "../practice/lanecost";
import { LiveStore } from "../practice/live";
import { KeyTimeline } from "../practice/pitch";
import { clock, store, toast } from "./app";
import { Store, useSelector } from "./store";

export type PracticePanel = null | "permission" | "picker" | "music";

export interface PracticeUi {
  /** Practice mode is on for the loaded song (the singing screen shows the lane). */
  on: boolean;
  /** Waiting for the core (start, finish, again). */
  busy: boolean;
  state: PracticeState | null;
  guide: PracticeGuide | null;
  /** The result screen is showing this take. */
  result: PracticeResult | null;
  history: PracticeHistory | null;
  panel: PracticePanel;
  /** The input picker: the inputs, the device being metered and its levels. */
  mics: MicList | null;
  meterDevice: string | null;
  levels: MicLevelsEvent | null;
  permission: MicPermission | null;
  /** The singer dismissed the music-in-input panel for this session (the chip stays). */
  musicSeen: boolean;
  /** Plain words when practice could not start at all (an older core). */
  error: string | null;
}

export const practiceStore = new Store<PracticeUi>({
  on: false,
  busy: false,
  state: null,
  guide: null,
  result: null,
  history: null,
  panel: null,
  mics: null,
  meterDevice: null,
  levels: null,
  permission: null,
  musicSeen: false,
  error: null,
});

export const usePractice = <T>(selector: (s: PracticeUi) => T): T => useSelector(practiceStore, selector);

/** What the lane draws the marker and the lit parts from. */
export const live = new LiveStore();
/** The key heard over song time (APP_API 12.1, "key_at(t) for the lane"). */
export const keys = new KeyTimeline();
/** performance.now() of the last practice://live batch (the marker hides when they stop). */
export let liveMs = -1e9;

/** The lane's frame loop asks to be woken by live batches and key changes. */
const kicks = new Set<() => void>();
export function onPracticeKick(f: () => void): () => void {
  kicks.add(f);
  return () => kicks.delete(f);
}
function kick(): void {
  for (const f of [...kicks]) f();
}

let api: Api | null = null;

/**
 * Debug hook for the real-app checks (app/ui/scripts/real-app-practice.mjs), like
 * window.__jkDebug: the practice store and the recent practice://live batches (about the
 * last minute). Read only by test scripts; nothing here is stored anywhere.
 */
const debugBatches: PracticeLive[] = [];
if (typeof window !== "undefined") {
  (window as unknown as { __jkPractice?: unknown }).__jkPractice = { store: practiceStore, batches: debugBatches, keys };
}
/** The page-wide listeners (store, visibility) are added once, whatever the number of init() calls (tests). */
let wired = false;

/** The key heard as the player reports it (an older core without it: the key asked for). */
function heardOf(p: { key_heard?: number; key_requested?: number } | null | undefined): number {
  return p?.key_heard ?? p?.key_requested ?? store.get().songSettings?.semitones ?? 0;
}

/** Subscribes to the practice, microphone and key events (called once from init()). */
export function initPractice(theApi: Api): void {
  api = theApi;
  theApi.on("practice://live", (b) => {
    live.apply(b);
    liveMs = performance.now();
    debugBatches.push(b);
    if (debugBatches.length > 4000) debugBatches.splice(0, debugBatches.length - 3000);
    kick();
  });
  theApi.on("practice://state", (st) => applyState(st));
  theApi.on("practice://result", (r) => {
    practiceStore.set({ result: r });
    void refreshHistory();
  });
  theApi.on("mic://levels", (lv) => {
    const p = practiceStore.get();
    if (p.panel === "picker" && (p.meterDevice === lv.device_id || p.meterDevice === null)) practiceStore.set({ levels: lv });
  });
  theApi.on("mic://permission", (perm) => {
    practiceStore.set({ permission: perm });
    const p = practiceStore.get();
    // The OS answered after the app's explanation: go on as the singer chose.
    if (p.on && p.panel === "permission" && perm.state !== "not_determined") {
      practiceStore.set({ panel: null });
      void startSession(false);
    }
  });
  theApi.on("mic://devices", () => {
    if (practiceStore.get().panel === "picker") void refreshMics();
  });
  // The key heard: from every clock and state event, so the bars move when the key is heard.
  theApi.on("playback://clock", (ev) => {
    if (!practiceStore.get().on) return;
    if (keys.update(ev.position_s, ev.key_heard ?? heardOf(store.get().player), ev.key_pending, ev.segment, performance.now())) kick();
  });
  theApi.on("playback://state", (ps) => {
    if (!practiceStore.get().on) return;
    if (ps.song_id && ps.song_id !== practiceStore.get().state?.song_id && practiceStore.get().state?.song_id) {
      // Another song was loaded: the core ended the session (APP_API 12.2).
      endLocally();
      return;
    }
    if (keys.update(ps.position_s, heardOf(ps), ps.key_pending, ps.key_segment, performance.now())) kick();
  });
  theApi.onWindowMinimised?.((m) => suspendFor(m));
  // `--selfcheck` runs (APP_API 10.1, "Practice lane"): the core evaluates
  // `window.__jkSelfcheckLane()` in the hidden window, as it does for the user agent; the
  // page measures the lane's drawing cost in this web view and reports it (selfcheck_lane).
  if (typeof window !== "undefined") {
    (window as unknown as { __jkSelfcheckLane?: () => Promise<unknown> }).__jkSelfcheckLane = async () => {
      const lane = await measureLaneCost().catch(() => null);
      if (lane) await theApi.selfcheckLane?.(lane).catch(() => undefined);
      return lane;
    };
  }
  if (wired) return;
  wired = true;
  // A hidden or minimised window closes the microphone; it reopens when the window is back.
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => suspendFor(document.hidden));
  }
  // The microphone is open only while practice is on screen (WO2 4.4): leaving the singing
  // screen (not full screen, which shows it alone) leaves practice.
  store.subscribe((s, prev) => {
    if (!practiceStore.get().on) return;
    if (s.screen !== "stage" && !s.fullscreen && (prev.screen === "stage" || prev.fullscreen)) void stopPractice();
    if (!s.song) endLocally();
  });
}

let suspended = false;
/** The input picker's meter was closed because the window was hidden (reopened when back). */
let meterHidden = false;
function suspendFor(hidden: boolean): void {
  // The picker's meter is a microphone too: closed while the window is hidden (APP_API 12.4,
  // "a hidden window closes it"), also when practice itself has no input (watch only).
  const p = practiceStore.get();
  if (hidden && p.panel === "picker" && p.meterDevice && !meterHidden) {
    meterHidden = true;
    void api?.micMeter?.(null).catch(() => undefined);
  } else if (!hidden && meterHidden) {
    meterHidden = false;
    if (p.panel === "picker" && p.meterDevice) void meter(p.meterDevice);
  }
  const st = practiceStore.get().state;
  if (!api?.practiceSuspendInput || !st?.active || !st.input || hidden === suspended) return;
  suspended = hidden;
  api.practiceSuspendInput(hidden).catch(() => undefined);
}

function applyState(st: PracticeState): void {
  const prev = practiceStore.get().state;
  if (prev && st.seq < prev.seq && st.song_id === prev.song_id) return;
  if (!st.active) {
    if (practiceStore.get().on && !practiceStore.get().busy) endLocally();
    return;
  }
  if (keys.setOctave(st.octave.shift, performance.now())) kick();
  const patch: Partial<PracticeUi> = { state: st };
  if (st.phase === "result" && st.last_result && !practiceStore.get().result) patch.result = st.last_result;
  if (st.music.presence === "present" && prev?.music.presence !== "present" && !practiceStore.get().musicSeen && practiceStore.get().panel === null) patch.panel = "music";
  practiceStore.set(patch);
  // The guide becomes usable after "preparing": fetch it for the lane.
  if ((st.phase === "listening" || st.phase === "watch_only") && !practiceStore.get().guide) void loadGuide();
}

function endLocally(): void {
  void api?.micMeter?.(null).catch(() => undefined);
  practiceStore.set({ on: false, busy: false, state: null, guide: null, result: null, panel: null, levels: null, meterDevice: null, musicSeen: false, error: null });
  live.clear();
  kick();
}

async function loadGuide(): Promise<void> {
  if (!api?.practiceGuide) return;
  try {
    const g = await api.practiceGuide();
    if (practiceStore.get().on) practiceStore.set({ guide: g });
    kick();
  } catch (err) {
    toast(`Practice data could not be read: ${errorMessage(err)}`, "error");
  }
}

export async function refreshHistory(): Promise<void> {
  const id = store.get().song?.summary.song_id;
  if (!api?.practiceHistory || !id) return;
  try {
    practiceStore.set({ history: await api.practiceHistory(id) });
  } catch {
    /* history is optional on screen */
  }
}

/** True when this core can do practice mode. */
export function practiceAvailable(): boolean {
  return !!api?.practiceStart;
}

/** "Practice" pressed (the plate or P): explain the microphone first where the OS will ask. */
export async function startPractice(): Promise<void> {
  const s = store.get();
  if (!api || !s.song) return;
  if (!api.practiceStart) {
    toast("This version of the app has no practice mode yet.", "error");
    return;
  }
  const player = s.player;
  keys.reset(heardOf(player), player?.key_segment ?? 0, 0);
  live.clear();
  practiceStore.set({ on: true, busy: true, state: null, guide: null, result: null, history: null, panel: null, musicSeen: false, error: null });
  let perm: MicPermission | null = null;
  try {
    perm = api.micPermission ? await api.micPermission() : null;
  } catch {
    perm = null;
  }
  practiceStore.set({ permission: perm });
  if (perm?.state === "not_determined") {
    // WO2 4.4: the app explains in its own words before the operating system asks.
    practiceStore.set({ busy: false, panel: "permission" });
    return;
  }
  await startSession(false);
}

async function startSession(watchOnly: boolean): Promise<void> {
  if (!api?.practiceStart) return;
  practiceStore.set({ busy: true });
  try {
    const st = await api.practiceStart(watchOnly ? { watch_only: true } : {});
    practiceStore.set({ busy: false });
    applyState(st);
    void refreshHistory();
  } catch (err) {
    practiceStore.set({ busy: false, on: false, error: errorMessage(err) });
    toast(`Practice could not start: ${errorMessage(err)}`, "error");
  }
}

/** The explanation's Continue: ask the OS (macOS shows its prompt), then start when it answers. */
export async function allowMicrophone(): Promise<void> {
  if (!api?.micRequestPermission) return startSession(false);
  try {
    const perm = await api.micRequestPermission();
    practiceStore.set({ permission: perm });
    if (perm.state !== "not_determined") {
      practiceStore.set({ panel: null });
      await startSession(false);
    }
    // Otherwise mic://permission brings the answer (initPractice).
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

/** Practice without a microphone: notes and keys, no marker, no grade. */
export async function watchOnly(): Promise<void> {
  practiceStore.set({ panel: null });
  const st = practiceStore.get().state;
  if (st?.active && api?.practiceStop) {
    // Already listening: start again without the microphone.
    await api.practiceStop(false).catch(() => undefined);
  }
  await startSession(true);
}

/** Leaves practice mode (the microphone closes; a long enough take is graded and saved). */
export async function stopPractice(): Promise<void> {
  if (!practiceStore.get().on) return;
  practiceStore.set({ busy: true });
  try {
    if (api?.practiceStop && practiceStore.get().state?.active) await api.practiceStop(true);
  } catch (err) {
    toast(errorMessage(err), "error");
  }
  endLocally();
}

export function togglePractice(): void {
  if (practiceStore.get().on) void stopPractice();
  else void startPractice();
}

/** "Finish": grade what was sung now (the song pauses). */
export async function finishTake(): Promise<void> {
  if (!api?.practiceFinish) return;
  clock.hold(performance.now());
  try {
    const r = await api.practiceFinish();
    if (r.result) practiceStore.set({ result: r.result });
    else toast("Nothing has been sung yet, so there is nothing to grade.");
    void refreshHistory();
  } catch (err) {
    clock.release();
    toast(errorMessage(err), "error");
  }
}

/** "Sing again": from the start, a new take. */
export async function singAgain(): Promise<void> {
  if (!api?.practiceAgain) return;
  live.clear();
  practiceStore.set({ result: null });
  clock.release();
  try {
    applyState(await api.practiceAgain());
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

/** Back from the result screen to the lane (the take stays graded). */
export function closeResult(): void {
  practiceStore.set({ result: null });
}

export async function setOctave(mode: OctaveMode, shift?: number): Promise<void> {
  const id = store.get().song?.summary.song_id;
  if (!api?.practiceSongSet || !id) return;
  try {
    applyState(await api.practiceSongSet(id, { octave: { mode, ...(shift !== undefined ? { shift } : {}) } }));
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

export async function setStrictness(strictness: Strictness | null): Promise<void> {
  const id = store.get().song?.summary.song_id;
  if (!api?.practiceSongSet || !id) return;
  try {
    applyState(await api.practiceSongSet(id, { strictness }));
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

/** "The guide is wrong here" on a lyric line, or its undo (saved in the song). */
export async function excludeLine(line: number | null, start: number, end: number, excluded: boolean): Promise<void> {
  const id = store.get().song?.summary.song_id;
  if (!api?.practiceExclude || !id) return;
  try {
    const r = await api.practiceExclude(id, { line, start, end }, excluded);
    const st = practiceStore.get().state;
    if (st) practiceStore.set({ state: { ...st, excluded: r.excluded } });
    // The guide's graded flags change: fetch it again.
    void loadGuide();
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

// --- the input picker (APP_API 12.4) -------------------------------------------------------

export async function refreshMics(): Promise<void> {
  if (!api?.micList) return;
  try {
    const mics = await api.micList();
    practiceStore.set({ mics });
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

export async function openPicker(): Promise<void> {
  practiceStore.set({ panel: "picker", levels: null });
  try {
    const perm = api?.micPermission ? await api.micPermission() : null;
    practiceStore.set({ permission: perm });
  } catch {
    /* shown as unknown */
  }
  await refreshMics();
  const m = practiceStore.get().mics;
  const dev = m?.inputs.find((i) => i.id === m.chosen.device_id) ?? m?.inputs.find((i) => i.id === m?.automatic) ?? null;
  if (dev && practiceStore.get().permission?.state !== "denied") await meter(dev.id);
}

/** Meters one device in the picker (only while the picker is open). */
export async function meter(deviceId: string | null): Promise<void> {
  practiceStore.set({ meterDevice: deviceId, levels: null });
  if (!api?.micMeter) return;
  // While practice listens, the session's own meters (mic://levels) are shown for its device.
  const st = practiceStore.get().state;
  if (st?.input && st.input.device.id === deviceId) return;
  try {
    const r = await api.micMeter(deviceId);
    if (r.error) toast(r.error.message, "error");
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

export async function closePicker(): Promise<void> {
  practiceStore.set({ panel: null, levels: null, meterDevice: null });
  await api?.micMeter?.(null).catch(() => undefined);
}

/** "Use this input": saved for this computer; a listening session reopens on it at once. */
export async function chooseInput(deviceId: string | null, deviceName: string | null, channel: number | null): Promise<void> {
  if (!api?.micChoose) return;
  try {
    await api.micChoose({ device_id: deviceId, device_name: deviceName, channel });
    await closePicker();
    const st = practiceStore.get().state;
    if (st?.phase === "watch_only" && st.watch_only?.code !== "chosen" && api.practiceStop) {
      // Watch only because there was no usable input: try again with the new one.
      await api.practiceStop(false).catch(() => undefined);
      await startSession(false);
    }
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

export async function openMicSettings(): Promise<void> {
  await api?.micOpenSettings?.().catch((err: unknown) => toast(errorMessage(err), "error"));
}

export function showPanel(panel: PracticePanel): void {
  practiceStore.set({ panel, ...(panel === null && practiceStore.get().panel === "music" ? { musicSeen: true } : {}) });
}

/** Manual timing offset for this computer and input (-500..500 ms). */
export async function setLatency(ms: number): Promise<void> {
  if (!api?.practiceLatencySet) return;
  try {
    await api.practiceLatencySet(ms);
  } catch (err) {
    toast(errorMessage(err), "error");
  }
}

// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode on the singing screen (WO2 Stage 4: 4.4 input side, 4.5, 4.6, 4.7), on the
// browser mock that implements docs/APP_API.md section 12: the Practice plate and P, the
// ordinary screen unchanged with practice off, the explanation before the OS asks, watch
// only, the Windows privacy switch, the input picker (kinds, Bluetooth warning, a meter per
// channel, the voice channel), music in the input, practice data not ready, the result
// screen (overall, grade, graded X of Y, weak lines, tendency, key and octave, history,
// Sing again, "guide wrong here"), the key HEARD (not the dial) moving the lane, and the
// microphone closing when the singing screen is left.
import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { App, handleShortcut } from "../App";
import { MockApi, type MockOptions } from "../api/mock";
import { go, store } from "../state/app";
import { keys, practiceStore } from "../state/practice";
import { keyAt } from "./pitch";
import { PracticeResultScreen, tendencyText, WEAK_LINE } from "./Result";

const root = document.createElement("div");
document.body.appendChild(root);
let current: MockApi | null = null;
afterEach(() => {
  render(null, root);
  current?.dispose();
  current = null;
});

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 8000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const button = (name: RegExp) => qa("button").find((b) => name.test(b.textContent ?? "") || name.test(b.getAttribute("aria-label") ?? ""));
const phase = () => practiceStore.get().state?.phase;

const TWINKLE = "Jane Taylor - Twinkle, Twinkle, Little Star.jksong";
const FRERE = "Traditional - Frère Jacques.jksong";

/** The app on the mock with a song loaded on the singing screen. */
async function open(options: Partial<MockOptions> = {}, songId = TWINKLE): Promise<MockApi> {
  render(null, root);
  store.set({ ready: false, info: null, settings: null, library: null, deleted: null, screen: "library", song: null, songSettings: null, player: null, modal: false, toasts: [], fullscreen: false, overlay: null, guideBatch: null });
  practiceStore.set({ on: false, busy: false, state: null, guide: null, result: null, history: null, panel: null, mics: null, meterDevice: null, levels: null, permission: null, musicSeen: false, error: null });
  current?.dispose();
  const api = new MockApi({ latencyMs: 0, ...options });
  current = api;
  render(<App api={api} scriptStartMs={0} />, root);
  await until(() => store.get().ready && !!store.get().library);
  await api.songLoad(songId).then((d) => store.set({ song: d, songSettings: d.settings, screen: "stage" }));
  await until(() => !!q(".stage__practice"));
  return api;
}

async function startViaPlate(): Promise<void> {
  q<HTMLButtonElement>(".stage__practice")!.click();
  await until(() => practiceStore.get().on && !practiceStore.get().busy);
}

describe("practice mode on the singing screen (mock core)", () => {
  it("is off by default: the ordinary singing screen is unchanged; the plate and P turn it on and off", async () => {
    await open();
    expect(q(".stage__lane")).not.toBeNull();
    expect(q(".practice")).toBeNull();
    expect(q(".stage__practice")!.getAttribute("aria-pressed")).toBe("false");
    await startViaPlate();
    await until(() => phase() === "listening" && !!practiceStore.get().guide);
    expect(q(".practice")).not.toBeNull();
    expect(q(".stage__lane")).toBeNull();
    expect(q(".stage__practice")!.getAttribute("aria-pressed")).toBe("true");
    // The guide the lane draws: the demo melody, graded rule applied.
    const g = practiceStore.get().guide!;
    expect(g.notes.length).toBeGreaterThan(100);
    expect(g.notes.filter((n) => !n.graded).length).toBeGreaterThan(0);
    // P (the shortcut) leaves practice; the microphone side is off again.
    handleShortcut(new KeyboardEvent("keydown", { key: "p" }));
    await until(() => !practiceStore.get().on);
    expect(q(".practice")).toBeNull();
    expect((await current!.practiceState()).active).toBe(false);
    handleShortcut(new KeyboardEvent("keydown", { key: "P" }));
    await until(() => phase() === "listening");
  });

  it("transport, key dial and mixer keep working in practice; leaving the singing screen closes the microphone", async () => {
    const api = await open();
    await startViaPlate();
    await until(() => phase() === "listening");
    await api.playerPlay();
    await until(() => store.get().player?.state === "playing");
    handleShortcut(new KeyboardEvent("keydown", { key: "]" }));
    await until(() => store.get().songSettings?.semitones === 3);
    expect(button(/^Mixer$/)).toBeTruthy();
    go("library");
    await until(() => !practiceStore.get().on);
    expect((await api.practiceState()).active).toBe(false);
  });

  it("macOS: explains in the app's own words BEFORE the system asks; Continue asks, then listens", async () => {
    const api = await open({ platform: "macos" });
    let asked = 0;
    const orig = api.micRequestPermission.bind(api);
    api.micRequestPermission = () => {
      asked++;
      return orig();
    };
    await startViaPlate();
    await until(() => !!q(".practice-panel"));
    expect(q(".practice-panel")!.textContent).toContain("never records, saves or sends your voice");
    expect(asked).toBe(0);
    button(/^Continue$/)!.click();
    await until(() => phase() === "listening", 4000);
    expect(asked).toBe(1);
  });

  it("watch only: no microphone gives notes and keys, no marker, no grade, and says why", async () => {
    await open({ mic: "none" });
    await startViaPlate();
    await until(() => phase() === "watch_only" && !!q(".practice-note"));
    expect(practiceStore.get().state!.watch_only!.code).toBe("no_device");
    expect(q(".practice-note")!.textContent).toContain("No microphone found");
    expect(q(".practice-note")!.textContent).toContain("nothing is graded");
    expect(q(".practice-chip--watch")).not.toBeNull();
  });

  it("Windows: a microphone privacy switch that is off is named, with the steps and the settings button", async () => {
    await open({ micPrivacyOff: true });
    await startViaPlate();
    await until(() => phase() === "watch_only" && !!q(".practice-note"));
    expect(q(".practice-note")!.textContent).toContain("Windows' microphone privacy switch is off");
    expect(q(".practice-note")!.textContent).toContain("Let desktop apps access your microphone");
    expect(button(/Open the privacy settings/)).toBeTruthy();
  });

  it("the input picker: every input with its kind, the Bluetooth warning, a meter per channel, and the voice channel saved", async () => {
    const api = await open();
    await startViaPlate();
    await until(() => phase() === "listening");
    button(/^Input…$/)!.click();
    await until(() => qa(".practice-picker__device").length === 3);
    const text = q(".practice-picker")!.textContent!;
    expect(text).toContain("USB");
    expect(text).toContain("Built-in");
    expect(text).toContain("Bluetooth");
    expect(text).toContain("Recommended");
    expect(text).toContain("low-quality mono");
    // Choose the mixer: its two channels are metered separately (never added together).
    const mixer = qa(".practice-picker__device").find((d) => /USB Audio Mixer/.test(d.textContent ?? ""))!;
    mixer.querySelector("input")!.click();
    await until(() => qa(".practice-picker__channel").length === 2);
    expect(qa(".practice-meter").length).toBe(2);
    expect(q(".practice-picker__channels")!.textContent).toContain("Voice?");
    // The built-in microphone is too quiet: plain advice, the app never touches the gain.
    let chosen: unknown = null;
    const orig = api.micChoose.bind(api);
    api.micChoose = (c) => {
      chosen = c;
      return orig(c);
    };
    qa(".practice-picker__channel")[1].querySelector("input")!.click();
    await wait();
    button(/^Use this input$/)!.click();
    await until(() => chosen !== null);
    expect(chosen).toMatchObject({ device_id: "wasapi:in:usb-mixer", channel: 1 });
    await until(() => practiceStore.get().panel === null);
  });

  it("music coming back through the mixer: a panel says so in plain words with the steps; the result is marked", async () => {
    await open({ music: "marginal", speed: 16 });
    await startViaPlate();
    await until(() => phase() === "listening");
    await current!.playerPlay();
    await until(() => practiceStore.get().panel === "music", 9000);
    const t = q(".practice-panel")!.textContent!;
    expect(t).toContain("LOOPBACK");
    expect(t).toContain("USB");
    expect(t).toContain("less certain");
    button(/^Got it$/)!.click();
    await until(() => !!q(".practice-chip--warn"));
    await until(() => !!q(".practice-result"), 20000);
    expect(practiceStore.get().result!.trust).toContain("removal_marginal");
    expect(q(".practice-result__trust")!.textContent).toContain("less certain");
  }, 30000);

  it("the result screen: overall, grade, graded X of Y, tendency, key and octave, sung share, weak lines, history, Sing again", async () => {
    const api = await open({ speed: 16 });
    await startViaPlate();
    await until(() => phase() === "listening");
    await api.playerPlay();
    await until(() => !!q(".practice-result"), 20000);
    const r = practiceStore.get().result!;
    expect(r.overall).not.toBeNull();
    expect(r.grade).toMatch(/^[A-DF][+]?$/);
    expect(r.graded).toBeLessThanOrEqual(r.total);
    const scr = q(".practice-result")!.textContent!;
    expect(scr).toContain(`${Math.round(r.overall!)}%`);
    expect(scr).toContain(`on ${r.graded} of ${r.total} notes`);
    expect(scr).toContain("B major (+2)");
    expect(scr).toContain("as written");
    expect(scr).toContain("100 % of the song");
    // The input lost part way (unplugged): the "Sung" row says so too, not only the notes.
    const box = document.createElement("div");
    render(<PracticeResultScreen result={{ ...r, trust: [...r.trust, "input_lost"] }} song={store.get().song!} />, box);
    expect(box.textContent).toContain("100 % of the song, but the input stopped part way");
    render(null, box);
    // Weak lines are obvious: a class, the word "weak" (never colour alone).
    const weak = r.lines.rows.filter((x) => x[2] !== null && x[2] < WEAK_LINE).length;
    expect(qa(".practice-result__row--weak").length).toBe(weak);
    expect(qa(".practice-result__weak").length).toBe(weak);
    // History: the song had a best and three recent; this take is added.
    await until(() => (practiceStore.get().history?.recent.length ?? 0) === 4);
    expect(q(".practice-result__history")!.textContent).toContain("Best");
    // "The guide is wrong here" on a line is saved with the song.
    qa(".practice-result__exclude")[0].click();
    await until(() => (practiceStore.get().state?.excluded.length ?? 0) === 1);
    expect((await api.practiceState()).excluded.length).toBe(1);
    // Sing again: a new take from the start.
    q<HTMLButtonElement>(".practice-result__again")!.click();
    await until(() => !q(".practice-result") && phase() === "listening");
    expect(store.get().player?.state).toBe("playing");
  }, 30000);

  it("practice data not ready: added on the spot with progress, then the lane; a failure says why with Try again", async () => {
    await open({}, FRERE);
    await startViaPlate();
    await until(() => phase() === "listening" && !!practiceStore.get().guide, 6000);
    expect(practiceStore.get().guide!.notes.length).toBeGreaterThan(10);
    // Made but not savable (the stick is full): used from memory for this session.
    await open({ guideFailIds: [FRERE] }, FRERE);
    await startViaPlate();
    await until(() => phase() === "listening" && !!practiceStore.get().guide, 6000);
    expect(practiceStore.get().state!.guide.in_memory).toBe(true);
    // Cannot be made at all: not ready, says why, offers Try again.
    await open({ guideHardFailIds: [FRERE] }, FRERE);
    await startViaPlate();
    await until(() => phase() === "not_ready", 6000);
    expect(q(".practice-panel")!.textContent).toContain("could not be read");
    expect(button(/Try again/)).toBeTruthy();
  }, 20000);

  it("the lane follows the key HEARD: a key change is pending until the engine hears it, at a song time", async () => {
    const api = await open();
    await startViaPlate();
    await until(() => phase() === "listening");
    await api.playerPlay();
    await until(() => store.get().player?.state === "playing");
    await wait(100);
    handleShortcut(new KeyboardEvent("keydown", { key: "[" })); // +2 -> +1 asked
    await until(() => keys.view().changes.length > 0, 3000);
    const c = keys.view().changes[0];
    // The dial says +1 at once; the lane keeps +2 until the change's song time (~0.28 s later).
    expect(store.get().songSettings?.semitones).toBe(1);
    expect(c.semitones).toBe(1);
    expect(c.from).toBe(2);
    expect(keyAt(keys.view(), c.at - 0.01)).toBe(2);
    expect(keyAt(keys.view(), c.at)).toBe(1);
    await until(() => (store.get().player?.key_heard ?? 2) === 1, 3000);
  });

  it("tendency words", () => {
    expect(tendencyText(null)).toContain("not enough");
    expect(tendencyText(3)).toBe("neither sharp nor flat on average");
    expect(tendencyText(-12)).toBe("a little flat (about 12 cents)");
    expect(tendencyText(30)).toBe("sharp (about 30 cents)");
  });
});

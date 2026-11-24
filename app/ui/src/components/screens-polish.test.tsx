// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// The pre-publication review of the screens (2026-11-24), on the mock: the library's
// search names its shortcut the platform's way, the confirmation names a song title
// first, the preset is not mistaken for "clean lyrics", the empty library and the empty
// Sing screen have a proper centred message, the mixer on the Sing screen is a drawer
// whose rarely used parts fold away, the shortcuts list has one row for the key keys,
// Settings keeps the user-agent string folded, and the self-check speaks plain words
// (the measurements stay in the report and under "Technical details").
import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi, type MockOptions } from "../api/mock";
import type { SelfCheckItem } from "../api/types";
import { loadSong, setOverlay, store } from "../state/app";
import { confirmName } from "./Library";
import { OR, plainCheck, SHORTCUTS } from "./Overlays";

const root = document.createElement("div");
document.body.appendChild(root);
let current: MockApi | null = null;
afterEach(() => {
  render(null, root);
  current?.dispose();
  current = null;
});

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 4000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));

async function open(options: Partial<MockOptions> = {}): Promise<MockApi> {
  render(null, root);
  store.set({ ready: false, info: null, settings: null, library: null, deleted: null, screen: "library", song: null, songSettings: null, player: null, modal: false, toasts: [], fullscreen: false, overlay: null });
  current?.dispose();
  const api = new MockApi({ latencyMs: 0, ...options });
  current = api;
  render(<App api={api} scriptStartMs={0} />, root);
  await until(() => store.get().ready && !!store.get().library && !!q(".library"));
  await wait();
  return api;
}

describe("confirmation wording", () => {
  it("names the title first and the artist after it, without nested brackets", () => {
    expect(confirmName({ title: "Frère Jacques", artist: "Traditional" })).toBe("“Frère Jacques” (Traditional)");
    expect(confirmName({ title: "Twinkle, Twinkle, Little Star", artist: "Jane Taylor (1806)" })).toBe("“Twinkle, Twinkle, Little Star” (Jane Taylor, 1806)");
    expect(confirmName({ title: "", artist: "" })).toBe("“Untitled”");
  });
});

describe("library", () => {
  it("search: a short placeholder; what it searches and Ctrl+F in the tooltip (Windows)", async () => {
    await open();
    const s = q<HTMLInputElement>("#library-search")!;
    expect(s.placeholder).toBe("Search songs");
    expect(s.title).toBe("Search the library by title, artist or album (Ctrl+F)");
    expect(s.getAttribute("aria-keyshortcuts")).toBe("Control+F");
  });

  it("search: ⌘F on the Mac", async () => {
    await open({ platform: "macos" });
    const s = q<HTMLInputElement>("#library-search")!;
    expect(s.title).toContain("(⌘F)");
    expect(s.title).not.toContain("Ctrl");
    expect(s.getAttribute("aria-keyshortcuts")).toBe("Meta+F");
  });

  it("the Studio preset is left off the strip, and the plain card says 'Made with Clean'", async () => {
    await open();
    const row = qa(".library__strip").find((e) => /Row, Row, Row Your Boat/.test(e.textContent ?? ""))!;
    expect(row.querySelector(".jk-strip-note")!.textContent).toBe("Lyrics not checked · Practice ready");
    await open({ skin: false });
    const card = qa(".title-strip__row").find((e) => /Row, Row, Row Your Boat/.test(e.textContent ?? ""))!;
    const badges = Array.from(card.querySelectorAll(".title-strip__badge")).map((b) => b.textContent);
    expect(badges).toContain("Made with Clean");
    expect(badges).not.toContain("Clean");
  });

  for (const skin of [true, false]) {
    it(`no songs: a centred heading, the explanation and the way to the Studio; no search or sort (${skin ? "jukebox" : "plain"} look)`, async () => {
      await open({ empty: true, skin });
      const empty = q(".library__empty--none")!;
      expect(empty.querySelector("h2")!.textContent).toBe("No songs yet");
      expect(empty.textContent).toContain(".jksong");
      expect(q("#library-search")).toBeNull();
      expect(q(".library__sort")).toBeNull();
      expect(q(".library__select")).toBeNull();
      // Rescan and Deleted songs stay: songs can be added to the folder or restored.
      expect(q(".library__rescan")).toBeTruthy();
      expect(q(".library__show-deleted")).toBeTruthy();
      empty.querySelector<HTMLButtonElement>(".library__empty-studio")!.click();
      await until(() => store.get().screen === "studio");
    });
  }

  it("no Studio button where this computer cannot run the Studio", async () => {
    await open({ empty: true, studio: "off" });
    expect(q(".library__empty--none")).toBeTruthy();
    expect(q(".library__empty-studio")).toBeNull();
  });
});

describe("Sing screen", () => {
  it("no song: heading, line and button in one centred group", async () => {
    await open();
    store.set({ screen: "stage" });
    await until(() => !!q(".stage--empty"));
    const welcome = q(".stage--empty .stage__welcome")!;
    expect(welcome.querySelector("h2")!.textContent).toBe("No song loaded");
    expect(welcome.querySelector(".stage__empty")!.textContent).toBe("Pick a song in the library to start singing.");
    expect(welcome.querySelector("button")!.textContent).toBe("Open the library");
  });

  it("the mixer is a drawer there: levels in view, output device and export folded; in Settings it is whole", async () => {
    await open();
    const id = store.get().library!.songs.find((s) => !s.error)!.song_id;
    await loadSong(id);
    store.set({ screen: "stage" });
    await until(() => !!q(".stage-screen__tools button"));
    q<HTMLButtonElement>(".stage-screen__tools button")!.click();
    await until(() => !!q(".stage-screen > .mixer"));
    const mixer = q(".stage-screen > .mixer")!;
    expect(mixer.classList.contains("mixer--drawer")).toBe(true);
    const folds = Array.from(mixer.querySelectorAll<HTMLDetailsElement>("details.mixer__fold"));
    expect(folds.map((f) => f.querySelector("summary")!.textContent)).toEqual(["Output device", "Export backing track"]);
    expect(folds.every((f) => !f.open)).toBe(true);
    // The levels are not folded.
    expect(mixer.querySelectorAll(".mixer__group input[type=range]").length).toBeGreaterThanOrEqual(5);
    store.set({ screen: "settings" });
    await until(() => !!q(".settings-screen .mixer"));
    const whole = q(".settings-screen .mixer")!;
    expect(whole.classList.contains("mixer--drawer")).toBe(false);
    expect(whole.querySelector("details")).toBeNull();
    expect(whole.querySelector(".mixer__export h3")!.textContent).toBe("Export backing track");
  });
});

describe("Settings wording", () => {
  it("plain hints, and the web engine string folded under Details", async () => {
    await open();
    store.set({ screen: "settings" });
    await until(() => !!q(".settings"));
    const t = q(".settings")!.textContent ?? "";
    expect(t).toContain("Off: a plain, flat layout.");
    expect(t).toContain("The size on a 1440 × 900 window; it scales with the window.");
    expect(t).toContain("so a performance without a microphone still shows a singer on the melody");
    expect(t).not.toContain("looks right on video");
    const details = q<HTMLDetailsElement>(".settings__details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")!.textContent).toBe("Details");
    expect(details.textContent).toContain("Web engine");
  });
});

describe("keyboard shortcuts", () => {
  it("one row for the key keys: [ ] or − +", async () => {
    const rows = SHORTCUTS.filter(([, what]) => what === "Key down / up one semitone");
    expect(rows).toHaveLength(1);
    expect(rows[0][0]).toEqual(["[", "]", OR, "−", "+"]);
    await open();
    setOverlay("help");
    await until(() => !!q(".help__table"));
    const th = Array.from(root.querySelectorAll(".help__table tr")).find((tr) => /Key down/.test(tr.textContent ?? ""))!.querySelector("th")!;
    expect(th.textContent).toBe("[ / ] or − / +");
    expect(Array.from(th.querySelectorAll("kbd")).map((k) => k.textContent)).toEqual(["[", "]", "−", "+"]);
  });
});

describe("self-check in plain words", () => {
  const item = (name: string, ok: boolean, detail: string): SelfCheckItem => ({ name, ok, detail });

  it("a passed check says what it means; a failed or skipped one shows its own words", () => {
    expect(plainCheck(item("Bundled test song", true, "Loaded tiny.jksong and rendered 4.0 s offline; output matches the reference (max diff 0)."))).toBe("The test song plays back exactly as it should.");
    expect(plainCheck(item("Practice lane", true, "600 frames at 1632x389 (x1): 0.45 ms average"))).toBe("This computer draws the practice notes smoothly.");
    expect(plainCheck(item("Memory", true, "total 8 GB, available 300 MB. Less than 512 MB is free: close other apps"))).toMatch(/^Memory is low/);
    expect(plainCheck(item("Recorder", false, "macOS has not allowed Screen Recording yet."))).toBeNull();
    expect(plainCheck(item("Microphone capture", true, "Skipped: the microphone permission is not determined."))).toBeNull();
    expect(plainCheck(item("Something new", true, "whatever"))).toBeNull();
  });

  it("the overlay lists plain words and keeps the numbers under Technical details", async () => {
    await open();
    setOverlay("selfcheck");
    await until(() => !!q(".selfcheck__items"), 6000);
    const list = q(".selfcheck__items")!.textContent ?? "";
    expect(list).not.toMatch(/max diff|frames at|dBFS|Mozilla/);
    const details = q<HTMLDetailsElement>(".selfcheck__details")!;
    expect(details.open).toBe(false);
    expect(details.textContent).toContain("max diff 0");
  });
});

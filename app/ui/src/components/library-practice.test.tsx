// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// Work Order 2, Stage 2: practice data in the Library (docs/APP_API.md 12.3). On the mock,
// in the jukebox look and the plain look: each song says whether practice is ready, a song
// without practice data offers "Add practice data" (with progress, then ready), the toolbar
// offers "Add practice data to all songs" with "n of N" progress and Stop, a failure says
// why and offers to try again, and guide://status patches the list without a rescan.
import { render } from "preact";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App";
import { MockApi, type MockOptions, readyGuide } from "../api/mock";
import { applyGuideStatus, guideNeedsAdding, guideUsable, store } from "../state/app";
import { practiceBadge } from "./Library";
import { Job } from "./Studio";

const root = document.createElement("div");
document.body.appendChild(root);
let current: MockApi | null = null;
afterEach(() => {
  render(null, root);
  current?.dispose();
  current = null;
});

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
async function until(cond: () => boolean, ms = 6000): Promise<void> {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("timed out waiting for the UI");
    await wait(10);
  }
}
const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T>(sel);
const qa = (sel: string) => Array.from(root.querySelectorAll<HTMLElement>(sel));
const strip = (title: RegExp) => qa(".library__strip").find((e) => title.test(e.textContent ?? ""));
const row = (title: RegExp) => qa(".title-strip__row").find((e) => title.test(e.textContent ?? ""));
const guideOf = (id: string) => store.get().library?.songs.find((s) => s.song_id === id)?.guide;

async function open(options: Partial<MockOptions> = {}): Promise<MockApi> {
  render(null, root);
  store.set({ ready: false, info: null, settings: null, library: null, deleted: null, screen: "library", song: null, songSettings: null, player: null, modal: false, toasts: [], fullscreen: false, overlay: null, guideBatch: null });
  current?.dispose();
  const api = new MockApi({ latencyMs: 0, ...options });
  current = api;
  render(<App api={api} scriptStartMs={0} />, root);
  await until(() => store.get().ready && (qa(".library__strip").length > 0 || qa(".title-strip").length > 0));
  await wait();
  return api;
}

const FRERE = "Traditional - Frère Jacques.jksong";
const TWINKLE = "Jane Taylor - Twinkle, Twinkle, Little Star.jksong";

describe("practice badge words", () => {
  it("names each state in plain words and never relies on colour alone", () => {
    expect(practiceBadge(undefined)).toBeNull();
    expect(practiceBadge(readyGuide())?.text).toBe("Practice ready");
    expect(practiceBadge(readyGuide())?.title).toContain("Graded on 96 of 120 notes");
    const none = { ...readyGuide(), state: "none" as const, counts: null, reason: "Practice data has not been added to this song yet." };
    expect(practiceBadge(none)).toMatchObject({ text: "Practice not ready", kind: "not-ready" });
    expect(practiceBadge({ ...none, state: "building", progress: 0.42 })?.text).toBe("Adding practice data 42 %");
    expect(practiceBadge({ ...none, state: "failed", error_code: "guide_no_lead" })?.text).toBe("No practice (no lead vocal)");
    expect(practiceBadge({ ...none, state: "failed", error_code: "guide_read_only", in_memory: true })?.text).toBe("Practice ready (this session)");
    expect(guideNeedsAdding(none) && !guideUsable(none)).toBe(true);
    expect(guideNeedsAdding({ ...none, state: "failed", error_code: "guide_no_lead" })).toBe(false);
    expect(guideUsable({ ...none, state: "failed", in_memory: true })).toBe(true);
  });
});

for (const skin of [true, false]) {
  const look = skin ? "jukebox look (skin on)" : "plain look";
  const card = (t: RegExp) => (skin ? strip(t) : row(t));
  describe(`practice data in the Library, ${look}`, () => {
    it("says which songs are ready and adds practice data to one song", async () => {
      await open({ skin });
      // Twinkle and Row have practice data in the mock; Frère Jacques does not.
      expect(card(/Twinkle, Twinkle/)!.getAttribute("data-practice")).toBe("ready");
      expect(card(/Frère Jacques/)!.getAttribute("data-practice")).toBe("not-ready");
      expect(card(/Twinkle, Twinkle/)!.textContent).toContain("Practice ready");
      expect(card(/Frère Jacques/)!.textContent).toContain("Practice not ready");
      // Damaged files say nothing about practice and offer nothing.
      expect(card(/interrupted copy/)!.querySelector(".library__practice-add")).toBeNull();
      // Only songs without practice data offer the control.
      expect(card(/Twinkle, Twinkle/)!.querySelector(".library__practice-add")).toBeNull();
      const add = card(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__practice-add")!;
      expect(add.getAttribute("aria-label")).toBe("Add practice data to Traditional - Frère Jacques");
      add.click();
      await until(() => guideOf(FRERE)?.state === "building");
      await until(() => /Adding practice data \d+ %/.test(card(/Frère Jacques/)!.textContent ?? ""));
      await until(() => guideOf(FRERE)?.state === "ready");
      expect(card(/Frère Jacques/)!.getAttribute("data-practice")).toBe("ready");
      expect(card(/Frère Jacques/)!.querySelector(".library__practice-add")).toBeNull();
    });

    it("adds practice data to all songs with progress, then hides the control", async () => {
      await open({ skin });
      const all = q<HTMLButtonElement>(".library__practice-all")!;
      expect(all.getAttribute("aria-label") ?? all.textContent).toMatch(/Add practice data to all songs/);
      all.click();
      await until(() => !!q(".library__practice-progress"));
      expect(q(".library__practice-progress")!.textContent).toMatch(/Adding practice data: \d of 2/);
      expect(q(".library__practice-stop")).not.toBeNull();
      await until(() => store.get().guideBatch === null && guideOf(FRERE)?.state === "ready");
      expect(guideOf("Sarah Josepha Hale - Mary Had a Little Lamb.jksong")?.state).toBe("ready");
      await until(() => q(".library__practice-all") === null);
      expect(q(".library__practice-progress")).toBeNull();
      expect(qa("[data-practice='not-ready']")).toHaveLength(0);
    });

    it("Stop leaves the songs as they were and says it was cancelled", async () => {
      await open({ skin });
      q<HTMLButtonElement>(".library__practice-all")!.click();
      await until(() => !!q(".library__practice-stop"));
      q<HTMLButtonElement>(".library__practice-stop")!.click();
      await until(() => store.get().guideBatch === null);
      const states = [guideOf(FRERE)?.state, guideOf("Sarah Josepha Hale - Mary Had a Little Lamb.jksong")?.state];
      expect(states.every((s) => s === "failed" || s === "ready")).toBe(true);
      expect(states).toContain("failed");
    });

    it("a failure says why and offers to try again", async () => {
      await open({ skin, guideFailIds: [FRERE] });
      card(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__practice-add")!.click();
      await until(() => guideOf(FRERE)?.state === "failed");
      const retry = card(/Frère Jacques/)!.querySelector<HTMLButtonElement>(".library__practice-add")!;
      expect(retry.getAttribute("aria-label")).toBe("Try again: add practice data to Traditional - Frère Jacques");
      expect(retry.getAttribute("title")).toContain("the drive is full");
      // Built but not saved: usable for this session.
      expect(card(/Frère Jacques/)!.textContent).toContain("Practice ready (this session)");
    });
  });
}

describe("guide://status events", () => {
  it("patch the library and the loaded song without a rescan", async () => {
    const api = await open({ skin: false });
    const before = store.get().library;
    applyGuideStatus(FRERE, { ...readyGuide(), state: "building", progress: 0.5 });
    expect(guideOf(FRERE)?.state).toBe("building");
    expect(store.get().library?.songs.length).toBe(before?.songs.length);
    await until(() => (row(/Frère Jacques/)!.textContent ?? "").includes("Adding practice data 50 %"));
    // The mock's own events take the same path.
    await api.guideBuild(TWINKLE, true);
    await until(() => guideOf(TWINKLE)?.state === "ready" && !!guideOf(TWINKLE)?.counts);
  });
});

describe("the Studio job's practice data stage", () => {
  const base = { job_id: "j1", input_name: "song.flac", output_song_id: "A - B.jksong", error: null };
  it("shows the guide stage after Save while the app makes practice data", async () => {
    render(<ul><Job job={{ ...base, state: "running", stage: "guide", overall: 0.97, message: "Making the practice data", guide: { ...readyGuide(), state: "building", progress: 0.5 }, stage_progress: 0.5 }} /></ul>, root);
    await wait();
    expect(qa(".studio__stage").map((e) => e.textContent)).toEqual(["Read", "Separate", "Lyrics", "Align", "Key & tempo", "Save", "Practice data 50 %"]);
    expect(qa(".studio__stage--done")).toHaveLength(6);
    expect(q(".studio__stage--current")!.textContent).toBe("Practice data 50 %");
  });
  it("a failed guide never fails the job: done, with the reason and a retry", async () => {
    const guide = { ...readyGuide(), state: "failed" as const, error_code: "guide_disk_full", reason: "Practice data could not be saved: the drive is full. Free 150 MB and press Try again.", counts: null };
    render(<ul><Job job={{ ...base, state: "done", stage: null, overall: 1, message: null, guide }} /></ul>, root);
    await wait();
    expect(q(".studio__job-state")!.textContent).toBe("Done");
    expect(root.textContent).toContain("the drive is full");
    expect(q(".studio__guide-retry")).not.toBeNull();
    expect(qa(".studio__stage--done")).toHaveLength(7);
  });
});

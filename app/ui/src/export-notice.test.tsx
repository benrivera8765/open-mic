// @vitest-environment happy-dom
// SPDX-License-Identifier: GPL-3.0-or-later
// "Export backing track" never hides processing (WORK_ORDER 5): when the core had to
// lower a FLAC export to keep it from clipping, `export://done` carries
// `applied_gain_db` and the UI says so plainly, in a toast and next to the saved file
// in the mixer. A 32-bit float WAV is never scaled and gets no such note.
import { render } from "preact";
import { afterAll, describe, expect, it } from "vitest";
import { MockApi } from "./api/mock";
import { Mixer } from "./components/Mixer";
import { exportGainNotice } from "./lib/format";
import { exportBacking, init, loadSong, store } from "./state/app";

const FIVE_POINT_EIGHT =
  "The mix peaked above full scale, so the FLAC file was lowered by 5.8 dB to avoid clipping. Choose 32-bit float WAV to keep the exact level.";

describe("exportGainNotice", () => {
  it("says nothing when the file holds the exact level", () => {
    expect(exportGainNotice(0, "Exports/a.flac")).toBeNull();
    expect(exportGainNotice(-0, "Exports/a.flac")).toBeNull();
    expect(exportGainNotice(undefined, "Exports/a.flac")).toBeNull(); // older core
    expect(exportGainNotice(Number.NaN, "Exports/a.flac")).toBeNull();
    expect(exportGainNotice(0, "Exports/a.wav")).toBeNull();
  });
  it("says plainly how much a FLAC export was lowered, and how to avoid it", () => {
    expect(exportGainNotice(-5.8, "Exports/a.flac")).toBe(FIVE_POINT_EIGHT);
    expect(exportGainNotice(-5.8007, "Exports/A - B (C major).FLAC")).toBe(FIVE_POINT_EIGHT);
    expect(exportGainNotice(-12.04, "x.flac")).toMatch(/lowered by 12\.0 dB/);
  });
  it("never rounds a real cut to 0.0 dB", () => {
    expect(exportGainNotice(-0.04, "x.flac")).toMatch(/lowered by 0\.04 dB to avoid clipping/);
    expect(exportGainNotice(-0.096, "x.flac")).toMatch(/lowered by 0\.1 dB/);
    expect(exportGainNotice(-0.0004, "x.flac")).toMatch(/lowered by less than 0\.01 dB/);
  });
  it("only suggests the WAV format for a FLAC file", () => {
    const n = exportGainNotice(-3, "x.wav")!;
    expect(n).toBe("The mix peaked above full scale, so the file was lowered by 3.0 dB to avoid clipping.");
  });
});

describe("export://done from the (mock) core", () => {
  const wait = (ms = 10) => new Promise((r) => setTimeout(r, ms));
  async function until(cond: () => boolean, ms = 4000): Promise<void> {
    const t0 = Date.now();
    while (!cond()) {
      if (Date.now() - t0 > ms) throw new Error("timed out");
      await wait();
    }
  }
  // Like jk-engine: a render peaking at 1.95 (+5.8 dBFS) must be lowered by 5.8 dB to
  // fit a 24-bit FLAC. Fast mock timers so the simulated export finishes quickly.
  const api = new MockApi({ latencyMs: 0, exportPeak: 1.95, setInterval: (f, ms) => setInterval(f, Math.min(ms, 5)) });
  const root = document.createElement("div");
  document.body.appendChild(root);
  afterAll(() => render(null, root));

  async function exportAndWait(format: "flac24" | "wav32f") {
    const before = new Set(Object.keys(store.get().exports));
    await exportBacking(format);
    const id = Object.keys(store.get().exports).find((k) => !before.has(k))!;
    expect(id).toBeTruthy();
    await until(() => store.get().exports[id].path !== null);
    return store.get().exports[id];
  }

  it("the event carries applied_gain_db: below 0 for a hot FLAC export, 0 for WAV", async () => {
    await init(api);
    const song = store.get().library!.songs.find((s) => !s.error)!;
    expect(await loadSong(song.song_id)).toBe(true);
    const seen: { applied_gain_db?: number; peak?: number; path: string }[] = [];
    const off = api.on("export://done", (e) => seen.push(e));
    await exportAndWait("flac24");
    await exportAndWait("wav32f");
    off();
    expect(seen).toHaveLength(2);
    expect(seen[0].path).toMatch(/\.flac$/);
    expect(seen[0].applied_gain_db!).toBeCloseTo(-5.80, 2);
    expect(seen[0].peak).toBe(1.95);
    expect(seen[1].path).toMatch(/\.wav$/);
    expect(seen[1].applied_gain_db).toBe(0);
  });

  it("tells the user when a FLAC export was lowered (toast and mixer note)", async () => {
    const job = await exportAndWait("flac24");
    expect(job.notice).toBe(FIVE_POINT_EIGHT);
    const t = store.get().toasts.at(-1)!;
    expect(t.kind).toBe("info");
    expect(t.text).toBe(`Exported: ${job.path}. ${FIVE_POINT_EIGHT}`);
    render(<Mixer />, root);
    await wait(20);
    const notes = Array.from(root.querySelectorAll(".mixer__export-job .note--warn")).map((e) => e.textContent);
    expect(notes).toContain(FIVE_POINT_EIGHT);
  });

  it("says nothing extra for a WAV export (never scaled)", async () => {
    const job = await exportAndWait("wav32f");
    expect(job.notice).toBeNull();
    expect(store.get().toasts.at(-1)!.text).toBe(`Backing track saved: ${job.path}`);
  });
});

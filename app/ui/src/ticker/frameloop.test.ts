// SPDX-License-Identifier: GPL-3.0-or-later
// The ticker's frame loop (frameloop.ts): 60 fps cap, stops when idle, and stops while
// the native window is minimised even though the page still reports itself visible,
// which is what WebView2 does (D-97, WORK_ORDER 4.7 "pause animation when hidden").
import { describe, expect, it } from "vitest";
import { FRAME_MS, FrameLoop, type FrameLoopEnv } from "./frameloop";

/** A fake requestAnimationFrame clock and a fake page and window. */
function fakeEnv() {
  let nextId = 1;
  const pending = new Map<number, (now: number) => void>();
  const listeners = new Set<() => void>();
  const minimisedCbs = new Set<(m: boolean) => void>();
  const doc = {
    hidden: false,
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  };
  const env: FrameLoopEnv = {
    requestFrame: (cb) => {
      const id = nextId++;
      pending.set(id, cb);
      return id;
    },
    cancelFrame: (id) => pending.delete(id),
    doc: doc as unknown as FrameLoopEnv["doc"],
    onWindowMinimised: (cb) => {
      minimisedCbs.add(cb);
      return () => minimisedCbs.delete(cb);
    },
  };
  let now = 1000;
  return {
    env,
    doc,
    pending,
    minimisedCbs,
    /** Runs one display refresh (`hz`): every pending frame callback. */
    refresh(hz = 60) {
      now += 1000 / hz;
      const cbs = [...pending.values()];
      pending.clear();
      for (const cb of cbs) cb(now);
    },
    setMinimised(m: boolean) {
      for (const cb of minimisedCbs) cb(m);
    },
    setPageHidden(h: boolean) {
      doc.hidden = h;
      for (const l of listeners) l();
    },
    listeners,
  };
}

describe("ticker frame loop", () => {
  it("stops drawing while the window is minimised although the page stays 'visible' (WebView2)", () => {
    const f = fakeEnv();
    const loop = new FrameLoop(f.env);
    loop.attach();
    let draws = 0;
    loop.draw = () => {
      draws++;
      return true; // playing: keeps animating
    };
    loop.kick();
    for (let i = 0; i < 30; i++) f.refresh();
    expect(draws).toBeGreaterThanOrEqual(29);
    // Minimised: document.visibilityState stays "visible" (doc.hidden false), Tauri says minimised.
    f.setMinimised(true);
    expect(f.doc.hidden).toBe(false);
    expect(loop.hidden).toBe(true);
    expect(loop.running).toBe(false);
    expect(f.pending.size).toBe(0); // the scheduled frame was cancelled
    const before = draws;
    loop.kick(); // clock events (play position updates) must not restart it
    for (let i = 0; i < 120; i++) f.refresh();
    expect(draws).toBe(before);
    expect(f.pending.size).toBe(0);
    // Restored: drawing resumes at once.
    f.setMinimised(false);
    expect(loop.running).toBe(true);
    for (let i = 0; i < 10; i++) f.refresh();
    expect(draws).toBeGreaterThan(before + 8);
    // Unmount: no frame left, no listener left.
    loop.detach();
    expect(f.pending.size).toBe(0);
    expect(f.minimisedCbs.size).toBe(0);
    expect(f.listeners.size).toBe(0);
  });

  it("still stops when the page itself is hidden, and when nothing moves", () => {
    const f = fakeEnv();
    const loop = new FrameLoop(f.env);
    loop.attach();
    let moving = true;
    let draws = 0;
    loop.draw = () => {
      draws++;
      return moving;
    };
    loop.kick();
    f.refresh();
    f.setPageHidden(true);
    expect(loop.running).toBe(false);
    loop.kick();
    expect(loop.running).toBe(false);
    f.setPageHidden(false);
    expect(loop.running).toBe(true);
    moving = false; // paused: one last frame, then idle
    f.refresh();
    f.refresh();
    expect(loop.running).toBe(false);
    const n = draws;
    f.refresh();
    expect(draws).toBe(n);
    // Without a draw function (no layout yet) a kick schedules nothing.
    loop.draw = null;
    loop.kick();
    expect(loop.running).toBe(false);
    loop.detach();
  });

  it("caps at 60 frames per second on a 144 Hz display", () => {
    const f = fakeEnv();
    const loop = new FrameLoop(f.env);
    let draws = 0;
    loop.draw = () => {
      draws++;
      return true;
    };
    loop.kick();
    for (let i = 0; i < 144; i++) f.refresh(144); // one second
    expect(draws).toBeGreaterThanOrEqual(57);
    expect(draws).toBeLessThanOrEqual(61);
    expect(FRAME_MS).toBeCloseTo(16.667, 2);
  });
});

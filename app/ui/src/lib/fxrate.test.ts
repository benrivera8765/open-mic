// SPDX-License-Identifier: GPL-3.0-or-later
// The animation frame rate setting (D-274): fxLoop on simulated 60, 144 and 240 Hz screens.
import { afterEach, describe, expect, it, vi } from "vitest";
import { effectFps, fxFpsCap, fxLoop, setFxFps } from "./fxrate";

/** Runs fxLoop for one simulated second on a screen of `hz` and counts the frames drawn. */
function framesInASecond(fps: number, hz: number): number {
  const queue: ((t: number) => void)[] = [];
  vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
    queue.push(cb);
    return queue.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
  let drawn = 0;
  const stop = fxLoop(fps, () => drawn++);
  for (let k = 1; k <= hz; k++) {
    const now = (k * 1000) / hz;
    const due = queue.splice(0);
    for (const cb of due) cb(now);
  }
  stop();
  return drawn;
}

describe("frame rate setting", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    setFxFps(0);
  });

  it("follows a 240 Hz screen at 240 fps, a 144 Hz one at 144", () => {
    expect(framesInASecond(240, 240)).toBeGreaterThanOrEqual(238);
    expect(framesInASecond(240, 144)).toBeGreaterThanOrEqual(142);
  });

  it("keeps to a lower rate on a faster screen", () => {
    const n120 = framesInASecond(120, 240);
    expect(n120).toBeGreaterThanOrEqual(118);
    expect(n120).toBeLessThanOrEqual(121);
    const n60 = framesInASecond(60, 144);
    expect(n60).toBeGreaterThanOrEqual(58);
    expect(n60).toBeLessThanOrEqual(61);
  });

  it("is off (standard rates) by default and clamps what it is given", () => {
    expect(fxFpsCap()).toBe(0);
    expect(effectFps(24)).toBe(24);
    setFxFps(240);
    expect(effectFps(24)).toBe(240);
    setFxFps(1000);
    expect(fxFpsCap()).toBe(240);
    setFxFps(0);
    expect(effectFps(30)).toBe(30);
  });
});

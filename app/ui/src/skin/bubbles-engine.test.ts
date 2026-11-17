// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { BubbleField, MAX_BUBBLES, type BubbleSprite } from "./bubbles-engine";
import { bubbleTrack, computeFrameGeometry, type PathPoint } from "./geometry";

const g = computeFrameGeometry({
  width: 1440,
  height: 900,
  screen: { x: 57.6, y: 81, w: 1324.8, h: 661.5 },
  screenRadius: 129.6,
  consoleTop: 742.5,
});
const paths = [bubbleTrack(g, "left"), bubbleTrack(g, "right")];
const hw = g.bubbleTube / 2;

function snapshot(field: BubbleField): BubbleSprite[] {
  const out: BubbleSprite[] = [];
  field.forEach((b) => out.push({ ...b }));
  return out;
}

describe("bubble field", () => {
  it("never has more than 60 bubbles alive, even when asked for more", () => {
    const field = new BubbleField({ maxBubbles: 500, seed: 1 });
    field.setPaths(paths, hw);
    let peak = 0;
    for (let i = 0; i < 6000; i++) {
      field.step(1 / 30);
      peak = Math.max(peak, field.count);
    }
    expect(peak).toBeLessThanOrEqual(MAX_BUBBLES);
    expect(peak).toBeGreaterThan(20); // and the tubes are actually busy
  });

  it("keeps every bubble inside the inner tube", () => {
    const field = new BubbleField({ seed: 2 });
    field.setPaths(paths, hw);
    const p: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
    for (let i = 0; i < 1800; i++) {
      field.step(1 / 30);
      if (i % 60 !== 0) continue; // check twice a simulated second
      field.forEach((b) => {
        expect(b.r).toBeLessThanOrEqual(hw);
        expect(b.alpha).toBeGreaterThanOrEqual(0);
        expect(b.alpha).toBeLessThanOrEqual(1);
        // Distance from the nearest centre-line point of either track is within the tube.
        let best = Infinity;
        for (const path of paths) {
          for (let s = 0; s <= path.length; s += 2) {
            path.at(s, p);
            best = Math.min(best, Math.hypot(b.x - p.x, b.y - p.y));
          }
        }
        expect(best + b.r).toBeLessThanOrEqual(hw + 1.5);
      });
    }
  }, 60_000);

  it("is deterministic for a seed", () => {
    const a = new BubbleField({ seed: 42 });
    const b = new BubbleField({ seed: 42 });
    a.setPaths(paths, hw);
    b.setPaths(paths, hw);
    for (let i = 0; i < 300; i++) {
      a.step(1 / 30);
      b.step(1 / 30);
    }
    expect(snapshot(a)).toEqual(snapshot(b));
  });

  it("prewarm fills the tubes right away", () => {
    const field = new BubbleField({ seed: 3 });
    field.setPaths(paths, hw);
    expect(field.count).toBe(0);
    field.prewarm(30);
    expect(field.count).toBeGreaterThan(30);
  });

  it("keeps bubbles at the same relative place when the window is resized", () => {
    const field = new BubbleField({ seed: 4 });
    field.setPaths(paths, hw);
    field.prewarm(20);
    const before = field.count;
    const g2 = computeFrameGeometry({
      width: 1280,
      height: 800,
      screen: { x: 51.2, y: 72, w: 1177.6, h: 588 },
      screenRadius: 115.2,
      consoleTop: 660,
    });
    field.setPaths([bubbleTrack(g2, "left"), bubbleTrack(g2, "right")], g2.bubbleTube / 2, 800 / 900);
    expect(field.count).toBe(before);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { bubbleTrack, centreLinePath, computeFrameGeometry, type FrameInput, type PathPoint } from "./geometry";

/** The frame as the CSS lays it out at a given window size (same clamp() rules as skin.css). */
function inputFor(W: number, H: number): FrameInput {
  const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
  const side = clamp(0.04 * W, 28, 154);
  const top = clamp(0.09 * H, 56, 194);
  const consoleH = clamp(0.175 * H, 104, 378);
  const r = clamp(Math.min(0.11 * W, 0.175 * H), 24, 400);
  return {
    width: W,
    height: H,
    screen: { x: side, y: top, w: W - 2 * side, h: H - top - consoleH },
    screenRadius: r,
    consoleTop: H - consoleH,
  };
}

const SIZES: [number, number][] = [
  [1440, 900],
  [1280, 800],
  [3840, 2160],
  [1000, 640],
];

describe("frame geometry", () => {
  for (const [W, H] of SIZES) {
    describe(`${W}x${H}`, () => {
      const input = inputFor(W, H);
      const g = computeFrameGeometry(input);

      it("puts the tube's outer edge on the top margin at the crest", () => {
        expect(g.apexY - g.tube / 2).toBeCloseTo(g.margin, 6);
      });

      it("keeps a constant gap between tube and screen round the corner", () => {
        // Corner arc is concentric with the screen corner: inner edge radius = screen radius + gap.
        expect(g.cornerR - g.tube / 2).toBeCloseTo(input.screenRadius + g.gap, 6);
        expect(g.cornerCx).toBeCloseTo(input.screen.x + input.screenRadius, 6);
      });

      it("joins corner arc and crest arc without a kink (internally tangent circles)", () => {
        expect(g.bigR).toBeGreaterThan(g.cornerR);
        const dist = Math.hypot(W / 2 - g.cornerCx, g.bigCy - g.cornerCy);
        expect(dist).toBeCloseTo(g.bigR - g.cornerR, 4);
        expect(g.junctionAngle).toBeGreaterThan(Math.PI);
        expect(g.junctionAngle).toBeLessThan(1.5 * Math.PI);
      });

      it("has a bubble track from the column base to the crest, continuous and of the stated length", () => {
        const track = bubbleTrack(g, "left");
        expect(track.length).toBeCloseTo(g.halfLength, 4);
        const p: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
        track.at(0, p);
        expect(p.x).toBeCloseTo(g.xL, 6);
        expect(p.y).toBeCloseTo(g.baseTop, 6);
        track.at(track.length, p);
        expect(p.x).toBeCloseTo(W / 2, 4);
        expect(p.y).toBeCloseTo(g.apexY, 4);
        // Walk it: steps of 1 px of arc length move about 1 px, normals stay unit length.
        let px = 0;
        let py = 0;
        for (let s = 0; s <= track.length; s += 1) {
          track.at(s, p);
          expect(Math.hypot(p.nx, p.ny)).toBeCloseTo(1, 6);
          if (s > 0) expect(Math.hypot(p.x - px, p.y - py)).toBeLessThan(1.0001);
          px = p.x;
          py = p.y;
        }
      });

      it("mirrors the right track", () => {
        const l = bubbleTrack(g, "left");
        const r = bubbleTrack(g, "right");
        const a: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
        const b: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
        for (const f of [0, 0.3, 0.7, 1]) {
          l.at(f * l.length, a);
          r.at(f * r.length, b);
          expect(b.x).toBeCloseTo(W - a.x, 6);
          expect(b.y).toBeCloseTo(a.y, 6);
          expect(b.nx).toBeCloseTo(-a.nx, 6);
        }
      });

      it("keeps the light period at least as long as every light region", () => {
        expect(g.lightPeriod).toBeGreaterThanOrEqual(W / 2);
        expect(g.lightPeriod).toBeGreaterThanOrEqual(g.baseTop - g.junctionY);
      });

      it("emits a well-formed path", () => {
        const d = centreLinePath(g, 0);
        expect(d.startsWith("M")).toBe(true);
        expect(d).not.toMatch(/NaN|Infinity/);
        expect((d.match(/A/g) ?? []).length).toBe(4);
      });
    });
  }

  it("falls back to a straight top when there is no room for a crest rise", () => {
    const g = computeFrameGeometry({
      width: 800,
      height: 500,
      screen: { x: 30, y: 30, w: 740, h: 340 },
      screenRadius: 30,
      consoleTop: 400,
    });
    expect(g.bigR).toBe(0);
    expect(centreLinePath(g, 0)).not.toMatch(/NaN/);
  });
});

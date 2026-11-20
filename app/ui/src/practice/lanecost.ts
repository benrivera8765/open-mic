// SPDX-License-Identifier: GPL-3.0-or-later
// The practice lane's drawing cost in this web view, for the self-check (WO2 4.5 / 4.8,
// APP_API 10.1 `lane`) and the screenshot script: frames of the bundled demo song's guide
// (the real Rust builder's output, src/practice/fixtures) drawn on an offscreen canvas the
// size of the lane at this window, with a simulated singer, plain and with the simple lane.
// Nothing is shown and nothing plays.
//
// What is timed: the frame function and the drawing commands AND their rasterisation. The
// canvases are made with `willReadFrequently`, which keeps them on the CPU (Skia / Core
// Graphics software raster), and each frame ends with a one-pixel read, so every pixel is
// really drawn inside the timed span. On a GPU-backed canvas the same read would time the
// wait for the GPU to come back (a pipeline stall of several ms that the real frame loop
// never has), not the drawing; the CPU raster is the honest, repeatable upper bound of the
// work the 60 fps loop does (the lyrics display comes on top of it).
import type { LaneCost, PracticeGuide } from "../api/types";
import fixture from "./fixtures/demo-song.practice.json";
import { type GuideFixture, practiceGuideOf } from "./guide";
import { type LaneInputs, type LaneLayout, layoutLane } from "./lane";
import { LaneRenderer, type LaneStyle } from "./lanerenderer";
import { LiveStore } from "./live";
import type { KeyView } from "./pitch";

/** A canvas kept on the CPU (software raster), so a pixel read times the drawing, not a GPU stall. */
export function cpuCanvas(): HTMLCanvasElement {
  const c = document.createElement("canvas");
  // The first getContext call fixes the attributes; the renderer's later calls get this context.
  c.getContext("2d", { willReadFrequently: true });
  return c;
}

export interface RasterBench {
  avg: number;
  p95: number;
  max: number;
  frames: number;
}

/**
 * Draws `frames` frames of layout `L` from `inputs(k)` on a fresh CPU canvas (tiles too),
 * each followed by a one-pixel read, and returns the times in ms. The first frames make the
 * tiles and sprites (as the real loop's first frames do) and are included.
 */
export function rasterBench(L: LaneLayout, style: LaneStyle, simple: boolean, dpr: number, frames: number, inputs: (k: number) => LaneInputs): RasterBench | null {
  const canvas = cpuCanvas();
  let r: LaneRenderer;
  try {
    r = new LaneRenderer(canvas, cpuCanvas);
  } catch {
    return null;
  }
  const ctx = canvas.getContext("2d");
  r.setSize(L.params.width, L.params.height, dpr);
  r.setLayout(L, style, simple);
  const times: number[] = [];
  for (let k = 0; k < frames; k++) {
    const t0 = performance.now();
    r.draw(inputs(k));
    ctx?.getImageData(0, 0, 1, 1);
    times.push(performance.now() - t0);
  }
  const sorted = [...times].sort((a, b) => a - b);
  return {
    avg: times.reduce((a, b) => a + b, 0) / Math.max(1, times.length),
    p95: sorted[Math.floor(sorted.length * 0.95)] ?? 0,
    max: sorted[sorted.length - 1] ?? 0,
    frames: times.length,
  };
}

/** A singer on the note due at t (with a little vibrato), as practice://live would bring it. */
export function singAlong(guide: PracticeGuide, live: LiveStore, t: number): void {
  const n = guide.notes.find((x) => x.start <= t && t < x.end && x.kind === "pitched");
  live.push(t - 0.04, n ? n.midi + 0.1 * Math.sin(t * 30) : null, -20);
}

export async function measureLaneCost(frames = 600): Promise<LaneCost | null> {
  if (typeof document === "undefined") return null;
  const demo = fixture as unknown as GuideFixture;
  const guide = practiceGuideOf("demo", demo.guide, demo.lyrics, demo.curve);
  const scale = Math.min(window.innerWidth / 1440, window.innerHeight / 900);
  // The lane's share of the skin's lyrics area (about 85 % of the width, 36 % of the height).
  const width = Math.round(window.innerWidth * 0.85);
  const height = Math.round(window.innerHeight * 0.36);
  const dpr = window.devicePixelRatio || 1;
  const scratch = document.createElement("canvas").getContext("2d");
  const measure = (t: string, px: number) => {
    if (!scratch) return t.length * px * 0.55;
    scratch.font = `700 ${px}px system-ui, sans-serif`;
    return scratch.measureText(t).width;
  };
  const L = layoutLane(guide, demo.lyrics, demo.duration_s, { width, height, pianoSide: "left", words: true, scale, dpr }, measure);
  const style: LaneStyle = { sung: "#ffc21a", marker: "#7dffd8", upcoming: "#f4f4f4", bar: "#ff3d8b", wordFont: "system-ui, sans-serif", wordWeight: "700", labelFont: "system-ui, sans-serif" };
  const keys: KeyView = { segment: 0, base: 0, changes: [], octave: 0, octaveFrom: 0, octaveSeenMs: -1e12 };
  const run = (simple: boolean): RasterBench | null => {
    const live = new LiveStore();
    return rasterBench(L, style, simple, dpr, frames, (k) => {
      const t = 2 + k / 60;
      singAlong(guide, live, t);
      const now = performance.now();
      return { t, nowMs: now, keys, spelling: "sharps", octaveMode: "auto", live, liveMs: now, watchOnly: false, reduceMotion: false, simple };
    });
  };
  const plain = run(false);
  const simple = run(true);
  if (!plain || !simple) return null;
  const round = (x: number) => Math.round(x * 1000) / 1000;
  return { frames, avg_ms: round(plain.avg), p95_ms: round(plain.p95), simple_avg_ms: round(simple.avg), width, height, dpr };
}

// SPDX-License-Identifier: GPL-3.0-or-later
// The lane renderer draws what the frame says (WO2 section 9, "The screen", carried down to
// the pixels): with a recording 2D context, every bar of the demo song ends up on screen,
// after the tile blit's own source and destination rectangles, exactly where the pitch rule
// puts it and on its piano key's row (whose bitmap is blitted by the same rule), at key 0,
// -5 and +3 and with an octave shift (to within half a device pixel, every bitmap copied
// 1:1 onto whole device pixels, the piano key exactly level with its bars); bars are drawn
// once into tiles and only moved (a key change redraws no tile); reduce-motion and the
// simple lane draw no tail.
import { describe, expect, it } from "vitest";
import fixture from "./fixtures/demo-song.practice.json";
import { type GuideFixture, practiceGuideOf } from "./guide";
import { barOf, FUTURE_S, type LaneInputs, layoutLane, xOf, yOf } from "./lane";
import { type BlitEntry, LaneRenderer, TILE_PX } from "./lanerenderer";
import { LiveStore } from "./live";
import type { KeyView } from "./pitch";

const demo = fixture as unknown as GuideFixture;
const guide = practiceGuideOf("demo", demo.guide, demo.lyrics, demo.curve);

interface FakeCanvas {
  width: number;
  height: number;
  id: number;
  calls: string[];
  getContext: () => unknown;
}
let ids = 0;
/** A canvas whose 2D context records method names and does nothing. */
function fakeCanvas(): FakeCanvas {
  const c: FakeCanvas = { width: 0, height: 0, id: ids++, calls: [], getContext: () => ctx };
  const target: Record<string, unknown> = {
    createLinearGradient: () => ({ addColorStop: () => undefined }),
    measureText: (t: string) => ({ width: t.length * 7 }),
  };
  const ctx = new Proxy(target, {
    get(t, k: string) {
      if (k in t) return t[k];
      return (...args: unknown[]) => {
        c.calls.push(k);
        void args;
      };
    },
    set(t, k: string, v) {
      t[k] = v;
      return true;
    },
  });
  return c;
}

function setup(dpr: number) {
  const L = layoutLane(guide, demo.lyrics, demo.duration_s, { width: 1200, height: 270, pianoSide: "left", words: true, scale: 1, dpr }, (t, px) => t.length * px * 0.55);
  const tiles: FakeCanvas[] = [];
  const main = fakeCanvas();
  const r = new LaneRenderer(main as unknown as HTMLCanvasElement, () => {
    const c = fakeCanvas();
    tiles.push(c);
    return c as unknown as HTMLCanvasElement;
  });
  r.setSize(1200, 270, dpr);
  r.setLayout(L, { sung: "#ffc21a", marker: "#7dffd8", upcoming: "#f4f4f4", bar: "#ff3d8b", wordFont: "sans-serif", wordWeight: "700", labelFont: "sans-serif" }, false);
  return { L, r, tiles, main };
}

const steady = (key: number, octave: number): KeyView => ({ segment: 0, base: key, changes: [], octave, octaveFrom: octave, octaveSeenMs: -1e12 });
const inputs = (t: number, keys: KeyView, extra: Partial<LaneInputs> = {}): LaneInputs => ({
  t,
  nowMs: 1e9,
  keys,
  spelling: "sharps",
  octaveMode: "auto",
  live: null,
  liveMs: 0,
  watchOnly: false,
  reduceMotion: false,
  simple: false,
  ...extra,
});

describe("lane renderer: the blits put every bar where the pitch rule says", () => {
  // 1.25 and 1.5: Windows display scaling; 2: Retina.
  for (const dpr of [1, 1.25, 1.5, 2]) {
    for (const [key, octave] of [
      [0, 0],
      [-5, 0],
      [3, 0],
      [-3, -1],
    ]) {
      it(`dpr ${dpr}, key ${key}, octave ${octave}: whole song`, () => {
        const { L, r } = setup(dpr);
        const tS = r.debugTileS();
        let checked = 0;
        for (let t = 0; t < demo.duration_s; t += 0.25) {
          r.debugLog = [];
          const f = r.draw(inputs(t, steady(key, octave)))!;
          const bars = r.debugLog.filter((e) => e.kind === "bars");
          const piano = r.debugLog.find((e) => e.kind === "piano")!;
          const P = r.debugPiano()!;
          for (let i = 0; i < L.notes.length; i++) {
            const n = L.notes[i];
            const x = xOf(L, t, n.start);
            if (x < L.barX + 2 || x > L.barX + L.barW - 2) continue;
            const k = Math.floor(n.start / tS + 1e-9);
            const e: BlitEntry | undefined = bars.find((b) => b.tile === k && x >= b.dx - 0.01 && x <= b.dx + b.dw + 0.01);
            expect(e, `t=${t} note ${i} has a blit`).toBeTruthy();
            // Where the tile's own drawing put the note, carried through the blit.
            const tileX = (n.start - k * tS) * L.pxPerSec;
            const tileY = r.tileY(n.lane);
            const screenX = e!.dx + (tileX * dpr - e!.sx) / dpr;
            const screenY = e!.dy + (tileY * dpr - e!.sy) / dpr;
            // The pitch rule, independently: midi + key + 12 x octave - tuning, on the key rows,
            // to within half a device pixel (the blits land on whole device pixels).
            const want = guide.notes[i].midi + key + 12 * octave - guide.tuning_cents / 100;
            const half = 0.5 / dpr + 1e-6;
            expect(Math.abs(screenX - x)).toBeLessThanOrEqual(half);
            expect(Math.abs(screenY - yOf(L, f.viewLow, want))).toBeLessThanOrEqual(half);
            expect(Math.abs(screenY - barOf(L, f, i).y)).toBeLessThanOrEqual(half);
            // ...and the piano key of that pitch is drawn at exactly the same height by its own blit.
            const p = Math.round(want);
            const keyY = piano.dy + (((P.hi - p + 0.5) * L.rowH) * dpr - piano.sy) / dpr;
            expect(Math.abs(keyY - (screenY + (want - p) * L.rowH))).toBeLessThan(0.01);
            checked++;
          }
          expect(e2eTileWidth(bars)).toBeLessThanOrEqual(TILE_PX + 0.01);
          // Every cached bitmap is copied 1:1 onto whole device pixels (no filtered blit).
          for (const e of r.debugLog) {
            for (const v of [e.sx, e.sy, e.sw, e.sh, e.dx * dpr, e.dy * dpr, e.dw * dpr, e.dh * dpr]) expect(Math.abs(v - Math.round(v)), `${e.kind} ${JSON.stringify(e)}`).toBeLessThan(1e-6);
            expect(Math.abs(e.dw * dpr - e.sw)).toBeLessThan(1e-6);
            expect(Math.abs(e.dh * dpr - e.sh)).toBeLessThan(1e-6);
          }
        }
        expect(checked).toBeGreaterThan(300);
      });
    }
  }

  it("bars are drawn once into tiles: a key change and the slide redraw no tile", () => {
    const { r, tiles } = setup(1);
    r.draw(inputs(3, steady(0, 0)));
    const made = tiles.length;
    const drawnIn = tiles.map((c) => c.calls.length);
    for (let k = 0; k < 20; k++) r.draw(inputs(3 + k / 60, { segment: 0, base: 0, changes: [{ at: 3.1, semitones: -3, from: 0, seenMs: 1e9 - 100 + k * 16, effectiveMs: 1e9 }], octave: 0, octaveFrom: 0, octaveSeenMs: -1e12 }));
    expect(tiles.length).toBe(made);
    expect(tiles.map((c) => c.calls.length)).toEqual(drawnIn);
    expect(FUTURE_S).toBeGreaterThanOrEqual(2);
  });

  it("reduce-motion and the simple lane draw the marker without a tail", () => {
    const { r } = setup(1);
    const live = new LiveStore();
    for (let t = 2.6; t < 3.0; t += 0.01) live.push(t, 67, -20);
    const full = r.draw(inputs(3.0, steady(0, 0), { live, liveMs: 1e9 }))!;
    expect(full.singer).not.toBeNull();
    expect(full.tail.length).toBeGreaterThan(20);
    for (const opt of [{ reduceMotion: true }, { simple: true }]) {
      const f = r.draw(inputs(3.0, steady(0, 0), { live, liveMs: 1e9, ...opt }))!;
      expect(f.singer).not.toBeNull();
      expect(f.tail.length).toBe(0);
    }
  });
});

/** Widest single tile blit (a tile is never stretched). */
function e2eTileWidth(bars: BlitEntry[]): number {
  return Math.max(0, ...bars.map((b) => b.dw));
}

describe("lane renderer: backing vocals (D-282)", () => {
  /** A canvas whose 2D context records each word drawn with its colour and alpha. */
  function recordingCanvas(log: { text: string; fill: string; alpha: number }[]) {
    const state = { fillStyle: "#000", globalAlpha: 1 };
    const stack: (typeof state)[] = [];
    const target: Record<string, unknown> = {
      createLinearGradient: () => ({ addColorStop: () => undefined }),
      measureText: (t: string) => ({ width: t.length * 7 }),
      save: () => stack.push({ ...state }),
      restore: () => Object.assign(state, stack.pop() ?? state),
      fillText: (t: string) => log.push({ text: t, fill: String(state.fillStyle), alpha: state.globalAlpha }),
    };
    const ctx = new Proxy(target, {
      get(t, k: string) {
        if (k in state) return state[k as keyof typeof state];
        if (k in t) return t[k];
        return () => undefined;
      },
      set(t, k: string, v) {
        if (k in state) (state as Record<string, unknown>)[k] = v;
        else t[k] = v;
        return true;
      },
    });
    return { width: 0, height: 0, getContext: () => ctx };
  }

  it("draws words in parentheses dimmed in the upcoming colour on both sides of the playhead", () => {
    const lyr = JSON.parse(JSON.stringify(demo.lyrics)) as typeof demo.lyrics;
    const ws = lyr!.lines[0].words;
    ws[0].chars[0][0] = "(" + ws[0].chars[0][0];
    const last = ws[1].chars[ws[1].chars.length - 1];
    last[0] = last[0] + ")";
    const text = (i: number) => ws[i].chars.map((c) => c[0]).join("").trim();
    const L = layoutLane(guide, lyr, demo.duration_s, { width: 1200, height: 270, pianoSide: "left", words: true, scale: 1, dpr: 1 }, (t, px) => t.length * px * 0.55);
    const log: { text: string; fill: string; alpha: number }[] = [];
    const r = new LaneRenderer(recordingCanvas(log) as unknown as HTMLCanvasElement, () => recordingCanvas(log) as unknown as HTMLCanvasElement);
    r.setSize(1200, 270, 1);
    const SUNG = "#ffc21a";
    const UP = "#f4f4f4";
    r.setLayout(L, { sung: SUNG, marker: "#7dffd8", upcoming: UP, bar: "#ff3d8b", wordFont: "sans-serif", wordWeight: "700", labelFont: "sans-serif" }, false);
    for (let t = 0; t < lyr!.lines[0].end + 2; t += 0.25) r.draw(inputs(t, steady(0, 0)));
    for (const i of [0, 1]) {
      const drawn = log.filter((e) => e.text === text(i));
      expect(drawn.length, text(i)).toBeGreaterThanOrEqual(2);
      expect(drawn.every((e) => e.fill === UP && e.alpha === 0.5), JSON.stringify(drawn)).toBe(true);
    }
    // The singer's words are unchanged: full strength, sung colour once sung.
    const sungWord = log.filter((e) => e.text === text(2));
    expect(sungWord.some((e) => e.fill === SUNG && e.alpha === 1)).toBe(true);
    expect(sungWord.some((e) => e.fill === UP && e.alpha === 1)).toBe(true);
    expect(sungWord.every((e) => e.alpha === 1)).toBe(true);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
// The page display's progress needle (page.ts point 6; the owner's request of 18 Nov 2026:
// "readd the bar that was in the previous build that would mark the progress of where the
// song is ... at the point where it sweeps over there should be a bar"). The scrolling
// ticker's marker needle rides the wipe edge.
//
//   1. Contract at 60 fps across whole songs (the page tests' song set: the demo song and
//      the tiny fixture, the mock's three songs, a generated rap, the synthetic stress
//      song, the end-to-end songs when present) at 1280x800, 1440x900, 2560x1440 and
//      3840x2160 in the skin's lyrics area, both paces: pageverify.ts (n) on every frame:
//      x on the gold/white boundary (within 0.5 px), on the row being sung, never left on
//      a row, waiting at the first letter during a lead-in, hidden in rests, inside the area.
//   2. Through the renderer (pagerenderer.ts) with a recording canvas: the needle is drawn
//      last, in the marker colour, at exactly the x where its row is split between the sung
//      and the upcoming bitmap (and at the text's start or end when the row is drawn in one
//      colour), at device pixel ratios 1 and 2; nothing is drawn when it is off.
//   3. The behaviours one by one on small synthetic songs: wrapped rows, lead-in, rests,
//      line changes with and without the preview; and the ticker's needle look, unchanged.
//
// Lyrics never reach the console: problem strings are redacted ([text]) before they are
// compared, and the synthetic songs are made-up words.
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { DEFAULT_APP_SETTINGS } from "../api/defaults";
import { FRERE_LYRICS, ROW_LYRICS, TWINKLE_LYRICS } from "../api/demo";
import { lyricsFaceMeasure } from "./lyrics-face-widths";
import {
  INK_PAD_EM,
  LINE_HEIGHT_EM,
  layoutPage,
  NEEDLE_HEAD_H_EM,
  NEEDLE_HEAD_W_EM,
  NEEDLE_OVER_EM,
  NEEDLE_W_EM,
  type PageLayout,
  pageFrame,
  PREVIEW_SCALE,
  TRANSITION_S,
} from "./page";
import { PageRenderer, type PageStyle } from "./pagerenderer";
import { NEEDLE_TOLERANCE_PX, verifyPage } from "./pageverify";
import { drawNeedle } from "./renderer";
import { denseRapLyrics, e2eSongs, e2eSongsDir, fixtureSongs, shimMeasure, stressLyrics } from "./testsongs-node";
import { WORD_TOLERANCE_S } from "./wordmotion";

const measure = lyricsFaceMeasure(shimMeasure, 800);
const TEXT_SIZE = DEFAULT_APP_SETTINGS.ui.lyrics_text_size;
const LANES = [
  { w: 1280, h: 800, lane: { w: 1074, h: 397 } },
  { w: 1440, h: 900, lane: { w: 1208, h: 450 } },
  { w: 2560, h: 1440, lane: { w: 2150, h: 732 } },
  { w: 3840, h: 2160, lane: { w: 3225, h: 1109 } },
];
const fontAt = (win: { w: number; h: number }) => TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
const redact = (s: string) => s.replace(/"[^"]*"/g, "[text]");
const sets: { name: string; lyrics: Lyrics }[] = [
  ...fixtureSongs(),
  { name: "demo: Twinkle, Twinkle, Little Star", lyrics: TWINKLE_LYRICS },
  { name: "demo: Frère Jacques", lyrics: FRERE_LYRICS },
  { name: "demo: Row, Row, Row Your Boat (triplets)", lyrics: ROW_LYRICS },
  { name: "synthetic dense rap", lyrics: denseRapLyrics(18) },
  { name: "synthetic stress", lyrics: stressLyrics() },
  ...e2eSongs(e2eSongsDir()),
];
const demoSong = fixtureSongs().find((s) => s.name.includes("demo-song"))?.lyrics;
const MARKER = DEFAULT_APP_SETTINGS.ui.colors.marker;

describe("progress needle: on the wipe edge on every frame at 60 fps, whole songs, four sizes", () => {
  it("the demo song is in the set", () => {
    expect(demoSong).toBeTruthy();
  });
  for (const s of sets) {
    for (const win of LANES) {
      for (const pace of ["word", "letter"] as const) {
        it(`${s.name} @ ${win.w}x${win.h}, ${pace} pace`, () => {
          const l = layoutPage(s.lyrics, { width: win.lane.w, height: win.lane.h, fontPx: fontAt(win), pace }, measure);
          const r = verifyPage(l, { frameDt: 1 / 60 });
          const needleIssues = r.firstProblems.filter((p) => p.includes("needle")).map(redact);
          expect(needleIssues).toEqual([]);
          expect(r.needleProblems).toBe(0);
          expect(r.needleOffEdge + r.needleWrongLine + r.needleWrongRow + r.needleBackwards + r.needleNotParked + r.needleInRest + r.needleOutside).toBe(0);
          expect(r.needleMaxDxPx).toBeLessThanOrEqual(NEEDLE_TOLERANCE_PX);
          expect(r.needleFrames).toBeGreaterThan(0);
          if (r.leadIns > 0) expect(r.needleParkedFrames).toBeGreaterThan(0);
        });
      }
    }
  }
});

// ---------------------------------------------------------------------------
// A recording Canvas 2D: enough of the API for PageRenderer, logging what is drawn.
// ---------------------------------------------------------------------------

interface Clip {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}
type Op =
  | { kind: "image"; img: FakeCanvas; dx: number; dy: number; dw: number; dh: number; clip: Clip | null }
  | { kind: "rect"; x: number; y: number; w: number; h: number; style: unknown; alpha: number; shadow: string }
  | { kind: "path"; pts: [number, number][]; style: unknown };

class FakeCtx {
  log: Op[] = [];
  fillStyle: unknown = "#000";
  strokeStyle: unknown = "#000";
  globalAlpha = 1;
  font = "";
  textBaseline = "";
  textAlign = "";
  lineJoin = "";
  miterLimit = 10;
  lineWidth = 1;
  shadowColor = "";
  shadowBlur = 0;
  shadowOffsetY = 0;
  imageSmoothingEnabled = true;
  imageSmoothingQuality = "low";
  fontKerning = "auto";
  /** The colour of this canvas's letters (the first fillText's fill). */
  textFill: unknown = null;
  private clipRect: Clip | null = null;
  private pending: Clip | null = null;
  private path: [number, number][] = [];
  private stack: { clip: Clip | null; fillStyle: unknown; globalAlpha: number; shadowColor: string }[] = [];
  setTransform(): void {}
  clearRect(): void {
    this.log = [];
  }
  save(): void {
    this.stack.push({ clip: this.clipRect, fillStyle: this.fillStyle, globalAlpha: this.globalAlpha, shadowColor: this.shadowColor });
  }
  restore(): void {
    const s = this.stack.pop()!;
    this.clipRect = s.clip;
    this.fillStyle = s.fillStyle;
    this.globalAlpha = s.globalAlpha;
    this.shadowColor = s.shadowColor;
  }
  beginPath(): void {
    this.pending = null;
    this.path = [];
  }
  rect(x: number, y: number, w: number, h: number): void {
    this.pending = { x0: x, y0: y, x1: x + w, y1: y + h };
  }
  clip(): void {
    this.clipRect = this.pending;
  }
  moveTo(x: number, y: number): void {
    this.path.push([x, y]);
  }
  lineTo(x: number, y: number): void {
    this.path.push([x, y]);
  }
  closePath(): void {}
  fill(): void {
    this.log.push({ kind: "path", pts: [...this.path], style: this.fillStyle });
  }
  drawImage(img: FakeCanvas, dx: number, dy: number, dw: number, dh: number): void {
    this.log.push({ kind: "image", img, dx, dy, dw, dh, clip: this.clipRect });
  }
  fillRect(x: number, y: number, w: number, h: number): void {
    this.log.push({ kind: "rect", x, y, w, h, style: this.fillStyle, alpha: this.globalAlpha, shadow: this.shadowColor });
  }
  fillText(): void {
    if (this.textFill === null) this.textFill = this.fillStyle;
  }
  strokeText(): void {}
  measureText(s: string) {
    return { width: s.length * 10, actualBoundingBoxAscent: 0, fontBoundingBoxAscent: 0, fontBoundingBoxDescent: 0 };
  }
  createLinearGradient() {
    return { addColorStop() {} };
  }
}
class FakeCanvas {
  width = 0;
  height = 0;
  ctx = new FakeCtx();
  getContext(): FakeCtx {
    return this.ctx;
  }
}

const STYLE: PageStyle = { fontFamily: "test", fontWeight: "800", sung: "#ffc21a", upcoming: "#f4f4f4", outline: "#000", shadow: "rgba(0, 0, 0, 0.55)" };

function rendererFor(l: PageLayout, dpr: number, marker: string | null = MARKER) {
  const main = new FakeCanvas();
  const r = new PageRenderer(main as unknown as HTMLCanvasElement, () => new FakeCanvas() as unknown as HTMLCanvasElement);
  r.setSize(l.width, l.height, dpr);
  r.setLayout(l, STYLE);
  r.setMarker(marker);
  return { r, ctx: main.ctx };
}

/** What a recorded frame shows of the needle: the shaft's centre x, its rect and head, and the rows split between the colours. */
function readFrame(ctx: FakeCtx) {
  const shafts = ctx.log.filter((o): o is Extract<Op, { kind: "rect" }> => o.kind === "rect" && o.style === MARKER);
  const heads = ctx.log.filter((o): o is Extract<Op, { kind: "path" }> => o.kind === "path" && o.style === MARKER);
  const images = ctx.log.filter((o): o is Extract<Op, { kind: "image" }> => o.kind === "image");
  const splits: { x: number; y0: number; y1: number }[] = [];
  for (let i = 0; i + 1 < images.length; i++) {
    const a = images[i];
    const b = images[i + 1];
    if (a.clip && b.clip && a.img.ctx.textFill === STYLE.sung && b.img.ctx.textFill === STYLE.upcoming && a.clip.x1 === b.clip.x0) splits.push({ x: a.clip.x1, y0: a.dy, y1: a.dy + a.dh });
  }
  return { shafts, heads, images, splits, lastOps: ctx.log.slice(-2).map((o) => o.kind) };
}

describe("progress needle: drawn by the renderer exactly where the row is split", () => {
  const songs = [
    ...(demoSong ? [{ name: "demo song", lyrics: demoSong }] : []),
    { name: "synthetic dense rap (wrapped and shrunk lines)", lyrics: denseRapLyrics(18) },
    { name: "synthetic stress (rests, zero-length letters)", lyrics: stressLyrics() },
  ];
  for (const s of songs) {
    for (const win of LANES) {
      for (const dpr of [1, 2]) {
        it(`${s.name} @ ${win.w}x${win.h}, device pixel ratio ${dpr}`, () => {
          const l = layoutPage(s.lyrics, { width: win.lane.w, height: win.lane.h, fontPx: fontAt(win) }, measure);
          const { r, ctx } = rendererFor(l, dpr);
          const t0 = Math.min(l.lines[0].start - 6, 0);
          const t1 = l.lines[l.lines.length - 1].leave + TRANSITION_S + 0.5;
          let drawn = 0;
          let partial = 0;
          let maxPure = 0;
          const bad: string[] = [];
          for (let t = t0; t <= t1; t += 1 / 60) {
            const f = r.draw(t)!;
            const n = f.needle;
            const fr = readFrame(ctx);
            if (!n) {
              if (fr.shafts.length || fr.heads.length || r.lastNeedle) bad.push(`t=${t.toFixed(3)}: a needle drawn without one in the frame`);
              continue;
            }
            drawn++;
            if (fr.shafts.length !== 1 || fr.heads.length !== 1) {
              bad.push(`t=${t.toFixed(3)}: ${fr.shafts.length} shafts, ${fr.heads.length} heads`);
              continue;
            }
            const sh = fr.shafts[0];
            const x = sh.x + sh.w / 2;
            const L = l.lines[n.line];
            const it = f.items.find((i) => i.line === n.line && i.role === n.role)!;
            // Drawn last (on top of the rows, the lead-in bar and the countdown), the ticker's shape at the line's size.
            if (fr.lastOps.join() !== "rect,path") bad.push(`t=${t.toFixed(3)}: not drawn last (${fr.lastOps.join()})`);
            if (Math.abs(sh.w - NEEDLE_W_EM * n.px) > 1e-9) bad.push(`t=${t.toFixed(3)}: shaft ${sh.w} wide`);
            const head = fr.heads[0].pts;
            if (Math.abs(head[1][0] - head[0][0] - NEEDLE_HEAD_W_EM * n.px) > 1e-9 || Math.abs(head[2][1] - head[0][1] - NEEDLE_HEAD_H_EM * n.px) > 1e-9 || Math.abs(head[2][0] - x) > 1e-9 || Math.abs(head[2][1] - sh.y) > 1e-9)
              bad.push(`t=${t.toFixed(3)}: head ${JSON.stringify(head)}`);
            if (Math.abs(sh.alpha - n.alpha) > 1e-12) bad.push(`t=${t.toFixed(3)}: alpha ${sh.alpha} vs ${n.alpha}`);
            // (x is read back as shaft left + half its width: equal up to rounding.)
            if (!r.lastNeedle || Math.abs(r.lastNeedle.x - x) > 1e-9) bad.push(`t=${t.toFixed(3)}: lastNeedle ${r.lastNeedle?.x} vs drawn ${x}`);
            maxPure = Math.max(maxPure, Math.abs(x - n.x));
            // The row as drawn: split between the colours, or one bitmap.
            const row = L.rows[n.row];
            const rowW = row.u1 - row.u0;
            const pad = INK_PAD_EM * L.px;
            const mid = (sh.y + sh.y + sh.h) / 2;
            if (n.sung > 0 && n.sung < rowW) {
              partial++;
              const hits = fr.splits.filter((sp) => mid >= sp.y0 && mid <= sp.y1 && Math.abs(sp.x - x) <= 1e-9);
              if (hits.length !== 1) bad.push(`t=${t.toFixed(3)}: needle at ${x}, splits ${JSON.stringify(fr.splits.map((sp) => sp.x))}`);
              if (r.lastNeedle?.split !== r.lastNeedle?.x) bad.push(`t=${t.toFixed(3)}: split recorded ${r.lastNeedle?.split} vs needle ${x}`);
            } else {
              // Whole row: at the text's end (all sung) or its start (nothing sung) in that bitmap.
              const colour = n.sung >= rowW ? STYLE.sung : STYLE.upcoming;
              const imgs = fr.images.filter((im) => !im.clip && im.img.ctx.textFill === colour && mid >= im.dy && mid <= im.dy + im.dh);
              const want = (im: (typeof imgs)[number]) => im.dx + (pad + (n.sung >= rowW ? rowW : 0)) * it.scale;
              if (!imgs.some((im) => Math.abs(want(im) - x) < 1e-9)) bad.push(`t=${t.toFixed(3)}: whole row, needle at ${x}, text edges ${JSON.stringify(imgs.map(want))}`);
            }
            // The shaft covers the row's band (plus NEEDLE_OVER_EM above and below).
            const bandTop = sh.y + NEEDLE_OVER_EM * n.px;
            if (Math.abs(sh.h - (LINE_HEIGHT_EM + 2 * NEEDLE_OVER_EM) * n.px) > 1e-9) bad.push(`t=${t.toFixed(3)}: shaft ${sh.h} tall`);
            if (Math.abs(bandTop - (it.top + row.top * it.scale)) > 0.5 / dpr + 1e-9) bad.push(`t=${t.toFixed(3)}: band top ${bandTop} vs ${it.top + row.top * it.scale}`);
          }
          expect(bad.slice(0, 5)).toEqual([]);
          expect(drawn).toBeGreaterThan(100);
          expect(partial).toBeGreaterThan(50);
          // Drawn at the pure boundary within the device-pixel snap of the rows (scale 1).
          expect(maxPure).toBeLessThanOrEqual(0.5 / dpr + 1e-9);
          expect(maxPure).toBeLessThanOrEqual(NEEDLE_TOLERANCE_PX);
        });
      }
    }
  }

  it("is not drawn when the setting is off, and comes back in another colour without re-rendering the rows", () => {
    const lyr = demoSong ?? TWINKLE_LYRICS;
    const l = layoutPage(lyr, { width: 1208, height: 450, fontPx: 56 }, measure);
    const { r, ctx } = rendererFor(l, 1, null);
    const L = l.lines[1];
    const t = (L.words[1].start + L.words[1].end) / 2;
    const f = r.draw(t)!;
    expect(f.needle).not.toBeNull(); // the frame knows where it would be
    expect(readFrame(ctx).shafts).toEqual([]);
    expect(r.lastNeedle).toBeNull();
    const rendered = r.stats.tilesRendered;
    r.setMarker("#ff0000");
    r.draw(t);
    const shafts = ctx.log.filter((o) => o.kind === "rect" && o.style === "#ff0000");
    expect(shafts.length).toBe(1);
    expect(r.stats.tilesRendered).toBe(rendered);
  });
});

// ---------------------------------------------------------------------------
// Behaviours one by one (made-up words).
// ---------------------------------------------------------------------------

function lineOf(words: [string, number, number][]): Lyrics["lines"][number] {
  const ws = words.map(([text, start, end]) => {
    const letters = [...text];
    const d = (end - start) / letters.length;
    return { start, end, chars: letters.map((c, k) => [c, start + k * d, start + (k + 1) * d] as [string, number, number]) };
  });
  return { start: ws[0].start, end: ws[ws.length - 1].end, words: ws };
}
const song = (...lines: Lyrics["lines"]): Lyrics => ({ lyrics_version: 1, language: "en", source: "user_text", checked: true, lines });
const said = (text: string, t0: number, dur = 0.3): Lyrics["lines"][number] => lineOf(text.split(" ").map((w, k) => [w, t0 + k * dur, t0 + (k + 1) * dur]));
const AREA = { width: 1208, height: 450, fontPx: 56 };

describe("progress needle details", () => {
  it("is on by default, in the marker colour (the ticker's mint)", () => {
    expect(DEFAULT_APP_SETTINGS.ui.show_marker).toBe(true);
    expect(DEFAULT_APP_SETTINGS.ui.colors.marker).toBe("#7dffd8");
  });

  it("travels along the row being sung and jumps to the next row's start when the wipe does", () => {
    const long = "this made up line is much longer than the screen is wide even at the smallest size so it wraps";
    const l = layoutPage(song(said(long, 2, 0.3)), AREA, measure);
    const L = l.lines[0];
    expect(L.rows.length).toBe(2);
    const [r0, r1] = L.rows;
    let prev: { row: number; x: number } | null = null;
    let atRowEnd = 0;
    let jumps = 0;
    for (let t = L.start - 0.5; t < L.end + 0.5; t += 1 / 240) {
      const f = pageFrame(l, t);
      const n = f.needle!;
      expect(n.line).toBe(0);
      const it = f.items.find((x) => x.line === 0)!;
      // The shaft over the row's band.
      expect(n.top).toBeCloseTo(it.top + L.rows[n.row].top - NEEDLE_OVER_EM * L.px, 9);
      expect(n.bottom - n.top).toBeCloseTo((LINE_HEIGHT_EM + 2 * NEEDLE_OVER_EM) * L.px, 9);
      if (n.row === 0) {
        expect(n.x).toBeGreaterThanOrEqual(r0.left - 1e-9);
        expect(n.x).toBeLessThanOrEqual(r0.left + (r0.u1 - r0.u0) + 1e-9);
        if (Math.abs(n.x - (r0.left + r0.u1 - r0.u0)) < 1e-9) atRowEnd++;
      }
      if (prev && prev.row === n.row) expect(n.x).toBeGreaterThanOrEqual(prev.x - 1e-9);
      if (prev && prev.row !== n.row) {
        jumps++;
        expect([prev.row, n.row]).toEqual([0, 1]);
        // From the end of the first row to the start of the second, as the wipe enters it.
        expect(prev.x).toBeCloseTo(r0.left + (r0.u1 - r0.u0), 9);
        expect(n.x - r1.left).toBeGreaterThan(0);
        expect(n.x - r1.left).toBeLessThan(0.2 * L.px);
        expect(it.wipe).toBeGreaterThan(r1.u0);
      }
      prev = { row: n.row, x: n.x };
    }
    expect(jumps).toBe(1);
    // The row's last word carries the space after it: the needle waits at the end of the row meanwhile.
    expect(atRowEnd).toBeGreaterThan(3);
  });

  it("waits at the first letter during the lead-in, then leaves it as the first word starts", () => {
    // 0.4 s gap (no bar), 2 s gap (bar), 8 s rest (countdown, then the line and its bar).
    const l = layoutPage(song(said("one two three", 0.5, 0.3), said("four five six", 1.8, 0.3), said("seven eight nine", 4.7, 0.3), said("ten eleven", 13.6, 0.3)), AREA, measure);
    for (const i of [2, 3]) {
      const L = l.lines[i];
      expect(L.leadIn).not.toBeNull();
      let frames = 0;
      for (let t = L.leadIn!.from; t < L.start - WORD_TOLERANCE_S; t += 1 / 60) {
        const f = pageFrame(l, t);
        expect(f.leadIn!.line).toBe(i);
        expect(f.needle!.line).toBe(i);
        expect(f.needle!.row).toBe(0);
        expect(f.needle!.x).toBe(L.rows[0].left);
        expect(f.needle!.alpha).toBe(1);
        frames++;
      }
      expect(frames).toBeGreaterThan(30);
      // Inside the first word a moment after it starts.
      const n = pageFrame(l, L.start + 0.1).needle!;
      expect(n.x).toBeGreaterThan(L.rows[0].left);
      expect(n.x).toBeLessThan(L.rows[0].left + (L.words[0].b - L.words[0].a));
    }
  });

  it("is hidden while a rest shows the countdown: it fades out with the line before and in with the line after", () => {
    const l = layoutPage(song(said("seven eight nine", 4.7, 0.3), said("ten eleven", 13.6, 0.3)), AREA, measure);
    const [a, b] = l.lines;
    expect(b.breakBefore).toBe(true);
    // Fully opaque at the end of the line while it waits to leave.
    const before = pageFrame(l, a.leave - 0.01).needle!;
    expect(before.line).toBe(0);
    expect(before.alpha).toBe(1);
    expect(before.x).toBeCloseTo(a.rows[0].left + a.rows[0].u1 - a.rows[0].u0, 9);
    // Fading with the line as it leaves.
    const leaving = pageFrame(l, a.leave + 0.05);
    expect(leaving.needle!.role).toBe("leaving");
    expect(leaving.needle!.alpha).toBeGreaterThan(0);
    expect(leaving.needle!.alpha).toBeLessThan(1);
    expect(leaving.needle!.alpha).toBeCloseTo(leaving.items.find((x) => x.line === 0)!.alpha, 12);
    // The rest: countdown on screen, no needle.
    const rest = l.rests.find((x) => x.line === 1)!;
    for (let t = rest.from; t < rest.to; t += 0.05) {
      const f = pageFrame(l, t);
      expect(f.needle).toBeNull();
    }
    expect(pageFrame(l, (rest.from + rest.to) / 2).countdown).not.toBeNull();
    // Back with the line, fading in with it, parked at its first letter.
    const back = pageFrame(l, b.current + 0.1);
    expect(back.needle!.line).toBe(1);
    expect(back.needle!.alpha).toBeCloseTo(back.items.find((x) => x.line === 1)!.alpha, 12);
    expect(back.needle!.alpha).toBeLessThan(1);
    expect(pageFrame(l, b.current + TRANSITION_S + 0.01).needle!.alpha).toBe(1);
    expect(back.needle!.x).toBe(b.rows[0].left);
    // The intro rest of a song that starts late: nothing before the first line appears.
    const late = layoutPage(song(said("ten eleven", 9, 0.3)), AREA, measure);
    expect(pageFrame(late, 2).needle).toBeNull();
    expect(pageFrame(late, late.lines[0].current + 0.5).needle!.x).toBe(late.lines[0].rows[0].left);
  });

  it("moves with the page change to the next line's start (with the preview: up and growing; without: in place)", () => {
    const lyr = song(said("first line here", 2, 0.4), said("second line now", 4, 0.4), said("third one", 6, 0.4));
    const l = layoutPage(lyr, AREA, measure);
    const [a, b] = l.lines;
    // At the end of the first line until the change starts.
    const end = pageFrame(l, b.current - 1e-4).needle!;
    expect(end.line).toBe(0);
    expect(end.x).toBeCloseTo(a.rows[0].left + a.rows[0].u1 - a.rows[0].u0, 9);
    // From the change on, at the second line's first letter, moving up and growing with it.
    let lastTop = Infinity;
    let lastPx = 0;
    for (let t = b.current; t <= b.current + TRANSITION_S; t += 1 / 120) {
      const f = pageFrame(l, t);
      const n = f.needle!;
      const it = f.items.find((x) => x.line === 1)!;
      expect(n.line).toBe(1);
      expect(n.alpha).toBe(1);
      expect(n.x).toBeCloseTo(b.cx + (b.rows[0].left - b.cx) * it.scale, 9);
      expect(n.top).toBeLessThanOrEqual(lastTop + 1e-9);
      expect(n.px).toBeGreaterThanOrEqual(lastPx - 1e-9);
      lastTop = n.top;
      lastPx = n.px;
    }
    expect(pageFrame(l, b.current).needle!.px).toBeCloseTo(PREVIEW_SCALE * b.px, 9);
    expect(lastPx).toBeCloseTo(b.px, 6);
    // Without the preview the next line fades in where the old one was; the needle is there at once.
    const flat = layoutPage(lyr, { ...AREA, showNext: false }, measure);
    const fb = flat.lines[1];
    const n = pageFrame(flat, fb.current + 0.02).needle!;
    expect(n.line).toBe(1);
    expect(n.alpha).toBe(1);
    expect(n.x).toBe(fb.rows[0].left);
  });

  it("keeps its contract on 200 generated rap songs (many shrunk and wrapped lines) at two sizes, both paces", () => {
    // (page.test.ts runs the random fuzz songs through the same check.)
    let checked = 0;
    for (let seed = 1; seed <= 200; seed++) {
      const lyr = denseRapLyrics(seed);
      for (const area of [AREA, { width: 600, height: 260, fontPx: 40 }])
        for (const pace of ["word", "letter"] as const) {
          const r = verifyPage(layoutPage(lyr, { ...area, pace }, measure), { dt: 0.05, frameDt: 1 / 20 });
          expect(r.firstProblems.filter((p) => p.includes("needle")).map(redact)).toEqual([]);
          checked += r.needleFrames;
        }
    }
    expect(checked).toBeGreaterThan(1000);
  });

  it("the scrolling ticker keeps its needle: a 3 px shaft and a 14 x 8 px head (drawNeedle)", () => {
    const ctx = new FakeCtx();
    drawNeedle(ctx as unknown as CanvasRenderingContext2D, 100, 10, 50, 3, 14, 8, MARKER);
    expect(ctx.log).toEqual([
      { kind: "rect", x: 98.5, y: 10, w: 3, h: 40, style: MARKER, alpha: 1, shadow: "" },
      { kind: "path", pts: [[93, 2], [107, 2], [100, 10]], style: MARKER },
    ]);
    // At the default 56 px text the page display's needle is the same size.
    expect(NEEDLE_W_EM * 56).toBeCloseTo(3, 12);
    expect(NEEDLE_HEAD_W_EM * 56).toBeCloseTo(14, 12);
    expect(NEEDLE_HEAD_H_EM * 56).toBeCloseTo(8, 12);
  });
});

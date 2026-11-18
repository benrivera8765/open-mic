// SPDX-License-Identifier: GPL-3.0-or-later
// Edge cases of the page display and its progress needle (page.ts points 1-6, D-218 and
// D-219), written by the independent verifier of 18 Nov 2026 for situations the other page
// tests reach only indirectly: songs with nothing to show, a line with no timed letter, a
// line too long for MAX_ROWS rows at the floor, a single word wider than the area, very
// long instrumental breaks, one-word lines closer together than 2 s, a tiny lyrics area,
// and the renderer driven the way the app drives it (seeking back and forth, pausing on
// one frame, a pixel-ratio change, colours and the needle switched while running).
//
// Made-up words only; problem strings are redacted before they are compared.
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { lyricsFaceMeasure } from "./lyrics-face-widths";
import { INK_PAD_EM, layoutPage, MAX_ROWS, NEEDLE_HEAD_H_EM, NEEDLE_OVER_EM, NEEDLE_W_EM, PAGE_MIN_SCALE, type PageLayout, pageFrame, TRANSITION_S } from "./page";
import { PageRenderer, type PageStyle } from "./pagerenderer";
import { NEEDLE_TOLERANCE_PX, verifyPage } from "./pageverify";
import { shimMeasure } from "./testsongs-node";

const measure = lyricsFaceMeasure(shimMeasure, 800);
const AREA = { width: 1208, height: 450, fontPx: 56 };
const redact = (s: string) => s.replace(/"[^"]*"/g, "[text]");
const MARKER = "#7dffd8";

function lineOf(words: [string, number, number][]): Lyrics["lines"][number] {
  const ws = words.map(([text, start, end]) => {
    const letters = [...text];
    const d = (end - start) / letters.length;
    return { start, end, chars: letters.map((c, k) => [c, start + k * d, start + (k + 1) * d] as [string, number, number]) };
  });
  return { start: ws[0]?.start ?? 0, end: ws[ws.length - 1]?.end ?? 0, words: ws };
}
const song = (...lines: Lyrics["lines"]): Lyrics => ({ lyrics_version: 1, language: "en", source: "user_text", checked: true, lines });
const said = (text: string, t0: number, dur = 0.3): Lyrics["lines"][number] => lineOf(text.split(" ").map((w, k) => [w, t0 + k * dur, t0 + (k + 1) * dur]));
const syllables = ["ba", "do", "ke", "ri", "mo", "ta", "lu", "ne", "zo", "pi", "sa", "fe"];
const madeUp = (n: number, seed = 1) =>
  Array.from({ length: n }, (_, k) => syllables[(k * 7 + seed) % syllables.length] + syllables[(k * 5 + seed * 3) % syllables.length] + syllables[(k + seed) % syllables.length].slice(0, 1)).join(" ");

/** verifyPage at 60 fps; returns the needle problems (redacted) and the report. */
function needleCheck(l: PageLayout) {
  const r = verifyPage(l, { frameDt: 1 / 60 });
  return { r, issues: r.firstProblems.filter((p) => p.includes("needle")).map(redact) };
}

describe("page display edge cases: the needle's contract", () => {
  it("a song with no lines, or only lines without letters, shows nothing and has no needle", () => {
    for (const lyr of [song(), song({ start: 1, end: 2, words: [] }, { start: 3, end: 4, words: [{ start: 3, end: 4, chars: [] }] })]) {
      const l = layoutPage(lyr, AREA, measure);
      expect(l.lines.length).toBe(0);
      for (let t = -2; t < 20; t += 0.25) {
        const f = pageFrame(l, t);
        expect(f.needle).toBeNull();
        expect(f.items).toEqual([]);
      }
      expect(verifyPage(l).needleProblems).toBe(0);
    }
  });

  it("a line with no timed letter (all zero-length) between sung lines: the needle jumps from its start to its end at its time, never backwards", () => {
    const untimed: Lyrics["lines"][number] = { start: 4, end: 4, words: [{ start: 4, end: 4, chars: [..."zozo"].map((c) => [c, 4, 4] as [string, number, number]) }, { start: 4, end: 4, chars: [..."pika"].map((c) => [c, 4, 4] as [string, number, number]) }] };
    for (const pace of ["word", "letter"] as const) {
      const l = layoutPage(song(said("bado keri mota", 1), untimed, said("lune zopi safe", 5.5)), { ...AREA, pace }, measure);
      expect(l.lines.length).toBeGreaterThanOrEqual(2);
      const { r, issues } = needleCheck(l);
      expect(issues).toEqual([]);
      expect(r.needleProblems).toBe(0);
      expect(r.needleFrames).toBeGreaterThan(100);
    }
  });

  it("a line too long for three rows at the floor: drawn smaller on three rows; the needle visits rows 1, 2, 3 in order, one jump each, inside the area", () => {
    const text = madeUp(64);
    const l = layoutPage(song(said(text, 2, 0.25)), AREA, measure);
    const L = l.lines[0];
    expect(L.rows.length).toBe(MAX_ROWS);
    expect(L.px).toBeLessThan(PAGE_MIN_SCALE * AREA.fontPx);
    const rowsSeen: number[] = [];
    let prev: { row: number; x: number } | null = null;
    for (let t = L.start - 1; t < L.end + 1; t += 1 / 120) {
      const n = pageFrame(l, t).needle!;
      expect(n).not.toBeNull();
      if (rowsSeen[rowsSeen.length - 1] !== n.row) rowsSeen.push(n.row);
      if (prev && prev.row === n.row) expect(n.x).toBeGreaterThanOrEqual(prev.x - 1e-9);
      // Inside the area, head included.
      expect(n.top - NEEDLE_HEAD_H_EM * n.px).toBeGreaterThanOrEqual(-0.5);
      expect(n.bottom).toBeLessThanOrEqual(l.height + 0.5);
      prev = { row: n.row, x: n.x };
    }
    expect(rowsSeen).toEqual([0, 1, 2]);
    const { r, issues } = needleCheck(l);
    expect(issues).toEqual([]);
    expect(r.overflowFrames).toBe(0);
  });

  it("a single word wider than the area: shrunk until it fits, the needle on its edge", () => {
    const word = "bado".repeat(30);
    const l = layoutPage(song(said("ke ri", 1), lineOf([[word, 3, 6]]), said("mo ta", 7)), AREA, measure);
    const L = l.lines.find((x) => x.text.length > 100)!;
    expect(L.rows.length).toBe(1);
    expect(L.width).toBeLessThanOrEqual(AREA.width);
    const { r, issues } = needleCheck(l);
    expect(issues).toEqual([]);
    expect(r.overflowFrames).toBe(0);
    expect(r.needleMaxDxPx).toBeLessThanOrEqual(NEEDLE_TOLERANCE_PX);
  });

  it("very long instrumental breaks (10 min, and a 30 min intro): no needle through the countdown, back with the line", () => {
    const l = layoutPage(song(said("bado keri", 1800), said("mota lune", 1802), said("zopi safe", 2402)), AREA, measure);
    expect(l.rests.length).toBe(2);
    for (const rest of l.rests) {
      let frames = 0;
      // (The countdown's opacity is 0 at the rest's very first instant.)
      const from = Number.isFinite(rest.from) ? rest.from + 0.01 : -5;
      for (let t = from; t < rest.to; t += 0.5) {
        const f = pageFrame(l, t);
        expect(f.needle).toBeNull();
        expect(f.countdown).not.toBeNull();
        frames++;
      }
      expect(frames).toBeGreaterThan(1000);
      const back = pageFrame(l, rest.to + TRANSITION_S + 0.01).needle!;
      expect(back.line).toBe(rest.line);
      expect(back.alpha).toBe(1);
      expect(back.x).toBe(l.lines[rest.line].rows[0].left);
    }
    const { r, issues } = needleCheck(l);
    expect(issues).toEqual([]);
    expect(r.needleInRest).toBe(0);
  });

  it("one-word lines closer together than 2 s (joined into phrases) and lines shorter than the change", () => {
    const lines = Array.from({ length: 40 }, (_, k) => said(madeUp(1, k), 2 + k * 0.45, k % 3 === 0 ? 0.1 : 0.35));
    for (const pace of ["word", "letter"] as const) {
      const l = layoutPage(song(...lines), { ...AREA, pace }, measure);
      const { r, issues } = needleCheck(l);
      expect(issues).toEqual([]);
      expect(r.needleProblems).toBe(0);
      expect(r.needleFrames).toBeGreaterThan(500);
    }
  });

  it("without the next line: at the very instant a line change starts the needle is still on the leaving line, and the contract (n) accepts that", () => {
    // Found by the verifier: the entering line fades in from opacity 0, so at t = its
    // `current` it is not drawn and pageFrame has no line in the slot; needleOf then keeps
    // the needle on the leaving line for that instant. pageverify (n) allowed a leaving line
    // only before a rest or at the end, so whenever a sampled frame fell exactly on a change
    // it reported "wrong line" (24 problems here at 60 fps; 3 on the demo song in the release
    // app's own __jkTicker.verify(), whose line times are round). Only that instant: a moment
    // later the needle is on the new line.
    const lines = Array.from({ length: 40 }, (_, k) => said(madeUp(1, k), 2 + k * 0.45, k % 3 === 0 ? 0.1 : 0.35));
    const l = layoutPage(song(...lines), { ...AREA, showNext: false }, measure);
    for (const pace of ["word", "letter"] as const) {
      const lp = layoutPage(song(...lines), { ...AREA, showNext: false, pace }, measure);
      const { r, issues } = needleCheck(lp);
      expect(issues).toEqual([]);
      expect(r.needleProblems).toBe(0);
      expect(r.needleFrames).toBeGreaterThan(500);
    }
    // Round line times (as the demo song's) sampled exactly on the changes at 30 fps.
    const round = layoutPage(song(said("bado keri mota", 1, 0.4), said("lune zopi", 2.5, 0.5), said("safe doke", 4, 0.5), said("rimo talu", 5.5, 0.5)), { ...AREA, showNext: false }, measure);
    const rr = verifyPage(round, { frameDt: 1 / 30 });
    expect(rr.needleProblems).toBe(0);
    let instants = 0;
    for (let i = 1; i < l.lines.length; i++) {
      const L = l.lines[i];
      if (L.breakBefore) continue;
      const at = pageFrame(l, L.current);
      expect(at.current).toBe(-1);
      expect(at.needle?.line).toBe(i - 1);
      expect(at.needle?.role).toBe("leaving");
      instants++;
      const after = pageFrame(l, L.current + 1e-4).needle!;
      expect(after.line).toBe(i);
      expect(after.alpha).toBe(1); // fully opaque on a line that is itself still at opacity ~0
    }
    expect(instants).toBeGreaterThan(30);
  });

  it("a tiny lyrics area (the song shrinks): the needle and its head stay inside", () => {
    const lyr = song(said(madeUp(14, 2), 1, 0.3), said(madeUp(9, 3), 6, 0.4), said(madeUp(20, 4), 11, 0.2));
    for (const area of [
      { width: 320, height: 140, fontPx: 56 },
      { width: 200, height: 90, fontPx: 120 },
    ]) {
      const l = layoutPage(lyr, area, measure);
      const { r, issues } = needleCheck(l);
      expect(issues).toEqual([]);
      expect(r.needleOutside).toBe(0);
      expect(r.overflowFrames).toBe(0);
    }
  });
});

// ---------------------------------------------------------------------------
// The renderer driven as the app drives it (a minimal recording canvas).
// ---------------------------------------------------------------------------

type Op = { k: "img"; fill: unknown; dx: number; dy: number; clip: number[] | null } | { k: "rect"; x: number; w: number; y: number; h: number; style: unknown; alpha: number } | { k: "path"; style: unknown };
class Ctx {
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
  textFill: unknown = null;
  private clipR: number[] | null = null;
  private pending: number[] | null = null;
  private stack: { clip: number[] | null; fill: unknown; alpha: number }[] = [];
  setTransform(): void {}
  clearRect(): void {
    this.log = [];
  }
  save(): void {
    this.stack.push({ clip: this.clipR, fill: this.fillStyle, alpha: this.globalAlpha });
  }
  restore(): void {
    const s = this.stack.pop()!;
    [this.clipR, this.fillStyle, this.globalAlpha] = [s.clip, s.fill, s.alpha];
  }
  beginPath(): void {}
  rect(x: number, y: number, w: number, h: number): void {
    this.pending = [x, y, x + w, y + h];
  }
  clip(): void {
    this.clipR = this.pending;
  }
  moveTo(): void {}
  lineTo(): void {}
  closePath(): void {}
  fill(): void {
    this.log.push({ k: "path", style: this.fillStyle });
  }
  drawImage(img: Canvas, dx: number, dy: number): void {
    this.log.push({ k: "img", fill: img.ctx.textFill, dx, dy, clip: this.clipR });
  }
  fillRect(x: number, y: number, w: number, h: number): void {
    this.log.push({ k: "rect", x, y, w, h, style: this.fillStyle, alpha: this.globalAlpha });
  }
  fillText(): void {
    if (this.textFill === null) this.textFill = this.fillStyle;
  }
  strokeText(): void {}
  measureText(s: string) {
    return { width: s.length * 10, actualBoundingBoxAscent: 0 };
  }
  createLinearGradient() {
    return { addColorStop() {} };
  }
}
class Canvas {
  width = 0;
  height = 0;
  ctx = new Ctx();
  getContext(): Ctx {
    return this.ctx;
  }
}
const STYLE: PageStyle = { fontFamily: "test", fontWeight: "800", sung: "#ffc21a", upcoming: "#f4f4f4", outline: "#000", shadow: "rgba(0,0,0,0.5)" };
function renderer(l: PageLayout, dpr = 1, marker: string | null = MARKER) {
  const main = new Canvas();
  const r = new PageRenderer(main as unknown as HTMLCanvasElement, () => new Canvas() as unknown as HTMLCanvasElement);
  r.setSize(l.width, l.height, dpr);
  r.setLayout(l, STYLE);
  r.setMarker(marker);
  return { r, ctx: main.ctx };
}
const needleOps = (ctx: Ctx, colour = MARKER) => ctx.log.filter((o) => (o.k === "rect" || o.k === "path") && o.style === colour);
/** The frame as drawn, independent of which canvas objects were used. */
const snapshot = (ctx: Ctx) => JSON.stringify(ctx.log);

describe("page display edge cases: the renderer as the app drives it", () => {
  const lyr = song(said(madeUp(30, 5), 1, 0.3), said(madeUp(6, 6), 11, 0.5), said(madeUp(5, 7), 20, 0.4), said(madeUp(40, 8), 24, 0.22), said(madeUp(4, 9), 36, 0.6));

  it("seeking back and forth draws exactly what a fresh renderer draws at that time (no state carried over)", () => {
    const l = layoutPage(lyr, AREA, measure);
    const a = renderer(l);
    let seed = 12345;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
    let compared = 0;
    let withNeedle = 0;
    for (let k = 0; k < 400; k++) {
      const t = -2 + rnd() * 44;
      a.r.draw(t);
      const b = renderer(l);
      b.r.draw(t);
      expect(snapshot(a.ctx)).toBe(snapshot(b.ctx));
      expect(a.r.lastNeedle).toEqual(b.r.lastNeedle);
      if (needleOps(a.ctx).length) withNeedle++;
      compared++;
    }
    expect(compared).toBe(400);
    expect(withNeedle).toBeGreaterThan(200);
  });

  it("paused on one lyric time: every redraw is identical, needle included", () => {
    const l = layoutPage(lyr, AREA, measure);
    const { r, ctx } = renderer(l);
    const t = l.lines[0].words[3].start + 0.1;
    r.draw(t);
    const first = snapshot(ctx);
    expect(needleOps(ctx).length).toBe(2);
    for (let i = 0; i < 5; i++) {
      r.draw(t);
      expect(snapshot(ctx)).toBe(first);
    }
  });

  it("a pixel-ratio change while running (window moved to another screen): the needle stays on the row split", () => {
    const l = layoutPage(lyr, AREA, measure);
    const { r } = renderer(l, 1);
    let checked = 0;
    for (const dpr of [1, 1.25, 2, 1.5]) {
      r.setSize(l.width, l.height, dpr);
      for (let t = 1; t < 40; t += 0.37) {
        r.draw(t);
        const n = r.lastNeedle;
        if (n && Number.isFinite(n.split)) {
          expect(Math.abs(n.x - n.split)).toBeLessThan(1e-9);
          checked++;
        }
      }
    }
    expect(checked).toBeGreaterThan(100);
  });

  it("new colours (a new style) and the needle switched off and on while running: the needle keeps the marker colour and its place", () => {
    const l = layoutPage(lyr, AREA, measure);
    const { r, ctx } = renderer(l);
    const t = l.lines[0].words[5].start + 0.05;
    r.draw(t);
    const before = r.lastNeedle!;
    r.setLayout(l, { ...STYLE, sung: "#00ff00", upcoming: "#ffffff" });
    r.draw(t);
    expect(needleOps(ctx).length).toBe(2);
    expect(r.lastNeedle).toEqual(before);
    r.setMarker(null);
    r.draw(t);
    expect(needleOps(ctx).length).toBe(0);
    expect(r.lastNeedle).toBeNull();
    r.setMarker("#123456");
    r.draw(t);
    expect(needleOps(ctx, "#123456").length).toBe(2);
    expect(r.lastNeedle).toEqual(before);
  });

  it("a song with no lines draws nothing and does not throw", () => {
    const l = layoutPage(song(), AREA, measure);
    const { r, ctx } = renderer(l);
    for (let t = -1; t < 5; t += 0.5) {
      expect(() => r.draw(t)).not.toThrow();
      expect(ctx.log).toEqual([]);
      expect(r.lastNeedle).toBeNull();
    }
  });

  it("the needle's shaft is the ticker's width at the line's size and is centred on the boundary at every pixel ratio", () => {
    const l = layoutPage(lyr, AREA, measure);
    for (const dpr of [1, 2]) {
      const { r, ctx } = renderer(l, dpr);
      for (let t = 1; t < 40; t += 0.5) {
        const f = r.draw(t)!;
        if (!f.needle) continue;
        const shaft = ctx.log.find((o): o is Extract<Op, { k: "rect" }> => o.k === "rect" && o.style === MARKER)!;
        expect(shaft.w).toBeCloseTo(NEEDLE_W_EM * f.needle.px, 9);
        expect(shaft.x + shaft.w / 2).toBeCloseTo(r.lastNeedle!.x, 9);
        // Head and overshoot stay within the row's ink pad above its band.
        const bandTop = f.needle.top + NEEDLE_OVER_EM * f.needle.px;
        expect(bandTop - (f.needle.top - NEEDLE_HEAD_H_EM * f.needle.px)).toBeLessThanOrEqual(INK_PAD_EM * f.needle.px + 1e-9);
      }
    }
  });
});

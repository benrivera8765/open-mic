// SPDX-License-Identifier: GPL-3.0-or-later
// Canvas 2D renderer of the practice lane (lane.ts), in the jukebox look (docs/DESIGN.md,
// WO2 4.5): bars are glowing light tubes (the palette's pink), lit parts gold (the "sung"
// colour), the marker mint (the "marker" colour), the keys cream and black glass with a
// chrome edge, the "now" line a chrome needle. Everything is drawn in code.
//
// Drawn once, moved every frame (WO2 trap 13, the fanless MacBook Air):
//   - bars: tiles of TILE_PX CSS px of time, drawn at key 0 with their glow, blitted at an
//     x that moves with time and a y offset that carries the key heard, the octave and the
//     view (a key change or a slide redraws nothing);
//   - words: tiles of the word strip, in the upcoming colour and in the sung colour; the
//     sung copy is blitted left of the now line;
//   - the faint row per key: one octave of rows, blitted as often as the view needs;
//   - the piano keys: one bitmap for the rows around the view (redrawn only when the view
//     leaves it or the spelling changes);
//   - the needle, the marker head, the edge arrows, the lit-tube sprite, the panel: sprites.
// Per frame: the blits, the lit parts (3-slice sprite), the two lit keys, the tail (two
// strokes), a cue pill and in rests the countdown. Under reduce-motion: no tail, no glow
// animation, a plain marker; the simple lane drops the tail and every glow.
//
// Device pixels: every cached bitmap is blitted 1:1 onto whole device pixels. A filtered
// (fractional) blit costs about 3x more to rasterise on a CPU-drawn canvas (1440x900:
// 1.45 -> 0.43 ms a frame; Retina 5.2 -> 1.4 ms; docs/benchmarks/practice-screen.md). The
// row height is a whole, even number of device pixels (lane.ts), bar tiles and the piano
// bitmap put pitch rows on whole device pixels, and one rounding per frame (`dy`, from the
// top edge of row 0) moves the bars, the key rows and the piano keys together, so a bar
// stays exactly on its key row; the time axis rounds once per frame too (`dx`). What moves
// is thus at most half a device pixel from the exact frame of lane.ts (checked by
// lanerenderer.test.ts); the marker and the tail are drawn exactly, the lit parts with the
// same dx / dy as the bars under them.
import { formatCountdown } from "../lib/format";
import type { TickerStats } from "../ticker/renderer";
import { firstNoteFrom, firstWordFrom, type LaneFrame, type LaneInputs, type LaneLayout, laneFrame, xOf, yOf } from "./lane";
import { displayName, isBlackKey, noteName } from "./pitch";

export interface LaneStyle {
  /** Lit bars, sung words, the key to sing (settings.ui.colors.sung, gold). */
  sung: string;
  /** The marker and the singer's key (settings.ui.colors.marker, mint). */
  marker: string;
  /** Words still to sing (settings.ui.colors.upcoming). */
  upcoming: string;
  /** The tubes (the palette's pink). */
  bar: string;
  wordFont: string;
  wordWeight: string;
  /** Key names, the countdown and the cue (the display face). */
  labelFont: string;
}

/** Width of a bar or word tile, CSS px. */
export const TILE_PX = 512;
const KEEP_BAR_TILES = 14;
const KEEP_WORD_TILES = 28;

/** One drawImage of a cached bitmap, for the tests (`debugLog`). */
export interface BlitEntry {
  kind: "bars" | "words-up" | "words-sung" | "rows" | "piano";
  tile: number;
  sx: number;
  sy: number;
  sw: number;
  sh: number;
  dx: number;
  dy: number;
  dw: number;
  dh: number;
}

type Ctx = CanvasRenderingContext2D;

/** A capsule (rounded bar) path, drawn with arcs (no roundRect needed). */
function capsule(ctx: Ctx, x0: number, x1: number, yc: number, h: number): void {
  const r = Math.min(h / 2, Math.max(0.5, (x1 - x0) / 2));
  const w = Math.max(x1 - x0, 2 * r);
  ctx.beginPath();
  ctx.moveTo(x0 + r, yc - h / 2);
  ctx.lineTo(x0 + w - r, yc - h / 2);
  ctx.arc(x0 + w - r, yc, h / 2, -Math.PI / 2, Math.PI / 2);
  ctx.lineTo(x0 + r, yc + h / 2);
  ctx.arc(x0 + r, yc, h / 2, Math.PI / 2, (3 * Math.PI) / 2);
  ctx.closePath();
}

function roundRect(ctx: Ctx, x: number, y: number, w: number, h: number, r: number): void {
  const rr = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.arcTo(x + w, y, x + w, y + rr, rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.arcTo(x + w, y + h, x + w - rr, y + h, rr);
  ctx.lineTo(x + rr, y + h);
  ctx.arcTo(x, y + h, x, y + h - rr, rr);
  ctx.lineTo(x, y + rr);
  ctx.arcTo(x, y, x + rr, y, rr);
  ctx.closePath();
}

/** "#ff3d8b" -> "rgba(255,61,139,a)" (any #rgb / #rrggbb; anything else is returned as is). */
export function withAlpha(color: string, a: number): string {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return color;
  const hex = m[1].length === 3 ? m[1].replace(/./g, (c) => c + c) : m[1];
  const n = parseInt(hex, 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}

/** Mixes a colour towards white (k > 0) or black (k < 0). */
function shade(color: string, k: number): string {
  const m = /^#([0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return color;
  const n = parseInt(m[1], 16);
  const ch = (v: number) => Math.round(k >= 0 ? v + (255 - v) * k : v * (1 + k));
  return `rgb(${ch((n >> 16) & 255)},${ch((n >> 8) & 255)},${ch(n & 255)})`;
}

export class LaneRenderer {
  private ctx: Ctx;
  private cssW = 0;
  private cssH = 0;
  private dpr = 1;
  private L: LaneLayout | null = null;
  private style: LaneStyle | null = null;
  private lyrics = new Map<number, number>();
  private simple = false;
  private barTiles = new Map<number, { c: HTMLCanvasElement; used: number }>();
  private wordTiles = new Map<string, { c: HTMLCanvasElement; used: number }>();
  private rowsBmp: HTMLCanvasElement | null = null;
  private panelBmp: HTMLCanvasElement | null = null;
  private pianoBmp: { c: HTMLCanvasElement; lo: number; hi: number; spelling: string } | null = null;
  private needleBmp: HTMLCanvasElement | null = null;
  private headBmp: { glow: HTMLCanvasElement; plain: HTMLCanvasElement; pad: number } | null = null;
  private arrowBmp: { up: HTMLCanvasElement; down: HTMLCanvasElement; pad: number } | null = null;
  private litBmp: { c: HTMLCanvasElement; cap: number; pad: number } | null = null;
  /** Lit piano keys (colour x black / white x half), drawn once with their glow. */
  private keyBmps = new Map<string, { c: HTMLCanvasElement; pad: number }>();
  private frameNo = 0;
  /** This frame's rounding of the time axis and of the pitch rows (see "Device pixels"). */
  private dx = 0;
  private dy = 0;
  readonly stats: TickerStats = { frames: 0, tilesRendered: 0, lastDrawMs: 0, maxDrawMs: 0, totalDrawMs: 0 };
  /** The last frame drawn (debug hooks, the screen checks). */
  last: LaneFrame | null = null;
  /** When an array: every cached-bitmap blit of the next frames is recorded (tests). */
  debugLog: BlitEntry[] | null = null;

  constructor(
    private canvas: HTMLCanvasElement,
    private makeCanvas: () => HTMLCanvasElement = () => document.createElement("canvas"),
  ) {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) throw new Error("Canvas 2D is not available");
    this.ctx = ctx as Ctx;
  }

  setSize(cssW: number, cssH: number, dpr: number): void {
    const w = Math.max(1, Math.round(cssW * dpr));
    const h = Math.max(1, Math.round(cssH * dpr));
    if (this.canvas.width !== w) this.canvas.width = w;
    if (this.canvas.height !== h) this.canvas.height = h;
    if (dpr !== this.dpr || cssW !== this.cssW || cssH !== this.cssH) this.clearCaches();
    this.cssW = cssW;
    this.cssH = cssH;
    this.dpr = dpr;
  }

  /** New layout, style or look: every bitmap is made again lazily. */
  setLayout(L: LaneLayout, style: LaneStyle, simple: boolean): void {
    this.L = L;
    this.style = style;
    this.simple = simple;
    this.clearCaches();
  }

  private clearCaches(): void {
    this.barTiles.clear();
    this.wordTiles.clear();
    this.rowsBmp = this.panelBmp = this.needleBmp = null;
    this.pianoBmp = null;
    this.headBmp = this.arrowBmp = this.litBmp = null;
    this.keyBmps.clear();
    this.lyrics.clear();
  }

  private canvasOf(cssW: number, cssH: number): { c: HTMLCanvasElement; g: Ctx } {
    const c = this.makeCanvas();
    c.width = Math.max(1, Math.ceil(cssW * this.dpr));
    c.height = Math.max(1, Math.ceil(cssH * this.dpr));
    const g = c.getContext("2d") as Ctx;
    g.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    return { c, g };
  }

  // --- tile geometry (shared by drawing and blitting) ------------------------------------

  /** v rounded to whole device pixels. */
  private snap(v: number): number {
    return Math.round(v * this.dpr) / this.dpr;
  }
  /** Glow room around a bar inside a tile, CSS px (whole device pixels). */
  private pad(): number {
    return Math.ceil((this.L!.barH * 1.1 + 4) * this.dpr) / this.dpr;
  }
  /** The lane pitch (whole) at a bar tile's top edge (+ pad): above the melody's highest note. */
  private tileTop(): number {
    return Math.ceil(this.L!.hi) + 2;
  }
  /** Height of a bar tile, CSS px (whole device pixels: rowH and pad are). */
  private tileH(): number {
    const L = this.L!;
    return (this.tileTop() - (Math.floor(L.lo) - 2)) * L.rowH + 2 * this.pad();
  }
  /** y of lane pitch m (key 0) inside a bar tile, CSS px. */
  tileY(m: number): number {
    return this.pad() + (this.tileTop() - m) * this.L!.rowH;
  }
  /** Seconds a tile covers. */
  private tileS(): number {
    return TILE_PX / this.L!.pxPerSec;
  }

  // --- bitmaps ----------------------------------------------------------------------------

  /**
   * A glass tube with its glow. Drawn once per tile (never per frame), so the shadow blur
   * costs only when a tile is made (measured: no difference in the CPU-raster bench
   * against a layered glow, which looked banded).
   */
  private tube(g: Ctx, x0: number, x1: number, yc: number, h: number, color: string, glow: boolean): void {
    if (glow) {
      g.shadowColor = withAlpha(color, 0.85);
      g.shadowBlur = h * 0.95;
    }
    const grad = g.createLinearGradient(0, yc - h / 2, 0, yc + h / 2);
    grad.addColorStop(0, shade(color, -0.45));
    grad.addColorStop(0.28, color);
    grad.addColorStop(0.5, shade(color, 0.55));
    grad.addColorStop(0.72, color);
    grad.addColorStop(1, shade(color, -0.5));
    g.fillStyle = grad;
    capsule(g, x0, x1, yc, h);
    g.fill();
    g.shadowBlur = 0;
    g.shadowColor = "transparent";
  }

  /** One bar tile: every note touching it, at key 0, with glow (none in the simple lane). */
  private barTile(k: number): HTMLCanvasElement {
    const hit = this.barTiles.get(k);
    if (hit) {
      hit.used = this.frameNo;
      return hit.c;
    }
    const L = this.L!;
    const st = this.style!;
    const { c, g } = this.canvasOf(TILE_PX, this.tileH());
    const t0 = k * this.tileS();
    const pps = L.pxPerSec;
    const pad = this.pad();
    const x = (tau: number) => (tau - t0) * pps;
    const h = L.barH;
    const glow = !this.simple;
    const from = firstNoteFrom(L, t0 - pad / pps);
    const until = t0 + (TILE_PX + pad) / pps;
    const drawable = (n: (typeof L.notes)[number]) => n.kind === "pitched" && Number.isFinite(n.lane);
    // A slide joins note j-1 to note j (a scoop the guide measured, notes close together).
    const joined = (j: number) => {
      if (j <= 0 || j >= L.notes.length) return false;
      const n = L.notes[j];
      const p = L.notes[j - 1];
      return n.glideIn > 0 && drawable(n) && drawable(p) && n.start - p.end < 0.12 && n.lane !== p.lane;
    };
    // Bars and slides as one neon tube (owner 2026-11-22: the slide must look "seamless",
    // "a smooth transition", not a separate bar): each run of joined notes is ONE path,
    // flat along each note (cap centre to cap centre) and an S-curve between them that
    // leaves and meets the bars level, stroked in layers (glow, rim, body, bright centre,
    // white core). A lone note is the same path with no curve, so every bar looks alike.
    const chains: number[][] = [];
    let j = from;
    while (j > 0 && joined(j)) j--; // start a run that began left of this tile
    for (; j < L.notes.length && L.notes[j].start < until; j++) {
      if (!drawable(L.notes[j])) continue;
      const run = [j];
      while (j + 1 < L.notes.length && joined(j + 1)) run.push(++j);
      chains.push(run);
    }
    const trace = (run: number[]) => {
      g.beginPath();
      run.forEach((i, q) => {
        const n = L.notes[i];
        const x0 = x(n.start);
        const x1 = x(n.end);
        const r = Math.min(h / 2, Math.max(0, (x1 - x0) / 2));
        const y = this.tileY(n.lane);
        const a = x0 + r;
        const b = Math.max(a, x1 - r);
        if (q === 0) g.moveTo(a, y);
        else {
          const pn = L.notes[run[q - 1]];
          const px = Math.max(x(pn.start) + Math.min(h / 2, (x(pn.end) - x(pn.start)) / 2), x(pn.end) - Math.min(h / 2, (x(pn.end) - x(pn.start)) / 2));
          const py = this.tileY(pn.lane);
          const dx = Math.max(1, a - px);
          g.bezierCurveTo(px + dx * 0.5, py, a - dx * 0.5, y, a, y);
        }
        g.lineTo(b + 0.01, y);
      });
    };
    g.lineCap = "round";
    g.lineJoin = "round";
    const layers: [number, string, number][] = [
      [1, shade(st.bar, -0.45), 1], // rim (dark edge of the glass)
      [0.72, st.bar, 1], // body
      [0.4, shade(st.bar, 0.55), 1], // bright centre
    ];
    for (const run of chains) {
      layers.forEach(([w, color, alpha], li) => {
        if (li === 0 && glow) {
          g.shadowColor = withAlpha(st.bar, 0.85);
          g.shadowBlur = h * 0.95;
        }
        g.globalAlpha = alpha;
        g.strokeStyle = color;
        g.lineWidth = h * w;
        trace(run);
        g.stroke();
        g.shadowBlur = 0;
        g.shadowColor = "transparent";
      });
      // The white core, along the whole run.
      g.globalAlpha = 0.92;
      g.strokeStyle = "rgba(255,246,250,0.95)";
      g.lineWidth = Math.max(1.2, h * 0.16);
      trace(run);
      g.stroke();
      // Specular line along the top of each flat part.
      g.globalAlpha = 0.4;
      g.strokeStyle = "#ffffff";
      g.lineWidth = Math.max(1, h * 0.09);
      g.beginPath();
      for (const i of run) {
        const n = L.notes[i];
        const x0 = x(n.start);
        const x1 = x(n.end);
        const y = this.tileY(n.lane);
        g.moveTo(x0 + h * 0.5, y - h * 0.3);
        g.lineTo(Math.max(x0 + h * 0.5, x1 - h * 0.5), y - h * 0.3);
      }
      g.stroke();
      g.globalAlpha = 1;
    }
    this.barTiles.set(k, { c, used: this.frameNo });
    this.stats.tilesRendered++;
    this.evict(this.barTiles, KEEP_BAR_TILES);
    return c;
  }

  /** One word tile in the upcoming or the sung colour (spoken spans marked along its top). */
  private wordTile(k: number, sung: boolean): HTMLCanvasElement {
    const key = `${k}:${sung ? 1 : 0}`;
    const hit = this.wordTiles.get(key);
    if (hit) {
      hit.used = this.frameNo;
      return hit.c;
    }
    const L = this.L!;
    const st = this.style!;
    const stripH = Math.max(1, L.wordBottom - L.wordTop + L.wordPx * 0.6);
    const { c, g } = this.canvasOf(TILE_PX, stripH);
    const t0 = k * this.tileS();
    const pps = L.pxPerSec;
    const x = (tau: number) => (tau - t0) * pps;
    const line = Math.round(L.wordPx * 1.35);
    g.font = `${st.wordWeight} ${L.wordPx}px ${st.wordFont}`;
    g.textBaseline = "alphabetic";
    g.lineJoin = "round";
    g.lineWidth = Math.max(2, L.wordPx * 0.2);
    g.strokeStyle = "rgba(8,6,14,0.92)";
    g.fillStyle = sung ? st.sung : st.upcoming;
    const maxW = 900;
    for (let j = firstWordFrom(L, t0 - maxW / pps); j < L.words.length && L.words[j].start < t0 + TILE_PX / pps; j++) {
      const wd = L.words[j];
      const wx = x(wd.start) + wd.shift;
      if (wx + wd.w < 0) continue;
      const by = wd.row * line + L.wordPx * 1.02;
      g.strokeText(wd.text, wx, by);
      g.fillText(wd.text, wx, by);
    }
    // Spoken spans: a dashed violet line over the words (WO2 owner decision 10: "speak" parts).
    for (const sp of L.spans) {
      if (sp.kind !== "spoken" || sp.end < t0 || sp.start > t0 + TILE_PX / pps) continue;
      g.strokeStyle = "rgba(176,140,255,0.85)";
      g.lineWidth = Math.max(1.5, L.wordPx * 0.1);
      g.setLineDash?.([L.wordPx * 0.4, L.wordPx * 0.3]);
      g.beginPath();
      g.moveTo(x(sp.start), 1.5);
      g.lineTo(x(sp.end), 1.5);
      g.stroke();
      g.setLineDash?.([]);
    }
    this.wordTiles.set(key, { c, used: this.frameNo });
    this.stats.tilesRendered++;
    this.evict(this.wordTiles, KEEP_WORD_TILES);
    return c;
  }

  private evict<K>(m: Map<K, { used: number }>, keep: number): void {
    if (m.size <= keep) return;
    let oldK: K | null = null;
    let old = Infinity;
    for (const [k, v] of m) if (v.used < old) [old, oldK] = [v.used, k];
    if (oldK !== null) m.delete(oldK);
  }

  /** One octave of faint rows (C at the bottom), as wide as the bar area. */
  private rows(): HTMLCanvasElement {
    if (this.rowsBmp) return this.rowsBmp;
    const L = this.L!;
    const { c, g } = this.canvasOf(L.barW, 12 * L.rowH);
    for (let pc = 0; pc < 12; pc++) {
      const y = (11 - pc) * L.rowH;
      g.fillStyle = isBlackKey(pc) ? "rgba(0,0,0,0.26)" : pc === 0 ? "rgba(255,240,220,0.06)" : "rgba(255,240,220,0.03)";
      g.fillRect(0, y, L.barW, L.rowH);
    }
    // Hairline boundaries between neighbouring white keys (B|C and E|F), as on a keyboard.
    g.fillStyle = "rgba(255,240,220,0.09)";
    g.fillRect(0, 0, L.barW, 1);
    g.fillRect(0, (11 - 4) * L.rowH, L.barW, 1);
    this.rowsBmp = c;
    return c;
  }

  /** The dark glass panel behind the bars and words, with a chrome-lit rim. */
  private panel(): HTMLCanvasElement {
    if (this.panelBmp) return this.panelBmp;
    const L = this.L!;
    const { c, g } = this.canvasOf(this.cssW, this.cssH);
    const x = L.barX;
    const w = L.barW;
    const grad = g.createLinearGradient(0, 0, 0, this.cssH);
    grad.addColorStop(0, "#1d1529");
    grad.addColorStop(0.55, "#120d1b");
    grad.addColorStop(1, "#0c0912");
    g.fillStyle = grad;
    roundRect(g, x, 0, w, this.cssH, Math.min(14, this.cssH * 0.05));
    g.fill();
    // Glass sheen across the top.
    const sheen = g.createLinearGradient(0, 0, 0, this.cssH * 0.35);
    sheen.addColorStop(0, "rgba(255,255,255,0.07)");
    sheen.addColorStop(1, "rgba(255,255,255,0)");
    g.fillStyle = sheen;
    g.fillRect(x, 0, w, this.cssH * 0.35);
    // A thin chrome line under the bars, above the words.
    if (L.wordRows > 0) {
      const cl = g.createLinearGradient(x, 0, x + w, 0);
      cl.addColorStop(0, "rgba(200,206,216,0)");
      cl.addColorStop(0.15, "rgba(220,226,236,0.55)");
      cl.addColorStop(0.85, "rgba(220,226,236,0.55)");
      cl.addColorStop(1, "rgba(200,206,216,0)");
      g.fillStyle = cl;
      g.fillRect(x, L.barBottom + 1, w, 1);
    }
    this.panelBmp = c;
    return c;
  }

  /** The chrome needle of the now line (with a soft halo unless simple). */
  private needle(): HTMLCanvasElement {
    if (this.needleBmp) return this.needleBmp;
    const L = this.L!;
    const s = L.params.scale;
    const w = Math.max(4, 5 * s);
    const capH = Math.max(6, 8 * s);
    const halo = this.simple ? 0 : Math.max(6, 10 * s);
    const H = L.barBottom - L.barTop + 2 * capH;
    const W = w * 3 + 2 * halo;
    const { c, g } = this.canvasOf(W, H + 2);
    const cx = W / 2;
    if (halo) {
      const hg = g.createLinearGradient(cx - halo - w, 0, cx + halo + w, 0);
      hg.addColorStop(0, "rgba(220,240,255,0)");
      hg.addColorStop(0.5, "rgba(220,240,255,0.16)");
      hg.addColorStop(1, "rgba(220,240,255,0)");
      g.fillStyle = hg;
      g.fillRect(cx - halo - w, capH, 2 * (halo + w), H - 2 * capH);
    }
    const chrome = g.createLinearGradient(cx - w / 2, 0, cx + w / 2, 0);
    chrome.addColorStop(0, "#4b515b");
    chrome.addColorStop(0.3, "#f5f8fc");
    chrome.addColorStop(0.55, "#9aa3ae");
    chrome.addColorStop(0.8, "#ffffff");
    chrome.addColorStop(1, "#3f444c");
    g.fillStyle = chrome;
    g.fillRect(cx - w / 2, capH * 0.6, w, H - capH * 1.2);
    // Needle caps (pointing in) top and bottom.
    for (const [y0, dir] of [
      [0, 1],
      [H, -1],
    ] as const) {
      g.beginPath();
      g.moveTo(cx - w * 1.5, y0);
      g.lineTo(cx + w * 1.5, y0);
      g.lineTo(cx, y0 + dir * capH);
      g.closePath();
      g.fill();
    }
    this.needleBmp = c;
    return c;
  }

  /** The marker head: a bold arrow pointing at the notes, glowing (and a plain one). */
  private head(): { glow: HTMLCanvasElement; plain: HTMLCanvasElement; pad: number } {
    if (this.headBmp) return this.headBmp;
    const L = this.L!;
    const st = this.style!;
    const hh = L.headH;
    const hw = L.headW;
    const pad = Math.ceil(hh * 0.8);
    const make = (glow: boolean) => {
      const { c, g } = this.canvasOf(hw + 2 * pad, hh + 2 * pad);
      const x0 = pad;
      const y0 = pad;
      const path = () => {
        const r = hh / 2;
        g.beginPath();
        g.moveTo(x0 + r, y0);
        g.lineTo(x0 + hw * 0.58, y0);
        g.lineTo(x0 + hw, y0 + r);
        g.lineTo(x0 + hw * 0.58, y0 + hh);
        g.lineTo(x0 + r, y0 + hh);
        g.arc(x0 + r, y0 + r, r, Math.PI / 2, (3 * Math.PI) / 2);
        g.closePath();
      };
      if (glow) {
        g.shadowColor = withAlpha(st.marker, 0.95);
        g.shadowBlur = hh * 0.75;
      }
      const grad = g.createLinearGradient(0, y0, 0, y0 + hh);
      grad.addColorStop(0, shade(st.marker, 0.65));
      grad.addColorStop(0.45, st.marker);
      grad.addColorStop(1, shade(st.marker, -0.35));
      g.fillStyle = grad;
      path();
      g.fill();
      g.shadowBlur = 0;
      g.shadowColor = "transparent";
      g.lineWidth = Math.max(1.5, hh * 0.08);
      g.strokeStyle = "rgba(6,24,20,0.9)";
      path();
      g.stroke();
      // A white-hot core.
      g.fillStyle = "rgba(255,255,255,0.75)";
      capsule(g, x0 + hh * 0.3, x0 + hw * 0.62, y0 + hh * 0.42, hh * 0.22);
      g.fill();
      return c;
    };
    this.headBmp = { glow: make(!this.simple), plain: make(false), pad };
    return this.headBmp;
  }

  /** Arrows for a marker pinned at the top or bottom edge (the pitch is out of the lane's range). */
  private arrows(): { up: HTMLCanvasElement; down: HTMLCanvasElement; pad: number } {
    if (this.arrowBmp) return this.arrowBmp;
    const L = this.L!;
    const st = this.style!;
    const s = L.headH;
    const pad = Math.ceil(s * 0.5);
    const make = (up: boolean) => {
      const { c, g } = this.canvasOf(s + 2 * pad, s + 2 * pad);
      if (!this.simple) {
        g.shadowColor = withAlpha(st.marker, 0.9);
        g.shadowBlur = s * 0.5;
      }
      g.fillStyle = st.marker;
      g.beginPath();
      const y0 = pad + (up ? s : 0);
      const y1 = pad + (up ? 0 : s);
      g.moveTo(pad, y0);
      g.lineTo(pad + s, y0);
      g.lineTo(pad + s / 2, y1);
      g.closePath();
      g.fill();
      g.shadowBlur = 0;
      g.lineWidth = Math.max(1.5, s * 0.08);
      g.strokeStyle = "rgba(6,24,20,0.9)";
      g.stroke();
      return c;
    };
    this.arrowBmp = { up: make(true), down: make(false), pad };
    return this.arrowBmp;
  }

  /** A short lit (gold) tube: caps and a middle column for a 3-slice draw. */
  private lit(): { c: HTMLCanvasElement; cap: number; pad: number } {
    if (this.litBmp) return this.litBmp;
    const L = this.L!;
    const h = L.barH;
    const pad = Math.ceil(h * 1.1 + 2);
    const cap = Math.ceil(h / 2);
    const w = cap * 2 + 4;
    const { c, g } = this.canvasOf(w + 2 * pad, h + 2 * pad);
    this.tube(g, pad, pad + w, pad + h / 2, h, this.style!.sung, !this.simple);
    g.strokeStyle = "rgba(255,252,236,0.95)";
    g.lineWidth = Math.max(1.2, h * 0.18);
    g.beginPath();
    g.moveTo(pad + cap * 0.6, pad + h / 2);
    g.lineTo(pad + w - cap * 0.6, pad + h / 2);
    g.stroke();
    this.litBmp = { c, cap, pad };
    return this.litBmp;
  }

  /** The piano keys for rows lo..hi (cream and black glass, names, chrome edge). */
  private piano(lo: number, hi: number, spelling: "sharps" | "flats"): { c: HTMLCanvasElement; lo: number; hi: number } {
    const P = this.pianoBmp;
    if (P && P.lo <= lo && P.hi >= hi && P.spelling === spelling) return P;
    const L = this.L!;
    const st = this.style!;
    const plo = lo - 8;
    const phi = hi + 8;
    const W = L.pianoW;
    const H = (phi - plo + 1) * L.rowH;
    const { c, g } = this.canvasOf(W, H);
    const left = L.params.pianoSide === "left";
    const yRow = (p: number) => (phi - p + 0.5) * L.rowH; // centre of row p
    const chromeW = Math.max(3, 4 * L.params.scale);
    const keyW = W - chromeW;
    const kx = left ? 0 : chromeW;
    g.fillStyle = "#0b0910";
    g.fillRect(0, 0, W, H);
    // White keys: a real keyboard's shapes (each spans its row and half of a black neighbour's).
    for (let p = plo; p <= phi; p++) {
      if (isBlackKey(p)) continue;
      const top = yRow(p + 0.5 + (isBlackKey(p + 1) ? 0.5 : 0)) + 0.6;
      const bot = yRow(p - 0.5 - (isBlackKey(p - 1) ? 0.5 : 0)) - 0.6;
      const grad = g.createLinearGradient(kx, 0, kx + keyW, 0);
      if (left) {
        grad.addColorStop(0, "#cfc4a8");
        grad.addColorStop(0.2, "#f2ead6");
        grad.addColorStop(0.85, "#fbf6ea");
        grad.addColorStop(1, "#e3d8bd");
      } else {
        grad.addColorStop(0, "#e3d8bd");
        grad.addColorStop(0.15, "#fbf6ea");
        grad.addColorStop(0.8, "#f2ead6");
        grad.addColorStop(1, "#cfc4a8");
      }
      g.fillStyle = grad;
      roundRect(g, kx + 1, top, keyW - 2, bot - top, Math.min(3, L.rowH * 0.2));
      g.fill();
      // A soft shadow at the key's lower edge (glass thickness).
      g.fillStyle = "rgba(90,70,40,0.25)";
      g.fillRect(kx + 2, bot - 1.2, keyW - 4, 1.2);
    }
    // Black keys: glass, from the outer edge, 60 % of the width.
    const bw = keyW * 0.6;
    const bx = left ? kx : kx + keyW - bw;
    for (let p = plo; p <= phi; p++) {
      if (!isBlackKey(p)) continue;
      const top = yRow(p + 0.5) + 0.8;
      const bot = yRow(p - 0.5) - 0.8;
      const grad = g.createLinearGradient(bx, 0, bx + bw, 0);
      grad.addColorStop(0, left ? "#050407" : "#2c2833");
      grad.addColorStop(0.7, "#1b1820");
      grad.addColorStop(1, left ? "#3a3542" : "#050407");
      g.fillStyle = grad;
      roundRect(g, bx, top, bw, bot - top, Math.min(3, L.rowH * 0.2));
      g.fill();
      g.fillStyle = "rgba(255,255,255,0.18)";
      g.fillRect(bx + bw * 0.12, top + 1.2, bw * 0.76, Math.max(1, L.rowH * 0.1));
    }
    // Names: every white key, and the black keys when the rows are tall enough.
    const lp = L.labelPx;
    g.textBaseline = "middle";
    for (let p = plo; p <= phi; p++) {
      const black = isBlackKey(p);
      if (black && L.rowH < 15 * L.params.scale) continue;
      const name = displayName(noteName(p, spelling));
      const isC = p % 12 === 0;
      g.font = `${isC ? 700 : 500} ${black ? lp * 0.82 : lp}px ${st.labelFont}`;
      g.fillStyle = black ? "rgba(246,239,217,0.82)" : isC ? "#7a1020" : "#3b2b1d";
      g.textAlign = left ? "right" : "left";
      const tx = black ? (left ? bx + bw - 3 : bx + 3) : left ? kx + keyW - 5 : kx + 5;
      g.fillText(name, tx, yRow(p) + 0.5);
    }
    g.textAlign = "left";
    // The chrome edge towards the lane.
    const cx = left ? W - chromeW : 0;
    const chrome = g.createLinearGradient(cx, 0, cx + chromeW, 0);
    chrome.addColorStop(0, "#5a606b");
    chrome.addColorStop(0.45, "#f6f8fb");
    chrome.addColorStop(1, "#6b717c");
    g.fillStyle = chrome;
    g.fillRect(cx, 0, chromeW, H);
    this.pianoBmp = { c, lo: plo, hi: phi, spelling };
    return this.pianoBmp;
  }

  /** Rows the cached piano bitmap holds (tests): row p is centred at (hi - p + 0.5) x rowH in it. */
  debugPiano(): { lo: number; hi: number } | null {
    return this.pianoBmp ? { lo: this.pianoBmp.lo, hi: this.pianoBmp.hi } : null;
  }
  /** Seconds one bar or word tile covers (tests). */
  debugTileS(): number {
    return this.tileS();
  }

  /**
   * One 1:1 copy of a cached bitmap onto whole device pixels (source in device px,
   * destination in CSS px; the callers pass values that are whole device pixels already,
   * up to floating-point error, which the rounding here removes).
   */
  private blit(kind: BlitEntry["kind"], tile: number, src: HTMLCanvasElement, sx: number, sy: number, sw: number, sh: number, dx: number, dy: number): void {
    const d = this.dpr;
    sx = Math.max(0, Math.round(sx));
    sy = Math.max(0, Math.round(sy));
    sw = Math.min(Math.round(sw), src.width - sx);
    sh = Math.min(Math.round(sh), src.height - sy);
    if (sw <= 0 || sh <= 0) return;
    dx = Math.round(dx * d) / d;
    dy = Math.round(dy * d) / d;
    const dw = sw / d;
    const dh = sh / d;
    this.ctx.drawImage(src, sx, sy, sw, sh, dx, dy, dw, dh);
    this.debugLog?.push({ kind, tile, sx, sy, sw, sh, dx, dy, dw, dh });
  }

  /** Screen y offset of the bar tiles at display shift `shift` (whole device pixels when the shift is whole). */
  private barDy(f: LaneFrame, shift: number): number {
    // Screen y of a pitch minus its tile y: the same for every pitch (the lane cancels out).
    return yOf(this.L!, f.viewLow, this.tileTop() + shift) - this.tileY(this.tileTop()) + this.dy;
  }

  /** Blits the bar tiles of one region [x0, x1) at display shift `shift`. */
  private drawBars(f: LaneFrame, x0: number, x1: number, shift: number): void {
    const L = this.L!;
    const d = this.dpr;
    const tS = this.tileS();
    const cx0 = Math.max(x0, L.barX);
    const cx1 = Math.min(x1, L.barX + L.barW);
    if (cx1 <= cx0) return;
    // During a slide the shift is fractional: this region's offset is rounded on its own.
    const dyTile = this.snap(this.barDy(f, shift));
    const th = this.tileH();
    // Vertical clip to the bar area (with the glow).
    const top = this.snap(Math.max(L.barTop - 2, dyTile));
    const bot = this.snap(Math.min(L.barBottom + 2, dyTile + th));
    if (bot <= top) return;
    const kFirst = Math.floor((f.t + (cx0 - L.nowX) / L.pxPerSec) / tS);
    const kLast = Math.floor((f.t + (cx1 - L.nowX) / L.pxPerSec) / tS);
    for (let k = Math.max(0, kFirst); k <= kLast; k++) {
      const tile = this.barTile(k);
      // Tile k starts at song time k x tS: k x TILE_PX px right of song time 0 (rounded once per frame).
      const tx = xOf(L, f.t, 0) + this.dx + k * TILE_PX;
      const a = this.snap(Math.max(cx0, tx));
      const b = this.snap(Math.min(cx1, tx + TILE_PX));
      if (b <= a) continue;
      this.blit("bars", k, tile, (a - tx) * d, (top - dyTile) * d, (b - a) * d, (bot - top) * d, a, top);
    }
  }

  private drawWords(f: LaneFrame): void {
    const L = this.L!;
    if (L.wordRows === 0) return;
    const d = this.dpr;
    const tS = this.tileS();
    const stripH = L.wordBottom - L.wordTop + L.wordPx * 0.6;
    const x0 = L.barX;
    const x1 = L.barX + L.barW;
    const nowX = this.snap(L.nowX);
    const top = this.snap(L.wordTop);
    const kFirst = Math.floor((f.t + (x0 - L.nowX) / L.pxPerSec) / tS);
    const kLast = Math.floor((f.t + (x1 - L.nowX) / L.pxPerSec) / tS);
    for (let k = Math.max(0, kFirst); k <= kLast; k++) {
      const tx = xOf(L, f.t, 0) + this.dx + k * TILE_PX;
      // Sung copy left of the now line, upcoming copy right of it.
      for (const sung of [true, false]) {
        const a = this.snap(Math.max(sung ? x0 : nowX, tx));
        const b = this.snap(Math.min(sung ? nowX : x1, tx + TILE_PX));
        if (b <= a) continue;
        const tile = this.wordTile(k, sung);
        this.blit(sung ? "words-sung" : "words-up", k, tile, (a - tx) * d, 0, (b - a) * d, stripH * d, a, top);
      }
    }
  }

  /** The row shading: one octave bitmap, repeated over the view. */
  private drawRows(f: LaneFrame): void {
    const L = this.L!;
    const bmp = this.rows();
    const d = this.dpr;
    const oLo = Math.floor((f.rowLow - 1) / 12);
    const oHi = Math.floor((f.rowHigh + 1) / 12);
    for (let o = oLo; o <= oHi; o++) {
      // The octave's top edge (above its B), on the frame's device-pixel grid.
      const top = yOf(L, f.viewLow, 12 * o + 11.5) + this.dy;
      const a = this.snap(Math.max(L.barTop, top));
      const b = this.snap(Math.min(L.barBottom, top + 12 * L.rowH));
      if (b <= a) continue;
      this.blit("rows", o, bmp, 0, (a - top) * d, L.barW * d, (b - a) * d, L.barX, a);
    }
  }

  /** Draws one frame at `inp.t`; returns the frame (lane.ts) it drew. */
  draw(inp: LaneInputs): LaneFrame | null {
    const L = this.L;
    const st = this.style;
    if (!L || !st) return null;
    const t0 = performance.now();
    this.frameNo++;
    const f = laneFrame(L, inp);
    const ctx = this.ctx;
    const d = this.dpr;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    ctx.setTransform(d, 0, 0, d, 0, 0);
    ctx.imageSmoothingEnabled = true;
    // This frame's grid (see "Device pixels"): the time axis and the top edge of row 0.
    const gx = xOf(L, f.t, 0);
    this.dx = this.snap(gx) - gx;
    const gy = yOf(L, f.viewLow, 0.5);
    this.dy = this.snap(gy) - gy;
    const panel = this.panel();
    ctx.drawImage(panel, 0, 0, panel.width / d, panel.height / d);

    // Bar area: rows, bars per region, lit parts.
    ctx.save();
    ctx.beginPath();
    ctx.rect(L.barX, L.barTop - 2, L.barW, L.barBottom - L.barTop + 4);
    ctx.clip();
    this.drawRows(f);
    for (const r of f.regions) this.drawBars(f, r.x0, r.x1, r.shift);
    if (f.lit.length) {
      const lit = this.lit();
      const h = L.barH;
      const lw = lit.c.width / d;
      const lh = lit.c.height / d;
      const capS = (lit.pad + lit.cap) * d;
      for (let q = 0; q < f.lit.length; q += 4) {
        const n = L.notes[f.lit[q]];
        // On the bar under it: the same rounding of the frame's grid.
        const xa = f.lit[q + 1] + this.dx;
        const xb = f.lit[q + 2] + this.dx;
        const y = f.lit[q + 3] + this.dy;
        const nx0 = xOf(L, f.t, n.start) + this.dx;
        const nx1 = xOf(L, f.t, n.end) + this.dx;
        const top = y - h / 2 - lit.pad;
        // A lit stretch is a short gold tube inside the pink one: rounded, softly glowing
        // ends (snapped to the note's own ends when it reaches them), a stretched middle.
        const x0 = xa - nx0 < h * 0.5 ? nx0 : xa;
        const x1 = nx1 - xb < h * 0.5 ? nx1 : xb;
        const cap = Math.min(lit.cap, (x1 - x0) / 2);
        if (cap <= 0.5) continue;
        ctx.drawImage(lit.c, 0, 0, capS, lit.c.height, x0 - lit.pad, top, lit.pad + cap, lh);
        ctx.drawImage(lit.c, lit.c.width - capS, 0, capS, lit.c.height, x1 - cap, top, lit.pad + cap, lh);
        if (x1 - cap > x0 + cap) ctx.drawImage(lit.c, (lw / 2) * d - 0.5, 0, 1, lit.c.height, x0 + cap, top, x1 - x0 - 2 * cap, lh);
      }
    }
    ctx.restore();

    // Fades at both ends of the bar area: notes come out of the dark on the right.
    const fadeW = Math.min(L.barW * 0.08, 70 * L.params.scale);
    const fr = ctx.createLinearGradient(L.barX + L.barW - fadeW, 0, L.barX + L.barW, 0);
    fr.addColorStop(0, "rgba(14,10,21,0)");
    fr.addColorStop(1, "rgba(14,10,21,0.9)");
    ctx.fillStyle = fr;
    ctx.fillRect(L.barX + L.barW - fadeW, L.barTop - 2, fadeW, L.barBottom - L.barTop + 4);
    const fl = ctx.createLinearGradient(L.barX, 0, L.barX + fadeW * 0.7, 0);
    fl.addColorStop(0, "rgba(14,10,21,0.85)");
    fl.addColorStop(1, "rgba(14,10,21,0)");
    ctx.fillStyle = fl;
    ctx.fillRect(L.barX, L.barTop - 2, fadeW * 0.7, L.barBottom - L.barTop + 4);

    // Words under the notes.
    this.drawWords(f);

    // The now needle.
    const nd = this.needle();
    const ndw = nd.width / d;
    const ndh = nd.height / d;
    ctx.drawImage(nd, this.snap(L.nowX - ndw / 2), this.snap(L.barTop - (ndh - (L.barBottom - L.barTop)) / 2), ndw, ndh);

    // The countdown in long rests.
    if (f.countdown !== null) {
      const px = Math.max(14, Math.min((L.barBottom - L.barTop) * 0.2, 34 * L.params.scale));
      ctx.font = `600 ${px}px ${st.labelFont}`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      const cx = L.nowX + (L.barX + L.barW - L.nowX) / 2;
      const cy = (L.barTop + L.barBottom) / 2;
      const text = `SINGING IN ${formatCountdown(f.countdown)}`;
      if (!this.simple) {
        // A soft gold halo: wide, faint strokes under the letters (no per-frame blur).
        ctx.lineJoin = "round";
        ctx.strokeStyle = withAlpha(st.sung, 0.12);
        ctx.lineWidth = px * 0.42;
        ctx.strokeText(text, cx, cy);
        ctx.strokeStyle = withAlpha(st.sung, 0.22);
        ctx.lineWidth = px * 0.2;
        ctx.strokeText(text, cx, cy);
      }
      ctx.fillStyle = "#f6efd9";
      ctx.fillText(text, cx, cy);
      ctx.textAlign = "left";
    }

    // The singer: tail, head (or edge arrow), cue.
    const sg = f.singer;
    if (sg) {
      const tail = f.tail;
      if (tail.length >= 4) {
        let xMin = L.nowX;
        for (let q = 0; q < tail.length; q += 2) if (tail[q] < xMin) xMin = tail[q];
        const pathTail = () => {
          ctx.beginPath();
          let pen = false;
          for (let q = 0; q + 1 < tail.length; q += 2) {
            const x = tail[q];
            const y = tail[q + 1];
            if (Number.isNaN(x)) {
              pen = false;
              continue;
            }
            if (pen) ctx.lineTo(x, y);
            else ctx.moveTo(x, y);
            pen = true;
          }
        };
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        // A soft halo (lighter than the marker, so it reads as light, not as a dark outline)
        // under the bright core, both fading out to the left.
        const gr = ctx.createLinearGradient(xMin, 0, L.nowX, 0);
        gr.addColorStop(0, "rgba(200,255,238,0)");
        gr.addColorStop(1, "rgba(200,255,238,0.2)");
        ctx.strokeStyle = gr;
        ctx.lineWidth = L.tailW * 1.75;
        pathTail();
        ctx.stroke();
        const gc = ctx.createLinearGradient(xMin, 0, L.nowX, 0);
        gc.addColorStop(0, withAlpha(st.marker, 0.05));
        gc.addColorStop(1, withAlpha(st.marker, 0.95));
        ctx.strokeStyle = gc;
        ctx.lineWidth = L.tailW;
        pathTail();
        ctx.stroke();
      }
      if (sg.edge === 0) {
        const hb = this.head();
        const plain = inp.reduceMotion || this.simple;
        const bmp = plain ? hb.plain : hb.glow;
        const w = bmp.width / d;
        const h = bmp.height / d;
        // The arrow's tip just past the needle, its body on the line.
        const x = L.nowX + L.headW * 0.3 - L.headW - hb.pad;
        if (!plain && sg.cue === 0 && f.target) {
          // On the note: the glow breathes (a glow animation; none under reduce-motion).
          ctx.globalAlpha = 0.85 + 0.15 * Math.sin(inp.nowMs / 140);
        }
        ctx.drawImage(bmp, x, sg.y - h / 2, w, h);
        ctx.globalAlpha = 1;
      } else {
        const ab = this.arrows();
        const bmp = sg.edge > 0 ? ab.up : ab.down;
        const w = bmp.width / d;
        const h = bmp.height / d;
        ctx.drawImage(bmp, L.nowX - w / 2, sg.edge > 0 ? L.barTop - ab.pad : L.barBottom - h + ab.pad, w, h);
      }
      if (sg.cue !== 0) this.drawCue(sg.cue, sg.edge === 0 ? sg.y : sg.edge > 0 ? L.barTop + L.headH : L.barBottom - L.headH);
    }

    // The piano keys, with the key to sing (gold) and the singer's key (mint) lit.
    this.drawPiano(f);

    this.last = f;
    // Render the next bar tile ahead, so a new tile never appears half made on screen. It is
    // work done in this frame, so it counts in the frame's time below (the stats are what
    // the screenshot script and the budget read).
    const ahead = Math.floor((f.t + (L.barX + L.barW - L.nowX) / L.pxPerSec) / this.tileS()) + 1;
    if (ahead * this.tileS() < L.duration && !this.barTiles.has(ahead)) this.barTile(ahead);
    const ms = performance.now() - t0;
    this.stats.frames++;
    this.stats.lastDrawMs = ms;
    this.stats.totalDrawMs += ms;
    if (ms > this.stats.maxDrawMs) this.stats.maxDrawMs = ms;
    return f;
  }

  /** "HIGHER" / "LOWER" with a drawn arrow (never colour alone), right of the marker. */
  private drawCue(cue: -1 | 1, y: number): void {
    const L = this.L!;
    const st = this.style!;
    const ctx = this.ctx;
    const px = Math.max(10, 12.5 * L.params.scale);
    const text = cue > 0 ? "HIGHER" : "LOWER";
    ctx.font = `700 ${px}px ${st.labelFont}`;
    const tw = ctx.measureText(text).width;
    const ah = px * 0.7;
    const padX = px * 0.55;
    const w = padX * 2 + ah + px * 0.35 + tw;
    const h = px * 1.6;
    const x = L.nowX + L.headW * 0.45;
    const yy = Math.max(L.barTop, Math.min(L.barBottom - h, y - h / 2 + (cue > 0 ? -h * 0.9 : h * 0.9)));
    ctx.fillStyle = "rgba(10,8,16,0.86)";
    roundRect(ctx, x, yy, w, h, h / 2);
    ctx.fill();
    ctx.strokeStyle = st.marker;
    ctx.lineWidth = Math.max(1.2, px * 0.1);
    ctx.stroke();
    // The arrow.
    const ax = x + padX;
    const ay = yy + h / 2;
    ctx.fillStyle = st.marker;
    ctx.beginPath();
    if (cue > 0) {
      ctx.moveTo(ax, ay + ah / 2);
      ctx.lineTo(ax + ah, ay + ah / 2);
      ctx.lineTo(ax + ah / 2, ay - ah / 2);
    } else {
      ctx.moveTo(ax, ay - ah / 2);
      ctx.lineTo(ax + ah, ay - ah / 2);
      ctx.lineTo(ax + ah / 2, ay + ah / 2);
    }
    ctx.closePath();
    ctx.fill();
    ctx.fillStyle = "#f6efd9";
    ctx.textBaseline = "middle";
    ctx.fillText(text, ax + ah + px * 0.35, ay + 0.5);
  }

  /** A lit key of width w and height h in `color`, with its glow (none in the simple lane). */
  private litKey(color: string, w: number, h: number): { c: HTMLCanvasElement; pad: number } {
    const key = `${color}|${w.toFixed(2)}|${h.toFixed(2)}`;
    const hit = this.keyBmps.get(key);
    if (hit) return hit;
    const L = this.L!;
    const pad = Math.ceil(L.rowH * 0.9);
    const { c, g } = this.canvasOf(w + 2 * pad, h + 2 * pad);
    if (!this.simple) {
      g.shadowColor = withAlpha(color, 0.9);
      g.shadowBlur = L.rowH * 0.6;
    }
    const gr = g.createLinearGradient(pad, 0, pad + w, 0);
    gr.addColorStop(0, shade(color, -0.15));
    gr.addColorStop(0.5, shade(color, 0.35));
    gr.addColorStop(1, color);
    g.fillStyle = gr;
    roundRect(g, pad, pad, w, h, Math.min(3, L.rowH * 0.2));
    g.fill();
    const out = { c, pad };
    this.keyBmps.set(key, out);
    return out;
  }

  private drawPiano(f: LaneFrame): void {
    const L = this.L!;
    const st = this.style!;
    const ctx = this.ctx;
    const d = this.dpr;
    const P = this.piano(f.rowLow, f.rowHigh, f.spelling);
    // Bitmap row p centre at (P.hi - p + 0.5) * rowH; screen row centre at yOf(p). The
    // bitmap's top edge (the top edge of row P.hi) on the frame's grid, like the bars.
    const offset = yOf(L, f.viewLow, P.hi + 0.5) + this.dy;
    const a = this.snap(Math.max(0, L.barTop - 2));
    const b = this.snap(Math.min(this.cssH, L.barBottom + 2));
    this.blit("piano", 0, P.c, 0, (a - offset) * d, P.c.width, (b - a) * d, L.pianoX, a);
    // Panel below and above the keys (the word strip's corner): plain dark.
    ctx.fillStyle = "#0b0910";
    if (a > 0) ctx.fillRect(L.pianoX, 0, L.pianoW, a);
    if (b < this.cssH) ctx.fillRect(L.pianoX, b, L.pianoW, this.cssH - b);
    const left = L.params.pianoSide === "left";
    const chromeW = Math.max(3, 4 * L.params.scale);
    const keyW = L.pianoW - chromeW;
    const kx = L.pianoX + (left ? 0 : chromeW);
    const bw = keyW * 0.6;
    const bx = left ? kx : kx + keyW - bw;
    const light = (p: number, color: string, half: boolean) => {
      const y0 = yOf(L, f.viewLow, p + 0.5) + this.dy;
      const y1 = yOf(L, f.viewLow, p - 0.5) + this.dy;
      if (y1 < L.barTop - 1 || y0 > L.barBottom + 1) return;
      const black = isBlackKey(p);
      let x = black ? bx : kx + 1;
      let w = black ? bw : keyW - 2;
      if (half) {
        // Both lit on one key: the singer's colour on the lane-side half.
        w = w / 2;
        if (left) x += w;
      }
      const spr = this.litKey(color, w, y1 - y0 - 1.6);
      ctx.drawImage(spr.c, x - spr.pad, y0 + 0.8 - spr.pad, spr.c.width / d, spr.c.height / d);
    };
    const tRow = f.target?.row ?? null;
    const sRow = f.singer && f.singer.edge === 0 ? f.singer.row : null;
    if (tRow !== null) light(tRow, st.sung, false);
    if (sRow !== null) light(sRow, st.marker, sRow === tRow);
    // Names on lit keys, always (a black key's name too, so the note to sing is named).
    ctx.textBaseline = "middle";
    for (const p of [tRow, sRow]) {
      if (p === null) continue;
      const black = isBlackKey(p);
      const y = yOf(L, f.viewLow, p) + this.dy;
      if (y < L.barTop || y > L.barBottom) continue;
      // A black key's name inside the black key, a white key's at its lane end.
      ctx.font = `700 ${black ? L.labelPx * 0.88 : L.labelPx}px ${st.labelFont}`;
      ctx.fillStyle = "#1a1206";
      ctx.textAlign = left ? "right" : "left";
      const tx = black ? (left ? bx + bw - 3 : bx + 3) : left ? kx + keyW - 5 : kx + 5;
      ctx.fillText(displayName(noteName(p, f.spelling)), tx, y + 0.5);
    }
    ctx.textAlign = "left";
  }
}

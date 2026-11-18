// SPDX-License-Identifier: GPL-3.0-or-later
// Canvas 2D renderer of the page display (page.ts, D-218).
//
// Each row of a line is pre-rendered once per colour into an offscreen canvas in device
// pixels: a dark outline with a soft shadow (drawn under every letter first), then the
// letters in the sung colour or in the upcoming colour. Every grapheme is drawn alone at
// the x the layout computed, so no kerning or ligature can move a letter away from its
// timing. The preview (the next line, smaller) has its own small bitmaps so it stays
// crisp. Per frame: clear, then for each line on screen blit its rows, the sung bitmap
// clipped left of the wipe edge and the upcoming bitmap right of it (the outline is the
// same in both, so the colour boundary runs cleanly through the letters); then the
// lead-in bar and, in rests, the countdown. A few bitmaps are kept (LRU) and the line
// after the preview is rendered ahead, so a line change never costs a frame.
import { formatCountdown } from "../lib/format";
import { CAP_EM, INK_PAD_EM, LINE_HEIGHT_EM, type PageFrame, type PageLayout, type PageLine, pageFrame, PREVIEW_SCALE, wipeInRow } from "./page";
import type { TickerStats } from "./renderer";

export interface PageStyle {
  fontFamily: string;
  fontWeight: string;
  sung: string;
  upcoming: string;
  /** Outline and shadow around the letters (legible over any background). */
  outline: string;
  shadow: string;
}

/** Outline width (the stroke straddles the glyph edge, so half of it shows), em. */
const OUTLINE_EM = 0.1;
/** Shadow blur and drop, em. */
const SHADOW_BLUR_EM = 0.12;
const SHADOW_DROP_EM = 0.045;
/** Lines whose bitmaps are kept. */
const KEEP_LINES = 8;

interface RowBitmaps {
  sung: HTMLCanvasElement | null;
  up: HTMLCanvasElement | null;
}

interface LineBitmaps {
  rows: RowBitmaps[];
  /** Upcoming colour at the preview size, one per row. */
  preview: (HTMLCanvasElement | null)[];
  used: number;
}

export class PageRenderer {
  private ctx: CanvasRenderingContext2D;
  private cssW = 0;
  private cssH = 0;
  private dpr = 1;
  private layout: PageLayout | null = null;
  private style: PageStyle | null = null;
  private lines = new Map<number, LineBitmaps>();
  private capCache = new Map<number, number>();
  private frameNo = 0;
  readonly stats: TickerStats = { frames: 0, tilesRendered: 0, lastDrawMs: 0, maxDrawMs: 0, totalDrawMs: 0 };
  /** The last frame drawn (for the debug hooks). */
  last: PageFrame | null = null;

  constructor(
    private canvas: HTMLCanvasElement,
    private makeCanvas: () => HTMLCanvasElement = () => document.createElement("canvas"),
  ) {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) throw new Error("Canvas 2D is not available");
    this.ctx = ctx;
  }

  /** Sets the canvas size (CSS px) and pixel ratio; drops the bitmaps if the ratio changed. */
  setSize(cssW: number, cssH: number, dpr: number): void {
    const w = Math.max(1, Math.round(cssW * dpr));
    const h = Math.max(1, Math.round(cssH * dpr));
    if (this.canvas.width !== w) this.canvas.width = w;
    if (this.canvas.height !== h) this.canvas.height = h;
    if (dpr !== this.dpr) this.lines.clear();
    this.cssW = cssW;
    this.cssH = cssH;
    this.dpr = dpr;
  }

  /** New layout or style: bitmaps are rebuilt lazily. */
  setLayout(layout: PageLayout, style: PageStyle): void {
    this.layout = layout;
    this.style = style;
    this.lines.clear();
    this.capCache.clear();
  }

  private font(px: number): string {
    const s = this.style!;
    return `${s.fontWeight} ${px}px ${s.fontFamily}`;
  }

  /** Height of a capital letter at `px` (the font's own, else CAP_EM). */
  capHeight(px: number): number {
    let c = this.capCache.get(px);
    if (c === undefined) {
      this.ctx.font = this.font(px);
      const m = this.ctx.measureText("H");
      c = m.actualBoundingBoxAscent > 0 ? m.actualBoundingBoxAscent : CAP_EM * px;
      this.capCache.set(px, c);
    }
    return c;
  }

  /** Baseline of a row band of a line at size `px`, from the band's top: capitals centred in the band. */
  private baselineIn(px: number): number {
    return (LINE_HEIGHT_EM * px) / 2 + this.capHeight(px) / 2;
  }

  /** One row of `line` in one colour at `scale` x the line's size. */
  private renderRow(line: PageLine, r: number, color: string, scale: number): HTMLCanvasElement | null {
    const s = this.style!;
    const row = line.rows[r];
    const px = line.px * scale;
    const pad = INK_PAD_EM * px;
    const wCss = (row.u1 - row.u0) * scale + 2 * pad;
    const hCss = LINE_HEIGHT_EM * px + 2 * pad;
    const c = this.makeCanvas();
    c.width = Math.max(1, Math.ceil(wCss * this.dpr));
    c.height = Math.max(1, Math.ceil(hCss * this.dpr));
    const ctx = c.getContext("2d");
    if (!ctx) return null;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    const k = ctx as CanvasRenderingContext2D & { fontKerning?: string };
    if ("fontKerning" in k) k.fontKerning = "none";
    ctx.font = this.font(px);
    ctx.textBaseline = "alphabetic";
    ctx.lineJoin = "round";
    ctx.miterLimit = 2;
    const y = pad + this.baselineIn(px);
    const xOf = (i: number) => pad + (line.glyphs[i].x - row.u0) * scale;
    // Outline (with the shadow under it) for every letter first, so no outline covers a neighbour's fill.
    ctx.strokeStyle = s.outline;
    ctx.lineWidth = OUTLINE_EM * px;
    ctx.shadowColor = s.shadow;
    // Shadow sizes are in device pixels (not transformed).
    ctx.shadowBlur = SHADOW_BLUR_EM * px * this.dpr;
    ctx.shadowOffsetY = SHADOW_DROP_EM * px * this.dpr;
    for (let i = row.first; i <= row.last; i++) ctx.strokeText(line.glyphs[i].g, xOf(i), y);
    ctx.shadowColor = "rgba(0, 0, 0, 0)";
    ctx.shadowBlur = 0;
    ctx.shadowOffsetY = 0;
    ctx.fillStyle = color;
    for (let i = row.first; i <= row.last; i++) ctx.fillText(line.glyphs[i].g, xOf(i), y);
    this.stats.tilesRendered++;
    return c;
  }

  private bitmaps(i: number): LineBitmaps {
    let b = this.lines.get(i);
    if (b) {
      b.used = this.frameNo;
      return b;
    }
    const L = this.layout!.lines[i];
    const s = this.style!;
    b = {
      rows: L.rows.map((_, r) => ({ sung: this.renderRow(L, r, s.sung, 1), up: this.renderRow(L, r, s.upcoming, 1) })),
      preview: L.rows.map((_, r) => this.renderRow(L, r, s.upcoming, PREVIEW_SCALE)),
      used: this.frameNo,
    };
    this.lines.set(i, b);
    if (this.lines.size > KEEP_LINES) {
      let oldest = -1;
      let oldestUsed = Infinity;
      for (const [k, v] of this.lines) if (v.used < oldestUsed) [oldest, oldestUsed] = [k, v.used];
      this.lines.delete(oldest);
    }
    return b;
  }

  /** Draws lyric time `t` (seconds); returns what was drawn. */
  draw(t: number): PageFrame | null {
    const started = performance.now();
    this.frameNo++;
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.cssW, this.cssH);
    const l = this.layout;
    const s = this.style;
    if (!l || !s) return null;
    const f = pageFrame(l, t);
    this.last = f;
    ctx.imageSmoothingEnabled = true;
    const q = ctx as CanvasRenderingContext2D & { imageSmoothingQuality?: string };
    if ("imageSmoothingQuality" in q) q.imageSmoothingQuality = "high";
    // Back to front: the leaving line, the preview, the current line.
    const order = { leaving: 0, preview: 1, entering: 2, current: 3 } as const;
    for (const it of [...f.items].sort((a, b) => order[a.role] - order[b.role])) {
      const L = l.lines[it.line];
      const bm = this.bitmaps(it.line);
      ctx.globalAlpha = it.alpha;
      const pad = INK_PAD_EM * L.px;
      L.rows.forEach((row, r) => {
        const rowW = row.u1 - row.u0;
        const sung = wipeInRow(L, r, it.wipe);
        if (it.role === "preview" && sung <= 0) {
          // The preview's own crisp bitmap (same geometry, drawn at the preview size).
          const pv = bm.preview[r];
          if (!pv) return;
          const x = this.snap(L.cx + (row.left - pad - L.cx) * it.scale);
          const y = this.snap(it.top + (row.top - pad) * it.scale);
          ctx.drawImage(pv, x, y, pv.width / this.dpr, pv.height / this.dpr);
          return;
        }
        const one = it.scale === 1;
        const dx = one ? this.snap(row.left - pad) : L.cx + (row.left - pad - L.cx) * it.scale;
        const dy = one ? this.snap(it.top + row.top - pad) : it.top + (row.top - pad) * it.scale;
        const up = bm.rows[r].up;
        const su = bm.rows[r].sung;
        if (!up || !su) return;
        const dw = (up.width / this.dpr) * it.scale;
        const dh = (up.height / this.dpr) * it.scale;
        if (sung <= 0) ctx.drawImage(up, dx, dy, dw, dh);
        else if (sung >= rowW) ctx.drawImage(su, dx, dy, dw, dh);
        else {
          const split = dx + (pad + sung) * it.scale;
          this.clipped(su, dx, dy, dw, dh, dx - 1, split);
          this.clipped(up, dx, dy, dw, dh, split, dx + dw + 1);
        }
      });
    }
    ctx.globalAlpha = 1;
    if (f.leadIn) this.drawLeadIn(f, l);
    if (f.countdown) this.drawCountdown(f, l);
    // Render the line after the preview ahead of time (it appears at the next change).
    const ahead = f.current + 2;
    if (ahead < l.lines.length && !this.lines.has(ahead)) this.bitmaps(ahead);
    const ms = performance.now() - started;
    this.stats.frames++;
    this.stats.lastDrawMs = ms;
    this.stats.totalDrawMs += ms;
    if (ms > this.stats.maxDrawMs) this.stats.maxDrawMs = ms;
    return f;
  }

  /** Rounds a CSS position to the device pixel grid (crisp blits at scale 1). */
  private snap(v: number): number {
    return Math.round(v * this.dpr) / this.dpr;
  }

  private clipped(img: HTMLCanvasElement, dx: number, dy: number, dw: number, dh: number, x0: number, x1: number): void {
    if (x1 <= x0) return;
    const ctx = this.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x0, dy - 1, x1 - x0, dh + 2);
    ctx.clip();
    ctx.drawImage(img, dx, dy, dw, dh);
    ctx.restore();
  }

  /** The lead-in bar: a gold block left of the line, eaten from the left until the first word. */
  private drawLeadIn(f: PageFrame, l: PageLayout): void {
    const b = f.leadIn!;
    const s = this.style!;
    const ctx = this.ctx;
    const L = l.lines[b.line];
    const cap = this.capHeight(L.px);
    const baseline = b.rowTop + this.baselineIn(L.px);
    const top = baseline - cap;
    const visible = b.w * b.frac;
    if (visible <= 0) return;
    const x0 = b.x + b.w - visible;
    const g = ctx.createLinearGradient(b.x, 0, b.x + b.w, 0);
    g.addColorStop(0, shade(s.sung, 0.22));
    g.addColorStop(0.55, shade(s.sung, 0.62));
    g.addColorStop(1, s.sung);
    ctx.save();
    ctx.shadowColor = s.shadow;
    ctx.shadowBlur = SHADOW_BLUR_EM * L.px * this.dpr;
    ctx.shadowOffsetY = SHADOW_DROP_EM * L.px * this.dpr;
    ctx.fillStyle = g;
    ctx.fillRect(x0, top, visible, cap);
    ctx.restore();
  }

  /** "Singing in 0:08" in rests, where the next line will appear. */
  private drawCountdown(f: PageFrame, l: PageLayout): void {
    const c = f.countdown!;
    if (c.remaining <= 0) return;
    const s = this.style!;
    const ctx = this.ctx;
    const P = l.fontPx;
    const big = Math.round(P * 0.8);
    const small = Math.round(P * 0.34);
    const cx = l.width / 2;
    const y = l.centerY;
    ctx.save();
    ctx.globalAlpha = c.alpha;
    ctx.textAlign = "center";
    ctx.lineJoin = "round";
    ctx.strokeStyle = s.outline;
    ctx.fillStyle = s.upcoming;
    ctx.font = this.font(small);
    ctx.textBaseline = "bottom";
    ctx.lineWidth = OUTLINE_EM * small;
    const labelY = y - big * 0.42;
    ctx.strokeText("Singing in", cx, labelY);
    ctx.fillText("Singing in", cx, labelY);
    ctx.font = this.font(big);
    ctx.textBaseline = "middle";
    ctx.lineWidth = OUTLINE_EM * big;
    const label = formatCountdown(c.remaining);
    const ty = y + big * 0.12;
    ctx.strokeText(label, cx, ty);
    ctx.fillText(label, cx, ty);
    ctx.restore();
  }
}

/** `color` (#rgb or #rrggbb) darkened to `k` of its brightness; other forms are returned as they are. */
export function shade(color: string, k: number): string {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return color;
  const hex = m[1].length === 3 ? [...m[1]].map((ch) => ch + ch).join("") : m[1];
  const v = [0, 2, 4].map((i) => Math.round(parseInt(hex.slice(i, i + 2), 16) * k));
  return `rgb(${v[0]}, ${v[1]}, ${v[2]})`;
}

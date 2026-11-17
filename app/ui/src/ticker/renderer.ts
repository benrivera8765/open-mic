// SPDX-License-Identifier: GPL-3.0-or-later
// Canvas 2D renderer of the lyrics ticker (docs/ARCHITECTURE.md section 6).
//
// The text strip is pre-rendered into offscreen tiles (TILE_CSS px wide), once in the
// sung colour and once in the upcoming colour; each glyph is drawn with its own
// fillText at the x the layout computed, so no ligature or kerning can shift it.
// Per frame we only clear, then blit the visible tiles twice with a translation:
// the upcoming tiles clipped to the right of the marker, the sung tiles clipped to
// the left, so the colour split sits exactly at the marker. Then the marker and,
// in breaks, the countdown.
//
// Tiles are created lazily for the visible range (plus one ahead) and kept in a small
// LRU cache, so a long song never holds a full-width bitmap (memory on the 8 GB Mac).
// Device pixel ratio is respected: tiles and the canvas are in device pixels.
import { formatCountdown } from "../lib/format";
import type { TickerLayout } from "./layout";
import { restAt } from "./layout";

export const TILE_CSS = 1024;

export interface TickerStyle {
  fontFamily: string;
  fontWeight: string;
  sung: string;
  upcoming: string;
  marker: string;
}

interface Tile {
  sung: HTMLCanvasElement | null;
  up: HTMLCanvasElement | null;
  used: number;
}

export interface TickerStats {
  frames: number;
  tilesRendered: number;
  lastDrawMs: number;
  maxDrawMs: number;
  totalDrawMs: number;
}

export class TickerRenderer {
  private ctx: CanvasRenderingContext2D;
  private cssW = 0;
  private cssH = 0;
  private dpr = 1;
  private layout: TickerLayout | null = null;
  private style: TickerStyle | null = null;
  private tiles = new Map<number, Tile>();
  private frameNo = 0;
  private laneH = 0;
  private laneY = 0;
  private baseline = 0;
  readonly stats: TickerStats = { frames: 0, tilesRendered: 0, lastDrawMs: 0, maxDrawMs: 0, totalDrawMs: 0 };

  constructor(
    private canvas: HTMLCanvasElement,
    private makeCanvas: () => HTMLCanvasElement = () => document.createElement("canvas"),
  ) {
    const ctx = canvas.getContext("2d", { alpha: true });
    if (!ctx) throw new Error("Canvas 2D is not available");
    this.ctx = ctx;
  }

  /** Sets the canvas size (CSS px) and pixel ratio; drops tiles if the ratio changed. */
  setSize(cssW: number, cssH: number, dpr: number): void {
    const w = Math.max(1, Math.round(cssW * dpr));
    const h = Math.max(1, Math.round(cssH * dpr));
    if (this.canvas.width !== w) this.canvas.width = w;
    if (this.canvas.height !== h) this.canvas.height = h;
    if (dpr !== this.dpr) this.tiles.clear();
    this.cssW = cssW;
    this.cssH = cssH;
    this.dpr = dpr;
    this.placeLane();
  }

  /** New layout or style: tiles are rebuilt lazily. */
  setLayout(layout: TickerLayout, style: TickerStyle): void {
    this.layout = layout;
    this.style = style;
    this.tiles.clear();
    this.placeLane();
  }

  private font(px: number): string {
    const s = this.style!;
    return `${s.fontWeight} ${px}px ${s.fontFamily}`;
  }

  private placeLane(): void {
    if (!this.layout || !this.style) return;
    const f = this.layout.fontPx;
    this.laneH = Math.ceil(f * 1.6);
    this.laneY = Math.round((this.cssH - this.laneH) / 2);
    // Baseline from the font's real ascent/descent when the engine reports them.
    this.ctx.font = this.font(f);
    const m = this.ctx.measureText("Hgy");
    const asc = m.fontBoundingBoxAscent || f * 0.8;
    const desc = m.fontBoundingBoxDescent || f * 0.2;
    this.baseline = (this.laneH + asc - desc) / 2;
  }

  private tile(i: number): Tile {
    let t = this.tiles.get(i);
    if (t) {
      t.used = this.frameNo;
      return t;
    }
    t = { sung: this.renderTile(i, this.style!.sung), up: this.renderTile(i, this.style!.upcoming), used: this.frameNo };
    this.tiles.set(i, t);
    // LRU: keep a few screens' worth.
    const max = Math.ceil(this.cssW / TILE_CSS) * 2 + 4;
    if (this.tiles.size > max) {
      let oldest = -1;
      let oldestUsed = Infinity;
      for (const [k, v] of this.tiles) if (v.used < oldestUsed) [oldest, oldestUsed] = [k, v.used];
      this.tiles.delete(oldest);
    }
    return t;
  }

  /** Draws the glyphs overlapping tile i in one colour; null when the tile is empty. */
  private renderTile(i: number, color: string): HTMLCanvasElement | null {
    const l = this.layout!;
    const x0 = i * TILE_CSS;
    const x1 = x0 + TILE_CSS;
    const pad = l.fontPx; // glyph ink may overhang its advance box
    const gs = l.glyphs;
    // First glyph that could touch the tile (glyph x is increasing).
    let lo = 0;
    let hi = gs.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (gs[mid].x + gs[mid].w < x0 - pad) lo = mid + 1;
      else hi = mid;
    }
    if (lo >= gs.length || gs[lo].x > x1 + pad) return null;
    const c = this.makeCanvas();
    c.width = Math.ceil(TILE_CSS * this.dpr);
    c.height = Math.ceil(this.laneH * this.dpr);
    const ctx = c.getContext("2d");
    if (!ctx) return null;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.font = this.font(l.fontPx);
    // Each grapheme is drawn alone, so kerning and ligatures cannot apply between
    // letters; turn kerning off anyway where the engine supports the property.
    const k = ctx as CanvasRenderingContext2D & { fontKerning?: string };
    if ("fontKerning" in k) k.fontKerning = "none";
    ctx.textBaseline = "alphabetic";
    ctx.fillStyle = color;
    for (let n = lo; n < gs.length && gs[n].x <= x1 + pad; n++) {
      const g = gs[n];
      ctx.fillText(g.g, g.x - x0, this.baseline);
    }
    this.stats.tilesRendered++;
    return c;
  }

  /** Draws the frame for lyric time `t` (seconds). */
  draw(t: number): void {
    const started = performance.now();
    this.frameNo++;
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.cssW, this.cssH);
    const l = this.layout;
    const s = this.style;
    if (!l || !s) return;
    const markerX = this.cssW * l.marker;
    const x = l.motion.at(t);
    const offset = markerX - x; // screen x = strip x + offset

    // Upcoming text, right of the marker.
    this.blit(markerX, this.cssW, offset, "up");
    // Sung text, left of the marker.
    this.blit(0, markerX, offset, "sung");

    // Countdown in instrumental breaks, just right of the marker, fading as the next line arrives.
    const rest = restAt(l, t);
    if (rest && rest.remaining > 0.5) {
      const f = Math.round(l.fontPx * 0.62);
      ctx.font = this.font(f);
      ctx.textBaseline = "middle";
      const label = formatCountdown(rest.remaining);
      const cx = markerX + l.fontPx * 0.6;
      const cw = ctx.measureText(label).width;
      const lineLeft = l.lines[rest.rest.nextLine].x0 + offset;
      const alpha = Math.max(0, Math.min(1, (lineLeft - (cx + cw)) / (l.fontPx * 1.5)));
      if (alpha > 0) {
        ctx.globalAlpha = alpha;
        ctx.fillStyle = s.upcoming;
        ctx.fillText(label, cx, this.cssH / 2);
        ctx.font = this.font(Math.round(l.fontPx * 0.3));
        ctx.textBaseline = "bottom";
        ctx.fillText("singing in", cx, this.cssH / 2 - f * 0.55);
        ctx.globalAlpha = 1;
      }
    }

    // The marker: a slim needle across the lane.
    const top = this.laneY - l.fontPx * 0.15;
    const h = this.laneH + l.fontPx * 0.3;
    ctx.fillStyle = s.marker;
    ctx.fillRect(markerX - 1.5, top, 3, h);
    ctx.beginPath();
    ctx.moveTo(markerX - 7, top - 8);
    ctx.lineTo(markerX + 7, top - 8);
    ctx.lineTo(markerX, top);
    ctx.closePath();
    ctx.fill();

    const ms = performance.now() - started;
    this.stats.frames++;
    this.stats.lastDrawMs = ms;
    this.stats.totalDrawMs += ms;
    if (ms > this.stats.maxDrawMs) this.stats.maxDrawMs = ms;
  }

  /** Blits the tiles covering screen [sx0, sx1) in one colour, clipped to that span. */
  private blit(sx0: number, sx1: number, offset: number, which: "sung" | "up"): void {
    if (sx1 <= sx0) return;
    const ctx = this.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.rect(sx0, 0, sx1 - sx0, this.cssH);
    ctx.clip();
    const first = Math.floor((sx0 - offset) / TILE_CSS);
    const last = Math.floor((sx1 - offset) / TILE_CSS);
    for (let i = first; i <= last; i++) {
      const tile = this.tile(i)[which];
      if (tile) ctx.drawImage(tile, i * TILE_CSS + offset, this.laneY, TILE_CSS, this.laneH);
    }
    ctx.restore();
    // Render the next tile ahead of time, so it never costs a frame when it appears.
    if (which === "up") this.tile(last + 1);
  }
}

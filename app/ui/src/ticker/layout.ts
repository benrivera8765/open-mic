// SPDX-License-Identifier: GPL-3.0-or-later
// Lyrics ticker layout: where every grapheme sits on the horizontal strip, and which
// strip position is under the marker at each moment (WORKING ORDER 4.4, trap 9).
//
// 1. Placement. Every grapheme (the lyrics' `chars` entries) is measured on its own
//    and placed at a precomputed x, so ligatures and kerning can never move a letter
//    away from its timing. Words are separated by one measured space, lines by a gap
//    whose width depends on how long the singer rests (point 3).
//
// 2. Cells and knots. A grapheme's "cell" is its glyph box, widened at most halfway
//    into a neighbouring separator when that helps (see `shareSeparator`). The
//    position under the marker at time t:
//      - inside a grapheme's [start, end): moves through its cell, from its left edge
//        at `start` to its right edge at `end` (held notes move smoothly through the
//        letter);
//      - in a gap between graphemes: moves through the separator;
//    so the mapping is continuous. The knots (time, x) at every cell edge are then
//    interpolated by a monotone spline (motion.ts) that smooths the speed but can
//    never leave a cell during its grapheme's interval: the letter under the marker
//    is always the letter being sung.
//
// 3. Breaks. A line gap of dt seconds is v * dt wide, v = (visible width right of the
//    marker) / LEAD, so the next line enters at the right edge LEAD seconds before it
//    is sung and moves in at a steady pace. Long breaks are capped: the old line moves
//    out to the left, the strip rests (a countdown shows), and the next line comes in
//    from the right during the last LEAD seconds. The intro works the same way.
//
// 4. Lookahead. Text sung within the next LOOKAHEAD seconds must already be fully on
//    screen right of the marker. Breaks satisfy that by construction (LEAD > LOOKAHEAD);
//    for very dense singing the font is scaled down (to no less than MIN_FONT_SCALE)
//    until it holds.
//
// Pure TypeScript with an injected `measure` function: the tests use fixed per-character
// widths, the renderer uses CanvasRenderingContext2D.measureText.
import type { Lyrics } from "../api/types";
import { MonotoneSpline, type Knot } from "./motion";

/** Width in CSS px of `text` drawn at `fontPx`. */
export type Measure = (text: string, fontPx: number) => number;

export interface TickerParams {
  /** Width of the ticker lane, CSS px. */
  width: number;
  /** Marker position, 0..1 from the left (settings.ui.marker_position). */
  marker: number;
  /** Requested font size, CSS px (settings.ui.lyrics_text_size scaled to the window). */
  fontPx: number;
  /** Seconds of upcoming text that must be visible right of the marker. */
  lookahead?: number;
  /** Seconds a line is visible before it is sung after a break. */
  lead?: number;
  /** Smallest font scale the lookahead rule may apply. */
  minFontScale?: number;
}

export const LOOKAHEAD_S = 2;
export const LEAD_S = 3;
export const MIN_FONT_SCALE = 0.6;
/** Breaks at least this long show a countdown. */
export const COUNTDOWN_MIN_GAP_S = 4;

/** One grapheme on the strip. */
export interface PlacedGlyph {
  g: string;
  /** Glyph box on the strip (advance width), CSS px at the effective font size. */
  x: number;
  w: number;
  start: number;
  end: number;
  line: number;
  word: number;
  /** Cell: the strip span that counts as "this letter is under the marker". */
  L: number;
  R: number;
  /** False for zero-length graphemes, which are never "being sung". */
  timed: boolean;
}

export interface PlacedLine {
  start: number;
  end: number;
  x0: number;
  x1: number;
  first: number;
  last: number;
}

/** A rest before a line (or the intro): when to show a countdown. */
export interface Rest {
  /** End of the previous line (0 for the intro). */
  from: number;
  /** Start of the next line. */
  to: number;
  nextLine: number;
  countdown: boolean;
}

export interface TickerLayout {
  width: number;
  marker: number;
  markerX: number;
  /** Effective font size after the lookahead rule. */
  fontPx: number;
  fontScale: number;
  spaceWidth: number;
  glyphs: PlacedGlyph[];
  /** Indices of the timed graphemes, in order. */
  timedIndex: number[];
  lines: PlacedLine[];
  rests: Rest[];
  knots: Knot[];
  motion: MonotoneSpline;
  /** Strip span that holds text. */
  textStart: number;
  textEnd: number;
  /** Seconds of text that is visible ahead in the worst case (diagnostics). */
  worstLookahead: number;
}

const EPS_T = 1e-6;

interface Flat {
  g: string;
  start: number;
  end: number;
  line: number;
  word: number;
}

/** Graphemes in order with sanitised times (monotone, start <= end). */
function flatten(lyrics: Lyrics): Flat[] {
  const out: Flat[] = [];
  let prevEnd = -Infinity;
  lyrics.lines.forEach((line, li) => {
    line.words.forEach((w, wi) => {
      for (const c of w.chars) {
        const start = Math.max(c[1], prevEnd);
        const end = Math.max(c[2], start);
        out.push({ g: c[0], start, end, line: li, word: wi });
        prevEnd = end;
      }
    });
  });
  return out;
}

/**
 * Splits a separator of width `sep` (time `d0`) between itself and the cells of the
 * graphemes on either side (widths w1, w2, durations d1, d2) so their average speeds
 * are as equal as possible, giving each neighbour at most half the separator. With
 * d0 = 0 this gives exactly half to each side (the separator must be crossed while
 * one of them is sung). Returns the widths added to the left and right cells.
 */
export function shareSeparator(sep: number, d0: number, w1: number, d1: number, w2: number, d2: number): [number, number] {
  if (sep <= 0) return [0, 0];
  const half = sep / 2;
  const give = (sigma: number): [number, number, number] => {
    const a1 = Math.min(half, Math.max(0, sigma * d1 - w1));
    const a2 = Math.min(half, Math.max(0, sigma * d2 - w2));
    const a0 = sigma * d0;
    return [a1, a0, a2];
  };
  const total = (sigma: number) => {
    const [a1, a0, a2] = give(sigma);
    return a1 + a0 + a2;
  };
  if (d0 <= EPS_T) return [half, half];
  // Bisection on the common speed sigma.
  let lo = 0;
  let hi = sep / d0; // separator alone at this speed already covers it
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2;
    if (total(mid) < sep) lo = mid;
    else hi = mid;
  }
  const [a1, , a2] = give(hi);
  return [a1, a2];
}

function placeOnce(lyrics: Lyrics, p: Required<TickerParams>, fontPx: number, measure: Measure): Omit<TickerLayout, "worstLookahead" | "fontScale"> {
  const flat = flatten(lyrics);
  const cache = new Map<string, number>();
  const widthOf = (g: string) => {
    let w = cache.get(g);
    if (w === undefined) {
      w = Math.max(0, measure(g, fontPx));
      cache.set(g, w);
    }
    return w;
  };
  const space = widthOf(" ") || 0.28 * fontPx;
  const markerX = p.width * p.marker;
  const leftW = markerX;
  const rightW = p.width - markerX;
  const v = rightW / p.lead; // cruise speed through breaks, px/s
  const minGap = Math.max(2.5 * space, 0.9 * fontPx);

  // --- 1. placement --------------------------------------------------------
  const glyphs: PlacedGlyph[] = [];
  const lines: PlacedLine[] = [];
  /** For each line > 0: the plan of its preceding break (sub-segments of the gap). */
  const breakPlans = new Map<number, { dt: number; dx: number }[]>();
  let x = 0;
  let prevLineEnd = 0;
  let li = -1;
  let wi = -1;
  for (let i = 0; i < flat.length; i++) {
    const f = flat[i];
    if (f.line !== li) {
      if (li >= 0) {
        lines[lines.length - 1].x1 = x;
        const dt = Math.max(0, f.start - prevLineEnd);
        const plan = breakPlan(dt, true, v, leftW, rightW, minGap, p.lead);
        breakPlans.set(f.line, plan);
        x += plan.reduce((a, s) => a + s.dx, 0);
      }
      lines.push({ start: f.start, end: f.end, x0: x, x1: x, first: glyphs.length, last: glyphs.length });
      li = f.line;
      wi = f.word;
    } else if (f.word !== wi) {
      x += space;
      wi = f.word;
    }
    const w = widthOf(f.g);
    glyphs.push({ g: f.g, x, w, start: f.start, end: f.end, line: f.line, word: f.word, L: x, R: x + w, timed: f.end - f.start > EPS_T });
    x += w;
    const line = lines[lines.length - 1];
    line.end = f.end;
    line.last = glyphs.length - 1;
    prevLineEnd = f.end;
  }
  if (lines.length > 0) lines[lines.length - 1].x1 = x;

  // --- 2. cells: share separators between timed neighbours ------------------
  const timed = glyphs.map((g, i) => (g.timed ? i : -1)).filter((i) => i >= 0);
  const knots: Knot[] = [];
  const push = (t: number, xx: number) => {
    const last = knots[knots.length - 1];
    if (last && t <= last.t + EPS_T) {
      // Same instant: keep the later (rightmost) position; the spline needs increasing t.
      if (xx > last.x) last.x = xx;
      return;
    }
    knots.push({ t, x: Math.max(xx, last ? last.x : xx) });
  };

  if (timed.length === 0) {
    // Nothing timed (empty lyrics): a still strip.
    const k0 = { t: 0, x: 0 };
    return { width: p.width, marker: p.marker, markerX, fontPx, spaceWidth: space, glyphs, timedIndex: timed, lines, rests: [], knots: [k0], motion: new MonotoneSpline([k0]), textStart: 0, textEnd: x };
  }

  // Cell edges first (left-to-right, each separator once).
  for (let n = 0; n + 1 < timed.length; n++) {
    const a = glyphs[timed[n]];
    const b = glyphs[timed[n + 1]];
    const lineBreak = b.line !== a.line;
    const plan = lineBreak ? breakPlans.get(b.line) : undefined;
    if (plan && plan.length > 1) continue; // long break: crossed during the rest, cells stay tight
    const sepStart = a.x + a.w;
    const sepEnd = b.x;
    const sep = sepEnd - sepStart;
    const [add1, add2] = shareSeparator(sep, b.start - a.end, a.R - a.L, a.end - a.start, b.R - b.L, b.end - b.start);
    a.R = sepStart + add1;
    b.L = sepEnd - add2;
  }
  // Untimed graphemes take no time: their cell is empty (never under the marker while sung).
  for (const g of glyphs) if (!g.timed) g.L = g.R = g.x;

  // --- 3. knots --------------------------------------------------------------
  const first = glyphs[timed[0]];
  const rests: Rest[] = [];
  // Intro: the first line enters from the right edge during the last LEAD seconds.
  const intro = breakPlan(Math.max(0, first.start), false, v, leftW, rightW, minGap, p.lead);
  {
    const introWidth = intro.reduce((a, s) => a + s.dx, 0);
    let t = first.start - intro.reduce((a, s) => a + s.dt, 0);
    let xx = first.L - introWidth;
    push(t, xx);
    for (const s of intro) {
      t += s.dt;
      xx += s.dx;
      push(t, xx);
    }
    rests.push({ from: 0, to: first.start, nextLine: first.line, countdown: first.start >= COUNTDOWN_MIN_GAP_S });
  }
  for (let n = 0; n < timed.length; n++) {
    const g = glyphs[timed[n]];
    push(g.start, g.L);
    push(g.end, g.R);
    const next = n + 1 < timed.length ? glyphs[timed[n + 1]] : null;
    if (!next) break;
    const plan = next.line !== g.line ? breakPlans.get(next.line) : undefined;
    if (plan && plan.length > 1) {
      // Long break: exit, rest, enter. Sub-segment widths were planned for the gap
      // alone; graphemes without time at the line ends ride along in the first and
      // last sub-segment.
      const extraBefore = lines[g.line].x1 - g.R;
      const extraAfter = next.L - lines[next.line].x0;
      let t = g.end;
      let xx = g.R;
      plan.forEach((s, k) => {
        t += s.dt;
        xx += s.dx + (k === 0 ? extraBefore : 0) + (k === plan.length - 1 ? extraAfter : 0);
        push(Math.min(t, next.start), Math.min(xx, next.L));
      });
    }
    if (next.line !== g.line) {
      const dt = next.start - g.end;
      rests.push({ from: g.end, to: next.start, nextLine: next.line, countdown: dt >= COUNTDOWN_MIN_GAP_S });
    }
  }
  // Outro: the last line moves out to the left at the cruise speed, then rests.
  const last = glyphs[timed[timed.length - 1]];
  const tailText = Math.max(0, x - last.R);
  push(last.end + (leftW + tailText) / v, last.R + leftW + tailText);

  return {
    width: p.width,
    marker: p.marker,
    markerX,
    fontPx,
    spaceWidth: space,
    glyphs,
    timedIndex: timed,
    lines,
    rests,
    knots,
    motion: new MonotoneSpline(knots),
    textStart: 0,
    textEnd: x,
  };
}

/**
 * Sub-segments (duration, distance) that cross a break of `dt` seconds.
 * Short and medium breaks: one segment at the cruise speed v (but at least `minGap`
 * wide between lines). Long breaks: exit (the previous line leaves to the left),
 * rest, enter (the next line comes in from the right edge over `lead` seconds).
 */
function breakPlan(dt: number, hasPrev: boolean, v: number, leftW: number, rightW: number, minGap: number, lead: number): { dt: number; dx: number }[] {
  const exitW = hasPrev ? leftW : 0;
  const full = exitW + rightW;
  if (v * dt <= full) {
    const width = hasPrev ? Math.max(minGap, v * dt) : v * dt;
    return [{ dt, dx: width }];
  }
  const exitDt = exitW / v;
  const rest = dt - exitDt - lead;
  const out: { dt: number; dx: number }[] = [];
  if (exitDt > 0) out.push({ dt: exitDt, dx: exitW });
  if (rest > 0) out.push({ dt: rest, dx: 0 });
  out.push({ dt: lead, dx: rightW });
  return out;
}

/**
 * Worst-case lookahead (seconds) of a layout: the smallest, over all times t at
 * which a grapheme starts within LOOKAHEAD seconds, of how much of the upcoming text
 * is fully visible. Also returns the largest overflow ratio (needed / available).
 */
export function lookaheadReport(l: TickerLayout, lookahead: number): { worst: number; ratio: number } {
  const rightW = l.width - l.markerX;
  let worst = Infinity;
  let ratio = 0;
  for (const g of l.glyphs) {
    // At t = start - lookahead the glyph must be fully visible (its right edge on screen).
    const t = g.start - lookahead;
    const need = g.x + g.w - l.motion.at(t);
    if (need > 0) ratio = Math.max(ratio, need / rightW);
    // When does this glyph's right edge first fit? Its lead time is start - that time.
    if (need > rightW) worst = Math.min(worst, lookahead * (rightW / need));
  }
  return { worst: worst === Infinity ? lookahead : worst, ratio };
}

/**
 * Lines without a single grapheme (`"words": []`, which SONG_FORMAT section 5 does not
 * forbid) take no room and no time on the strip. They are dropped before layout so
 * that line indices inside the layout (PlacedGlyph.line, `lines`, Rest.nextLine)
 * always refer to the same line; otherwise a break before such a line would look up
 * a missing PlacedLine and throw.
 */
function withoutEmptyLines(lyrics: Lyrics): Lyrics {
  const hasText = (l: Lyrics["lines"][number]) => l.words.some((w) => w.chars.length > 0);
  return lyrics.lines.every(hasText) ? lyrics : { ...lyrics, lines: lyrics.lines.filter(hasText) };
}

/**
 * Lays out the lyrics for a lane `width` px wide. Applies the lookahead rule by
 * scaling the font down when needed (never below `minFontScale`).
 * Line indices in the result count only lines that have at least one grapheme.
 */
export function layoutTicker(lyricsIn: Lyrics, params: TickerParams, measure: Measure): TickerLayout {
  const lyrics = withoutEmptyLines(lyricsIn);
  const p: Required<TickerParams> = {
    lookahead: LOOKAHEAD_S,
    lead: LEAD_S,
    minFontScale: MIN_FONT_SCALE,
    ...params,
    marker: Math.min(0.9, Math.max(0.05, params.marker)),
  };
  let scale = 1;
  let placed = placeOnce(lyrics, p, p.fontPx, measure);
  let report = lookaheadReport({ ...placed, fontScale: 1, worstLookahead: 0 }, p.lookahead);
  for (let iter = 0; iter < 6 && report.ratio > 0.98 && scale > p.minFontScale; iter++) {
    scale = Math.max(p.minFontScale, (scale * 0.97) / report.ratio);
    placed = placeOnce(lyrics, p, p.fontPx * scale, measure);
    report = lookaheadReport({ ...placed, fontScale: scale, worstLookahead: 0 }, p.lookahead);
  }
  return { ...placed, fontScale: scale, worstLookahead: report.worst };
}

/** Index of the grapheme whose cell contains strip position `x` (binary search), or -1. */
export function glyphAtX(l: TickerLayout, x: number): number {
  // Cells of timed graphemes are disjoint and ordered; untimed ones have empty cells.
  const idx = l.timedIndex;
  let lo = 0;
  let hi = idx.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const g = l.glyphs[idx[mid]];
    if (x < g.L) hi = mid - 1;
    else if (x >= g.R) lo = mid + 1;
    else return idx[mid];
  }
  return -1;
}

/** The rest (break) at time t, if a countdown should show; with seconds left. */
export function restAt(l: TickerLayout, t: number): { rest: Rest; remaining: number } | null {
  for (const r of l.rests) {
    if (t >= r.from && t < r.to) return r.countdown ? { rest: r, remaining: r.to - t } : null;
  }
  return null;
}

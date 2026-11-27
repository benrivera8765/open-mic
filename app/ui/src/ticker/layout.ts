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
//    for dense singing the font is scaled down in two steps:
//      a. the whole song, to no less than MIN_FONT_SCALE, so a song keeps one text size
//         where that is enough;
//      b. then only the lines that still do not fit, each on its own, to no less than
//         MIN_LINE_SCALE. Every grapheme of a line has that line's size; all lines share
//         one baseline. A rap verse gets smaller text while the rest of the song keeps
//         its size (D-167).
//
// 5. Pace (settings.ui.lyrics_motion, D-216). Point 2 describes the "letter" pace:
//    exact to the letter, but letters of fixed width sung at very different speeds make
//    the text surge and brake within every word. The "word" pace (the default) keeps
//    the same placement, breaks and lookahead rule and replaces the knots of point 2 by
//    word-level knots (`wordKnots`): a word's first letter reaches the marker when the
//    word starts (give or take WORD_TOLERANCE_S), a word carries the space after it at
//    its own pace, a short pause lets the word just sung stay at the marker a little
//    longer, and the motion between the knots is a smooth speed profile with one steady
//    pace per word (wordmotion.ts).
//

// Pure TypeScript with an injected `measure` function: the tests use fixed per-character
// widths, the renderer uses CanvasRenderingContext2D.measureText.
import type { LyricsMotion, Lyrics } from "../api/types";
import { backingWords } from "../lib/lyrics";
import { MonotoneSpline, type Knot } from "./motion";
import { type FloatingKnot, LINGER_S, type Motion, RampMotion, straightenKnots, WORD_TOLERANCE_S } from "./wordmotion";

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
  /** Smallest font scale the lookahead rule may apply to the whole song. */
  minFontScale?: number;
  /** Smallest font scale the lookahead rule may apply to a single dense line. */
  minLineScale?: number;
  /**
   * "letter": exact to the letter (D-94, D-95); "word": smooth, paced by word (D-216).
   * The app passes settings.ui.lyrics_motion (default "word"); left out, the layout is
   * letter-exact as it always was.
   */
  pace?: LyricsMotion;
}

export const LOOKAHEAD_S = 2;
export const LEAD_S = 3;
export const MIN_FONT_SCALE = 0.6;
/**
 * Floor for single dense lines: 0.45 x 56 px = 25.2 px at 1440x900 and 22.4 px at
 * 1280x800 with the default text size (D-167).
 */
export const MIN_LINE_SCALE = 0.45;
/** Breaks at least this long show a countdown. */
export const COUNTDOWN_MIN_GAP_S = 4;

/** One grapheme on the strip. */
export interface PlacedGlyph {
  g: string;
  /** Glyph box on the strip (advance width), CSS px at the effective font size. */
  x: number;
  w: number;
  /** Font size this grapheme is measured and drawn at (its line's size), CSS px. */
  px: number;
  start: number;
  end: number;
  line: number;
  word: number;
  /** Cell: the strip span that counts as "this letter is under the marker". */
  L: number;
  R: number;
  /** False for zero-length graphemes, which are never "being sung". */
  timed: boolean;
  /** Backing vocals (its word is in parentheses; owner 2026-11-27, D-282): drawn dimmed, in
   * the upcoming colour even once sung. */
  backing?: boolean;
}

export interface PlacedLine {
  start: number;
  end: number;
  x0: number;
  x1: number;
  first: number;
  last: number;
  /** Font size of this line, CSS px (the layout's fontPx unless the line is dense). */
  px: number;
  /** Width of a word space in this line. */
  space: number;
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

/** A word on the strip, by its timed letters (zero-length letters ride along). */
export interface PlacedWord {
  line: number;
  word: number;
  /** First and last grapheme of the word (timed or not). */
  first: number;
  last: number;
  /** Left edge of the first timed grapheme, right edge of the last one. */
  a: number;
  b: number;
  /** Start of the first timed grapheme, end of the last one. */
  start: number;
  end: number;
}

export interface TickerLayout {
  width: number;
  marker: number;
  markerX: number;
  /** How the text is paced (TickerParams.pace). */
  pace: LyricsMotion;
  /** Effective font size of the song after the lookahead rule (dense lines may be smaller). */
  fontPx: number;
  fontScale: number;
  /** Smallest scale of any line (equals fontScale unless some lines were made smaller). */
  minLineScale: number;
  /** Number of lines drawn smaller than fontPx. */
  denseLines: number;
  spaceWidth: number;
  glyphs: PlacedGlyph[];
  /** Indices of the timed graphemes, in order. */
  timedIndex: number[];
  /** Words with at least one timed grapheme, in order. */
  words: PlacedWord[];
  lines: PlacedLine[];
  rests: Rest[];
  knots: Knot[];
  /** Letter pace: a MonotoneSpline through the cell edges; word pace: a RampMotion. */
  motion: Motion;
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

type Placed = Omit<TickerLayout, "worstLookahead" | "fontScale" | "minLineScale" | "denseLines">;

/**
 * Places the lyrics at font size `fontPx`; `factor[line]` (<= 1) makes single lines
 * smaller (null: every line at `fontPx`). Line indices are those of `lyrics.lines`
 * after `withoutEmptyLines`, which equal the indices of the placed lines.
 */
function placeOnce(lyrics: Lyrics, p: Required<TickerParams>, fontPx: number, measure: Measure, factor: ArrayLike<number> | null = null): Placed {
  const flat = flatten(lyrics);
  const backing = backingWords(lyrics);
  const caches = new Map<number, Map<string, number>>();
  const widthOf = (g: string, px: number) => {
    let cache = caches.get(px);
    if (!cache) {
      cache = new Map<string, number>();
      caches.set(px, cache);
    }
    let w = cache.get(g);
    if (w === undefined) {
      w = Math.max(0, measure(g, px));
      cache.set(g, w);
    }
    return w;
  };
  const spaceAt = (px: number) => widthOf(" ", px) || 0.28 * px;
  const pxOf = (line: number) => (factor ? fontPx * factor[line] : fontPx);
  const space = spaceAt(fontPx);
  const markerX = p.width * p.marker;
  const leftW = markerX;
  const rightW = p.width - markerX;
  const v = rightW / p.lead; // cruise speed through breaks, px/s
  // Smallest gap between two lines: set by the larger of the two.
  const minGapAt = (px: number) => Math.max(2.5 * spaceAt(px), 0.9 * px);

  // --- 1. placement --------------------------------------------------------
  const glyphs: PlacedGlyph[] = [];
  const lines: PlacedLine[] = [];
  /** For each line > 0: the plan of its preceding break (sub-segments of the gap). */
  const breakPlans = new Map<number, { dt: number; dx: number }[]>();
  let x = 0;
  let prevLineEnd = 0;
  let li = -1;
  let wi = -1;
  let px = fontPx;
  let lineSpace = space;
  for (let i = 0; i < flat.length; i++) {
    const f = flat[i];
    if (f.line !== li) {
      const nextPx = pxOf(f.line);
      if (li >= 0) {
        lines[lines.length - 1].x1 = x;
        const dt = Math.max(0, f.start - prevLineEnd);
        // A short gap next to a dense line shrinks with it (the passage is zoomed out
        // as a whole); long breaks keep their timing (the next line enters LEAD s ahead).
        const zoom = factor ? Math.min(factor[li], factor[f.line]) : 1;
        const plan = breakPlan(dt, true, v, leftW, rightW, minGapAt(Math.max(px, nextPx)), p.lead, zoom);
        breakPlans.set(f.line, plan);
        x += plan.reduce((a, s) => a + s.dx, 0);
      }
      px = nextPx;
      lineSpace = spaceAt(px);
      lines.push({ start: f.start, end: f.end, x0: x, x1: x, first: glyphs.length, last: glyphs.length, px, space: lineSpace });
      li = f.line;
      wi = f.word;
    } else if (f.word !== wi) {
      x += lineSpace;
      wi = f.word;
    }
    const w = widthOf(f.g, px);
    glyphs.push({ g: f.g, x, w, px, start: f.start, end: f.end, line: f.line, word: f.word, L: x, R: x + w, timed: f.end - f.start > EPS_T, backing: backing[f.line]?.[f.word] === true });
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
    return { width: p.width, marker: p.marker, markerX, pace: p.pace, fontPx, spaceWidth: space, glyphs, timedIndex: timed, words: [], lines, rests: [], knots: [k0], motion: new MonotoneSpline([k0]), textStart: 0, textEnd: x };
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
  const last = glyphs[timed[timed.length - 1]];
  // Rests: the intro and every line change (a countdown shows for long ones).
  const rests: Rest[] = [{ from: 0, to: first.start, nextLine: first.line, countdown: first.start >= COUNTDOWN_MIN_GAP_S }];
  for (let n = 0; n + 1 < timed.length; n++) {
    const g = glyphs[timed[n]];
    const next = glyphs[timed[n + 1]];
    if (next.line !== g.line) rests.push({ from: g.end, to: next.start, nextLine: next.line, countdown: next.start - g.end >= COUNTDOWN_MIN_GAP_S });
  }
  const words = placeWords(glyphs);
  // Intro: the first line enters from the right edge during the last LEAD seconds.
  const intro = breakPlan(Math.max(0, first.start), false, v, leftW, rightW, 0, p.lead);
  // Outro: the last line moves out to the left at the cruise speed, then rests.
  const tailText = Math.max(0, x - last.R);
  const outro = { t: last.end + (leftW + tailText) / v, x: last.R + leftW + tailText };
  const base = { width: p.width, marker: p.marker, markerX, pace: p.pace, fontPx, spaceWidth: space, glyphs, timedIndex: timed, words, lines, rests, textStart: 0, textEnd: x };
  if (p.pace === "word") {
    const knots = straightenKnots(wordKnots(words, lines, breakPlans, intro, outro));
    return { ...base, knots, motion: new RampMotion(knots) };
  }
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
  }
  push(outro.t, outro.x);
  return { ...base, knots, motion: new MonotoneSpline(knots) };
}

/** Groups the graphemes into words; words without a timed grapheme are left out. */
export function placeWords(glyphs: PlacedGlyph[]): PlacedWord[] {
  const out: PlacedWord[] = [];
  for (let i = 0; i < glyphs.length; ) {
    const g0 = glyphs[i];
    let j = i;
    while (j + 1 < glyphs.length && glyphs[j + 1].line === g0.line && glyphs[j + 1].word === g0.word) j++;
    let ft = -1;
    let lt = -1;
    for (let k = i; k <= j; k++) {
      if (!glyphs[k].timed) continue;
      if (ft < 0) ft = k;
      lt = k;
    }
    if (ft >= 0) out.push({ line: g0.line, word: g0.word, first: i, last: j, a: glyphs[ft].x, b: glyphs[lt].x + glyphs[lt].w, start: glyphs[ft].start, end: glyphs[lt].end });
    i = j + 1;
  }
  return out;
}

/**
 * Knots of the word pace (point 5). For each word: its first timed letter reaches the
 * marker at the word's start, free to move by WORD_TOLERANCE_S (straightenKnots). A
 * word has no knot at its end: it carries the space after it, crossing word and space
 * at one pace, so the text never dashes across a space. Only a pause longer than that
 * pace needs by more than LINGER_S puts a knot LINGER_S after the word's end; the rest
 * of the pause crosses the space more slowly. Line changes use the breaks of point 3
 * exactly as the letter pace does (a short line gap counts as the space after the last
 * word); knots after an intro or a long break and around a long break are fixed.
 */
export function wordKnots(words: PlacedWord[], lines: PlacedLine[], breakPlans: Map<number, { dt: number; dx: number }[]>, intro: { dt: number; dx: number }[], outro: Knot): FloatingKnot[] {
  const out: FloatingKnot[] = [];
  const push = (t: number, xx: number, slack = 0) => {
    const last = out[out.length - 1];
    if (last && t <= last.t + EPS_T) {
      // Same instant (the end of an intro or a long break): one fixed knot, rightmost x.
      if (xx > last.x) last.x = xx;
      last.lo = last.hi = last.t;
      return;
    }
    out.push({ t, x: Math.max(xx, last ? last.x : xx), lo: t - slack, hi: t + slack });
  };
  const first = words[0];
  let t = first.start - intro.reduce((a, s) => a + s.dt, 0);
  let xx = first.a - intro.reduce((a, s) => a + s.dx, 0);
  push(t, xx);
  for (const s of intro) {
    t += s.dt;
    xx += s.dx;
    push(t, xx);
  }
  for (let k = 0; k < words.length; k++) {
    const w = words[k];
    push(w.start, w.a, WORD_TOLERANCE_S);
    const next = words[k + 1];
    if (!next) {
      push(w.end, w.b);
      break;
    }
    const plan = next.line !== w.line ? breakPlans.get(next.line) : undefined;
    if (plan && plan.length > 1) {
      // Long break: exit, rest, enter, as for the letter pace. The plan runs from the
      // end of the line (its last grapheme, timed or not) to the start of the next one.
      // Zero-length graphemes at the line ends ride along in the first and last
      // sub-segment when they share the time of the nearest sung word; at a time of
      // their own, the text glides over them up to that time (starting the plan at the
      // sung word instead put the next line at the marker early, standing still there).
      const lineA = lines[w.line];
      const lineB = lines[next.line];
      const tailOwn = lineA.end > w.end + EPS_T;
      const headOwn = lineB.start < next.start - EPS_T;
      push(w.end, w.b);
      if (tailOwn) push(lineA.end, lineA.x1);
      const extraBefore = tailOwn ? 0 : lineA.x1 - w.b;
      const extraAfter = headOwn ? 0 : next.a - lineB.x0;
      let tt = tailOwn ? lineA.end : w.end;
      let x2 = tailOwn ? lineA.x1 : w.b;
      plan.forEach((s, i) => {
        tt += s.dt;
        x2 += s.dx + (i === 0 ? extraBefore : 0) + (i === plan.length - 1 ? extraAfter : 0);
        push(Math.min(tt, next.start), Math.min(x2, next.a));
      });
      continue;
    }
    // A pause before the next word: crossing word and space at one steady speed would
    // keep the word's last letter at the marker `linger` seconds into the pause. Up to
    // LINGER_S that is what happens (the text keeps gliding through a short pause);
    // beyond it the word ends on a knot LINGER_S into the pause and the space is
    // crossed more slowly for the rest of it.
    const sep = next.a - w.b;
    const gap = next.start - w.end;
    const wordW = w.b - w.a;
    const linger = (wordW * gap - sep * (w.end - w.start)) / (wordW + sep);
    if (gap > EPS_T && linger > LINGER_S) push(w.end + LINGER_S, w.b, WORD_TOLERANCE_S);
  }
  push(outro.t, outro.x);
  return out;
}

/**
 * Sub-segments (duration, distance) that cross a break of `dt` seconds.
 * Short and medium breaks: one segment at the cruise speed v, times `zoom` (the
 * smaller line factor of a dense passage), but at least `minGap` wide between lines. Long breaks: exit (the previous line leaves to the left),
 * rest, enter (the next line comes in from the right edge over `lead` seconds).
 */
function breakPlan(dt: number, hasPrev: boolean, v: number, leftW: number, rightW: number, minGap: number, lead: number, zoom = 1): { dt: number; dx: number }[] {
  const exitW = hasPrev ? leftW : 0;
  const full = exitW + rightW;
  if (v * dt <= full) {
    const width = hasPrev ? Math.max(minGap, v * dt * zoom) : v * dt;
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
 * Per line: the factor (<= 1) by which that line's text would have to shrink so that
 * every grapheme is fully visible `lookahead` seconds before it is sung, with the
 * needed width at most `target` x the width right of the marker. A grapheme that does
 * not fit (needs more than `trigger` x that width) asks every line between the one
 * under the marker and its own line to shrink by the same factor. The short gaps
 * between those lines shrink with them; only a gap or intro before the first of them
 * is left out of that factor.
 */
function lineShrink(l: Placed, lookahead: number, trigger: number, target: number): Float64Array {
  const rightW = l.width - l.markerX;
  const lines = l.lines;
  const want = new Float64Array(lines.length).fill(1);
  /** First line whose text ends right of strip position x. */
  const lineFrom = (x: number) => {
    let lo = 0;
    let hi = lines.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lines[mid].x1 <= x) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  };
  for (const g of l.glyphs) {
    const xa = l.motion.at(g.start - lookahead);
    const xb = g.x + g.w;
    const need = xb - xa;
    if (need <= trigger * rightW) continue;
    const a = Math.min(lineFrom(xa), g.line);
    const fixed = Math.max(0, lines[a].x0 - xa);
    const text = need - fixed;
    const f = text > 0 ? Math.max(0, (target * rightW - fixed) / text) : 1;
    for (let k = a; k <= g.line; k++) want[k] = Math.min(want[k], f);
  }
  return want;
}

/**
 * Lines without a single grapheme (`"words": []`, which SONG_FORMAT section 5 does not
 * forbid) take no room and no time on the strip. They are dropped before layout so
 * that line indices inside the layout (PlacedGlyph.line, `lines`, Rest.nextLine)
 * always refer to the same line; otherwise a break before such a line would look up
 * a missing PlacedLine and throw.
 */
export function withoutEmptyLines(lyrics: Lyrics): Lyrics {
  const hasText = (l: Lyrics["lines"][number]) => l.words.some((w) => w.chars.length > 0);
  return lyrics.lines.every(hasText) ? lyrics : { ...lyrics, lines: lyrics.lines.filter(hasText) };
}

/**
 * Lays out the lyrics for a lane `width` px wide. Applies the lookahead rule by
 * scaling the whole song's font down when needed (never below `minFontScale`), then
 * single lines that still do not fit (never below `minLineScale`).
 * Line indices in the result count only lines that have at least one grapheme.
 */
export function layoutTicker(lyricsIn: Lyrics, params: TickerParams, measure: Measure): TickerLayout {
  const lyrics = withoutEmptyLines(lyricsIn);
  const p: Required<TickerParams> = {
    lookahead: LOOKAHEAD_S,
    lead: LEAD_S,
    minFontScale: MIN_FONT_SCALE,
    minLineScale: MIN_LINE_SCALE,
    pace: "letter",
    ...params,
    marker: Math.min(0.9, Math.max(0.05, params.marker)),
  };
  const asLayout = (pl: Placed, fontScale: number): TickerLayout => ({ ...pl, fontScale, minLineScale: fontScale, denseLines: 0, worstLookahead: 0 });
  // a. The whole song.
  let scale = 1;
  let placed = placeOnce(lyrics, p, p.fontPx, measure);
  let report = lookaheadReport(asLayout(placed, 1), p.lookahead);
  for (let iter = 0; iter < 6 && report.ratio > 0.98 && scale > p.minFontScale; iter++) {
    scale = Math.max(p.minFontScale, (scale * 0.97) / report.ratio);
    placed = placeOnce(lyrics, p, p.fontPx * scale, measure);
    report = lookaheadReport(asLayout(placed, scale), p.lookahead);
  }
  // b. Single dense lines. The factor is relative to the song's size; shrinking one
  // line can move where the marker is 2 s earlier, so repeat until nothing changes.
  const floor = Math.min(1, p.minLineScale / scale);
  let minFactor = 1;
  let dense = 0;
  if (report.ratio > 0.98 && floor < 1) {
    const factor = new Float64Array(placed.lines.length).fill(1);
    for (let iter = 0; iter < 16; iter++) {
      const want = lineShrink(placed, p.lookahead, 0.995, 0.97);
      let changed = false;
      for (let k = 0; k < factor.length; k++) {
        const f = Math.max(floor, factor[k] * want[k]);
        if (f < factor[k] - 1e-6) {
          factor[k] = f;
          changed = true;
        }
      }
      if (!changed) break;
      placed = placeOnce(lyrics, p, p.fontPx * scale, measure, factor);
    }
    for (const f of factor) {
      minFactor = Math.min(minFactor, f);
      if (f < 1) dense++;
    }
    report = lookaheadReport(asLayout(placed, scale), p.lookahead);
  }
  return { ...placed, fontScale: scale, minLineScale: scale * minFactor, denseLines: dense, worstLookahead: report.worst };
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

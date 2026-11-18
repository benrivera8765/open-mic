// SPDX-License-Identifier: GPL-3.0-or-later
// Page display of the lyrics: static lines with a colour wipe (settings.ui.lyrics_display
// = "page", the default since the owner's request of 18 Nov 2026, D-218).
//
// The owner, after the word-paced ticker: "instead of the words moving from like a
// conveyor belt I instead want the phrase of the lyrics to appear on screen, stay
// static, while the progress bar goes over which word is being sung" (with two frames of
// a karaoke video: bold white words with a dark outline, the sung part gold, the colour
// boundary sweeping through the letters, a small gold bar left of the line before it).
//
// 1. Lines. The current line stands still, centred in the lyrics area and large; the
//    next line is shown underneath, smaller and dimmer (settings.ui.show_next_line), so
//    the singer reads ahead (WORK_ORDER 4.4: 2-3 s). When a line has been sung, the next
//    one moves up into its place within TRANSITION_S and the line after it fades in
//    below. Every state is a pure function of the lyric time (`pageFrame`), so pausing,
//    seeking and the lyrics offset need nothing special.
// 2. Wipe. The sung colour sweeps left to right through the current line, row by row in
//    reading order. Where its edge is at time t is the line's own motion, built with the
//    ticker's pacing unchanged (layout.ts point 5, wordmotion.ts): for the word pace (the
//    default) word-level knots through RampMotion: every word's first letter is reached
//    when the word starts (within 40 ms), a word carries the space after it at its own
//    pace, speed and acceleration are continuous, the edge never runs backwards and never
//    stops while the line is sung; for the letter pace the letter cells and the monotone
//    spline (D-94, D-95). A line has a run-up before its first word and a run-out after
//    its last, both outside the text, so the wipe enters the first letter and leaves the
//    last at its own steady speed instead of starting from rest.
// 3. Fit. A line too wide for the area at the chosen size is drawn smaller, down to
//    PAGE_MIN_SCALE; still too wide, it is wrapped onto two (at most MAX_ROWS) rows of
//    about equal width, each as large as fits (below the floor only a single word wider
//    than the area, or a line too long for MAX_ROWS rows at the floor: from about 30
//    words of 4-5 letters). If the area is too short for a line and the preview
//    under it, the whole song is drawn smaller. Nothing overflows or is clipped.
// 4. Pauses. Before a line that follows a pause of at least LEADIN_MIN_GAP_S, a lead-in
//    bar left of the line drains to nothing exactly when its first word starts. A rest
//    of at least COUNTDOWN_MIN_GAP_S (the intro included) clears the screen and shows the
//    countdown ("Singing in 0:08") as the ticker does; the next line appears ENTRY_S
//    (3 s, the ticker's entry) before it is sung, with its lead-in bar.
// 5. Phrases. A two-line page can show a line only from the moment the line above it
//    becomes current, so the line after a short one (lyrics transcribed in fragments
//    such as "from a" / "distant planet") would be readable for less than LOOKAHEAD_S.
//    Such lines are joined into one display line with the lines that follow them (only
//    across pauses shorter than PHRASE_MAX_GAP_S, and only while the phrase fits two
//    rows, at PAGE_MIN_SCALE at the least) until the next display line is shown
//    LOOKAHEAD_S ahead.
//    The wipe runs on through the joined lines as through the words of one line.
// 6. Progress needle (settings.ui.show_marker, the owner's request of 18 Nov 2026 to bring
//    back "the bar ... that would mark the progress of where the song is"): the scrolling
//    ticker's marker needle (renderer.ts `drawNeedle`, colour settings.ui.colors.marker)
//    rides the wipe edge: its x is the gold/white boundary of the line in the current slot
//    on every frame (`pageFrame(...).needle`), on the row being wiped, as tall as that row's
//    band plus NEEDLE_OVER_EM above and below. Before a line's first word it waits at the
//    first letter (the wipe's run-up lies left of the text), after the last word at the
//    end of the line; at a row break it jumps to the next row's start when the wipe enters
//    that row; at a line change it moves with the next line as that line moves up into
//    place (it is on the entering line from the change on). Rests with a countdown hide it:
//    it fades out with the line before the rest and fades in with the line after it.
//
// Pure TypeScript with an injected `measure`, like layout.ts: the tests use fixed widths
// and the lyrics face's width table, the renderer measures with Canvas 2D.
import type { Lyrics, LyricsMotion } from "../api/types";
import { COUNTDOWN_MIN_GAP_S, LEAD_S, LOOKAHEAD_S, type Measure, type PlacedGlyph, type PlacedWord, placeWords, shareSeparator, withoutEmptyLines, wordKnots } from "./layout";
import { type Knot, MonotoneSpline } from "./motion";
import { type Motion, RampMotion, straightenKnots } from "./wordmotion";

export interface PageParams {
  /** Size of the lyrics area, CSS px. */
  width: number;
  height: number;
  /** Requested font size, CSS px (settings.ui.lyrics_text_size scaled to the window). */
  fontPx: number;
  /** settings.ui.lyrics_motion (default "word"). */
  pace?: LyricsMotion;
  /** settings.ui.show_next_line (default true). */
  showNext?: boolean;
}

/** A line too wide for the area shrinks to this share of the song's size before it wraps. */
export const PAGE_MIN_SCALE = 0.7;
export const MAX_ROWS = 3;
/** Row pitch, em of the line's size. */
export const LINE_HEIGHT_EM = 1.25;
/** The next line's size and opacity relative to the current line. */
export const PREVIEW_SCALE = 0.62;
export const PREVIEW_ALPHA = 0.62;
/** Space between the current line and the preview, em of the song's size. */
export const PREVIEW_GAP_EM = 0.3;
/** A line change (the next line moving up, the old one fading out), seconds. */
export const TRANSITION_S = 0.2;
/** The fully sung line stays this long before the next one moves up (when the gap allows). */
export const SWITCH_HOLD_S = 0.05;
/** After a rest with a countdown (and after a long intro) a line appears this long before it is sung. */
export const ENTRY_S = LEAD_S;
/** A pause at least this long before a line gets a lead-in bar. */
export const LEADIN_MIN_GAP_S = 1;
/** Lines are joined into a phrase (point 5) only across pauses shorter than this. */
export const PHRASE_MAX_GAP_S = LEADIN_MIN_GAP_S;
/** Longest lead-in bar. */
export const LEADIN_MAX_S = 3;
/** The last line stays this long after it has been sung. */
export const OUTRO_HOLD_S = 2;
/** Run-up before a line's first word and run-out after its last (outside the text). */
export const RUN_S = 0.3;
/** Ink (outline, shadow, accents, descenders) reaches this far outside a row's band, em. */
export const INK_PAD_EM = 0.3;
/** The lead-in bar: width, gap to the line, nominal height (the cap height), em. */
export const BAR_W_EM = 0.7;
export const BAR_GAP_EM = 0.35;
export const CAP_EM = 0.7;
/**
 * The progress needle (point 6): the scrolling ticker's needle (3 px wide, a head 14 px
 * wide and 8 px tall at the default 56 px text, renderer.ts) in em of the line's size, so
 * it keeps that look at every size; it reaches NEEDLE_OVER_EM above and below the row's
 * band. Head plus overshoot (0.27 em) stay inside the row's ink pad (INK_PAD_EM).
 */
export const NEEDLE_W_EM = 3 / 56;
export const NEEDLE_HEAD_W_EM = 14 / 56;
export const NEEDLE_HEAD_H_EM = 8 / 56;
export const NEEDLE_OVER_EM = 0.12;
/** Space kept clear at the left and right of the area (share of its width, plus INK_PAD_EM). */
const SIDE_SHARE = 0.02;
/** A line leaving before a rest rises by up to this much while it fades out, em. */
const RISE_EM = 0.35;
/** A new preview line rises into its place by up to this much while it fades in, em of the song's size. */
const PREVIEW_RISE_EM = 0.3;
/** A leaving line is gone after this share of the transition (it moves up with the page). */
const LEAVE_FADE = 0.6;
const EPS_T = 1e-6;

/** One row of a line (a line has more than one only when it had to wrap). */
export interface PageRow {
  /** Glyph range, indices into PageLine.glyphs. */
  first: number;
  last: number;
  /** Reading-order span of the row's text (PlacedGlyph.x units). */
  u0: number;
  u1: number;
  /** Left edge of the row's text in the area at the line's own size, CSS px. */
  left: number;
  /** Top of the row's band inside the line block; rows are LINE_HEIGHT_EM tall. */
  top: number;
}

export interface PageLine {
  index: number;
  /** The song's lines shown as this display line (more than one when joined into a phrase, point 5). */
  sourceLines: number[];
  text: string;
  /** Font size of this line, CSS px. */
  px: number;
  /**
   * Graphemes in reading order. `x` is the position along the line as if it were one row
   * (spaces included), at `px`; L/R are the letter cells of the letter pace.
   */
  glyphs: PlacedGlyph[];
  /** Words with at least one timed grapheme. */
  words: PlacedWord[];
  rows: PageRow[];
  /** Horizontal centre the rows are placed around (the area's centre unless the lead-in bar needs room). */
  cx: number;
  /** Widest row and block height (rows x LINE_HEIGHT_EM x px), CSS px. */
  width: number;
  height: number;
  /** Reading-order length of the text. */
  uEnd: number;
  /** First timed letter's start and last timed letter's end. */
  start: number;
  end: number;
  /** Seconds since the previous line ended (since 0 for the first line). */
  gapBefore: number;
  knots: Knot[];
  /** Wipe position (reading order) at a lyric time. */
  motion: Motion;
  /** When the line starts to become the current line (-Infinity: from the start). */
  current: number;
  /** "up": moves up from the preview slot; "fade": fades in where it stands (after a rest, or without a preview). */
  enter: "up" | "fade";
  /** When it starts to leave (the next line's `current`, or after the last word before a rest). */
  leave: number;
  /** When it fades into the preview slot under the line before it (null: never previewed). */
  previewFrom: number | null;
  /** Top of the block as the current line, and in the preview slot (scaled), CSS px. */
  currentTop: number;
  previewTop: number;
  /** How far it moves up while it leaves (as far as the next line moves up, room allowing). */
  leaveRise: number;
  /** The lead-in bar drains from `from` to `to` (the first word's start). */
  leadIn: { from: number; to: number } | null;
  /** A rest with a countdown comes before this line. */
  breakBefore: boolean;
  shrunk: boolean;
  wrapped: boolean;
}

/** A rest with a countdown: shown from `from` to `to`; `start` is when singing resumes. */
export interface PageRest {
  from: number;
  to: number;
  start: number;
  line: number;
}

export interface PageLayout {
  width: number;
  height: number;
  pace: LyricsMotion;
  showNext: boolean;
  /** Requested size and the song's size after the height rule (lines may be smaller still). */
  requestedPx: number;
  fontPx: number;
  fontScale: number;
  /** Vertical centre of the current line's block. */
  centerY: number;
  lines: PageLine[];
  rests: PageRest[];
  /** Smallest line size / requested size; lines drawn smaller than fontPx; lines wrapped. */
  minLineScale: number;
  shrunkLines: number;
  wrappedLines: number;
  glyphCount: number;
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

interface Src {
  g: string;
  start: number;
  end: number;
  word: number;
}

/** Graphemes per line with sanitised times (monotone across the song, start <= end), as layout.ts does. */
function sanitized(lyrics: Lyrics): Src[][] {
  let prevEnd = -Infinity;
  return lyrics.lines.map((line) => {
    const out: Src[] = [];
    line.words.forEach((w, wi) => {
      for (const c of w.chars) {
        const start = Math.max(c[1], prevEnd);
        const end = Math.max(c[2], start);
        out.push({ g: c[0], start, end, word: wi });
        prevEnd = end;
      }
    });
    return out;
  });
}

type WidthOf = (g: string, px: number) => number;

/** First timed letter's start and last timed letter's end (the first grapheme's start if none is timed). */
function timesOf(s: Src[]): { start: number; end: number } {
  let start = NaN;
  let end = NaN;
  for (const g of s) {
    if (g.end - g.start <= EPS_T) continue;
    if (Number.isNaN(start)) start = g.start;
    end = g.end;
  }
  if (Number.isNaN(start)) start = end = s.length ? s[0].start : 0;
  return { start, end };
}

/** The song's lines `lines` as one display line (word indices renumbered so each stays a word). */
function joined(src: Src[][], lines: number[]): Src[] {
  if (lines.length === 1) return src[lines[0]];
  const out: Src[] = [];
  let base = 0;
  for (const li of lines) {
    let top = -1;
    for (const s of src[li]) {
      out.push({ ...s, word: base + s.word });
      top = Math.max(top, s.word);
    }
    base += top + 1;
  }
  return out;
}

interface Run {
  px: number;
  glyphs: PlacedGlyph[];
  /** Word spans in reading order: glyph range and [u0, u1). */
  spans: { first: number; last: number; u0: number; u1: number }[];
  uEnd: number;
}

/** The line as one row at `px`: graphemes back to back, words one measured space apart. */
function placeRun(src: Src[], line: number, px: number, widthOf: WidthOf): Run {
  const glyphs: PlacedGlyph[] = [];
  const spans: Run["spans"] = [];
  const space = widthOf(" ", px) || 0.28 * px;
  let u = 0;
  let wi = -1;
  for (const s of src) {
    if (s.word !== wi) {
      if (wi >= 0) u += space;
      wi = s.word;
      spans.push({ first: glyphs.length, last: glyphs.length, u0: u, u1: u });
    }
    const w = widthOf(s.g, px);
    glyphs.push({ g: s.g, x: u, w, px, start: s.start, end: s.end, line, word: s.word, L: u, R: u + w, timed: s.end - s.start > EPS_T });
    u += w;
    const sp = spans[spans.length - 1];
    sp.last = glyphs.length - 1;
    sp.u1 = u;
  }
  return { px, glyphs, spans, uEnd: u };
}

/** Row breaks (index of each row's first word) splitting `spans` into `k` rows of about equal width. */
function balancedBreaks(spans: Run["spans"], k: number): number[] {
  const m = spans.length;
  const widthOf = (a: number, b: number) => spans[b].u1 - spans[a].u0; // words a..b on one row
  if (k <= 1 || m <= 1) return [0];
  if (k === 2 || m === 2) {
    let best = [0, 1];
    let bestW = Infinity;
    for (let j = 1; j < m; j++) {
      const w = Math.max(widthOf(0, j - 1), widthOf(j, m - 1));
      if (w < bestW - 1e-9) [best, bestW] = [[0, j], w];
    }
    return best;
  }
  let best = [0, 1, 2];
  let bestW = Infinity;
  for (let j1 = 1; j1 < m - 1; j1++) {
    for (let j2 = j1 + 1; j2 < m; j2++) {
      const w = Math.max(widthOf(0, j1 - 1), widthOf(j1, j2 - 1), widthOf(j2, m - 1));
      if (w < bestW - 1e-9) [best, bestW] = [[0, j1, j2], w];
    }
  }
  return best;
}

/** Widest row of `run` cut at `breaks`. */
function widestRow(run: Run, breaks: number[]): number {
  let w = 0;
  for (let r = 0; r < breaks.length; r++) {
    const a = breaks[r];
    const b = r + 1 < breaks.length ? breaks[r + 1] - 1 : run.spans.length - 1;
    w = Math.max(w, run.spans[b].u1 - run.spans[a].u0);
  }
  return w;
}

interface Fitted {
  run: Run;
  breaks: number[];
  shrunk: boolean;
  wrapped: boolean;
}

/**
 * Point 3: the largest size up to `P` at which the line fits `colW` on one row, down to
 * PAGE_MIN_SCALE x P; otherwise wrapped onto 2..MAX_ROWS balanced rows, each as large as
 * fits; a line that fits nowhere above the floor takes the most rows and the size that fits.
 */
function fitLine(src: Src[], line: number, P: number, colW: number, widthOf: WidthOf): Fitted {
  const atSize = (px: number, breaks: number[]) => {
    // Text widths are close to linear in the size; re-measure and nudge until it fits.
    let run = placeRun(src, line, px, widthOf);
    for (let i = 0; i < 8 && widestRow(run, breaks) > colW && run.px > 1; i++) {
      run = placeRun(src, line, run.px * Math.min(0.999, colW / widestRow(run, breaks)), widthOf);
    }
    return run;
  };
  const one = placeRun(src, line, P, widthOf);
  if (one.uEnd <= colW) return { run: one, breaks: [0], shrunk: false, wrapped: false };
  if (colW / one.uEnd >= PAGE_MIN_SCALE) {
    const run = atSize(P * (colW / one.uEnd), [0]);
    if (run.px >= PAGE_MIN_SCALE * P - 1e-6) return { run, breaks: [0], shrunk: true, wrapped: false };
  }
  let last: Fitted | null = null;
  for (let k = 2; k <= Math.min(MAX_ROWS, one.spans.length); k++) {
    const breaks = balancedBreaks(one.spans, k);
    const w = widestRow(one, breaks);
    const run = w <= colW ? one : atSize(P * (colW / w), breaks);
    last = { run, breaks, shrunk: run.px < P - 1e-9, wrapped: true };
    if (run.px >= PAGE_MIN_SCALE * P - 1e-6) return last;
  }
  if (last) return last;
  // One word wider than the area: it alone decides the size.
  return { run: atSize(P * (colW / one.uEnd), [0]), breaks: [0], shrunk: true, wrapped: false };
}

/**
 * Point 5: the display lines, as lists of the song's line indices. Greedy, in order: the
 * next song line joins the current phrase when, shown as a display line of its own, it
 * would be previewed less than LOOKAHEAD_S before it is sung, the pause before it is
 * shorter than PHRASE_MAX_GAP_S and the phrase still fits two rows (at PAGE_MIN_SCALE x P at
 * the least: point 3 then draws it as large as it fits).
 */
function phrases(src: Src[][], times: { start: number; end: number }[], P: number, colW: number, widthOf: WidthOf): number[][] {
  const out: number[][] = [];
  if (src.length === 0) return out;
  const fits = (lines: number[]) => {
    const run = placeRun(joined(src, lines), 0, P, widthOf);
    if (run.uEnd <= colW) return true;
    return run.spans.length >= 2 && widestRow(run, balancedBreaks(run.spans, 2)) * PAGE_MIN_SCALE <= colW;
  };
  let g = [0];
  // End of the display line before `g`; null when `g` opens the song or follows a rest
  // with a countdown (the line after `g` is then shown ENTRY_S ahead or more anyway).
  let prevEnd: number | null = null;
  for (let m = 1; m < src.length; m++) {
    const gap = times[m].start - times[m - 1].end;
    const lead = prevEnd === null ? Infinity : times[m].start - (prevEnd + SWITCH_HOLD_S + TRANSITION_S);
    if (gap < PHRASE_MAX_GAP_S && lead < LOOKAHEAD_S && fits([...g, m])) {
      g.push(m);
      continue;
    }
    out.push(g);
    prevEnd = gap >= COUNTDOWN_MIN_GAP_S ? null : times[g[g.length - 1]].end;
    g = [m];
  }
  out.push(g);
  return out;
}

// ---------------------------------------------------------------------------
// Wipe motion (point 2)
// ---------------------------------------------------------------------------

/** The line's wipe: the ticker's knots for one line between a run-up and a run-out. */
function lineMotion(glyphs: PlacedGlyph[], words: PlacedWord[], uEnd: number, pace: LyricsMotion): { knots: Knot[]; motion: Motion } {
  if (words.length === 0) {
    // Nothing timed: the line counts as sung at its first grapheme's time.
    const t0 = glyphs.length ? glyphs[0].start : 0;
    const knots = [
      { t: t0 - EPS_T * 10, x: 0 },
      { t: t0, x: uEnd },
    ];
    return { knots, motion: new RampMotion(knots) };
  }
  const first = words[0];
  const last = words[words.length - 1];
  const second = words[1];
  // Run-up and run-out at the first and last word's own pace.
  const v0 = second ? (second.a - first.a) / Math.max(0.05, second.start - first.start) : (first.b - first.a) / Math.max(0.05, first.end - first.start);
  const v1 = (last.b - last.a) / Math.max(0.05, last.end - last.start);
  if (pace === "word") {
    const knots = straightenKnots(wordKnots(words, [], new Map(), [{ dt: RUN_S, dx: v0 * RUN_S }], { t: last.end + RUN_S, x: last.b + v1 * RUN_S }));
    return { knots, motion: new RampMotion(knots) };
  }
  // Letter pace: cells share the separators between timed neighbours (layout.ts point 2),
  // knots at every cell edge, the monotone spline through them.
  const timed = glyphs.filter((g) => g.timed);
  for (let n = 0; n + 1 < timed.length; n++) {
    const a = timed[n];
    const b = timed[n + 1];
    const sepStart = a.x + a.w;
    const sep = b.x - sepStart;
    const [add1, add2] = shareSeparator(sep, b.start - a.end, a.R - a.L, a.end - a.start, b.R - b.L, b.end - b.start);
    a.R = sepStart + add1;
    b.L = b.x - add2;
  }
  for (const g of glyphs) if (!g.timed) g.L = g.R = g.x;
  const knots: Knot[] = [];
  const push = (t: number, xx: number) => {
    const k = knots[knots.length - 1];
    if (k && t <= k.t + EPS_T) {
      if (xx > k.x) k.x = xx;
      return;
    }
    knots.push({ t, x: Math.max(xx, k ? k.x : xx) });
  };
  const g0 = timed[0];
  const g1 = timed[timed.length - 1];
  push(g0.start - RUN_S, g0.L - v0 * RUN_S);
  for (const g of timed) {
    push(g.start, g.L);
    push(g.end, g.R);
  }
  push(g1.end + RUN_S, g1.R + v1 * RUN_S);
  return { knots, motion: new MonotoneSpline(knots) };
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

const smooth = (u: number) => (u <= 0 ? 0 : u >= 1 ? 1 : u * u * (3 - 2 * u));

/** Left/right room kept clear in an area `width` px wide for text of size `px`. */
export function sidePad(width: number, px: number): number {
  return SIDE_SHARE * width + INK_PAD_EM * px;
}

/** Lays out the lyrics as pages (see the header). Line indices count only lines with text. */
export function layoutPage(lyricsIn: Lyrics, params: PageParams, measure: Measure): PageLayout {
  const lyrics = withoutEmptyLines(lyricsIn);
  const pace: LyricsMotion = params.pace ?? "word";
  const showNext = params.showNext ?? true;
  const W = Math.max(1, params.width);
  const H = Math.max(1, params.height);
  const requested = Math.max(1, params.fontPx);
  const src = sanitized(lyrics);
  const caches = new Map<number, Map<string, number>>();
  const widthOf: WidthOf = (g, px) => {
    let c = caches.get(px);
    if (!c) {
      c = new Map();
      caches.set(px, c);
    }
    let w = c.get(g);
    if (w === undefined) {
      w = Math.max(0, measure(g, px));
      c.set(g, w);
    }
    return w;
  };
  // Timing of the song's lines (independent of the size).
  const srcTimes = src.map(timesOf);
  const gapsOf = (tm: { start: number; end: number }[]) => tm.map((x, i) => (i === 0 ? x.start : x.start - tm[i - 1].end));

  // Sizes: each display line fits the width (point 3); then the song shrinks until the
  // tallest pair (a line and the preview under it) fits the height. Display lines are the
  // song's lines, short ones joined into phrases (point 5), which depends on the width.
  let P = requested;
  let groups: number[][] = [];
  let gsrc: Src[][] = [];
  let fitted: Fitted[] = [];
  let centerY = H / 2;
  let breakBefore: boolean[] = [];
  for (let iter = 0; iter < 16; iter++) {
    const pad = sidePad(W, P);
    const colW = Math.max(1, W - 2 * pad - (BAR_W_EM + BAR_GAP_EM) * P);
    groups = showNext ? phrases(src, srcTimes, P, colW, widthOf) : src.map((_, i) => [i]);
    gsrc = groups.map((g) => joined(src, g));
    breakBefore = gapsOf(gsrc.map(timesOf)).map((g) => g >= COUNTDOWN_MIN_GAP_S);
    fitted = gsrc.map((s, i) => fitLine(s, i, P, colW, widthOf));
    const h = fitted.map((f) => f.breaks.length * LINE_HEIGHT_EM * f.run.px);
    const ink = fitted.map((f) => INK_PAD_EM * f.run.px);
    let up = 0;
    let down = 0;
    for (let i = 0; i < fitted.length; i++) {
      up = Math.max(up, h[i] / 2 + ink[i]);
      const below = i + 1 < fitted.length && showNext && !breakBefore[i + 1] ? PREVIEW_GAP_EM * P + PREVIEW_SCALE * (h[i + 1] + ink[i + 1]) : ink[i];
      down = Math.max(down, h[i] / 2 + below);
    }
    // The current line sits a little above the middle so the pair is centred.
    const pref = showNext ? H / 2 - (PREVIEW_SCALE * LINE_HEIGHT_EM * P + PREVIEW_GAP_EM * P) / 2 : H / 2;
    if (up + down <= H || P <= 4) {
      centerY = Math.min(Math.max(pref, up), Math.max(up, H - down));
      break;
    }
    P = Math.max(4, P * (H / (up + down)) * 0.98);
  }
  const n = gsrc.length;
  const times = gsrc.map(timesOf);
  const gapBefore = gapsOf(times);
  const previewed = (i: number) => showNext && i > 0 && !breakBefore[i];

  // Placement, rows and the wipe of every line.
  const lines: PageLine[] = fitted.map((f, i) => {
    const run = f.run;
    const px = run.px;
    const rows: PageRow[] = f.breaks.map((a, r) => {
      const b = r + 1 < f.breaks.length ? f.breaks[r + 1] - 1 : run.spans.length - 1;
      const sa = run.spans[a];
      const sb = run.spans[b];
      return { first: sa.first, last: sb.last, u0: sa.u0, u1: sb.u1, left: 0, top: r * LINE_HEIGHT_EM * px };
    });
    // Centre the rows; move them right just as far as the lead-in bar needs.
    const pad = sidePad(W, P);
    const bar = (BAR_W_EM + BAR_GAP_EM) * px;
    let lo = -Infinity;
    let hi = Infinity;
    rows.forEach((row, r) => {
      const w = row.u1 - row.u0;
      lo = Math.max(lo, pad + (r === 0 ? bar : 0) + w / 2);
      hi = Math.min(hi, W - pad - w / 2);
    });
    const cx = lo <= hi ? Math.min(Math.max(W / 2, lo), hi) : (lo + hi) / 2;
    for (const row of rows) row.left = cx - (row.u1 - row.u0) / 2;
    const words = placeWords(run.glyphs);
    const { knots, motion } = lineMotion(run.glyphs, words, run.uEnd, pace);
    const text = gsrc[i].reduce((acc, s, k) => acc + (k > 0 && s.word !== gsrc[i][k - 1].word ? " " : "") + s.g, "");
    return {
      index: i,
      sourceLines: groups[i],
      text,
      px,
      glyphs: run.glyphs,
      words,
      rows,
      cx,
      width: Math.max(...rows.map((r) => r.u1 - r.u0)),
      height: rows.length * LINE_HEIGHT_EM * px,
      uEnd: run.uEnd,
      start: times[i].start,
      end: times[i].end,
      gapBefore: gapBefore[i],
      knots,
      motion,
      current: -Infinity,
      enter: "fade",
      leave: Infinity,
      previewFrom: null,
      currentTop: centerY - (rows.length * LINE_HEIGHT_EM * px) / 2,
      previewTop: 0,
      leaveRise: 0,
      leadIn: null,
      breakBefore: breakBefore[i],
      shrunk: f.shrunk,
      wrapped: f.wrapped,
    };
  });

  // Schedule (points 1 and 4).
  const rests: PageRest[] = [];
  for (let i = 0; i < n; i++) {
    const L = lines[i];
    if (i === 0) {
      L.current = breakBefore[0] ? L.start - ENTRY_S : -Infinity;
      L.enter = "fade";
    } else if (breakBefore[i]) {
      L.current = L.start - ENTRY_S;
      L.enter = "fade";
    } else {
      const prev = lines[i - 1];
      // Within SWITCH_HOLD_S after the previous line's last word, sooner when the gap is
      // short; never before the previous line has finished its own entry. A line without
      // a preview (settings.ui.show_next_line off) is not on screen until it is current,
      // so it does not wait for that past TRANSITION_S before its first word: after a run
      // of lines shorter than the transition the wait would add up and a line would be
      // sung before it is shown.
      const hold = Math.min(SWITCH_HOLD_S, Math.max(0, L.gapBefore - TRANSITION_S));
      const settled = prev.current + TRANSITION_S;
      L.current = Math.max(prev.end + hold, previewed(i) ? settled : Math.min(settled, L.start - TRANSITION_S));
      L.enter = previewed(i) ? "up" : "fade";
    }
    if (i > 0) {
      const prev = lines[i - 1];
      prev.leave = breakBefore[i] ? Math.max(prev.end + SWITCH_HOLD_S, prev.current + TRANSITION_S) : L.current;
      if (previewed(i)) {
        L.previewFrom = prev.current;
        L.previewTop = centerY + prev.height / 2 + PREVIEW_GAP_EM * P;
      }
      // The whole page moves up: the old line by as much as the new one, room allowing.
      const room = Math.max(0, prev.currentTop - INK_PAD_EM * prev.px);
      prev.leaveRise = Math.min(room, L.enter === "up" ? L.previewTop - L.currentTop : RISE_EM * prev.px);
    }
    if (breakBefore[i]) rests.push({ from: i === 0 ? -Infinity : lines[i - 1].leave + TRANSITION_S, to: L.current, start: L.start, line: i });
    if (L.gapBefore >= LEADIN_MIN_GAP_S) {
      // Never before the song starts: a song sung from 2 s opens with a full bar.
      const from = Math.max(L.current + TRANSITION_S, L.start - LEADIN_MAX_S, 0);
      if (L.start - from > 0.2) L.leadIn = { from, to: L.start };
    }
  }
  if (n > 0) {
    const last = lines[n - 1];
    last.leave = last.end + OUTRO_HOLD_S;
    last.leaveRise = Math.min(Math.max(0, last.currentTop - INK_PAD_EM * last.px), RISE_EM * last.px);
  }

  const minPx = n ? Math.min(...lines.map((l) => l.px)) : P;
  return {
    width: W,
    height: H,
    pace,
    showNext,
    requestedPx: requested,
    fontPx: P,
    fontScale: P / requested,
    centerY,
    lines,
    rests,
    minLineScale: minPx / requested,
    shrunkLines: lines.filter((l) => l.px < P - 1e-9).length,
    wrappedLines: lines.filter((l) => l.rows.length > 1).length,
    glyphCount: lines.reduce((a, l) => a + l.glyphs.length, 0),
  };
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/** One line as drawn: block top-left maps to (cx + (x - cx) * scale, top + y * scale). */
export interface PageItem {
  line: number;
  role: "current" | "entering" | "leaving" | "preview";
  top: number;
  scale: number;
  alpha: number;
  /** Wipe position in reading order (PlacedGlyph.x units); <= 0 nothing sung, >= uEnd all sung. */
  wipe: number;
}

export interface PageLeadIn {
  line: number;
  /** Share of the bar left, 1 -> 0 (0 exactly when the first word starts). */
  frac: number;
  /** The full bar: left, width, and the line's size (CSS px); the bar's right edge stays put. */
  x: number;
  w: number;
  px: number;
  /** Top of the line's first row band (the renderer centres the bar on the letters). */
  rowTop: number;
}

export interface PageCountdown {
  remaining: number;
  alpha: number;
  line: number;
}

/** The progress needle (point 6) on the line it rides. */
export interface PageNeedle {
  line: number;
  /** The item it rides (current or entering; leaving only before a rest or at the end of the song). */
  role: PageItem["role"];
  /** Row it is on (`needleRow`) and how much of that row is sung (`wipeInRow`, CSS px at the line's size). */
  row: number;
  sung: number;
  /** The gold/white boundary's x on screen, and the top and bottom of the needle's shaft (its head sits above `top`), CSS px. */
  x: number;
  top: number;
  bottom: number;
  /** The needle's size unit: the line's size times the item's scale, CSS px. */
  px: number;
  alpha: number;
}

export interface PageFrame {
  t: number;
  items: PageItem[];
  leadIn: PageLeadIn | null;
  countdown: PageCountdown | null;
  /** Line in (or entering) the current slot, else -1. */
  current: number;
  /** Where the wipe edge is (point 6); null in rests with a countdown and when nothing is on screen. */
  needle: PageNeedle | null;
}

/** How line i looks at time t (null: not drawn). */
export function lineItem(l: PageLayout, i: number, t: number): PageItem | null {
  const L = l.lines[i];
  if (!L) return null;
  const tr = TRANSITION_S;
  const wipe = L.motion.at(t);
  if (t >= L.leave + tr) return null;
  if (t >= L.leave) {
    const p = smooth((t - L.leave) / tr);
    return { line: i, role: "leaving", top: L.currentTop - L.leaveRise * p, scale: 1, alpha: 1 - smooth((t - L.leave) / (tr * LEAVE_FADE)), wipe };
  }
  if (t >= L.current + tr) return { line: i, role: "current", top: L.currentTop, scale: 1, alpha: 1, wipe };
  if (t >= L.current) {
    const p = smooth((t - L.current) / tr);
    if (L.enter === "up" && L.previewFrom !== null) {
      const top = L.previewTop + (L.currentTop - L.previewTop) * p;
      return { line: i, role: "entering", top, scale: PREVIEW_SCALE + (1 - PREVIEW_SCALE) * p, alpha: PREVIEW_ALPHA + (1 - PREVIEW_ALPHA) * p, wipe };
    }
    return { line: i, role: "entering", top: L.currentTop, scale: 1, alpha: p, wipe };
  }
  if (L.previewFrom === null || t < L.previewFrom) return null;
  const p = smooth((t - L.previewFrom) / tr);
  // Rises into place from a little lower, as far as the area allows.
  const room = Math.max(0, l.height - (L.previewTop + PREVIEW_SCALE * (L.height + INK_PAD_EM * L.px)));
  const lift = Math.min(PREVIEW_RISE_EM * l.fontPx, room) * (1 - p);
  return { line: i, role: "preview", top: L.previewTop + lift, scale: PREVIEW_SCALE, alpha: PREVIEW_ALPHA * p, wipe };
}

/** Index of the last line with `current <= t` (-1 if none). */
export function currentIndexAt(l: PageLayout, t: number): number {
  let lo = 0;
  let hi = l.lines.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (l.lines[mid].current <= t) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}

/** Everything drawn at lyric time t. */
export function pageFrame(l: PageLayout, t: number): PageFrame {
  const c = currentIndexAt(l, t);
  const items: PageItem[] = [];
  for (const i of [c - 1, c, c + 1]) {
    if (i < 0 || i >= l.lines.length) continue;
    const it = lineItem(l, i, t);
    if (it && it.alpha > 0) items.push(it);
  }
  let current = -1;
  for (const it of items) if (it.role === "current" || it.role === "entering") current = it.line;
  let leadIn: PageLeadIn | null = null;
  const C = current >= 0 ? l.lines[current] : null;
  if (C && C.leadIn && t >= C.leadIn.from && t < C.leadIn.to && t >= C.current + TRANSITION_S) {
    const w = BAR_W_EM * C.px;
    const right = C.rows[0].left - BAR_GAP_EM * C.px;
    leadIn = { line: current, frac: (C.leadIn.to - t) / (C.leadIn.to - C.leadIn.from), x: right - w, w, px: C.px, rowTop: C.currentTop + C.rows[0].top };
  }
  let countdown: PageCountdown | null = null;
  for (const r of l.rests) {
    if (t < r.from || t >= r.to + TRANSITION_S) continue;
    const a = Math.min(r.from === -Infinity ? 1 : smooth((t - r.from) / TRANSITION_S), 1 - smooth((t - r.to) / TRANSITION_S));
    if (a > 0) countdown = { remaining: r.start - t, alpha: a, line: r.line };
    break;
  }
  return { t, items, leadIn, countdown, current, needle: needleOf(l, items, current) };
}

/** How much of row r of `line` is sung at wipe position `wipe` (0 .. the row's width). */
export function wipeInRow(line: PageLine, r: number, wipe: number): number {
  const row = line.rows[r];
  return Math.min(row.u1 - row.u0, Math.max(0, wipe - row.u0));
}

/**
 * The row the wipe edge is on: the last row the wipe has entered (its start passed), else
 * the first. In the space between two rows the edge stays at the end of the earlier row;
 * it moves to the next row's start as soon as the wipe enters it.
 */
export function needleRow(line: PageLine, wipe: number): number {
  let r = 0;
  for (let k = 1; k < line.rows.length; k++) if (wipe > line.rows[k].u0) r = k;
  return r;
}

/**
 * Point 6: the needle on the line in the current slot (current or entering) at its wipe
 * edge, placed as the renderer places that row (`top + row.top * scale`, x scaled about
 * the line's centre). Without a line in the slot it stays on a line leaving before a rest
 * (or at the song's end) and fades with it; it comes back with the line after the rest,
 * fading in with it. Elsewhere it is fully opaque, also while it moves up with a line.
 */
function needleOf(l: PageLayout, items: PageItem[], current: number): PageNeedle | null {
  const it = current >= 0 ? items.find((x) => x.line === current && (x.role === "current" || x.role === "entering")) : items.find((x) => x.role === "leaving");
  if (!it) return null;
  const L = l.lines[it.line];
  if (!L || L.rows.length === 0) return null;
  const r = needleRow(L, it.wipe);
  const row = L.rows[r];
  const sung = wipeInRow(L, r, it.wipe);
  const s = it.scale;
  const px = L.px * s;
  const band = it.top + row.top * s;
  const fades = it.role === "leaving" || (it.role === "entering" && L.breakBefore);
  return {
    line: it.line,
    role: it.role,
    row: r,
    sung,
    x: L.cx + (row.left + sung - L.cx) * s,
    top: band - NEEDLE_OVER_EM * px,
    bottom: band + (LINE_HEIGHT_EM + NEEDLE_OVER_EM) * px,
    px,
    alpha: fades ? it.alpha : 1,
  };
}

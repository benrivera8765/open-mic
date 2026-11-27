// SPDX-License-Identifier: GPL-3.0-or-later
// The practice lane (Work Order 2 section 4.5): layout and frames, as pure functions.
//
// What it shows, from the work order and the owner's two reference pictures:
//
// 1. Bars. The guide's notes arrive from the right as horizontal bars (light tubes) and
//    pass a fixed "now" line NOW_FRACTION of the way into the bar area. FUTURE_S seconds of
//    upcoming notes are visible right of it (WO2: at least two). Time runs right to left at
//    `pxPerSec`.
// 2. Height. Every bar sits at lane(target) of docs/APP_API.md 12.1: the guide's measured
//    pitch plus the key heard plus 12 x the singer's octave shift, moved by the recording's
//    tuning, so it lines up with the piano key of the note to sing. One semitone is one row
//    of `rowH` pixels, the row of key p centred on lane pitch p.
// 3. Range. The rows shown cover the melody's range in the current key plus MARGIN_ST
//    above and below, never fewer than MIN_ROWS (more than an octave). If that does not fit
//    at MIN_ROW_PX ("fit" mode impossible), the rows stay at MIN_ROW_PX and the view follows
//    the melody up and down smoothly ("scroll" mode: a smoothed centre of the notes around
//    the now line, a pure function of time, so it never jumps).
// 4. Key change. When the key HEARD changes (KeyTimeline in pitch.ts, never the dial), the
//    bars from the change on slide to their new height within SLIDE_MS and the view
//    re-fits within REFIT_MS once the change is heard (only as far as needed: in fit mode a
//    small change moves the bars and not the keyboard). Bars sung before a change keep the
//    key they were sung in. The octave shift moves the whole lane the same way. Under
//    reduce-motion both happen at once.
// 5. Marker. The singer's newest pitch (practice://live) on the now line, the tail of the
//    last TAIL_S behind it, placed by each value's song time. Hidden when there is no pitch
//    (silence, breath, watch only) or the values stopped coming. Above or below the view:
//    pinned at that edge with an arrow. While a graded note is due and the singer is more
//    than half a semitone off (octaves forgiven unless strict), a "higher" / "lower" cue.
// 6. Words. Each note's word (the first note of a word carries it) in a strip under the
//    bars, starting exactly where its note starts, as in the lane picture; a second row
//    takes a word that would touch the one before it.
// 7. Rests. Before a gap of COUNTDOWN_GAP_S or more, while no note is in view yet: the
//    countdown "Singing in 0:08", as the lyrics show it.
//
// The renderer (lanerenderer.ts) draws exactly what `laneFrame` returns; laneverify.ts
// checks a whole song against the rule independently.
import type { NoteSpelling, OctaveMode, PracticeGuide } from "../api/types";
import type { LiveStore } from "./live";
import { foldCents, isBlackKey, type KeyView, keyAt, laneOf, noteName } from "./pitch";
import type { Lyrics } from "../api/types";
import { backingWords } from "../lib/lyrics";

export type Measure = (text: string, fontPx: number) => number;

export const NOW_FRACTION = 0.3;
export const FUTURE_S = 3.0;
export const MARGIN_ST = 1.5;
/** An octave and one: the lane never shows less than an octave (WO2 4.5). */
export const MIN_ROWS = 13;
/** Thinnest row (readable bar) and thickest, CSS px at 1440x900 (scaled with the window). */
export const MIN_ROW_PX = 12;
export const MAX_ROW_PX = 30;
/** Bar thickness as a share of the row. */
export const BAR_FRACTION = 0.72;
export const TAIL_S = 1.0;
export const SLIDE_MS = 180;
export const REFIT_MS = 450;
export const COUNTDOWN_GAP_S = 4;
/** The marker hides when the newest value is this much older than the now line (song s) or than now (ms). */
export const STALE_S = 0.25;
export const STALE_MS = 350;
/** Off by more than this (semitones) while a graded note is due: the higher / lower cue. */
export const CUE_ST = 0.5;
/** Scroll mode: the view follows the notes within +-SMOOTH_S of a point SMOOTH_AHEAD_S ahead. */
const SMOOTH_S = 1.6;
const SMOOTH_AHEAD_S = 0.8;
const CENTRE_STEP_S = 0.05;
/** Scroll mode: the view moves at most this many semitones a second. */
const MAX_SCROLL_ST = 7;
/** Rows of words under the bars, at most. */
const WORD_ROWS = 3;

export interface LaneParams {
  /** The lane canvas, CSS px (piano + bars + word strip). */
  width: number;
  height: number;
  pianoSide: "left" | "right";
  /** settings.practice.words_under_notes */
  words: boolean;
  /** Window scale: min(window width / 1440, window height / 900). */
  scale: number;
  /**
   * Device pixels per CSS px (default 1). The row height is a whole, even number of device
   * pixels, so the renderer can place every cached bitmap on whole device pixels (a plain
   * copy, about 3x cheaper to draw than a filtered one) with the bars still exactly on
   * their piano rows (lanerenderer.ts, "Device pixels").
   */
  dpr?: number;
}

export interface LaneNote {
  /** Index into PracticeGuide.notes (the `note` of hits and done). */
  i: number;
  start: number;
  end: number;
  /** lane() at key 0, octave 0: midi - tuning / 100. */
  lane: number;
  kind: "pitched" | "unpitched";
  dim: boolean;
  graded: boolean;
  glideIn: number;
  /** "line:word" of the lyric word the note belongs to, or null. */
  word: string | null;
}

/** A lyric word in the strip under the bars: every word of the lyrics, at its own time. */
export interface LaneWord {
  /** Song time the word starts (lyrics time + the song's lyrics offset), s. */
  start: number;
  text: string;
  /** Row in the strip (0 = top). */
  row: number;
  /** Width at the word font, px. */
  w: number;
  /** Drawn this far right of its start (px) because the rows were full there (0 almost always). */
  shift: number;
  /** Backing vocals (in parentheses, D-282): drawn dimmed, never in the sung colour. */
  backing: boolean;
}

export interface LaneLayout {
  params: LaneParams;
  duration: number;
  tuning: number;
  notes: LaneNote[];
  words: LaneWord[];
  spans: { start: number; end: number; kind: string }[];
  curve: PracticeGuide["curve"];
  /** Pitched notes' lane range at key 0. */
  lo: number;
  hi: number;
  pianoX: number;
  pianoW: number;
  barX: number;
  barW: number;
  barTop: number;
  barBottom: number;
  wordTop: number;
  wordBottom: number;
  wordRows: number;
  wordPx: number;
  labelPx: number;
  nowX: number;
  pxPerSec: number;
  /** Seconds visible left of the now line. */
  pastS: number;
  rowH: number;
  /** Rows the bar area shows (fractional). */
  rows: number;
  barH: number;
  mode: "fit" | "scroll";
  /** fit: lowest lane pitch shown at shift 0. */
  fitLow0: number;
  /** scroll: smoothed melody centre (key 0) every CENTRE_STEP_S from 0. */
  centres: Float32Array;
  /** Marker head size (at least as tall as a bar, WO2 4.5). */
  headH: number;
  headW: number;
  /** Tail thickness: never a hairline. */
  tailW: number;
}

/** Note index of the first note ending at or after t (binary search over end times; notes do not overlap). */
export function firstNoteFrom(L: LaneLayout, t: number): number {
  let lo = 0;
  let hi = L.notes.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (L.notes[mid].end < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Index of the first word starting at or after `t` (L.words is in time order). */
export function firstWordFrom(L: LaneLayout, t: number): number {
  let lo = 0;
  let hi = L.words.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (L.words[mid].start < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

const clamp = (x: number, a: number, b: number) => Math.max(a, Math.min(b, x));
const ease = (p: number) => (p <= 0 ? 0 : p >= 1 ? 1 : 1 - (1 - p) * (1 - p) * (1 - p));

/** Lays the lane out for a guide at a size. `measure` measures the word font (tests: the lyrics face's width table). */
export function layoutLane(guide: PracticeGuide, lyrics: Lyrics | null, duration: number, params: LaneParams, measure: Measure, lyricsOffsetS = 0): LaneLayout {
  const s = Math.max(0.35, params.scale);
  const tuning = guide.tuning_cents;
  const pianoW = Math.round(clamp(params.width * 0.065, 54 * s, 132 * s));
  const pianoX = params.pianoSide === "left" ? 0 : params.width - pianoW;
  const barX = params.pianoSide === "left" ? pianoW : 0;
  const barW = Math.max(40, params.width - pianoW);
  const wordPx = Math.round(clamp(16 * s, 11, 60));
  const notes: LaneNote[] = guide.notes.map((n, i) => ({
    i,
    start: n.start,
    end: Math.max(n.end, n.start + 0.01),
    lane: n.kind === "pitched" || n.midi > 0 ? laneOf(n.midi, tuning) : Number.NaN,
    kind: n.kind,
    dim: n.dim,
    graded: n.graded,
    glideIn: n.glide_in,
    word: n.word ? `${n.word[0]}:${n.word[1]}` : null,
  }));
  const pitched = notes.filter((n) => n.kind === "pitched" && Number.isFinite(n.lane));
  let lo = pitched.length ? Math.min(...pitched.map((n) => n.lane)) : 57;
  let hi = pitched.length ? Math.max(...pitched.map((n) => n.lane)) : 69;
  // Unpitched notes without a pitch ride at the middle of the melody.
  for (const n of notes) if (!Number.isFinite(n.lane)) n.lane = Math.round((lo + hi) / 2);
  lo = Math.min(lo, ...notes.map((n) => n.lane));
  hi = Math.max(hi, ...notes.map((n) => n.lane));

  // Time scale: FUTURE_S seconds right of the now line.
  const nowOff = Math.round(barW * NOW_FRACTION);
  const nowX = barX + nowOff;
  const pxPerSec = (barW - nowOff) / FUTURE_S;

  // Words: every lyric word (owner 2026-11-22: the strip must match the lyrics below it),
  // in up to WORD_ROWS rows; a word that fits no row (very fast singing) moves right just
  // enough to clear the row that frees first. A word starts under the first bar sung on
  // it (owner 2026-11-23: "you can't tell which lyrics go to which bar"), else at its
  // lyric time.
  let wordRows = 0;
  const words: LaneWord[] = [];
  if (params.words && lyrics) {
    const gap = wordPx * 0.45;
    const rowEnd = new Array<number>(WORD_ROWS).fill(-Infinity);
    const firstBar = new Map<string, number>();
    for (const n of notes) {
      if (n.word === null || n.kind !== "pitched") continue;
      const t = firstBar.get(n.word);
      if (t === undefined || n.start < t) firstBar.set(n.word, n.start);
    }
    const backing = backingWords(lyrics);
    for (const [li, line] of lyrics.lines.entries()) {
      for (const [wi, wd] of line.words.entries()) {
        const text = wd.chars.map((c) => c[0]).join("").trim();
        if (!text) continue;
        const start = firstBar.get(`${li}:${wi}`) ?? wd.start + lyricsOffsetS;
        const x = start * pxPerSec;
        const w = measure(text, wordPx);
        let row = rowEnd.findIndex((e) => x >= e + gap);
        let shift = 0;
        if (row < 0) {
          row = rowEnd.indexOf(Math.min(...rowEnd));
          shift = rowEnd[row] + gap - x;
        }
        rowEnd[row] = x + shift + w;
        words.push({ start, text, row, w, shift, backing: backing[li]?.[wi] === true });
        wordRows = Math.max(wordRows, row + 1);
      }
    }
    words.sort((a, b) => a.start - b.start);
  }
  const wordLine = Math.round(wordPx * 1.35);
  const stripH = wordRows > 0 ? wordRows * wordLine + Math.round(wordPx * 0.5) : 0;
  const barTop = Math.round(Math.max(6, 8 * s));
  const barBottom = params.height - stripH - Math.round(Math.max(4, 6 * s));
  const wordTop = barBottom + Math.round(Math.max(3, 4 * s));
  const regionH = Math.max(40, barBottom - barTop);

  // Rows: fit the melody (+ margins, at least MIN_ROWS) or scroll at the thinnest readable row.
  const needed = Math.max(MIN_ROWS, hi - lo + 2 * MARGIN_ST);
  const minRow = MIN_ROW_PX * s;
  const maxRow = MAX_ROW_PX * s;
  let rowH = regionH / needed;
  let mode: "fit" | "scroll" = "fit";
  if (rowH < minRow) {
    rowH = minRow;
    mode = "scroll";
  } else if (rowH > maxRow) rowH = maxRow;
  // Whole, even device pixels: rounded down when fitting (the melody still fits), up when
  // scrolling (never thinner than the readable minimum).
  const dpr = params.dpr && params.dpr > 0 ? params.dpr : 1;
  const even = (mode === "fit" ? Math.floor : Math.ceil)((rowH * dpr) / 2) * 2;
  rowH = Math.max(2, even) / dpr;
  const rows = regionH / rowH;
  const fitLow0 = (lo + hi) / 2 - rows / 2;

  // Scroll mode: a Gaussian-weighted centre of the notes around each moment, precomputed.
  const steps = Math.max(1, Math.ceil(duration / CENTRE_STEP_S) + 1);
  const centres = new Float32Array(mode === "scroll" ? steps : 0);
  if (mode === "scroll") {
    let j0 = 0;
    for (let k = 0; k < steps; k++) {
      const c = k * CENTRE_STEP_S + SMOOTH_AHEAD_S;
      while (j0 < notes.length && notes[j0].end < c - 2 * SMOOTH_S) j0++;
      let wsum = 0;
      let sum = 0;
      for (let j = j0; j < notes.length && notes[j].start < c + 2 * SMOOTH_S; j++) {
        const n = notes[j];
        const mid = (n.start + n.end) / 2;
        const w = Math.exp(-((mid - c) ** 2) / (2 * SMOOTH_S * SMOOTH_S)) * (n.end - n.start);
        wsum += w;
        sum += w * n.lane;
      }
      centres[k] = wsum > 1e-9 ? sum / wsum : Number.NaN;
    }
    // Gaps: hold the last known centre (and the first one before the first note).
    let last = Number.NaN;
    for (let k = 0; k < steps; k++) {
      if (Number.isNaN(centres[k])) centres[k] = last;
      else last = centres[k];
    }
    let first = Number.NaN;
    for (let k = steps - 1; k >= 0; k--) {
      if (Number.isNaN(centres[k])) centres[k] = Number.isNaN(first) ? (lo + hi) / 2 : first;
      else first = centres[k];
    }
    // Slew limit, forward then backward: the view never scrolls faster than MAX_SCROLL_ST
    // semitones a second, and the backward pass makes it start early (it anticipates a
    // jump of the melody instead of chasing it).
    const maxStep = MAX_SCROLL_ST * CENTRE_STEP_S;
    for (let k = 1; k < steps; k++) centres[k] = clamp(centres[k], centres[k - 1] - maxStep, centres[k - 1] + maxStep);
    for (let k = steps - 2; k >= 0; k--) centres[k] = clamp(centres[k], centres[k + 1] - maxStep, centres[k + 1] + maxStep);
  }

  const barH = rowH * BAR_FRACTION;
  // The marker: bold, at least as tall as a bar (WO2 4.5), never a hairline.
  const headH = Math.max(barH * 1.8, 22 * s);
  return {
    params,
    duration,
    tuning,
    notes,
    words,
    spans: guide.spans,
    curve: guide.curve,
    lo,
    hi,
    pianoX,
    pianoW,
    barX,
    barW,
    barTop,
    barBottom,
    wordTop,
    wordBottom: wordTop + wordRows * wordLine,
    wordRows,
    wordPx,
    labelPx: Math.max(8, Math.min(rowH * 0.66, 13.5 * s)),
    nowX,
    pxPerSec,
    pastS: nowOff / pxPerSec,
    rowH,
    rows,
    barH,
    mode,
    fitLow0,
    centres,
    headH,
    headW: headH * 1.5,
    tailW: Math.max(7 * s, barH * 0.72),
  };
}

/** Screen x of song time `tau` when the now line is at song time `t`. */
export const xOf = (L: LaneLayout, t: number, tau: number): number => L.nowX + (tau - t) * L.pxPerSec;

/** Screen y of lane pitch `m` with `viewLow` at the bottom edge of the bar area (row p centred on p). */
export const yOf = (L: LaneLayout, viewLow: number, m: number): number => L.barBottom - (m - viewLow + 0.5) * L.rowH;

/** Smoothed melody centre at t (scroll mode), key 0. */
function centreAt(L: LaneLayout, t: number): number {
  const k = clamp(t / CENTRE_STEP_S, 0, L.centres.length - 1);
  const k0 = Math.floor(k);
  const k1 = Math.min(L.centres.length - 1, k0 + 1);
  return L.centres[k0] + (L.centres[k1] - L.centres[k0]) * (k - k0);
}

/**
 * The lowest lane pitch shown (at the bottom edge of the bar area, row centre - 0.5) at
 * song time t with the whole lane shifted by `shift` semitones (key + 12 x octave).
 */
export function viewLowAt(L: LaneLayout, t: number, shift: number): number {
  // Edge rows: the bottom row's centre is viewLow, so rows run from viewLow - 0.5.
  const span = L.rows - 1;
  if (L.mode === "fit") {
    // As close to the shift-0 window as possible while the melody (+ margins) is inside.
    const lo = L.lo + shift - MARGIN_ST + 0.5;
    const hi = L.hi + shift + MARGIN_ST - 0.5 - span;
    return clamp(L.fitLow0 + 0.5, Math.min(lo, hi), Math.max(lo, hi)) ;
  }
  const c = centreAt(L, t) + shift;
  return clamp(c - span / 2, L.lo + shift - MARGIN_ST + 0.5, L.hi + shift + MARGIN_ST - 0.5 - span);
}

export interface LaneInputs {
  /** The lane's "now": position - device offset (APP_API 12.7), song seconds. */
  t: number;
  /** performance.now() (the key slide and the marker's staleness). */
  nowMs: number;
  keys: KeyView;
  spelling: NoteSpelling;
  octaveMode: OctaveMode;
  live: LiveStore | null;
  /** performance.now() of the last live batch (the marker hides when they stop). */
  liveMs: number;
  /** No marker, no grade (WO2 4.4 "watch only"). */
  watchOnly: boolean;
  /**
   * Watch only that follows the singer (owner, 2026-11-22, for filming): a perfect voice
   * rides the melody, with the marker, the tail and the lit bars of a flawless take.
   */
  follow?: boolean;
  reduceMotion: boolean;
  /** settings.practice.simple_lane: no tail, no glow. */
  simple: boolean;
}

/** One stretch of bars at one display shift (more than one only around a key change). */
export interface LaneRegion {
  t0: number;
  t1: number;
  x0: number;
  x1: number;
  /** Display shift in semitones (key + 12 x octave), animated during a slide. */
  shift: number;
}

export interface LaneFrame {
  t: number;
  viewLow: number;
  /** Integer rows at least partly visible. */
  rowLow: number;
  rowHigh: number;
  regions: LaneRegion[];
  /** The note to sing now (graded or not) and its key. */
  target: { note: number; lane: number; row: number; name: string; graded: boolean } | null;
  /** The singer's marker, or null when hidden. edge: 1 above the view, -1 below. cue: 1 "higher", -1 "lower". */
  singer: { midi: number; lane: number; y: number; row: number; name: string; edge: -1 | 0 | 1; cue: -1 | 0 | 1 } | null;
  /** Tail points [x, y, ...] with NaN pairs at gaps; empty under reduce-motion and simple. */
  tail: number[];
  /** Lit parts of bars: [note, x0, x1, y] per stretch. */
  lit: number[];
  /** Seconds until singing starts again, or null. */
  countdown: number | null;
  /** Key and octave in effect at the now line (steady values, no animation). */
  key: number;
  octave: number;
  spelling: NoteSpelling;
  /** Something is still animating (a slide): the loop must keep drawing. */
  animating: boolean;
}

/** Display shift of the regions at time `t` from `nowMs` (pure: same inputs, same answer). */
function regionsOf(L: LaneLayout, inp: LaneInputs, t0: number, t1: number): { regions: LaneRegion[]; animating: boolean } {
  const v = inp.keys;
  const instant = inp.reduceMotion;
  let animating = false;
  const po = instant ? 1 : ease((inp.nowMs - v.octaveSeenMs) / SLIDE_MS);
  if (po < 1) animating = true;
  const oct = v.octaveFrom + (v.octave - v.octaveFrom) * po;
  const regions: LaneRegion[] = [];
  let from = t0;
  let shift = v.base;
  for (const c of v.changes) {
    if (c.at <= t0) {
      const p = instant ? 1 : ease((inp.nowMs - c.seenMs) / SLIDE_MS);
      if (p < 1) animating = true;
      shift = c.from + (c.semitones - c.from) * p;
      continue;
    }
    if (c.at >= t1) {
      // A change beyond the view still slides (nothing to draw for it yet).
      break;
    }
    regions.push({ t0: from, t1: c.at, x0: xOf(L, inp.t, from), x1: xOf(L, inp.t, c.at), shift: shift + 12 * oct });
    const p = instant ? 1 : ease((inp.nowMs - c.seenMs) / SLIDE_MS);
    if (p < 1) animating = true;
    shift = c.from + (c.semitones - c.from) * p;
    from = c.at;
  }
  regions.push({ t0: from, t1, x0: xOf(L, inp.t, from), x1: xOf(L, inp.t, t1), shift: shift + 12 * oct });
  return { regions, animating };
}

/** The view's shift at t (re-fits after a heard change; instant under reduce-motion). */
function viewShiftOf(inp: LaneInputs): { shift: number; animating: boolean } {
  const v = inp.keys;
  let animating = false;
  let key = v.base;
  for (const c of v.changes) {
    if (c.at > inp.t) break;
    const eff = Number.isNaN(c.effectiveMs) ? inp.nowMs : c.effectiveMs;
    const p = inp.reduceMotion ? 1 : ease((inp.nowMs - eff - SLIDE_MS * 0.5) / REFIT_MS);
    if (p < 1) animating = true;
    key = c.from + (c.semitones - c.from) * p;
  }
  const po = inp.reduceMotion ? 1 : ease((inp.nowMs - v.octaveSeenMs - SLIDE_MS * 0.5) / REFIT_MS);
  if (po < 1) animating = true;
  const oct = v.octaveFrom + (v.octave - v.octaveFrom) * po;
  return { shift: key + 12 * oct, animating };
}

const scratch = { t: 0, midi: 0, level: 0 };

/** Gaps shorter than this are sung through (a smooth slide to the next note); longer ones are rests. */
const FOLLOW_SLIDE_GAP_S = 0.3;
/** Leaps wider than this (semitones) are not slid through: the voice lifts off and lands on the next note. */
const FOLLOW_SLIDE_MAX_ST = 4.5;

/**
 * The perfect voice of watch-only "follow": its pitch (lane units, key 0) at song time
 * `ts`, or NaN in a rest or on an unpitched note. Exactly on every pitched note; across a
 * short gap it slides with an S-curve from one note to the next, as a singer does.
 */
export function followLaneAt(L: LaneLayout, ts: number): number {
  const notes = L.notes;
  const j = firstNoteFrom(L, ts);
  if (j >= notes.length) return Number.NaN;
  const n = notes[j];
  if (n.start <= ts) return n.kind === "pitched" ? n.lane : Number.NaN;
  // In the gap before note j.
  const p = j > 0 ? notes[j - 1] : null;
  if (!p || p.kind !== "pitched" || n.kind !== "pitched") return Number.NaN;
  const gap = n.start - p.end;
  if (gap <= 0 || gap >= FOLLOW_SLIDE_GAP_S || Math.abs(n.lane - p.lane) > FOLLOW_SLIDE_MAX_ST) return Number.NaN;
  const f = (ts - p.end) / gap;
  return p.lane + (n.lane - p.lane) * f * f * (3 - 2 * f);
}

/** Everything one frame shows at lane time `inp.t`. */
export function laneFrame(L: LaneLayout, inp: LaneInputs): LaneFrame {
  const t = inp.t;
  const t0 = t - L.pastS - 0.5;
  const t1 = t + FUTURE_S + 0.5;
  const { regions, animating: aBars } = regionsOf(L, inp, t0, t1);
  const view = viewShiftOf(inp);
  const viewLow = viewLowAt(L, t, view.shift);
  const key = keyAt(inp.keys, t);
  const octave = inp.keys.octave;
  const steadyShift = key + 12 * octave;

  // The note to sing now.
  let target: LaneFrame["target"] = null;
  const j = firstNoteFrom(L, t);
  if (j < L.notes.length && L.notes[j].start <= t) {
    const n = L.notes[j];
    if (n.kind === "pitched") {
      const lane = n.lane + steadyShift;
      const row = Math.round(lane);
      target = { note: n.i, lane, row, name: noteName(row, inp.spelling), graded: n.graded && !n.dim };
    }
  }

  // The singer.
  let singer: LaneFrame["singer"] = null;
  const tail: number[] = [];
  const live = inp.live;
  const viewHigh = viewLow + L.rows - 1;
  const yClamp = (y: number) => clamp(y, L.barTop + L.headH * 0.5, L.barBottom - L.headH * 0.5);
  if (!inp.watchOnly && live && live.size > 0 && live.at(0, scratch) && inp.nowMs - inp.liveMs < STALE_MS && scratch.t >= t - STALE_S && Number.isFinite(scratch.midi)) {
    const lane = laneOf(scratch.midi, L.tuning);
    const edge: -1 | 0 | 1 = lane > viewHigh + 0.5 ? 1 : lane < viewLow - 0.5 ? -1 : 0;
    const y = edge === 0 ? yOf(L, viewLow, lane) : edge > 0 ? L.barTop + L.headH * 0.5 : L.barBottom - L.headH * 0.5;
    let cue: -1 | 0 | 1 = 0;
    if (target && target.graded) {
      let err = (lane - target.lane) * 100;
      if (inp.octaveMode !== "strict") err = foldCents(err);
      if (err > CUE_ST * 100) cue = -1;
      else if (err < -CUE_ST * 100) cue = 1;
    }
    const row = Math.round(lane);
    singer = { midi: scratch.midi, lane, y, row, name: noteName(row, inp.spelling), edge, cue };
  }
  const follow = inp.watchOnly && !!inp.follow;
  if (follow) {
    const base = followLaneAt(L, t);
    if (Number.isFinite(base)) {
      const lane = base + steadyShift;
      const edge: -1 | 0 | 1 = lane > viewHigh + 0.5 ? 1 : lane < viewLow - 0.5 ? -1 : 0;
      const y = edge === 0 ? yOf(L, viewLow, lane) : edge > 0 ? L.barTop + L.headH * 0.5 : L.barBottom - L.headH * 0.5;
      const row = Math.round(lane);
      singer = { midi: lane + L.tuning / 100, lane, y, row, name: noteName(row, inp.spelling), edge, cue: 0 };
    }
    if (singer && !inp.reduceMotion && !inp.simple) {
      // Tail: the perfect voice over the last TAIL_S, sampled every 1/60 s, broken at rests.
      const step = 1 / 60;
      let pen = false;
      for (let k = 0; k * step <= TAIL_S; k++) {
        const ts = t - k * step;
        const ln = followLaneAt(L, ts);
        if (!Number.isFinite(ln)) {
          if (pen) tail.push(Number.NaN, Number.NaN);
          pen = false;
          continue;
        }
        tail.push(xOf(L, t, ts), yClamp(yOf(L, viewLow, ln + steadyShift)));
        pen = true;
      }
    }
  }
  if (singer && !inp.reduceMotion && !inp.simple && live) {
    // Tail: the last TAIL_S of values, broken at gaps; the newest point is the head itself.
    let prevT = Number.NaN;
    for (let k = 0; live.at(k, scratch); k++) {
      if (scratch.t < t - TAIL_S) break;
      if (scratch.t > t + 0.05) continue;
      const ok = Number.isFinite(scratch.midi) && (Number.isNaN(prevT) || prevT - scratch.t < 0.06);
      if (!ok) {
        if (tail.length && !Number.isNaN(tail[tail.length - 1])) tail.push(Number.NaN, Number.NaN);
        prevT = Number.isFinite(scratch.midi) ? scratch.t : Number.NaN;
        if (!Number.isFinite(scratch.midi)) continue;
      }
      const x = k === 0 ? L.nowX : Math.min(L.nowX, xOf(L, t, scratch.t));
      tail.push(x, yClamp(yOf(L, viewLow, laneOf(scratch.midi, L.tuning))));
      prevT = scratch.t;
    }
  }

  // Lit parts: decided on-pitch stretches of graded notes in view.
  const lit: number[] = [];
  if (follow) {
    // The perfect voice hits every graded note from its start to the now line.
    for (let k = firstNoteFrom(L, t0); k < L.notes.length && L.notes[k].start < t; k++) {
      const n = L.notes[k];
      if (n.kind !== "pitched" || !n.graded || n.dim) continue;
      const to = Math.min(n.end, t);
      for (const rg of regions) {
        const a = Math.max(n.start, rg.t0);
        const b = Math.min(to, rg.t1);
        if (b <= a) continue;
        lit.push(n.i, xOf(L, t, a), xOf(L, t, b), yOf(L, viewLow, n.lane + rg.shift));
      }
    }
  } else if (live) {
    for (let k = firstNoteFrom(L, t0); k < L.notes.length && L.notes[k].start < t + 0.1; k++) {
      const n = L.notes[k];
      const h = live.hitsOf(n.i);
      if (!h) continue;
      for (let q = 0; q + 2 < h.length; q += 3) {
        if (h[q + 2] !== 0) continue;
        const from = Math.max(h[q], n.start);
        let to = Math.min(h[q + 1], n.end, t);
        // The stretch being sung now reaches the now line (the scorer's batches lag ~50 ms).
        if (singer && singer.cue === 0 && target && target.note === n.i && h[q + 1] >= t - 0.25 && q + 3 >= h.length) to = Math.min(n.end, t);
        if (to <= from) continue;
        // A stretch across a heard key change is cut there, each part on its own region's
        // bar: the bar itself is cut at the change (target(note, t) uses key_at(t)), so a
        // single piece at the shift of its start would float off the bar after the change.
        for (const rg of regions) {
          const a = Math.max(from, rg.t0);
          const b = Math.min(to, rg.t1);
          if (b <= a) continue;
          lit.push(n.i, xOf(L, t, a), xOf(L, t, b), yOf(L, viewLow, n.lane + rg.shift));
        }
      }
    }
  }

  // Countdown before a long rest's end, until the next notes come into view.
  let countdown: number | null = null;
  if (j < L.notes.length && L.notes[j].start > t) {
    const next = L.notes[j].start;
    const prevEnd = j > 0 ? L.notes[j - 1].end : 0;
    if (next - prevEnd >= COUNTDOWN_GAP_S && next - t > FUTURE_S * 0.92) countdown = next - t;
  }

  return {
    t,
    viewLow,
    rowLow: Math.floor(viewLow - 0.5),
    rowHigh: Math.ceil(viewHigh + 0.5),
    regions,
    target,
    singer,
    tail,
    lit,
    countdown,
    key,
    octave,
    spelling: inp.spelling,
    animating: aBars || view.animating,
  };
}

/** The piano keys in view: row p, its rectangle (y0 top, y1 bottom), black or white, its name. */
export function pianoRows(L: LaneLayout, f: LaneFrame): { p: number; y0: number; y1: number; black: boolean; name: string }[] {
  const out: { p: number; y0: number; y1: number; black: boolean; name: string }[] = [];
  for (let p = f.rowLow; p <= f.rowHigh; p++) {
    const y0 = yOf(L, f.viewLow, p + 0.5);
    const y1 = yOf(L, f.viewLow, p - 0.5);
    if (y1 < L.barTop - 0.5 || y0 > L.barBottom + 0.5) continue;
    out.push({ p, y0, y1, black: isBlackKey(p), name: noteName(p, f.spelling) });
  }
  return out;
}

/** The bar of note `i` in frame `f`: its region's shift at the note's start, its centre y and x span. */
export function barOf(L: LaneLayout, f: LaneFrame, i: number): { x0: number; x1: number; y: number; shift: number } {
  const n = L.notes[i];
  const r = f.regions.find((rg) => n.start >= rg.t0 && n.start < rg.t1) ?? (n.start < f.regions[0].t0 ? f.regions[0] : f.regions[f.regions.length - 1]);
  return { x0: xOf(L, f.t, n.start), x1: xOf(L, f.t, n.end), y: yOf(L, f.viewLow, n.lane + r.shift), shift: r.shift };
}

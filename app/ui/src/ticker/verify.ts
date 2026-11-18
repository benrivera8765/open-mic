// SPDX-License-Identifier: GPL-3.0-or-later
// Checks a ticker layout against its own timing data, the way WORK_ORDER section 9
// "Ticker" asks: sample time every `dt` seconds across the whole song and verify
//   1. inside a grapheme's [start, end) the position under the marker lies in that
//      grapheme's cell (and the cell lookup returns it);
//   2. the motion never goes backwards;
//   3. the speed is at most `maxSecantRatio` times the average speed of the
//      surrounding knot intervals;
//   4. whenever a grapheme starts within `lookahead` seconds, it is already fully
//      visible right of the marker.
// Used by the unit test (fixed-width text shim) and, through window.__jkTicker, by the
// headless browser run with real fonts (scripts/screenshots.mjs).
//
// The word pace (layout.ts point 5, D-216) is checked against its own contract instead
// of points 1 and 3 (the letter cells do not apply to it); `wrongLetter` then counts
// samples with a wrong WORD under the marker:
//   W1. onset: every word's first timed letter reaches the marker within
//       WORD_TOLERANCE_S (40 ms) of the word's start (solved exactly, not sampled);
//   W2. word under the marker: while a word is sung (but its first and last 40 ms) the
//       marker is on it or on the space after it, which the word carries at its own pace; and
//       a word's letters are under the marker no earlier than 40 ms before it starts
//       and no later than 40 ms after the next word starts (in a pause the word just
//       sung may stay, nothing else is sung meanwhile);
//   W3. never backwards, and never stopped while a line is sung (speed of at least
//       MIN_SUNG_SPEED_EM);
//   4.  lookahead, as for the letter pace.
// `frameSmoothness` measures what the eye sees at 60 frames a second for either pace:
// the speed of each frame and how much it changes from one frame to the next.
import { glyphAtX, LOOKAHEAD_S, type TickerLayout } from "./layout";
import { WORD_TOLERANCE_S } from "./wordmotion";

/** Slowest speed (em/s) that still counts as moving while a line is sung (word pace). */
export const MIN_SUNG_SPEED_EM = 0.05;

export interface TickerVerifyReport {
  samples: number;
  /** Samples that fell inside some grapheme's [start, end). */
  sungSamples: number;
  /** Share of sung samples where the marker is on the glyph box itself (not its share of a space). */
  onGlyphBox: number;
  wrongLetter: number;
  backwards: number;
  tooFast: number;
  lookaheadMisses: number;
  maxSpeedPx: number;
  maxSpeedEm: number;
  maxSpeedOverSecant: number;
  /** Smallest lookahead (s) seen while singing continues within the lookahead window. */
  minLookaheadS: number;
  fontPx: number;
  fontScale: number;
  /** Smallest line scale (dense lines drawn smaller, layout.ts point 4b). */
  minLineScale: number;
  /** Lines drawn smaller than fontPx. */
  denseLines: number;
  firstProblems: string[];
  /** Pace of the checked layout; the fields below are filled for the word pace only. */
  pace?: "word" | "letter";
  /** Largest |time the first letter reaches the marker - word start|, ms (W1). */
  onsetMaxMs?: number;
  /** Words whose onset misses the tolerance (W1). */
  onsetMisses?: number;
  /** Share of sung samples with the marker on the sung word's letters. */
  onWord?: number;
  /** Share of sung samples with the marker more than half a space beside the sung word. */
  beyondHalfSpace?: number;
  /** Slowest speed (em/s) while a line is sung, and the samples below MIN_SUNG_SPEED_EM (W3). */
  minSungSpeedEm?: number;
  stopped?: number;
}

export interface FrameSmoothness {
  /** Frame pairs measured (both frames inside a sung line). */
  frames: number;
  /** Largest change of speed from one frame to the next, px/s and em/s. */
  maxDvPx: number;
  maxDvEm: number;
  /** 99th percentile of that change, em/s. */
  p99DvEm: number;
  /** Frame pairs whose speed changes by more than `threshold` (relative to the faster frame). */
  bigSteps: number;
  bigStepShare: number;
  threshold: number;
  maxSpeedEm: number;
  minSpeedEm: number;
}

/**
 * Speed of every frame at `fps` (strip distance per frame times fps) while a line is
 * sung, and how much it changes between consecutive frames: the stutter the eye sees.
 */
export function frameSmoothness(layout: TickerLayout, opts: { fps?: number; threshold?: number } = {}): FrameSmoothness {
  const fps = opts.fps ?? 60;
  const threshold = opts.threshold ?? 0.25;
  const dt = 1 / fps;
  const m = layout.motion;
  const em = layout.fontPx;
  const dvs: number[] = [];
  let big = 0;
  let maxSpeed = 0;
  let minSpeed = Infinity;
  for (const line of layout.lines) {
    let prevV = NaN;
    for (let k = 0; ; k++) {
      const t = line.start + k * dt;
      if (t + dt > line.end + 1e-9) break;
      const v = (m.at(t + dt) - m.at(t)) * fps;
      maxSpeed = Math.max(maxSpeed, v);
      minSpeed = Math.min(minSpeed, v);
      if (!Number.isNaN(prevV)) {
        const d = Math.abs(v - prevV);
        dvs.push(d);
        if (d > threshold * Math.max(v, prevV, 1e-9)) big++;
      }
      prevV = v;
    }
  }
  const sorted = [...dvs].sort((a, b) => a - b);
  const maxDv = sorted.length ? sorted[sorted.length - 1] : 0;
  return {
    frames: dvs.length,
    maxDvPx: maxDv,
    maxDvEm: maxDv / em,
    p99DvEm: sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(0.99 * sorted.length))] / em : 0,
    bigSteps: big,
    bigStepShare: dvs.length ? big / dvs.length : 0,
    threshold,
    maxSpeedEm: maxSpeed / em,
    minSpeedEm: (minSpeed === Infinity ? 0 : minSpeed) / em,
  };
}

/** The time (s) at which the monotone motion first reaches strip position `x` (bisection in [lo, hi]). */
export function timeAtX(layout: TickerLayout, x: number, lo: number, hi: number): number {
  const m = layout.motion;
  if (m.at(lo) >= x) return lo;
  if (m.at(hi) < x) return hi;
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2;
    if (m.at(mid) >= x) hi = mid;
    else lo = mid;
  }
  return hi;
}

export function verifyTickerLayout(layout: TickerLayout, opts: { dt?: number; lookahead?: number; maxSecantRatio?: number; pad?: number; tolerance?: number } = {}): TickerVerifyReport {
  return layout.pace === "word" ? verifyWordPace(layout, opts) : verifyLetterPace(layout, opts);
}

function verifyLetterPace(layout: TickerLayout, opts: { dt?: number; lookahead?: number; maxSecantRatio?: number; pad?: number }): TickerVerifyReport {
  const dt = opts.dt ?? 0.01;
  const lookahead = opts.lookahead ?? LOOKAHEAD_S;
  const maxRatio = opts.maxSecantRatio ?? 3;
  const pad = opts.pad ?? 5;
  const TOL = 1e-6;
  const gl = layout.glyphs;
  const timed = layout.timedIndex.map((i) => gl[i]);
  const motion = layout.motion;
  const rightW = layout.width - layout.markerX;
  const r: TickerVerifyReport = {
    samples: 0,
    sungSamples: 0,
    onGlyphBox: 1,
    wrongLetter: 0,
    backwards: 0,
    tooFast: 0,
    lookaheadMisses: 0,
    maxSpeedPx: 0,
    maxSpeedEm: 0,
    maxSpeedOverSecant: 0,
    minLookaheadS: Infinity,
    fontPx: layout.fontPx,
    fontScale: layout.fontScale,
    minLineScale: layout.minLineScale,
    denseLines: layout.denseLines,
    firstProblems: [],
  };
  if (timed.length === 0) return r;
  const problem = (s: string) => {
    if (r.firstProblems.length < 10) r.firstProblems.push(s);
  };
  const t0 = Math.floor((timed[0].start - pad) / dt) * dt;
  const t1 = timed[timed.length - 1].end + pad;
  let prevX = -Infinity;
  let onBox = 0;
  let ti = 0;
  let ahead = 0;
  let vis = 0;
  for (let k = 0; ; k++) {
    const t = t0 + k * dt;
    if (t > t1) break;
    r.samples++;
    const x = motion.at(t);
    if (x < prevX - TOL) {
      r.backwards++;
      problem(`t=${t.toFixed(3)}: moved backwards`);
    }
    prevX = x;

    const v = motion.speed(t);
    r.maxSpeedPx = Math.max(r.maxSpeedPx, v);
    const sec = Math.max(motion.secantAt(t - dt), motion.secantAt(t), motion.secantAt(t + dt));
    if (v > 1e-6) {
      const ratio = sec > 0 ? v / sec : Infinity;
      r.maxSpeedOverSecant = Math.max(r.maxSpeedOverSecant, ratio);
      if (ratio > maxRatio + 1e-9) {
        r.tooFast++;
        problem(`t=${t.toFixed(3)}: speed ${v.toFixed(0)} px/s is ${ratio.toFixed(2)}x the local average`);
      }
    }

    while (ti < timed.length && timed[ti].end <= t) ti++;
    if (ti < timed.length && timed[ti].start <= t) {
      const g = timed[ti];
      r.sungSamples++;
      const inCell = x >= g.L - TOL && x <= g.R + TOL;
      const hit = glyphAtX(layout, Math.min(x, g.R - 1e-7));
      if (!inCell || gl[hit] !== g) {
        r.wrongLetter++;
        problem(`t=${t.toFixed(3)}: "${g.g}" [${g.start}, ${g.end}) is sung but the marker is at ${x.toFixed(2)}, cell [${g.L.toFixed(2)}, ${g.R.toFixed(2)}]`);
      }
      if (x >= g.x - TOL && x <= g.x + g.w + TOL) onBox++;
    }

    while (ahead + 1 < gl.length && gl[ahead + 1].start <= t + lookahead) ahead++;
    const a = gl[ahead];
    if (a.start > t && a.start <= t + lookahead && a.x + a.w - x > rightW + TOL) {
      r.lookaheadMisses++;
      problem(`t=${t.toFixed(3)}: "${a.g}" starts at ${a.start} but is not fully on screen yet`);
    }
    while (vis < gl.length && gl[vis].x + gl[vis].w - x <= rightW + TOL) vis++;
    if (vis < gl.length) {
      const singingSoon = ti < timed.length && timed[ti].start <= t + lookahead;
      if (singingSoon) r.minLookaheadS = Math.min(r.minLookaheadS, gl[vis].start - t);
    }
  }
  r.onGlyphBox = r.sungSamples ? onBox / r.sungSamples : 1;
  r.maxSpeedEm = r.maxSpeedPx / layout.fontPx;
  return r;
}

/** Word pace: checks W1-W3 and 4 (see the header). `tooFast` does not apply and stays 0. */
function verifyWordPace(layout: TickerLayout, opts: { dt?: number; lookahead?: number; pad?: number; tolerance?: number }): TickerVerifyReport {
  const dt = opts.dt ?? 0.01;
  const lookahead = opts.lookahead ?? LOOKAHEAD_S;
  const pad = opts.pad ?? 5;
  const tol = opts.tolerance ?? WORD_TOLERANCE_S;
  const TOL = 1e-6;
  const gl = layout.glyphs;
  const words = layout.words;
  const motion = layout.motion;
  const rightW = layout.width - layout.markerX;
  const r: TickerVerifyReport = {
    samples: 0,
    sungSamples: 0,
    onGlyphBox: 1,
    wrongLetter: 0,
    backwards: 0,
    tooFast: 0,
    lookaheadMisses: 0,
    maxSpeedPx: 0,
    maxSpeedEm: 0,
    maxSpeedOverSecant: 0,
    minLookaheadS: Infinity,
    fontPx: layout.fontPx,
    fontScale: layout.fontScale,
    minLineScale: layout.minLineScale,
    denseLines: layout.denseLines,
    firstProblems: [],
    pace: "word",
    onsetMaxMs: 0,
    onsetMisses: 0,
    onWord: 1,
    beyondHalfSpace: 0,
    minSungSpeedEm: Infinity,
    stopped: 0,
  };
  if (words.length === 0) return r;
  const problem = (s: string) => {
    if (r.firstProblems.length < 10) r.firstProblems.push(s);
  };
  const text = (w: (typeof words)[number]) =>
    gl
      .slice(w.first, w.last + 1)
      .map((g) => g.g)
      .join("");
  const t0 = Math.floor((words[0].start - pad) / dt) * dt;
  const t1 = words[words.length - 1].end + pad;
  // W1: the exact moment each word's first timed letter reaches the marker.
  for (const w of words) {
    const reach = timeAtX(layout, w.a, Math.min(t0, layout.motion.start) - 1, t1 + 60);
    const err = reach - w.start;
    r.onsetMaxMs = Math.max(r.onsetMaxMs!, Math.abs(err) * 1000);
    if (Math.abs(err) > tol + 1e-6) {
      r.onsetMisses!++;
      problem(`"${text(w)}" starts at ${w.start} but reaches the marker at ${reach.toFixed(3)} (${(err * 1000).toFixed(0)} ms)`);
    }
  }
  const minSpeed = MIN_SUNG_SPEED_EM * layout.fontPx;
  let prevX = -Infinity;
  let onWord = 0;
  let beyond = 0;
  let wi = 0; // the word sung now (or the next one)
  let ui = 0; // the last word starting at or left of the marker
  let li = 0;
  let ahead = 0;
  let vis = 0;
  for (let k = 0; ; k++) {
    const t = t0 + k * dt;
    if (t > t1) break;
    r.samples++;
    const x = motion.at(t);
    if (x < prevX - TOL) {
      r.backwards++;
      problem(`t=${t.toFixed(3)}: moved backwards`);
    }
    prevX = x;
    r.maxSpeedPx = Math.max(r.maxSpeedPx, motion.speed(t));
    // W3: moving while a line is sung (distance over the next sample step).
    while (li < layout.lines.length && layout.lines[li].end <= t) li++;
    if (li < layout.lines.length && layout.lines[li].start <= t) {
      const vf = (motion.at(t + dt) - x) / dt;
      r.minSungSpeedEm = Math.min(r.minSungSpeedEm!, vf / layout.fontPx);
      if (vf < minSpeed) {
        r.stopped!++;
        problem(`t=${t.toFixed(3)}: ${vf.toFixed(1)} px/s while a line is sung`);
      }
    }
    // W2a: a word's letters are under the marker from tol before it starts until tol
    // after the next word starts.
    while (ui + 1 < words.length && words[ui + 1].a <= x + TOL) ui++;
    const u = words[ui];
    const until = ui + 1 < words.length ? words[ui + 1].start : u.end;
    if (x >= u.a - TOL && x < u.b - TOL && (t < u.start - tol - TOL || t > until + tol + TOL)) {
      r.wrongLetter++;
      problem(`t=${t.toFixed(3)}: the marker is on "${text(u)}" [${u.start}, ${u.end}] at ${x.toFixed(2)}`);
    }
    while (wi < words.length && words[wi].end <= t) wi++;
    if (wi < words.length && words[wi].start <= t) {
      const w = words[wi];
      r.sungSamples++;
      // W2b: while it is sung (but its first and last tol), the marker is on the word or the space after it.
      const nextA = wi + 1 < words.length ? words[wi + 1].a : Infinity;
      if (t >= w.start + tol + TOL && t < w.end - tol - TOL && (x < w.a - TOL || x >= nextA - TOL)) {
        r.wrongLetter++;
        problem(`t=${t.toFixed(3)}: "${text(w)}" [${w.start}, ${w.end}] is sung but the marker is at ${x.toFixed(2)}, word at [${w.a.toFixed(2)}, ${w.b.toFixed(2)}]`);
      }
      if (x >= w.a - TOL && x <= w.b + TOL) onWord++;
      const before = wi > 0 && words[wi - 1].line === w.line ? w.a - words[wi - 1].b : layout.spaceWidth;
      const after = wi + 1 < words.length && words[wi + 1].line === w.line ? words[wi + 1].a - w.b : layout.spaceWidth;
      if (x < w.a - before / 2 - TOL || x > w.b + after / 2 + TOL) beyond++;
    }
    // 4: lookahead, as for the letter pace.
    while (ahead + 1 < gl.length && gl[ahead + 1].start <= t + lookahead) ahead++;
    const a = gl[ahead];
    if (a.start > t && a.start <= t + lookahead && a.x + a.w - x > rightW + TOL) {
      r.lookaheadMisses++;
      problem(`t=${t.toFixed(3)}: "${a.g}" starts at ${a.start} but is not fully on screen yet`);
    }
    while (vis < gl.length && gl[vis].x + gl[vis].w - x <= rightW + TOL) vis++;
    if (vis < gl.length) {
      const singingSoon = wi < words.length && words[wi].start <= t + lookahead;
      if (singingSoon) r.minLookaheadS = Math.min(r.minLookaheadS, gl[vis].start - t);
    }
  }
  r.onWord = r.sungSamples ? onWord / r.sungSamples : 1;
  r.onGlyphBox = r.onWord;
  r.beyondHalfSpace = r.sungSamples ? beyond / r.sungSamples : 0;
  r.maxSpeedEm = r.maxSpeedPx / layout.fontPx;
  if (r.minSungSpeedEm === Infinity) r.minSungSpeedEm = 0;
  return r;
}

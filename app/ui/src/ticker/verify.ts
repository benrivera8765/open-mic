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
import { glyphAtX, LOOKAHEAD_S, type TickerLayout } from "./layout";

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
}

export function verifyTickerLayout(layout: TickerLayout, opts: { dt?: number; lookahead?: number; maxSecantRatio?: number; pad?: number } = {}): TickerVerifyReport {
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

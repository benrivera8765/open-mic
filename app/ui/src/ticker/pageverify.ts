// SPDX-License-Identifier: GPL-3.0-or-later
// Checks a page layout (page.ts, D-218) against its own timing data across the whole
// song, the way verify.ts checks the ticker. Used by the unit tests (page.test.ts) and,
// through window.__jkTicker.verify(), in the real app with real fonts.
//
//   (a) Wipe on the word being sung: each line's wipe is checked with the ticker's own
//       contract (verify.ts), on the line laid out as a one-line strip: word pace W1
//       (every word's first letter reached within 40 ms of its start), W2 (while a word
//       is sung, but its first and last 40 ms, the edge is on it or on the space after
//       it, which the word carries at its own pace), W3; letter pace: the edge in the
//       letter's cell.
//   (b) Never backwards within a line; for the word pace never stopped (at least
//       MIN_SUNG_SPEED_EM) while the line is sung.
//   (c) Every line fully visible, as the current line or the preview, at least
//       LOOKAHEAD_S before its first word. A two-line page cannot show a line before the
//       line above it is current: when that comes later than LOOKAHEAD_S before, the line
//       is listed as an exception (with the reason) if it was previewed from that moment,
//       and counted as a miss otherwise.
//   (d) Nothing outside the area, checked on frames across the whole song.
//   (e) The lead-in bar: only before lines after a pause of LEADIN_MIN_GAP_S or more,
//       draining monotonically, gone exactly when the first word starts.
//   (f) Line changes start within 250 ms of the previous line's last word, and the new
//       line has settled before its lead-in bar starts.
//   (n) The progress needle (page.ts point 6), on every frame checked for (d): its x is
//       the gold/white boundary as the renderer draws it (the row split between the sung
//       and the upcoming colour; with no row split, the end of the last fully sung row, or
//       the start of the first row), within NEEDLE_TOLERANCE_PX; it rides the line in the
//       current slot (a line leaving before a rest or at the end only while no line is in
//       the slot, and any leaving line at the instant a change starts before the next line
//       is drawn: without the preview it fades in from opacity 0); it is on the row of the word being sung (outside the word's first and
//       last 40 ms) and spans that row's band; it never moves left on a settled row; during
//       a lead-in bar it waits at the line's first letter (until the first word's 40 ms
//       tolerance); it is hidden while a rest shows the countdown; it stays inside the area.
import { LOOKAHEAD_S, type PlacedLine, type TickerLayout } from "./layout";
import {
  BAR_W_EM,
  INK_PAD_EM,
  LEADIN_MIN_GAP_S,
  LINE_HEIGHT_EM,
  NEEDLE_HEAD_H_EM,
  NEEDLE_HEAD_W_EM,
  NEEDLE_W_EM,
  type PageFrame,
  type PageLayout,
  type PageLine,
  pageFrame,
  PREVIEW_ALPHA,
  TRANSITION_S,
} from "./page";
import { type FrameSmoothness, frameSmoothness, type TickerVerifyReport, verifyTickerLayout } from "./verify";
import { WORD_TOLERANCE_S } from "./wordmotion";

/** Longest allowed time from a line's last word to the start of the change to the next line. */
export const SWITCH_MAX_S = 0.25;
/** (n) The needle and the gold/white boundary may differ by this much (CSS px). */
export const NEEDLE_TOLERANCE_PX = 0.5;

/** A page line as a one-line ticker strip (marker at 0, an endless lane), so verify.ts applies unchanged. */
export function lineAsTicker(l: PageLayout, line: PageLine): TickerLayout {
  const timedIndex: number[] = [];
  line.glyphs.forEach((g, i) => g.timed && timedIndex.push(i));
  const placed: PlacedLine = { start: line.start, end: line.end, x0: 0, x1: line.uEnd, first: 0, last: line.glyphs.length - 1, px: line.px, space: 0.28 * line.px };
  return {
    width: 1e12,
    marker: 0,
    markerX: 0,
    pace: l.pace,
    fontPx: line.px,
    fontScale: 1,
    minLineScale: 1,
    denseLines: 0,
    spaceWidth: placed.space,
    glyphs: line.glyphs,
    timedIndex,
    words: line.words,
    lines: [placed],
    rests: [],
    knots: line.knots,
    motion: line.motion,
    textStart: 0,
    textEnd: line.uEnd,
    worstLookahead: Infinity,
  };
}

export interface PageLeadException {
  line: number;
  leadS: number;
  reason: string;
}

export interface PageVerifyReport extends TickerVerifyReport {
  display: "page";
  lines: number;
  /** (c) Smallest time a line was fully visible before its first word, and the lines under LOOKAHEAD_S. */
  minLeadS: number;
  leadExceptions: PageLeadException[];
  /** Frames with a line or bar outside the area (d), sampled every `frameDt`. */
  frames: number;
  overflowFrames: number;
  /** Frames in which a line that should be readable was missing or faded (c). */
  hiddenFrames: number;
  /** (e) */
  leadIns: number;
  leadInProblems: number;
  /** (f) Largest delay from a line's last word to the start of the change, ms; changes over SWITCH_MAX_S. */
  maxSwitchMs: number;
  switchProblems: number;
  /** Lines drawn smaller than the song's size, and lines wrapped onto more rows. */
  shrunkLines: number;
  wrappedLines: number;
  /** The wipe at 60 fps while lines are sung (frameSmoothness of each line, combined). */
  smooth: FrameSmoothness;
  /** (n) Frames with the needle; problems by kind; largest distance to the gold/white boundary (CSS px); lead-in frames checked for the parked needle. */
  needleFrames: number;
  needleProblems: number;
  needleOffEdge: number;
  needleMaxDxPx: number;
  needleWrongLine: number;
  needleWrongRow: number;
  needleBackwards: number;
  needleNotParked: number;
  needleParkedFrames: number;
  needleInRest: number;
  needleOutside: number;
}

/** (n) The gold/white boundary of line `line` (drawn as `role`) in frame `f`, from the rows' own split (see the header). */
export function wipeBoundary(layout: PageLayout, f: PageFrame, line: number, role: string): { row: number; x: number } | null {
  const it = f.items.find((x) => x.line === line && x.role === role);
  const L = layout.lines[line];
  if (!it || !L || L.rows.length === 0) return null;
  let row = -1;
  let u = 0;
  // The row split between the colours (at most one: rows are wiped in reading order).
  L.rows.forEach((rw, r) => {
    const sung = it.wipe - rw.u0;
    if (sung > 0 && sung < rw.u1 - rw.u0) [row, u] = [r, sung];
  });
  // Else the end of the last row fully sung, else the start of the first row.
  if (row < 0)
    for (let r = L.rows.length - 1; r >= 0; r--) {
      const rw = L.rows[r];
      if (it.wipe >= rw.u1 && it.wipe > rw.u0) {
        [row, u] = [r, rw.u1 - rw.u0];
        break;
      }
    }
  if (row < 0) [row, u] = [0, 0];
  return { row, x: L.cx + (L.rows[row].left + u - L.cx) * it.scale };
}

export function verifyPage(layout: PageLayout, opts: { dt?: number; frameDt?: number; tolerance?: number } = {}): PageVerifyReport {
  const dt = opts.dt ?? 0.01;
  const frameDt = opts.frameDt ?? 1 / 30;
  const tol = opts.tolerance ?? WORD_TOLERANCE_S;
  const lines = layout.lines;
  const r: PageVerifyReport = {
    display: "page",
    lines: lines.length,
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
    denseLines: layout.shrunkLines + layout.wrappedLines,
    firstProblems: [],
    pace: layout.pace,
    onsetMaxMs: 0,
    onsetMisses: 0,
    onWord: 1,
    beyondHalfSpace: 0,
    minSungSpeedEm: Infinity,
    stopped: 0,
    minLeadS: Infinity,
    leadExceptions: [],
    frames: 0,
    overflowFrames: 0,
    hiddenFrames: 0,
    leadIns: 0,
    leadInProblems: 0,
    maxSwitchMs: 0,
    switchProblems: 0,
    shrunkLines: layout.shrunkLines,
    wrappedLines: layout.wrappedLines,
    smooth: { frames: 0, maxDvPx: 0, maxDvEm: 0, p99DvEm: 0, bigSteps: 0, bigStepShare: 0, threshold: 0.25, maxSpeedEm: 0, minSpeedEm: Infinity },
    needleFrames: 0,
    needleProblems: 0,
    needleOffEdge: 0,
    needleMaxDxPx: 0,
    needleWrongLine: 0,
    needleWrongRow: 0,
    needleBackwards: 0,
    needleNotParked: 0,
    needleParkedFrames: 0,
    needleInRest: 0,
    needleOutside: 0,
  };
  const problem = (s: string) => {
    if (r.firstProblems.length < 10) r.firstProblems.push(s);
  };
  if (lines.length === 0) return r;

  // (a), (b): the ticker's contract on every line, and the wipe's smoothness.
  let onWordSamples = 0;
  let beyond = 0;
  const p99s: number[] = [];
  for (const line of lines) {
    if (line.words.length === 0) continue;
    const strip = lineAsTicker(layout, line);
    const v = verifyTickerLayout(strip, { dt, pad: 0.5, tolerance: tol });
    r.samples += v.samples;
    r.sungSamples += v.sungSamples;
    r.wrongLetter += v.wrongLetter;
    r.backwards += v.backwards;
    r.tooFast += v.tooFast;
    r.maxSpeedPx = Math.max(r.maxSpeedPx, v.maxSpeedPx);
    r.maxSpeedEm = Math.max(r.maxSpeedEm, v.maxSpeedEm);
    r.maxSpeedOverSecant = Math.max(r.maxSpeedOverSecant, v.maxSpeedOverSecant);
    onWordSamples += (v.onWord ?? v.onGlyphBox) * v.sungSamples;
    beyond += (v.beyondHalfSpace ?? 0) * v.sungSamples;
    if (layout.pace === "word") {
      r.onsetMaxMs = Math.max(r.onsetMaxMs!, v.onsetMaxMs ?? 0);
      r.onsetMisses! += v.onsetMisses ?? 0;
      r.stopped! += v.stopped ?? 0;
      r.minSungSpeedEm = Math.min(r.minSungSpeedEm!, v.minSungSpeedEm ?? Infinity);
    }
    for (const p of v.firstProblems) problem(`line ${line.index + 1} "${line.text}": ${p}`);
    const s = frameSmoothness(strip);
    r.smooth.frames += s.frames;
    r.smooth.bigSteps += s.bigSteps;
    if (s.maxDvEm > r.smooth.maxDvEm) [r.smooth.maxDvEm, r.smooth.maxDvPx] = [s.maxDvEm, s.maxDvPx];
    r.smooth.maxSpeedEm = Math.max(r.smooth.maxSpeedEm, s.maxSpeedEm);
    if (s.frames > 0) r.smooth.minSpeedEm = Math.min(r.smooth.minSpeedEm, s.minSpeedEm);
    if (s.frames > 0) p99s.push(s.p99DvEm);
  }
  r.onWord = r.sungSamples ? onWordSamples / r.sungSamples : 1;
  r.onGlyphBox = r.onWord;
  r.beyondHalfSpace = r.sungSamples ? beyond / r.sungSamples : 0;
  r.smooth.bigStepShare = r.smooth.frames ? r.smooth.bigSteps / r.smooth.frames : 0;
  r.smooth.p99DvEm = p99s.length ? Math.max(...p99s) : 0;
  if (r.smooth.minSpeedEm === Infinity) r.smooth.minSpeedEm = 0;
  if (r.minSungSpeedEm === Infinity) r.minSungSpeedEm = 0;
  if (layout.pace !== "word") {
    // Fields of the word contract that do not apply to the letter pace.
    delete r.onsetMaxMs;
    delete r.onsetMisses;
    delete r.stopped;
    delete r.minSungSpeedEm;
  }

  // (c) Lead time: from the moment the line is fully shown (preview or current) to its first word.
  const visibleFrom = lines.map((L) => Math.min(L.previewFrom !== null ? L.previewFrom + TRANSITION_S : Infinity, L.current + TRANSITION_S));
  lines.forEach((L, i) => {
    const lead = L.start - visibleFrom[i];
    r.minLeadS = Math.min(r.minLeadS, lead);
    if (lead >= LOOKAHEAD_S - 1e-9) return;
    const prev = i > 0 ? lines[i - 1] : null;
    if (prev && L.previewFrom !== null && L.previewFrom === prev.current) {
      const shown = prev.current === -Infinity ? "from the start" : `${(L.start - prev.current).toFixed(2)} s before`;
      r.leadExceptions.push({ line: i, leadS: lead, reason: `previewed as soon as line ${i} ("${prev.text}") became current, ${shown} line ${i + 1} starts (line ${i} is ${(prev.end - prev.start).toFixed(2)} s long)` });
    } else {
      r.lookaheadMisses++;
      problem(`line ${i + 1} "${L.text}" is fully shown only ${lead.toFixed(2)} s before it is sung`);
    }
  });
  r.minLookaheadS = r.minLeadS;

  // (e), (f) from the schedule.
  lines.forEach((L, i) => {
    const wants = L.gapBefore >= LEADIN_MIN_GAP_S;
    if (wants !== (L.leadIn !== null)) {
      r.leadInProblems++;
      problem(`line ${i + 1}: lead-in bar ${L.leadIn ? "shown" : "missing"} after a ${L.gapBefore.toFixed(2)} s pause`);
    }
    if (L.leadIn) {
      r.leadIns++;
      if (Math.abs(L.leadIn.to - L.start) > 1e-9) {
        r.leadInProblems++;
        problem(`line ${i + 1}: lead-in ends at ${L.leadIn.to}, first word at ${L.start}`);
      }
      if (L.leadIn.from < L.current + TRANSITION_S - 1e-9) {
        r.switchProblems++;
        problem(`line ${i + 1}: lead-in starts before the line has settled`);
      }
    }
    if (i === 0) return;
    const prev = lines[i - 1];
    const change = L.breakBefore ? prev.leave : L.current;
    const d = change - prev.end;
    r.maxSwitchMs = Math.max(r.maxSwitchMs, d * 1000);
    if (d < -1e-9 || d > SWITCH_MAX_S + 1e-9) {
      r.switchProblems++;
      problem(`line ${i}: the change to line ${i + 1} starts ${(d * 1000).toFixed(0)} ms after its last word`);
    }
  });

  // (c), (d), (e) on frames across the song.
  const first = lines[0];
  const last = lines[lines.length - 1];
  const t0 = Math.min(first.start - 6, 0);
  const t1 = last.leave + TRANSITION_S + 0.5;
  const W = layout.width;
  const H = layout.height;
  const lastFrac = new Map<number, number>();
  // (n) The needle on frame f.
  type NeedleKind = "needleOffEdge" | "needleWrongLine" | "needleWrongRow" | "needleBackwards" | "needleNotParked" | "needleInRest" | "needleOutside";
  let lastNeedle: { line: number; row: number; x: number } | null = null;
  const needleProblem = (kind: NeedleKind, t: number, s: string) => {
    r[kind]++;
    r.needleProblems++;
    problem(`t=${t.toFixed(3)}: needle ${s}`);
  };
  const checkNeedle = (f: PageFrame, t: number) => {
    const n = f.needle;
    const inRest = layout.rests.some((rs) => t >= rs.from && t < rs.to);
    if (inRest && n) needleProblem("needleInRest", t, `on line ${n.line + 1} during a rest with the countdown`);
    // The line it must ride: the one in the current slot, else one leaving before a rest or
    // at the end; or one leaving at the very instant its change starts while the next line
    // is not drawn yet (without the preview the next line fades in from opacity 0, so at
    // t = its `current` nothing is in the slot and the needle stays on the leaving line).
    const slot = f.current >= 0 ? f.items.find((x) => x.line === f.current && (x.role === "current" || x.role === "entering")) : undefined;
    const leaving = f.items.find((x) => x.role === "leaving");
    const handOver = (i: number) => i + 1 < lines.length && lines[i + 1].current <= t;
    const want = slot ?? (f.current < 0 && leaving && !inRest && (leaving.line === lines.length - 1 || lines[leaving.line + 1].breakBefore || handOver(leaving.line)) ? leaving : undefined);
    if (!n) {
      if (want) needleProblem("needleWrongLine", t, `missing (line ${want.line + 1} is ${want.role})`);
      lastNeedle = null;
      return;
    }
    r.needleFrames++;
    if (!want || want.line !== n.line || want.role !== n.role) {
      needleProblem("needleWrongLine", t, `on line ${n.line + 1} (${n.role}), expected ${want ? `line ${want.line + 1} (${want.role})` : "none"}`);
      lastNeedle = null;
      return;
    }
    const L = lines[n.line];
    const it = want;
    // On the gold/white boundary.
    const b = wipeBoundary(layout, f, n.line, n.role)!;
    const dx = Math.abs(n.x - b.x);
    if (b.row === n.row) r.needleMaxDxPx = Math.max(r.needleMaxDxPx, dx);
    if (b.row !== n.row || dx > NEEDLE_TOLERANCE_PX) needleProblem("needleOffEdge", t, `at x ${n.x.toFixed(2)} on row ${n.row + 1}, the boundary at x ${b.x.toFixed(2)} on row ${b.row + 1}`);
    // On the row of the word being sung; or, where (a) lets the edge be in the space the
    // word carries up to the next word's first sung letter, on a row of that space (it can
    // hold the row break and graphemes of the next word sung in no time); or at the end of
    // the row before while the wipe has not reached the word's row (the letter pace's first
    // cell can start in the space).
    const k = L.words.findIndex((w) => t >= w.start + tol && t < w.end - tol);
    if (k >= 0) {
      const sung = L.words[k];
      const wr = L.rows.findIndex((rw) => sung.first >= rw.first && sung.first <= rw.last);
      const nextA = k + 1 < L.words.length ? L.words[k + 1].a : L.uEnd;
      const rw = L.rows[n.row];
      const inCarried = it.wipe >= sung.a - 1e-9 && it.wipe <= nextA + 1e-9 && rw.u0 < nextA && rw.u1 > sung.a;
      const between = n.row === wr - 1 && it.wipe <= L.rows[wr].u0;
      if (wr !== n.row && !inCarried && !between) needleProblem("needleWrongRow", t, `on row ${n.row + 1} of line ${n.line + 1}, the word sung is on row ${wr + 1}`);
    }
    const band = it.top + L.rows[n.row].top * it.scale;
    const bandEnd = band + LINE_HEIGHT_EM * L.px * it.scale;
    if (n.top > band + 1e-6 || n.bottom < bandEnd - 1e-6) needleProblem("needleWrongRow", t, `spans y ${n.top.toFixed(1)}..${n.bottom.toFixed(1)}, its row's band ${band.toFixed(1)}..${bandEnd.toFixed(1)}`);
    // Never to the left on a settled row.
    if (it.role === "current" && lastNeedle && lastNeedle.line === n.line && lastNeedle.row === n.row && n.x < lastNeedle.x - 1e-6) needleProblem("needleBackwards", t, `moved left from ${lastNeedle.x.toFixed(2)} to ${n.x.toFixed(2)}`);
    lastNeedle = it.role === "current" ? { line: n.line, row: n.row, x: n.x } : null;
    // Parked at the first letter during the lead-in bar. (Graphemes before the first sung
    // letter that an aligner gave no time are swept during the wipe's run-up, as the
    // wipe always did: then the needle may only have crossed those.)
    if (f.leadIn) {
      const P = lines[f.leadIn.line];
      if (n.line !== f.leadIn.line) needleProblem("needleNotParked", t, `on line ${n.line + 1} during line ${f.leadIn.line + 1}'s lead-in`);
      else if (t < P.start - tol) {
        r.needleParkedFrames++;
        const firstA = P.words.length ? P.words[0].a : 0;
        const ok = n.row === 0 && (firstA > 0 ? n.x >= P.rows[0].left - 1e-9 && n.x <= P.rows[0].left + firstA + 1e-6 : Math.abs(n.x - P.rows[0].left) <= NEEDLE_TOLERANCE_PX);
        if (!ok) needleProblem("needleNotParked", t, `at x ${n.x.toFixed(2)} (row ${n.row + 1}) during the lead-in, the first letter at x ${P.rows[0].left.toFixed(2)}`);
      }
    }
    // Inside the area: shaft and head.
    const half = (Math.max(NEEDLE_W_EM, NEEDLE_HEAD_W_EM) * n.px) / 2;
    const top = n.top - NEEDLE_HEAD_H_EM * n.px;
    if (n.x - half < -0.5 || n.x + half > W + 0.5 || top < -0.5 || n.bottom > H + 0.5) needleProblem("needleOutside", t, `x ${(n.x - half).toFixed(1)}..${(n.x + half).toFixed(1)}, y ${top.toFixed(1)}..${n.bottom.toFixed(1)} outside ${W}x${H}`);
  };
  for (let k = 0; ; k++) {
    const t = t0 + k * frameDt;
    if (t > t1) break;
    r.frames++;
    const f = pageFrame(layout, t);
    let out = false;
    for (const it of f.items) {
      const L = lines[it.line];
      const ink = INK_PAD_EM * L.px;
      for (const row of L.rows) {
        const x0 = L.cx + (row.left - ink - L.cx) * it.scale;
        const x1 = L.cx + (row.left + (row.u1 - row.u0) + ink - L.cx) * it.scale;
        const y0 = it.top + (row.top - ink) * it.scale;
        const y1 = it.top + (row.top + LINE_HEIGHT_EM * L.px + ink) * it.scale;
        if (x0 < -0.5 || x1 > W + 0.5 || y0 < -0.5 || y1 > H + 0.5) {
          out = true;
          problem(`t=${t.toFixed(2)}: line ${it.line + 1} (${it.role}) row at x ${x0.toFixed(1)}..${x1.toFixed(1)}, y ${y0.toFixed(1)}..${y1.toFixed(1)} outside ${W}x${H}`);
        }
      }
    }
    if (f.leadIn) {
      const b = f.leadIn;
      if (b.x < -0.5 || b.x + BAR_W_EM * b.px > W + 0.5) out = true;
      const L = lines[b.line];
      if (!L.leadIn) {
        r.leadInProblems++;
        problem(`t=${t.toFixed(2)}: a lead-in bar for line ${b.line + 1}, which has none`);
      }
      const prev = lastFrac.get(b.line);
      if (b.frac < 0 || b.frac > 1 + 1e-9 || (prev !== undefined && b.frac > prev + 1e-12)) {
        r.leadInProblems++;
        problem(`t=${t.toFixed(2)}: lead-in bar of line ${b.line + 1} at ${b.frac.toFixed(4)} after ${prev?.toFixed(4)}`);
      }
      lastFrac.set(b.line, b.frac);
    }
    if (out) r.overflowFrames++;
    checkNeedle(f, t);
    // (c) every line that should be readable now is drawn, fully.
    lines.forEach((L, i) => {
      if (t < visibleFrom[i] || t >= L.start) return;
      const it = f.items.find((x) => x.line === i);
      if (!it || it.alpha < PREVIEW_ALPHA - 1e-6) {
        r.hiddenFrames++;
        problem(`t=${t.toFixed(2)}: line ${i + 1} should be readable (${(L.start - t).toFixed(2)} s before it is sung) but is ${it ? `at ${it.alpha.toFixed(2)} opacity` : "not drawn"}`);
      }
    });
  }
  // (e) at the exact end: still there one frame before the first word, gone at it.
  for (const L of lines) {
    if (!L.leadIn) continue;
    const before = pageFrame(layout, L.leadIn.to - 1 / 60).leadIn;
    const at = pageFrame(layout, L.leadIn.to).leadIn;
    const exp = 1 / 60 / (L.leadIn.to - L.leadIn.from);
    if (!before || before.line !== L.index || Math.abs(before.frac - exp) > 1e-6 || (at && at.line === L.index)) {
      r.leadInProblems++;
      problem(`line ${L.index + 1}: lead-in bar ${before ? before.frac.toFixed(4) : "absent"} one frame before the first word, ${at ? at.frac.toFixed(4) : "gone"} at it`);
    }
  }
  return r;
}

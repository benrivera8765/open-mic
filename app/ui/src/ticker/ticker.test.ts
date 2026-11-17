// SPDX-License-Identifier: GPL-3.0-or-later
// WORK_ORDER section 9 "Ticker": the letter under the marker matches the timing data
// across whole songs, at several window sizes.
//
// Text is measured with a deterministic shim (fixed per-character widths, in em), so
// the test is exact and independent of fonts. Time is sampled every 10 ms across the
// whole song (from 5 s before the first letter to 5 s after the last) at window
// sizes 1280x800, 1440x900, 2560x1440 and 3840x2160. For every sample:
//   1. inside a grapheme's [start, end): the strip position under the marker lies in
//      that grapheme's cell, and the cell lookup returns that grapheme;
//   2. motion is monotone (never moves backwards);
//   3. speed is bounded: at most 3x the average speed of the surrounding knot
//      intervals (Steffen bound is 2x at knots), and an absolute em/s ceiling;
//   4. lookahead: whenever a grapheme starts within the next 2 s, it is already fully
//      visible right of the marker.
// Numbers are printed (and written to $TICKER_REPORT if set) for the benchmarks doc.
import { existsSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { FRERE_LYRICS, lyricsFromScore, prng, ROW_LYRICS, TWINKLE_LYRICS } from "../api/demo";
import { glyphAtX, layoutTicker, LOOKAHEAD_S, type Measure, restAt, shareSeparator } from "./layout";
import { type TickerVerifyReport, verifyTickerLayout } from "./verify";
import { MonotoneSpline } from "./motion";
import { readJksong } from "./jksong-node";

// ---------------------------------------------------------------------------
// Deterministic text measurement: fixed advance per character (em).
// ---------------------------------------------------------------------------
const NARROW = new Set([..."il.,;:!'|Iı"]);
const SEMI = new Set([..."fjtr()-?\"[]"]);
const WIDE = new Set([..."mwMW@"]);
function charEm(ch: string): number {
  if (ch === " ") return 0.28;
  if (NARROW.has(ch)) return 0.28;
  if (SEMI.has(ch)) return 0.36;
  if (WIDE.has(ch)) return 0.86;
  if (/[A-Z]/.test(ch)) return 0.68;
  if (/[0-9]/.test(ch)) return 0.56;
  return 0.56;
}
/** A grapheme is as wide as its base character (combining marks add nothing). */
export const shimMeasure: Measure = (text, fontPx) => {
  let em = 0;
  for (const g of text.normalize("NFD").replace(/\p{M}/gu, "")) em += charEm(g);
  return em * fontPx;
};

// ---------------------------------------------------------------------------
// Lyrics under test
// ---------------------------------------------------------------------------
const here = fileURLToPath(new URL(".", import.meta.url));
const fixturesDir = join(here, "..", "..", "..", "..", "tests", "fixtures", "generated");

/** A synthetic worst case: very fast lines, long holds, zero gaps, zero-length letters, a 40 s break. */
function stressLyrics(): Lyrics {
  const rnd = prng(99);
  const words = "the quick brown fox jumps over lazy dogs while singing merrily along naïve café".split(" ");
  const lines = [];
  let beat = 2;
  for (let l = 0; l < 14; l++) {
    const n = 3 + Math.floor(rnd() * 6);
    const ws: [string, number][] = [];
    for (let k = 0; k < n; k++) ws.push([words[Math.floor(rnd() * words.length)], l % 4 === 2 ? 0.25 : 0.5 + Math.floor(rnd() * 3) * 0.5]);
    lines.push({ beat, words: ws });
    beat += ws.reduce((a, w) => a + w[1], 0) + (l === 6 ? 80 : l % 3 === 0 ? 0 : 1 + Math.floor(rnd() * 3));
  }
  const lyr = lyricsFromScore(lines, { bpm: 120, offset: 0, seed: 7 });
  // Zero-length graphemes (an aligner sometimes emits them): the comma-like last
  // letter of a few words gets start == end.
  lyr.lines.forEach((line, li) => {
    if (li % 5 !== 1) return;
    const w = line.words[0];
    const last = w.chars[w.chars.length - 1];
    const prev = w.chars[w.chars.length - 2];
    if (prev) {
      prev[2] = last[2];
      last[1] = last[2];
    }
  });
  return lyr;
}

const sets: { name: string; lyrics: Lyrics; natural: boolean }[] = [
  { name: "demo: Twinkle, Twinkle, Little Star", lyrics: TWINKLE_LYRICS, natural: true },
  { name: "demo: Frère Jacques", lyrics: FRERE_LYRICS, natural: true },
  { name: "demo: Row, Row, Row Your Boat (triplets)", lyrics: ROW_LYRICS, natural: true },
  { name: "synthetic stress", lyrics: stressLyrics(), natural: false },
];
if (existsSync(fixturesDir)) {
  for (const f of readdirSync(fixturesDir).filter((n) => n.endsWith(".jksong")).sort()) {
    const { lyrics } = readJksong(join(fixturesDir, f));
    if (lyrics) sets.push({ name: `fixture: ${f}`, lyrics, natural: true });
  }
}

const WINDOWS = [
  { w: 1280, h: 800 },
  { w: 1440, h: 900 },
  { w: 2560, h: 1440 },
  { w: 3840, h: 2160 },
];
/** settings.ui.lyrics_text_size default (CSS px at 1440x900), scaled like the app does. */
const TEXT_SIZE = 56;
const MARKER = 0.33;
const DT = 0.01;

interface Row extends TickerVerifyReport {
  set: string;
  window: string;
}
const rows: Row[] = [];

function sample(lyrics: Lyrics, win: { w: number; h: number }): Row {
  const fontPx = TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
  const layout = layoutTicker(lyrics, { width: win.w, marker: MARKER, fontPx }, shimMeasure);
  return { set: "", window: `${win.w}x${win.h}`, ...verifyTickerLayout(layout, { dt: DT, lookahead: LOOKAHEAD_S, maxSecantRatio: 3 }) };
}

describe("lyrics ticker: letter under the marker", () => {
  for (const s of sets) {
    for (const win of WINDOWS) {
      it(`${s.name} @ ${win.w}x${win.h}`, () => {
        const row = sample(s.lyrics, win);
        row.set = s.name;
        rows.push(row);
        expect(row.firstProblems).toEqual([]);
        expect(row.sungSamples).toBeGreaterThan(0);
        expect(row.wrongLetter).toBe(0);
        expect(row.backwards).toBe(0);
        expect(row.tooFast).toBe(0);
        expect(row.lookaheadMisses).toBe(0);
        expect(row.minLookaheadS).toBeGreaterThanOrEqual(LOOKAHEAD_S - 1e-9);
        if (s.natural) {
          // Natural singing: the font is not shrunk and the speed stays bounded.
          expect(row.fontScale).toBe(1);
          expect(row.maxSpeedEm).toBeLessThan(40);
        }
      });
    }
  }

  it("report", () => {
    const lines = rows.map(
      (r) =>
        `${r.set.padEnd(46)} ${r.window.padEnd(10)} samples ${String(r.samples).padStart(6)}  sung ${String(r.sungSamples).padStart(6)}  on-glyph ${(r.onGlyphBox * 100).toFixed(1)}%  font x${r.fontScale.toFixed(2)}  max ${r.maxSpeedPx.toFixed(0)} px/s = ${r.maxSpeedEm.toFixed(1)} em/s  max/secant ${r.maxSpeedOverSecant.toFixed(2)}  min lookahead ${r.minLookaheadS === Infinity ? "n/a" : r.minLookaheadS.toFixed(2) + " s"}`,
    );
    console.log(`\nTicker check (${DT * 1000} ms steps, marker ${MARKER}, text ${TEXT_SIZE}px @1440x900):\n${lines.join("\n")}\n`);
    if (process.env.TICKER_REPORT) writeFileSync(process.env.TICKER_REPORT, JSON.stringify(rows, null, 2));
    expect(rows.length).toBe(sets.length * WINDOWS.length);
  });
});

describe("ticker layout details", () => {
  it("draws every grapheme at a precomputed x: words by one space, letters back to back", () => {
    const l = layoutTicker(TWINKLE_LYRICS, { width: 1440, marker: 0.33, fontPx: 56 }, shimMeasure);
    const g = l.glyphs;
    for (let i = 1; i < g.length; i++) {
      if (g[i].line !== g[i - 1].line) continue;
      const gap = g[i].x - (g[i - 1].x + g[i - 1].w);
      if (g[i].word === g[i - 1].word) expect(gap).toBeCloseTo(0, 9);
      else expect(gap).toBeCloseTo(l.spaceWidth, 9);
    }
  });

  it("shows a countdown in long breaks and brings the next line in from the right in time", () => {
    const l = layoutTicker(TWINKLE_LYRICS, { width: 1440, marker: 0.33, fontPx: 56 }, shimMeasure);
    const breaks = l.rests.filter((r) => r.countdown);
    expect(breaks.length).toBeGreaterThanOrEqual(2); // intro + the 12 s break
    for (const r of breaks) {
      const mid = (r.from + r.to) / 2;
      const c = restAt(l, mid);
      expect(c?.remaining).toBeCloseTo(r.to - mid, 9);
      const line = l.lines[r.nextLine];
      const rightW = l.width - l.markerX;
      // 3 s ahead the line is at (or inside) the right edge; 2 s ahead it is visible.
      expect(line.x0 - l.motion.at(r.to - 3)).toBeLessThanOrEqual(rightW + 1e-6);
      expect(line.x0 - l.motion.at(r.to - 2)).toBeLessThan(rightW);
      // Well before that, in a long break, the next line is still off screen to the right.
      if (r.to - r.from > 8) expect(line.x0 - l.motion.at(r.to - 6)).toBeGreaterThanOrEqual(rightW - 1e-6);
    }
  });

  it("handles graphemes made of several code points (combining marks) as one letter", () => {
    const lyr: Lyrics = {
      lyrics_version: 1,
      language: "en",
      source: "user_text",
      checked: true,
      lines: [{ start: 1, end: 2, words: [{ start: 1, end: 2, chars: [["n", 1, 1.2], ["a", 1.2, 1.4], ["ï", 1.4, 1.6], ["v", 1.6, 1.8], ["e", 1.8, 2]] }] }],
    };
    const l = layoutTicker(lyr, { width: 1440, marker: 0.33, fontPx: 56 }, shimMeasure);
    expect(l.glyphs.map((g) => g.g)).toEqual(["n", "a", "ï", "v", "e"]);
    const x = l.motion.at(1.5);
    expect(l.glyphs[glyphAtX(l, x)].g).toBe("ï");
  });

  it("ignores lines without words (allowed by the format) instead of breaking the line indices", () => {
    // Found by the independent verifier: a `"words": []` line before a long break made
    // the layout (and the renderer's countdown) look up a missing line and throw.
    const word = (s: string, t0: number) => ({ start: t0, end: t0 + 0.2 * s.length, chars: [...s].map((c, k) => [c, t0 + 0.2 * k, t0 + 0.2 * (k + 1)] as [string, number, number]) });
    const lyr: Lyrics = {
      lyrics_version: 1,
      language: "en",
      source: "user_text",
      checked: true,
      lines: [
        { start: 1, end: 2, words: [word("hello", 1)] },
        { start: 5, end: 5, words: [] },
        { start: 20, end: 21, words: [word("world", 20)] },
      ],
    };
    const l = layoutTicker(lyr, { width: 1440, marker: 0.33, fontPx: 56 }, shimMeasure);
    const rest = restAt(l, 10);
    expect(rest).not.toBeNull();
    expect(l.glyphs[l.lines[rest!.rest.nextLine].first].g).toBe("w");
    const r = verifyTickerLayout(l);
    expect(r.wrongLetter).toBe(0);
    expect(r.backwards).toBe(0);
  });

  it("splits a separator crossed in zero time evenly, and shares a short one by speed", () => {
    expect(shareSeparator(10, 0, 20, 0.1, 20, 0.1)).toEqual([5, 5]);
    const [a, b] = shareSeparator(10, 0.01, 20, 1, 20, 0.05);
    // The slow left neighbour (20 px in 1 s) takes more of the separator than the fast one.
    expect(a).toBeGreaterThan(b);
    expect(a).toBeLessThanOrEqual(5);
  });

  it("monotone spline never leaves the knot interval and keeps speed continuous", () => {
    const rnd = prng(3);
    const knots = [{ t: 0, x: 0 }];
    for (let i = 1; i < 300; i++) knots.push({ t: knots[i - 1].t + 0.01 + rnd() * 0.5, x: knots[i - 1].x + (rnd() < 0.1 ? 0 : rnd() * 40) });
    const s = new MonotoneSpline(knots);
    for (let i = 0; i + 1 < knots.length; i++) {
      for (let u = 0; u <= 1; u += 0.05) {
        const t = knots[i].t + u * (knots[i + 1].t - knots[i].t);
        const x = s.at(t);
        expect(x).toBeGreaterThanOrEqual(knots[i].x - 1e-9);
        expect(x).toBeLessThanOrEqual(knots[i + 1].x + 1e-9);
      }
      // C1: speed from both sides of a knot agrees
      const tk = knots[i + 1].t;
      if (i + 2 < knots.length) expect(Math.abs(s.speed(tk - 1e-9) - s.speed(tk + 1e-9))).toBeLessThan(1e-3 * (1 + s.speed(tk)));
    }
  });
});

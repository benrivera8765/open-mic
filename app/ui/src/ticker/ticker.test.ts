// SPDX-License-Identifier: GPL-3.0-or-later
// WORK_ORDER section 9 "Ticker": the letter under the marker matches the timing data
// across whole songs, at several window sizes.
//
// Text is measured with a deterministic shim (fixed per-character widths, in em), so
// the test is exact and independent of fonts. Time is sampled every 10 ms across the
// whole song (from 5 s before the first letter to 5 s after the last) at window
// sizes 1280x800, 1440x900, 2560x1440 and 3840x2160. A second block repeats the
// check the way the app runs on the owner's Mac: the lane width the jukebox skin
// leaves at 1440x900 and 1280x800, and the advance widths of the skin's lyrics face
// (lyrics-face-widths.ts). For every sample:
//   1. inside a grapheme's [start, end): the strip position under the marker lies in
//      that grapheme's cell, and the cell lookup returns that grapheme;
//   2. motion is monotone (never moves backwards);
//   3. speed is bounded: at most 3x the average speed of the surrounding knot
//      intervals (Steffen bound is 2x at knots), and an absolute em/s ceiling;
//   4. lookahead: whenever a grapheme starts within the next 2 s, it is already fully
//      visible right of the marker.
// Numbers are printed (and written to $TICKER_REPORT if set) for the benchmarks doc.
// The blocks above test the letter pace (settings.ui.lyrics_motion = "letter", the
// layout's own default) and are unchanged since D-167.
//
// Word pace (the app's default since the owner's request of 18 Nov 2026, D-216): the
// same songs at the same sizes, checked against the word contract of verify.ts
// (W1 onset within 40 ms, W2 word under the marker, W3 never backwards or stopped while
// a line is sung, 4 lookahead), and compared with the letter pace frame by frame at
// 60 fps: the largest change of speed between consecutive frames and the share of
// frames whose speed changes by more than 25 %.
//
// Real songs: the .jksong files of the end-to-end run (docs/benchmarks/end-to-end.md)
// are read at test time from $JK_TICKER_SONGS, or $JK_BUILD/e2e-stick/Library, or
// ../jukeoke-build/e2e-stick/Library next to the repository, and skipped when absent.
// Their lyrics are never copied into the repository; `denseRapLyrics` stands in for
// the rap song with generated words and a timing density profile like its own.
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { DEFAULT_APP_SETTINGS } from "../api/defaults";
import { FRERE_LYRICS, lyricsFromScore, prng, ROW_LYRICS, TWINKLE_LYRICS } from "../api/demo";
import { glyphAtX, layoutTicker, LOOKAHEAD_S, type Measure, MIN_LINE_SCALE, restAt, shareSeparator, type TickerLayout } from "./layout";
import { type FrameSmoothness, frameSmoothness, MIN_SUNG_SPEED_EM, type TickerVerifyReport, verifyTickerLayout } from "./verify";
import { MonotoneSpline } from "./motion";
import { RampMotion, straightenKnots, WORD_TOLERANCE_S } from "./wordmotion";
import { readJksong } from "./jksong-node";
import { lyricsFaceMeasure } from "./lyrics-face-widths";
import { TickerRenderer } from "./renderer";

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

/**
 * Generated rap: lines of 3-9 common words (4.1 letters a word on average), sung at
 * 2.5-5.7 words a second with 0-0.55 s between lines, two verses with slower hooks.
 * The end-to-end rap song measures 3-8 words a line (p10-p90), 3.95 words/s median
 * line rate (p90 5.41, max 5.71), 4.11 letters a word and 0.30 s median gap. Seed 18
 * needs single lines down to 0.46x in the skin's lane, like that song (0.456x).
 */
export function denseRapLyrics(seed: number): Lyrics {
  const rnd = prng(seed);
  const vocab = "a I am so we go up on it in the and you get got all now how big run said back down town like this that just make take every money crowd people always better nothing counting standing around another jumping through minute rhythm".split(" ");
  const pick = () => vocab[Math.floor(rnd() * vocab.length)];
  const lines: { beat: number; words: [string, number][] }[] = [];
  let t = 20; // seconds (60 BPM: one beat per second)
  const section = (n: number, lo: number, hi: number) => {
    for (let l = 0; l < n; l++) {
      const ws = Array.from({ length: [3, 5, 6, 7, 7, 7, 8, 8, 8, 9][Math.floor(rnd() * 10)] }, pick);
      const dur = ws.length / (lo + rnd() * (hi - lo));
      const units = ws.reduce((a, w) => a + w.length + 0.5, 0);
      lines.push({ beat: t, words: ws.map((w) => [w, (dur * (w.length + 0.5)) / units] as [string, number]) });
      t += dur + [0, 0.15, 0.3, 0.3, 0.45, 0.55][Math.floor(rnd() * 6)];
    }
  };
  section(16, 2.5, 5.7);
  t += 8;
  section(8, 2.2, 3.2);
  t += 4;
  section(16, 2.5, 5.7);
  t += 10;
  section(8, 2.2, 3.2);
  return lyricsFromScore(lines, { bpm: 60, offset: 0, seed });
}

/** Folder with the end-to-end run's song files, or null (MSYS paths like /c/... accepted). */
function e2eSongsDir(): string | null {
  const msys = (p: string) => p.replace(/^\/([a-zA-Z])\//, "$1:/");
  const candidates = [
    process.env.JK_TICKER_SONGS,
    process.env.JK_BUILD ? join(msys(process.env.JK_BUILD), "e2e-stick", "Library") : undefined,
    join(here, "..", "..", "..", "..", "..", "jukeoke-build", "e2e-stick", "Library"),
  ];
  for (const c of candidates) if (c && existsSync(msys(c))) return msys(c);
  return null;
}
const songsDir = e2eSongsDir();
const e2eSongs: { name: string; lyrics: Lyrics }[] = [];
if (songsDir) {
  for (const f of readdirSync(songsDir).filter((n) => n.endsWith(".jksong") && !n.startsWith(".")).sort()) {
    const { lyrics } = readJksong(join(songsDir, f));
    if (lyrics) e2eSongs.push({ name: `e2e: ${f.replace(/\.jksong$/, "")}`, lyrics });
  }
}

const fixtures: { name: string; lyrics: Lyrics }[] = [];
if (existsSync(fixturesDir)) {
  for (const f of readdirSync(fixturesDir).filter((n) => n.endsWith(".jksong")).sort()) {
    const { lyrics } = readJksong(join(fixturesDir, f));
    if (lyrics) fixtures.push({ name: `fixture: ${f}`, lyrics });
  }
}

const sets: { name: string; lyrics: Lyrics; natural: boolean }[] = [
  { name: "demo: Twinkle, Twinkle, Little Star", lyrics: TWINKLE_LYRICS, natural: true },
  { name: "demo: Frère Jacques", lyrics: FRERE_LYRICS, natural: true },
  { name: "demo: Row, Row, Row Your Boat (triplets)", lyrics: ROW_LYRICS, natural: true },
  { name: "synthetic stress", lyrics: stressLyrics(), natural: false },
  { name: "synthetic dense rap", lyrics: denseRapLyrics(18), natural: false },
  ...fixtures.map((f) => ({ ...f, natural: true })),
  ...e2eSongs.map((f) => ({ ...f, natural: false })),
];

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
    if (process.env.TICKER_REPORT) writeFileSync(process.env.TICKER_REPORT, JSON.stringify({ windows: rows, skinLane: laneRows }, null, 2));
    expect(rows.length).toBe(sets.length * WINDOWS.length);
  });
});

// ---------------------------------------------------------------------------
// The app's own geometry: the jukebox skin's lane and the lyrics face.
// ---------------------------------------------------------------------------
/**
 * Width of the ticker lane (CSS px) inside the jukebox skin with default settings,
 * measured on 2026-11-17 in headless Edge 157 on the UI with the mock (`.ticker`
 * bounding box on the Stage, skin on): 1220.3 px windowed and 1208.8 px full screen at
 * 1440x900, 1084.8 / 1074.5 px at 1280x800. The narrower full-screen lane is used,
 * rounded down. Re-measure if the skin's frame or the Stage padding changes.
 */
const SKIN_LANES = [
  { w: 1440, h: 900, lane: 1208 },
  { w: 1280, h: 800, lane: 1074 },
];
const laneMeasure = lyricsFaceMeasure(shimMeasure);
const laneRows: (Row & { minLinePx: number; lines: number })[] = [];
const laneSets: { name: string; lyrics: Lyrics }[] = [
  ...fixtures,
  { name: "demo: Twinkle, Twinkle, Little Star", lyrics: TWINKLE_LYRICS },
  { name: "synthetic dense rap", lyrics: denseRapLyrics(18) },
  ...e2eSongs,
];

describe("lyrics ticker in the skin's lane, lyrics face, default settings (WORK_ORDER 4.4: 2 s ahead)", () => {
  for (const s of laneSets) {
    for (const win of SKIN_LANES) {
      it(`${s.name} @ ${win.w}x${win.h} (lane ${win.lane} px)`, () => {
        const fontPx = TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
        const layout = layoutTicker(s.lyrics, { width: win.lane, marker: MARKER, fontPx }, laneMeasure);
        const r = verifyTickerLayout(layout, { dt: DT, lookahead: LOOKAHEAD_S, maxSecantRatio: 3 });
        const minLinePx = Math.min(...layout.lines.map((l) => l.px));
        laneRows.push({ ...r, set: s.name, window: `${win.w}x${win.h}`, minLinePx, lines: layout.lines.length });
        expect(r.firstProblems).toEqual([]);
        expect(r.wrongLetter).toBe(0);
        expect(r.backwards).toBe(0);
        expect(r.tooFast).toBe(0);
        expect(r.lookaheadMisses).toBe(0);
        expect(r.minLookaheadS).toBeGreaterThanOrEqual(LOOKAHEAD_S - 1e-9);
        expect(minLinePx).toBeGreaterThanOrEqual(MIN_LINE_SCALE * fontPx - 1e-9);
      });
    }
  }

  it.skipIf(songsDir === null)("end-to-end songs found and checked above (skipped without $JK_TICKER_SONGS or $JK_BUILD/e2e-stick/Library)", () => {
    expect(e2eSongs.length).toBeGreaterThan(0);
  });

  it("report", () => {
    const lines = laneRows.map(
      (r) =>
        `${r.set.padEnd(46)} ${r.window.padEnd(9)} font x${r.fontScale.toFixed(2)}, dense lines ${String(r.denseLines).padStart(2)}/${String(r.lines).padStart(3)} down to x${r.minLineScale.toFixed(3)} (${r.minLinePx.toFixed(1)} px)  max ${r.maxSpeedEm.toFixed(1)} em/s  min lookahead ${r.minLookaheadS === Infinity ? "n/a" : r.minLookaheadS.toFixed(2) + " s"}`,
    );
    console.log(`\nTicker in the skin's lane (lyrics face, marker ${MARKER}, text ${TEXT_SIZE}px @1440x900):\n${lines.join("\n")}\n`);
    expect(laneRows.length).toBe(laneSets.length * SKIN_LANES.length);
  });
});

describe("dense lines", () => {
  const fontPx = TEXT_SIZE;
  const rap = layoutTicker(denseRapLyrics(18), { width: 1208, marker: MARKER, fontPx }, laneMeasure);

  it("only the lines that do not fit are drawn smaller, each line at one size, never below the floor", () => {
    expect(rap.fontScale).toBeCloseTo(0.6, 9);
    expect(rap.denseLines).toBeGreaterThan(0);
    const songPx = rap.fontPx;
    expect(rap.lines.filter((l) => l.px < songPx - 1e-9).length).toBe(rap.denseLines);
    expect(rap.denseLines).toBeLessThan(rap.lines.length / 2);
    for (const g of rap.glyphs) expect(g.px).toBe(rap.lines[g.line].px);
    for (const l of rap.lines) {
      expect(l.px).toBeLessThanOrEqual(songPx + 1e-9);
      expect(l.px).toBeGreaterThanOrEqual(MIN_LINE_SCALE * fontPx - 1e-9);
    }
    expect(rap.minLineScale).toBeCloseTo(Math.min(...rap.lines.map((l) => l.px)) / fontPx, 9);
  });

  it("a song denser than the floor allows keeps the letter under the marker and degrades gracefully", () => {
    // Seed 15 sustains more text than fits 2 s ahead even at the floor.
    const l = layoutTicker(denseRapLyrics(15), { width: 1208, marker: MARKER, fontPx }, laneMeasure);
    const r = verifyTickerLayout(l);
    expect(l.minLineScale).toBeCloseTo(MIN_LINE_SCALE, 9);
    expect(r.wrongLetter).toBe(0);
    expect(r.backwards).toBe(0);
    expect(r.tooFast).toBe(0);
    expect(r.minLookaheadS).toBeGreaterThan(1.5);
    expect(r.minLookaheadS).toBeLessThan(LOOKAHEAD_S);
  });

  it("songs that fit keep one size", () => {
    const l = layoutTicker(TWINKLE_LYRICS, { width: 1208, marker: MARKER, fontPx }, laneMeasure);
    expect(l.denseLines).toBe(0);
    expect(l.minLineScale).toBe(l.fontScale);
    for (const line of l.lines) expect(line.px).toBe(l.fontPx);
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

// ---------------------------------------------------------------------------
// Word pace: smooth, paced by word (settings.ui.lyrics_motion = "word", D-216)
// ---------------------------------------------------------------------------
interface WordRow {
  set: string;
  window: string;
  word: TickerVerifyReport;
  smoothWord: FrameSmoothness;
  smoothLetter: FrameSmoothness;
}
const wordRows: WordRow[] = [];
const wordLaneRows: (TickerVerifyReport & { set: string; window: string; minLinePx: number })[] = [];

describe("lyrics ticker, word pace: smooth, paced by word (owner's request, D-216)", () => {
  for (const s of sets) {
    for (const win of WINDOWS) {
      it(`${s.name} @ ${win.w}x${win.h}`, () => {
        const fontPx = TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
        const params = { width: win.w, marker: MARKER, fontPx };
        const word = layoutTicker(s.lyrics, { ...params, pace: "word" }, shimMeasure);
        const letter = layoutTicker(s.lyrics, { ...params, pace: "letter" }, shimMeasure);
        const r = verifyTickerLayout(word, { dt: DT, lookahead: LOOKAHEAD_S });
        const sw = frameSmoothness(word);
        const sl = frameSmoothness(letter);
        wordRows.push({ set: s.name, window: `${win.w}x${win.h}`, word: r, smoothWord: sw, smoothLetter: sl });
        expect(word.pace).toBe("word");
        expect(r.pace).toBe("word");
        expect(r.firstProblems).toEqual([]);
        expect(r.sungSamples).toBeGreaterThan(0);
        // (a) The word under the marker: every word's first letter reaches the marker
        // within 40 ms of its start; no word's letters are at the marker early or late.
        expect(r.onsetMisses).toBe(0);
        expect(r.onsetMaxMs).toBeLessThanOrEqual(WORD_TOLERANCE_S * 1000 + 1e-3);
        expect(r.wrongLetter).toBe(0);
        // (b) Never backwards, never stopped while a line is sung.
        expect(r.backwards).toBe(0);
        expect(r.stopped).toBe(0);
        expect(r.minSungSpeedEm).toBeGreaterThanOrEqual(MIN_SUNG_SPEED_EM);
        // (c) Smoothness at 60 fps, against the letter pace on the same song and size.
        expect(sw.frames).toBe(sl.frames);
        expect(sw.bigStepShare).toBeLessThanOrEqual(0.01);
        expect(sw.bigStepShare).toBeLessThanOrEqual(sl.bigStepShare / 10);
        expect(sw.maxDvEm).toBeLessThan(sl.maxDvEm / 3);
        expect(sw.p99DvEm).toBeLessThan(sl.p99DvEm / 5);
        // (d) Lookahead: 2 s of upcoming text, with the same font rules.
        expect(r.lookaheadMisses).toBe(0);
        expect(r.minLookaheadS).toBeGreaterThanOrEqual(LOOKAHEAD_S - 1e-9);
        if (s.natural) expect(word.fontScale).toBe(1);
      });
    }
  }

  it("report", () => {
    const pct = (x: number) => `${(x * 100).toFixed(2)}%`.padStart(7);
    const lines = wordRows.map(
      (r) =>
        `${r.set.padEnd(46)} ${r.window.padEnd(10)} onset <= ${r.word.onsetMaxMs!.toFixed(1).padStart(4)} ms  on word ${(r.word.onWord! * 100).toFixed(1)}%  min sung speed ${r.word.minSungSpeedEm!.toFixed(2)} em/s  font x${r.word.fontScale.toFixed(2)}  lookahead ${r.word.minLookaheadS === Infinity ? "n/a " : r.word.minLookaheadS.toFixed(2) + " s"}  | frame-to-frame speed change, word vs letter: max ${r.smoothWord.maxDvEm.toFixed(2)} vs ${r.smoothLetter.maxDvEm.toFixed(2)} em/s, p99 ${r.smoothWord.p99DvEm.toFixed(2)} vs ${r.smoothLetter.p99DvEm.toFixed(2)} em/s, >25 % ${pct(r.smoothWord.bigStepShare)} vs ${pct(r.smoothLetter.bigStepShare)} of ${r.smoothWord.frames} frames`,
    );
    console.log(`\nTicker, word pace (${DT * 1000} ms steps, frames at 60 fps, marker ${MARKER}, text ${TEXT_SIZE}px @1440x900):\n${lines.join("\n")}\n`);
    expect(wordRows.length).toBe(sets.length * WINDOWS.length);
  });
});

describe("word pace in the skin's lane, lyrics face, default settings", () => {
  for (const s of laneSets) {
    for (const win of SKIN_LANES) {
      it(`${s.name} @ ${win.w}x${win.h} (lane ${win.lane} px)`, () => {
        const fontPx = TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
        const layout = layoutTicker(s.lyrics, { width: win.lane, marker: MARKER, fontPx, pace: "word" }, laneMeasure);
        const r = verifyTickerLayout(layout, { dt: DT, lookahead: LOOKAHEAD_S });
        const minLinePx = Math.min(...layout.lines.map((l) => l.px));
        wordLaneRows.push({ ...r, set: s.name, window: `${win.w}x${win.h}`, minLinePx });
        expect(r.firstProblems).toEqual([]);
        expect(r.onsetMisses).toBe(0);
        expect(r.wrongLetter).toBe(0);
        expect(r.backwards).toBe(0);
        expect(r.stopped).toBe(0);
        expect(r.lookaheadMisses).toBe(0);
        expect(r.minLookaheadS).toBeGreaterThanOrEqual(LOOKAHEAD_S - 1e-9);
        expect(minLinePx).toBeGreaterThanOrEqual(MIN_LINE_SCALE * fontPx - 1e-9);
      });
    }
  }

  it("report", () => {
    const lines = wordLaneRows.map(
      (r) =>
        `${r.set.padEnd(46)} ${r.window.padEnd(9)} font x${r.fontScale.toFixed(2)}, dense lines ${String(r.denseLines).padStart(2)} down to x${r.minLineScale.toFixed(3)} (${r.minLinePx.toFixed(1)} px)  onset <= ${r.onsetMaxMs!.toFixed(1)} ms  min lookahead ${r.minLookaheadS === Infinity ? "n/a" : r.minLookaheadS.toFixed(2) + " s"}`,
    );
    console.log(`\nTicker, word pace, in the skin's lane (lyrics face, marker ${MARKER}, text ${TEXT_SIZE}px @1440x900):\n${lines.join("\n")}\n`);
    // Completes the report file of the first block (whose report runs before the lane
    // blocks have filled their rows).
    if (process.env.TICKER_REPORT && existsSync(process.env.TICKER_REPORT)) {
      const prev = JSON.parse(readFileSync(process.env.TICKER_REPORT, "utf8")) as Record<string, unknown>;
      writeFileSync(process.env.TICKER_REPORT, JSON.stringify({ ...prev, skinLane: laneRows, wordPace: wordRows, wordPaceSkinLane: wordLaneRows }, null, 2));
    }
    expect(wordLaneRows.length).toBe(laneSets.length * SKIN_LANES.length);
  });
});

/** One line of words; each word's letters share its time evenly unless `chars` times are given. */
function oneLine(words: { text: string; start: number; end: number; chars?: [number, number][] }[]): Lyrics {
  const ws = words.map((w) => {
    const letters = [...w.text];
    const d = (w.end - w.start) / letters.length;
    return { start: w.start, end: w.end, chars: letters.map((c, k) => [c, w.chars?.[k][0] ?? w.start + k * d, w.chars?.[k][1] ?? w.start + (k + 1) * d] as [string, number, number]) };
  });
  return { lyrics_version: 1, language: "en", source: "user_text", checked: true, lines: [{ start: ws[0].start, end: ws[ws.length - 1].end, words: ws }] };
}
/** Strip distance per 60 fps frame times 60, at t. */
const frameSpeed = (l: TickerLayout, t: number) => (l.motion.at(t + 1 / 60) - l.motion.at(t)) * 60;

describe("word pace details", () => {
  const params = { width: 1440, marker: 0.33, fontPx: 56 };

  it("is the app's default; the layout alone stays letter-exact unless asked", () => {
    expect(DEFAULT_APP_SETTINGS.ui.lyrics_motion).toBe("word");
    expect(layoutTicker(TWINKLE_LYRICS, params, shimMeasure).pace).toBe("letter");
    expect(layoutTicker(TWINKLE_LYRICS, { ...params, pace: "word" }, shimMeasure).pace).toBe("word");
  });

  it("a held word passes the marker slowly, quick words pass it faster", () => {
    const lyr = oneLine([
      { text: "we", start: 1.0, end: 1.2 },
      { text: "go", start: 1.2, end: 1.4 },
      { text: "so", start: 1.4, end: 3.4 },
      { text: "far", start: 3.4, end: 3.6 },
      { text: "away", start: 3.6, end: 3.9 },
    ]);
    const l = layoutTicker(lyr, { ...params, pace: "word" }, shimMeasure);
    const so = l.words[2];
    const held = frameSpeed(l, 2.4);
    const quick = frameSpeed(l, 1.3);
    // The held word moves at about its own pace (its letters and the space it carries, over its 2 s).
    const ownPace = (l.words[3].a - so.a) / (so.end - so.start);
    expect(held).toBeGreaterThan(0.5 * ownPace);
    expect(held).toBeLessThan(1.5 * ownPace);
    expect(quick).toBeGreaterThan(4 * held);
    // While "so" is held the marker stays on it or on the space after it.
    for (let t = so.start + WORD_TOLERANCE_S; t < so.end - WORD_TOLERANCE_S; t += 0.01) {
      const x = l.motion.at(t);
      expect(x).toBeGreaterThanOrEqual(so.a - 1e-6);
      expect(x).toBeLessThan(l.words[3].a);
    }
  });

  it("does not dash across the space between two held words sung back to back (the letter pace does)", () => {
    // "oh" held, its "h" short; "no" with a short "n", then held.
    const lyr = oneLine([
      { text: "oh", start: 1, end: 3, chars: [[1, 2.9], [2.9, 3]] },
      { text: "no", start: 3, end: 5, chars: [[3, 3.1], [3.1, 5]] },
    ]);
    const word = layoutTicker(lyr, { ...params, pace: "word" }, shimMeasure);
    const letter = layoutTicker(lyr, { ...params, pace: "letter" }, shimMeasure);
    const avg = (word.words[1].a - word.words[0].a) / 2; // "oh" and its space over 2 s
    let maxWord = 0;
    let maxLetter = 0;
    for (let t = 2.5; t < 3.5; t += 1 / 60) {
      maxWord = Math.max(maxWord, frameSpeed(word, t));
      maxLetter = Math.max(maxLetter, frameSpeed(letter, t));
    }
    expect(maxWord).toBeLessThan(2 * avg);
    expect(maxLetter).toBeGreaterThan(4 * avg);
  });

  it("keeps gliding through a short pause, slows down in a long one, and never stops while the line is sung", () => {
    const lyr = oneLine([
      { text: "one", start: 1, end: 1.3 },
      { text: "two", start: 1.45, end: 1.75 },
      { text: "three", start: 3.25, end: 3.6 },
    ]);
    const l = layoutTicker(lyr, { ...params, pace: "word" }, shimMeasure);
    const inOne = frameSpeed(l, 1.15);
    const inTwo = frameSpeed(l, 1.6);
    for (let t = 1.3; t < 1.45; t += 0.01) expect(frameSpeed(l, t)).toBeGreaterThan(0.5 * Math.min(inOne, inTwo));
    expect(frameSpeed(l, 2.6)).toBeLessThan(0.25 * inTwo);
    for (let t = 1; t < 3.6; t += 0.01) expect(frameSpeed(l, t)).toBeGreaterThan(MIN_SUNG_SPEED_EM * params.fontPx);
    expect(verifyTickerLayout(l).firstProblems).toEqual([]);
  });

  it("keeps the instrumental breaks: countdown, next line brought in from the right in time", () => {
    const l = layoutTicker(TWINKLE_LYRICS, { ...params, pace: "word" }, shimMeasure);
    const breaks = l.rests.filter((r) => r.countdown);
    expect(breaks.length).toBeGreaterThanOrEqual(2);
    const rightW = l.width - l.markerX;
    for (const r of breaks) {
      const mid = (r.from + r.to) / 2;
      expect(restAt(l, mid)?.remaining).toBeCloseTo(r.to - mid, 9);
      const line = l.lines[r.nextLine];
      expect(line.x0 - l.motion.at(r.to - 3)).toBeLessThanOrEqual(rightW + 1e-6);
      expect(line.x0 - l.motion.at(r.to - 2)).toBeLessThan(rightW);
      if (r.to - r.from > 8) {
        expect(line.x0 - l.motion.at(r.to - 6)).toBeGreaterThanOrEqual(rightW - 1e-6);
        // During the rest itself the strip stands still while the countdown runs.
        expect(l.motion.at(mid + 0.5)).toBeCloseTo(l.motion.at(mid), 9);
      }
    }
  });

  it("handles lines without words and lyrics without any timed letter", () => {
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
    const l = layoutTicker(lyr, { ...params, pace: "word" }, shimMeasure);
    expect(verifyTickerLayout(l).firstProblems).toEqual([]);
    expect(l.glyphs[l.lines[restAt(l, 10)!.rest.nextLine].first].g).toBe("w");
    const untimed: Lyrics = { ...lyr, lines: [{ start: 1, end: 1, words: [{ start: 1, end: 1, chars: [["a", 1, 1]] }] }] };
    expect(() => layoutTicker(untimed, { ...params, pace: "word" }, shimMeasure).motion.at(1)).not.toThrow();
  });

  it("after a long break, zero-length words at the line ends do not bring the next line in early", () => {
    // An aligner can leave a word with start == end away from its neighbours: here one
    // 1 s after the last sung word and one 1.5 s before the next line's first sung word.
    const word = (s: string, t0: number, d = 0.2) => ({ start: t0, end: t0 + d * s.length, chars: [...s].map((c, k) => [c, t0 + d * k, t0 + d * (k + 1)] as [string, number, number]) });
    const lyr: Lyrics = {
      lyrics_version: 1,
      language: "en",
      source: "user_text",
      checked: true,
      lines: [
        { start: 1, end: 3, words: [word("hello", 1), word("oh", 3, 0)] },
        { start: 28.5, end: 31, words: [word("ah", 28.5, 0), word("world", 30)] },
      ],
    };
    const l = layoutTicker(lyr, { ...params, pace: "word" }, shimMeasure);
    const r = verifyTickerLayout(l);
    expect(r.firstProblems).toEqual([]);
    expect(r.onsetMaxMs).toBeLessThanOrEqual(WORD_TOLERANCE_S * 1000 + 1e-3);
    // The next line is at the marker as "ah" (no length) is due, then glides on to "world".
    const world = l.words[1];
    expect(l.motion.at(28.5)).toBeCloseTo(l.lines[1].x0, 6);
    expect(l.motion.at(29.5)).toBeLessThan(world.a - 10);
    expect(l.motion.at(29.5)).toBeGreaterThan(l.lines[1].x0 + 10);
    // The old line glides over "oh" (no length, 1 s after "hello") before it leaves.
    expect(l.motion.at(2.5)).toBeLessThan(l.lines[0].x1);
    expect(l.motion.at(2.5)).toBeGreaterThan(l.words[0].b);
  });

  it("RampMotion stays exact when sagging plateaus need many passes (no jump at a knot)", () => {
    for (let seed = 1; seed <= 3000; seed++) {
      const rnd = prng(seed);
      const knots = [{ t: 0, x: 0 }];
      const n = 3 + Math.floor(rnd() * 40);
      for (let i = 1; i < n; i++) {
        const r = rnd();
        const h = r < 0.3 ? 0.002 + rnd() * 0.02 : r < 0.6 ? 0.05 + rnd() * 0.3 : r < 0.9 ? 0.5 + rnd() * 3 : 10 + rnd() * 50;
        const dx = rnd() < 0.08 ? 0 : rnd() < 0.3 ? 0.01 + rnd() : rnd() < 0.5 ? 20 + rnd() * 60 : 200 + rnd() * 2000;
        knots.push({ t: knots[i - 1].t + h, x: knots[i - 1].x + dx });
      }
      const m = new RampMotion(knots);
      for (let i = 0; i + 1 < n; i++) {
        const [a, b] = [m.t[i], m.t[i + 1]];
        const cuts = [a, a + m.rb[i], b - m.ra[i + 1], b];
        let integral = 0;
        for (let q = 0; q < 3; q++) {
          const [t0, t1] = [cuts[q], cuts[q + 1]];
          if (t1 > t0) integral += ((t1 - t0) / 6) * (m.speed(t0 + 1e-12) + 4 * m.speed((t0 + t1) / 2) + m.speed(t1 - 1e-12));
        }
        const dx = m.x[i + 1] - m.x[i];
        expect(Math.abs(integral - dx), `seed ${seed}, interval ${i}`).toBeLessThan(1e-6 * (1 + dx));
        if (dx > 0) expect(m.c[i]).toBeGreaterThanOrEqual(0.5 * (dx / (b - a)) - 1e-9);
      }
    }
  });

  it("RampMotion passes every knot exactly, never runs backwards, and its speed and acceleration are continuous", () => {
    const rnd = prng(5);
    const knots = [{ t: 0, x: 0 }];
    for (let i = 1; i < 400; i++) {
      const rest = rnd() < 0.05;
      knots.push({ t: knots[i - 1].t + 0.01 + rnd() * (rnd() < 0.2 ? 2 : 0.4), x: knots[i - 1].x + (rest ? 0 : 1 + rnd() * (rnd() < 0.2 ? 400 : 60)) });
    }
    const m = new RampMotion(knots);
    for (const k of knots) expect(m.at(k.t)).toBeCloseTo(k.x, 6);
    for (let i = 0; i + 1 < knots.length; i++) {
      const a = knots[i];
      const b = knots[i + 1];
      const h = b.t - a.t;
      let prev = -Infinity;
      for (let k = 0; k <= 50; k++) {
        const t = a.t + (k / 50) * h;
        const x = m.at(t);
        expect(x).toBeGreaterThanOrEqual(prev - 1e-9);
        prev = x;
        expect(m.speed(t)).toBeGreaterThanOrEqual(0);
        // A rest interval stands still.
        if (b.x === a.x && k < 50) expect(m.speed(t)).toBe(0);
      }
      // The closed-form position agrees with the integral of the speed. The speed is a
      // cubic on each piece (ramp out of knot i, plateau, ramp into knot i+1), where
      // Simpson's rule is exact.
      const cuts = [a.t, a.t + m.rb[i], b.t - m.ra[i + 1], b.t];
      let integral = 0;
      for (let q = 0; q < 3; q++) {
        const [t0, t1] = [cuts[q], cuts[q + 1]];
        if (t1 > t0) integral += ((t1 - t0) / 6) * (m.speed(t0 + 1e-12) + 4 * m.speed((t0 + t1) / 2) + m.speed(t1 - 1e-12));
      }
      expect(Math.abs(integral - (b.x - a.x))).toBeLessThan(1e-6 * (1 + b.x - a.x));
      expect(m.at(a.t + h / 2)).toBeCloseTo(a.x + (() => {
        const half = a.t + h / 2;
        let acc = 0;
        for (let q = 0; q < 3; q++) {
          const t0 = cuts[q];
          const t1 = Math.min(cuts[q + 1], half);
          if (t1 > t0) acc += ((t1 - t0) / 6) * (m.speed(t0 + 1e-12) + 4 * m.speed((t0 + t1) / 2) + m.speed(t1 - 1e-12));
        }
        return acc;
      })(), 6);
      // Speed and acceleration agree on both sides of the knot (probed at a scale far
      // below the ramp around it, which the plateau guard may have shortened).
      if (i + 2 < knots.length) {
        const L = m.ra[i + 1] + m.rb[i + 1];
        const e = L > 0 ? L * 1e-5 : 1e-6;
        const v0 = m.speed(b.t - e);
        const v1 = m.speed(b.t + e);
        expect(Math.abs(v0 - v1)).toBeLessThan(1e-3 * (1 + v0));
        const a0 = (m.speed(b.t - e) - m.speed(b.t - 2 * e)) / e;
        const a1 = (m.speed(b.t + 2 * e) - m.speed(b.t + e)) / e;
        const scale = L > 0 ? Math.abs(m.c[i + 1] - m.c[i]) / L : 0;
        expect(Math.abs(a0 - a1)).toBeLessThan(1e-3 * scale + 1e-3 * (1 + Math.abs(a0)));
      }
    }
  });

  it("straightening moves a knot only within its window, and only where the speeds on its two sides differ by more than 2x", () => {
    const knots = [
      { t: 0, x: 0, lo: 0, hi: 0 },
      { t: 1, x: 100, lo: 0.96, hi: 1.04 }, // 100 px/s, then 120 px/s: left alone
      { t: 1.5, x: 160, lo: 1.46, hi: 1.54 }, // then 600 px/s over 50 ms: moved apart
      { t: 1.55, x: 190, lo: 1.51, hi: 1.59 },
      { t: 3, x: 300, lo: 3, hi: 3 },
    ];
    const out = straightenKnots(knots);
    expect(out[0].t).toBe(0);
    expect(out[1].t).toBe(1);
    expect(out[4].t).toBe(3);
    for (let j = 0; j < knots.length; j++) {
      expect(out[j].x).toBe(knots[j].x);
      expect(out[j].t).toBeGreaterThanOrEqual(knots[j].lo - 1e-12);
      expect(out[j].t).toBeLessThanOrEqual(knots[j].hi + 1e-12);
    }
    expect(out[3].t - out[2].t).toBeGreaterThan(0.05);
  });
});

describe("backing vocals (words in parentheses, D-282)", () => {
  // Owner 2026-11-27: "words in parentheses show as words that you're supposed to sing".
  const words = [
    { text: "(Oh", start: 1, end: 1.4 },
    { text: "my)", start: 1.5, end: 1.9 },
    { text: "we", start: 2.2, end: 2.5 },
    { text: "go", start: 2.6, end: 3.2 },
  ];

  it("flags the glyphs of words in parentheses and leaves the layout as it was", () => {
    const withParens = layoutTicker(oneLine(words), { width: 1000, marker: 0.3, fontPx: 40 }, shimMeasure);
    expect(withParens.glyphs.filter((g) => g.backing).map((g) => g.g).join("")).toBe("(Ohmy)");
    expect(withParens.glyphs.filter((g) => !g.backing).map((g) => g.g).join("")).toBe("wego");
    const plain = layoutTicker(oneLine(words.map((w) => ({ ...w, text: w.text.replace(/[()]/g, "") }))), { width: 1000, marker: 0.3, fontPx: 40 }, shimMeasure);
    expect(plain.glyphs.every((g) => !g.backing)).toBe(true);
  });

  it("draws them dimmed in the upcoming colour in the sung tiles too", () => {
    const log: { text: string; fill: string; alpha: number }[] = [];
    const recording = () => {
      const state: Record<string, unknown> = { fillStyle: "#000", globalAlpha: 1 };
      const target: Record<string, unknown> = {
        measureText: () => ({ width: 10, fontBoundingBoxAscent: 30, fontBoundingBoxDescent: 8 }),
        fillText: (t: string) => log.push({ text: t, fill: String(state.fillStyle), alpha: Number(state.globalAlpha) }),
      };
      const ctx = new Proxy(target, {
        get: (t, k: string) => (k in state ? state[k] : k in t ? t[k] : () => undefined),
        set: (t, k: string, v) => {
          if (k in state) state[k] = v;
          else t[k] = v;
          return true;
        },
      });
      return { width: 0, height: 0, getContext: () => ctx } as unknown as HTMLCanvasElement;
    };
    const r = new TickerRenderer(recording(), recording);
    r.setSize(1000, 80, 1);
    r.setLayout(layoutTicker(oneLine(words), { width: 1000, marker: 0.3, fontPx: 40 }, shimMeasure), { fontFamily: "sans-serif", fontWeight: "700", sung: "#ffc21a", upcoming: "#f4f4f4", marker: "#7dffd8" });
    for (let t = 0; t <= 4; t += 0.1) r.draw(t);
    const of = (chars: string) => log.filter((e) => chars.includes(e.text));
    expect(of("(Ohmy)").length).toBeGreaterThan(0);
    expect(of("(Ohmy)").every((e) => e.fill === "#f4f4f4" && e.alpha === 0.5)).toBe(true);
    expect(of("wego").some((e) => e.fill === "#ffc21a" && e.alpha === 1)).toBe(true);
    expect(of("wego").every((e) => e.alpha === 1)).toBe(true);
  });
});

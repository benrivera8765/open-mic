// SPDX-License-Identifier: GPL-3.0-or-later
// The page display (static lines with a colour wipe, D-218; the owner's request of
// 18 Nov 2026), checked across whole songs the way ticker.test.ts checks the ticker.
//
// Songs: the whole demo song (tests/fixtures/generated/demo-song.jksong, the song the app
// ships) and the tiny fixture, the mock's three demo songs, a generated rap with the
// end-to-end rap song's density, a synthetic worst case (zero gaps, zero-length letters,
// an 80 s break), and the three end-to-end songs (read at test time from
// $JK_BUILD/e2e-stick/Library, skipped when absent; their lyrics never enter the
// repository). Geometry: the jukebox skin's lyrics area at 1280x800, 1440x900, 2560x1440
// and 3840x2160 (the narrowest width, full screen, and the shortest height, windowed with
// the "unchecked lyrics" note, measured in headless Edge 157 on 18 Nov 2026), the default
// text size, and the advance widths of the lyrics face at weight 800 (the page display's
// weight in the skin). For every song and size (pageverify.ts):
//   (a) the wipe edge is on the word being sung (D-216 tolerance: first letter within
//       40 ms of the word's start; while it is sung, on it or the space it carries);
//   (b) never backwards within a line, never stopped while the line is sung;
//   (c) every line fully shown (current or next) >= 2.0 s before its first word, unless
//       the line above it is current later than that (reported, with the reason);
//   (d) nothing outside the area, on frames across the whole song (fit, shrink or wrap);
//   (e) a lead-in bar only after a pause, draining monotonically, gone at the first word;
//   (f) line changes start within 250 ms of the previous line's last word.
// (g) is ticker.test.ts itself, which this change does not touch.
import { writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { DEFAULT_APP_SETTINGS } from "../api/defaults";
import { FRERE_LYRICS, prng, ROW_LYRICS, TWINKLE_LYRICS } from "../api/demo";
import { LOOKAHEAD_S } from "./layout";
import { lyricsFaceMeasure } from "./lyrics-face-widths";
import {
  BAR_GAP_EM,
  BAR_W_EM,
  ENTRY_S,
  LEADIN_MAX_S,
  LINE_HEIGHT_EM,
  layoutPage,
  PAGE_MIN_SCALE,
  type PageLayout,
  pageFrame,
  PREVIEW_ALPHA,
  PREVIEW_SCALE,
  sidePad,
  TRANSITION_S,
} from "./page";
import { lineAsTicker, type PageVerifyReport, SWITCH_MAX_S, verifyPage } from "./pageverify";
import { frameSmoothness } from "./verify";
import { WORD_TOLERANCE_S } from "./wordmotion";
import { denseRapLyrics, e2eSongs, e2eSongsDir, fixtureSongs, shimMeasure, stressLyrics } from "./testsongs-node";

const measure = lyricsFaceMeasure(shimMeasure, 800);
const TEXT_SIZE = DEFAULT_APP_SETTINGS.ui.lyrics_text_size;

/** The skin's lyrics area (CSS px): narrowest width (full screen) x shortest height (windowed, with the unchecked-lyrics note). */
const LANES = [
  { w: 1280, h: 800, lane: { w: 1074, h: 397 } },
  { w: 1440, h: 900, lane: { w: 1208, h: 450 } },
  { w: 2560, h: 1440, lane: { w: 2150, h: 732 } },
  { w: 3840, h: 2160, lane: { w: 3225, h: 1109 } },
];

const songsDir = e2eSongsDir();
const e2e = e2eSongs(songsDir);
const sets: { name: string; lyrics: Lyrics }[] = [
  ...fixtureSongs(),
  { name: "demo: Twinkle, Twinkle, Little Star", lyrics: TWINKLE_LYRICS },
  { name: "demo: Frère Jacques", lyrics: FRERE_LYRICS },
  { name: "demo: Row, Row, Row Your Boat (triplets)", lyrics: ROW_LYRICS },
  { name: "synthetic dense rap", lyrics: denseRapLyrics(18) },
  { name: "synthetic stress", lyrics: stressLyrics() },
  ...e2e,
];

/** Line text quoted in the verify messages replaced by [text]: the end-to-end songs' lyrics never reach the console or a report file. */
const redact = (s: string) => s.replace(/"[^"]*"/g, "[text]");
const redactReport = (r: PageVerifyReport) => ({ ...r, firstProblems: r.firstProblems.map(redact), leadExceptions: r.leadExceptions.map((e) => ({ ...e, reason: redact(e.reason) })) });
const fontAt = (win: { w: number; h: number }) => TEXT_SIZE * Math.min(win.w / 1440, win.h / 900);
const layoutFor = (lyrics: Lyrics, win: (typeof LANES)[number], pace: "word" | "letter" = "word", showNext = true) =>
  layoutPage(lyrics, { width: win.lane.w, height: win.lane.h, fontPx: fontAt(win), pace, showNext }, measure);

interface Row {
  set: string;
  window: string;
  r: PageVerifyReport;
  layout: PageLayout;
  letter?: PageVerifyReport;
}
const rows: Row[] = [];

describe("page display: whole songs in the skin's lyrics area (owner's request, D-218)", () => {
  for (const s of sets) {
    for (const win of LANES) {
      it(`${s.name} @ ${win.w}x${win.h} (area ${win.lane.w}x${win.lane.h})`, () => {
        const layout = layoutFor(s.lyrics, win);
        const r = verifyPage(layout);
        const row: Row = { set: s.name, window: `${win.w}x${win.h}`, r, layout };
        rows.push(row);
        expect(r.firstProblems).toEqual([]);
        expect(r.sungSamples).toBeGreaterThan(0);
        // (a)
        expect(r.onsetMisses).toBe(0);
        expect(r.onsetMaxMs).toBeLessThanOrEqual(WORD_TOLERANCE_S * 1000 + 1e-3);
        expect(r.wrongLetter).toBe(0);
        // (b)
        expect(r.backwards).toBe(0);
        expect(r.stopped).toBe(0);
        // (c)
        expect(r.lookaheadMisses).toBe(0);
        expect(r.hiddenFrames).toBe(0);
        for (const ex of r.leadExceptions) expect(layout.lines[ex.line].previewFrom).toBe(layout.lines[ex.line - 1].current);
        // (d)
        expect(r.overflowFrames).toBe(0);
        const pad = sidePad(layout.width, layout.fontPx);
        for (const L of layout.lines) {
          for (const [k, rw] of L.rows.entries()) {
            expect(rw.left - (k === 0 ? (BAR_W_EM + BAR_GAP_EM) * L.px : 0)).toBeGreaterThanOrEqual(pad - 1e-6);
            expect(rw.left + (rw.u1 - rw.u0)).toBeLessThanOrEqual(layout.width - pad + 1e-6);
          }
          // Below the floor only when one word alone is too wide.
          if (L.px < PAGE_MIN_SCALE * layout.fontPx - 1e-6) expect(L.rows.length === 3 || L.words.length === 1).toBe(true);
        }
        // (e), (f)
        expect(r.leadInProblems).toBe(0);
        expect(r.switchProblems).toBe(0);
        expect(r.maxSwitchMs).toBeLessThanOrEqual(SWITCH_MAX_S * 1000 + 1e-6);
        // The letter pace keeps its own contract (letter in its cell, never backwards).
        if (win.w === 1440) {
          const lr = verifyPage(layoutFor(s.lyrics, win, "letter"));
          row.letter = lr;
          expect(lr.wrongLetter).toBe(0);
          expect(lr.backwards).toBe(0);
          expect(lr.overflowFrames).toBe(0);
          expect(lr.lookaheadMisses).toBe(0);
        }
      });
    }
  }

  it.skipIf(songsDir === null)("end-to-end songs found and checked above (skipped without $JK_TICKER_SONGS or $JK_BUILD/e2e-stick/Library)", () => {
    expect(e2e.length).toBeGreaterThan(0);
  });

  it("report", () => {
    const pct = (x: number) => `${(x * 100).toFixed(2)}%`;
    const lines = rows.map((x) => {
      const r = x.r;
      const ex = r.leadExceptions.length ? `${r.leadExceptions.length} under ${LOOKAHEAD_S} s (min ${r.minLeadS.toFixed(2)} s)` : "none";
      const l = x.letter ? ` | letter pace: wrong letters ${x.letter.wrongLetter}, >25 % ${pct(x.letter.smooth.bigStepShare)}, max change ${x.letter.smooth.maxDvEm.toFixed(2)} em/s` : "";
      return `${x.set.padEnd(46)} ${x.window.padEnd(10)} lines ${String(r.lines).padStart(3)}  font ${x.layout.fontPx.toFixed(1)} px x${x.layout.fontScale.toFixed(2)}, shrunk ${r.shrunkLines}, wrapped ${r.wrappedLines}, smallest x${x.layout.minLineScale.toFixed(2)}  onset <= ${r.onsetMaxMs!.toFixed(1)} ms  on word ${pct(r.onWord!)}  min speed ${r.minSungSpeedEm!.toFixed(2)} em/s  lead min ${r.minLeadS === Infinity ? "n/a" : r.minLeadS.toFixed(2) + " s"}, exceptions ${ex}  lead-ins ${r.leadIns}  switch <= ${r.maxSwitchMs.toFixed(0)} ms  frames ${r.frames}, overflow ${r.overflowFrames}  wipe: >25 % ${pct(r.smooth.bigStepShare)} of ${r.smooth.frames}, max change ${r.smooth.maxDvEm.toFixed(2)} em/s${l}`;
    });
    const exceptions = rows
      .filter((x) => x.window === "1440x900")
      .flatMap((x) => x.r.leadExceptions.map((e) => `  ${x.set}: line ${e.line + 1} shown ${e.leadS.toFixed(2)} s ahead: ${redact(e.reason)}`));
    console.log(`\nPage display (text ${TEXT_SIZE}px @1440x900, lyrics face 800, word pace):\n${lines.join("\n")}\n\nLines shown less than ${LOOKAHEAD_S} s ahead at 1440x900 (all previewed as early as a two-line page allows):\n${exceptions.join("\n") || "  none"}\n`);
    if (process.env.PAGE_REPORT)
      writeFileSync(
        process.env.PAGE_REPORT,
        JSON.stringify(
          rows.map((x) => ({ set: x.set, window: x.window, fontPx: x.layout.fontPx, minLineScale: x.layout.minLineScale, report: redactReport(x.r), letter: x.letter ? redactReport(x.letter) : null })),
          null,
          1,
        ),
      );
    expect(rows.length).toBe(sets.length * LANES.length);
  });
});

/** One line of words; letters share each word's time evenly. */
function lineOf(words: [string, number, number][]): Lyrics["lines"][number] {
  const ws = words.map(([text, start, end]) => {
    const letters = [...text];
    const d = (end - start) / letters.length;
    return { start, end, chars: letters.map((c, k) => [c, start + k * d, start + (k + 1) * d] as [string, number, number]) };
  });
  return { start: ws[0].start, end: ws[ws.length - 1].end, words: ws };
}
const song = (...lines: Lyrics["lines"]): Lyrics => ({ lyrics_version: 1, language: "en", source: "user_text", checked: true, lines });
/** Words of `text` sung back to back from t0, `dur` seconds each. */
const said = (text: string, t0: number, dur = 0.3): Lyrics["lines"][number] => lineOf(text.split(" ").map((w, k) => [w, t0 + k * dur, t0 + (k + 1) * dur]));
const AREA = { width: 1208, height: 450, fontPx: 56 };

describe("page display details", () => {
  it("is the app's default, with gold sung and white upcoming colours", () => {
    expect(DEFAULT_APP_SETTINGS.ui.lyrics_display).toBe("page");
    expect(DEFAULT_APP_SETTINGS.ui.show_next_line).toBe(true);
    expect(DEFAULT_APP_SETTINGS.ui.colors.sung).toBe("#ffc21a");
    expect(DEFAULT_APP_SETTINGS.ui.colors.upcoming).toBe("#f4f4f4");
  });

  it("keeps the line still while the wipe crosses it word by word, in reading order", () => {
    const l = layoutPage(song(said("and anybody could see", 2, 0.5), said("the rest of it", 5, 0.4)), AREA, measure);
    const L = l.lines[0];
    expect(L.rows.length).toBe(1);
    let prev = -Infinity;
    for (let t = L.start; t < L.end; t += 1 / 60) {
      const f = pageFrame(l, t);
      const it = f.items.find((x) => x.line === 0)!;
      if (t >= L.current + TRANSITION_S) {
        expect(it.top).toBe(L.currentTop);
        expect(it.scale).toBe(1);
      }
      expect(it.wipe).toBeGreaterThanOrEqual(prev);
      prev = it.wipe;
    }
    // "could" starts at 3.0 s: the wipe is at its first letter then, and inside it a moment later.
    const could = L.words[2];
    expect(Math.abs(L.motion.at(3.0) - could.a)).toBeLessThan(1e-6);
    expect(L.motion.at(3.2)).toBeGreaterThan(could.a);
    expect(L.motion.at(3.2)).toBeLessThan(could.b);
  });

  it("shrinks a long line, then wraps a longer one onto two rows wiped in reading order", () => {
    const medium = "this line is a bit too long to fit at full size";
    const long = "this line is much longer than the screen is wide even at the smallest size so it wraps";
    const l = layoutPage(song(said(medium, 2, 0.3), said(long, 30, 0.3)), AREA, measure);
    const [a, b] = l.lines;
    expect(a.rows.length).toBe(1);
    expect(a.px).toBeLessThan(l.fontPx);
    expect(a.px).toBeGreaterThanOrEqual(PAGE_MIN_SCALE * l.fontPx - 1e-6);
    expect(b.rows.length).toBe(2);
    expect(b.wrapped).toBe(true);
    // Balanced rows; the second row starts sung only once the first is complete.
    const [r0, r1] = b.rows;
    expect(Math.abs(r0.u1 - r0.u0 - (r1.u1 - r1.u0))).toBeLessThan(0.35 * Math.max(r0.u1 - r0.u0, r1.u1 - r1.u0));
    for (let t = b.start; t < b.end; t += 0.05) {
      const u = b.motion.at(t);
      if (u > r1.u0) expect(u).toBeGreaterThanOrEqual(r0.u1);
    }
    // Both rows inside the area, and the current line with the preview above it fits vertically.
    const r = verifyPage(l);
    expect(r.overflowFrames).toBe(0);
    expect(r.firstProblems).toEqual([]);
  });

  it("a single word wider than the area is the only thing drawn below the floor", () => {
    const l = layoutPage(song(said("supercalifragilisticexpialidociousandthensomemoreletters", 2, 3)), { width: 600, height: 400, fontPx: 56 }, measure);
    const L = l.lines[0];
    expect(L.px).toBeLessThan(PAGE_MIN_SCALE * 56);
    expect(L.rows[0].left + L.width).toBeLessThanOrEqual(600 - sidePad(600, l.fontPx) + 1e-6);
    expect(verifyPage(l).overflowFrames).toBe(0);
  });

  it("previews the next line under the current one, then moves it up within the transition", () => {
    const l = layoutPage(song(said("first line here", 2, 0.4), said("second line now", 4, 0.4), said("third one", 6, 0.4)), AREA, measure);
    const [a, b, c] = l.lines;
    // While the first line is sung, the second is shown smaller and dimmer underneath it.
    const f = pageFrame(l, 2.5);
    const pv = f.items.find((x) => x.line === 1)!;
    expect(pv.role).toBe("preview");
    expect(pv.scale).toBe(PREVIEW_SCALE);
    expect(pv.alpha).toBeCloseTo(PREVIEW_ALPHA, 9);
    expect(pv.top).toBeGreaterThanOrEqual(a.currentTop + a.height);
    expect(f.items.find((x) => x.line === 2)).toBeUndefined();
    // The change starts right after the first line's last word and takes TRANSITION_S.
    expect(b.current - a.end).toBeGreaterThanOrEqual(0);
    expect(b.current - a.end).toBeLessThanOrEqual(SWITCH_MAX_S);
    let lastTop = Infinity;
    for (let t = b.current; t <= b.current + TRANSITION_S + 1e-9; t += 0.01) {
      const it = pageFrame(l, t).items.find((x) => x.line === 1)!;
      expect(it.top).toBeLessThanOrEqual(lastTop + 1e-9);
      lastTop = it.top;
    }
    const after = pageFrame(l, b.current + TRANSITION_S + 0.01);
    expect(after.items.find((x) => x.line === 1)!.role).toBe("current");
    expect(after.items.find((x) => x.line === 0)).toBeUndefined();
    expect(after.items.find((x) => x.line === 2)!.role).toBe("preview");
    expect(c.previewFrom).toBe(b.current);
  });

  it("without the next line: no preview, the next line fades in where the current one was", () => {
    const l = layoutPage(song(said("first line here", 2, 0.4), said("second line now", 4, 0.4)), { ...AREA, showNext: false }, measure);
    expect(l.lines.every((L) => L.previewFrom === null)).toBe(true);
    expect(pageFrame(l, 2.5).items.map((x) => x.line)).toEqual([0]);
    expect(l.lines[1].currentTop).toBeCloseTo(AREA.height / 2 - l.lines[1].height / 2, 6);
  });

  it("songs with nothing to sing: no lines, lines without words, lines of zero-length words", () => {
    const empty = { start: 4, end: 5, words: [] };
    const ghost = (t: number) => ({ start: t, end: t, words: [{ start: t, end: t, chars: [["a", t, t], ["h", t, t]] as [string, number, number][] }] });
    for (const lyr of [song(), song(empty), song(said("before it", 2), empty, ghost(4.5), said("after it", 6))]) {
      for (const showNext of [true, false]) {
        const l = layoutPage(lyr, { ...AREA, showNext }, measure);
        for (let t = -3; t < 12; t += 0.07) for (const it of pageFrame(l, t).items) expect([it.top, it.alpha, it.wipe].every(Number.isFinite)).toBe(true);
        const r = verifyPage(l);
        expect(r.overflowFrames + r.backwards + r.hiddenFrames + r.switchProblems).toBe(0);
      }
    }
    // The line without words is dropped; the zero-length one is shown and counts as sung at its time.
    const l = layoutPage(song(said("before it", 2), empty, ghost(4.5), said("after it", 6)), AREA, measure);
    expect(l.lines.length).toBe(3);
    expect(l.lines[1].motion.at(4.5)).toBeGreaterThanOrEqual(l.lines[1].uEnd);
  });

  it("without the next line, a run of lines shorter than the change never leaves a line sung before it is on screen", () => {
    // One-word lines of 0.15 s back to back (shouts on lines of their own). Each line used
    // to wait for the previous one's 0.2 s entry, so the waits added up: 16 lines in, a
    // line was made current 0.7 s after its word had started and was sung unseen.
    const shouts = Array.from({ length: 16 }, (_, k) => said(["hey", "ho", "go", "now"][k % 4], 3 + k * 0.15, 0.15));
    for (const showNext of [false, true]) {
      const l = layoutPage(song(said("ready set", 0.5, 0.4), ...shouts, said("and the end", 6, 0.4)), { ...AREA, showNext }, measure);
      for (const L of l.lines) {
        expect(L.current).toBeLessThanOrEqual(L.start + 1e-9);
        for (const w of L.words)
          for (let t = w.start + WORD_TOLERANCE_S; t < w.end; t += 0.01) {
            const it = pageFrame(l, t).items.find((x) => x.line === L.index);
            expect(it && (it.role === "current" || it.role === "entering")).toBe(true);
          }
      }
      const r = verifyPage(l);
      expect(r.switchProblems + r.overflowFrames + r.hiddenFrames + r.backwards + r.wrongLetter).toBe(0);
    }
  });

  it("draws a lead-in bar only after a pause, draining to nothing exactly at the first word", () => {
    // 0.4 s gap (no bar), 2 s gap (bar), 8 s rest (countdown, then the line and its bar).
    const l = layoutPage(song(said("one two three", 0.5, 0.3), said("four five six", 1.8, 0.3), said("seven eight nine", 4.7, 0.3), said("ten eleven", 13.6, 0.3)), AREA, measure);
    const [a, b, c, d] = l.lines;
    expect(a.leadIn).toBeNull(); // sung from 0.5 s: no pause before it
    expect(b.leadIn).toBeNull();
    expect(c.leadIn).not.toBeNull();
    expect(c.leadIn!.to).toBe(c.start);
    expect(c.leadIn!.from).toBeGreaterThanOrEqual(c.current + TRANSITION_S - 1e-9);
    let prev = Infinity;
    for (let t = c.leadIn!.from; t < c.start; t += 1 / 60) {
      const bar = pageFrame(l, t).leadIn!;
      expect(bar.line).toBe(2);
      expect(bar.frac).toBeLessThanOrEqual(prev);
      expect(bar.x).toBeCloseTo(c.rows[0].left - (BAR_GAP_EM + BAR_W_EM) * c.px, 6);
      prev = bar.frac;
    }
    expect(pageFrame(l, c.start).leadIn).toBeNull();
    // After the 8 s rest: a countdown, then the line ENTRY_S ahead with a bar of at most LEADIN_MAX_S.
    expect(d.breakBefore).toBe(true);
    const mid = pageFrame(l, (c.end + d.start) / 2);
    expect(mid.items).toEqual([]);
    expect(mid.countdown!.remaining).toBeCloseTo(d.start - (c.end + d.start) / 2, 9);
    expect(d.current).toBeCloseTo(d.start - ENTRY_S, 9);
    expect(d.leadIn!.to - d.leadIn!.from).toBeLessThanOrEqual(LEADIN_MAX_S + 1e-9);
    expect(pageFrame(l, d.start - 1).leadIn!.line).toBe(3);
  });

  it("a song sung from 2 s opens with a full lead-in bar that is gone at the first word", () => {
    const l = layoutPage(song(said("hello there you", 2, 0.4), said("and more", 4, 0.4)), AREA, measure);
    const L = l.lines[0];
    expect(L.current).toBe(-Infinity);
    expect(L.leadIn).toEqual({ from: 0, to: L.start });
    expect(pageFrame(l, 0).leadIn!.frac).toBe(1);
    expect(pageFrame(l, 1).leadIn!.frac).toBeCloseTo(0.5, 9);
    expect(pageFrame(l, L.start).leadIn).toBeNull();
  });

  it("a short area shrinks the whole song so a line and its preview still fit", () => {
    const lyr = song(said("this is quite a long line that wraps onto two rows at this size for sure yes", 2, 0.25), said("and another long line that also needs two rows to fit in here okay then", 8, 0.25));
    const l = layoutPage(lyr, { width: 700, height: 120, fontPx: 44 }, measure);
    expect(l.fontPx).toBeLessThan(44);
    const r = verifyPage(l);
    expect(r.overflowFrames).toBe(0);
    for (const L of l.lines) expect(L.currentTop - 0.3 * L.px).toBeGreaterThanOrEqual(-1e-6);
    expect(l.lines[0].currentTop + l.lines[0].height + 0.3 * l.fontPx + PREVIEW_SCALE * (l.lines[1].height + 0.3 * l.lines[1].px)).toBeLessThanOrEqual(120 + 1e-6);
    expect(LINE_HEIGHT_EM).toBeGreaterThan(1);
  });

  it("the wipe pace follows settings.ui.lyrics_motion: smooth by word, or exact by letter", () => {
    const lyr = song(lineOf([["oh", 1, 3], ["no", 3, 5], ["yes", 5, 5.3]]));
    const word = layoutPage(lyr, { ...AREA, pace: "word" }, measure);
    const letter = layoutPage(lyr, { ...AREA, pace: "letter" }, measure);
    expect(word.pace).toBe("word");
    expect(letter.pace).toBe("letter");
    // Letter pace: the edge is in the letter's own cell while it is sung.
    const L = letter.lines[0];
    for (const g of L.glyphs.filter((x) => x.timed)) {
      const x = L.motion.at((g.start + g.end) / 2);
      expect(x).toBeGreaterThanOrEqual(g.L - 1e-6);
      expect(x).toBeLessThanOrEqual(g.R + 1e-6);
    }
    expect(verifyPage(word).firstProblems).toEqual([]);
    expect(verifyPage(letter).firstProblems).toEqual([]);
  });

  it("the word-paced wipe is smoother than the letter-exact one on the demo song", () => {
    const demo = fixtureSongs().find((s) => s.name.includes("demo"))?.lyrics ?? TWINKLE_LYRICS;
    const w = layoutPage(demo, { ...AREA, pace: "word" }, measure);
    const le = layoutPage(demo, { ...AREA, pace: "letter" }, measure);
    const sw = verifyPage(w).smooth;
    const sl = verifyPage(le).smooth;
    expect(sw.frames).toBeGreaterThan(0);
    expect(sw.bigStepShare).toBeLessThanOrEqual(0.01);
    expect(sw.bigStepShare).toBeLessThan(sl.bigStepShare);
    expect(sw.maxDvEm).toBeLessThan(sl.maxDvEm);
    // The combined figure is the worst line's own (verify.ts frameSmoothness on the line as a strip).
    const worst = Math.max(...w.lines.map((L) => frameSmoothness(lineAsTicker(w, L)).maxDvEm));
    expect(sw.maxDvEm).toBeCloseTo(worst, 9);
  });
});

/** A random song with what aligners produce at their worst (as the D-217 fuzzing of the ticker). */
function randomSong(seed: number): Lyrics {
  const rnd = prng(seed);
  const abc = "abcdefghijklmnopqrstuvwxyzéü'";
  const lines: Lyrics["lines"] = [];
  let t = rnd() * 6;
  const nLines = 1 + Math.floor(rnd() * 14);
  for (let l = 0; l < nLines; l++) {
    const words: Lyrics["lines"][number]["words"] = [];
    const nWords = 1 + Math.floor(rnd() * 11);
    for (let k = 0; k < nWords; k++) {
      const chars: [string, number, number][] = [];
      const n = 1 + Math.floor(rnd() * 12);
      for (let c = 0; c < n; c++) {
        const r = rnd();
        const d = r < 0.1 ? 0 : r < 0.18 ? 0.5 + rnd() * 2.5 : 0.03 + rnd() * 0.27;
        // Now and then a letter starts before the previous one ended (overlap) or out of order.
        const s = rnd() < 0.05 ? t - rnd() * 0.3 : t;
        chars.push([abc[Math.floor(rnd() * abc.length)], s, s + d]);
        t = s + d + (rnd() < 0.7 ? 0 : rnd() * 0.08);
      }
      words.push({ start: chars[0][1], end: chars[chars.length - 1][2], chars });
      t += rnd() < 0.3 ? 0 : rnd() * 0.8;
    }
    lines.push({ start: words[0].start, end: words[words.length - 1].end, words });
    const r = rnd();
    t += r < 0.15 ? 4 + rnd() * 81 : r < 0.5 ? rnd() * 0.3 : rnd() * 3;
  }
  return song(...lines);
}

describe("page display: random songs (zero-length and held letters, overlaps, one-word lines, long rests)", () => {
  for (const area of [AREA, { width: 600, height: 260, fontPx: 40 }]) {
    it(`400 songs in ${area.width}x${area.height}`, () => {
      const bad: string[] = [];
      let exceptions = 0;
      for (let seed = 1; seed <= 400; seed++) {
        for (const pace of ["word", "letter"] as const) {
          const l = layoutPage(randomSong(seed), { ...area, pace }, measure);
          const r = verifyPage(l, { dt: 0.02, frameDt: 1 / 20 });
          exceptions += r.leadExceptions.length;
          const fail =
            r.backwards ||
            r.wrongLetter ||
            r.overflowFrames ||
            r.hiddenFrames ||
            r.leadInProblems ||
            r.switchProblems ||
            r.lookaheadMisses ||
            (pace === "word" && (r.onsetMisses || r.stopped));
          if (fail && bad.length < 5) bad.push(`seed ${seed} ${pace}: ${r.firstProblems.slice(0, 3).join(" | ")}`);
        }
      }
      expect(bad).toEqual([]);
      expect(exceptions).toBeGreaterThanOrEqual(0);
    });
  }
  it("200 songs without the next line: the line being sung is always on screen", () => {
    const bad: string[] = [];
    for (let seed = 1; seed <= 200; seed++) {
      const l = layoutPage(randomSong(seed), { ...AREA, pace: "word", showNext: false }, measure);
      const r = verifyPage(l, { dt: 0.02, frameDt: 1 / 20 });
      let unseen = 0;
      for (const L of l.lines)
        for (const w of L.words)
          for (let t = w.start + WORD_TOLERANCE_S; t < w.end; t += 0.02) {
            const it = pageFrame(l, t).items.find((x) => x.line === L.index);
            if (!it || (it.role !== "current" && it.role !== "entering")) unseen++;
          }
      // Lines are shown later than LOOKAHEAD_S without the preview (lookahead misses are the setting's cost).
      const fail = unseen || r.backwards || r.wrongLetter || r.overflowFrames || r.hiddenFrames || r.leadInProblems || r.switchProblems || r.onsetMisses || r.stopped;
      if (fail && bad.length < 5) bad.push(`seed ${seed}: unseen ${unseen}, ${r.firstProblems.slice(0, 2).join(" | ")}`);
    }
    expect(bad).toEqual([]);
  });
});

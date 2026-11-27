// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers for the lyrics model (docs/SONG_FORMAT.md section 5): text views,
// grapheme segmentation and the edits the sync editor makes (nudge, drag, tap-along,
// text correction). No DOM, no Tauri: unit-tested in Node.
//
// Every edit returns a NEW Lyrics object (the input is never mutated) and keeps the
// format's invariants: start <= end; chars in order inside a word; words in order
// inside a line; lines in order and not overlapping. Unknown keys are preserved.
import type { LyricChar, LyricLine, LyricWord, Lyrics } from "../api/types";

/** Times are stored rounded to the millisecond (avoids float noise like 12.340000001). */
export const roundTime = (t: number): number => Math.round(t * 1000) / 1000;

type SegmenterLike = { segment(text: string): Iterable<{ segment: string }> };
let segmenter: SegmenterLike | null | undefined;

/**
 * Splits text into Unicode extended grapheme clusters (UAX #29) with Intl.Segmenter
 * (Safari 14.1+, Chrome 87+). Falls back to code points where it is missing.
 */
export function segmentGraphemes(text: string): string[] {
  if (segmenter === undefined) {
    const Seg = (Intl as unknown as { Segmenter?: new (l?: string, o?: { granularity: string }) => SegmenterLike }).Segmenter;
    segmenter = Seg ? new Seg(undefined, { granularity: "grapheme" }) : null;
  }
  if (!segmenter) return Array.from(text);
  return Array.from(segmenter.segment(text), (s) => s.segment);
}

export const wordText = (w: LyricWord): string => w.chars.map((c) => c[0]).join("");
export const lineText = (l: LyricLine): string => l.words.map(wordText).join(" ");
export const lyricsText = (l: Lyrics): string => l.lines.map(lineText).join("\n");

export function cloneLyrics(l: Lyrics): Lyrics {
  return JSON.parse(JSON.stringify(l)) as Lyrics;
}

/** Index of the line being sung at time t, or of the next line, or -1 after the end. */
export function lineIndexAt(lyrics: Lyrics, t: number): number {
  const lines = lyrics.lines;
  for (let i = 0; i < lines.length; i++) {
    if (t < lines[i].end) return i;
  }
  return -1;
}

/** Recomputes word and line start/end from their chars (after edits). */
function refreshBounds(line: LyricLine): void {
  for (const w of line.words) {
    if (w.chars.length > 0) {
      w.start = w.chars[0][1];
      w.end = w.chars[w.chars.length - 1][2];
    }
  }
  if (line.words.length > 0) {
    line.start = line.words[0].start;
    line.end = line.words[line.words.length - 1].end;
  }
}

/** Applies f to every time in a line (chars, words, line). */
function mapLineTimes(line: LyricLine, f: (t: number) => number): void {
  for (const w of line.words) {
    for (const c of w.chars) {
      c[1] = roundTime(f(c[1]));
      c[2] = roundTime(f(c[2]));
    }
    w.start = roundTime(f(w.start));
    w.end = roundTime(f(w.end));
  }
  line.start = roundTime(f(line.start));
  line.end = roundTime(f(line.end));
}

/**
 * How far line i may move: its start may go from `minStart` (end of the previous
 * line, or 0) to `maxStart` (so its end stays before the next line's start, or the
 * song's end).
 */
export function lineMoveBounds(lyrics: Lyrics, i: number, songDuration: number): { minStart: number; maxStart: number } {
  const line = lyrics.lines[i];
  const prevEnd = i > 0 ? lyrics.lines[i - 1].end : 0;
  const nextStart = i + 1 < lyrics.lines.length ? lyrics.lines[i + 1].start : Math.max(songDuration, line.end);
  const dur = line.end - line.start;
  return { minStart: prevEnd, maxStart: Math.max(prevEnd, nextStart - dur) };
}

/** Moves line i by `delta` seconds, clamped so lines never overlap. */
export function shiftLine(lyrics: Lyrics, i: number, delta: number, songDuration: number): Lyrics {
  const out = cloneLyrics(lyrics);
  const line = out.lines[i];
  if (!line) return out;
  const { minStart, maxStart } = lineMoveBounds(out, i, songDuration);
  const target = Math.min(maxStart, Math.max(minStart, line.start + delta));
  const d = target - line.start;
  if (d !== 0) mapLineTimes(line, (t) => t + d);
  return out;
}

/** Moves line i so it starts at `start` (clamped like shiftLine). */
export function moveLineTo(lyrics: Lyrics, i: number, start: number, songDuration: number): Lyrics {
  const line = lyrics.lines[i];
  if (!line) return cloneLyrics(lyrics);
  return shiftLine(lyrics, i, start - line.start, songDuration);
}

/** Re-times one word to [start, end], scaling its chars proportionally. */
function retimeWord(w: LyricWord, start: number, end: number): void {
  const oldStart = w.start;
  const oldDur = w.end - w.start;
  const newDur = Math.max(0, end - start);
  const n = w.chars.length;
  w.chars.forEach((c, k) => {
    if (oldDur > 0) {
      c[1] = roundTime(start + ((c[1] - oldStart) / oldDur) * newDur);
      c[2] = roundTime(start + ((c[2] - oldStart) / oldDur) * newDur);
    } else {
      c[1] = roundTime(start + (k / n) * newDur);
      c[2] = roundTime(start + ((k + 1) / n) * newDur);
    }
  });
  if (n > 0) {
    // Pin the ends exactly (rounding inside must not move the word's boundaries).
    w.chars[0][1] = roundTime(start);
    w.chars[n - 1][2] = roundTime(end);
  }
  w.start = roundTime(start);
  w.end = roundTime(end);
}

/**
 * Tap-along: `taps[k]` is the time the singer tapped for word k of line i. Tapped
 * words start at their tap; each keeps the share of its slot it had before (so a
 * word that ended early still ends early), and its letters scale with it. Words
 * after the last tap move with the last tapped word. The result is clamped between
 * the neighbouring lines.
 */
export function retimeLineFromTaps(lyrics: Lyrics, i: number, taps: number[], songDuration: number): Lyrics {
  const out = cloneLyrics(lyrics);
  const line = out.lines[i];
  if (!line || taps.length === 0) return out;
  const prevEnd = i > 0 ? out.lines[i - 1].end : 0;
  const nextStart = i + 1 < out.lines.length ? out.lines[i + 1].start : Math.max(songDuration, line.end);
  const words = line.words;
  const old = words.map((w) => ({ start: w.start, end: w.end }));

  // New word starts: the taps (forced increasing and inside the free span), then the
  // untapped rest shifted with the last tap.
  const n = words.length;
  const starts: number[] = [];
  let last = prevEnd;
  for (let k = 0; k < n; k++) {
    let s: number;
    if (k < taps.length) s = taps[k];
    else s = old[k].start + (starts[taps.length - 1] - old[taps.length - 1].start);
    s = Math.max(s, last);
    starts.push(s);
    last = s;
  }
  for (let k = 0; k < n; k++) {
    const s = starts[k];
    let e: number;
    if (k + 1 < n) {
      const oldSlot = old[k + 1].start - old[k].start;
      const share = oldSlot > 0 ? (old[k].end - old[k].start) / oldSlot : 1;
      e = s + Math.min(1, Math.max(0, share)) * (starts[k + 1] - s);
    } else {
      e = s + (old[k].end - old[k].start);
    }
    e = Math.min(e, nextStart);
    retimeWord(words[k], Math.min(s, nextStart), Math.max(Math.min(s, nextStart), e));
  }
  refreshBounds(line);
  return out;
}

/**
 * Replaces the text of line i. Words that survive with the same count keep their
 * times; letters are re-split inside each word (kept as they were when the letter
 * count did not change). If the number of words changed, the line's time span is
 * shared between the new words by letter count. Empty text removes the line.
 */
export function editLineText(lyrics: Lyrics, i: number, text: string): Lyrics {
  const out = cloneLyrics(lyrics);
  const line = out.lines[i];
  if (!line) return out;
  const newWords = text.trim().split(/\s+/).filter((w) => w.length > 0);
  if (newWords.length === 0) {
    out.lines.splice(i, 1);
    return out;
  }
  const graphemes = newWords.map(segmentGraphemes);
  if (newWords.length === line.words.length) {
    line.words.forEach((w, k) => {
      const g = graphemes[k];
      if (g.length === w.chars.length) {
        w.chars = w.chars.map((c, j) => [g[j], c[1], c[2]] as LyricChar);
      } else {
        w.chars = evenChars(g, w.start, w.end);
      }
    });
  } else {
    const total = graphemes.reduce((a, g) => a + g.length, 0);
    const span = line.end - line.start;
    let t = line.start;
    line.words = graphemes.map((g, k) => {
      const end = k === graphemes.length - 1 ? line.end : t + (g.length / total) * span;
      const w: LyricWord = { start: roundTime(t), end: roundTime(end), chars: evenChars(g, t, end) };
      t = end;
      return w;
    });
  }
  refreshBounds(line);
  return out;
}

function evenChars(g: string[], start: number, end: number): LyricChar[] {
  const n = g.length;
  return g.map((ch, j) => [
    ch,
    roundTime(start + ((end - start) * j) / n),
    j === n - 1 ? roundTime(end) : roundTime(start + ((end - start) * (j + 1)) / n),
  ]);
}

/** Problems that would make a file fail `jukeoke-cli verify` (empty = fine). */
export function validateLyrics(lyrics: Lyrics, songDuration?: number): string[] {
  const problems: string[] = [];
  let prevLineEnd = -Infinity;
  lyrics.lines.forEach((line, li) => {
    const where = `line ${li + 1}`;
    if (line.start > line.end) problems.push(`${where}: starts after it ends`);
    if (line.start < prevLineEnd - 1e-9) problems.push(`${where}: overlaps the line before`);
    prevLineEnd = line.end;
    let prevWordEnd = -Infinity;
    line.words.forEach((w, wi) => {
      if (w.chars.length === 0) problems.push(`${where}, word ${wi + 1}: no letters`);
      if (w.start < prevWordEnd - 1e-9) problems.push(`${where}, word ${wi + 1}: overlaps the word before`);
      prevWordEnd = w.end;
      let prevCharEnd = -Infinity;
      for (const c of w.chars) {
        if (c[1] > c[2]) problems.push(`${where}, "${c[0]}": starts after it ends`);
        if (c[1] < prevCharEnd - 1e-9) problems.push(`${where}, "${c[0]}": out of order`);
        prevCharEnd = c[2];
      }
    });
    if (songDuration !== undefined && line.end > songDuration + 1e-6) problems.push(`${where}: ends after the song`);
  });
  return problems;
}

/** Text lines for `studio_realign` plus the anchors of the lines fixed by hand. */
export function realignRequest(lyrics: Lyrics, anchoredLines: Iterable<number>): { text: string; anchors: { line: number; start: number }[] } {
  const anchors = [...new Set(anchoredLines)]
    .filter((i) => i >= 0 && i < lyrics.lines.length)
    .sort((a, b) => a - b)
    .map((i) => ({ line: i, start: lyrics.lines[i].start }));
  return { text: lyricsText(lyrics), anchors };
}

// ---------------------------------------------------------------------------
// Backing vocals (owner, 2026-11-27: "words in parentheses show as words that you're
// supposed to sing"; DECISIONS D-282)
// ---------------------------------------------------------------------------

const OPENS = new Set(["(", "\uFF08"]);
const CLOSES = new Set([")", "\uFF09"]);
/** A letter or digit: Rust's `char::is_alphanumeric` (Alphabetic or a number). */
const COUNTS = /[\p{Alphabetic}\p{N}]/u;

/** Code points of a word's text. */
const codePoints = (w: LyricWord): string[] => Array.from(wordText(w));

function closesUnopened(line: LyricLine): boolean {
  let depth = 0;
  for (const w of line.words)
    for (const c of codePoints(w)) {
      if (OPENS.has(c)) depth++;
      else if (CLOSES.has(c)) {
        if (depth === 0) return true;
        depth--;
      }
    }
  return false;
}

/**
 * Which words are backing vocals, not the singer's to sing: one flag per word, per line.
 * Lyrics put what the backing singers sing in parentheses: "(Can't help myself) I get so
 * lonely". A word is backing when every letter or digit of it lies inside parentheses
 * (`(` `)` or the full-width `（` `）`), or when its line carries `"kind": "backing"`. A word
 * with no letter or digit is backing when it starts inside parentheses or holds one.
 * Parentheses opened on one line and closed on the next carry over to it (a line with no text
 * in between is passed over, so views that drop such lines get the same flags, whatever kind
 * they carry); one never closed ends with its line. When every word with a letter or digit
 * would be backing, none is.
 *
 * The same rule decides which words get practice bars in the core
 * (crates/jk-format/src/lyrics.rs `Lyrics::backing_words`); change both together.
 * Backing words are drawn dimmed and never take the sung colour (ticker, page, practice lane).
 */
export function backingWords(lyrics: Lyrics | null | undefined): boolean[][] {
  if (!lyrics) return [];
  const out: boolean[][] = [];
  let carry = false;
  // Some word with a letter or digit, and some such word that is the singer's.
  let anyLetters = false;
  let leadLetters = false;
  const hasLetters = (w: LyricWord) => codePoints(w).some((c) => COUNTS.test(c));
  for (const line of lyrics.lines) {
    if (line.words.every((w) => w.chars.length === 0)) {
      out.push(line.words.map(() => false));
      continue;
    }
    if (line.kind === "backing") {
      out.push(line.words.map(() => true));
      if (line.words.some(hasLetters)) anyLetters = true;
      carry = false;
      continue;
    }
    let depth: number = carry && closesUnopened(line) ? 1 : 0;
    const flags: boolean[] = [];
    for (const w of line.words) {
      const startDepth = depth;
      let letters = 0;
      let inside = 0;
      let paren = false;
      for (const c of codePoints(w)) {
        if (OPENS.has(c)) {
          depth++;
          paren = true;
        } else if (CLOSES.has(c)) {
          depth = Math.max(0, depth - 1);
          paren = true;
        } else if (COUNTS.test(c)) {
          letters++;
          if (depth > 0) inside++;
        }
      }
      const backing = letters > 0 ? inside === letters : startDepth > 0 || paren;
      if (letters > 0) {
        anyLetters = true;
        if (!backing) leadLetters = true;
      }
      flags.push(backing);
    }
    carry = depth > 0;
    out.push(flags);
  }
  if (anyLetters && !leadLetters) return out.map((f) => f.map(() => false));
  return out;
}

/** How bright backing words are drawn, against the singer's words (alpha). */
export const BACKING_ALPHA = 0.5;

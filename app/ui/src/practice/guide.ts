// SPDX-License-Identifier: GPL-3.0-or-later
// Helpers around the melody guide as the lane draws it (`practice_guide`, docs/APP_API.md
// 12.5; the stored form is docs/SONG_FORMAT.md section 10). The core sends the guide with
// the graded rule applied and the word references checked; these helpers turn a stored
// guide (the demo-song fixture, the browser mock) into that shape, apply SONG_FORMAT
// 10.4's word rule, and read words and lines for the screen. Pure, no DOM.
import type { GuideStatus, Lyrics, PracticeGuide, PracticeGuideNote } from "../api/types";

/** A note of the stored `guide` chunk (SONG_FORMAT 10.1). */
export interface StoredGuideNote {
  start: number;
  end: number;
  midi: number;
  confidence: number;
  kind: string;
  octave?: string;
  glide_in?: number;
  word?: [number, number, number] | null;
}

export interface StoredGuide {
  low_confidence: number;
  tuning: { cents: number; strength: number; use_cents: number };
  notes: StoredGuideNote[];
  spans?: { start: number; end: number; kind: string }[];
  provenance?: { method?: string; version?: string; created_utc?: string };
}

/** The demo-song fixture written by scripts/extract-demo-guide.mjs. */
export interface GuideFixture {
  title: string;
  duration_s: number;
  key: { tonic: string; mode: "major" | "minor"; name: string } | null;
  lyrics: Lyrics;
  guide: StoredGuide;
  curve: { hop_s: number; t0_s: number; midi: number[] } | null;
}

/** SONG_FORMAT 10.1: which notes are graded (exclusions: midpoint inside an excluded span). */
export function isGraded(n: StoredGuideNote, lowConfidence: number, excluded: { start: number; end: number }[] = []): boolean {
  if (n.kind !== "pitched") return false;
  if (n.confidence < lowConfidence) return false;
  const oct = n.octave ?? "unchecked";
  if (oct !== "ok" && oct !== "unchecked") return false;
  const mid = (n.start + n.end) / 2;
  return !excluded.some((x) => mid >= x.start && mid < x.end);
}

/**
 * SONG_FORMAT 10.4: for each note, the word with the largest overlap between the note and
 * the word (shifted by the lyrics offset), the earlier word on a tie, kept only when that
 * overlap is at least 30 % of the note or 50 % of the word. A display aid only: it never
 * moves, splits or deletes a note.
 */
export function wordRefs(notes: { start: number; end: number }[], lyrics: Lyrics | null, offsetS = 0): ([number, number] | null)[] {
  if (!lyrics) return notes.map(() => null);
  const words: { l: number; w: number; s: number; e: number }[] = [];
  lyrics.lines.forEach((line, l) => line.words.forEach((wd, w) => words.push({ l, w, s: wd.start + offsetS, e: wd.end + offsetS })));
  let from = 0;
  return notes.map((n) => {
    // Words are in time order; skip those that end before the note.
    while (from < words.length && words[from].e <= n.start) from++;
    let best: { l: number; w: number } | null = null;
    let bestOv = 0;
    let bestLen = 1;
    for (let k = Math.max(0, from - 1); k < words.length && words[k].s < n.end; k++) {
      const x = words[k];
      const ov = Math.min(n.end, x.e) - Math.max(n.start, x.s);
      if (ov > bestOv + 1e-9) {
        bestOv = ov;
        best = x;
        bestLen = Math.max(1e-9, x.e - x.s);
      }
    }
    if (!best) return null;
    const noteLen = Math.max(1e-9, n.end - n.start);
    return bestOv >= 0.3 * noteLen || bestOv >= 0.5 * bestLen ? [best.l, best.w] : null;
  });
}

/** The word's text as shown under its note (its graphemes, trailing spaces trimmed). */
export function wordText(lyrics: Lyrics | null, ref: [number, number] | null): string | null {
  if (!lyrics || !ref) return null;
  const w = lyrics.lines[ref[0]]?.words[ref[1]];
  if (!w) return null;
  return w.chars.map((c) => c[0]).join("").trim() || null;
}

/** Rows of the result's breakdown: lyric lines (their times), else phrases cut at gaps of 1 s. */
export function guideLines(notes: { start: number; end: number }[], lyrics: Lyrics | null): { line: number; start: number; end: number }[] {
  if (lyrics && lyrics.lines.length > 0) return lyrics.lines.map((l, i) => ({ line: i, start: l.start, end: l.end }));
  const out: { line: number; start: number; end: number }[] = [];
  for (const n of notes) {
    const last = out[out.length - 1];
    if (last && n.start - last.end < 1) last.end = Math.max(last.end, n.end);
    else out.push({ line: out.length, start: n.start, end: n.end });
  }
  return out;
}

/** A ready GuideStatus for a guide made here (fixture, mock). */
export function readyStatus(g: StoredGuide, graded: number): GuideStatus {
  const pitched = g.notes.filter((n) => n.kind === "pitched").length;
  return {
    state: "ready",
    reason: null,
    error_code: null,
    progress: null,
    in_memory: false,
    pending_write: false,
    method: g.provenance?.method ?? "jk-pitch pyin+notes",
    version: g.provenance?.version ?? "0.1.0",
    created_utc: g.provenance?.created_utc ?? null,
    counts: {
      notes: g.notes.length,
      pitched,
      unpitched: g.notes.length - pitched,
      low_confidence: g.notes.filter((n) => n.confidence < g.low_confidence).length,
      octave_flags: g.notes.filter((n) => n.octave !== undefined && n.octave !== "ok" && n.octave !== "unchecked").length,
      graded,
    },
    tuning_cents: g.tuning.use_cents,
  };
}

/**
 * A stored guide as `practice_guide` returns it: graded rule and exclusions applied, word
 * references recomputed by the 10.4 rule against `lyrics` (the core does the same when the
 * stored cache does not match), range over the pitched notes.
 */
export function practiceGuideOf(
  songId: string,
  g: StoredGuide,
  lyrics: Lyrics | null,
  curve: PracticeGuide["curve"],
  opts: { offsetS?: number; excluded?: { start: number; end: number }[] } = {},
): PracticeGuide {
  const refs = wordRefs(g.notes, lyrics, opts.offsetS ?? 0);
  const notes: PracticeGuideNote[] = g.notes.map((n, i) => {
    const flagged = (n.octave ?? "unchecked") !== "ok" && (n.octave ?? "unchecked") !== "unchecked";
    return {
      start: n.start,
      end: n.end,
      midi: n.midi,
      kind: n.kind === "pitched" ? "pitched" : "unpitched",
      confidence: n.confidence,
      dim: n.confidence < g.low_confidence || flagged,
      graded: isGraded(n, g.low_confidence, opts.excluded),
      glide_in: n.glide_in ?? 0,
      word: refs[i],
    };
  });
  const pitched = notes.filter((n) => n.kind === "pitched");
  const range = pitched.length ? { low: Math.min(...pitched.map((n) => n.midi)), high: Math.max(...pitched.map((n) => n.midi)) } : null;
  return {
    song_id: songId,
    status: readyStatus(g, notes.filter((n) => n.graded).length),
    tuning_cents: g.tuning.use_cents,
    range,
    notes,
    spans: g.spans ?? [],
    curve,
    lines: guideLines(notes, lyrics),
  };
}

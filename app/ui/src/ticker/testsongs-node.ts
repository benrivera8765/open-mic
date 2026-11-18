// SPDX-License-Identifier: GPL-3.0-or-later
// Songs and text measurement for the page display's tests (page.test.ts), the same ones
// the ticker's tests use (ticker.test.ts keeps its own copies, unchanged since D-216):
// a deterministic per-character width shim, a generated rap with the end-to-end rap
// song's density, a synthetic worst case, the generated fixtures (the whole demo song)
// and the end-to-end songs, read at test time from $JK_TICKER_SONGS,
// $JK_BUILD/e2e-stick/Library or ../jukeoke-build/e2e-stick/Library and skipped when
// absent. Their lyrics are never copied into the repository. Node only (never shipped).
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Lyrics } from "../api/types";
import { lyricsFromScore, prng } from "../api/demo";
import { readJksong } from "./jksong-node";
import type { Measure } from "./layout";

const NARROW = new Set([..."il.,;:!'|Iı"]);
const SEMI = new Set([..."fjtr()-?\"[]"]);
const WIDE = new Set([..."mwMW@"]);
function charEm(ch: string): number {
  if (ch === " ") return 0.28;
  if (NARROW.has(ch)) return 0.28;
  if (SEMI.has(ch)) return 0.36;
  if (WIDE.has(ch)) return 0.86;
  if (/[A-Z]/.test(ch)) return 0.68;
  return 0.56;
}
/** Fixed advance per character (em); a grapheme is as wide as its base character. */
export const shimMeasure: Measure = (text, fontPx) => {
  let em = 0;
  for (const g of text.normalize("NFD").replace(/\p{M}/gu, "")) em += charEm(g);
  return em * fontPx;
};

/** Very fast lines, long holds, zero gaps, zero-length letters, an 80 s break (as ticker.test.ts). */
export function stressLyrics(): Lyrics {
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

/** Generated rap with the end-to-end rap song's density profile (as ticker.test.ts). */
export function denseRapLyrics(seed: number): Lyrics {
  const rnd = prng(seed);
  const vocab = "a I am so we go up on it in the and you get got all now how big run said back down town like this that just make take every money crowd people always better nothing counting standing around another jumping through minute rhythm".split(" ");
  const pick = () => vocab[Math.floor(rnd() * vocab.length)];
  const lines: { beat: number; words: [string, number][] }[] = [];
  let t = 20;
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

const here = fileURLToPath(new URL(".", import.meta.url));

/** The generated .jksong fixtures (tests/fixtures/generated: the demo song and a tiny one). */
export function fixtureSongs(): { name: string; lyrics: Lyrics }[] {
  const dir = join(here, "..", "..", "..", "..", "tests", "fixtures", "generated");
  if (!existsSync(dir)) return [];
  const out: { name: string; lyrics: Lyrics }[] = [];
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".jksong")).sort()) {
    const { lyrics } = readJksong(join(dir, f));
    if (lyrics) out.push({ name: `fixture: ${f}`, lyrics });
  }
  return out;
}

/** Folder with the end-to-end run's song files, or null (MSYS paths like /c/... accepted). */
export function e2eSongsDir(): string | null {
  const msys = (p: string) => p.replace(/^\/([a-zA-Z])\//, "$1:/");
  const candidates = [
    process.env.JK_TICKER_SONGS,
    process.env.JK_BUILD ? join(msys(process.env.JK_BUILD), "e2e-stick", "Library") : undefined,
    join(here, "..", "..", "..", "..", "..", "jukeoke-build", "e2e-stick", "Library"),
  ];
  for (const c of candidates) if (c && existsSync(msys(c))) return msys(c);
  return null;
}

export function e2eSongs(dir: string | null): { name: string; lyrics: Lyrics }[] {
  if (!dir) return [];
  const out: { name: string; lyrics: Lyrics }[] = [];
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".jksong") && !n.startsWith(".")).sort()) {
    const { lyrics } = readJksong(join(dir, f));
    if (lyrics) out.push({ name: `e2e: ${f.replace(/\.jksong$/, "")}`, lyrics });
  }
  return out;
}

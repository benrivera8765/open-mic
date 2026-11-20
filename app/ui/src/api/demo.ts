// SPDX-License-Identifier: GPL-3.0-or-later
// Demo library for the browser mock and the UI tests.
//
// Only public-domain texts: "Twinkle, Twinkle, Little Star" (Jane Taylor, 1806),
// "Frère Jacques" (traditional), "Row, Row, Row Your Boat" (traditional, 1852) and
// "Mary Had a Little Lamb" (Sarah Josepha Hale, 1830). No audio exists here: the
// mock only simulates a clock.
//
// The lyrics use the SONG_FORMAT section 5 schema with letter timings produced by a
// small deterministic "singer" model: each syllable sits on the beat grid, consonants
// are short, vowels take the rest, line ends are held, and there are real
// instrumental breaks. When the engine stream's tests/fixtures/generated/
// demo-song.jksong exists, scripts/extract-jksong-lyrics.mjs can copy its lyrics into
// src/api/demo-song.lyrics.json, which then replaces the generated Twinkle lyrics.
import type { LyricChar, LyricLine, LyricWord, Lyrics, SongSettings, SongSummary, StemInfo } from "./types";
import { roundTime, segmentGraphemes } from "../lib/lyrics";

/** A word of the score: its text and how many beats it lasts. */
export type ScoreWord = [text: string, beats: number];
/** A sung line: the beat it starts on and its words. */
export interface ScoreLine {
  beat: number;
  words: ScoreWord[];
}

/** Deterministic PRNG (mulberry32) so generated timings are identical on every run. */
export function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const VOWELS = /^[aeiouyàáâäãåæèéêëìíîïòóôöõøùúûüýÿœ]/i;
const PUNCT = /^[\p{P}\p{S}]$/u;

/**
 * Letter timings for one sung word lasting [start, end): consonants about 60 ms
 * (less when the word is short), punctuation 30 ms, vowels share the rest.
 */
function timeWord(text: string, start: number, end: number): LyricChar[] {
  const g = segmentGraphemes(text);
  const dur = end - start;
  const kinds = g.map((ch) => (PUNCT.test(ch) ? "p" : VOWELS.test(ch.normalize("NFD")) ? "v" : "c"));
  const vowels = kinds.filter((k) => k === "v").length;
  let cons = 0.06;
  let punct = 0.03;
  const fixed = kinds.reduce((a, k) => a + (k === "c" ? cons : k === "p" ? punct : 0), 0);
  // Short words: shrink the fixed parts so vowels keep at least 40 % of the word.
  const room = vowels > 0 ? dur * 0.6 : dur;
  if (fixed > room) {
    const s = room / fixed;
    cons *= s;
    punct *= s;
  }
  const fixedNow = kinds.reduce((a, k) => a + (k === "c" ? cons : k === "p" ? punct : 0), 0);
  const vowel = vowels > 0 ? (dur - fixedNow) / vowels : 0;
  // Without vowels ("hmm", "Ng"), consonants share the word evenly.
  const scale = vowels > 0 ? 1 : dur / Math.max(fixedNow, 1e-9);
  let t = start;
  return g.map((ch, k) => {
    const d = kinds[k] === "v" ? vowel : (kinds[k] === "c" ? cons : punct) * scale;
    const s = t;
    t = k === g.length - 1 ? end : t + d;
    return [ch, roundTime(s), roundTime(t)] as LyricChar;
  });
}

/**
 * Turns a score into letter-timed lyrics. `legato` is the share of each word's beats
 * that is sung (the rest is a tiny gap); some words are fully legato (no gap) so the
 * ticker also meets zero-gap word boundaries. The last word of a line is held for
 * `hold` of its beats, then a breath.
 */
export function lyricsFromScore(
  score: ScoreLine[],
  opts: { bpm: number; offset: number; seed: number; language?: string; source?: string; checked?: boolean },
): Lyrics {
  const spb = 60 / opts.bpm;
  const rnd = prng(opts.seed);
  const lines: LyricLine[] = score.map((sl) => {
    let beat = sl.beat;
    const words: LyricWord[] = sl.words.map(([text, beats], k) => {
      const isLast = k === sl.words.length - 1;
      const start = opts.offset + beat * spb;
      const share = isLast ? 0.82 : rnd() < 0.35 ? 1.0 : 0.88 + rnd() * 0.08;
      const end = start + beats * spb * share;
      beat += beats;
      const chars = timeWord(text, start, end);
      return { start: chars[0][1], end: chars[chars.length - 1][2], confidence: roundTime(0.8 + rnd() * 0.19), chars };
    });
    return { start: words[0].start, end: words[words.length - 1].end, words };
  });
  return {
    lyrics_version: 1,
    language: opts.language ?? "en",
    source: opts.source ?? "user_text",
    checked: opts.checked ?? true,
    alignment: { method: "ctc-forced", model: "demo-generator", anchored_lines: false },
    lines,
  };
}

/** Parses "Twin-kle(2)" style compact scores: words separated by spaces, beats in (). */
function line(beat: number, spec: string): ScoreLine {
  const words: ScoreWord[] = spec.split(" ").map((tok) => {
    const m = /^(.*)\(([\d.]+)\)$/.exec(tok);
    return m ? [m[1], Number(m[2])] : [tok, 1];
  });
  return { beat, words };
}

// --- Twinkle, Twinkle, Little Star (Jane Taylor, 1806), 100 BPM, 8 beats per line ---
const TWINKLE_A = [
  "Twinkle,(2) twinkle,(2) little(2) star,(2)",
  "How I wonder(2) what you are!(2)",
  "Up above(2) the world so high,(2)",
  "Like a diamond(2) in the sky.(2)",
  "Twinkle,(2) twinkle,(2) little(2) star,(2)",
  "How I wonder(2) what you are!(2)",
];
const TWINKLE_B = [
  "When the blazing(2) sun is gone,(2)",
  "When he nothing(2) shines upon,(3)",
  "Then you show your little(2) light,(2)",
  "Twinkle,(2) twinkle,(2) all the night.(2)",
  "Twinkle,(2) twinkle,(2) little(2) star,(2)",
  "How I wonder(2) what you are!(2)",
];

export function twinkleScore(): ScoreLine[] {
  // Intro 13 beats (7.8 s), verse 1, a 20-beat (12 s) instrumental break, verse 2,
  // a short 5-beat break, verse 1 again. Lines follow each other with no rest.
  const out: ScoreLine[] = [];
  let beat = 13;
  for (const s of TWINKLE_A) { out.push(line(beat, s)); beat += 8; }
  beat += 20;
  for (const s of TWINKLE_B) { out.push(line(beat, s)); beat += 8; }
  beat += 5;
  for (const s of TWINKLE_A) { out.push(line(beat, s)); beat += 8; }
  return out;
}

// --- Frère Jacques (traditional), 112 BPM: accents and a combining-mark test ---
function frereScore(): ScoreLine[] {
  const verse = [
    "Frère(2) Jacques,(2) frère(2) Jacques,(2)",
    "Dormez-vous?(4) Dormez-vous?(4)",
    "Sonnez(1) les(0.5) matines,(2.5) sonnez(1) les(0.5) matines,(2.5)",
    "Ding,(1.33) dang,(1.33) dong.(1.34) Ding,(1.33) dang,(1.33) dong.(1.34)",
  ];
  const out: ScoreLine[] = [];
  let beat = 8;
  for (let r = 0; r < 2; r++) {
    for (const s of verse) { out.push(line(beat, s)); beat += 8; }
    beat += 4;
  }
  return out;
}

// --- Row, Row, Row Your Boat (traditional, 1852), 104 BPM: a fast "merrily" line ---
export function rowScore(): ScoreLine[] {
  const verse = [
    "Row,(1.5) row,(1.5) row(1) your(0.5) boat,(1.5)",
    "Gently(1) down(0.5) the(1) stream.(3.5)",
    "Merrily,(1) merrily,(1) merrily,(1) merrily,(1)",
    "Life(1) is(0.5) but(1) a(0.5) dream.(3)",
  ];
  const out: ScoreLine[] = [];
  let beat = 6;
  for (let r = 0; r < 3; r++) {
    for (const s of verse) {
      const l = line(beat, s);
      out.push(l);
      beat += l.words.reduce((a, w) => a + w[1], 0) + 0.5;
    }
    beat += 6;
  }
  return out;
}

/**
 * Lyrics copied from the engine stream's demo-song.jksong by
 * scripts/extract-jksong-lyrics.mjs, if that has been run (the glob is empty otherwise).
 */
const extracted = import.meta.glob<{ default: Lyrics }>("./demo-song.lyrics.json", { eager: true });
export const EXTRACTED_DEMO_LYRICS: Lyrics | null = Object.values(extracted)[0]?.default ?? null;

export const TWINKLE_LYRICS: Lyrics = lyricsFromScore(twinkleScore(), { bpm: 100, offset: 0, seed: 1806 });
export const FRERE_LYRICS: Lyrics = lyricsFromScore(frereScore(), { bpm: 112, offset: 0.2, seed: 1780, language: "fr" });
export const ROW_LYRICS: Lyrics = lyricsFromScore(rowScore(), {
  bpm: 104,
  offset: 0.1,
  seed: 1852,
  source: "transcribed",
  checked: false,
});

export interface DemoSong {
  summary: SongSummary;
  lyrics: Lyrics | null;
  settings: SongSettings;
  stems: StemInfo[];
  sample_rate: number;
  channels: number;
  /** Two colours for the generated cover. */
  cover: [string, string] | null;
}

const STEMS: StemInfo[] = [
  { role: "lead_vocal", in_karaoke_mix: false, pitched: true },
  { role: "backing_vocals", in_karaoke_mix: true, pitched: true },
  { role: "drums", in_karaoke_mix: true, pitched: false },
  { role: "other", in_karaoke_mix: true, pitched: true },
];

export const DEFAULT_SONG_SETTINGS: SongSettings = {
  semitones: 0,
  shift_drums: false,
  guide_vocal: 0,
  backing_on: true,
  backing_level: 1,
  levels: { drums: 1, other: 1 },
  lyrics_offset_ms: 0,
};

const key = (tonic: string, mode: "major" | "minor") => ({ tonic, mode, name: `${tonic} ${mode}` });
const end = (l: Lyrics, tail: number) => Math.ceil(l.lines[l.lines.length - 1].end + tail);

function song(
  id: string,
  partial: Partial<SongSummary> & Pick<SongSummary, "title" | "artist">,
  lyrics: Lyrics | null,
  cover: [string, string] | null,
  settings: Partial<SongSettings> = {},
): DemoSong {
  const s = { ...DEFAULT_SONG_SETTINGS, ...settings };
  return {
    summary: {
      song_id: id,
      album: null,
      duration_s: 120,
      key: null,
      tempo_bpm: null,
      has_lyrics: lyrics !== null,
      lyrics_checked: lyrics?.checked ?? false,
      semitones: s.semitones,
      file_bytes: 96_000_000,
      modified_ms: Date.UTC(2026, 10, 17, 9, 30),
      preset: "full",
      error: null,
      ...partial,
    },
    lyrics,
    settings: s,
    stems: STEMS,
    sample_rate: 44100,
    channels: 2,
    cover,
  };
}

/** The demo library. Built on demand so tests can use fresh copies. */
export function demoLibrary(): DemoSong[] {
  const twinkle = EXTRACTED_DEMO_LYRICS ?? TWINKLE_LYRICS;
  return [
    song(
      "Jane Taylor - Twinkle, Twinkle, Little Star.jksong",
      { title: "Twinkle, Twinkle, Little Star", artist: "Jane Taylor (1806)", duration_s: end(twinkle, 6), key: key("A", "major"), tempo_bpm: 100, file_bytes: 88_400_000, modified_ms: Date.UTC(2026, 10, 17, 9, 30) },
      twinkle,
      ["#ff3d8b", "#9b6bff"],
      { semitones: 2 },
    ),
    song(
      "Traditional - Frère Jacques.jksong",
      { title: "Frère Jacques", artist: "Traditional", album: "Rounds", duration_s: end(FRERE_LYRICS, 4), key: key("F", "major"), tempo_bpm: 112, file_bytes: 61_200_000, modified_ms: Date.UTC(2026, 10, 16, 18, 5) },
      FRERE_LYRICS,
      ["#ff8a1f", "#ffd23f"],
    ),
    song(
      "Traditional - Row, Row, Row Your Boat.jksong",
      { title: "Row, Row, Row Your Boat", artist: "Traditional (1852)", duration_s: end(ROW_LYRICS, 5), key: key("G", "major"), tempo_bpm: 104, file_bytes: 74_900_000, modified_ms: Date.UTC(2026, 10, 15, 12, 0), preset: "clean" },
      ROW_LYRICS,
      ["#4dff6a", "#7dffd8"],
      { lyrics_offset_ms: 40 },
    ),
    song(
      "Sarah Josepha Hale - Mary Had a Little Lamb.jksong",
      { title: "Mary Had a Little Lamb", artist: "Sarah Josepha Hale (1830)", duration_s: 74, key: key("D#", "minor"), tempo_bpm: 96, file_bytes: 52_000_000, modified_ms: Date.UTC(2026, 10, 14, 8, 0) },
      null,
      null,
    ),
    song(
      "Copies/Twinkle (interrupted copy).jksong",
      { title: "Twinkle (interrupted copy)", artist: "Jane Taylor (1806)", duration_s: 0, file_bytes: 41_200_000, modified_ms: Date.UTC(2026, 10, 13, 8, 0), has_lyrics: false,
        error: "This file is incomplete: it is 41.2 MB but should be 88.4 MB. It was probably copied while the stick was pulled out. Copy it again from the original." },
      null,
      null,
    ),
    song(
      "From a newer app.jksong",
      { title: "From a newer app", artist: "Unknown", duration_s: 0, file_bytes: 90_100_000, modified_ms: Date.UTC(2026, 10, 12, 8, 0), has_lyrics: false,
        error: "This song was made by a newer version of the app (format 2.0). Please update the app to play it." },
      null,
      null,
    ),
  ];
}

/** Extra generated songs (no lyrics) for `?many=N` library performance checks. */
export function fillerSongs(n: number): DemoSong[] {
  const rnd = prng(42);
  const tonics = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  return Array.from({ length: n }, (_, i) =>
    song(
      `Filler/Practice track ${i + 1}.jksong`,
      {
        title: `Practice track ${i + 1}`,
        artist: `Generated tone set ${String.fromCharCode(65 + (i % 26))}`,
        duration_s: 90 + Math.floor(rnd() * 200),
        key: key(tonics[i % 12], rnd() < 0.7 ? "major" : "minor"),
        tempo_bpm: 70 + Math.floor(rnd() * 90),
        file_bytes: 50_000_000 + Math.floor(rnd() * 90_000_000),
        modified_ms: Date.UTC(2026, 9, 1) + i * 3_600_000,
      },
      null,
      [`hsl(${(i * 37) % 360} 80% 60%)`, `hsl(${(i * 37 + 120) % 360} 70% 40%)`],
    ),
  );
}

/** A generated cover: a two-colour gradient with the title's initials, as an SVG data URL. */
export function coverDataUrl(title: string, colors: [string, string], px: number): string {
  const initials = title
    .split(/\s+/)
    .filter((w) => /\p{L}/u.test(w))
    .slice(0, 2)
    .map((w) => segmentGraphemes(w.replace(/^\P{L}+/u, ""))[0] ?? "")
    .join("")
    .toUpperCase();
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${px}" height="${px}" viewBox="0 0 100 100">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${colors[0]}"/>` +
    `<stop offset="1" stop-color="${colors[1]}"/></linearGradient></defs>` +
    `<rect width="100" height="100" fill="url(#g)"/>` +
    `<circle cx="50" cy="50" r="30" fill="none" stroke="rgba(255,255,255,0.55)" stroke-width="3"/>` +
    `<text x="50" y="60" font-family="system-ui, sans-serif" font-size="28" font-weight="700" text-anchor="middle" fill="#fff">${initials}</text></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

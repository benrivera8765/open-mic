// SPDX-License-Identifier: GPL-3.0-or-later
// backingWords (D-282): the same cases as crates/jk-format/src/lyrics.rs `backing_tests`,
// so the UI dims exactly the words the core gives no practice bars.
import { describe, expect, it } from "vitest";
import type { LyricLine, Lyrics } from "../api/types";
import { backingWords } from "./lyrics";

function line(texts: string[], t: number, kind?: string): LyricLine {
  const words = texts.map((s, i) => {
    const a = t + i * 0.4;
    return { start: a, end: a + 0.3, chars: [[s, a, a + 0.3] as [string, number, number]] };
  });
  return { start: t, end: t + texts.length * 0.4, words, ...(kind ? { kind } : {}) };
}

const lyrics = (lines: LyricLine[]): Lyrics => ({ lyrics_version: 1, language: "en", source: "user_text", checked: false, lines });

describe("backing vocals (words in parentheses)", () => {
  it("marks words in parentheses as backing", () => {
    const l = lyrics([line(["(Can't", "help", "myself)", "I", "get", "so"], 0), line(["lonely", "(ooh)"], 3)]);
    expect(backingWords(l)).toEqual([
      [true, true, true, false, false, false],
      [false, true],
    ]);
  });

  it("leaves a word only half in parentheses to the singer", () => {
    const l = lyrics([line(["love(d)", "(oh)yeah", "-", "(", "ooh", ")", "you"], 0)]);
    expect(backingWords(l)).toEqual([[false, false, false, true, true, true, false]]);
  });

  it("carries parentheses to the line that closes them, and ends unclosed ones with their line", () => {
    const l = lyrics([line(["sing", "(can't", "help"], 0), line(["myself)", "now"], 2), line(["(never", "closed"], 4), line(["next", "line"], 6)]);
    expect(backingWords(l)).toEqual([
      [false, true, true],
      [true, false],
      [true, true],
      [false, false],
    ]);
  });

  it("passes over a line with no text, so views that drop such lines agree", () => {
    const blank = line(["x"], 1.6);
    blank.words[0].chars = [];
    const empty: LyricLine = { start: 1.5, end: 1.5, words: [] };
    expect(backingWords(lyrics([line(["sing", "(can't"], 0), empty, blank, line(["help)", "now"], 2)]))).toEqual([[false, true], [], [false], [true, false]]);
    const blank2 = line(["x"], 1.6);
    blank2.words[0].chars = [];
    expect(backingWords(lyrics([line(["(all", "of", "it)"], 0), blank2]))).toEqual([[false, false, false], [false]]);
    // An empty line marked as backing is passed over too (the ticker and page drop it).
    const emptyBacking: LyricLine = { start: 1.5, end: 1.5, words: [], kind: "backing" };
    expect(backingWords(lyrics([line(["sing", "(can't"], 0), emptyBacking, line(["help)", "now"], 2)]))).toEqual([[false, true], [], [true, false]]);
    // Words without letters ("-", a music sign) do not keep the safeguard from applying.
    expect(backingWords(lyrics([line(["(all", "of", "it)", "-"], 0), line(["♪"], 2)]))).toEqual([[false, false, false, false], [false]]);
  });

  it("honours a backing line and full-width parentheses", () => {
    const l = lyrics([line(["la", "la"], 0, "backing"), line(["\uFF08ah\uFF09", "oh"], 2)]);
    expect(backingWords(l)).toEqual([
      [true, true],
      [true, false],
    ]);
  });

  it("treats lyrics that put everything in parentheses as all sung", () => {
    expect(backingWords(lyrics([line(["(all", "of", "it)"], 0)]))).toEqual([[false, false, false]]);
    expect(backingWords(null)).toEqual([]);
  });
});

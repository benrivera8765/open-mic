// SPDX-License-Identifier: GPL-3.0-or-later
// Unit tests of the pure helpers: key names, formatting, lyrics edits, the playback clock.
import { describe, expect, it } from "vitest";
import type { Lyrics } from "../api/types";
import { TWINKLE_LYRICS } from "../api/demo";
import { formatBytes, formatCountdown, formatTime, selectionCode } from "./format";
import { keyLabel, keyShort, pitchClass, transposedKeyName } from "./keys";
import {
  editLineText,
  lineMoveBounds,
  lineText,
  lyricsText,
  moveLineTo,
  realignRequest,
  retimeLineFromTaps,
  segmentGraphemes,
  shiftLine,
  validateLyrics,
} from "./lyrics";
import { lyricTime, PlaybackClock } from "./clock";
import { prng } from "../api/demo";

describe("key names", () => {
  it("shows the key change the way the work order asks", () => {
    expect(keyLabel({ tonic: "A", mode: "major", name: "A major" }, 2)).toBe("A major → B major (+2)");
    expect(keyLabel({ tonic: "A", mode: "major", name: "A major" }, 2, "->")).toBe("A major -> B major (+2)");
    expect(keyLabel({ tonic: "A", mode: "major", name: "A major" }, 0)).toBe("A major (original key)");
    expect(keyLabel(null, -3)).toBe("-3 semitones");
  });
  it("spells keys conventionally (flats for flat keys)", () => {
    expect(transposedKeyName({ tonic: "A", mode: "major", name: "" }, 1)).toBe("B♭ major");
    expect(transposedKeyName({ tonic: "D#", mode: "minor", name: "" }, 0)).toBe("E♭ minor");
    expect(transposedKeyName({ tonic: "E", mode: "minor", name: "" }, -3)).toBe("C♯ minor");
    expect(transposedKeyName({ tonic: "C", mode: "major", name: "" }, 12)).toBe("C major");
    expect(transposedKeyName({ tonic: "C", mode: "major", name: "" }, -12)).toBe("C major");
    expect(keyShort({ tonic: "G", mode: "major", name: "" }, 0)).toBe("G maj");
    expect(pitchClass("Bb")).toBe(10);
    expect(pitchClass("H")).toBe(-1);
  });
});

describe("formatting", () => {
  it("formats times, countdowns, sizes and selection codes", () => {
    expect(formatTime(187.4)).toBe("3:07");
    expect(formatTime(3723)).toBe("1:02:03");
    expect(formatCountdown(7.2)).toBe("0:08");
    expect(formatCountdown(0.1)).toBe("0:01");
    expect(formatBytes(18_734_000_000)).toBe("18.7 GB");
    expect(selectionCode(0)).toBe("A1");
    expect(selectionCode(9)).toBe("B1");
    expect(selectionCode(8 * 9)).toBe("J1"); // I is skipped like on real jukeboxes
  });
});

const L = (): Lyrics => JSON.parse(JSON.stringify(TWINKLE_LYRICS)) as Lyrics;

describe("lyrics edits", () => {
  it("segments grapheme clusters (combining marks stay with their letter)", () => {
    expect(segmentGraphemes("naïve")).toEqual(["n", "a", "ï", "v", "e"]);
    expect(segmentGraphemes("café")).toHaveLength(4);
  });

  it("nudges a line without overlapping its neighbours and keeps letters with it", () => {
    const l = L();
    const dur = 120;
    const moved = shiftLine(l, 1, 0.05, dur);
    expect(moved.lines[1].start).toBeCloseTo(l.lines[1].start + 0.05, 6);
    expect(moved.lines[1].words[0].chars[0][1]).toBeCloseTo(l.lines[1].words[0].chars[0][1] + 0.05, 6);
    expect(l.lines[1].start).toBe(TWINKLE_LYRICS.lines[1].start); // input not mutated
    // A huge move is clamped to the gap before the next line.
    const far = shiftLine(l, 1, 100, dur);
    expect(far.lines[1].end).toBeLessThanOrEqual(far.lines[2].start + 1e-9);
    expect(validateLyrics(far, dur)).toEqual([]);
    const { minStart } = lineMoveBounds(l, 1, dur);
    expect(moveLineTo(l, 1, -50, dur).lines[1].start).toBeCloseTo(minStart, 6);
  });

  it("re-times a line from taps: words start at the taps, letters scale with their word", () => {
    const l = L();
    const i = 2;
    const n = l.lines[i].words.length;
    const start = l.lines[i].start + 0.2;
    const taps = Array.from({ length: n }, (_, k) => start + k * 0.5);
    const out = retimeLineFromTaps(l, i, taps, 120);
    out.lines[i].words.forEach((w, k) => {
      expect(w.start).toBeCloseTo(taps[k], 3);
      expect(w.chars[0][1]).toBeCloseTo(taps[k], 3);
      if (k + 1 < n) expect(w.end).toBeLessThanOrEqual(taps[k + 1] + 1e-9);
    });
    expect(validateLyrics(out, 120)).toEqual([]);
    // Fewer taps than words: the rest moves with the last tap.
    const partial = retimeLineFromTaps(l, i, taps.slice(0, 2), 120);
    expect(validateLyrics(partial, 120)).toEqual([]);
  });

  it("corrects text: same words keep their times, new words share the line", () => {
    const l = L();
    const same = editLineText(l, 0, "Twinkle, twinkle, little stars,");
    expect(lineText(same.lines[0])).toBe("Twinkle, twinkle, little stars,");
    expect(same.lines[0].words[3].start).toBe(l.lines[0].words[3].start);
    expect(same.lines[0].words[3].end).toBe(l.lines[0].words[3].end);
    const more = editLineText(l, 0, "Twinkle twinkle little shining star");
    expect(more.lines[0].words).toHaveLength(5);
    expect(more.lines[0].start).toBe(l.lines[0].start);
    expect(more.lines[0].end).toBe(l.lines[0].end);
    expect(validateLyrics(more)).toEqual([]);
    expect(editLineText(l, 0, "  ").lines).toHaveLength(l.lines.length - 1);
  });

  it("builds the re-align request with the hand-fixed lines as anchors", () => {
    const l = L();
    const r = realignRequest(l, [3, 1, 3, 99]);
    expect(r.text).toBe(lyricsText(l));
    expect(r.text.split("\n")).toHaveLength(l.lines.length);
    expect(r.anchors).toEqual([
      { line: 1, start: l.lines[1].start },
      { line: 3, start: l.lines[3].start },
    ]);
  });
});

describe("playback clock (events + performance.now interpolation)", () => {
  /** Simulates an engine sending clock events every ~33 ms with 0-4 ms IPC delay. */
  function simulate(opts: { seconds: number; pauseAt?: number; uiPause?: boolean }) {
    const rnd = prng(11);
    const clock = new PlaybackClock();
    const t0 = 1000; // ms
    let nextEvent = t0;
    const queue: { at: number; pos: number; playing: boolean }[] = [];
    let maxErr = 0;
    let prev = -Infinity;
    let monotone = true;
    let overrun = 0;
    let held = false;
    const truth = (now: number) => Math.min((now - t0) / 1000, opts.pauseAt ?? Infinity);
    clock.onEvent({ position_s: 0, playing: true }, t0);
    for (let now = t0; now < t0 + opts.seconds * 1000; now += 1000 / 60) {
      while (nextEvent <= now) {
        const playing = opts.pauseAt === undefined || (nextEvent - t0) / 1000 < opts.pauseAt;
        queue.push({ at: nextEvent + rnd() * 4, pos: truth(nextEvent), playing });
        nextEvent += 33.3;
      }
      for (const ev of queue.splice(0).sort((a, b) => a.at - b.at)) {
        if (ev.at <= now) clock.onEvent({ position_s: ev.pos, playing: ev.playing }, ev.at);
        else queue.push(ev);
      }
      // The UI asks for the pause 20 ms before the engine actually stops (IPC + fade).
      if (opts.uiPause && !held && opts.pauseAt !== undefined && (now - t0) / 1000 >= opts.pauseAt - 0.02) {
        clock.hold(now);
        held = true;
      }
      const shown = clock.position(now);
      const tr = truth(now);
      if (opts.pauseAt === undefined || (now - t0) / 1000 < opts.pauseAt - 0.05) {
        if (now - t0 > 300) maxErr = Math.max(maxErr, Math.abs(shown - tr));
        if (shown < prev - 1e-9) monotone = false;
        prev = shown;
      } else {
        overrun = Math.max(overrun, shown - (opts.pauseAt ?? 0));
      }
    }
    return { maxErr, monotone, overrun, final: clock.position(t0 + opts.seconds * 1000) };
  }

  it("follows the engine within a few ms and never moves backwards while playing", () => {
    const r = simulate({ seconds: 20 });
    expect(r.monotone).toBe(true);
    expect(r.maxErr).toBeLessThan(0.006);
  });

  it("does not run ahead after a pause the UI asked for, and settles on the engine's position", () => {
    const r = simulate({ seconds: 4, pauseAt: 2, uiPause: true });
    expect(r.overrun).toBeLessThan(0.002);
    expect(Math.abs(r.final - 2)).toBeLessThan(0.002);
  });

  it("a pause it did not ask for (end of song) is overrun by at most one event interval, then glides back", () => {
    const r = simulate({ seconds: 4, pauseAt: 2 });
    expect(r.overrun).toBeLessThan(0.04);
    expect(Math.abs(r.final - 2)).toBeLessThan(0.001);
  });

  it("after a seek while playing, waits at the target until the engine's audio moves (no running ahead)", () => {
    // Engine model measured in the real app (null device): after player_seek the clock
    // events report the target itself, the new audio starts ~30 ms later; events every
    // ~33 ms, 1 ms IPC. One stale event sent before the seek arrives during the wait.
    const c = new PlaybackClock();
    for (let t = 0; t <= 1000; t += 33) c.onEvent({ position_s: 10 + t / 1000, playing: true }, t + 1);
    const seekAt = 1010;
    const startAt = seekAt + 30;
    const truth = (now: number) => (now < seekAt ? 10 + now / 1000 : now < startAt ? 20 : 20 + (now - startAt) / 1000);
    c.jump(20, seekAt);
    c.onEvent({ position_s: 11.009, playing: true }, seekAt + 0.5); // stale, sent before the seek
    c.onEvent({ position_s: 20, playing: true }, seekAt + 1.5); // engine: at the target, not moving yet
    let ahead = 0;
    let prev = -Infinity;
    let back = false;
    let nextEv = seekAt + 37;
    for (let now = seekAt; now < seekAt + 2000; now += 1) {
      if (now >= nextEv) {
        c.onEvent({ position_s: truth(now - 1), playing: true }, now);
        nextEv += 33;
      }
      if (now % 4 === 0) {
        const shown = c.position(now);
        if (shown < prev - 1e-9) back = true;
        prev = shown;
        ahead = Math.max(ahead, shown - truth(now));
      }
    }
    expect(back).toBe(false);
    // Bounded by the event interval's extrapolation, not by the seek latency (was ~30 ms).
    expect(ahead).toBeLessThan(0.003);
  });

  it("snaps on a seek and applies the lyric offsets", () => {
    const c = new PlaybackClock();
    c.onEvent({ position_s: 10, playing: true }, 0);
    c.onEvent({ position_s: 60, playing: true }, 33);
    expect(c.position(33)).toBeCloseTo(60, 6);
    expect(lyricTime(60, 40, 160)).toBeCloseTo(59.8, 9);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
// The practice lane across whole songs (WO2 section 9, "The screen"): at several window
// sizes, at key 0 and shifted keys (-5, +3, every key -12..12 for the names) and with
// octave shifts, each bar is at its note's height on the piano keys, every lyric word is in the strip at its
// own time, the marker is at the sung pitch's height and the lit key is the right one; plus
// reduce-motion and the simple lane, the key heard (not the dial) moving the bars, the
// range rule, the countdown and the marker's honesty (hidden with nobody singing).
//
// Songs: the demo song's real guide (tests/fixtures/generated/demo-song.jksong + the Rust
// builder, extracted by scripts/extract-demo-guide.mjs), the browser mock's generated guides
// (Twinkle at A major with its saved +2, Row with a -14 cent tuning and spoken notes) and a
// wide generated melody that forces the scrolling view.
import { describe, expect, it } from "vitest";
import type { PracticeGuide } from "../api/types";
import { mockGuideFor } from "../api/mock-practice";
import { demoLibrary } from "../api/demo";
import { lyricsFaceMeasure } from "../ticker/lyrics-face-widths";
import fixture from "./fixtures/demo-song.practice.json";
import { type GuideFixture, practiceGuideOf } from "./guide";
import { barOf, FUTURE_S, laneFrame, type LaneInputs, layoutLane, type LaneParams, MIN_ROWS, pianoRows, viewLowAt } from "./lane";
import { verifyLane } from "./laneverify";
import { LiveStore } from "./live";
import { KeyTimeline, keyAt, noteName, spellingFor } from "./pitch";

const measure = lyricsFaceMeasure((t, px) => t.length * px * 0.55, 700);
const demo = fixture as unknown as GuideFixture;
const demoGuide: PracticeGuide = practiceGuideOf("demo", demo.guide, demo.lyrics, demo.curve);

/**
 * Lane sizes: the lane's share (about 60 %) of the skin's lyrics area at 1440x900
 * (~1200x450) and 1280x800 (~1070x400), and at 3840x2160 (scale 2.4), full screen at
 * 1440x900, and the lane alone (lyrics below switched off). Piano on either side.
 */
const SIZES: LaneParams[] = [
  { width: 1200, height: 270, pianoSide: "left", words: true, scale: 1 },
  { width: 1070, height: 240, pianoSide: "left", words: true, scale: 0.889 },
  { width: 3000, height: 680, pianoSide: "left", words: true, scale: 2.4 },
  { width: 1360, height: 470, pianoSide: "right", words: true, scale: 1, dpr: 2 },
  { width: 1070, height: 240, pianoSide: "left", words: true, scale: 0.889, dpr: 1.25 },
  { width: 1200, height: 420, pianoSide: "left", words: false, scale: 1 },
];

describe("practice lane: the demo song's real guide, whole song", () => {
  for (const size of SIZES) {
    for (const sc of [
      { key: 0, octave: 0 },
      { key: -5, octave: 0 },
      { key: 3, octave: 0 },
      { key: -3, octave: -1 },
      { key: 2, octave: 1 },
    ]) {
      it(`${size.width}x${size.height} piano ${size.pianoSide}, key ${sc.key}, octave ${sc.octave}`, () => {
        const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, size, measure);
        const spelling = spellingFor(demo.key, sc.key);
        const rep = verifyLane(L, demoGuide, demo.lyrics, { ...sc, spelling, step: 0.05 });
        expect(rep.firstErrors).toEqual([]);
        expect(rep.barErrors + rep.nameErrors + rep.wordErrors + rep.targetErrors + rep.markerErrors + rep.cueErrors).toBe(0);
        expect(rep.barsChecked).toBeGreaterThan(500);
        expect(rep.targetChecked).toBeGreaterThan(400);
        expect(rep.markerChecked).toBe(rep.targetChecked);
        if (size.words) expect(rep.wordsChecked).toBe(demo.lyrics.lines.reduce((k, l) => k + l.words.length, 0));
        expect(rep.wordOverlaps).toBe(0);
        expect(rep.outOfView).toBe(0);
        // WO2 4.5: at least 2 s of upcoming notes; the now line a quarter to a third of the way in.
        expect(rep.lookaheadS).toBeGreaterThanOrEqual(2);
        expect(rep.nowFraction).toBeGreaterThanOrEqual(0.25);
        expect(rep.nowFraction).toBeLessThanOrEqual(1 / 3);
        // Never less than an octave.
        expect(rep.rows).toBeGreaterThanOrEqual(12);
        expect(rep.tailFrames).toBeGreaterThan(0);
        // The demo song's 8-s instrumental break shows the countdown.
        expect(rep.countdownFrames).toBeGreaterThan(20);
      });
    }
  }

  it("a singer off the note gets the right cue, and octaves are forgiven unless strict", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    for (const sc of [
      { key: 0, octave: 0, singerOffset: 0.8 },
      { key: -5, octave: 0, singerOffset: -0.8 },
      { key: 0, octave: 0, singerOffset: 0.3 },
      { key: 0, octave: 0, singerOctave: -1 },
      { key: 0, octave: 0, singerOctave: -1, octaveMode: "strict" as const },
    ]) {
      const rep = verifyLane(L, demoGuide, demo.lyrics, { ...sc, spelling: "sharps", step: 0.1 });
      expect(rep.cueErrors, JSON.stringify(sc)).toBe(0);
      expect(rep.markerErrors, rep.firstErrors.join("\n")).toBe(0);
    }
  });

  it("reduce-motion and the simple lane: no tail; reduce-motion moves the bars at once", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    for (const opt of [{ reduceMotion: true }, { simple: true }]) {
      const rep = verifyLane(L, demoGuide, demo.lyrics, { key: -2, octave: 0, spelling: "flats", ...opt });
      expect(rep.barErrors + rep.markerErrors + rep.targetErrors).toBe(0);
      expect(rep.tailFrames).toBe(0);
    }
  });
});

describe("practice lane: the mock's guides (generated from the real melodies)", () => {
  const lib = demoLibrary();
  for (const idx of [0, 2]) {
    const song = lib[idx];
    const guide = mockGuideFor(song)!;
    it(`${song.summary.title}: every key -12..12 and octave shifts at 1440x900, names spelled with the key`, () => {
      const L = layoutLane(guide, song.lyrics, song.summary.duration_s, SIZES[0], measure);
      for (let key = -12; key <= 12; key++) {
        const spelling = spellingFor(song.summary.key, key);
        const octave = key <= -6 ? 1 : key >= 8 ? -1 : 0;
        const rep = verifyLane(L, guide, song.lyrics, { key, octave, spelling, step: 0.25 });
        expect(rep.firstErrors, `key ${key}`).toEqual([]);
        expect(rep.barsChecked).toBeGreaterThan(100);
      }
    });
  }

  it("the spoken and low-confidence notes are not graded, and tuning moves the bars onto the keys", () => {
    const row = mockGuideFor(lib[2])!;
    expect(row.tuning_cents).toBe(-14);
    expect(row.notes.some((n) => n.kind === "unpitched" && !n.graded)).toBe(true);
    expect(row.notes.some((n) => n.dim && !n.graded)).toBe(true);
    const L = layoutLane(row, lib[2].lyrics, lib[2].summary.duration_s, SIZES[0], measure);
    // A G3 recorded 14 cents flat (55 - 0.14) sits exactly on the G3 row: lane() takes the tuning off.
    const n = L.notes.find((x) => x.kind === "pitched")!;
    expect(n.lane - Math.round(n.lane)).toBeCloseTo(0, 6);
  });
});

describe("practice lane: names, range, key heard", () => {
  it("a C4 heard at -3 is named A3 on the keys; spelling agrees with the key name", () => {
    expect(noteName(60 - 3, spellingFor({ tonic: "C", mode: "major", name: "C major" }, -3))).toBe("A3");
    // C major -1 = B major (sharps), -2 = Bb major (flats), +1 = Db major (flats), +6 = F# major (sharps).
    const C = { tonic: "C", mode: "major" as const, name: "C major" };
    expect(spellingFor(C, -1)).toBe("sharps");
    expect(spellingFor(C, -2)).toBe("flats");
    expect(spellingFor(C, 1)).toBe("flats");
    expect(spellingFor(C, 6)).toBe("sharps");
    expect(noteName(61, spellingFor(C, 1))).toBe("Db4");
    expect(noteName(61, spellingFor(C, 6))).toBe("C#4");
    // A minor -2 = G minor (flats); E minor +2 = F# minor (sharps); no key: sharps.
    expect(spellingFor({ tonic: "A", mode: "minor", name: "A minor" }, -2)).toBe("flats");
    expect(spellingFor({ tonic: "E", mode: "minor", name: "E minor" }, 2)).toBe("sharps");
    expect(spellingFor(null, -2)).toBe("sharps");
    // A C4 of a C-major song at every key -12..12: the key heard's name and its note, spelled alike.
    const want = ["C3", "Db3", "D3", "Eb3", "E3", "F3", "F#3", "G3", "Ab3", "A3", "Bb3", "B3", "C4", "Db4", "D4", "Eb4", "E4", "F4", "F#4", "G4", "Ab4", "A4", "Bb4", "B4", "C5"];
    for (let k = -12; k <= 12; k++) expect(noteName(60 + k, spellingFor(C, k)), `key ${k}`).toBe(want[k + 12]);
  });

  it("the range: the melody in the current key plus margins, never less than an octave, scrolling when it does not fit", () => {
    // A narrow melody (5 semitones) still shows MIN_ROWS rows.
    const narrow: PracticeGuide = { ...demoGuide, notes: demoGuide.notes.map((n) => ({ ...n, midi: 60 + (n.midi % 5) })) };
    const Ln = layoutLane(narrow, null, demo.duration_s, SIZES[0], measure);
    expect(Ln.mode).toBe("fit");
    expect(Ln.rows).toBeGreaterThanOrEqual(MIN_ROWS - 1e-9);
    // A melody over three octaves does not fit at a readable row: the view scrolls with it, smoothly.
    // (a slow rise over three octaves and back, steps of at most ~2.3 semitones, as melodies move).
    const wide: PracticeGuide = { ...demoGuide, notes: demoGuide.notes.map((n, i) => ({ ...n, midi: 45 + Math.round(36 * (0.5 - 0.5 * Math.cos((2 * Math.PI * i) / 42))) })) };
    const Lw = layoutLane(wide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    expect(Lw.mode).toBe("scroll");
    let worstJump = 0;
    let prev = viewLowAt(Lw, 0, 0);
    for (let t = 0.01; t < demo.duration_s; t += 1 / 60) {
      const v = viewLowAt(Lw, t, 0);
      worstJump = Math.max(worstJump, Math.abs(v - prev));
      prev = v;
    }
    // At most a quarter of a row per frame at 60 fps: smooth, never a jump.
    expect(worstJump).toBeLessThan(0.25);
    const rep = verifyLane(Lw, wide, demo.lyrics, { key: 0, octave: 0, spelling: "sharps", step: 0.05 });
    expect(rep.firstErrors).toEqual([]);
    expect(rep.outOfView).toBe(0);
  });

  it("the bars follow the key HEARD: a pending change moves the bars from its song time on, sliding; reduce-motion jumps", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    const kt = new KeyTimeline();
    kt.reset(0, 1);
    // The dial went to -3 at 3.0 s; the engine says it is heard from 3.28 s.
    kt.update(3.0, 0, { semitones: -3, at_s: 3.28 }, 1, 1000);
    const v = kt.view();
    expect(keyAt(v, 3.27)).toBe(0);
    expect(keyAt(v, 3.28)).toBe(-3);
    const live = new LiveStore();
    const base: LaneInputs = { t: 3.0, nowMs: 1000, keys: v, spelling: "sharps", octaveMode: "auto", live, liveMs: 0, watchOnly: false, reduceMotion: false, simple: false };
    // Right after: two regions; the later one is still sliding.
    const f0 = laneFrame(L, base);
    expect(f0.regions.length).toBe(2);
    expect(f0.regions[1].shift).toBe(0);
    expect(f0.animating).toBe(true);
    // 0.2 s later the slide is done: the bars after 3.28 s are 3 rows lower; before, unchanged.
    const f1 = laneFrame(L, { ...base, t: 3.2, nowMs: 1200 });
    expect(f1.regions[1].shift).toBe(-3);
    expect(f1.regions[0].shift).toBe(0);
    // The note at 3.495 s (G4 = 67) is drawn as E4 (64) relative to the keys; the lit key at 3.6 s is E4.
    const i = demoGuide.notes.findIndex((n) => n.start > 3.4 && n.start < 3.6);
    const f2 = laneFrame(L, { ...base, t: 3.6, nowMs: 2500 });
    const b = barOf(L, f2, i);
    const row = pianoRows(L, f2).find((r) => b.y >= r.y0 && b.y <= r.y1)!;
    expect(row.name).toBe("E4");
    expect(f2.target!.name).toBe("E4");
    expect(f2.key).toBe(-3);
    // Reduce-motion: no slide at all.
    const fr = laneFrame(L, { ...base, reduceMotion: true });
    expect(fr.regions[1].shift).toBe(-3);
    expect(fr.animating).toBe(false);
  });

  it("an event a few ms past a known change that still carries the old key does not flip the bars back", () => {
    // Seen in the real app (Stage 3b): the change to -3 heard from 3.596 s, then an event at
    // 3.598 s still saying 0, then one at 3.657 s saying -3. The bars must stay at -3.
    const kt = new KeyTimeline();
    kt.reset(0, 1);
    kt.update(3.2, 0, { semitones: -3, at_s: 3.596 }, 1, 1000);
    kt.update(3.598, 0, null, 1, 1400);
    expect(keyAt(kt.view(), 3.6)).toBe(-3);
    expect(kt.view().changes.length).toBe(1);
    kt.update(3.657, -3, null, 1, 1460);
    expect(kt.view().changes.map((c) => [c.at, c.semitones])).toEqual([[3.596, -3]]);
    // Well after the change, the old key IS a change back the UI did not see coming.
    kt.update(4.5, 0, null, 1, 2300);
    expect(keyAt(kt.view(), 4.6)).toBe(0);
    // A key the UI never knew of is followed at once, as before.
    const k2 = new KeyTimeline();
    k2.reset(0, 1);
    k2.update(5.0, -2, null, 1, 1000);
    expect(keyAt(k2.view(), 5.0)).toBe(-2);
  });

  it("the marker hides with nobody singing, when the values stop, and in watch only", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    const live = new LiveStore();
    const kt = new KeyTimeline();
    kt.reset(0, 0);
    const base: LaneInputs = { t: 3.0, nowMs: 5000, keys: kt.view(), spelling: "sharps", octaveMode: "auto", live, liveMs: 5000, watchOnly: false, reduceMotion: false, simple: false };
    live.push(2.96, null, -70);
    expect(laneFrame(L, base).singer).toBeNull();
    live.push(2.97, 67, -20);
    expect(laneFrame(L, base).singer).not.toBeNull();
    expect(laneFrame(L, { ...base, liveMs: 4000 }).singer).toBeNull();
    expect(laneFrame(L, { ...base, watchOnly: true }).singer).toBeNull();
    expect(laneFrame(L, { ...base, t: 4 }).singer).toBeNull();
  });

  it("a lit stretch across a heard key change is cut there, each part on its own bar", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    const i = demoGuide.notes.findIndex((n) => n.kind === "pitched" && n.end - n.start > 0.5);
    const n = demoGuide.notes[i];
    const mid = (n.start + n.end) / 2;
    const kt = new KeyTimeline();
    kt.reset(0, 1);
    kt.update(mid - 0.28, 0, { semitones: -5, at_s: mid }, 1, 1000);
    const live = new LiveStore();
    live.apply({ seq: 1, segment: 1, pitch: [], hits: [[i, n.start, n.end, 0]], done: [] });
    const f = laneFrame(L, { t: n.end + 0.5, nowMs: 5000, keys: kt.view(), spelling: "sharps", octaveMode: "auto", live, liveMs: -1e9, watchOnly: false, reduceMotion: false, simple: false });
    expect(f.regions.length).toBe(2);
    expect(f.lit.length / 4).toBe(2);
    // Before the change: on the bar at key 0; after it: 5 rows lower, where the bar is drawn.
    expect(f.lit[3]).toBeCloseTo(barOf(L, f, i).y, 6);
    expect(f.lit[7] - f.lit[3]).toBeCloseTo(5 * L.rowH, 6);
    expect(f.lit[2]).toBeCloseTo(f.regions[0].x1, 6);
    expect(f.lit[5]).toBeCloseTo(f.regions[1].x0, 6);
  });

  it("lit parts stay lit as they move left; a note sung again keeps its latest attempt", () => {
    const L = layoutLane(demoGuide, demo.lyrics, demo.duration_s, SIZES[0], measure);
    const live = new LiveStore();
    const kt = new KeyTimeline();
    kt.reset(0, 0);
    live.apply({ seq: 1, segment: 0, pitch: [], hits: [[2, 3.0, 3.2, 0], [2, 3.2, 3.3, 1], [2, 3.3, 3.45, 0]], done: [] });
    const f = laneFrame(L, { t: 4.5, nowMs: 0, keys: kt.view(), spelling: "sharps", octaveMode: "auto", live, liveMs: -1e9, watchOnly: false, reduceMotion: false, simple: false });
    expect(f.lit.length / 4).toBe(2);
    expect(f.lit[1]).toBeCloseTo(L.nowX + (3.0 - 4.5) * L.pxPerSec, 6);
    // A continuation extends the last stretch; going back in time is a new attempt.
    live.addHit(2, 3.45, 3.455, 0);
    expect(live.hitsOf(2)!.length).toBe(9);
    live.addHit(2, 2.995, 3.05, -1);
    expect(live.hitsOf(2)!.length).toBe(3);
    expect(FUTURE_S).toBeGreaterThanOrEqual(2);
  });
});

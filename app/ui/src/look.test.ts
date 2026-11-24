// SPDX-License-Identifier: GPL-3.0-or-later
// The owner's jukebox customisation (2026-11-22): look settings, silver / pilaster frame
// art, the pilaster's offset bubble tracks, and watch-only "follow".
import { describe, expect, it } from "vitest";
import { DEFAULT_APP_SETTINGS, DEFAULT_LOOK, LANE_BAR_COLORS, LOOK_CHOICES, lookOf, THEMES, themeState } from "./api/defaults";
import type { LaneLayout, LaneNote } from "./practice/lane";
import { followLaneAt } from "./practice/lane";
import { joinedNotes } from "./practice/lanerenderer";
import { pilasterChannels, trimSvg, tubeSvg } from "./skin/frame-art";
import { PIPE_STRIDE, pipeStrip } from "./skin/PipeGL";
import { CelebrationGate, RARE_GAP_MS } from "./practice/celebration";
import { bubbleTrack, computeFrameGeometry } from "./skin/geometry";

const W = 1440;
const H = 900;
const g = computeFrameGeometry({
  width: W,
  height: H,
  screen: { x: W * 0.05, y: H * 0.09, w: W * 0.9, h: H * 0.735 },
  screenRadius: Math.min(W * 0.11, H * 0.175),
  consoleTop: H * 0.825,
});

describe("look settings", () => {
  it("uses the owner's defaults when nothing is saved", () => {
    expect(lookOf(undefined)).toEqual(DEFAULT_LOOK);
    expect(lookOf({})).toEqual(DEFAULT_LOOK);
    expect(DEFAULT_LOOK).toMatchObject({ cabinet: "pearl", accents: "silver", badge_field: "lapis", badge_text: "gold", lattice: "silver", inner_trim: true });
  });

  it("keeps known values and drops unknown ones one by one", () => {
    const look = lookOf({ look: { cabinet: "maple", frame: "plasma" as never, celebrate: false, celebration: "fireworks" } });
    expect(look.cabinet).toBe("maple");
    expect(look.frame).toBe(DEFAULT_LOOK.frame);
    expect(look.celebrate).toBe(false);
    expect(look.celebration).toBe("fireworks");
  });
});

describe("frame art options", () => {
  it("draws silver chrome bands by default and red enamel on request", () => {
    expect(trimSvg(g, 180)).toContain("#6a727c");
    expect(trimSvg(g, 180)).not.toContain("#c8102e");
    expect(trimSvg(g, 180, { accents: "red" })).toContain("#c8102e");
  });

  it("draws the pilaster panel with two neon bubble channels and a shell finial", () => {
    const tube = tubeSvg(g, { style: "pilaster" });
    expect(tube).not.toMatch(/NaN|undefined|Infinity/);
    // Two solid amber neon tubes and no pale line down their middle.
    expect(tube).toContain("#ff520e");
    expect(tube).toContain("#ff7812");
    expect(tube).not.toContain("#ffd9a8");
    const trim = trimSvg(g, 180, { style: "pilaster" });
    expect(trim).not.toMatch(/NaN|undefined|Infinity/);
    expect(trim).toContain(" Q");
    // No glare line down the panel.
    expect(trim).not.toContain('stroke-opacity="0.72"');
  });

  it("draws the lava lamp and lighting tube styles", () => {
    for (const style of ["lava", "light"] as const) {
      const tube = tubeSvg(g, { style });
      expect(tube).not.toMatch(/NaN|undefined|Infinity/);
      expect(trimSvg(g, 180, { style })).not.toMatch(/NaN|undefined|Infinity/);
    }
    // Both are glass tubes with glare lines (the pilaster panel has none).
    expect(trimSvg(g, 180, { style: "light" })).toContain('stroke-opacity="0.72"');
    expect(trimSvg(g, 180, { style: "lava" })).toContain('stroke-opacity="0.72"');
    // The lightning tube is blue water with no electrode rod.
    expect(tubeSvg(g, { style: "light" })).toContain("#8ec8ff");
    expect(tubeSvg(g, { style: "light" })).not.toContain("#05020c");
    expect(lookOf({ look: { frame: "lava" } }).frame).toBe("lava");
    expect(lookOf({ look: { frame: "ocean" } }).frame).toBe("ocean");
    const ocean = tubeSvg(g, { style: "ocean" });
    expect(ocean).not.toMatch(/NaN|undefined|Infinity/);
    expect(ocean).toContain("stroke-dasharray");
    expect(lookOf({ look: { frame: "light", badge_rim: "gold" } })).toMatchObject({ frame: "light", badge_rim: "gold" });
    expect(DEFAULT_LOOK.badge_rim).toBe("lapis");
  });

  it("gives the column bases the cabinet's finish", () => {
    expect(trimSvg(g, 180, { cabinet: "walnut" })).toContain("#6a4326");
    expect(trimSvg(g, 180, { cabinet: "maple" })).toContain("#e2b880");
    const pearl = trimSvg(g, 180, { cabinet: "pearl" });
    expect(pearl).toContain("#fff1e6");
    expect(pearl).toContain("url(#pearlGlint)");
    expect(trimSvg(g, 180, { cabinet: "aquarium" })).toContain("#2a4c5c");
    expect(lookOf({ look: { cabinet: "aquarium" } }).cabinet).toBe("aquarium");
  });

  it("runs the pilaster's bubble tracks either side of the centre line", () => {
    const { offset } = pilasterChannels(g);
    const mid = bubbleTrack(g, "left");
    const inner = bubbleTrack(g, "left", g.baseTop, -offset);
    const outer = bubbleTrack(g, "left", g.baseTop, offset);
    expect(inner.length).toBeLessThan(mid.length);
    expect(outer.length).toBeGreaterThan(mid.length);
    const p = { x: 0, y: 0, nx: 0, ny: 0 };
    inner.at(1, p);
    expect(p.x).toBeCloseTo(g.xL + offset, 5);
  });
});

describe("watch only: follow the singer", () => {
  const note = (i: number, start: number, end: number, lane: number, kind: LaneNote["kind"] = "pitched"): LaneNote => ({ i, start, end, lane, kind, dim: false, graded: true, glideIn: 0, word: null });
  const L = {
    notes: [note(0, 1, 2, 60), note(1, 2.2, 3, 62), note(2, 3.1, 4, 70), note(3, 5, 6, 64), note(4, 6, 7, 0, "unpitched")],
  } as unknown as LaneLayout;

  it("is exactly on every pitched note and silent in rests and on unpitched notes", () => {
    expect(followLaneAt(L, 1.5)).toBe(60);
    expect(followLaneAt(L, 2.5)).toBe(62);
    expect(followLaneAt(L, 0.5)).toBeNaN();
    expect(followLaneAt(L, 4.5)).toBeNaN();
    expect(followLaneAt(L, 6.5)).toBeNaN();
  });

  it("slides through a short gap and lifts off over a wide leap", () => {
    expect(followLaneAt(L, 2.1)).toBeCloseTo(61, 5);
    expect(followLaneAt(L, 2.05)).toBeGreaterThan(60);
    expect(followLaneAt(L, 2.05)).toBeLessThan(61);
    expect(followLaneAt(L, 3.05)).toBeNaN();
  });
});

describe("lane: notes of one word are one tube", () => {
  const note = (i: number, start: number, end: number, lane: number, word: string | null, glideIn = 0): LaneNote => ({ i, start, end, lane, kind: "pitched", dim: false, graded: true, glideIn, word });
  const lay = (notes: LaneNote[]) => ({ notes }) as unknown as LaneLayout;

  it("joins the notes of one word across a short gap, up to an octave", () => {
    // "sorry": two bars of one word, a quarter second apart, three semitones up.
    const L = lay([note(0, 1.0, 2.0, 60, "4:3"), note(1, 2.25, 2.8, 63, "4:3")]);
    expect(joinedNotes(L, 1)).toBe(true);
  });

  it("does not join different words, long breaks or leaps beyond an octave", () => {
    expect(joinedNotes(lay([note(0, 1.0, 2.0, 60, "4:3"), note(1, 2.25, 2.8, 63, "4:4")]), 1)).toBe(false);
    expect(joinedNotes(lay([note(0, 1.0, 2.0, 60, "4:3"), note(1, 2.6, 2.8, 63, "4:3")]), 1)).toBe(false);
    expect(joinedNotes(lay([note(0, 1.0, 2.0, 60, "4:3"), note(1, 2.1, 2.8, 74, "4:3")]), 1)).toBe(false);
  });

  it("still joins a measured scoop between words, but never across more than a fifth", () => {
    expect(joinedNotes(lay([note(0, 1.0, 2.0, 60, "1:1"), note(1, 2.05, 2.8, 62, "1:2", 0.05)]), 1)).toBe(true);
    expect(joinedNotes(lay([note(0, 1.0, 2.0, 60, "1:1"), note(1, 2.05, 2.8, 70, "1:2", 0.05)]), 1)).toBe(false);
  });
});

describe("themes", () => {
  it("presets only use options the look knows, and Deluxe is the default look", () => {
    for (const t of THEMES) {
      for (const [k, v] of Object.entries(t.look)) {
        const choices = (LOOK_CHOICES as Record<string, readonly unknown[] | undefined>)[k];
        if (choices) expect(choices, `${t.id}.${k}`).toContain(v);
      }
      expect(lookOf({ look: { ...t.look, theme: t.id } })).toMatchObject(t.look);
    }
    const deluxe = THEMES.find((t) => t.id === "deluxe")!;
    expect(DEFAULT_LOOK).toMatchObject(deluxe.look);
    expect(deluxe.colors).toEqual(DEFAULT_APP_SETTINGS.ui.colors);
  });

  it("makes Molten from fire, a forge, bronze, iron, magma and ember", () => {
    const molten = THEMES.find((t) => t.id === "molten")!;
    expect(molten.look).toMatchObject({ cabinet: "forge", frame: "fire", accents: "iron", lattice: "iron", metal: "bronze", badge_field: "magma", badge_text: "ember", glow: "ember", lane_bars: "ember" });
  });

  it("tells a theme as applied from one changed afterwards (colours included)", () => {
    const molten = THEMES.find((t) => t.id === "molten")!;
    const applied = { look: { ...molten.look, theme: molten.id }, colors: { ...molten.colors } };
    expect(themeState(applied)).toEqual({ theme: molten, modified: false });
    expect(themeState({ ...applied, look: { ...applied.look, cabinet: "walnut" } }).modified).toBe(true);
    expect(themeState({ ...applied, colors: { ...molten.colors, sung: "#ffffff" } }).modified).toBe(true);
    // Nothing saved yet: the default look is Deluxe, unchanged.
    expect(themeState({ colors: { ...DEFAULT_APP_SETTINGS.ui.colors } })).toMatchObject({ modified: false });
    expect(themeState({ colors: { ...DEFAULT_APP_SETTINGS.ui.colors } }).theme?.id).toBe("deluxe");
  });

  it("has a note bar colour for every lane option", () => {
    for (const v of LOOK_CHOICES.lane_bars) expect(LANE_BAR_COLORS[v]).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("draws bronze rings, iron bands, a forge base and the clear fire pipe", () => {
    const molten = trimSvg(g, 180, { style: "fire", accents: "iron", cabinet: "forge", metal: "bronze" });
    expect(molten).toContain("#6a3414"); // bronze ring
    expect(molten).toContain("#1c1f22"); // iron band
    expect(molten).toContain("#3a2418"); // forge base
    expect(molten).not.toContain("#815e20"); // no gold ring
    expect(trimSvg(g, 180)).toContain("#815e20");
    const fire = tubeSvg(g, { style: "fire" });
    expect(fire).toContain("#ffc890");
  });
});

describe("GPU fire pipe (D-272)", () => {
  it("runs one strip from foot to foot, measured from the nearer foot and from the left, across -1..1", () => {
    const v = pipeStrip(g);
    const S = PIPE_STRIDE;
    const n = v.length / S;
    expect(n % 2).toBe(0);
    let maxS = 0;
    for (let i = 0; i < n; i++) {
      expect(Math.abs(v[i * S + 3])).toBe(1);
      expect(v[i * S + 2]).toBeGreaterThanOrEqual(0);
      if (i > 1) expect(v[i * S + 4]).toBeGreaterThanOrEqual(v[(i - 2) * S + 4]);
      maxS = Math.max(maxS, v[i * S + 2]);
    }
    // Both ends at a foot (distance 0), the crest furthest from both; the two sides mirror.
    expect(v[2]).toBe(0);
    expect(v[(n - 1) * S + 2]).toBeCloseTo(0, 6);
    expect(v[(n / 2) * S + 2]).toBeCloseTo(maxS, 0);
    expect(v[(n - 1) * S + 4]).toBeCloseTo(2 * bubbleTrack(g, "left").length, 0);
    expect(v[0] + v[(n - 2) * S + 0]).toBeCloseTo(W, 0);
  });

  it("is on by default and is not part of a theme", () => {
    expect(DEFAULT_LOOK.gpu_fx).toBe(true);
    for (const t of THEMES) expect("gpu_fx" in t.look).toBe(false);
  });
});

describe("how often perfect phrases are celebrated", () => {
  const run = (freq: "often" | "every" | "second" | "third" | "rare", times: number[]) => {
    const g = new CelebrationGate();
    return times.map((t) => g.allow(freq, t));
  };
  it("celebrates every one, every second or every third", () => {
    expect(run("every", [0, 1, 2, 3])).toEqual([true, true, true, true]);
    expect(run("often", [0, 1, 2])).toEqual([true, true, true]);
    expect(run("second", [0, 1, 2, 3, 4])).toEqual([true, false, true, false, true]);
    expect(run("third", [0, 1, 2, 3, 4, 5, 6])).toEqual([true, false, false, true, false, false, true]);
  });
  it("now and then: at most once every 30 seconds", () => {
    expect(run("rare", [0, 5000, RARE_GAP_MS - 1, RARE_GAP_MS, RARE_GAP_MS + 10, 2 * RARE_GAP_MS + 1])).toEqual([true, false, false, true, false, true]);
  });
  it("lets Molten blaze with the inferno", () => {
    expect(THEMES.find((t) => t.id === "molten")!.look.celebration).toBe("inferno");
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  angleDelta,
  angleFor,
  clampSemitones,
  detentDegrees,
  formatSemitones,
  pointerAngle,
  prettyKeyLabel,
  stepsForRotation,
  wheelSteps,
} from "./keydial-math";
import { resolveMotion } from "./motion";

describe("key dial maths", () => {
  it("spreads -12..+12 over 270 degrees, 0 at the top", () => {
    expect(detentDegrees()).toBe(11.25);
    expect(angleFor(0)).toBe(0);
    expect(angleFor(-12)).toBe(-135);
    expect(angleFor(12)).toBe(135);
    expect(angleFor(2)).toBe(22.5);
  });

  it("clamps and rounds", () => {
    expect(clampSemitones(13)).toBe(12);
    expect(clampSemitones(-20)).toBe(-12);
    expect(clampSemitones(2.4)).toBe(2);
    expect(clampSemitones(Number.NaN)).toBe(0);
  });

  it("measures pointer angles clockwise from 12 o'clock", () => {
    expect(pointerAngle(0, 0, 0, -10)).toBeCloseTo(0);
    expect(pointerAngle(0, 0, 10, 0)).toBeCloseTo(90);
    expect(pointerAngle(0, 0, -10, 0)).toBeCloseTo(-90);
  });

  it("unwraps angle differences across 180 degrees", () => {
    expect(angleDelta(170, -170)).toBeCloseTo(20);
    expect(angleDelta(-170, 170)).toBeCloseTo(-20);
    expect(angleDelta(10, 30)).toBeCloseTo(20);
  });

  it("turns rotation into whole detents, symmetric round zero", () => {
    expect(stepsForRotation(5)).toBe(0);
    expect(stepsForRotation(6)).toBe(1);
    expect(stepsForRotation(-6)).toBe(-1);
    expect(stepsForRotation(22.5)).toBe(2);
    expect(stepsForRotation(-135)).toBe(-12);
  });

  it("treats a mouse notch as one detent and accumulates trackpad pixels", () => {
    expect(wheelSteps(0, -100, 0)).toEqual({ steps: 1, remainder: 0 });
    expect(wheelSteps(0, 120, 0)).toEqual({ steps: -1, remainder: 0 });
    expect(wheelSteps(0, 3, 1)).toEqual({ steps: -1, remainder: 0 });
    let r = { steps: 0, remainder: 0 };
    let total = 0;
    for (let i = 0; i < 10; i++) {
      r = wheelSteps(r.remainder, -10, 0);
      total += r.steps;
    }
    expect(total).toBe(2); // 100 px up = 2 detents of 40 px
  });

  it("formats values and labels", () => {
    expect(formatSemitones(2)).toBe("+2");
    expect(formatSemitones(0)).toBe("0");
    expect(formatSemitones(-3)).toBe("−3");
    expect(prettyKeyLabel("A major -> B major (+2)")).toBe("A major → B major (+2)");
  });
});

describe("motion setting", () => {
  it("follows the system only when set to system", () => {
    expect(resolveMotion("system", true)).toBe("reduce");
    expect(resolveMotion("system", false)).toBe("full");
    expect(resolveMotion("full", true)).toBe("full");
    expect(resolveMotion("reduce", false)).toBe("reduce");
  });
});

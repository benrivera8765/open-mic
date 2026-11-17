// SPDX-License-Identifier: GPL-3.0-or-later
// Pure helpers behind the key dial (unit-tested in keydial-math.test.ts).

/** The dial sweeps 270 degrees for -12..+12: one detent every 11.25 degrees. */
export const SWEEP_DEG = 270;
export const DEFAULT_MIN = -12;
export const DEFAULT_MAX = 12;

export function detentDegrees(min = DEFAULT_MIN, max = DEFAULT_MAX): number {
  return SWEEP_DEG / Math.max(1, max - min);
}

/** Rotation of the pointer for `n` semitones (0 = straight up, clockwise positive). */
export function angleFor(n: number, min = DEFAULT_MIN, max = DEFAULT_MAX): number {
  const mid = (min + max) / 2;
  return (n - mid) * detentDegrees(min, max);
}

export function clampSemitones(n: number, min = DEFAULT_MIN, max = DEFAULT_MAX): number {
  if (!Number.isFinite(n)) return 0;
  return Math.max(min, Math.min(max, Math.round(n)));
}

/** Smallest signed difference b - a between two angles in degrees, in (-180, 180]. */
export function angleDelta(a: number, b: number): number {
  let d = (b - a) % 360;
  if (d <= -180) d += 360;
  if (d > 180) d -= 360;
  return d;
}

/** Pointer angle in degrees (0 = up, clockwise positive) of (x, y) around (cx, cy). */
export function pointerAngle(cx: number, cy: number, x: number, y: number): number {
  return (Math.atan2(x - cx, cy - y) * 180) / Math.PI;
}

/** Whole detents in an accumulated rotation (rounded to the nearest detent). */
export function stepsForRotation(deg: number, min = DEFAULT_MIN, max = DEFAULT_MAX): number {
  const s = deg / detentDegrees(min, max);
  // Round half away from zero so a small wiggle round 0 does not flip-flop.
  return s >= 0 ? Math.floor(s + 0.5) : -Math.floor(-s + 0.5);
}

/** "+2", "0", "−3" (true minus sign, it lines up with the plus in condensed type). */
export function formatSemitones(n: number): string {
  if (n > 0) return `+${n}`;
  if (n < 0) return `−${-n}`;
  return "0";
}

/** "A major -> B major (+2)" -> "A major → B major (+2)" for display. */
export function prettyKeyLabel(label: string): string {
  return label.replace(/\s*->\s*/g, " → ");
}

/**
 * Wheel handling. A mouse notch (|delta| >= 50 px, or line/page mode) is one
 * detent; trackpad pixels accumulate, one detent per 40 px. Returns the steps to
 * apply (wheel up = key up) and the new remainder.
 */
export function wheelSteps(remainder: number, deltaY: number, deltaMode: number): { steps: number; remainder: number } {
  if (deltaMode !== 0 || Math.abs(deltaY) >= 50) {
    return { steps: deltaY < 0 ? 1 : deltaY > 0 ? -1 : 0, remainder: 0 };
  }
  const acc = remainder - deltaY;
  const steps = acc >= 0 ? Math.floor(acc / 40) : -Math.floor(-acc / 40);
  return { steps, remainder: acc - steps * 40 };
}

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { fitBadgeText } from "./Badge";

describe("badge name fitting", () => {
  it("keeps a short name at full size, unsqueezed", () => {
    const f = fitBadgeText("Jukeoke");
    expect(f.fontSize).toBeGreaterThan(40);
    expect(f.squeeze).toBe(false);
  });

  it("shrinks a medium name without squeezing it", () => {
    const f = fitBadgeText("Jukeoke Pro");
    expect(f.fontSize).toBeLessThan(44);
    expect(f.fontSize).toBeGreaterThanOrEqual(26);
    expect(f.squeeze).toBe(false);
  });

  it("squeezes a long name instead of shrinking it below the legible size", () => {
    const f = fitBadgeText("Jukeoke Karaoke Studio Deluxe");
    expect(f.fontSize).toBe(26);
    expect(f.squeeze).toBe(true);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { formatMs, modeLabel } from "./format";

describe("format helpers", () => {
  it("labels every data mode in plain words", () => {
    expect(modeLabel("portable")).toMatch(/^Portable/);
    expect(modeLabel("dev")).toMatch(/^Developer/);
    expect(modeLabel("user")).toMatch(/^Installed/);
  });

  it("formats milliseconds and handles missing values", () => {
    expect(formatMs(12.34)).toBe("12.3 ms");
    expect(formatMs(null)).toBe("n/a");
    expect(formatMs(Number.NaN)).toBe("n/a");
  });
});

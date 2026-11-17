// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { roomSvg, svgUrl, trimSvg, tubeSvg } from "./frame-art";
import { computeFrameGeometry } from "./geometry";

/** Minimal well-formedness check: every element is closed in order, attributes quoted. */
function wellFormed(xml: string): boolean {
  const stack: string[] = [];
  const tag = /<(\/?)([a-zA-Z][\w:-]*)((?:\s+[\w:-]+="[^"<]*")*)\s*(\/?)>/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = tag.exec(xml)) !== null) {
    if (xml.slice(last, m.index).includes("<")) return false; // something that did not parse as a tag
    last = tag.lastIndex;
    const [, close, name, , self] = m;
    if (self) continue;
    if (close) {
      if (stack.pop() !== name) return false;
    } else stack.push(name);
  }
  return stack.length === 0 && !xml.slice(last).includes("<");
}

describe("frame artwork", () => {
  for (const [W, H] of [
    [1440, 900],
    [1280, 800],
    [3840, 2160],
  ]) {
    const g = computeFrameGeometry({
      width: W,
      height: H,
      screen: { x: W * 0.04, y: H * 0.09, w: W * 0.92, h: H * 0.735 },
      screenRadius: Math.min(W * 0.11, H * 0.175),
      consoleTop: H * 0.825,
    });
    it(`builds well-formed standalone SVG at ${W}x${H}`, () => {
      for (const svg of [roomSvg(g), tubeSvg(g), trimSvg(g, 180)]) {
        expect(svg.startsWith('<svg xmlns="http://www.w3.org/2000/svg"')).toBe(true);
        expect(svg).toContain(`width="${W}" height="${H}"`);
        expect(svg).not.toMatch(/NaN|undefined|Infinity/);
        expect(wellFormed(svg)).toBe(true);
      }
    });
  }

  it("encodes a data: URL that survives CSS and HTML attribute contexts", () => {
    const url = svgUrl('<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0" fill="url(#a)"/></svg>');
    expect(url.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
    expect(url).not.toMatch(/[<>"#\s]/);
  });
});

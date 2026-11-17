// SPDX-License-Identifier: GPL-3.0-or-later
// The Safari 16.4 CSS check (scripts/check-dist-css.mjs) must really catch newer
// features, and must accept them when a fallback is present.
import { describe, expect, it } from "vitest";
import { checkCssText } from "../scripts/check-dist-css.mjs";

describe("CSS baseline check (Safari 16.4 / Chrome 120)", () => {
  it("rejects features newer than the baseline", () => {
    const bad = [
      ".a { color: light-dark(#000, #fff); }",
      "h1 { text-wrap: balance; }",
      ".a { .b { color: red; } }",
      "@starting-style { .a { opacity: 0; } }",
      "input:user-invalid { color: red; }",
      ".a { content-visibility: auto; }",
      ".a { backdrop-filter: blur(4px); }",
      ".a { field-sizing: content; }",
    ];
    for (const css of bad) expect(checkCssText(css, "test.css"), css).not.toEqual([]);
  });

  it("accepts baseline features and proper fallbacks", () => {
    const good = [
      ".a:focus-visible { outline: 2px solid; } .b:has(> img) { gap: 1rem; }",
      ".a { inset: 0; aspect-ratio: 1; accent-color: red; font-size: clamp(1rem, 2vw, 3rem); }",
      "@container (min-width: 30rem) { .a { color: red; } }",
      ".a { color: #000; color: light-dark(#000, #fff); }",
      "@supports (text-wrap: balance) { h1 { text-wrap: balance; } }",
      ".a { -webkit-backdrop-filter: blur(4px); backdrop-filter: blur(4px); }",
      ".a { height: 100dvh; }",
    ];
    for (const css of good) expect(checkCssText(css, "test.css"), css).toEqual([]);
  });
});

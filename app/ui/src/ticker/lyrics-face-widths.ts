// SPDX-License-Identifier: GPL-3.0-or-later
// Advance widths of the skin's lyrics face, Atkinson Hyperlegible Next at weight 700 (SIL OFL,
// bundled in src/assets/fonts), for tests that need the real app's text widths without a
// browser. Measured with CanvasRenderingContext2D.measureText at 100 px in headless Microsoft
// Edge 157.0.4341.60 (the WebView2 engine) from the bundled woff2; advances scale linearly with
// the font size (26 letters at 33.6 px: 458.42 px measured, 458.56 px scaled). Covers printable
// ASCII, Latin-1 letters and typographic quotes and dashes; other graphemes fall back to
// `fallback`. Units: thousandths of an em.
import type { Measure } from "./layout";

const CHARS = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖ×ØÙÚÛÜÝÞßàáâãäåæçèéêëìíîïðñòóôõö÷øùúûüýþÿ‘’“”–—…";
const ADVANCE = [
  312, 303, 390, 765, 610, 969, 725, 210, 335, 335, 485, 595, 271, 373, 272, 440,
  659, 440, 579, 600, 624, 607, 626, 537, 624, 571, 272, 271, 569, 612, 569, 547,
  781, 678, 643, 658, 688, 597, 566, 718, 701, 424, 561, 663, 552, 858, 717, 746,
  629, 756, 650, 610, 607, 686, 644, 886, 685, 650, 618, 340, 440, 344, 582, 400,
  288, 552, 596, 510, 596, 563, 365, 593, 571, 300, 259, 555, 318, 882, 571, 576,
  596, 599, 376, 508, 376, 567, 530, 740, 539, 521, 505, 381, 254, 381, 537, 678,
  678, 678, 678, 678, 678, 966, 658, 597, 597, 597, 597, 424, 424, 424, 424, 708,
  717, 746, 746, 746, 746, 746, 536, 746, 686, 686, 686, 686, 650, 629, 621, 552,
  552, 552, 552, 552, 552, 878, 510, 563, 563, 563, 563, 300, 300, 300, 314, 588,
  571, 576, 576, 576, 576, 576, 595, 576, 567, 567, 567, 567, 521, 596, 521, 269,
  269, 479, 479, 501, 832, 780,
];

const TABLE = new Map<string, number>([...CHARS].map((c, i) => [c, ADVANCE[i] / 1000]));

/** Width of a grapheme in the lyrics face; `fallback` for graphemes outside the table. */
export function lyricsFaceMeasure(fallback: Measure): Measure {
  return (text, fontPx) => {
    let em = 0;
    for (const ch of text) {
      const a = TABLE.get(ch);
      if (a === undefined) return fallback(text, fontPx);
      em += a;
    }
    return em * fontPx;
  };
}

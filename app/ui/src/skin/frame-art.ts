// SPDX-License-Identifier: GPL-3.0-or-later
// The frame's static artwork as standalone SVG documents (strings), shown through
// <img src="data:image/svg+xml,...">.
//
// Why images and not inline SVG: an engine keeps the rendered raster of an SVG
// *image* and reuses it, but inline SVG is vector content that gets re-rendered,
// blur filters and all, whenever anything underneath it is repainted. In a web
// view that does not composite the drifting light on its own layer (measured with
// Playwright's WebKit build for Windows: the page fell to ~4 frames per second with
// inline SVG), that is the difference between a cheap blit and re-running every
// filter of the frame 12 times a second. With images the art costs one rasterisation
// per window size everywhere.
//
// Pure string builders (no DOM), so they are unit-testable.

import { centreLinePath, enclosedPath, type FrameGeometry } from "./geometry";

/** Number formatting for attributes: 2 decimals, no trailing noise. */
function n(v: number): string {
  return String(Math.round(v * 100) / 100);
}

/** `<tag a="1" b="2"/>` from an attribute object (values are numbers or safe strings). */
function el(tag: string, attrs: Record<string, string | number>, children = ""): string {
  const a = Object.entries(attrs)
    .map(([k, v]) => ` ${k}="${typeof v === "number" ? n(v) : v}"`)
    .join("");
  return children ? `<${tag}${a}>${children}</${tag}>` : `<${tag}${a}/>`;
}

function svgDoc(g: FrameGeometry, defs: string, body: string): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${n(g.width)}" height="${n(g.height)}" viewBox="0 0 ${n(g.width)} ${n(g.height)}">` +
    `<defs>${defs}</defs>${body}</svg>`
  );
}

function stops(list: [number, string][]): string {
  return list.map(([o, c]) => el("stop", { offset: o, "stop-color": c })).join("");
}

function linear(id: string, horizontal: boolean, list: [number, string][]): string {
  return el("linearGradient", { id, x1: 0, y1: 0, x2: horizontal ? 1 : 0, y2: horizontal ? 0 : 1 }, stops(list));
}

function blur(id: string, sd: number, margin = "-5%", size = "110%"): string {
  return el("filter", { id, x: margin, y: margin, width: size, height: size }, el("feGaussianBlur", { stdDeviation: Math.max(0.1, sd) }));
}

/** A stroke along the tube centre line offset by `offset`. */
function along(g: FrameGeometry, offset: number, attrs: Record<string, string | number>): string {
  return el("path", { d: centreLinePath(g, offset), fill: "none", ...attrs });
}

/** As a data: URL for <img src>. */
export function svgUrl(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** The dark room outside the arch (photo 1: the jukebox is the light source). */
export function roomSvg(g: FrameGeometry): string {
  const defs = linear("room", false, [
    [0, "#0a0610"],
    [1, "#140a12"],
  ]);
  const d = `M0 0H${n(g.width)}V${n(g.height)}H0Z ${enclosedPath(g, g.tube / 2 - 0.5)}`;
  return svgDoc(g, defs, el("path", { d, fill: "url(#room)", "fill-rule": "evenodd" }));
}

/**
 * Shading of the frosted tube, drawn over the moving light and under the bubbles:
 * darker towards both edges, a bright core, the clear inner bubble tube, and a
 * dark seam plus a thin chrome line along both rims.
 */
export function tubeSvg(g: FrameGeometry): string {
  const T = g.tube;
  const bt = g.bubbleTube;
  const defs = blur("edge", T * 0.11) + blur("core", T * 0.13);
  const body = [
    along(g, T * 0.38, { stroke: "#12040a", "stroke-opacity": 0.6, "stroke-width": T * 0.26, filter: "url(#edge)" }),
    along(g, -T * 0.38, { stroke: "#12040a", "stroke-opacity": 0.6, "stroke-width": T * 0.26, filter: "url(#edge)" }),
    along(g, T * 0.02, { stroke: "#ffffff", "stroke-opacity": 0.26, "stroke-width": T * 0.32, filter: "url(#core)" }),
    along(g, 0, { stroke: "#ffffff", "stroke-opacity": 0.1, "stroke-width": bt + 2 }),
    along(g, 0, { stroke: "#3a0626", "stroke-opacity": 0.2, "stroke-width": bt }),
    along(g, bt / 2, { stroke: "#ffffff", "stroke-opacity": 0.26, "stroke-width": 0.7 }),
    along(g, -bt / 2, { stroke: "#ffffff", "stroke-opacity": 0.26, "stroke-width": 0.7 }),
    along(g, T / 2, { stroke: "#140905", "stroke-width": 3 }),
    along(g, -T / 2, { stroke: "#140905", "stroke-width": 3 }),
    along(g, T / 2 - 1, { stroke: "#ece7e0", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
    along(g, -T / 2 + 1, { stroke: "#ece7e0", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
  ].join("");
  return svgDoc(g, defs, body);
}

/** Chrome ring stack with red enamel bands (photo 2), centred on x, starting at y. */
function ringStack(g: FrameGeometry, x: number, y: number): string {
  const T = g.tube;
  const ov = T * 0.17;
  const ringW = T + 2 * ov;
  const h1 = g.ringHeight / 5;
  const h2 = g.ringHeight / 5;
  const enW = T + ov * 0.5;
  let out =
    el("rect", { x: x - ringW / 2 - 1, y: y + 1, width: ringW + 2, height: g.ringHeight + 3, rx: h1 * 0.4, fill: "#000", opacity: 0.5, filter: "url(#soft)" }) +
    // Finial: a small chrome dome on top.
    el("ellipse", { cx: x, cy: y + 0.5, rx: T * 0.3, ry: T * 0.2, fill: "url(#dome)" });
  for (let i = 0; i < 3; i++) {
    const ry = y + i * (h1 + h2);
    out +=
      el("rect", { x: x - ringW / 2, y: ry, width: ringW, height: h1, rx: h1 * 0.3, fill: "url(#ring)" }) +
      el("rect", { x: x - ringW / 2, y: ry, width: ringW, height: h1, rx: h1 * 0.3, fill: "url(#ribs)" }) +
      // Stepped chrome ends.
      el("rect", { x: x - ringW / 2 - ov * 0.25, y: ry + h1 * 0.18, width: ov * 0.5, height: h1 * 0.64, rx: h1 * 0.2, fill: "url(#ring)" }) +
      el("rect", { x: x + ringW / 2 - ov * 0.25, y: ry + h1 * 0.18, width: ov * 0.5, height: h1 * 0.64, rx: h1 * 0.2, fill: "url(#ring)" }) +
      el("rect", { x: x - ringW / 2, y: ry + h1 * 0.08, width: ringW, height: Math.max(0.6, h1 * 0.1), fill: "#fff", opacity: 0.75 });
    if (i < 2) {
      const ey = ry + h1;
      out +=
        el("rect", { x: x - enW / 2, y: ey, width: enW, height: h2, fill: "url(#enamel)" }) +
        el("rect", { x: x - enW / 2, y: ey + h2 * 0.16, width: enW, height: Math.max(0.6, h2 * 0.12), fill: "#fff", opacity: 0.45 });
    }
  }
  return out;
}

/** Walnut base block with chrome bands at the foot of a column. */
function base(g: FrameGeometry, x: number): string {
  const T = g.tube;
  const w = T * 1.5;
  const top = g.baseTop;
  const h = g.height - top + 2;
  const band = Math.max(2, T * 0.14);
  return (
    el("rect", { x: x - w / 2 - 1, y: top - 2, width: w + 2, height: h + 2, rx: T * 0.16, fill: "#000", opacity: 0.55, filter: "url(#soft)" }) +
    el("rect", { x: x - w / 2, y: top, width: w, height: h, rx: T * 0.14, fill: "url(#walnut)" }) +
    el("rect", { x: x - w / 2 - 1, y: top, width: w + 2, height: band, rx: band * 0.4, fill: "url(#ring)" }) +
    el("rect", { x: x - w / 2 - 1, y: top, width: w + 2, height: band, rx: band * 0.4, fill: "url(#ribs)" }) +
    el("rect", { x: x - w / 2, y: top + h * 0.42, width: w, height: band * 0.55, fill: "url(#ring)" }) +
    el("rect", { x: x - w / 2, y: top + band, width: w, height: band * 0.8, fill: "#000", opacity: 0.35 })
  );
}

/** y of the tube centre line at x (top part of the arch). */
export function archY(g: FrameGeometry, x: number): number {
  if (g.bigR <= 0) return g.cornerCy - g.cornerR;
  const dx = x - g.width / 2;
  return g.bigCy - Math.sqrt(Math.max(0, g.bigR * g.bigR - dx * dx));
}

/**
 * Chrome clamps either side of the crest: the same ring + red enamel motif as the
 * column tops, wrapped round the arch tube where the crest is fixed to it.
 * (Deliberately not scrolls or wings: no maker's badge shape.)
 */
function crestClamps(g: FrameGeometry, crestW: number): string {
  const T = g.tube;
  const band = T * 0.17;
  const len = band * 5.4;
  const ov = T * 0.14;
  const h = T + 2 * ov;
  let out = "";
  for (const sgn of [-1, 1]) {
    const x = g.width / 2 + sgn * (crestW * 0.5 + len * 0.62);
    const y = archY(g, x);
    const angle = (Math.atan2(x - g.width / 2, g.bigR > 0 ? g.bigCy - y : 1e9) * 180) / Math.PI;
    let inner =
      el("rect", { x: -len / 2 - 1, y: -h / 2 + 1, width: len + 2, height: h + 2, rx: band * 0.4, fill: "#000", opacity: 0.5, filter: "url(#soft)" }) +
      el("rect", { x: -len / 2 + band, y: -T / 2 - ov * 0.4, width: len - 2 * band, height: T + ov * 0.8, fill: "url(#enamelH)" });
    for (const bx of [-len / 2, len / 2 - band, -band / 2]) {
      inner +=
        el("rect", { x: bx, y: -h / 2, width: band, height: h, rx: band * 0.3, fill: "url(#ringH)" }) +
        el("rect", { x: bx + band * 0.12, y: -h / 2, width: Math.max(0.6, band * 0.12), height: h, fill: "#fff", opacity: 0.7 });
    }
    out += el("g", { transform: `translate(${n(x)} ${n(y)}) rotate(${angle.toFixed(3)})` }, inner);
  }
  return out;
}

/**
 * Everything in front of the bubbles: glass reflections on the tube, the chrome
 * ring stacks, the column bases and the crest clamps.
 */
export function trimSvg(g: FrameGeometry, crestW: number): string {
  const T = g.tube;
  const ribW = Math.max(2.5, T * 0.075);
  const defs =
    linear("ring", false, [
      [0, "#ffffff"],
      [0.25, "#cfd2d9"],
      [0.5, "#5c5f67"],
      [0.62, "#9599a2"],
      [0.85, "#eef0f3"],
      [1, "#6d7078"],
    ]) +
    el(
      "pattern",
      { id: "ribs", width: ribW, height: 10, patternUnits: "userSpaceOnUse" },
      el("rect", { width: Math.max(0.8, T * 0.02), height: 10, fill: "#000", opacity: 0.28 }),
    ) +
    linear("enamel", false, [
      [0, "#f03752"],
      [0.5, "#c8102e"],
      [1, "#5e0513"],
    ]) +
    el("radialGradient", { id: "dome", cx: 0.4, cy: 0.25, r: 0.8 }, stops([
      [0, "#ffffff"],
      [0.5, "#b5b9c1"],
      [1, "#4a4d54"],
    ])) +
    linear("walnut", true, [
      [0, "#1f1009"],
      [0.35, "#6a4326"],
      [0.55, "#5a3a22"],
      [1, "#21120a"],
    ]) +
    linear("ringH", true, [
      [0, "#ffffff"],
      [0.3, "#c9ccd3"],
      [0.55, "#55585f"],
      [0.8, "#eceef1"],
      [1, "#7a7d85"],
    ]) +
    linear("enamelH", false, [
      [0, "#7a0618"],
      [0.3, "#f03752"],
      [0.55, "#c8102e"],
      [1, "#5e0513"],
    ]) +
    blur("soft", Math.max(1, T * 0.06), "-20%", "140%");
  const body =
    // Glass reflections on the front of the tube.
    along(g, -T * 0.24, { stroke: "#ffffff", "stroke-opacity": 0.07, "stroke-width": T * 0.14 }) +
    along(g, -T * 0.2, { stroke: "#ffffff", "stroke-opacity": 0.72, "stroke-width": Math.max(1, T * 0.035) }) +
    ringStack(g, g.xL, g.ringTop) +
    ringStack(g, g.width - g.xL, g.ringTop) +
    base(g, g.xL) +
    base(g, g.width - g.xL) +
    crestClamps(g, crestW);
  return svgDoc(g, defs, body);
}

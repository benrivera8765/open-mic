// SPDX-License-Identifier: GPL-3.0-or-later
// Geometry of the jukebox frame (docs/DESIGN.md, "Window anatomy").
//
// Everything decorative around the screen is derived from ONE curve: the centre
// line of the glowing tube. It runs up the left column, round the left shoulder,
// over a flat "three-centred" arch to the crest, and down the right side as a
// mirror image. The tube, its bloom, its chrome rims, the inner bubble tube and
// the bubbles' track are all exact offsets of that curve (lines shift, arcs keep
// their centre and change radius), so they stay concentric at every window size.
//
// The arch (left half, y grows downwards):
//
//        apex (W/2, apexY) -----------  big arc, centre C2 = (W/2, apexY + bigR)
//      /
//     (  corner arc, centre C1 = top-left corner centre of the screen
//     |  (concentric with the screen's rounded corner, so the gap stays constant)
//     |  vertical line x = xL down to the base
//
// The corner arc and the big arc are internally tangent at the junction J, which
// lies on the line C2 -> C1, so the curve is smooth (no kink) and its curvature
// only steps once - the classic basket-handle arch.
//
// Pure functions, no DOM: unit-tested in geometry.test.ts.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** What the frame component measures from its CSS layout (all CSS px, frame-relative). */
export interface FrameInput {
  width: number;
  height: number;
  /** The screen cell (outer edge of its chrome bezel). Must be horizontally centred. */
  screen: Rect;
  /** Top-corner radius of the screen cell, from CSS. */
  screenRadius: number;
  /** Top edge of the console row. */
  consoleTop: number;
}

export interface FrameGeometry {
  width: number;
  height: number;
  /** Outer margin between the window edge and the tube. */
  margin: number;
  /** Tube thickness. */
  tube: number;
  /** Gap between the tube and the screen bezel. */
  gap: number;
  /** x of the left tube centre line (the right one is width - xL). */
  xL: number;
  /** Corner arc: centre (cornerCx, cornerCy), radius cornerR (centre line). */
  cornerCx: number;
  cornerCy: number;
  cornerR: number;
  /** Big arc: centre (width / 2, bigCy), radius bigR. 0 when the top is straight. */
  bigCy: number;
  bigR: number;
  /** Angle (radians, y down, in (PI, 1.5 PI]) of the junction on both circles. */
  junctionAngle: number;
  /** y of the tube centre line at the crest. */
  apexY: number;
  /** Chrome ring stack at the top of each column. */
  ringTop: number;
  ringHeight: number;
  /** Light regions split here (hidden under the middle of the ring stack). */
  junctionY: number;
  /** Where the tube ends at the bottom (top of the column base block). */
  baseTop: number;
  /** Width of the inner bubble tube. */
  bubbleTube: number;
  /** Period (CSS px) of the palette gradient that drifts through the tubes. */
  lightPeriod: number;
  /** Arc length of the left half of the centre line (base -> crest). */
  halfLength: number;
}

const THREE_HALF_PI = 1.5 * Math.PI;

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** Rounds to 1/100 px so path strings stay short and stable. */
function r2(v: number): number {
  return Math.round(v * 100) / 100;
}

export function computeFrameGeometry(input: FrameInput): FrameGeometry {
  const { width: W, height: H, screen, consoleTop } = input;
  const side = Math.max(8, screen.x);
  // Split the side column: margin | tube | gap. Proportions chosen by eye
  // against the reference photos (tube dominant, thin dark seam each side).
  const margin = clamp(side * 0.07, 1.5, 6);
  const gap = clamp(side * 0.17, 4, 30);
  const tube = Math.max(4, side - margin - gap);
  const xL = margin + tube / 2;
  const d = screen.x - xL; // screen edge -> tube centre line

  const rs = clamp(input.screenRadius, 0, Math.min(screen.w, screen.h) / 2);
  const cornerCx = screen.x + rs;
  const cornerCy = screen.y + rs;
  const cornerR = rs + d;

  // Crest: the tube's outer edge touches the top margin at the centre.
  const apexY = margin + tube / 2;
  const cornerTopY = cornerCy - cornerR; // y of the corner circle's top point
  const rise = cornerTopY - apexY;

  let bigR = 0;
  let bigCy = 0;
  let junctionAngle = THREE_HALF_PI;
  if (rise > 0.5) {
    // Internally tangent circles: |C2 - C1| = bigR - cornerR, with
    // C2 = (W/2, apexY + bigR). Solving for bigR:
    //   dx^2 + (bigR - e)^2 = (bigR - cornerR)^2,  e = cornerCy - apexY
    //   bigR = (dx^2 + e^2 - cornerR^2) / (2 (e - cornerR))
    const dx = W / 2 - cornerCx;
    const e = cornerCy - apexY;
    bigR = (dx * dx + e * e - cornerR * cornerR) / (2 * (e - cornerR));
    bigCy = apexY + bigR;
    // Junction direction: from C2 towards C1 (third quadrant in y-down coordinates).
    let a = Math.atan2(cornerCy - bigCy, cornerCx - W / 2);
    if (a < 0) a += 2 * Math.PI;
    junctionAngle = a;
  }

  // Chrome ring stack sits on the straight part of the column, just below the shoulder.
  const ringHeight = tube * 1.05;
  const ringTop = cornerCy + Math.max(2, tube * 0.08);
  const junctionY = ringTop + ringHeight / 2;

  // The base block takes the lower part of the console row.
  const consoleH = Math.max(0, H - consoleTop);
  const baseTop = Math.max(junctionY + ringHeight, H - Math.max(consoleH * 0.5, tube * 1.3));

  const bubbleTube = clamp(tube * 0.34, 6, 44);

  // Arc length of the left half: vertical + corner arc + big arc (or straight top).
  const vertical = Math.max(0, baseTop - cornerCy);
  const cornerArc = cornerR * (junctionAngle - Math.PI);
  const topPart =
    bigR > 0 ? bigR * (THREE_HALF_PI - junctionAngle) : Math.max(0, W / 2 - cornerCx);
  const halfLength = vertical + cornerArc + topPart;

  // Palette period: long enough that the arch half and a column together show
  // roughly two thirds of the palette at once (photo 1: pink/red arch, green ->
  // yellow -> orange columns), and never shorter than a light region.
  const lightPeriod = Math.round(Math.max(W / 2 + 16, H, halfLength * 1.35));

  return {
    width: W,
    height: H,
    margin,
    tube,
    gap,
    xL,
    cornerCx,
    cornerCy,
    cornerR,
    bigCy,
    bigR,
    junctionAngle,
    apexY,
    ringTop,
    ringHeight,
    junctionY,
    baseTop,
    bubbleTube,
    lightPeriod,
    halfLength,
  };
}

/**
 * SVG path data of the tube centre line offset by `offset` (positive = outwards,
 * away from the screen), from the bottom of the left column over the crest to the
 * bottom of the right column. `bottom` overrides where the vertical parts end.
 */
export function centreLinePath(g: FrameGeometry, offset: number, bottom = g.baseTop): string {
  const W = g.width;
  const o = offset;
  const xl = g.xL - o;
  const xr = W - g.xL + o;
  const rc = g.cornerR + o;
  const parts: string[] = [];
  parts.push(`M${r2(xl)} ${r2(bottom)}`, `L${r2(xl)} ${r2(g.cornerCy)}`);
  if (g.bigR > 0) {
    const rb = g.bigR + o;
    const ca = Math.cos(g.junctionAngle);
    const sa = Math.sin(g.junctionAngle);
    // Left junction, apex, right junction (mirror), then right corner.
    const jx = g.cornerCx + rc * ca;
    const jy = g.cornerCy + rc * sa;
    const ay = g.bigCy - rb;
    parts.push(
      `A${r2(rc)} ${r2(rc)} 0 0 1 ${r2(jx)} ${r2(jy)}`,
      `A${r2(rb)} ${r2(rb)} 0 0 1 ${r2(W / 2)} ${r2(ay)}`,
      `A${r2(rb)} ${r2(rb)} 0 0 1 ${r2(W - jx)} ${r2(jy)}`,
      `A${r2(rc)} ${r2(rc)} 0 0 1 ${r2(xr)} ${r2(g.cornerCy)}`,
    );
  } else {
    const ty = g.cornerCy - rc;
    parts.push(
      `A${r2(rc)} ${r2(rc)} 0 0 1 ${r2(g.cornerCx)} ${r2(ty)}`,
      `L${r2(W - g.cornerCx)} ${r2(ty)}`,
      `A${r2(rc)} ${r2(rc)} 0 0 1 ${r2(xr)} ${r2(g.cornerCy)}`,
    );
  }
  parts.push(`L${r2(xr)} ${r2(bottom)}`);
  return parts.join("");
}

/**
 * Closed path of the region enclosed by the centre line offset by `offset`
 * (the cabinet face when offset = -tube/2, the whole lit silhouette when
 * offset = +tube/2), closed along the bottom of the frame.
 */
export function enclosedPath(g: FrameGeometry, offset: number): string {
  return `${centreLinePath(g, offset, g.height)}L${r2(g.xL - offset)} ${r2(g.height)}Z`;
}

// ---------------------------------------------------------------------------
// Arc-length parametrisation of the left half (bubbles travel along it).

export type Segment =
  | { kind: "line"; x0: number; y0: number; x1: number; y1: number; len: number }
  | { kind: "arc"; cx: number; cy: number; r: number; a0: number; a1: number; len: number };

/** A point on a path plus the unit normal pointing to the left of the direction of travel. */
export interface PathPoint {
  x: number;
  y: number;
  nx: number;
  ny: number;
}

/**
 * A polyline of lines and arcs, parametrised by arc length. `at()` writes into a
 * caller-owned object, so the per-frame bubble update allocates nothing.
 */
export class TubePath {
  readonly segments: readonly Segment[];
  readonly length: number;
  private readonly starts: number[];
  /** Mirror x around this value (right half) when not null. */
  private readonly mirrorAt: number | null;

  constructor(segments: Segment[], mirrorAt: number | null = null) {
    this.segments = segments;
    this.mirrorAt = mirrorAt;
    this.starts = [];
    let s = 0;
    for (const seg of segments) {
      this.starts.push(s);
      s += seg.len;
    }
    this.length = s;
  }

  /** Position and normal at arc length `s` (clamped to the path). */
  at(s: number, out: PathPoint): PathPoint {
    const segs = this.segments;
    if (segs.length === 0) {
      out.x = out.y = out.nx = out.ny = 0;
      return out;
    }
    const sc = s < 0 ? 0 : s > this.length ? this.length : s;
    let i = segs.length - 1;
    while (i > 0 && this.starts[i] > sc) i--;
    const seg = segs[i];
    const local = sc - this.starts[i];
    if (seg.kind === "line") {
      const t = seg.len > 0 ? local / seg.len : 0;
      const tx = seg.len > 0 ? (seg.x1 - seg.x0) / seg.len : 0;
      const ty = seg.len > 0 ? (seg.y1 - seg.y0) / seg.len : -1;
      out.x = seg.x0 + (seg.x1 - seg.x0) * t;
      out.y = seg.y0 + (seg.y1 - seg.y0) * t;
      // Normal = tangent rotated by -90 degrees (left of travel, y down).
      out.nx = ty;
      out.ny = -tx;
    } else {
      const t = seg.len > 0 ? local / seg.len : 0;
      const a = seg.a0 + (seg.a1 - seg.a0) * t;
      const c = Math.cos(a);
      const sn = Math.sin(a);
      out.x = seg.cx + seg.r * c;
      out.y = seg.cy + seg.r * sn;
      // Radial direction; for a clockwise (increasing angle) arc the left normal points outwards.
      const dir = seg.a1 >= seg.a0 ? 1 : -1;
      out.nx = c * dir;
      out.ny = sn * dir;
    }
    if (this.mirrorAt !== null) {
      out.x = 2 * this.mirrorAt - out.x;
      out.nx = -out.nx;
    }
    return out;
  }
}

/**
 * The bubble track for one side: from `startY` (normally the column base) up the
 * column, round the shoulder and over the arch to the crest.
 */
export function bubbleTrack(g: FrameGeometry, side: "left" | "right", startY = g.baseTop, offset = 0): TubePath {
  // offset: as centreLinePath (positive = outwards, away from the screen); the pilaster
  // frame style runs two small bubble tubes either side of the centre line.
  const segs: Segment[] = [];
  const x = g.xL - offset;
  const cornerR = g.cornerR + offset;
  const bigR = g.bigR > 0 ? g.bigR + offset : 0;
  const vLen = Math.max(0, startY - g.cornerCy);
  segs.push({ kind: "line", x0: x, y0: startY, x1: x, y1: g.cornerCy, len: vLen });
  const a1 = g.bigR > 0 ? g.junctionAngle : THREE_HALF_PI;
  segs.push({
    kind: "arc",
    cx: g.cornerCx,
    cy: g.cornerCy,
    r: cornerR,
    a0: Math.PI,
    a1,
    len: cornerR * (a1 - Math.PI),
  });
  if (g.bigR > 0) {
    segs.push({
      kind: "arc",
      cx: g.width / 2,
      cy: g.bigCy,
      r: bigR,
      a0: g.junctionAngle,
      a1: THREE_HALF_PI,
      len: bigR * (THREE_HALF_PI - g.junctionAngle),
    });
  } else {
    const y = g.cornerCy - cornerR;
    const len = Math.max(0, g.width / 2 - g.cornerCx);
    segs.push({ kind: "line", x0: g.cornerCx, y0: y, x1: g.width / 2, y1: y, len });
  }
  return new TubePath(segs, side === "right" ? g.width / 2 : null);
}

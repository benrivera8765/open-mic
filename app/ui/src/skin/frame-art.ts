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
export function roomSvg(g: FrameGeometry, waterBacking: boolean | "fire" = false): string {
  const fire = waterBacking === "fire";
  const defs =
    linear("room", false, [
      [0, "#0a0610"],
      [1, "#140a12"],
    ]) + (fire ? blur("halo", g.tube * 0.22) : "");
  const d = `M0 0H${n(g.width)}V${n(g.height)}H0Z ${enclosedPath(g, g.tube / 2 - 0.5)}`;
  // The lightning tube's water is see-through: a dark, partly clear backing behind the
  // tube makes it read as clear blue water in glass instead of an opaque drink.
  const backing = fire
    ? // The fire pipe: a soft dark shadow round it, so the flame stands out from a bright
      // (lava) cabinet, and a dark inside, so the flame is the only light in it.
      along(g, 0, { stroke: "#120400", "stroke-opacity": 0.6, "stroke-width": g.tube * 1.9, filter: "url(#halo)" }) +
      along(g, 0, { stroke: "#070200", "stroke-opacity": 0.78, "stroke-width": g.tube })
    : waterBacking
      ? along(g, 0, { stroke: "#04141f", "stroke-opacity": 0.42, "stroke-width": g.tube })
      : "";
  return svgDoc(g, defs, el("path", { d, fill: "url(#room)", "fill-rule": "evenodd" }) + backing);
}

/**
 * The border round the screen: neon bubble tubes, pearl pilasters (owner's photo 7), a
 * lava lamp, or a lightning tube of blue water ("light", the name it was saved under first).
 */
export type FrameStyleName = "neon" | "pilaster" | "lava" | "light" | "ocean" | "fire" | "water" | "steam" | "data";

/**
 * Glass pipes whose contents are drawn on the GPU (PipeGL.tsx; D-272, D-275): fire, water
 * coursing through, steam, cyberpunk data. This art
 * draws only the clear glass, tinted by what is inside.
 */
export const GLASS_PIPES: Readonly<Partial<Record<FrameStyleName, string>>> = {
  fire: "#ffc890",
  water: "#9fe0ff",
  steam: "#f2f5f8",
  data: "#7ae8ff",
};
/** The bands between the chrome rings: silver chrome (owner, 2026-11-22) or the original red enamel. */
export type AccentName = "silver" | "red" | "iron" | "copper";
/** The warm metal of the rings, domes and bands: gold chrome or bronze (Molten). */
export type MetalName = "gold" | "bronze" | "chrome" | "brass";

/** The cabinet finish (the column bases match it). */
export type CabinetName = "walnut" | "maple" | "pearl" | "aquarium" | "forge" | "ocean" | "brass" | "cyber";

export interface FrameArtOptions {
  style?: FrameStyleName;
  accents?: AccentName;
  cabinet?: CabinetName;
  metal?: MetalName;
}

type Stops = [number, string][];

/** The warm metal of rings, domes and clamps (ring and ringH across, dome lit from top left). */
const METALS: Record<MetalName, { ring: Stops; dome: Stops; ringH: Stops }> = {
  gold: {
    ring: [[0, "#fff6d8"], [0.25, "#e9d087"], [0.5, "#815e20"], [0.62, "#c49940"], [0.85, "#f8e7b4"], [1, "#966f27"]],
    dome: [[0, "#fff6d8"], [0.5, "#debb68"], [1, "#694b16"]],
    ringH: [[0, "#fff6d8"], [0.3, "#e7cb80"], [0.55, "#77571c"], [0.8, "#f7e5b0"], [1, "#a67c2b"]],
  },
  bronze: {
    ring: [[0, "#ffe0c2"], [0.25, "#dc9a66"], [0.5, "#6a3414"], [0.62, "#b8703a"], [0.85, "#f2c094"], [1, "#8a4a20"]],
    dome: [[0, "#ffe0c2"], [0.5, "#c8804a"], [1, "#5a2c12"]],
    ringH: [[0, "#ffe0c2"], [0.3, "#d8945e"], [0.55, "#6a3414"], [0.8, "#f0bc8c"], [1, "#9a5424"]],
  },
  chrome: {
    ring: [[0, "#ffffff"], [0.25, "#d4dae0"], [0.5, "#5c646e"], [0.62, "#a8b0b9"], [0.85, "#f2f5f8"], [1, "#7a828c"]],
    dome: [[0, "#ffffff"], [0.5, "#b8c0c9"], [1, "#4a525b"]],
    ringH: [[0, "#ffffff"], [0.3, "#cfd6dd"], [0.55, "#5a626b"], [0.8, "#eef2f5"], [1, "#868e98"]],
  },
  brass: {
    ring: [[0, "#fff2c4"], [0.25, "#dcc070"], [0.5, "#6e5216"], [0.62, "#b4943c"], [0.85, "#f0dc98"], [1, "#806020"]],
    dome: [[0, "#fff2c4"], [0.5, "#c8a44a"], [1, "#5a4210"]],
    ringH: [[0, "#fff2c4"], [0.3, "#d4b660"], [0.55, "#6a4e14"], [0.8, "#ecd690"], [1, "#8c6a24"]],
  },
};

/** The bands between the rings (band across, bandH along). */
const ACCENTS: Record<AccentName, { band: Stops; bandH: Stops }> = {
  silver: {
    band: [[0, "#f6f8fa"], [0.28, "#c6cdd5"], [0.5, "#6a727c"], [0.6, "#a3abb5"], [0.84, "#eef2f5"], [1, "#7b838d"]],
    bandH: [[0, "#575f68"], [0.3, "#f2f5f8"], [0.55, "#8c949e"], [0.8, "#d9dee4"], [1, "#4b525a"]],
  },
  red: {
    band: [[0, "#f03752"], [0.5, "#c8102e"], [1, "#5e0513"]],
    bandH: [[0, "#7a0618"], [0.3, "#f03752"], [0.55, "#c8102e"], [1, "#5e0513"]],
  },
  iron: {
    band: [[0, "#9aa0a6"], [0.28, "#5a6066"], [0.5, "#1c1f22"], [0.6, "#3e4449"], [0.84, "#7a8086"], [1, "#24282c"]],
    bandH: [[0, "#1a1d20"], [0.3, "#8a9096"], [0.55, "#3a3f44"], [0.8, "#6a7076"], [1, "#16191c"]],
  },
  copper: {
    band: [[0, "#ffd2b0"], [0.28, "#e08a54"], [0.5, "#7a3412"], [0.6, "#c0683a"], [0.84, "#f6b890"], [1, "#8a4020"]],
    bandH: [[0, "#6a2a0c"], [0.3, "#f6b890"], [0.55, "#a8542a"], [0.8, "#eaa070"], [1, "#5a2408"]],
  },
};

/** Base block colours across its width (left to right) for each cabinet finish. */
const BASE_FINISH: Record<CabinetName, [number, string][]> = {
  walnut: [
    [0, "#1f1009"],
    [0.35, "#6a4326"],
    [0.55, "#5a3a22"],
    [1, "#21120a"],
  ],
  maple: [
    [0, "#7a5230"],
    [0.3, "#e2b880"],
    [0.55, "#d2a46a"],
    [1, "#80562f"],
  ],
  // Forge: blackened cast iron with the warmth of the fire showing through.
  forge: [
    [0, "#120a07"],
    [0.32, "#3a2418"],
    [0.52, "#2a1810"],
    [0.8, "#4a2414"],
    [1, "#0e0806"],
  ],
  // Ocean: deep sea-blue stone.
  ocean: [
    [0, "#041628"],
    [0.35, "#1a5480"],
    [0.55, "#123e62"],
    [1, "#03101e"],
  ],
  // Steampunk: riveted brass.
  brass: [
    [0, "#3a2a08"],
    [0.3, "#d8b860"],
    [0.5, "#a0802c"],
    [0.75, "#f0d890"],
    [1, "#4a3408"],
  ],
  // Cyberpunk: black with cyan and yellow edge lights.
  cyber: [
    [0, "#2ae0ff"],
    [0.08, "#0a141c"],
    [0.5, "#05080c"],
    [0.92, "#0a141c"],
    [1, "#ffd23a"],
  ],
  // Aquarium: dark slate stone, like the base of a fish tank stand.
  aquarium: [
    [0, "#0b1f2a"],
    [0.35, "#2a4c5c"],
    [0.55, "#20404e"],
    [1, "#0a1a24"],
  ],
  // Mother of pearl, lit warm from inside: rose, cream and peach with a cooler sheen.
  pearl: [
    [0, "#8a4a52"],
    [0.18, "#f3b9c4"],
    [0.36, "#fff1e6"],
    [0.5, "#f9d3e2"],
    [0.66, "#ffd6a8"],
    [0.84, "#f0a88c"],
    [1, "#7a3c3a"],
  ],
};

/**
 * The pilaster style's two small bubble tubes: their centre-line offset either side of
 * the tube's centre line and their width (CSS px). Shared with the bubble layer.
 */
export function pilasterChannels(g: FrameGeometry): { offset: number; width: number } {
  const T = g.tube;
  return { offset: T * 0.27, width: Math.max(4, Math.min(26, T * 0.145)) };
}

/**
 * Pearl pilaster (photo 7, re-coloured). The colour is the drifting light underneath
 * (every jukebox colour in a pearly tint); this layer only adds pearl lustre (soft white sheens on both flanks of
 * each tube and between them, a faint lilac / mint play at the edges) and two small
 * bubble tubes of glowing amber liquid (opaque, so the pearl's colour never tints them),
 * darker where the glass curves away, a warm core, a soft glint on one side, and a halo
 * that melts into the pearl.
 */
function pilasterTubeSvg(g: FrameGeometry): string {
  const T = g.tube;
  const { offset: co, width: cw } = pilasterChannels(g);
  const defs =
    blur("edge", T * 0.07) +
    blur("core", T * 0.1) +
    blur("glow", Math.max(0.8, cw * 0.45), "-20%", "140%") +
    blur("neon", Math.max(0.5, cw * 0.22)) +
    blur("spec", Math.max(0.4, cw * 0.12));
  const parts: string[] = [
    along(g, 0, { stroke: "#fff6f0", "stroke-opacity": 0.2, "stroke-width": T * 0.96 }),
    along(g, T * 0.4, { stroke: "#ffffff", "stroke-opacity": 0.3, "stroke-width": T * 0.14, filter: "url(#core)" }),
    along(g, T * 0.12, { stroke: "#ffffff", "stroke-opacity": 0.2, "stroke-width": T * 0.12, filter: "url(#core)" }),
    along(g, -T * 0.12, { stroke: "#ffffff", "stroke-opacity": 0.16, "stroke-width": T * 0.12, filter: "url(#core)" }),
    along(g, -T * 0.4, { stroke: "#ffffff", "stroke-opacity": 0.26, "stroke-width": T * 0.14, filter: "url(#core)" }),
    along(g, T * 0.44, { stroke: "#dcc8ff", "stroke-opacity": 0.14, "stroke-width": T * 0.08, filter: "url(#core)" }),
    along(g, -T * 0.44, { stroke: "#c6ffe9", "stroke-opacity": 0.1, "stroke-width": T * 0.08, filter: "url(#core)" }),
    along(g, T * 0.47, { stroke: "#3a1006", "stroke-opacity": 0.42, "stroke-width": T * 0.08, filter: "url(#edge)" }),
    along(g, -T * 0.47, { stroke: "#3a1006", "stroke-opacity": 0.38, "stroke-width": T * 0.08, filter: "url(#edge)" }),
  ];
  for (const s of [1, -1]) {
    const o = s * co;
    // Outer tube a little redder, inner one more amber, as in the photo.
    const body = s > 0 ? "#ff520e" : "#ff7812";
    const core = s > 0 ? "#ffa030" : "#ffbe3c";
    parts.push(
      // Pearl hugging the tube on both sides.
      along(g, o + cw * 1.05, { stroke: "#ffffff", "stroke-opacity": 0.2, "stroke-width": cw * 0.7, filter: "url(#core)" }),
      along(g, o - cw * 1.05, { stroke: "#ffffff", "stroke-opacity": 0.2, "stroke-width": cw * 0.7, filter: "url(#core)" }),
      // The liquid.
      along(g, o, { stroke: "#ff7a1a", "stroke-opacity": 0.38, "stroke-width": cw * 1.6, filter: "url(#glow)" }),
      // Opaque: the tubes keep their amber whatever colour the pearl drifts through.
      along(g, o, { stroke: body, "stroke-width": cw }),
      along(g, o + cw * 0.36, { stroke: "#7a1a04", "stroke-opacity": 0.45, "stroke-width": cw * 0.26, filter: "url(#spec)" }),
      along(g, o - cw * 0.36, { stroke: "#7a1a04", "stroke-opacity": 0.45, "stroke-width": cw * 0.26, filter: "url(#spec)" }),
      along(g, o, { stroke: core, "stroke-opacity": 0.85, "stroke-width": cw * 0.5, filter: "url(#neon)" }),
      along(g, o - cw * 0.2, { stroke: "#fff3d6", "stroke-opacity": 0.28, "stroke-width": cw * 0.16, filter: "url(#spec)" }),
    );
  }
  parts.push(
    along(g, T / 2, { stroke: "#1a0c04", "stroke-width": 3.4 }),
    along(g, -T / 2, { stroke: "#1a0c04", "stroke-width": 3.4 }),
    along(g, T / 2 - 1.1, { stroke: "#f3dc95", "stroke-opacity": 0.95, "stroke-width": 1.5 }),
    along(g, -T / 2 + 1.1, { stroke: "#f3dc95", "stroke-opacity": 0.95, "stroke-width": 1.5 }),
    along(g, T / 2 - 2.2, { stroke: "#7a5414", "stroke-opacity": 0.6, "stroke-width": 0.6 }),
    along(g, -T / 2 + 2.2, { stroke: "#7a5414", "stroke-opacity": 0.6, "stroke-width": 0.6 }),
  );
  return svgDoc(g, defs, parts.join(""));
}

/**
 * Lava lamp: a glass tube of deep coloured liquid (the drifting light underneath, in
 * the lava palette), darker towards the walls where it is deepest, with soft glass
 * reflections. The wax blobs are drawn by LavaLamp.tsx on top of this.
 */
function lavaTubeSvg(g: FrameGeometry): string {
  const T = g.tube;
  const defs = blur("edge", T * 0.1) + blur("core", T * 0.14);
  const body = [
    along(g, T * 0.42, { stroke: "#12020c", "stroke-opacity": 0.45, "stroke-width": T * 0.2, filter: "url(#edge)" }),
    along(g, -T * 0.42, { stroke: "#12020c", "stroke-opacity": 0.45, "stroke-width": T * 0.2, filter: "url(#edge)" }),
    along(g, 0, { stroke: "#ff6a9a", "stroke-opacity": 0.12, "stroke-width": T * 0.5, filter: "url(#core)" }),
    along(g, T * 0.3, { stroke: "#ffffff", "stroke-opacity": 0.08, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, T / 2, { stroke: "#140905", "stroke-width": 3 }),
    along(g, -T / 2, { stroke: "#140905", "stroke-width": 3 }),
    along(g, T / 2 - 1, { stroke: "#ece7e0", "stroke-opacity": 0.75, "stroke-width": 1.1 }),
    along(g, -T / 2 + 1, { stroke: "#ece7e0", "stroke-opacity": 0.75, "stroke-width": 1.1 }),
  ].join("");
  return svgDoc(g, defs, body);
}

/**
 * Lightning tube (the owner's photo of a glowing tube of blue water): clear light-blue
 * water (the drifting blue light underneath, half transparent over a dark backing in
 * roomSvg), brightest just inside the glass, with faint ripples of light; no electrode. The purple lightning is Lightning.tsx and
 * the small bubbles are the bubble layer, both on top.
 */
function waterTubeSvg(g: FrameGeometry): string {
  const T = g.tube;
  const defs = blur("edge", T * 0.06) + blur("core", T * 0.12);
  const body = [
    along(g, 0, { stroke: "#e8fbff", "stroke-opacity": 0.04, "stroke-width": T * 0.96 }),
    along(g, T * 0.41, { stroke: "#8ec8ff", "stroke-opacity": 0.45, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, -T * 0.41, { stroke: "#8ec8ff", "stroke-opacity": 0.45, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, T * 0.3, { stroke: "#9fe9ff", "stroke-opacity": 0.12, "stroke-width": T * 0.04, filter: "url(#edge)" }),
    along(g, -T * 0.22, { stroke: "#9fe9ff", "stroke-opacity": 0.1, "stroke-width": T * 0.03, filter: "url(#edge)" }),
    along(g, T * 0.47, { stroke: "#032030", "stroke-opacity": 0.4, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    along(g, -T * 0.47, { stroke: "#032030", "stroke-opacity": 0.4, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    along(g, T * 0.16, { stroke: "#ffffff", "stroke-opacity": 0.1, "stroke-width": T * 0.12, filter: "url(#core)" }),
    along(g, T / 2, { stroke: "#06141c", "stroke-width": 3 }),
    along(g, -T / 2, { stroke: "#06141c", "stroke-width": 3 }),
    along(g, T / 2 - 1, { stroke: "#dff8ff", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
    along(g, -T / 2 + 1, { stroke: "#dff8ff", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
  ].join("");
  return svgDoc(g, defs, body);
}

/**
 * Ocean tube (goes with the aquarium cabinet): clear tropical sea water (the drifting
 * teal light underneath, partly see-through over the dark backing in roomSvg), lit
 * glass edges, shade at the walls and wavy caustic light; the fish, kelp, coral and
 * jellyfish are OceanTube.tsx and the bubbles the bubble layer, on top.
 */
function oceanTubeSvg(g: FrameGeometry): string {
  const T = g.tube;
  const defs = blur("edge", T * 0.06) + blur("core", T * 0.12);
  const caustic = (o: number, dash: string, op: number) =>
    along(g, o, { stroke: "#d8fdff", "stroke-opacity": op, "stroke-width": Math.max(0.8, T * 0.018), "stroke-dasharray": dash, "stroke-linecap": "round" });
  const body = [
    along(g, 0, { stroke: "#e0fbff", "stroke-opacity": 0.05, "stroke-width": T * 0.96 }),
    along(g, T * 0.41, { stroke: "#9ef0ff", "stroke-opacity": 0.45, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, -T * 0.41, { stroke: "#9ef0ff", "stroke-opacity": 0.45, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, T * 0.47, { stroke: "#022030", "stroke-opacity": 0.45, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    along(g, -T * 0.47, { stroke: "#022030", "stroke-opacity": 0.45, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    caustic(T * 0.28, "16 24 6 30", 0.16),
    caustic(T * 0.08, "10 28 20 14", 0.12),
    caustic(-T * 0.16, "22 18 8 34", 0.12),
    caustic(-T * 0.32, "8 30 14 22", 0.14),
    along(g, T / 2, { stroke: "#06141c", "stroke-width": 3 }),
    along(g, -T / 2, { stroke: "#06141c", "stroke-width": 3 }),
    along(g, T / 2 - 1, { stroke: "#dff8ff", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
    along(g, -T / 2 + 1, { stroke: "#dff8ff", "stroke-opacity": 0.8, "stroke-width": 1.1 }),
  ].join("");
  return svgDoc(g, defs, body);
}

/**
 * Glass pipe (Molten's fire, and the GPU pipes of D-275): a clear glass pipe whose
 * contents PipeGL.tsx draws (the drifting light underneath is their glow). Dark inside
 * (the dark backing in roomSvg), glass walls catching the light inside (`tint`), a soft
 * reflection.
 */
function glassTubeSvg(g: FrameGeometry, tint: string): string {
  const T = g.tube;
  const defs = blur("edge", T * 0.06) + blur("core", T * 0.12);
  const body = [
    along(g, T * 0.41, { stroke: tint, "stroke-opacity": 0.32, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, -T * 0.41, { stroke: tint, "stroke-opacity": 0.32, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, T * 0.47, { stroke: "#120604", "stroke-opacity": 0.5, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    along(g, -T * 0.47, { stroke: "#120604", "stroke-opacity": 0.5, "stroke-width": T * 0.06, filter: "url(#edge)" }),
    along(g, T * 0.2, { stroke: "#ffffff", "stroke-opacity": 0.07, "stroke-width": T * 0.1, filter: "url(#core)" }),
    along(g, T / 2, { stroke: "#140804", "stroke-width": 3 }),
    along(g, -T / 2, { stroke: "#140804", "stroke-width": 3 }),
    along(g, T / 2 - 1, { stroke: "#ffe0c0", "stroke-opacity": 0.7, "stroke-width": 1.1 }),
    along(g, -T / 2 + 1, { stroke: "#ffe0c0", "stroke-opacity": 0.7, "stroke-width": 1.1 }),
  ].join("");
  return svgDoc(g, defs, body);
}

/**
 * Shading of the frosted tube, drawn over the moving light and under the bubbles:
 * darker towards both edges, a bright core, the clear inner bubble tube, and a
 * dark seam plus a thin chrome line along both rims. The pilaster style has its own.
 */
export function tubeSvg(g: FrameGeometry, opts: FrameArtOptions = {}): string {
  if (opts.style === "pilaster") return pilasterTubeSvg(g);
  if (opts.style === "lava") return lavaTubeSvg(g);
  if (opts.style === "light") return waterTubeSvg(g);
  if (opts.style === "ocean") return oceanTubeSvg(g);
  const tint = opts.style ? GLASS_PIPES[opts.style] : undefined;
  if (tint) return glassTubeSvg(g, tint);
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

/** A scalloped gold fan sitting on the ring stack, its foot at (x, y), `T` = tube width. */
function shellFinial(T: number, x: number, y: number): string {
  const R = T * 0.5;
  const ribs = 9;
  let d = `M${n(x - R)} ${n(y)}`;
  const lines: string[] = [];
  for (let i = 0; i < ribs; i++) {
    const a0 = Math.PI + (i / ribs) * Math.PI;
    const a1 = Math.PI + ((i + 1) / ribs) * Math.PI;
    const am = (a0 + a1) / 2;
    const ex = x + R * Math.cos(a1);
    const ey = y + R * Math.sin(a1);
    d += ` Q${n(x + R * 1.16 * Math.cos(am))} ${n(y + R * 1.16 * Math.sin(am))} ${n(ex)} ${n(ey)}`;
    if (i < ribs - 1) lines.push(`M${n(x)} ${n(y)} L${n(ex)} ${n(ey)}`);
  }
  d += " Z";
  const rib = lines.join(" ");
  return (
    el("path", { d, fill: "#000", opacity: 0.45, transform: "translate(0 1.5)", filter: "url(#soft)" }) +
    el("path", { d, fill: "url(#dome)", stroke: "#5e4210", "stroke-width": Math.max(0.6, T * 0.012) }) +
    el("path", { d: rib, fill: "none", stroke: "#5e4210", "stroke-opacity": 0.75, "stroke-width": Math.max(0.6, T * 0.018) }) +
    el("path", { d: rib, fill: "none", stroke: "#fff6d8", "stroke-opacity": 0.55, "stroke-width": Math.max(0.5, T * 0.01), transform: "translate(0.8 0)" }) +
    el("ellipse", { cx: x, cy: y, rx: R * 0.26, ry: R * 0.16, fill: "url(#ring)" })
  );
}

/** Chrome ring stack with enamel (or silver chrome) bands (photo 2), centred on x, starting at y. */
function ringStack(g: FrameGeometry, x: number, y: number, shell = false): string {
  const T = g.tube;
  const ov = T * 0.17;
  const ringW = T + 2 * ov;
  const h1 = g.ringHeight / 5;
  const h2 = g.ringHeight / 5;
  const enW = T + ov * 0.5;
  let out =
    el("rect", { x: x - ringW / 2 - 1, y: y + 1, width: ringW + 2, height: g.ringHeight + 3, rx: h1 * 0.4, fill: "#000", opacity: 0.5, filter: "url(#soft)" }) +
    // Finial: a small chrome dome on top, or a scalloped fan on the pilasters (photo 7).
    (shell ? shellFinial(T, x, y + h1 * 0.25) : el("ellipse", { cx: x, cy: y + 0.5, rx: T * 0.3, ry: T * 0.2, fill: "url(#dome)" }));
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

/** Base block (in the cabinet's finish) with chrome bands at the foot of a column. */
function base(g: FrameGeometry, x: number, pearl = false): string {
  const T = g.tube;
  const w = T * 1.5;
  const top = g.baseTop;
  const h = g.height - top + 2;
  const band = Math.max(2, T * 0.14);
  return (
    el("rect", { x: x - w / 2 - 1, y: top - 2, width: w + 2, height: h + 2, rx: T * 0.16, fill: "#000", opacity: 0.55, filter: "url(#soft)" }) +
    el("rect", { x: x - w / 2, y: top, width: w, height: h, rx: T * 0.14, fill: "url(#walnut)" }) +
    (pearl
      ? el("rect", { x: x - w / 2, y: top, width: w, height: h, rx: T * 0.14, fill: "url(#pearlPlay)" }) +
        el("rect", { x: x - w / 2, y: top, width: w, height: h, rx: T * 0.14, fill: "url(#pearlGlint)" })
      : "") +
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
export function trimSvg(g: FrameGeometry, crestW: number, opts: FrameArtOptions = {}): string {
  const T = g.tube;
  const M = METALS[opts.metal ?? "gold"];
  const A = ACCENTS[opts.accents ?? "silver"];
  const pilaster = opts.style === "pilaster";
  // Glass reflections on the front of a glass tube (neon, lava, lightning); not on the
  // pilaster panel.
  const glassLines = !pilaster;
  const ribW = Math.max(2.5, T * 0.075);
  const defs =
    linear("ring", false, M.ring) +
    el(
      "pattern",
      { id: "ribs", width: ribW, height: 10, patternUnits: "userSpaceOnUse" },
      el("rect", { width: Math.max(0.8, T * 0.02), height: 10, fill: "#000", opacity: 0.28 }),
    ) +
    linear("enamel", false, A.band) +
    el("radialGradient", { id: "dome", cx: 0.4, cy: 0.25, r: 0.8 }, stops(M.dome)) +
    linear("walnut", true, BASE_FINISH[opts.cabinet ?? "walnut"]) +
    // Pearl bases: a soft glint and a faint play of lilac and mint across the shell.
    el(
      "radialGradient",
      { id: "pearlGlint", cx: 0.36, cy: 0.22, r: 0.75 },
      el("stop", { offset: 0, "stop-color": "#ffffff", "stop-opacity": 0.6 }) + el("stop", { offset: 1, "stop-color": "#ffffff", "stop-opacity": 0 }),
    ) +
    el(
      "linearGradient",
      { id: "pearlPlay", x1: 0, y1: 0, x2: 1, y2: 1 },
      el("stop", { offset: 0, "stop-color": "#c9b8ff", "stop-opacity": 0 }) +
        el("stop", { offset: 0.35, "stop-color": "#c9b8ff", "stop-opacity": 0.28 }) +
        el("stop", { offset: 0.6, "stop-color": "#b8ffe6", "stop-opacity": 0.22 }) +
        el("stop", { offset: 1, "stop-color": "#ffd0e0", "stop-opacity": 0 }),
    ) +
    linear("ringH", true, M.ringH) +
    linear("enamelH", false, A.bandH) +
    blur("soft", Math.max(1, T * 0.06), "-20%", "140%");
  const body =
    // Glass reflections on the front of the neon tube (the pilaster panel has none).
    (!glassLines
      ? ""
      : along(g, -T * 0.24, { stroke: "#ffffff", "stroke-opacity": 0.07, "stroke-width": T * 0.14 }) +
        along(g, -T * 0.2, { stroke: "#ffffff", "stroke-opacity": 0.72, "stroke-width": Math.max(1, T * 0.035) })) +
    ringStack(g, g.xL, g.ringTop, pilaster) +
    ringStack(g, g.width - g.xL, g.ringTop, pilaster) +
    base(g, g.xL, opts.cabinet === "pearl") +
    base(g, g.width - g.xL, opts.cabinet === "pearl") +
    crestClamps(g, crestW);
  return svgDoc(g, defs, body);
}

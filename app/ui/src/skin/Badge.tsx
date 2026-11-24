// SPDX-License-Identifier: GPL-3.0-or-later
// The app's own badge (docs/DESIGN.md "Badge"): an original design, NOT taken from
// any jukebox maker. A long, round-cornered lozenge (an elongated hexagon, no
// wings, no centre disc) with a polished gold chrome rim, a medium gold border, a
// field inside it, the app name in a retro script with a swash underline, and one
// tiny four-point star. Drawn entirely in SVG.
//
// Field and lettering are the owner's choice (settings ui.look, 2026-11-22): a
// polished lapis lazuli field (ultramarine with pyrite flecks and faint calcite
// veins) with gold chrome lettering by default; the original red enamel, or silver
// chrome; cream lettering. Molten theme (2026-11-23): bronze for every gold part, a
// magma field (black crust split by glowing cracks) and ember lettering (white-hot to
// orange, with a heat glow).
//
// The name comes from branding.json, the only place it is written down
// (docs/ARCHITECTURE.md section 9): pass it as `name` (AppInfo.name from the Rust
// core); without the prop the badge reads it from the page, where the Vite build
// has injected it (<meta name="application-name"> or the <title>).

import { useSvgId } from "./svg-id";

export type BadgeFieldFinish = "lapis" | "red" | "silver" | "magma" | "copper" | "carbon" | "glass";
export type BadgeLetteringFinish = "gold" | "cream" | "ember" | "neon" | "white";
type MetalKind = "gold" | "bronze" | "chrome" | "brass" | "gunmetal";
export type BadgeRimFinish = "lapis" | "gold";

export interface BadgeProps {
  /** The product name. Defaults to the name injected into the page from branding.json. */
  name?: string;
  class?: string;
  /** Accessible label; defaults to the name. */
  title?: string;
  /** The field inside the medium gold border (lapis lazuli by default). */
  field?: BadgeFieldFinish;
  /** The lettering: gold chrome (default) or the original cream. */
  lettering?: BadgeLetteringFinish;
  /** The frame round the field: lapis lazuli with a thin gold edge (default) or gold chrome. */
  rim?: BadgeRimFinish;
  /** Every gold part (rim, border, edges, gold lettering): gold chrome (default) or bronze. */
  metal?: MetalKind;
}

type Stops = readonly (readonly [number, string])[];

/** Gradient stops of the warm metal: rim chrome, border chrome, lettering, flat lettering. */
const METAL_STOPS: Record<MetalKind, { chrome: Stops; chrome2: Stops; text: Stops; flat: Stops; edge: string }> = {
  chrome: {
    chrome: [[0, "#ffffff"], [0.22, "#dfe4ea"], [0.48, "#8a929c"], [0.55, "#3e454e"], [0.64, "#b4bcc5"], [0.86, "#f4f6f8"], [1, "#8e96a0"]],
    chrome2: [[0, "#5a626c"], [0.5, "#f6f8fa"], [1, "#6a727c"]],
    text: [[0, "#ffffff"], [0.26, "#e4e9ee"], [0.47, "#9aa3ad"], [0.55, "#4a525b"], [0.64, "#c2cad2"], [0.84, "#ffffff"], [1, "#a0a8b2"]],
    flat: [[0, "#ffffff"], [0.5, "#c8d0d8"], [1, "#7a828c"]],
    edge: "#1e2328",
  },
  brass: {
    chrome: [[0, "#fff2c4"], [0.22, "#e6cc80"], [0.48, "#9a7826"], [0.55, "#4e3a0c"], [0.64, "#ccaa52"], [0.86, "#f6e4a8"], [1, "#a88430"]],
    chrome2: [[0, "#7a5a18"], [0.5, "#f6e4a8"], [1, "#8a6a20"]],
    text: [[0, "#fffae0"], [0.26, "#f0d88a"], [0.47, "#c09a3a"], [0.55, "#6e5212"], [0.64, "#d8b85a"], [0.84, "#fff0c0"], [1, "#b89034"]],
    flat: [[0, "#fff0c0"], [0.5, "#d8b450"], [1, "#94701e"]],
    edge: "#3a2a06",
  },
  gunmetal: {
    chrome: [[0, "#e4eaf0"], [0.22, "#a4b0bc"], [0.48, "#5a6672"], [0.55, "#1c2228"], [0.64, "#7a8692"], [0.86, "#cdd5dd"], [1, "#4a5560"]],
    chrome2: [[0, "#2a3238"], [0.5, "#c4ccd4"], [1, "#3a434c"]],
    text: [[0, "#f0f4f8"], [0.26, "#b8c2cc"], [0.47, "#6a7682"], [0.55, "#262d34"], [0.64, "#8a96a2"], [0.84, "#dfe5eb"], [1, "#5a6672"]],
    flat: [[0, "#dfe5eb"], [0.5, "#8a96a2"], [1, "#3a434c"]],
    edge: "#0e1216",
  },
  gold: {
    chrome: [[0, "#fff6d8"], [0.22, "#ecd38e"], [0.48, "#9a7228"], [0.55, "#563c0f"], [0.64, "#cca34a"], [0.86, "#f9e9b9"], [1, "#bc8e35"]],
    chrome2: [[0, "#825f21"], [0.5, "#fbecc0"], [1, "#946d26"]],
    text: [[0, "#fffbe6"], [0.26, "#f6d77e"], [0.47, "#c8952f"], [0.55, "#7d5410"], [0.64, "#dcae4e"], [0.84, "#fff1bd"], [1, "#c79533"]],
    flat: [[0, "#fff1bd"], [0.5, "#e2b04a"], [1, "#a0731f"]],
    edge: "#4a2f05",
  },
  bronze: {
    chrome: [[0, "#ffe0c2"], [0.22, "#e8a878"], [0.48, "#9a5424"], [0.55, "#4a2008"], [0.64, "#c07840"], [0.86, "#f6c8a0"], [1, "#a8602c"]],
    chrome2: [[0, "#7a3a14"], [0.5, "#f6c8a0"], [1, "#8a4a1c"]],
    text: [[0, "#fff0e0"], [0.26, "#f0b888"], [0.47, "#c07840"], [0.55, "#6a3010"], [0.64, "#d8945e"], [0.84, "#ffd8b8"], [1, "#b86a34"]],
    flat: [[0, "#ffd8b8"], [0.5, "#d08a52"], [1, "#8a4a1c"]],
    edge: "#3a1606",
  },
};

/** Ember lettering: white heat on top, through gold and orange to red at the foot. */
const EMBER_TEXT: Stops = [[0, "#fffbe0"], [0.3, "#ffe27a"], [0.55, "#ffa024"], [0.78, "#ff5a0a"], [1, "#c02a04"]];
const EMBER_FLAT: Stops = [[0, "#ffe27a"], [0.5, "#ff8a1a"], [1, "#c83a06"]];
/** Glossy white lettering (Frutiger Aero, Water): white with a sky-blue band. */
const WHITE_TEXT: Stops = [[0, "#ffffff"], [0.45, "#f4fbff"], [0.55, "#c4e8ff"], [0.7, "#ffffff"], [1, "#e0f2ff"]];
const WHITE_FLAT: Stops = [[0, "#ffffff"], [0.5, "#e8f6ff"], [1, "#bfe2ff"]];

/** Rivets round the inside of the copper field. */
const RIVETS: [number, number][] = [
  [78, 34],
  [150, 31],
  [222, 34],
  [78, 86],
  [150, 89],
  [222, 86],
  [56, 60],
  [244, 60],
];

function gradStops(stops: Stops) {
  return stops.map(([o, c]) => <stop key={o} offset={o} stop-color={c} />);
}

/** The product name as the build injected it into the page (branding.json). */
export function appName(): string {
  if (typeof document === "undefined") return "";
  const meta = document.querySelector('meta[name="application-name"]');
  const fromMeta = meta?.getAttribute("content")?.trim();
  if (fromMeta && !fromMeta.includes("%")) return fromMeta;
  return document.title.split(/\s[-|:]\s/)[0].trim();
}

// Badge outline (viewBox 0 0 300 120): an elongated hexagon. The rim is drawn
// as a thick stroke with round joins, which rounds every corner.
const OUTLINE = "M28 60 L62 16 L238 16 L272 60 L238 104 L62 104 Z";
const FIELD = "M44 60 L71 26 L229 26 L256 60 L229 94 L71 94 Z";
const PIN = "M53 60 L76 31 L224 31 L247 60 L224 89 L76 89 Z";

/** Widest the name may be inside the field (viewBox units). */
const TEXT_MAX_W = 172;
/** Smallest script size before the name is squeezed instead of shrunk further. */
const TEXT_MIN_SIZE = 26;

/**
 * Font size for the name, and whether it must be squeezed (textLength) to fit.
 * Pacifico averages ~0.55 em per character: short names get the full 44 units,
 * longer ones shrink down to TEXT_MIN_SIZE, and only names still too wide at that
 * size are squeezed horizontally, so a long name stays legible.
 */
export function fitBadgeText(text: string): { fontSize: number; squeeze: boolean } {
  const est = Math.max(1, text.length * 0.55);
  const fontSize = Math.max(TEXT_MIN_SIZE, Math.min(44, (TEXT_MAX_W - 4) / est));
  return { fontSize, squeeze: est * fontSize > TEXT_MAX_W };
}

/** Four-point star centred on (x, y). */
function star(x: number, y: number, r: number): string {
  const k = r * 0.28;
  return `M${x} ${y - r} L${x + k} ${y - k} L${x + r} ${y} L${x + k} ${y + k} L${x} ${y + r} L${x - k} ${y + k} L${x - r} ${y} L${x - k} ${y - k} Z`;
}

/** Small deterministic PRNG so the lapis flecks are the same on every render. */
function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), a | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Pyrite flecks scattered through the lapis (x, y, r); the field's clip trims the strays. */
const FLECKS: [number, number, number][] = (() => {
  const rnd = seeded(1979);
  const out: [number, number, number][] = [];
  for (let i = 0; i < 46; i++) {
    const big = rnd() < 0.18;
    out.push([Math.round((60 + rnd() * 180) * 10) / 10, Math.round((28 + rnd() * 64) * 10) / 10, Math.round((big ? 0.9 + rnd() * 0.9 : 0.35 + rnd() * 0.5) * 100) / 100]);
  }
  return out;
})();

/** The outline's outer edge (OUTLINE grown by half its rim stroke), to clip the rim's flecks. */
const OUTER = "M16 60 L57 7 L243 7 L284 60 L243 113 L57 113 Z";

/** Pyrite flecks scattered round the lapis frame: sampled along the outline, a little either side. */
const RIM_FLECKS: [number, number, number][] = (() => {
  const rnd = seeded(2024);
  const pts: [number, number][] = [
    [28, 60],
    [62, 16],
    [238, 16],
    [272, 60],
    [238, 104],
    [62, 104],
  ];
  const out: [number, number, number][] = [];
  for (let i = 0; i < 40; i++) {
    const k = Math.floor(rnd() * pts.length);
    const [ax, ay] = pts[k];
    const [bx, by] = pts[(k + 1) % pts.length];
    const t = rnd();
    const off = (rnd() - 0.5) * 12;
    const len = Math.hypot(bx - ax, by - ay) || 1;
    const nx = -(by - ay) / len;
    const ny = (bx - ax) / len;
    const big = rnd() < 0.2;
    out.push([
      Math.round((ax + (bx - ax) * t + nx * off) * 10) / 10,
      Math.round((ay + (by - ay) * t + ny * off) * 10) / 10,
      Math.round((big ? 0.8 + rnd() * 0.7 : 0.3 + rnd() * 0.45) * 100) / 100,
    ]);
  }
  return out;
})();

/** Glowing cracks in the magma crust: jagged lines across the field, with a few branches. */
const CRACKS: string[] = (() => {
  const rnd = seeded(4711);
  const line = (x0: number, y0: number, x1: number, y1: number, n: number, jag: number) => {
    let d = `M${x0} ${y0}`;
    for (let i = 1; i <= n; i++) {
      const t = i / n;
      const j = i === n ? 0 : (rnd() - 0.5) * jag;
      d += ` L${Math.round((x0 + (x1 - x0) * t + j * 0.4) * 10) / 10} ${Math.round((y0 + (y1 - y0) * t + j) * 10) / 10}`;
    }
    return d;
  };
  return [
    line(40, 70, 262, 58, 14, 9),
    line(70, 26, 112, 96, 7, 8),
    line(196, 22, 168, 98, 7, 8),
    line(232, 30, 250, 92, 5, 7),
    line(60, 40, 130, 46, 6, 6),
    line(150, 64, 214, 92, 6, 6),
    line(48, 82, 104, 88, 5, 5),
  ];
})();

/** Faint calcite veins in the lapis. */
const VEINS = [
  "M58 72 C 92 60, 118 82, 150 70 S 210 52, 246 64",
  "M80 40 C 104 50, 128 36, 160 44 S 214 48, 232 38",
  "M70 88 C 110 80, 140 92, 178 84 S 222 78, 240 86",
];

export function Badge({ name, class: className, title, field = "lapis", lettering = "gold", rim = "lapis", metal = "gold" }: BadgeProps) {
  const id = useSvgId("jkb");
  const text = (name ?? appName()) || " ";
  const { fontSize, squeeze } = fitBadgeText(text);
  const M = METAL_STOPS[metal];
  const gold = lettering === "gold";
  const ember = lettering === "ember";
  const neon = lettering === "neon";
  const white = lettering === "white";
  const shaded = gold || ember || neon || white;
  const under =
    field === "lapis" ? "#040a24" : field === "silver" ? "#23272d" : field === "magma" ? "#0e0402" : field === "copper" ? "#2a1004" : field === "carbon" ? "#000000" : field === "glass" ? "#0a3a7a" : "#3d0410";
  const pin =
    field === "silver"
      ? "#3b4048"
      : field === "magma"
        ? "#ff9a4a"
        : field === "carbon"
          ? "#2ae0ff"
          : field === "glass"
            ? "#ffffff"
            : field === "copper"
              ? "#f0c070"
              : gold || field === "lapis"
                ? metal === "bronze"
                  ? "#e8a878"
                  : metal === "chrome" || metal === "gunmetal"
                    ? "#dfe5eb"
                    : "#e8c66e"
                : "#f6efd9";
  const ink = gold ? `url(#${id}-goldtext)` : ember ? `url(#${id}-embertext)` : white ? `url(#${id}-whitetext)` : neon ? "#effeff" : "#f6efd9";
  const inkFlat = gold ? `url(#${id}-goldflat)` : ember ? `url(#${id}-emberflat)` : white ? `url(#${id}-whiteflat)` : neon ? "#ffd23a" : "#f6efd9";
  const edge = gold ? M.edge : ember ? "#2a0602" : white ? "#1a5a9a" : neon ? "#0a8ab0" : undefined;
  /** Colour of the blurred glow behind the lettering, if it glows. */
  const glowInk = ember ? "#ff5a0a" : neon ? "#2ae0ff" : null;
  return (
    <svg
      class={`jk-badge jk-badge--${field} jk-badge--${lettering} jk-badge--rim-${rim}${className ? ` ${className}` : ""}`}
      viewBox="0 0 300 120"
      role="img"
      aria-label={title ?? text}
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        <linearGradient id={`${id}-chrome`} x1="0" y1="0" x2="0" y2="1">
          {gradStops(M.chrome)}
        </linearGradient>
        <linearGradient id={`${id}-chrome2`} x1="0" y1="0" x2="0" y2="1">
          {gradStops(M.chrome2)}
        </linearGradient>
        {/* Magma: a black crust, warmer towards the middle, heat welling up from below. */}
        <radialGradient id={`${id}-magma`} cx="0.5" cy="0.6" r="0.8">
          <stop offset="0" stop-color="#4a1a0a" />
          <stop offset="0.45" stop-color="#2a100a" />
          <stop offset="1" stop-color="#100605" />
        </radialGradient>
        <radialGradient id={`${id}-heat`} cx="0.5" cy="1" r="0.7">
          <stop offset="0" stop-color="#ff5a0a" stop-opacity="0.55" />
          <stop offset="1" stop-color="#ff5a0a" stop-opacity="0" />
        </radialGradient>
        <radialGradient id={`${id}-ember`} cx="0.4" cy="0.35" r="0.75">
          <stop offset="0" stop-color="#fff2c0" />
          <stop offset="0.45" stop-color="#ff8a1a" />
          <stop offset="1" stop-color="#8a1a02" />
        </radialGradient>
        <linearGradient id={`${id}-embertext`} gradientUnits="userSpaceOnUse" x1="0" y1="34" x2="0" y2="84">
          {gradStops(EMBER_TEXT)}
        </linearGradient>
        <linearGradient id={`${id}-emberflat`} x1="0" y1="0" x2="0" y2="1">
          {gradStops(EMBER_FLAT)}
        </linearGradient>
        <linearGradient id={`${id}-whitetext`} gradientUnits="userSpaceOnUse" x1="0" y1="34" x2="0" y2="84">
          {gradStops(WHITE_TEXT)}
        </linearGradient>
        <linearGradient id={`${id}-whiteflat`} x1="0" y1="0" x2="0" y2="1">
          {gradStops(WHITE_FLAT)}
        </linearGradient>
        {/* Copper with a warm sheen; rivets; carbon fibre weave; glossy aqua glass. */}
        <radialGradient id={`${id}-copper`} cx="0.42" cy="0.3" r="0.9">
          <stop offset="0" stop-color="#a85a2c" />
          <stop offset="0.45" stop-color="#6a2e10" />
          <stop offset="0.85" stop-color="#3a1606" />
          <stop offset="1" stop-color="#1e0a02" />
        </radialGradient>
        <radialGradient id={`${id}-rivet`} cx="0.35" cy="0.3" r="0.8">
          <stop offset="0" stop-color="#fff0c8" />
          <stop offset="0.5" stop-color="#c8963c" />
          <stop offset="1" stop-color="#4a2a08" />
        </radialGradient>
        <pattern id={`${id}-weave`} width="6" height="6" patternUnits="userSpaceOnUse">
          <rect width="3" height="3" fill="#2c3036" />
          <rect x="3" y="3" width="3" height="3" fill="#2c3036" />
          <rect x="3" width="3" height="3" fill="#121418" />
          <rect y="3" width="3" height="3" fill="#121418" />
          <rect width="3" height="1" fill="#454a54" opacity="0.6" />
          <rect x="3" y="3" width="3" height="1" fill="#454a54" opacity="0.6" />
        </pattern>
        <linearGradient id={`${id}-carbonsheen`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#ffffff" stop-opacity="0" />
          <stop offset="0.45" stop-color="#ffffff" stop-opacity="0.16" />
          <stop offset="0.55" stop-color="#ffffff" stop-opacity="0" />
        </linearGradient>
        <linearGradient id={`${id}-aqua`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#c8f6ff" />
          <stop offset="0.35" stop-color="#3ab8ff" />
          <stop offset="0.75" stop-color="#0a6ad0" />
          <stop offset="1" stop-color="#2ac87a" />
        </linearGradient>
        <filter id={`${id}-glow`} x="-20%" y="-40%" width="140%" height="180%">
          <feGaussianBlur stdDeviation="2.4" />
        </filter>
        <linearGradient id={`${id}-enamel`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#ef3a55" />
          <stop offset="0.45" stop-color="#c8102e" />
          <stop offset="1" stop-color="#6d0717" />
        </linearGradient>
        {/* Lapis lazuli: deep ultramarine, lighter where the light falls, near-black at the rim. */}
        <radialGradient id={`${id}-lapis`} cx="0.42" cy="0.3" r="0.85">
          <stop offset="0" stop-color="#3f72e4" />
          <stop offset="0.3" stop-color="#224ab8" />
          <stop offset="0.66" stop-color="#132c82" />
          <stop offset="1" stop-color="#070f3d" />
        </radialGradient>
        <radialGradient id={`${id}-lapis2`} cx="0.76" cy="0.78" r="0.42">
          <stop offset="0" stop-color="#5a88f5" stop-opacity="0.38" />
          <stop offset="1" stop-color="#5a88f5" stop-opacity="0" />
        </radialGradient>
        <radialGradient id={`${id}-pyrite`} cx="0.35" cy="0.3" r="0.8">
          <stop offset="0" stop-color="#fff4c4" />
          <stop offset="0.5" stop-color="#e2b24a" />
          <stop offset="1" stop-color="#7a5414" />
        </radialGradient>
        <linearGradient id={`${id}-silver`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#f7f9fb" />
          <stop offset="0.24" stop-color="#cfd5dc" />
          <stop offset="0.48" stop-color="#7f8892" />
          <stop offset="0.55" stop-color="#4a515a" />
          <stop offset="0.65" stop-color="#aeb6bf" />
          <stop offset="0.86" stop-color="#eef1f4" />
          <stop offset="1" stop-color="#9aa2ab" />
        </linearGradient>
        {/* Gold chrome lettering: bright top, a dark horizon band, bright again below (across the glyphs). */}
        <linearGradient id={`${id}-goldtext`} gradientUnits="userSpaceOnUse" x1="0" y1="34" x2="0" y2="84">
          {gradStops(M.text)}
        </linearGradient>
        <linearGradient id={`${id}-goldflat`} x1="0" y1="0" x2="0" y2="1">
          {gradStops(M.flat)}
        </linearGradient>
        <radialGradient id={`${id}-gloss`} cx="0.5" cy="0" r="0.75" fx="0.5" fy="0">
          <stop offset="0" stop-color="#fff" stop-opacity="0.55" />
          <stop offset="0.55" stop-color="#fff" stop-opacity="0.08" />
          <stop offset="1" stop-color="#fff" stop-opacity="0" />
        </radialGradient>
        <clipPath id={`${id}-clip`}>
          <path d={FIELD} />
        </clipPath>
        <clipPath id={`${id}-rimclip`}>
          <path d={OUTER} />
        </clipPath>
        {/* The lapis frame: lit from above, deep below, with a little bounce light at the foot. */}
        <linearGradient id={`${id}-rimlapis`} gradientUnits="userSpaceOnUse" x1="0" y1="7" x2="0" y2="113">
          <stop offset="0" stop-color="#5486ee" />
          <stop offset="0.3" stop-color="#2148b4" />
          <stop offset="0.58" stop-color="#11297a" />
          <stop offset="0.86" stop-color="#091852" />
          <stop offset="1" stop-color="#1a3a92" />
        </linearGradient>
        {/* Bevel over the frame: a highlight on top, shade underneath. */}
        <linearGradient id={`${id}-rimshade`} gradientUnits="userSpaceOnUse" x1="0" y1="7" x2="0" y2="113">
          <stop offset="0" stop-color="#ffffff" stop-opacity="0.42" />
          <stop offset="0.36" stop-color="#ffffff" stop-opacity="0" />
          <stop offset="0.64" stop-color="#000000" stop-opacity="0" />
          <stop offset="1" stop-color="#000000" stop-opacity="0.38" />
        </linearGradient>
        <filter id={`${id}-shadow`} x="-20%" y="-30%" width="140%" height="160%">
          <feGaussianBlur stdDeviation="3" />
        </filter>
      </defs>
      {/* Drop shadow under the whole badge. */}
      <path d={OUTLINE} fill="#000" opacity="0.55" transform="translate(0 4)" filter={`url(#${id}-shadow)`} stroke="#000" stroke-width="16" stroke-linejoin="round" />
      {rim === "lapis" ? (
        <>
          {/* Lapis lazuli frame with a thin gold edge outside and inside. */}
          <path d={OUTLINE} fill={`url(#${id}-chrome)`} stroke={`url(#${id}-chrome)`} stroke-width="18.6" stroke-linejoin="round" />
          <path d={OUTLINE} fill={`url(#${id}-rimlapis)`} stroke={`url(#${id}-rimlapis)`} stroke-width="15.6" stroke-linejoin="round" />
          <g clip-path={`url(#${id}-rimclip)`}>
            <path d={OUTLINE} fill="none" stroke="#b9cdf5" stroke-opacity="0.13" stroke-width="1.6" stroke-dasharray="22 9 6 14" stroke-linejoin="round" transform="translate(0 -3)" />
            <path d={OUTLINE} fill="none" stroke="#e8f0ff" stroke-opacity="0.12" stroke-width="0.6" stroke-dasharray="14 18 30 8" stroke-linejoin="round" transform="translate(0 3)" />
            {RIM_FLECKS.map(([x, y, r]) => (
              <circle key={`r${x}-${y}`} cx={x} cy={y} r={r} fill={`url(#${id}-pyrite)`} opacity={r > 0.8 ? 0.95 : 0.7} />
            ))}
          </g>
          <path d={OUTLINE} fill="none" stroke={`url(#${id}-rimshade)`} stroke-width="15.6" stroke-linejoin="round" />
          <path d={FIELD} fill="none" stroke="#050b2a" stroke-width="6" stroke-linejoin="round" opacity="0.55" />
          <path d={FIELD} fill={`url(#${id}-chrome2)`} stroke={`url(#${id}-chrome2)`} stroke-width="3.4" stroke-linejoin="round" />
        </>
      ) : (
        <>
          {/* Chrome rim: thick round-joined stroke + fill, then a reversed bevel. */}
          <path d={OUTLINE} fill={`url(#${id}-chrome)`} stroke={`url(#${id}-chrome)`} stroke-width="16" stroke-linejoin="round" />
          <path d={OUTLINE} fill="none" stroke="#24252a" stroke-width="17.5" stroke-linejoin="round" opacity="0.35" />
          <path d={OUTLINE} fill={`url(#${id}-chrome)`} stroke={`url(#${id}-chrome)`} stroke-width="15" stroke-linejoin="round" />
          {/* The medium gold border round the field, set off from the rim by a fine groove. */}
          <path d={FIELD} fill="none" stroke="#3a2806" stroke-width="10.5" stroke-linejoin="round" opacity="0.5" />
          <path d={FIELD} fill={`url(#${id}-chrome2)`} stroke={`url(#${id}-chrome2)`} stroke-width="8" stroke-linejoin="round" />
        </>
      )}
      {/* The field. */}
      {field === "lapis" ? (
        <>
          <path d={FIELD} fill={`url(#${id}-lapis)`} stroke="#050b2a" stroke-width="1.2" stroke-linejoin="round" />
          <g clip-path={`url(#${id}-clip)`}>
            <rect x="40" y="20" width="220" height="80" fill={`url(#${id}-lapis2)`} />
            {VEINS.map((d) => (
              <path key={d} d={d} fill="none" stroke="#b9cdf5" stroke-opacity="0.15" stroke-width="2.2" stroke-linecap="round" />
            ))}
            {VEINS.map((d) => (
              <path key={`${d}-f`} d={d} fill="none" stroke="#e8f0ff" stroke-opacity="0.17" stroke-width="0.6" stroke-linecap="round" />
            ))}
            {FLECKS.map(([x, y, r]) => (
              <circle key={`${x}-${y}`} cx={x} cy={y} r={r} fill={`url(#${id}-pyrite)`} opacity={r > 0.9 ? 0.95 : 0.7} />
            ))}
            {/* Shade at the rim: the polished stone is domed like a cabochon. */}
            <path d={FIELD} fill="none" stroke="#020619" stroke-width="12" stroke-linejoin="round" opacity="0.26" />
            <path d={FIELD} fill="none" stroke="#020619" stroke-width="5" stroke-linejoin="round" opacity="0.32" />
            {/* Light caught in the lower curve of the dome. */}
            <ellipse cx="150" cy="96" rx="92" ry="9" fill="#7fa6ff" opacity="0.16" />
          </g>
        </>
      ) : field === "magma" ? (
        <>
          <path d={FIELD} fill={`url(#${id}-magma)`} stroke="#0a0302" stroke-width="1.2" stroke-linejoin="round" />
          <g clip-path={`url(#${id}-clip)`}>
            <rect x="40" y="20" width="220" height="80" fill={`url(#${id}-heat)`} />
            {/* Each crack: a blurred red heat, an orange body, a white-hot thread. */}
            <g filter={`url(#${id}-glow)`}>
              {CRACKS.map((d) => (
                <path key={`g${d}`} d={d} fill="none" stroke="#ff4a0a" stroke-width="4.2" stroke-linejoin="round" stroke-linecap="round" />
              ))}
            </g>
            {CRACKS.map((d) => (
              <path key={`b${d}`} d={d} fill="none" stroke="#ff9a2a" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round" />
            ))}
            {CRACKS.map((d) => (
              <path key={`c${d}`} d={d} fill="none" stroke="#fff0b8" stroke-width="0.45" stroke-linejoin="round" stroke-linecap="round" />
            ))}
            {FLECKS.filter(([, , r]) => r > 0.6).map(([x, y, r]) => (
              <circle key={`${x}-${y}`} cx={x} cy={y} r={r * 0.9} fill={`url(#${id}-ember)`} opacity="0.85" />
            ))}
            <path d={FIELD} fill="none" stroke="#000" stroke-width="12" stroke-linejoin="round" opacity="0.3" />
            <path d={FIELD} fill="none" stroke="#000" stroke-width="5" stroke-linejoin="round" opacity="0.35" />
          </g>
        </>
      ) : field === "copper" ? (
        <>
          <path d={FIELD} fill={`url(#${id}-copper)`} stroke="#2a1004" stroke-width="1.2" stroke-linejoin="round" />
          <g clip-path={`url(#${id}-clip)`}>
            {VEINS.map((d) => (
              <path key={`v${d}`} d={d} fill="none" stroke="#3a9a7a" stroke-opacity="0.32" stroke-width="6" stroke-linecap="round" />
            ))}
            {RIVETS.map(([x, y]) => (
              <circle key={`rv${x}-${y}`} cx={x} cy={y} r="2.2" fill={`url(#${id}-rivet)`} stroke="#2a1004" stroke-width="0.4" />
            ))}
            <path d={FIELD} fill="none" stroke="#1a0802" stroke-width="10" stroke-linejoin="round" opacity="0.3" />
          </g>
        </>
      ) : field === "carbon" ? (
        <>
          <path d={FIELD} fill="#0c0d10" stroke="#000" stroke-width="1.2" stroke-linejoin="round" />
          <path d={FIELD} fill={`url(#${id}-weave)`} />
          <path d={FIELD} fill={`url(#${id}-carbonsheen)`} />
        </>
      ) : field === "glass" ? (
        <>
          <path d={FIELD} fill={`url(#${id}-aqua)`} stroke="#0a3a7a" stroke-width="1.2" stroke-linejoin="round" />
          <g clip-path={`url(#${id}-clip)`}>
            <ellipse cx="150" cy="36" rx="104" ry="17" fill="#ffffff" opacity="0.5" />
            <ellipse cx="150" cy="98" rx="90" ry="14" fill="#9dffb0" opacity="0.35" />
            <circle cx="214" cy="74" r="6" fill="#ffffff" opacity="0.25" />
            <circle cx="88" cy="70" r="4" fill="#ffffff" opacity="0.22" />
          </g>
        </>
      ) : field === "silver" ? (
        <path d={FIELD} fill={`url(#${id}-silver)`} stroke="#2a2e34" stroke-width="1.2" stroke-linejoin="round" />
      ) : (
        <path d={FIELD} fill={`url(#${id}-enamel)`} stroke="#4a0410" stroke-width="1.2" stroke-linejoin="round" />
      )}
      <path d={PIN} fill="none" stroke={pin} stroke-width="1.3" stroke-linejoin="round" opacity={field === "silver" ? 0.7 : 0.88} />
      {/* Ember and neon lettering glow: a blurred copy behind it. */}
      {glowInk && (
        <text
          class="jk-badge-text"
          x="150"
          y="70"
          text-anchor="middle"
          font-size={fontSize}
          fill={glowInk}
          opacity="0.9"
          filter={`url(#${id}-glow)`}
          textLength={squeeze ? TEXT_MAX_W : undefined}
          lengthAdjust={squeeze ? "spacingAndGlyphs" : undefined}
        >
          {text}
        </text>
      )}
      {/* Name: dark under-print for depth, then the lettering. */}
      <text
        class="jk-badge-text"
        x="150"
        y="70"
        text-anchor="middle"
        font-size={fontSize}
        fill={under}
        transform="translate(1.6 2.2)"
        textLength={squeeze ? TEXT_MAX_W : undefined}
        lengthAdjust={squeeze ? "spacingAndGlyphs" : undefined}
      >
        {text}
      </text>
      <text
        class="jk-badge-text jk-badge-name"
        x="150"
        y="70"
        text-anchor="middle"
        font-size={fontSize}
        fill={ink}
        stroke={edge}
        stroke-width={shaded ? 0.75 : undefined}
        paint-order="stroke"
        textLength={squeeze ? TEXT_MAX_W : undefined}
        lengthAdjust={squeeze ? "spacingAndGlyphs" : undefined}
      >
        {text}
      </text>
      {/* Swash underline, tapering at both ends. */}
      <path d="M82 83 Q150 75 222 79 Q151 79.5 82 85.6 Z" fill={inkFlat} stroke={edge} stroke-width={shaded ? 0.35 : undefined} opacity="0.95" />
      {/* The tiny star. */}
      <path d={star(229, 41, 6.5)} fill={inkFlat} stroke={edge} stroke-width={shaded ? 0.4 : undefined} />
      <circle cx="229" cy="41" r="1.4" fill={gold || white ? "#fffbe6" : ember ? "#fff6d0" : neon ? "#ffffff" : "#ffd23f"} />
      {/* Gloss over the field (clipped to it); a sharper glint on the polished stone. */}
      <g clip-path={`url(#${id}-clip)`}>
        <ellipse cx="150" cy="30" rx="120" ry="30" fill={`url(#${id}-gloss)`} opacity={field === "silver" ? 0.5 : field === "magma" || field === "carbon" ? 0.3 : 1} />
        {field === "lapis" && <ellipse cx="118" cy="34" rx="46" ry="6.5" fill="#fff" opacity="0.13" transform="rotate(-6 118 34)" />}
      </g>
    </svg>
  );
}

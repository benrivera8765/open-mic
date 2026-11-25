// SPDX-License-Identifier: GPL-3.0-or-later
// Compass-star medallion (docs/DESIGN.md): an eight-point star, every point split
// into a light and a dark facet, on a red enamel disc inside a gold-chrome bezel.
// A generic heraldic motif drawn from scratch in SVG.

import { useSvgId } from "./svg-id";

export interface MedallionProps {
  class?: string;
  /** Accessible label; decorative (hidden from assistive tech) when absent. */
  title?: string;
  /** A thin silver chrome ring between the gold bezel and the red enamel (owner, 2026-11-22). */
  trim?: boolean;
  /** The bezel and star: gold chrome (default), bronze (Molten), chrome or brass. */
  metal?: "gold" | "bronze" | "chrome" | "brass";
  /** The thin ring's metal: silver chrome (default), iron or copper. */
  trimFinish?: "silver" | "iron" | "copper";
  /** The enamel disc, following the theme's glow: red (pink glow, default), ember, aqua, violet, amber. */
  enamel?: "pink" | "ember" | "aqua" | "violet" | "amber";
}

/** Bezel stops and the star's light and dark facets (long points, short points). */
const METALS = {
  gold: { bezel: ["#fff6d8", "#d9b25a", "#7a5a1c", "#e9cf86", "#8a6624"], line: "#5a3f10", facets: ["#fff1bf", "#f3d57a", "#d09a2c", "#a8761c"] },
  bronze: { bezel: ["#ffe0c2", "#d08a52", "#6a3414", "#eab084", "#8a4a20"], line: "#4a2008", facets: ["#ffe2c4", "#f0b080", "#c0682c", "#94461a"] },
  chrome: { bezel: ["#ffffff", "#c4cbd3", "#56606a", "#e6eaee", "#727a84"], line: "#2a3038", facets: ["#ffffff", "#e2e7ec", "#9aa3ad", "#727b86"] },
  brass: { bezel: ["#fff2c4", "#d8b45a", "#6e5216", "#ecd48c", "#86661e"], line: "#4a360a", facets: ["#fff4cc", "#ecd488", "#c4962e", "#9a7418"] },
} as const;

/** Enamel disc: light, middle, dark. */
const ENAMELS = {
  pink: ["#ff4a5f", "#c8102e", "#5e0513"],
  ember: ["#ff7a3a", "#c8360a", "#5e1604"],
  aqua: ["#5ad0ff", "#0a6ab0", "#04284a"],
  violet: ["#b884ff", "#5a1ab0", "#200648"],
  amber: ["#ffc860", "#b8740a", "#4a2a02"],
} as const;

const TRIMS = {
  silver: ["#ffffff", "#c3cad2", "#5c646e", "#e3e8ed", "#6f7782"],
  iron: ["#a8aeb4", "#5c6268", "#1e2124", "#7c8288", "#2c3034"],
  copper: ["#ffd8b8", "#e08a54", "#7a3412", "#f6b890", "#8a4020"],
} as const;

/** Polygon points of one facet of a star point at angle `a` (radians, 0 = up). */
function facet(a: number, r: number, inner: number, half: number, side: 1 | -1): string {
  const tip = [50 + r * Math.sin(a), 50 - r * Math.cos(a)];
  const b = a + side * half;
  const base = [50 + inner * Math.sin(b), 50 - inner * Math.cos(b)];
  return `50,50 ${tip[0].toFixed(2)},${tip[1].toFixed(2)} ${base[0].toFixed(2)},${base[1].toFixed(2)}`;
}

const POINTS: { a: number; r: number }[] = [];
for (let i = 0; i < 8; i++) {
  POINTS.push({ a: (i * Math.PI) / 4, r: i % 2 === 0 ? 35 : 21 });
}

export function Medallion({ class: className, title, trim = false, metal = "gold", trimFinish = "silver", enamel = "pink" }: MedallionProps) {
  const id = useSvgId("jkm");
  const m = METALS[metal];
  const t = TRIMS[trimFinish];
  return (
    <svg
      class={`jk-medallion${className ? ` ${className}` : ""}`}
      viewBox="0 0 100 100"
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : "true"}
    >
      <defs>
        <linearGradient id={`${id}-bezel`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color={m.bezel[0]} />
          <stop offset="0.3" stop-color={m.bezel[1]} />
          <stop offset="0.5" stop-color={m.bezel[2]} />
          <stop offset="0.7" stop-color={m.bezel[3]} />
          <stop offset="1" stop-color={m.bezel[4]} />
        </linearGradient>
        <radialGradient id={`${id}-enamel`} cx="0.45" cy="0.4" r="0.65">
          <stop offset="0" stop-color={ENAMELS[enamel][0]} />
          <stop offset="0.5" stop-color={ENAMELS[enamel][1]} />
          <stop offset="1" stop-color={ENAMELS[enamel][2]} />
        </radialGradient>
        <linearGradient id={`${id}-silver`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color={t[0]} />
          <stop offset="0.3" stop-color={t[1]} />
          <stop offset="0.5" stop-color={t[2]} />
          <stop offset="0.7" stop-color={t[3]} />
          <stop offset="1" stop-color={t[4]} />
        </linearGradient>
        <linearGradient id={`${id}-gloss`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#fff" stop-opacity="0.5" />
          <stop offset="1" stop-color="#fff" stop-opacity="0" />
        </linearGradient>
      </defs>
      <circle cx="50" cy="51.5" r="48" fill="#000" opacity="0.45" />
      <circle cx="50" cy="50" r="48" fill={`url(#${id}-bezel)`} />
      <circle cx="50" cy="50" r="43.5" fill="none" stroke={m.line} stroke-width="1" />
      {trim && <circle cx="50" cy="50" r="43" fill={`url(#${id}-silver)`} stroke="#2b2f35" stroke-width="0.5" />}
      <circle cx="50" cy="50" r={trim ? 40.4 : 42.5} fill={`url(#${id}-enamel)`} stroke={trim ? "#2b2f35" : "none"} stroke-width="0.5" />
      {/* Fine engine-turned rings in the enamel. */}
      <circle cx="50" cy="50" r="37.5" fill="none" stroke="#fff" stroke-opacity="0.13" stroke-width="0.6" />
      <circle cx="50" cy="50" r="31" fill="none" stroke="#fff" stroke-opacity="0.1" stroke-width="0.5" />
      {/* Star: short (diagonal) points first, long (cardinal) points on top. */}
      {[1, 3, 5, 7, 0, 2, 4, 6].map((i) => {
        const p = POINTS[i];
        const inner = i % 2 === 0 ? 8.5 : 7;
        return (
          <g key={i}>
            <polygon points={facet(p.a, p.r, inner, Math.PI / 8, -1)} fill={i % 2 === 0 ? m.facets[0] : m.facets[1]} />
            <polygon points={facet(p.a, p.r, inner, Math.PI / 8, 1)} fill={i % 2 === 0 ? m.facets[2] : m.facets[3]} />
          </g>
        );
      })}
      <circle cx="50" cy="50" r="4.2" fill={`url(#${id}-bezel)`} stroke={m.line} stroke-width="0.6" />
      <ellipse cx="50" cy="30" rx="30" ry="15" fill={`url(#${id}-gloss)`} />
    </svg>
  );
}

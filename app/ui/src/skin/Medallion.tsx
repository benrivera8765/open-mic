// SPDX-License-Identifier: GPL-3.0-or-later
// Compass-star medallion (docs/DESIGN.md): an eight-point star, every point split
// into a light and a dark facet, on a red enamel disc inside a gold-chrome bezel.
// A generic heraldic motif drawn from scratch in SVG.

import { useSvgId } from "./svg-id";

export interface MedallionProps {
  class?: string;
  /** Accessible label; decorative (hidden from assistive tech) when absent. */
  title?: string;
}

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

export function Medallion({ class: className, title }: MedallionProps) {
  const id = useSvgId("jkm");
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
          <stop offset="0" stop-color="#fff6d8" />
          <stop offset="0.3" stop-color="#d9b25a" />
          <stop offset="0.5" stop-color="#7a5a1c" />
          <stop offset="0.7" stop-color="#e9cf86" />
          <stop offset="1" stop-color="#8a6624" />
        </linearGradient>
        <radialGradient id={`${id}-enamel`} cx="0.45" cy="0.4" r="0.65">
          <stop offset="0" stop-color="#ff4a5f" />
          <stop offset="0.5" stop-color="#c8102e" />
          <stop offset="1" stop-color="#5e0513" />
        </radialGradient>
        <linearGradient id={`${id}-gloss`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#fff" stop-opacity="0.5" />
          <stop offset="1" stop-color="#fff" stop-opacity="0" />
        </linearGradient>
      </defs>
      <circle cx="50" cy="51.5" r="48" fill="#000" opacity="0.45" />
      <circle cx="50" cy="50" r="48" fill={`url(#${id}-bezel)`} />
      <circle cx="50" cy="50" r="43.5" fill="none" stroke="#5a3f10" stroke-width="1" />
      <circle cx="50" cy="50" r="42.5" fill={`url(#${id}-enamel)`} />
      {/* Fine engine-turned rings in the enamel. */}
      <circle cx="50" cy="50" r="37.5" fill="none" stroke="#fff" stroke-opacity="0.13" stroke-width="0.6" />
      <circle cx="50" cy="50" r="31" fill="none" stroke="#fff" stroke-opacity="0.1" stroke-width="0.5" />
      {/* Star: short (diagonal) points first, long (cardinal) points on top. */}
      {[1, 3, 5, 7, 0, 2, 4, 6].map((i) => {
        const p = POINTS[i];
        const inner = i % 2 === 0 ? 8.5 : 7;
        return (
          <g key={i}>
            <polygon points={facet(p.a, p.r, inner, Math.PI / 8, -1)} fill={i % 2 === 0 ? "#fff1bf" : "#f3d57a"} />
            <polygon points={facet(p.a, p.r, inner, Math.PI / 8, 1)} fill={i % 2 === 0 ? "#d09a2c" : "#a8761c"} />
          </g>
        );
      })}
      <circle cx="50" cy="50" r="4.2" fill={`url(#${id}-bezel)`} stroke="#5a3f10" stroke-width="0.6" />
      <ellipse cx="50" cy="30" rx="30" ry="15" fill={`url(#${id}-gloss)`} />
    </svg>
  );
}

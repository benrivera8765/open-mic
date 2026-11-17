// SPDX-License-Identifier: GPL-3.0-or-later
// Diamond-lattice grille (docs/DESIGN.md): warm chrome bars crossing over a red
// speaker cloth that is lit from behind, with a stud on every crossing. One SVG
// pattern tile, so it fills any box without stretching the diamonds.

import { useSvgId } from "./svg-id";

export interface GrilleProps {
  class?: string;
  /** Width of one diamond in CSS px (height is 1.35 x width, like the photos). */
  cell?: number;
  /** Backlight on (default) or a dark, unlit cloth. */
  lit?: boolean;
}

export function Grille({ class: className, cell = 26, lit = true }: GrilleProps) {
  const id = useSvgId("jkg");
  const w = cell;
  const h = cell * 1.35;
  const bw = Math.max(1.6, cell * 0.11); // bar width
  // Diagonals through the tile, plus copies shifted one tile sideways, so strokes
  // are not cut off at the tile corners.
  const diag: string[] = [];
  for (const dx of [-w, 0, w]) {
    diag.push(`M${dx} 0 L${dx + w} ${h}`);
    diag.push(`M${dx + w} 0 L${dx} ${h}`);
  }
  const d = diag.join(" ");
  const studs: [number, number][] = [
    [w / 2, h / 2],
    [0, 0],
    [w, 0],
    [0, h],
    [w, h],
  ];
  return (
    <svg class={`jk-grille${className ? ` ${className}` : ""}`} aria-hidden="true" preserveAspectRatio="none">
      <defs>
        <radialGradient id={`${id}-cloth`} cx="0.5" cy="0.55" r="0.75">
          <stop offset="0" stop-color={lit ? "#c3203f" : "#5a0a18"} />
          <stop offset="0.55" stop-color={lit ? "#7c0c22" : "#3a0510"} />
          <stop offset="1" stop-color="#2a030b" />
        </radialGradient>
        <pattern id={`${id}-weave`} width="3" height="3" patternUnits="userSpaceOnUse">
          <rect width="3" height="3" fill="#000" opacity="0" />
          <circle cx="1.5" cy="1.5" r="0.8" fill="#000" opacity="0.28" />
        </pattern>
        <radialGradient id={`${id}-stud`} cx="0.35" cy="0.3" r="0.8">
          <stop offset="0" stop-color="#fffbe9" />
          <stop offset="0.45" stop-color="#d8bd73" />
          <stop offset="1" stop-color="#5e4614" />
        </radialGradient>
        <pattern id={`${id}-lattice`} width={w} height={h} patternUnits="userSpaceOnUse">
          {/* Shadow cast on the cloth, body, highlight. */}
          <path d={d} stroke="#000" stroke-opacity="0.55" stroke-width={bw + 1.6} transform="translate(0 1.4)" fill="none" />
          <path d={d} stroke="#8e7432" stroke-width={bw + 0.8} fill="none" />
          <path d={d} stroke="#e8d7a4" stroke-width={bw * 0.7} fill="none" />
          <path d={d} stroke="#fffaf0" stroke-opacity="0.85" stroke-width={Math.max(0.6, bw * 0.22)} transform="translate(0 -0.4)" fill="none" />
          {studs.map(([x, y]) => (
            <circle key={`${x}-${y}`} cx={x} cy={y} r={bw * 0.95} fill={`url(#${id}-stud)`} stroke="#3d2c08" stroke-width="0.5" />
          ))}
        </pattern>
      </defs>
      <rect width="100%" height="100%" fill={`url(#${id}-cloth)`} />
      <rect width="100%" height="100%" fill={`url(#${id}-weave)`} />
      <rect width="100%" height="100%" fill={`url(#${id}-lattice)`} />
    </svg>
  );
}

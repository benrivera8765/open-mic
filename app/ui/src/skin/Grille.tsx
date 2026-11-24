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
  /** The lattice bars and studs: warm gold chrome (default), silver chrome (owner, 2026-11-22) or iron. */
  bars?: "gold" | "silver" | "iron";
  /** The speaker cloth's colour, following the theme's glow: red (pink glow, default), ember, aqua, violet. */
  cloth?: "pink" | "ember" | "aqua" | "violet" | "amber" | "lime";
}

/** Cloth gradient (lit: centre, middle; unlit: centre, middle; edge) per glow colour. */
const CLOTH = {
  pink: { lit: ["#c3203f", "#7c0c22"], dark: ["#5a0a18", "#3a0510"], edge: "#2a030b" },
  ember: { lit: ["#ff6a14", "#9a2a06"], dark: ["#6a1a04", "#3a0c02"], edge: "#200602" },
  aqua: { lit: ["#1c9ad8", "#0a3e70"], dark: ["#0a3050", "#061a30"], edge: "#020c1a" },
  violet: { lit: ["#9a4ae8", "#401478"], dark: ["#3a1060", "#200838"], edge: "#0e0420" },
  amber: { lit: ["#e8a020", "#7a4a08"], dark: ["#5a3a08", "#3a2404"], edge: "#1e1202" },
  lime: { lit: ["#5ac830", "#1e6a10"], dark: ["#1e4a10", "#10300a"], edge: "#061a04" },
} as const;

/** Bar colours: shadow edge, body, highlight; stud gradient light, mid, dark, outline. */
const BAR_FINISH = {
  gold: { edge: "#8e7432", body: "#e8d7a4", hi: "#fffaf0", stud: ["#fffbe9", "#d8bd73", "#5e4614"], ring: "#3d2c08" },
  silver: { edge: "#5f6770", body: "#dfe4ea", hi: "#ffffff", stud: ["#ffffff", "#bcc4cd", "#3f464e"], ring: "#262a30" },
  iron: { edge: "#101214", body: "#4a5056", hi: "#9aa0a6", stud: ["#b4bac0", "#50565c", "#141618"], ring: "#08090a" },
} as const;

export function Grille({ class: className, cell = 26, lit = true, bars = "gold", cloth = "pink" }: GrilleProps) {
  const id = useSvgId("jkg");
  const fin = BAR_FINISH[bars];
  const cl = CLOTH[cloth];
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
          <stop offset="0" stop-color={lit ? cl.lit[0] : cl.dark[0]} />
          <stop offset="0.55" stop-color={lit ? cl.lit[1] : cl.dark[1]} />
          <stop offset="1" stop-color={cl.edge} />
        </radialGradient>
        <pattern id={`${id}-weave`} width="3" height="3" patternUnits="userSpaceOnUse">
          <rect width="3" height="3" fill="#000" opacity="0" />
          <circle cx="1.5" cy="1.5" r="0.8" fill="#000" opacity="0.28" />
        </pattern>
        <radialGradient id={`${id}-stud`} cx="0.35" cy="0.3" r="0.8">
          <stop offset="0" stop-color={fin.stud[0]} />
          <stop offset="0.45" stop-color={fin.stud[1]} />
          <stop offset="1" stop-color={fin.stud[2]} />
        </radialGradient>
        <pattern id={`${id}-lattice`} width={w} height={h} patternUnits="userSpaceOnUse">
          {/* Shadow cast on the cloth, body, highlight. */}
          <path d={d} stroke="#000" stroke-opacity="0.55" stroke-width={bw + 1.6} transform="translate(0 1.4)" fill="none" />
          <path d={d} stroke={fin.edge} stroke-width={bw + 0.8} fill="none" />
          <path d={d} stroke={fin.body} stroke-width={bw * 0.7} fill="none" />
          <path d={d} stroke={fin.hi} stroke-opacity="0.85" stroke-width={Math.max(0.6, bw * 0.22)} transform="translate(0 -0.4)" fill="none" />
          {studs.map(([x, y]) => (
            <circle key={`${x}-${y}`} cx={x} cy={y} r={bw * 0.95} fill={`url(#${id}-stud)`} stroke={fin.ring} stroke-width="0.5" />
          ))}
        </pattern>
      </defs>
      <rect width="100%" height="100%" fill={`url(#${id}-cloth)`} />
      <rect width="100%" height="100%" fill={`url(#${id}-weave)`} />
      <rect width="100%" height="100%" fill={`url(#${id}-lattice)`} />
    </svg>
  );
}

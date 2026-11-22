// SPDX-License-Identifier: GPL-3.0-or-later
// The app's own badge (docs/DESIGN.md "Badge"): an original design, NOT taken from
// any jukebox maker. A long, round-cornered lozenge (an elongated hexagon, no
// wings, no centre disc) with a polished chrome rim, a red enamel field with a
// cream pinstripe, the app name in a cream retro script with a swash underline,
// and one tiny four-point star. Drawn entirely in SVG.
//
// The name comes from branding.json, the only place it is written down
// (docs/ARCHITECTURE.md section 9): pass it as `name` (AppInfo.name from the Rust
// core); without the prop the badge reads it from the page, where the Vite build
// has injected it (<meta name="application-name"> or the <title>).

import { useSvgId } from "./svg-id";

export interface BadgeProps {
  /** The product name. Defaults to the name injected into the page from branding.json. */
  name?: string;
  class?: string;
  /** Accessible label; defaults to the name. */
  title?: string;
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

/** Widest the name may be inside the enamel field (viewBox units). */
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

export function Badge({ name, class: className, title }: BadgeProps) {
  const id = useSvgId("jkb");
  const text = (name ?? appName()) || " ";
  const { fontSize, squeeze } = fitBadgeText(text);
  return (
    <svg
      class={`jk-badge${className ? ` ${className}` : ""}`}
      viewBox="0 0 300 120"
      role="img"
      aria-label={title ?? text}
      preserveAspectRatio="xMidYMid meet"
    >
      <defs>
        <linearGradient id={`${id}-chrome`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#fff6d8" />
          <stop offset="0.22" stop-color="#ecd38e" />
          <stop offset="0.48" stop-color="#9a7228" />
          <stop offset="0.55" stop-color="#563c0f" />
          <stop offset="0.64" stop-color="#cca34a" />
          <stop offset="0.86" stop-color="#f9e9b9" />
          <stop offset="1" stop-color="#bc8e35" />
        </linearGradient>
        <linearGradient id={`${id}-chrome2`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#825f21" />
          <stop offset="0.5" stop-color="#fbecc0" />
          <stop offset="1" stop-color="#946d26" />
        </linearGradient>
        <linearGradient id={`${id}-enamel`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#ef3a55" />
          <stop offset="0.45" stop-color="#c8102e" />
          <stop offset="1" stop-color="#6d0717" />
        </linearGradient>
        <radialGradient id={`${id}-gloss`} cx="0.5" cy="0" r="0.75" fx="0.5" fy="0">
          <stop offset="0" stop-color="#fff" stop-opacity="0.55" />
          <stop offset="0.55" stop-color="#fff" stop-opacity="0.08" />
          <stop offset="1" stop-color="#fff" stop-opacity="0" />
        </radialGradient>
        <clipPath id={`${id}-clip`}>
          <path d={FIELD} />
        </clipPath>
        <filter id={`${id}-shadow`} x="-20%" y="-30%" width="140%" height="160%">
          <feGaussianBlur stdDeviation="3" />
        </filter>
      </defs>
      {/* Drop shadow under the whole badge. */}
      <path d={OUTLINE} fill="#000" opacity="0.55" transform="translate(0 4)" filter={`url(#${id}-shadow)`} stroke="#000" stroke-width="16" stroke-linejoin="round" />
      {/* Chrome rim: thick round-joined stroke + fill, then a reversed bevel. */}
      <path d={OUTLINE} fill={`url(#${id}-chrome)`} stroke={`url(#${id}-chrome)`} stroke-width="16" stroke-linejoin="round" />
      <path d={OUTLINE} fill="none" stroke="#24252a" stroke-width="17.5" stroke-linejoin="round" opacity="0.35" />
      <path d={OUTLINE} fill={`url(#${id}-chrome)`} stroke={`url(#${id}-chrome)`} stroke-width="15" stroke-linejoin="round" />
      <path d={FIELD} fill={`url(#${id}-chrome2)`} stroke={`url(#${id}-chrome2)`} stroke-width="8" stroke-linejoin="round" />
      {/* Enamel field. */}
      <path d={FIELD} fill={`url(#${id}-enamel)`} stroke="#4a0410" stroke-width="1.2" stroke-linejoin="round" />
      <path d={PIN} fill="none" stroke="#f6efd9" stroke-width="1.3" stroke-linejoin="round" opacity="0.85" />
      {/* Name: dark under-print for depth, then cream script. */}
      <text
        class="jk-badge-text"
        x="150"
        y="70"
        text-anchor="middle"
        font-size={fontSize}
        fill="#3d0410"
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
        fill="#f6efd9"
        textLength={squeeze ? TEXT_MAX_W : undefined}
        lengthAdjust={squeeze ? "spacingAndGlyphs" : undefined}
      >
        {text}
      </text>
      {/* Swash underline, tapering at both ends. */}
      <path d="M82 83 Q150 75 222 79 Q151 79.5 82 85.6 Z" fill="#f6efd9" opacity="0.92" />
      {/* The tiny star. */}
      <path d={star(229, 41, 6.5)} fill="#f6efd9" />
      <circle cx="229" cy="41" r="1.4" fill="#ffd23f" />
      {/* Gloss over the enamel (clipped to the field). */}
      <g clip-path={`url(#${id}-clip)`}>
        <ellipse cx="150" cy="30" rx="120" ry="30" fill={`url(#${id}-gloss)`} />
      </g>
    </svg>
  );
}

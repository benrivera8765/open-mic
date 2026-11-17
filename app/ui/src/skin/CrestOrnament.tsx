// SPDX-License-Identifier: GPL-3.0-or-later
// The frame's default crest ornament (docs/DESIGN.md "Arch crest"): a stylised
// music note on a red enamel oval, held by a pair of chrome C-scrolls. Our own
// design, drawn in SVG. The frame shows it when no `crest` content is passed.

import { useSvgId } from "./svg-id";

export function CrestOrnament({ class: className }: { class?: string }) {
  const id = useSvgId("jkc");
  const scroll = "M70 78 C44 86 18 66 25 42 C30 24 54 22 56 38 C57 49 44 51 41 42";
  return (
    <svg class={`jk-crest-ornament${className ? ` ${className}` : ""}`} viewBox="0 0 200 100" aria-hidden="true">
      <defs>
        <linearGradient id={`${id}-chrome`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#ffffff" />
          <stop offset="0.35" stop-color="#bfc3cb" />
          <stop offset="0.55" stop-color="#55585f" />
          <stop offset="0.75" stop-color="#e6e8ec" />
          <stop offset="1" stop-color="#7c7f87" />
        </linearGradient>
        <radialGradient id={`${id}-enamel`} cx="0.45" cy="0.35" r="0.7">
          <stop offset="0" stop-color="#ff5068" />
          <stop offset="0.55" stop-color="#c8102e" />
          <stop offset="1" stop-color="#5e0513" />
        </radialGradient>
      </defs>
      <g fill="none" stroke-linecap="round">
        <path d={scroll} stroke="#000" stroke-opacity="0.5" stroke-width="9" transform="translate(0 2)" />
        <path d={scroll} stroke={`url(#${id}-chrome)`} stroke-width="7" />
        <g transform="translate(200 0) scale(-1 1)">
          <path d={scroll} stroke="#000" stroke-opacity="0.5" stroke-width="9" transform="translate(0 2)" />
          <path d={scroll} stroke={`url(#${id}-chrome)`} stroke-width="7" />
        </g>
      </g>
      {/* A chrome stud above the oval. */}
      <circle cx="100" cy="17" r="5" fill={`url(#${id}-chrome)`} stroke="#3a3c42" stroke-width="0.6" />
      <ellipse cx="100" cy="56" rx="35" ry="32" fill="#000" opacity="0.45" transform="translate(0 2)" />
      <ellipse cx="100" cy="55" rx="35" ry="32" fill={`url(#${id}-chrome)`} />
      <ellipse cx="100" cy="55" rx="29" ry="26.5" fill={`url(#${id}-enamel)`} />
      {/* Eighth note. */}
      <g fill="#f6efd9">
        <ellipse cx="93" cy="67" rx="8.2" ry="6" transform="rotate(-24 93 67)" />
        <rect x="99" y="35" width="3" height="32" rx="1" />
        <path d="M100 35 C110 40 118 46 111 60 C113 49 107 45 100 43 Z" />
      </g>
      <ellipse cx="100" cy="40" rx="22" ry="9" fill="#fff" opacity="0.18" />
    </svg>
  );
}

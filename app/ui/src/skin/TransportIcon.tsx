// SPDX-License-Identifier: GPL-3.0-or-later
// Drawn transport symbols for plate buttons. Unicode media symbols (U+23EE etc.)
// render as colour emoji in some WebKit builds and differ in size between fonts,
// so the skin draws its own: monochrome (currentColor), sized to the label font.

export type TransportIconName = "play" | "pause" | "stop" | "restart" | "record" | "prev" | "next" | "back" | "forward";

const PATHS: Record<TransportIconName, string> = {
  play: "M7 4.5 L19.5 12 L7 19.5 Z",
  pause: "M6 5 H10 V19 H6 Z M14 5 H18 V19 H14 Z",
  stop: "M6 6 H18 V18 H6 Z",
  restart: "M5 5 H8 V19 H5 Z M19 5 V19 L9 12 Z",
  prev: "M5 5 H8 V19 H5 Z M19 5 V19 L9 12 Z",
  next: "M16 5 H19 V19 H16 Z M5 5 V19 L15 12 Z",
  record: "M12 5.5 A6.5 6.5 0 1 1 11.99 5.5 Z",
  // Skip back / forward a few seconds: two small triangles.
  back: "M11.5 6 V18 L3 12 Z M21 6 V18 L12.5 12 Z",
  forward: "M3 6 V18 L11.5 12 Z M12.5 6 V18 L21 12 Z",
};

export function TransportIcon({ name, class: className }: { name: TransportIconName; class?: string }) {
  return (
    <svg class={`jk-icon${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d={PATHS[name]} fill="currentColor" />
    </svg>
  );
}

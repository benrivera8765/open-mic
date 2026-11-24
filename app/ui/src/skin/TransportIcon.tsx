// SPDX-License-Identifier: GPL-3.0-or-later
// Drawn transport symbols for plate buttons. Unicode media symbols (U+23EE etc.)
// render as colour emoji in some WebKit builds and differ in size between fonts,
// so the skin draws its own: monochrome (currentColor), sized to the label font.

export type TransportIconName = "play" | "pause" | "stop" | "restart" | "record" | "prev" | "next" | "back" | "forward" | "back5" | "forward5" | "trash";

const PATHS: Record<Exclude<TransportIconName, "back5" | "forward5">, string> = {
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
  // Delete (Library): a bin with its lid and two slots (even-odd fill cuts the slots).
  trash:
    "M9 2.5 H15 V4 H20 V6.5 H4 V4 H9 Z M5.5 8 H18.5 L17.4 20 Q17.25 21.5 15.75 21.5 H8.25 Q6.75 21.5 6.6 20 Z M9.1 10.5 V19 H10.7 V10.5 Z M13.3 10.5 V19 H14.9 V10.5 Z",
};

/**
 * Skip 5 seconds (owner, 2026-11-23): a circular arrow turning back or forward round a
 * "5", so the buttons say how far they jump.
 */
function SkipFive({ forward, className }: { forward: boolean; className?: string }) {
  const arc = forward ? "M12 5.4 A7.2 7.2 0 1 0 19.2 12.6" : "M12 5.4 A7.2 7.2 0 1 1 4.8 12.6";
  const head = forward ? "M11.2 1.8 L17 5.4 L11.2 9 Z" : "M12.8 1.8 L7 5.4 L12.8 9 Z";
  return (
    <svg class={`jk-icon jk-icon--skip5${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d={arc} fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
      <path d={head} fill="currentColor" />
      <text x="12" y="17" text-anchor="middle" font-size="11" font-weight="700" fill="currentColor" style="font-family: var(--jk-font-display)">
        5
      </text>
    </svg>
  );
}

export function TransportIcon({ name, class: className }: { name: TransportIconName; class?: string }) {
  if (name === "back5" || name === "forward5") return <SkipFive forward={name === "forward5"} className={className} />;
  return (
    <svg class={`jk-icon${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d={PATHS[name]} fill="currentColor" fill-rule="evenodd" />
    </svg>
  );
}

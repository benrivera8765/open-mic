// SPDX-License-Identifier: GPL-3.0-or-later
// A selection plate button (docs/DESIGN.md, photo 2's small "MAKE SELECTIONS"
// plate): a polished chrome bezel round a black glass face, white condensed
// capitals. `active` lights it from behind in the jukebox pink, like a lit
// selector button. A real <button>, so focus, Enter/Space and screen readers work.

import type { ComponentChildren } from "preact";

export interface PlateButtonProps {
  label: ComponentChildren;
  onClick?: (event: MouseEvent) => void;
  /** Lit (toggled on). Leave undefined for a plain push button. */
  active?: boolean;
  disabled?: boolean;
  title?: string;
  /** Accessible name when the label is only a symbol (e.g. a play triangle). */
  ariaLabel?: string;
  /** "sm" | "md" (default) | "lg". */
  size?: "sm" | "md" | "lg";
  class?: string;
}

export function PlateButton({
  label,
  onClick,
  active,
  disabled = false,
  title,
  ariaLabel,
  size = "md",
  class: className,
}: PlateButtonProps) {
  const cls = [
    "jk-plate",
    `jk-plate--${size}`,
    active ? "jk-plate--active" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      class={cls}
      onClick={onClick}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel}
      // Toggle semantics only when the caller manages an active state.
      aria-pressed={active === undefined ? undefined : active ? "true" : "false"}
    >
      <span class="jk-plate-glass">
        <span class="jk-plate-label">{label}</span>
      </span>
    </button>
  );
}

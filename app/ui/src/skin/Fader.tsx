// SPDX-License-Identifier: GPL-3.0-or-later
// A mixer fader styled as a jukebox fitting: a black glass slot in a chrome bezel,
// a light inside the slot that rises with the level, a ribbed chrome cap, and a
// small plate label underneath. Vertical by default (the console is wide but not
// tall); `orientation="horizontal"` for side panels.
//
// Mixer levels are cheap for the engine to change, so onChange fires continuously
// while dragging. Keyboard: arrows +-0.02, PageUp/PageDown +-0.1, Home/End.

import { useRef } from "preact/hooks";

export interface FaderProps {
  /** 0..1 */
  value: number;
  onChange(value: number): void;
  label: string;
  disabled?: boolean;
  title?: string;
  /** Read-out for assistive tech and the small display; defaults to a percentage. */
  valueText?: string;
  orientation?: "vertical" | "horizontal";
  class?: string;
}

function clamp01(v: number): number {
  if (!Number.isFinite(v)) return 0;
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

export function Fader({
  value,
  onChange,
  label,
  disabled = false,
  title,
  valueText,
  orientation = "vertical",
  class: className,
}: FaderProps) {
  const v = clamp01(value);
  const trackRef = useRef<HTMLDivElement>(null);
  const dragId = useRef<number | null>(null);
  const vertical = orientation === "vertical";
  const text = valueText ?? `${Math.round(v * 100)}%`;

  const set = (n: number) => {
    const c = clamp01(Math.round(n * 1000) / 1000);
    if (c !== v) onChange(c);
  };

  const fromPointer = (e: PointerEvent) => {
    const el = trackRef.current;
    if (!el) return;
    const box = el.getBoundingClientRect();
    // The cap's travel is the track minus the cap; the CSS keeps the cap at
    // 22 % of the track length (see .jk-fader-cap).
    if (vertical) {
      const cap = box.height * 0.22;
      set(1 - (e.clientY - box.top - cap / 2) / Math.max(1, box.height - cap));
    } else {
      const cap = box.width * 0.22;
      set((e.clientX - box.left - cap / 2) / Math.max(1, box.width - cap));
    }
  };

  const onPointerDown = (e: PointerEvent) => {
    if (disabled || e.button !== 0) return;
    dragId.current = e.pointerId;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    (e.currentTarget as HTMLElement).focus({ preventScroll: true });
    fromPointer(e);
    e.preventDefault();
  };
  const onPointerMove = (e: PointerEvent) => {
    if (dragId.current === e.pointerId) fromPointer(e);
  };
  const onPointerUp = (e: PointerEvent) => {
    if (dragId.current === e.pointerId) dragId.current = null;
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // Ctrl/Cmd/Alt combinations belong to the app and the system.
    if (disabled || e.ctrlKey || e.metaKey || e.altKey) return;
    const steps: Record<string, number> = {
      ArrowUp: 0.02,
      ArrowRight: 0.02,
      ArrowDown: -0.02,
      ArrowLeft: -0.02,
      PageUp: 0.1,
      PageDown: -0.1,
    };
    if (e.key in steps) set(v + steps[e.key]);
    else if (e.key === "Home") set(0);
    else if (e.key === "End") set(1);
    else return;
    e.preventDefault();
  };

  const onWheel = (e: WheelEvent) => {
    if (disabled) return;
    e.preventDefault();
    const step = e.deltaMode !== 0 || Math.abs(e.deltaY) >= 50 ? 0.05 : Math.abs(e.deltaY) / 800;
    set(v + (e.deltaY < 0 ? step : -step));
  };

  return (
    <div
      class={`jk-fader jk-fader--${orientation}${disabled ? " jk-fader--disabled" : ""}${className ? ` ${className}` : ""}`}
      title={title}
      style={`--jk-fader-v: ${v}`}
    >
      <div
        ref={trackRef}
        class="jk-fader-track"
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={label}
        aria-orientation={orientation}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(v * 100)}
        aria-valuetext={text}
        aria-disabled={disabled ? "true" : undefined}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={onKeyDown}
        onWheel={onWheel}
      >
        <div class="jk-fader-slot">
          <div class="jk-fader-fill" />
        </div>
        <div class="jk-fader-cap" />
      </div>
      <div class="jk-fader-label">
        <span class="jk-fader-name">{label}</span>
        <span class="jk-fader-value">{text}</span>
      </div>
    </div>
  );
}

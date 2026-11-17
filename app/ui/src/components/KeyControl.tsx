// SPDX-License-Identifier: GPL-3.0-or-later
// KEY CONTROL, the most used control in the app (WORKING ORDER 4.5): -12..+12
// semitones, "A major → B major (+2)", big − / + buttons, keyboard [ ] and − / +
// (handled app-wide in App.tsx), and the mouse wheel over the control.
// Plain styling here; the skin restyles it as a chrome dial through the class names:
// .key-control, .key-control__down, .key-control__up, .key-control__face,
// .key-control__label, .key-control__value, .key-control__scale, .key-control__tick,
// .key-control__tick--on, .key-control__tick--zero, .key-control__reset.
import { useRef } from "preact/hooks";
import { KEY_MAX, KEY_MIN, keyLabel, signed } from "../lib/keys";
import { setKey, shiftKey, useApp } from "../state/app";

/** Wheel distance (in pixels) per semitone; trackpads send many small deltas. */
const WHEEL_STEP = 60;

export function KeyControl({ compact = false }: { compact?: boolean }) {
  const song = useApp((s) => s.song);
  const semis = useApp((s) => s.songSettings?.semitones ?? 0);
  const wheel = useRef(0);
  const disabled = !song;
  const label = song ? keyLabel(song.summary.key, semis) : "No song loaded";

  const onWheel = (e: WheelEvent) => {
    if (disabled) return;
    e.preventDefault();
    // Normalise line/page deltas to pixels, then step once per WHEEL_STEP.
    const px = e.deltaMode === 1 ? e.deltaY * 40 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
    wheel.current -= px; // wheel up (negative deltaY) = key up
    while (Math.abs(wheel.current) >= WHEEL_STEP) {
      const dir = Math.sign(wheel.current);
      shiftKey(dir);
      wheel.current -= dir * WHEEL_STEP;
    }
  };

  return (
    <div
      class={`key-control${compact ? " key-control--compact" : ""}`}
      role="group"
      aria-label="Key"
      onWheel={onWheel as unknown as (e: Event) => void}
      data-semitones={semis}
    >
      <button type="button" class="key-control__down" aria-label="Key down one semitone" title="Key down ( [ or − )" disabled={disabled || semis <= KEY_MIN} onClick={() => shiftKey(-1)}>
        <span aria-hidden="true">−</span>
      </button>
      <div class="key-control__face">
        <span class="key-control__caption">Key</span>
        <output class="key-control__label" aria-live="polite">
          {label}
        </output>
        <span class="key-control__value" aria-hidden="true">
          {signed(semis)}
        </span>
        {!compact && (
          <div class="key-control__scale" aria-hidden="true">
            {Array.from({ length: KEY_MAX - KEY_MIN + 1 }, (_, i) => {
              const v = i + KEY_MIN;
              return (
                <span
                  key={v}
                  class={`key-control__tick${v === semis ? " key-control__tick--on" : ""}${v === 0 ? " key-control__tick--zero" : ""}`}
                  onClick={() => !disabled && setKey(v)}
                />
              );
            })}
          </div>
        )}
      </div>
      <button type="button" class="key-control__up" aria-label="Key up one semitone" title="Key up ( ] or + )" disabled={disabled || semis >= KEY_MAX} onClick={() => shiftKey(1)}>
        <span aria-hidden="true">+</span>
      </button>
      {!compact && (
        <button type="button" class="key-control__reset button button--small" disabled={disabled || semis === 0} onClick={() => setKey(0)}>
          Original key
        </button>
      )}
    </div>
  );
}

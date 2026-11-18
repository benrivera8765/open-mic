// SPDX-License-Identifier: GPL-3.0-or-later
// KEY CONTROL, the most used control in the app (WORKING ORDER 4.5): -12..+12
// semitones, "A major → B major (+2)", big − / + buttons, keyboard [ ] and − / +
// (handled app-wide in App.tsx), and the mouse wheel over the control.
// Plain styling here; the skin restyles it as a chrome dial through the class names:
// .key-control, .key-control__down, .key-control__up, .key-control__face,
// .key-control__label, .key-control__value, .key-control__scale, .key-control__tick,
// .key-control__tick--on, .key-control__tick--zero, .key-control__reset.
import { useRef } from "preact/hooks";
import type { KeyPreparing } from "../api/types";
import { KEY_MAX, KEY_MIN, keyLabel, signed } from "../lib/keys";
import { msSinceSeek, setKey, shiftKey, useApp } from "../state/app";

/** "Preparing key +2… 40%" while a key (or mix) is rendered ahead (WORK_ORDER 4.5). */
export function keyPrepareText(p: KeyPreparing | null | undefined): string | null {
  if (!p) return null;
  const pct = Math.max(0, Math.min(100, Math.floor((Number.isFinite(p.progress) ? p.progress : 0) * 100)));
  return p.reason === "mix" ? `Preparing the new mix (key ${signed(p.semitones)})… ${pct}%` : `Preparing key ${signed(p.semitones)}… ${pct}%`;
}

/**
 * The progress shown never steps backwards within one key change: the core's figure can
 * dip by about 1 % (HANDOFF 2, key change on slow computers), so the note keeps the
 * highest value shown so far for the same key and reason, and starts again from the
 * core's figure for a new key, a new reason, or after the preparation has ended.
 * A seek restarts the core's figure from the new position (measured in the release app:
 * 41 % -> 0 %), so shortly after a seek (`afterSeek`) a drop of more than 2 points is
 * shown as it is instead of holding the old figure (D-215).
 */
export function monotonicPrepare(prev: KeyPreparing | null, next: KeyPreparing | null | undefined, afterSeek = false): KeyPreparing | null {
  if (!next) return null;
  const progress = Number.isFinite(next.progress) ? next.progress : 0;
  if (prev && prev.semitones === next.semitones && prev.reason === next.reason) {
    if (afterSeek && progress < prev.progress - 0.02) return { ...next, progress };
    return { ...next, progress: Math.max(prev.progress, progress) };
  }
  return { ...next, progress };
}

/** How long after a seek a drop in the core's figure counts as a restart (D-215). */
const SEEK_RESTART_MS = 1000;

/** The previous key keeps playing meanwhile; shown next to the key control. */
export function KeyPrepareNote() {
  const raw = useApp((s) => s.player?.key_preparing ?? null);
  const shown = useRef<KeyPreparing | null>(null);
  shown.current = monotonicPrepare(shown.current, raw, msSinceSeek() < SEEK_RESTART_MS);
  const text = keyPrepareText(shown.current);
  if (!text) return null;
  return (
    <p class="key-prepare" role="status" title="This computer renders the new key ahead so playback never stutters; the previous key plays until it is ready.">
      {text}
    </p>
  );
}

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
      <KeyPrepareNote />
    </div>
  );
}

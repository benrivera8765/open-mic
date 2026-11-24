// SPDX-License-Identifier: GPL-3.0-or-later
// The key dial: the app's most used control (WORK_ORDER 4.5 and 4.7), so it is the
// biggest and most physical thing on the console. A knurled chrome ring round a
// black glass face; one detent per semitone over a 270-degree sweep; a lit pointer
// that springs into each detent; "-" and "+" plates either side; the key names
// ("A major -> B major (+2)") on a glass strip below.
//
// Input: drag round the ring (or up/down near the centre), mouse wheel / trackpad,
// keyboard when focused (arrows, [ and ], + and -, PageUp/PageDown = 2, Home/End =
// limits, 0 = original key), the plates, double-click = original key.
//
// Commit policy. Every key change makes the audio engine build and prime a new
// pitch-shifter instance and crossfade to it (DECISIONS D-31), so the dial does
// not fire onChange for every detent it passes while being spun: the pointer
// moves at once, and onChange fires once the hand stops - on release for a drag,
// 160 ms after the last wheel/key/plate step otherwise. The face shows the
// pending value meanwhile; the label (from the parent) catches up on commit.

import { useEffect, useRef, useState } from "preact/hooks";
import { PlateButton } from "./PlateButton";
import {
  angleDelta,
  angleFor,
  clampSemitones,
  DEFAULT_MAX,
  DEFAULT_MIN,
  detentDegrees,
  formatSemitones,
  pointerAngle,
  prettyKeyLabel,
  stepsForRotation,
  wheelSteps,
} from "./keydial-math";


/** Drag gearing: the hand turns this much further than the pointer moves (one semitone per ~22 degrees). */
const DRAG_GEAR = 0.5;
export interface KeyDialProps {
  semitones: number;
  min?: number;
  max?: number;
  /** Shown under the dial, e.g. "A major -> B major (+2)". */
  label: string;
  onChange(n: number): void;
  disabled?: boolean;
  class?: string;
}

const COMMIT_DELAY_MS = 160;
/** How long a committed value may wait for the parent to echo it back. */
const ECHO_TIMEOUT_MS = 1500;

interface Drag {
  pointerId: number;
  cx: number;
  cy: number;
  radius: number;
  lastAngle: number;
  rotation: number;
  startY: number;
  base: number;
  near: boolean;
}

export function KeyDial({
  semitones,
  min = DEFAULT_MIN,
  max = DEFAULT_MAX,
  label,
  onChange,
  disabled = false,
  class: className,
}: KeyDialProps) {
  const committed = clampSemitones(semitones, min, max);
  const [pending, setPending] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);
  const [bump, setBump] = useState(0);
  const shown = pending ?? committed;

  const dialRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<Drag | null>(null);
  const commitTimer = useRef(0);
  const echoTimer = useRef(0);
  const wheelRemainder = useRef(0);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const shownRef = useRef(shown);
  shownRef.current = shown;
  const committedRef = useRef(committed);
  committedRef.current = committed;

  // The parent has caught up (or moved the key itself): drop the pending value.
  useEffect(() => {
    if (pending !== null && pending === committed && !dragRef.current) {
      setPending(null);
      clearTimeout(echoTimer.current);
    }
  }, [committed, pending]);

  useEffect(
    () => () => {
      clearTimeout(commitTimer.current);
      clearTimeout(echoTimer.current);
    },
    [],
  );

  const commit = (n: number) => {
    clearTimeout(commitTimer.current);
    commitTimer.current = 0;
    if (n !== committedRef.current) onChangeRef.current(n);
    // If the parent never echoes the value back, stop showing it after a while.
    clearTimeout(echoTimer.current);
    // Only drop the value this commit was waiting for, never a newer one.
    echoTimer.current = window.setTimeout(() => setPending((p) => (p === n ? null : p)), ECHO_TIMEOUT_MS);
    if (n === committedRef.current) setPending(null);
  };

  /** Moves the pointer by `steps` detents; commits after a short pause. */
  const stepBy = (steps: number) => {
    if (disabled || steps === 0) return;
    const from = shownRef.current;
    const to = clampSemitones(from + steps, min, max);
    if (to === from) {
      setBump((b) => b + 1); // at the end stop: a small shake instead
      return;
    }
    setPending(to);
    shownRef.current = to;
    clearTimeout(echoTimer.current);
    clearTimeout(commitTimer.current);
    commitTimer.current = window.setTimeout(() => commit(to), COMMIT_DELAY_MS);
  };

  const setTo = (n: number) => stepBy(clampSemitones(n, min, max) - shownRef.current);

  const onKeyDown = (e: KeyboardEvent) => {
    // Leave Ctrl/Cmd/Alt combinations (Ctrl+0, Cmd+Backspace, Alt+Arrow, ...) to the
    // app and the system: only plain keys (Shift allowed, for "+") turn the dial.
    if (disabled || e.ctrlKey || e.metaKey || e.altKey) return;
    let steps = 0;
    switch (e.key) {
      case "ArrowUp":
      case "ArrowRight":
      case "]":
      case "+":
      case "=":
        steps = 1;
        break;
      case "ArrowDown":
      case "ArrowLeft":
      case "[":
      case "-":
        steps = -1;
        break;
      case "PageUp":
        steps = 2;
        break;
      case "PageDown":
        steps = -2;
        break;
      case "Home":
        steps = min - shownRef.current;
        break;
      case "End":
        steps = max - shownRef.current;
        break;
      case "0":
      case "Delete":
      case "Backspace":
        steps = -shownRef.current;
        break;
      default:
        return;
    }
    e.preventDefault();
    if (steps === 0) setBump((b) => b + 1);
    else stepBy(steps);
  };

  const onWheel = (e: WheelEvent) => {
    if (disabled) return;
    e.preventDefault();
    const r = wheelSteps(wheelRemainder.current, e.deltaY, e.deltaMode);
    wheelRemainder.current = r.remainder;
    if (r.steps !== 0) stepBy(r.steps);
  };

  const onPointerDown = (e: PointerEvent) => {
    if (disabled || e.button !== 0) return;
    const el = dialRef.current;
    if (!el) return;
    const box = el.getBoundingClientRect();
    const cx = box.left + box.width / 2;
    const cy = box.top + box.height / 2;
    const radius = box.width / 2;
    const near = Math.hypot(e.clientX - cx, e.clientY - cy) < radius * 0.3;
    dragRef.current = {
      pointerId: e.pointerId,
      cx,
      cy,
      radius,
      lastAngle: pointerAngle(cx, cy, e.clientX, e.clientY),
      rotation: 0,
      startY: e.clientY,
      base: shownRef.current,
      near,
    };
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    clearTimeout(commitTimer.current);
    clearTimeout(echoTimer.current);
    setDragging(true);
    e.preventDefault();
  };

  const onPointerMove = (e: PointerEvent) => {
    const d = dragRef.current;
    if (!d || e.pointerId !== d.pointerId) return;
    let steps: number;
    if (d.near) {
      // Grabbed near the centre: angles are meaningless there, use vertical travel.
      steps = Math.round((d.startY - e.clientY) / Math.max(16, d.radius * 0.5));
    } else {
      const a = pointerAngle(d.cx, d.cy, e.clientX, e.clientY);
      d.rotation += angleDelta(d.lastAngle, a);
      d.lastAngle = a;
      // Half a turn of the hand per detent's worth of key: one semitone per about 22
      // degrees, so the key can be turned on camera without overshooting.
      steps = stepsForRotation(d.rotation * DRAG_GEAR, min, max);
    }
    const to = clampSemitones(d.base + steps, min, max);
    if (to !== shownRef.current) {
      shownRef.current = to;
      setPending(to);
    }
  };

  const endDrag = (e: PointerEvent) => {
    const d = dragRef.current;
    if (!d || e.pointerId !== d.pointerId) return;
    dragRef.current = null;
    setDragging(false);
    commit(shownRef.current);
  };

  const angle = angleFor(shown, min, max);
  const zeroAngle = angleFor(0, min, max);
  const detent = detentDegrees(min, max);
  const pretty = prettyKeyLabel(label);

  // Ticks and the lit arc from the original key to the current one (viewBox -50..50).
  const ticks = [];
  for (let n = min; n <= max; n++) {
    const a = (angleFor(n, min, max) * Math.PI) / 180;
    const major = n === 0 || n % 6 === 0;
    const r0 = major ? 33.5 : 36.5;
    const r1 = 40.5;
    const lit = n === shown;
    const between = (n > 0 && n <= shown) || (n < 0 && n >= shown);
    ticks.push(
      <line
        key={n}
        x1={(r0 * Math.sin(a)).toFixed(2)}
        y1={(-r0 * Math.cos(a)).toFixed(2)}
        x2={(r1 * Math.sin(a)).toFixed(2)}
        y2={(-r1 * Math.cos(a)).toFixed(2)}
        class={`jk-dial-tick${major ? " jk-dial-tick--major" : ""}${lit ? " jk-dial-tick--lit" : between ? " jk-dial-tick--between" : ""}`}
      />,
    );
  }
  const arcR = 44;
  const a0 = (Math.min(zeroAngle, angle) * Math.PI) / 180;
  const a1 = (Math.max(zeroAngle, angle) * Math.PI) / 180;
  const arc =
    shown === 0
      ? ""
      : `M${(arcR * Math.sin(a0)).toFixed(2)} ${(-arcR * Math.cos(a0)).toFixed(2)} A${arcR} ${arcR} 0 ${a1 - a0 > Math.PI ? 1 : 0} 1 ${(arcR * Math.sin(a1)).toFixed(2)} ${(-arcR * Math.cos(a1)).toFixed(2)}`;

  const cls = [
    "jk-keydial",
    disabled ? "jk-keydial--disabled" : "",
    dragging ? "jk-keydial--dragging" : "",
    pending !== null && pending !== committed ? "jk-keydial--pending" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div class={cls} style={`--jk-detent: ${detent}deg`}>
      <PlateButton
        class="jk-keydial-down"
        size="sm"
        label={<span class="jk-sign jk-sign--minus" aria-hidden="true" />}
        ariaLabel="Key down one semitone"
        title="Key down one semitone ( [ )"
        disabled={disabled || shown <= min}
        onClick={() => stepBy(-1)}
      />
      <div
        ref={dialRef}
        class={`jk-dial${bump % 2 === 1 ? " jk-dial--bump-a" : bump > 0 ? " jk-dial--bump-b" : ""}`}
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label="Key"
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={shown}
        aria-valuetext={pretty || formatSemitones(shown)}
        aria-disabled={disabled ? "true" : undefined}
        title={disabled ? undefined : "Turn, scroll or use [ ] to change the key. Double-click or 0: original key."}
        onKeyDown={onKeyDown}
        onWheel={onWheel}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDblClick={() => setTo(0)}
      >
        <div class="jk-dial-chrome" />
        <div class="jk-dial-knurl" style={`transform: rotate(${angle}deg)`} />
        {/* Thin silver chrome ring inside the gold knurling (shown with the frame's silver trim). */}
        <div class="jk-dial-trim" />
        <div class="jk-dial-face">
          <svg class="jk-dial-ticks" viewBox="-50 -50 100 100" aria-hidden="true">
            {arc && <path class="jk-dial-arc" d={arc} />}
            {ticks}
          </svg>
          <div class="jk-dial-pointer" style={`transform: rotate(${angle}deg)`}>
            <span class="jk-dial-notch" />
          </div>
          <div class="jk-dial-readout" aria-hidden="true">
            <span class="jk-dial-value" key={shown}>
              {formatSemitones(shown)}
            </span>
            <span class="jk-dial-caption">KEY</span>
          </div>
          <div class="jk-dial-glass" />
        </div>
      </div>
      <PlateButton
        class="jk-keydial-up"
        size="sm"
        label={<span class="jk-sign jk-sign--plus" aria-hidden="true" />}
        ariaLabel="Key up one semitone"
        title="Key up one semitone ( ] )"
        disabled={disabled || shown >= max}
        onClick={() => stepBy(1)}
      />
      <div class="jk-keydial-label" aria-hidden="true" title={pretty}>
        {pretty}
      </div>
    </div>
  );
}

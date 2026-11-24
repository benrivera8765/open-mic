// SPDX-License-Identifier: GPL-3.0-or-later
// The animation frame rate (owner, 2026-11-23: "allow it to go to 240 fps on my computer
// for the effects"; settings ui.look.fx_fps, Windows only, D-274).
//
// "standard" (0 here) keeps every animation at the rate it was designed for: the frame
// effects at 8-30 fps and the lyrics and notes lane at 60 fps, which is what keeps a fanless
// MacBook Air cool. A number lets every one of them run up to that rate, as fast as the
// screen refreshes: bubbles, lava, fire, lightning, fish, the drifting light, the
// celebrations, the notes lane and the lyrics.

import { useEffect, useState } from "preact/hooks";

let cap = 0;
const subs = new Set<() => void>();

/** Sets the frame rate cap (0 = standard; otherwise 30-240 fps). Every running animation restarts at it. */
export function setFxFps(fps: number): void {
  const v = fps > 0 ? Math.min(240, Math.max(30, Math.round(fps))) : 0;
  if (v === cap) return;
  cap = v;
  for (const f of subs) f();
}

/** The cap in force: 0 = standard. */
export function fxFpsCap(): number {
  return cap;
}

/** The rate an animation designed for `base` fps runs at. */
export function effectFps(base: number): number {
  return cap > 0 ? cap : base;
}

/** `effectFps(base)`, re-rendering the caller when the setting changes (put it in the loop effect's deps). */
export function useEffectFps(base: number): number {
  const [, bump] = useState(0);
  useEffect(() => {
    const f = () => bump((n) => n + 1);
    subs.add(f);
    return () => {
      subs.delete(f);
    };
  }, []);
  return effectFps(base);
}

/**
 * Calls `frame(now)` at most `fps` times a second until the returned stop function is
 * called. Below 50 fps it sleeps between frames (a timer, then requestAnimationFrame to
 * line the draw up with the display), so a slow effect never wakes the page on every
 * display frame. From 50 fps it follows the display and skips frames to keep to the rate,
 * advancing its schedule by whole periods (so 240 on a 144 Hz screen is 144, and 120 on a
 * 240 Hz screen is 120, not 80).
 */
export function fxLoop(fps: number, frame: (now: number) => void): () => void {
  const period = 1000 / Math.max(1, fps);
  const follow = fps >= 50;
  let stopped = false;
  let raf = 0;
  let timer = 0;
  let last = Number.NEGATIVE_INFINITY;
  const tick = (now: number) => {
    raf = 0;
    if (stopped) return;
    if (follow) {
      if (now - last < period - 1.5) {
        raf = requestAnimationFrame(tick);
        return;
      }
      last = now - last > period * 4 ? now : last + period * Math.max(1, Math.floor((now - last) / period));
    }
    frame(now);
    if (stopped) return;
    if (follow) raf = requestAnimationFrame(tick);
    else timer = window.setTimeout(wake, period);
  };
  const wake = () => {
    timer = 0;
    if (!stopped) raf = requestAnimationFrame(tick);
  };
  if (follow) raf = requestAnimationFrame(tick);
  else timer = window.setTimeout(wake, period);
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    if (raf) cancelAnimationFrame(raf);
  };
}

// SPDX-License-Identifier: GPL-3.0-or-later
// The lyrics ticker's frame loop (WORKING ORDER 4.4 / 4.7, the fanless MacBook Air):
//   - at most 60 frames per second, even on 120 Hz displays;
//   - stops completely when nothing moves (`draw` returns false) and while the window is
//     hidden: either the page reports itself hidden, or the native window is minimised.
//     The second signal is needed because WebView2 keeps `document.visibilityState`
//     "visible" for a minimised window (D-97); it is the same Tauri signal the jukebox
//     skin uses for its light drift and bubbles (`Api.onWindowMinimised`: the window's
//     resize event plus `isMinimized()`).

import { fxFpsCap } from "../lib/fxrate";

export const FRAME_MS = 1000 / 60;

/** The frame period now: 60 fps, or the frame rate setting when raised (D-274). */
function framePeriod(): number {
  const cap = fxFpsCap();
  return cap > 60 ? 1000 / cap : FRAME_MS;
}

/** `Api.onWindowMinimised`: calls back with true while minimised, false when restored. */
export type MinimisedSource = (cb: (minimised: boolean) => void) => () => void;

export interface FrameLoopEnv {
  requestFrame: (cb: (now: number) => void) => number;
  cancelFrame: (id: number) => void;
  /** The page: `hidden` plus `visibilitychange` events (null outside a browser). */
  doc: Pick<Document, "hidden" | "addEventListener" | "removeEventListener"> | null;
  /** The native window's minimised state, where the host reports it (Tauri). */
  onWindowMinimised?: MinimisedSource;
}

export class FrameLoop {
  /** Draws one frame at `now`; returns true while something still moves. */
  draw: ((now: number) => boolean) | null = null;
  private raf = 0;
  private last = 0;
  private minimised = false;
  private offs: (() => void)[] = [];

  constructor(private readonly env: FrameLoopEnv) {}

  /** Starts following page visibility and the window's minimised state. */
  attach(): void {
    this.detach();
    const doc = this.env.doc;
    if (doc) {
      doc.addEventListener("visibilitychange", this.onVisibility);
      this.offs.push(() => doc.removeEventListener("visibilitychange", this.onVisibility));
    }
    const off = this.env.onWindowMinimised?.((m) => {
      this.minimised = m;
      this.onVisibility();
    });
    if (off) this.offs.push(off);
  }

  detach(): void {
    this.stop();
    for (const off of this.offs.splice(0)) off();
    this.minimised = false;
  }

  /** True while nothing may be drawn: page hidden or window minimised. */
  get hidden(): boolean {
    return !!this.env.doc?.hidden || this.minimised;
  }

  /** True while a frame is scheduled. */
  get running(): boolean {
    return this.raf !== 0;
  }

  /** Schedules a frame unless one is pending, nothing can be drawn, or the window is hidden. */
  kick = (): void => {
    if (!this.raf && this.draw && !this.hidden) this.raf = this.env.requestFrame(this.frame);
  };

  stop(): void {
    if (this.raf) this.env.cancelFrame(this.raf);
    this.raf = 0;
  }

  private onVisibility = (): void => {
    if (this.hidden) this.stop();
    else this.kick();
  };

  private frame = (now: number): void => {
    this.raf = 0;
    const draw = this.draw;
    if (!draw || this.hidden) return;
    const period = framePeriod();
    // A little slack for timer jitter.
    if (now - this.last < period - (period > 8 ? 2 : 1)) {
      this.raf = this.env.requestFrame(this.frame);
      return;
    }
    // Advance the 60 fps schedule by whole frame periods instead of jumping to `now`:
    // with `last = now` a 75 Hz screen drew every second refresh (37.5 fps) and a
    // 144 Hz one 48 fps; this keeps 60 fps there too and never more on average.
    this.last += period * Math.max(1, Math.floor((now - this.last) / period));
    if (draw(now) && !this.raf) this.raf = this.env.requestFrame(this.frame);
  };
}

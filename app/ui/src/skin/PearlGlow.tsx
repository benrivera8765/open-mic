// SPDX-License-Identifier: GPL-3.0-or-later
// The warm light inside the pearlescent cabinet (owner's photo 7, 2026-11-22): the
// lights behind the moulded panel slowly turn through warm jukebox colours.
//
// How it stays cheap: a TINY canvas (96 x 54) stretched over the frame by the
// compositor (bilinear upscaling makes a perfectly smooth glow out of it), redrawn
// 8 times a second at most. Each redraw is a few gradient fills on 5 184 pixels; the
// pearl sheen and fluting on top are a static image (wood.ts pearlTexture), so nothing
// big ever repaints. Stops when the page is hidden, the window minimised, or motion
// is reduced (one still frame then).

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";

const W = 96;
const H = 54;
const FPS = 8;
/** One full turn through the warm colours. */
export const PEARL_CYCLE_MS = 36_000;

/** Warm jukebox colours, cyclic: rose, red, orange, amber, cream, coral pink. */
export const WARM_PALETTE: readonly string[] = ["#ff4f8b", "#ff3b3b", "#ff8a1f", "#ffc23d", "#ffe9bf", "#ff8fae"];

/** The forge cabinet's magma under the crust (Molten): red heat, orange, gold, white heat. */
export const FORGE_PALETTE: readonly string[] = ["#ff2a06", "#ff5a0a", "#ff8a1a", "#ffb02e", "#ff4a08", "#c81404"];

function rgbOf(hex: string): [number, number, number] {
  const v = parseInt(hex.slice(1), 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/** Colour at phase p (any real; one palette turn per 1.0). */
export function warmAt(palette: readonly [number, number, number][], p: number): string {
  const n = palette.length;
  const x = (((p % 1) + 1) % 1) * n;
  const i = Math.floor(x);
  const f = x - i;
  const a = palette[i % n];
  const b = palette[(i + 1) % n];
  // Smoothstep between stops, so no colour band has a visible edge as it passes.
  const s = f * f * (3 - 2 * f);
  return `rgb(${Math.round(a[0] + (b[0] - a[0]) * s)},${Math.round(a[1] + (b[1] - a[1]) * s)},${Math.round(a[2] + (b[2] - a[2]) * s)})`;
}

export interface PearlGlowProps {
  /** False stops the loop (hidden page, minimised window). */
  running: boolean;
  /** False: one still frame (reduced motion). */
  animate: boolean;
  /** Colours to turn through (WARM_PALETTE by default). Pass a constant: a new array restarts the loop. */
  palette?: readonly string[];
  /** One full turn through the palette, ms. */
  cycleMs?: number;
  class?: string;
}

export function PearlGlow({ running, animate, palette = WARM_PALETTE, cycleMs = PEARL_CYCLE_MS, class: className }: PearlGlowProps) {
  const fps = useEffectFps(FPS);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  /** Phase carried across pauses, so the colours resume where they stopped. */
  const phaseRef = useRef(0.08);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const pal = palette.map(rgbOf);
    const draw = (p: number) => {
      // Colour runs across the panel: five stops a sixth of a turn apart, all drifting.
      const gx = ctx.createLinearGradient(0, 0, W, 0);
      for (let i = 0; i <= 4; i++) gx.addColorStop(i / 4, warmAt(pal, p + i * 0.165));
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = gx;
      ctx.fillRect(0, 0, W, H);
      // Two pools of light wandering slowly along the panel.
      ctx.globalCompositeOperation = "lighter";
      for (let k = 0; k < 2; k++) {
        const ang = 6.2832 * (p * (k ? -1 : 1) + k * 0.5);
        const cx = W * (0.5 + 0.38 * Math.sin(ang));
        const cy = H * (0.62 + 0.12 * Math.cos(ang * 1.3));
        const r = H * 0.95;
        const rg = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
        rg.addColorStop(0, "rgba(255,214,150,0.3)");
        rg.addColorStop(1, "rgba(255,214,150,0)");
        ctx.fillStyle = rg;
        ctx.fillRect(0, 0, W, H);
      }
      // Lit from inside: brighter across the middle, falling off at the top and bottom.
      ctx.globalCompositeOperation = "source-over";
      const gy = ctx.createLinearGradient(0, 0, 0, H);
      gy.addColorStop(0, "rgba(60,10,20,0.28)");
      gy.addColorStop(0.32, "rgba(60,10,20,0)");
      gy.addColorStop(0.74, "rgba(60,10,20,0)");
      gy.addColorStop(1, "rgba(50,8,16,0.34)");
      ctx.fillStyle = gy;
      ctx.fillRect(0, 0, W, H);
    };
    draw(phaseRef.current);
    if (!animate || !running) return;

    let last = performance.now();
    return fxLoop(fps, (now: number) => {
      phaseRef.current += (now - last) / cycleMs;
      last = now;
      draw(phaseRef.current);
    });
  }, [running, animate, palette, cycleMs, fps]);

  return <canvas ref={canvasRef} class={className} width={W} height={H} aria-hidden="true" />;
}

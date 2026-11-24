// SPDX-License-Identifier: GPL-3.0-or-later
// The bubble layer: ONE canvas over the frame, drawn at most 30 times a second.
//
// Rules (WORK_ORDER 4.7, docs/DESIGN.md "Bubbles"):
//  - at most 60 bubbles (bubbles-engine.ts enforces it);
//  - capped at 30 fps: a timer wakes the loop about every 33 ms and the drawing
//    happens inside requestAnimationFrame, so it lines up with the display and
//    never runs while the page is hidden (browsers do not fire rAF then);
//  - stops completely when `running` is false (hidden or minimised window,
//    paused, reduce-motion) - no timer, no rAF;
//  - only clears what it drew last frame (dirty rectangles), so a 4K canvas costs
//    no more per frame than a small one;
//  - no allocation in the loop: sprites are cached per radius, dirty rectangles
//    live in a preallocated typed array.

import { useEffect, useRef } from "preact/hooks";
import { useEffectFps } from "../lib/fxrate";
import { BubbleField, MAX_BUBBLES, type BubbleSprite } from "./bubbles-engine";
import { bubbleTrack, type FrameGeometry } from "./geometry";

/** Live counters, read by the performance measurement (tools/measure-skin.mjs). */
export const bubbleStats = {
  frames: 0,
  drawMsTotal: 0,
  drawMsMax: 0,
  alive: 0,
  running: false,
};

/**
 * 30 fps as designed. A frame is drawn only if at least the period less 1 ms has passed
 * since the last one, by the rAF timestamps (which are the display frame times): the
 * slack lets a 60, 120 or 240 Hz display land every 2nd, 4th or 8th frame (33.3 ms)
 * without ever going faster than ~31 fps. The frame rate setting (D-274) raises it.
 */
const FRAME_MS = 1000 / 30;
/** Backing-store cap: never more device pixels than a 4K screen. */
const MAX_CANVAS_PIXELS = 3840 * 2160;

export interface BubblesProps {
  geometry: FrameGeometry;
  /** False stops the loop entirely. */
  running: boolean;
  maxBubbles?: number;
  /**
   * Two small bubble tubes either side of the centre line (the pilaster frame style)
   * instead of the one inner tube: their centre-line offset and width, CSS px.
   */
  channels?: { offset: number; width: number } | null;
  class?: string;
}

/** Renders one bubble sprite: clear middle, bright rim, a tiny highlight. */
function makeSprite(radius: number): HTMLCanvasElement {
  const size = Math.ceil(radius * 2 + 4);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  // Faint dark outline: keeps the bubble readable against the brightest light.
  ctx.beginPath();
  ctx.arc(m, m, radius + 0.6, 0, Math.PI * 2);
  ctx.fillStyle = "rgba(40, 0, 24, 0.28)";
  ctx.fill();
  const g = ctx.createRadialGradient(m, m, 0, m, m, radius);
  g.addColorStop(0, "rgba(255,255,255,0.02)");
  g.addColorStop(0.6, "rgba(255,255,255,0.07)");
  g.addColorStop(0.8, "rgba(255,255,255,0.42)");
  g.addColorStop(0.93, "rgba(255,255,255,0.95)");
  g.addColorStop(1, "rgba(255,255,255,0.25)");
  ctx.beginPath();
  ctx.arc(m, m, radius, 0, Math.PI * 2);
  ctx.fillStyle = g;
  ctx.fill();
  // Specular dot, upper left.
  const hr = Math.max(0.6, radius * 0.24);
  const hx = m - radius * 0.36;
  const hy = m - radius * 0.38;
  const h = ctx.createRadialGradient(hx, hy, 0, hx, hy, hr);
  h.addColorStop(0, "rgba(255,255,255,0.95)");
  h.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = h;
  ctx.fillRect(hx - hr, hy - hr, hr * 2, hr * 2);
  return c;
}

export function Bubbles({ geometry, running, maxBubbles = MAX_BUBBLES, channels = null, class: className }: BubblesProps) {
  const fps = useEffectFps(1000 / FRAME_MS);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fieldRef = useRef<BubbleField | null>(null);
  if (fieldRef.current === null) fieldRef.current = new BubbleField({ maxBubbles });
  /** Device pixels per CSS pixel of the backing store. */
  const pxRef = useRef(1);
  /** How many tracks the field ran on last time (0 = none yet). */
  const pathsRef = useRef(0);

  // Geometry -> paths and canvas size.
  useEffect(() => {
    const canvas = canvasRef.current;
    const field = fieldRef.current;
    if (!canvas || !field) return;
    const g = geometry;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    let k = Math.min(dpr, 2);
    if (g.width * g.height * k * k > MAX_CANVAS_PIXELS) {
      k = Math.sqrt(MAX_CANVAS_PIXELS / (g.width * g.height));
    }
    pxRef.current = k;
    canvas.width = Math.max(1, Math.round(g.width * k));
    canvas.height = Math.max(1, Math.round(g.height * k));
    const scale = Math.max(0.5, g.height / 900);
    // A different set of tubes (neon <-> pilaster): start them full, from scratch.
    const nPaths = channels ? 4 : 2;
    if (pathsRef.current !== 0 && pathsRef.current !== nPaths) field.clear();
    pathsRef.current = nPaths;
    if (channels) {
      const o = channels.offset;
      field.setPaths(
        [bubbleTrack(g, "left", g.baseTop, -o), bubbleTrack(g, "right", g.baseTop, -o), bubbleTrack(g, "left", g.baseTop, o), bubbleTrack(g, "right", g.baseTop, o)],
        channels.width / 2,
        scale,
      );
    } else {
      field.setPaths([bubbleTrack(g, "left"), bubbleTrack(g, "right")], g.bubbleTube / 2, scale);
    }
    // First appearance: start with tubes already full (one crossing takes ~20 s).
    if (field.count === 0) field.prewarm(30);
  }, [geometry, channels?.offset, channels?.width]);

  // The loop.
  useEffect(() => {
    const canvas = canvasRef.current;
    const field = fieldRef.current;
    bubbleStats.running = running;
    if (!canvas || !field || !running) {
      return;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const sprites = new Map<number, HTMLCanvasElement>();
    // Dirty rectangles of the previous frame: x, y, w, h per bubble (device px).
    const dirty = new Float32Array(MAX_BUBBLES * 4 + 4);
    let dirtyCount = 0;
    let last = performance.now();
    let timer = 0;
    let raf = 0;
    let stopped = false;
    // The frame period: 30 fps as designed, or the frame rate setting (D-274); from 50 fps on
    // every display frame is a candidate (no timer between frames).
    const frameMs = 1000 / fps;
    const minGap = frameMs - 1;
    const follow = fps >= 50;

    // Whole-canvas clear once at start (size may have changed).
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    const drawOne = (b: BubbleSprite) => {
      const k = pxRef.current;
      // Quantise the radius to half device pixels so the sprite cache stays small.
      const key = Math.max(1, Math.round(b.r * k * 2));
      let sprite = sprites.get(key);
      if (!sprite) {
        sprite = makeSprite(key / 2);
        sprites.set(key, sprite);
      }
      const half = sprite.width / 2;
      const x = Math.round(b.x * k - half);
      const y = Math.round(b.y * k - half);
      ctx.globalAlpha = b.alpha;
      ctx.drawImage(sprite, x, y);
      if (dirtyCount < MAX_BUBBLES) {
        const i = dirtyCount * 4;
        dirty[i] = x - 1;
        dirty[i + 1] = y - 1;
        dirty[i + 2] = sprite.width + 2;
        dirty[i + 3] = sprite.height + 2;
        dirtyCount++;
      }
    };

    const frame = (now: number) => {
      raf = 0;
      if (stopped) return;
      if (now - last < minGap) {
        // The timer woke us a display frame early: wait for the next one.
        raf = requestAnimationFrame(frame);
        return;
      }
      const t0 = performance.now();
      const dt = Math.min(0.1, (now - last) / 1000);
      // Advance by whole periods (as lib/fxrate fxLoop does): with last = now a 144 Hz
      // screen drew 60 fps bubbles at 48 fps and 120 fps ones at 72.
      last = now - last > frameMs * 4 ? now : last + frameMs * Math.max(1, Math.floor((now - last) / frameMs));
      field.step(dt);
      for (let i = 0; i < dirtyCount; i++) {
        const j = i * 4;
        ctx.clearRect(dirty[j], dirty[j + 1], dirty[j + 2], dirty[j + 3]);
      }
      dirtyCount = 0;
      field.forEach(drawOne);
      ctx.globalAlpha = 1;
      const ms = performance.now() - t0;
      bubbleStats.frames++;
      bubbleStats.drawMsTotal += ms;
      if (ms > bubbleStats.drawMsMax) bubbleStats.drawMsMax = ms;
      bubbleStats.alive = dirtyCount;
      schedule();
    };

    // Created once, so scheduling the next frame allocates no closure.
    const wake = () => {
      timer = 0;
      if (!stopped) raf = requestAnimationFrame(frame);
    };

    const schedule = () => {
      if (stopped) return;
      // Wake about half a frame interval early: timers are coarse in some engines
      // (WebKit fired 10-30 ms late in our tests). rAF then lines the draw up with the
      // display, and the MIN_GAP_MS gate in frame() keeps it at <= 30 fps, so the
      // worst case is one extra, empty rAF callback per drawn frame.
      if (follow) {
        raf = requestAnimationFrame(frame);
        return;
      }
      const wait = Math.max(0, frameMs - (performance.now() - last) - 17);
      timer = window.setTimeout(wake, wait);
    };

    raf = requestAnimationFrame((now) => {
      last = now - frameMs; // draw on the first frame
      frame(now);
    });

    return () => {
      stopped = true;
      bubbleStats.running = false;
      if (timer) clearTimeout(timer);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [running, geometry, channels?.offset, channels?.width, fps]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

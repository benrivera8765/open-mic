// SPDX-License-Identifier: GPL-3.0-or-later
// The lava-lamp frame style (owner, 2026-11-23): wax drifting UP through the frame's
// tube, over the deep liquid that frame-art.ts lavaTubeSvg shades.
//
// Like a real lamp: a hot pool of wax sits at the foot of each column over a
// flickering heater glow. Blobs bud off the pool (they swell while still attached by a
// neck), let go, and rise along the tube (up the column, round the shoulder, over the
// arch), big ones faster than small ones, swaying gently and changing shape all the
// time (each blob is 3-4 soft lobes that drift and breathe; it stretches into a
// teardrop while it rises fast). A blob that catches up with another merges into it.
// At the crest they slow, cool and melt into the wax gathered there, and later a new
// blob buds at the bottom. Every blob lights the liquid round it (a glow that moves with
// it), hotter and yellower low down, deeper orange-red higher up, so the light in the
// lamp changes all the time.
//
// Cost: one canvas, at most 24 frames a second, 8 blobs a side, sprites cached per
// size, no allocation per frame. Stops when `running` is false (hidden, minimised);
// under reduced motion one still frame is drawn.

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";
import { bubbleTrack, type FrameGeometry, type PathPoint, type TubePath } from "./geometry";

const FPS = 24;
const PER_SIDE = 10;
const MAX_CANVAS_PIXELS = 3840 * 2160;

const BUDDING = 0;
const RISING = 1;
const MELTING = 2;
const WAITING = 3;

interface Lobe {
  /** Offset along the tube and across it, in blob radii. */
  along: number;
  across: number;
  /** Radius as a share of the blob's. */
  size: number;
  phase: number;
  speed: number;
}

interface Blob {
  path: number;
  state: number;
  s: number;
  v: number;
  r: number;
  rMax: number;
  /** Seconds left in the current state (budding, melting, waiting). */
  timer: number;
  swayPhase: number;
  swaySpeed: number;
  lobes: Lobe[];
}

export interface LavaLampProps {
  geometry: FrameGeometry;
  /** False: draw one still frame and stop. */
  running: boolean;
  class?: string;
}

/**
 * Wax body: an even colour with a soft rim, so overlapping lobes fuse into ONE smooth
 * shape (a bright centre per lobe made blobs look like bunches of grapes). `hot` is the
 * golden orange of wax near the heater, otherwise the deeper orange-red of cooled wax.
 */
function makeBody(r: number, hot: boolean): HTMLCanvasElement {
  const size = Math.ceil(r * 2 + 4);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, 0, m, m, r);
  if (hot) {
    g.addColorStop(0, "rgba(255,150,40,1)");
    g.addColorStop(0.7, "rgba(255,132,34,1)");
    g.addColorStop(0.88, "rgba(240,86,30,0.92)");
  } else {
    g.addColorStop(0, "rgba(240,88,32,1)");
    g.addColorStop(0.7, "rgba(228,74,32,1)");
    g.addColorStop(0.88, "rgba(186,40,44,0.92)");
  }
  g.addColorStop(1, "rgba(150,20,40,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(m, m, r, 0, Math.PI * 2);
  ctx.fill();
  return c;
}

/** The solid inside of a lobe (no rim), drawn over all lobes so only the outer outline shows. */
function makeFill(r: number, hot: boolean): HTMLCanvasElement {
  const size = Math.ceil(r * 2 + 4);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, 0, m, m, r * 0.86);
  const solid = hot ? "rgba(255,138,36," : "rgba(232,78,32,";
  g.addColorStop(0, solid + "1)");
  g.addColorStop(0.82, solid + "1)");
  g.addColorStop(1, solid + "0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(m, m, r * 0.86, 0, Math.PI * 2);
  ctx.fill();
  return c;
}

/** The inside of a blob glowing: one warm core per blob, drawn over its lobes. */
function makeCore(r: number): HTMLCanvasElement {
  const size = Math.ceil(r * 2 + 4);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, 0, m, m, r);
  g.addColorStop(0, "rgba(255,244,190,0.95)");
  g.addColorStop(0.45, "rgba(255,206,90,0.55)");
  g.addColorStop(1, "rgba(255,170,60,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  return c;
}

/** The light a blob throws into the liquid round it (drawn additively). */
function makeGlow(r: number): HTMLCanvasElement {
  const size = Math.ceil(r * 4.4 + 4);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, r * 0.3, m, m, r * 2.2);
  g.addColorStop(0, "rgba(255,140,60,0.5)");
  g.addColorStop(0.5, "rgba(255,80,60,0.16)");
  g.addColorStop(1, "rgba(255,60,80,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  return c;
}

function makeLobes(rnd: () => number): Lobe[] {
  const side = rnd() < 0.5 ? -1 : 1;
  const lobes: Lobe[] = [
    { along: 0, across: 0, size: 1, phase: rnd() * 6.28, speed: 0.4 + rnd() * 0.5 },
    { along: 0.3, across: 0.08 * side, size: 0.86 + rnd() * 0.1, phase: rnd() * 6.28, speed: 0.5 + rnd() * 0.6 },
    { along: -0.32, across: -0.06 * side, size: 0.78 + rnd() * 0.1, phase: rnd() * 6.28, speed: 0.4 + rnd() * 0.7 },
  ];
  if (rnd() < 0.55) lobes.push({ along: 0.05, across: 0.22 * -side, size: 0.74 + rnd() * 0.1, phase: rnd() * 6.28, speed: 0.6 + rnd() * 0.6 });
  return lobes;
}

/** Blob sizes vary a lot: droplets, medium blobs and a few big ones. */
function pickSize(T: number, rnd: () => number): number {
  const u = rnd();
  if (u < 0.25) return T * (0.14 + rnd() * 0.06);
  if (u < 0.75) return T * (0.25 + rnd() * 0.1);
  return T * (0.37 + rnd() * 0.08);
}

export function LavaLamp({ geometry, running, class: className }: LavaLampProps) {
  const fps = useEffectFps(FPS);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const blobsRef = useRef<Blob[]>([]);
  const pathsRef = useRef<TubePath[]>([]);
  const pxRef = useRef(1);
  /** Wax gathered at the crest, 0..1, per side. */
  const topRef = useRef([0.5, 0.5]);

  // Geometry -> tracks, canvas size and (first time) the blobs.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const g = geometry;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    let k = Math.min(dpr, 2);
    if (g.width * g.height * k * k > MAX_CANVAS_PIXELS) k = Math.sqrt(MAX_CANVAS_PIXELS / (g.width * g.height));
    pxRef.current = k;
    canvas.width = Math.max(1, Math.round(g.width * k));
    canvas.height = Math.max(1, Math.round(g.height * k));
    const old = pathsRef.current;
    const paths = [bubbleTrack(g, "left"), bubbleTrack(g, "right")];
    pathsRef.current = paths;
    const T = g.tube;
    if (blobsRef.current.length === 0) {
      // Start with a lamp that has been on for a while: blobs spread up the tube.
      const blobs: Blob[] = [];
      for (let p = 0; p < 2; p++) {
        const L = paths[p].length;
        for (let i = 0; i < PER_SIDE; i++) {
          const rMax = pickSize(T, Math.random);
          const first = i === 0;
          blobs.push({
            path: p,
            state: first ? BUDDING : RISING,
            s: first ? T * 0.75 : T * 1.4 + ((i - 1 + Math.random() * 0.8) / (PER_SIDE - 1)) * (L - T * 2.2),
            v: 0,
            r: first ? rMax * 0.3 : rMax,
            rMax,
            timer: first ? 2.5 : 0,
            swayPhase: Math.random() * 6.28,
            swaySpeed: 0.15 + Math.random() * 0.25,
            lobes: makeLobes(Math.random),
          });
        }
      }
      blobsRef.current = blobs;
    } else {
      // Resize: keep each blob at the same fraction of its track; sizes follow the tube.
      for (const b of blobsRef.current) {
        const before = old[b.path]?.length ?? 1;
        b.s = (b.s / Math.max(1, before)) * paths[b.path].length;
        b.rMax = Math.min(b.rMax, T * 0.44);
        b.r = Math.min(b.r, b.rMax);
      }
    }
  }, [geometry]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const g = geometry;
    const T = g.tube;
    const scale = Math.max(0.5, g.height / 900);
    const hot = new Map<number, HTMLCanvasElement>();
    const cool = new Map<number, HTMLCanvasElement>();
    const glows = new Map<number, HTMLCanvasElement>();
    const cores = new Map<number, HTMLCanvasElement>();
    const fillHot = new Map<number, HTMLCanvasElement>();
    const fillCool = new Map<number, HTMLCanvasElement>();
    const sprite = (cache: Map<number, HTMLCanvasElement>, r: number, make: (r: number) => HTMLCanvasElement) => {
      const key = Math.max(2, Math.round(r));
      let c = cache.get(key);
      if (!c) {
        c = make(key);
        cache.set(key, c);
      }
      return c;
    };
    const pt: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
    let time = 0;
    const poolS = T * 0.55;

    const speedOf = (b: Blob) => (9 + 22 * Math.min(1, Math.max(0, (b.rMax / T - 0.1) / 0.34))) * scale;

    const step = (dt: number) => {
      time += dt;
      const blobs = blobsRef.current;
      const top = topRef.current;
      top[0] = Math.max(0.2, top[0] - dt * 0.015);
      top[1] = Math.max(0.2, top[1] - dt * 0.015);
      for (const b of blobs) {
        const path = pathsRef.current[b.path];
        if (!path) continue;
        const L = path.length;
        if (b.state === WAITING) {
          b.timer -= dt;
          if (b.timer <= 0) {
            b.state = BUDDING;
            b.timer = 3 + Math.random() * 1.5;
            b.rMax = pickSize(T, Math.random);
            b.r = 0;
            b.s = T * 0.7;
            b.v = 0;
            b.lobes = makeLobes(Math.random);
          }
          continue;
        }
        if (b.state === BUDDING) {
          // Swelling on the pool, then letting go.
          b.timer -= dt;
          const total = 3.5;
          const p = Math.min(1, 1 - b.timer / total);
          b.r = b.rMax * Math.min(1, 0.15 + p * 0.95);
          b.s = T * 0.7 + b.rMax * 1.3 * p * p;
          if (b.timer <= 0) {
            b.state = RISING;
            b.v = speedOf(b) * 0.25;
          }
          continue;
        }
        const f = Math.min(1, b.s / L);
        if (b.state === RISING) {
          // Buoyant: speeds up after letting go, wavers a little, slows as it cools near the top.
          const want = speedOf(b) * (0.8 + 0.25 * Math.sin(time * 0.35 + b.swayPhase)) * (1 - 0.55 * f * f * f);
          b.v += (want - b.v) * Math.min(1, dt * 0.4);
          b.s += b.v * dt;
          if (b.s >= L - T * 0.45 - b.r * 0.4) {
            b.state = MELTING;
            b.timer = 2.6;
          }
        } else if (b.state === MELTING) {
          b.timer -= dt;
          b.v *= 1 - Math.min(1, dt * 1.2);
          b.s = Math.min(L - T * 0.15, b.s + b.v * dt);
          b.r = b.rMax * Math.max(0, b.timer / 2.6);
          if (b.timer <= 0) {
            top[b.path] = Math.min(1, top[b.path] + (b.rMax / T) * 0.5);
            b.state = WAITING;
            b.timer = 0.5 + Math.random() * 4;
          }
        }
      }
      // A blob that catches up with another merges into it (the bigger one keeps going).
      for (let i = 0; i < blobs.length; i++) {
        const a = blobs[i];
        if (a.state !== RISING) continue;
        for (let j = 0; j < blobs.length; j++) {
          const c = blobs[j];
          if (i === j || c.path !== a.path || c.state !== RISING) continue;
          if (Math.abs(a.s - c.s) < (a.r + c.r) * 0.38) {
            const big = a.rMax >= c.rMax ? a : c;
            const small = big === a ? c : a;
            big.rMax = Math.min(T * 0.45, Math.cbrt(big.rMax ** 3 + small.rMax ** 3));
            big.r = big.rMax;
            small.state = WAITING;
            small.timer = 1 + Math.random() * 3;
            small.r = 0;
          }
        }
      }
    };

    /**
     * One soft lobe of wax at arc length s, `lat` across the tube, radius r. Pass 0 draws
     * it with its soft rim, pass 1 only its solid inside (to hide the rims of the other
     * lobes of the same blob, so the blob reads as one smooth shape).
     */
    const lobeAt = (path: TubePath, s: number, lat: number, r: number, heat: number, pass = 0) => {
      const k = pxRef.current;
      path.at(Math.max(0, Math.min(path.length, s)), pt);
      const x = (pt.x + pt.nx * lat) * k;
      const y = (pt.y + pt.ny * lat) * k;
      const rr = r * k;
      const cs = pass === 0 ? sprite(cool, rr, (q) => makeBody(q, false)) : sprite(fillCool, rr, (q) => makeFill(q, false));
      ctx.globalAlpha = 1;
      ctx.drawImage(cs, x - cs.width / 2, y - cs.height / 2);
      if (heat > 0.02) {
        const hs = pass === 0 ? sprite(hot, rr, (q) => makeBody(q, true)) : sprite(fillHot, rr, (q) => makeFill(q, true));
        ctx.globalAlpha = heat;
        ctx.drawImage(hs, x - hs.width / 2, y - hs.height / 2);
      }
    };

    const glowAt = (path: TubePath, s: number, lat: number, r: number, alpha: number) => {
      const k = pxRef.current;
      path.at(Math.max(0, Math.min(path.length, s)), pt);
      const x = (pt.x + pt.nx * lat) * k;
      const y = (pt.y + pt.ny * lat) * k;
      const gs = sprite(glows, r * k, makeGlow);
      ctx.globalAlpha = alpha;
      ctx.drawImage(gs, x - gs.width / 2, y - gs.height / 2);
    };

    const drawBlob = (b: Blob, path: TubePath, heat: number) => {
      if (b.r < 0.8) return;
      const fast = Math.min(1, Math.abs(b.v) / (30 * scale));
      const stretch = 1 + fast * 0.55;
      const room = Math.max(0, T * 0.47 - b.r);
      const sway = Math.sin(time * b.swaySpeed + b.swayPhase) * room * 0.55;
      for (let pass = 0; pass < 2; pass++) {
        for (const l of b.lobes) {
          const w = time * l.speed + l.phase;
          const along = (l.along + 0.1 * Math.sin(w)) * stretch * b.r;
          const across = (l.across + 0.07 * Math.cos(w * 1.3)) * b.r;
          const size = l.size * (1 + 0.1 * Math.sin(w * 0.8)) * (l.along < 0 ? 1 - fast * 0.2 : 1);
          lobeAt(path, b.s + along, sway + across, b.r * size, heat, pass);
        }
      }
      // One glowing core, a little ahead of the middle (the top of a rising blob is hottest).
      const k = pxRef.current;
      path.at(Math.max(0, Math.min(path.length, b.s + b.r * 0.12 * stretch)), pt);
      const cs = sprite(cores, b.r * 0.78 * k, makeCore);
      ctx.globalAlpha = 0.35 + 0.55 * heat;
      ctx.drawImage(cs, (pt.x + pt.nx * sway) * k - cs.width / 2, (pt.y + pt.ny * sway) * k - cs.height / 2);
    };

    const draw = () => {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      const paths = pathsRef.current;
      const blobs = blobsRef.current;
      // The light: the heater under each pool, flickering, and every blob's glow.
      ctx.globalCompositeOperation = "lighter";
      for (let p = 0; p < paths.length; p++) {
        const flicker = 0.42 + 0.1 * Math.sin(time * 1.7 + p) + 0.06 * Math.sin(time * 5.3 + p * 2) + 0.04 * Math.sin(time * 11.1);
        glowAt(paths[p], poolS * 0.6, 0, T * 0.75, flicker);
        glowAt(paths[p], paths[p].length - T * 0.2, 0, T * 0.32 * (0.6 + topRef.current[p] * 0.6), 0.22);
      }
      for (const b of blobs) {
        if (b.state === WAITING || b.r < 0.8) continue;
        const path = paths[b.path];
        if (!path) continue;
        const f = Math.min(1, b.s / path.length);
        glowAt(path, b.s, 0, b.r, 0.18 + 0.4 * (1 - f));
      }
      ctx.globalCompositeOperation = "source-over";
      // The pools: hot at the bottom, cooler wax gathered at the crest.
      for (let p = 0; p < paths.length; p++) {
        const path = paths[p];
        // A wide mound across the tube, with a bulge slowly swelling on top.
        const w0 = time * 0.4 + p * 1.9;
        for (let pass = 0; pass < 2; pass++) {
          lobeAt(path, T * 0.12, -T * 0.2 + Math.sin(w0) * T * 0.02, T * 0.34, 1, pass);
          lobeAt(path, T * 0.12, T * 0.2 + Math.cos(w0 * 1.1) * T * 0.02, T * 0.34, 1, pass);
          lobeAt(path, T * 0.26, Math.sin(w0 * 0.7) * T * 0.05, T * 0.36, 1, pass);
          lobeAt(path, T * (0.5 + 0.06 * Math.sin(w0 * 0.6)), Math.sin(w0 * 0.5) * T * 0.1, T * 0.22 * (1 + 0.15 * Math.sin(w0 * 0.8)), 1, pass);
        }
        const fill = topRef.current[p];
        const L = path.length;
        lobeAt(path, L - T * 0.12, Math.sin(time * 0.3 + p) * T * 0.04, T * 0.3 * (0.55 + fill * 0.6), 0);
        lobeAt(path, L - T * 0.5, Math.cos(time * 0.27 + p) * T * 0.05, T * 0.22 * (0.4 + fill * 0.7), 0.05);
      }
      // The blobs, the budding ones with a neck down to the pool.
      for (const b of blobs) {
        if (b.state === WAITING) continue;
        const path = paths[b.path];
        if (!path) continue;
        const f = Math.min(1, b.s / path.length);
        const heat = b.state === BUDDING ? 1 : Math.max(0, 1 - f * 1.15);
        if (b.state === BUDDING) lobeAt(path, (b.s + poolS) / 2, 0, Math.max(1, b.r * 0.55), 1);
        drawBlob(b, path, heat);
      }
      ctx.globalAlpha = 1;
    };

    draw();
    if (!running) return;

    let last = performance.now();
    return fxLoop(fps, (now: number) => {
      step(Math.min(0.1, (now - last) / 1000));
      last = now;
      draw();
    });
  }, [running, geometry, fps]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

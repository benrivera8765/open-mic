// SPDX-License-Identifier: GPL-3.0-or-later
// The fire frame style (owner, 2026-11-23, the Molten theme): "the pipes will have a long
// flame in the pipes connecting from one end to the other ... no water in the clear pipe,
// it will just be brilliant fire". frame-art.ts fireTubeSvg draws the clear glass and the
// drifting fire light underneath is its glow; this draws the flame:
//
//   - one continuous flame from a burner at the foot of the left column, up and over the
//     arch, down to a burner at the foot of the right column: layers from a deep red
//     outer flame to a white-hot heart, drawn small and scaled up so its edges are soft,
//     swelling into bright tongues that run from both burners towards the crest;
//   - flame licks (long soft wisps along the pipe) and embers flying ahead of them;
//   - a hot glow at each burner.
//
// Cost: one canvas plus a quarter-size buffer for the soft flame body (upscaling is the
// blur), at most 30 frames a second, fixed pools and buffers (no allocation per frame),
// additive blending. Stops when `running` is false (hidden, minimised); under reduced
// motion one still frame is drawn.

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";
import { bubbleTrack, type FrameGeometry, type PathPoint, type TubePath } from "./geometry";

const FPS = 30;
const MAX_CANVAS_PIXELS = 3840 * 2160;
const LICKS = 80;
const EMBERS = 50;
/** The flame body is drawn at this share of the canvas resolution and scaled up (soft edges). */
const BODY_SCALE = 0.25;
/** Flame layers: width as a share of the flame's half-width, colour, heart only (thins where the flame is weak). */
const LAYERS: readonly [number, string, boolean][] = [
  [1.0, "rgba(200,30,6,0.5)", false],
  [0.74, "rgba(255,84,12,0.55)", false],
  [0.5, "rgba(255,150,30,0.6)", false],
  [0.3, "rgba(255,214,110,0.7)", true],
  [0.14, "rgba(255,250,225,0.8)", true],
];

interface Lick {
  alive: boolean;
  u: number;
  lat: number;
  v: number;
  age: number;
  life: number;
  size: number;
  spark: boolean;
}

export interface FireTubeProps {
  geometry: FrameGeometry;
  /** False: draw one still frame and stop. */
  running: boolean;
  class?: string;
}

function sprite(r: number, stops: [number, string][]): HTMLCanvasElement {
  const size = Math.ceil(r * 2 + 2);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, 0, m, m, r);
  for (const [o, col] of stops) g.addColorStop(o, col);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  return c;
}

/** Smooth flicker noise: a few sines of unrelated frequencies. */
function wave(x: number): number {
  return Math.sin(x) * 0.5 + Math.sin(1.73 * x + 1.3) * 0.3 + Math.sin(2.91 * x + 0.7) * 0.2;
}

export function FireTube({ geometry, running, class: className }: FireTubeProps) {
  const fps = useEffectFps(FPS);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const g = geometry;
    const T = g.tube;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    let k = Math.min(dpr, 2);
    if (g.width * g.height * k * k > MAX_CANVAS_PIXELS) k = Math.sqrt(MAX_CANVAS_PIXELS / (g.width * g.height));
    canvas.width = Math.max(1, Math.round(g.width * k));
    canvas.height = Math.max(1, Math.round(g.height * k));
    const left: TubePath = bubbleTrack(g, "left");
    const right: TubePath = bubbleTrack(g, "right");
    const L = left.length;
    const U = 2 * L;
    const latMax = T * 0.42;
    const step = Math.max(5, T * 0.16);
    const N = Math.max(8, Math.ceil(U / step) + 1);
    const body = document.createElement("canvas");
    body.width = Math.max(1, Math.round(canvas.width * BODY_SCALE));
    body.height = Math.max(1, Math.round(canvas.height * BODY_SCALE));
    const bctx = body.getContext("2d");
    if (!bctx) return;
    const bk = k * BODY_SCALE;
    // Per sample along the tube: centre point and normal (fixed; body-buffer units), flame centre, width and heat (per frame).
    const px = new Float32Array(N);
    const py = new Float32Array(N);
    const nx = new Float32Array(N);
    const ny = new Float32Array(N);
    const cl = new Float32Array(N);
    const hw = new Float32Array(N);
    const heat = new Float32Array(N);
    const pt: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
    const at = (u: number, out: PathPoint) => {
      const onLeft = u <= L;
      (onLeft ? left : right).at(Math.max(0, Math.min(L, onLeft ? u : U - u)), out);
    };
    for (let i = 0; i < N; i++) {
      at(Math.min(U, i * step), pt);
      px[i] = pt.x * bk;
      py[i] = pt.y * bk;
      nx[i] = pt.nx;
      ny[i] = pt.ny;
    }
    const flame = sprite(Math.max(6, T * 0.3 * k), [
      [0, "rgba(255,246,200,0.75)"],
      [0.3, "rgba(255,190,70,0.5)"],
      [0.65, "rgba(255,100,16,0.22)"],
      [1, "rgba(200,30,0,0)"],
    ]);
    const ember = sprite(Math.max(2, T * 0.06 * k), [
      [0, "rgba(255,255,230,1)"],
      [0.4, "rgba(255,190,80,0.8)"],
      [1, "rgba(255,90,20,0)"],
    ]);
    const burner = sprite(Math.max(10, T * 1.1 * k), [
      [0, "rgba(255,250,220,0.85)"],
      [0.3, "rgba(255,170,50,0.55)"],
      [1, "rgba(255,60,10,0)"],
    ]);
    const licks: Lick[] = [];
    for (let i = 0; i < LICKS + EMBERS; i++) licks.push({ alive: false, u: 0, lat: 0, v: 0, age: 0, life: 1, size: 1, spark: i >= LICKS });
    let lickAcc = 0;
    let emberAcc = 0;
    let time = 0;

    const spawn = (spark: boolean) => {
      const from = spark ? LICKS : 0;
      const to = spark ? LICKS + EMBERS : LICKS;
      for (let i = from; i < to; i++) {
        const p = licks[i];
        if (p.alive) continue;
        p.alive = true;
        p.u = Math.random() * U;
        const dir = p.u < L ? 1 : -1;
        p.v = dir * T * (spark ? 5 + Math.random() * 4 : 2.2 + Math.random() * 1.8);
        p.lat = (Math.random() - 0.5) * T * (spark ? 0.6 : 0.3);
        p.age = 0;
        p.life = spark ? 0.5 + Math.random() * 0.6 : 0.25 + Math.random() * 0.3;
        p.size = spark ? 0.6 + Math.random() * 0.8 : 0.55 + Math.random() * 0.6;
        return;
      }
    };

    const draw = (dt: number) => {
      time += dt;
      // The flame body this frame, in the small buffer: tongues of heat running from both
      // burners towards the crest, the flame swelling and thinning with them.
      for (let i = 0; i < N; i++) {
        const u = Math.min(U, i * step);
        const dist = Math.min(u, U - u);
        const s = dist / T - time * 4.2;
        const foot = Math.min(1, dist / (T * 1.2));
        const tongue = 0.5 + 0.5 * wave(s * 0.42 + 0.3);
        const flick = 0.82 + 0.18 * Math.sin(time * 13 + s * 1.7);
        heat[i] = Math.min(1, (0.3 + 0.7 * tongue * tongue) * flick) * (0.65 + 0.35 * foot);
        cl[i] = T * 0.08 * wave(s * 0.33 + 1.1) * bk;
        hw[i] = T * (0.16 + 0.24 * heat[i]) * (0.7 + 0.3 * foot) * bk;
      }
      bctx.setTransform(1, 0, 0, 1, 0, 0);
      bctx.clearRect(0, 0, body.width, body.height);
      bctx.globalCompositeOperation = "lighter";
      const lim = latMax * bk;
      for (const [scale, color, heart] of LAYERS) {
        bctx.beginPath();
        for (let i = 0; i < N; i++) {
          const w = hw[i] * scale * (heart ? Math.max(0.15, heat[i] * 1.3 - 0.3) : 1);
          const off = Math.max(-lim, Math.min(lim, cl[i] - w));
          const x = px[i] + nx[i] * off;
          const y = py[i] + ny[i] * off;
          if (i === 0) bctx.moveTo(x, y);
          else bctx.lineTo(x, y);
        }
        for (let i = N - 1; i >= 0; i--) {
          const w = hw[i] * scale * (heart ? Math.max(0.15, heat[i] * 1.3 - 0.3) : 1);
          const off = Math.max(-lim, Math.min(lim, cl[i] + w));
          bctx.lineTo(px[i] + nx[i] * off, py[i] + ny[i] * off);
        }
        bctx.closePath();
        bctx.fillStyle = color;
        bctx.fill();
      }
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.globalCompositeOperation = "lighter";
      ctx.imageSmoothingEnabled = true;
      ctx.globalAlpha = 1;
      ctx.drawImage(body, 0, 0, canvas.width, canvas.height);
      // Licks and embers streaming towards the crest.
      if (dt > 0) {
        lickAcc += dt * 110;
        emberAcc += dt * 30;
        while (lickAcc >= 1) {
          lickAcc -= 1;
          spawn(false);
        }
        while (emberAcc >= 1) {
          emberAcc -= 1;
          spawn(true);
        }
      }
      for (const p of licks) {
        if (!p.alive) continue;
        p.age += dt;
        const t = p.age / p.life;
        if (t >= 1) {
          p.alive = false;
          continue;
        }
        p.u += p.v * dt;
        if (p.u < 0 || p.u > U) {
          p.alive = false;
          continue;
        }
        at(p.u, pt);
        const lat = Math.max(-latMax, Math.min(latMax, p.lat + (p.spark ? Math.sin(p.age * 20 + p.u) * T * 0.04 : 0)));
        const x = (pt.x + pt.nx * lat) * k;
        const y = (pt.y + pt.ny * lat) * k;
        if (p.spark) {
          const s = ember.width * p.size;
          ctx.globalAlpha = (1 - t) * (0.7 + 0.3 * Math.random());
          ctx.drawImage(ember, x - s / 2, y - s / 2, s, s);
          continue;
        }
        // A lick: a soft wisp stretched along the pipe, swelling then fading.
        const s = flame.width * p.size * (0.6 + 0.6 * Math.sin(Math.PI * Math.min(1, t * 1.4)));
        ctx.globalAlpha = (1 - t * t) * (0.75 + 0.25 * Math.random());
        // x axis along the pipe (the normal turned a quarter), stretched; y across it, squeezed.
        ctx.setTransform(-pt.ny * 2.6, pt.nx * 2.6, pt.nx * 0.7, pt.ny * 0.7, x, y);
        ctx.drawImage(flame, -s / 2, -s / 2, s, s);
        ctx.setTransform(1, 0, 0, 1, 0, 0);
      }
      // The burners at both feet.
      for (const u of [0, U]) {
        at(u, pt);
        const s = burner.width * (0.9 + 0.1 * Math.sin(time * 11 + u));
        ctx.globalAlpha = 0.8;
        ctx.drawImage(burner, pt.x * k - s / 2, pt.y * k - s / 2, s, s);
      }
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    };

    // A still frame starts with licks already burning.
    for (let i = 0; i < 20; i++) draw(0.03);
    if (!running) return;
    let last = performance.now();
    return fxLoop(fps, (now: number) => {
      draw(Math.min(0.1, (now - last) / 1000));
      last = now;
    });
  }, [running, geometry, fps]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

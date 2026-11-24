// SPDX-License-Identifier: GPL-3.0-or-later
// The lightning-tube frame style (owner, 2026-11-23): electricity in a tube of clear
// blue water (frame-art.ts waterTubeSvg draws the water and the glass; the small
// bubbles are the ordinary bubble layer).
//
// One continuous line of electricity runs the whole tube, from the terminal at the
// foot of the left column, up and over the arch, down to the terminal at the foot of
// the right column, and dances inside the tube like a Tesla-coil arc:
//   - its path across the tube is a sum of travelling waves, so the whole arc writhes
//     and snakes all the time, pinned to the two terminals at its ends;
//   - a jagged crackle on top that changes every frame but carries over a little from
//     the frame before, so it flickers like a real arc instead of shimmering noise;
//   - a second full arc with its own waves, so two strands dance round each other, and a
//     third, fainter strand that comes and goes;
//   - short streamers that flick off the arc towards the glass and vanish;
//   - now and then a surge, when the whole arc flares brighter;
//   - both terminals glow and pulse.
// Brilliant purple, drawn additively in three passes (glow, body, white-hot core), no
// blur filters.
//
// Cost: one canvas, at most 30 frames a second, preallocated point buffers; stops when
// `running` is false (hidden, minimised); under reduced motion one still frame is drawn.

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";
import { bubbleTrack, type FrameGeometry, type PathPoint, type TubePath } from "./geometry";

const FPS = 30;
const MAX_CANVAS_PIXELS = 3840 * 2160;
const STREAMERS = 6;
const STREAMER_PTS = 10;

interface Strand {
  phase: number;
  /** Crackle carried from frame to frame, one value per point. */
  kink: Float32Array;
  xs: Float32Array;
  ys: Float32Array;
  /** 0..1, how visible the strand is now (the second one comes and goes). */
  level: number;
  target: number;
  next: number;
}

interface Streamer {
  alive: boolean;
  u: number;
  dir: number;
  age: number;
  life: number;
  ds: Float32Array;
  dl: Float32Array;
}

export interface LightningProps {
  geometry: FrameGeometry;
  /** False: draw one still frame and stop. */
  running: boolean;
  class?: string;
}

function makeSpark(r: number): HTMLCanvasElement {
  const size = Math.ceil(r * 2 + 2);
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const ctx = c.getContext("2d");
  if (!ctx) return c;
  const m = size / 2;
  const g = ctx.createRadialGradient(m, m, 0, m, m, r);
  g.addColorStop(0, "rgba(255,240,255,0.95)");
  g.addColorStop(0.25, "rgba(214,130,255,0.6)");
  g.addColorStop(1, "rgba(160,60,255,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  return c;
}

export function Lightning({ geometry, running, class: className }: LightningProps) {
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
    /** The whole tube as one line: u = 0 at the left terminal, 2L at the right one. */
    const U = 2 * L;
    const latMax = T * 0.42;
    const step = Math.max(6, T * 0.24);
    const N = Math.max(8, Math.ceil(U / step) + 1);
    const pt: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
    const spark = makeSpark(Math.max(4, T * 0.2 * k));
    const wGlow = Math.max(6, T * 0.17) * k;
    const wBody = Math.max(2, T * 0.05) * k;
    const wCore = Math.max(1, T * 0.019) * k;
    const sx = new Float32Array(STREAMER_PTS);
    const sy = new Float32Array(STREAMER_PTS);
    let time = 0;
    let surge = 0;

    const place = (u: number, lat: number, out: { x: number; y: number }) => {
      const onLeft = u <= L;
      const s = Math.max(0, Math.min(L, onLeft ? u : U - u));
      (onLeft ? left : right).at(s, pt);
      const l = Math.max(-latMax, Math.min(latMax, lat));
      out.x = (pt.x + pt.nx * l) * k;
      out.y = (pt.y + pt.ny * l) * k;
    };
    const tmp = { x: 0, y: 0 };

    /** Eases the arc into the terminals at both ends. */
    const pin = (u: number) => {
      const a = Math.min(1, u / (T * 1.4));
      const b = Math.min(1, (U - u) / (T * 1.4));
      return a * a * (3 - 2 * a) * (b * b * (3 - 2 * b));
    };

    /** Where the arc crosses the tube at u: travelling waves, so it snakes all the time. */
    const wave = (u: number, ph: number) =>
      (Math.sin(u / (T * 1.3) + time * 1.8 + ph) * 0.55 +
        Math.sin(u / (T * 0.55) - time * 3.1 + ph * 1.7) * 0.3 +
        Math.sin(u / (T * 3.2) + time * 0.7 + ph * 0.6) * 0.35) *
      latMax *
      0.62;

    // Two full arcs (the owner asked for a second one) and a faint third that comes and goes.
    const strands: Strand[] = [0, 1, 2].map((i) => ({
      phase: [0.7, 3.4, 5.6][i],
      kink: new Float32Array(N),
      xs: new Float32Array(N),
      ys: new Float32Array(N),
      level: i < 2 ? 1 : 0.5,
      target: i < 2 ? 1 : 0.5,
      next: 0,
    }));
    const streamers: Streamer[] = [];
    for (let i = 0; i < STREAMERS; i++) {
      streamers.push({ alive: false, u: 0, dir: 1, age: 0, life: 0, ds: new Float32Array(STREAMER_PTS), dl: new Float32Array(STREAMER_PTS) });
    }

    const stroke = (xs: Float32Array, ys: Float32Array, count: number, alpha: number, bright: number) => {
      if (count < 2 || alpha <= 0.01) return;
      ctx.beginPath();
      ctx.moveTo(xs[0], ys[0]);
      for (let i = 1; i < count; i++) ctx.lineTo(xs[i], ys[i]);
      ctx.globalAlpha = Math.min(1, alpha);
      ctx.strokeStyle = "rgba(168,52,255,0.34)";
      ctx.lineWidth = wGlow * bright;
      ctx.stroke();
      ctx.strokeStyle = "rgba(204,112,255,0.8)";
      ctx.lineWidth = wBody * bright;
      ctx.stroke();
      ctx.strokeStyle = "rgba(252,234,255,0.95)";
      ctx.lineWidth = wCore * bright;
      ctx.stroke();
    };

    const sparkAt = (x: number, y: number, alpha: number, scale: number) => {
      ctx.globalAlpha = alpha;
      const w = spark.width * scale;
      ctx.drawImage(spark, x - w / 2, y - w / 2, w, w);
    };

    const draw = (dt: number) => {
      time += dt;
      surge = Math.max(0, surge - dt);
      if (dt > 0 && surge === 0 && Math.random() < dt * 0.35) surge = 0.12 + Math.random() * 0.12;
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.globalCompositeOperation = "lighter";
      ctx.lineCap = "round";
      ctx.lineJoin = "round";

      const lead = strands[0];
      for (let si = 0; si < strands.length; si++) {
        const st = strands[si];
        if (si > 1) {
          // The faint third strand comes and goes.
          st.next -= dt;
          if (st.next <= 0) {
            st.target = Math.random() < 0.6 ? 0.35 + Math.random() * 0.5 : 0;
            st.next = 0.3 + Math.random() * 1.2;
          }
          st.level += (st.target - st.level) * Math.min(1, dt * 6);
        }
        for (let i = 0; i < N; i++) {
          const u = Math.min(U, i * step);
          const p = pin(u);
          st.kink[i] = st.kink[i] * 0.55 + (Math.random() - 0.5) * T * 0.09;
          const lat = (wave(u, st.phase) + st.kink[i] + (Math.random() - 0.5) * T * 0.025) * p;
          place(u, lat, tmp);
          st.xs[i] = tmp.x;
          st.ys[i] = tmp.y;
        }
        const flicker = si < 2 ? 0.78 + 0.22 * Math.random() : 0.6 + 0.4 * Math.random();
        const bright = (si === 0 ? 1 : si === 1 ? 0.92 : 0.7) * (surge > 0 ? 1.5 : 1);
        stroke(st.xs, st.ys, N, st.level * flicker * (surge > 0 ? 1.15 : 1), bright);
      }

      // Streamers flicking off the arc towards the glass.
      if (dt > 0 && Math.random() < dt * 9) {
        const s = streamers.find((q) => !q.alive);
        if (s) {
          s.alive = true;
          s.u = T * 1.5 + Math.random() * (U - T * 3);
          s.dir = Math.random() < 0.5 ? -1 : 1;
          s.age = 0;
          s.life = 0.07 + Math.random() * 0.16;
          let ds = 0;
          let dl = 0;
          for (let i = 0; i < STREAMER_PTS; i++) {
            s.ds[i] = ds;
            s.dl[i] = dl;
            ds += (Math.random() - 0.5) * T * 0.22;
            dl += s.dir * T * (0.03 + Math.random() * 0.06);
          }
        }
      }
      for (const s of streamers) {
        if (!s.alive) continue;
        s.age += dt;
        if (s.age >= s.life) {
          s.alive = false;
          continue;
        }
        const base = wave(s.u, lead.phase) * pin(s.u);
        for (let i = 0; i < STREAMER_PTS; i++) {
          place(s.u + s.ds[i], base + s.dl[i] + (Math.random() - 0.5) * T * 0.03, tmp);
          sx[i] = tmp.x;
          sy[i] = tmp.y;
        }
        stroke(sx, sy, STREAMER_PTS, (1 - s.age / s.life) * (0.6 + 0.4 * Math.random()), 0.6);
        if (Math.abs(base + s.dl[STREAMER_PTS - 1]) > latMax * 0.9) sparkAt(sx[STREAMER_PTS - 1], sy[STREAMER_PTS - 1], 0.6, 0.8);
      }

      // The two terminals.
      for (const u of [0, U]) {
        place(u, 0, tmp);
        sparkAt(tmp.x, tmp.y, 0.7 + 0.25 * Math.sin(time * 9 + u) * Math.random(), 2.2 + (surge > 0 ? 0.8 : 0));
      }
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    };

    draw(0);
    if (!running) return;
    let last = performance.now();
    return fxLoop(fps, (now: number) => {
      draw(Math.min(0.1, (now - last) / 1000));
      last = now;
    });
  }, [running, geometry, fps]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

// SPDX-License-Identifier: GPL-3.0-or-later
// The ocean-tube frame style (owner, 2026-11-23: a border to go with the aquarium
// cabinet): the screen border is a glass tube of tropical sea water (frame-art.ts
// oceanTubeSvg draws the water and the glass; the small bubbles are the bubble layer).
// In it, drawn here:
//   - fish (the same kinds as the aquarium, sea-life.ts) swimming along the tube, up a
//     column, over the arch and down the other side, turning to follow the curve,
//     changing depth across the tube and turning round now and then;
//   - kelp swaying up from coral at the foot of each column, and a starfish on the glass;
//   - a jellyfish drifting slowly round each half of the arch, pulsing and glowing.
//
// Cost: one canvas, at most 24 frames a second, a handful of creatures; stops when
// `running` is false (hidden, minimised); under reduced motion one still frame is drawn.

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";
import { bubbleTrack, type FrameGeometry, type PathPoint, type TubePath } from "./geometry";
import { drawFishShape, type FishKind } from "./sea-life";

const FPS = 24;
const MAX_CANVAS_PIXELS = 3840 * 2160;

interface SeaFish {
  kind: FishKind;
  /** Position along the whole tube: 0 = foot of the left column, U = foot of the right one. */
  u: number;
  lat: number;
  latT: number;
  v: number;
  size: number;
  phase: number;
  next: number;
}

interface Jelly {
  u: number;
  v: number;
  lo: number;
  hi: number;
  phase: number;
}

export interface OceanTubeProps {
  geometry: FrameGeometry;
  /** False: draw one still frame and stop. */
  running: boolean;
  class?: string;
}

export function OceanTube({ geometry, running, class: className }: OceanTubeProps) {
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
    const pt: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
    const a = { x: 0, y: 0 };
    const b = { x: 0, y: 0 };
    const column = Math.max(T * 2, g.baseTop - g.cornerCy);
    let time = 0;

    /** Canvas CSS-px position of (u, lat) along the whole tube. */
    const place = (u: number, lat: number, out: { x: number; y: number }) => {
      const onLeft = u <= L;
      const s = Math.max(0, Math.min(L, onLeft ? u : U - u));
      (onLeft ? left : right).at(s, pt);
      out.x = pt.x + pt.nx * lat;
      out.y = pt.y + pt.ny * lat;
    };

    const kinds: FishKind[] = ["clown", "blueTang", "yellowTang", "clown", "blueTang", "yellowTang", "tetra", "tetra", "tetra"];
    const fish: SeaFish[] = kinds.map((kind, i) => ({
      kind,
      u: ((i + Math.random() * 0.8) / kinds.length) * U,
      lat: (Math.random() - 0.5) * latMax,
      latT: 0,
      v: (Math.random() < 0.5 ? -1 : 1) * T * (0.3 + Math.random() * 0.35) * (kind === "tetra" ? 1.4 : 1),
      size: T * (kind === "tetra" ? 0.36 : 0.52 + Math.random() * 0.12),
      phase: Math.random() * 6.28,
      next: 1 + Math.random() * 4,
    }));
    const jellies: Jelly[] = [
      { u: L * 0.72, v: T * 0.09, lo: L * 0.5, hi: L * 0.95, phase: 0 },
      { u: U - L * 0.66, v: -T * 0.08, lo: U - L * 0.95, hi: U - L * 0.5, phase: 2.1 },
    ];

    const drawKelp = (u0: number, side: number) => {
      const h = column * 0.36;
      for (let i = 0; i < 3; i++) {
        const baseLat = (i - 1) * T * 0.25;
        const hh = h * (0.7 + 0.3 * ((i * 37) % 10) / 10);
        const ph = i * 1.9 + side * 0.7;
        const n = 10;
        const grad = ctx.createLinearGradient(0, g.baseTop, 0, g.baseTop - hh);
        grad.addColorStop(0, "#0e5a2c");
        grad.addColorStop(1, "#46b04a");
        ctx.fillStyle = grad;
        ctx.beginPath();
        for (let j = 0; j <= n; j++) {
          const f = j / n;
          const lat = baseLat + Math.sin(time * 0.9 + ph + f * 2.4) * T * 0.12 * f;
          const w = T * 0.13 * (1 - f * 0.7);
          place(u0 + side * f * hh, lat - w / 2, a);
          if (j === 0) ctx.moveTo(a.x, a.y);
          else ctx.lineTo(a.x, a.y);
        }
        for (let j = n; j >= 0; j--) {
          const f = j / n;
          const lat = baseLat + Math.sin(time * 0.9 + ph + f * 2.4) * T * 0.12 * f;
          const w = T * 0.13 * (1 - f * 0.7);
          place(u0 + side * f * hh, lat + w / 2, a);
          ctx.lineTo(a.x, a.y);
        }
        ctx.closePath();
        ctx.fill();
      }
    };

    const drawCoral = (u0: number) => {
      place(u0, 0, a);
      const x0 = a.x;
      const y0 = a.y;
      ctx.lineCap = "round";
      const branch = (x: number, y: number, ang: number, len: number, wdt: number, depth: number, color: string) => {
        const x2 = x + Math.cos(ang) * len;
        const y2 = y + Math.sin(ang) * len;
        ctx.strokeStyle = color;
        ctx.lineWidth = wdt;
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(x2, y2);
        ctx.stroke();
        if (depth > 0) {
          branch(x2, y2, ang - 0.45, len * 0.72, wdt * 0.72, depth - 1, color);
          branch(x2, y2, ang + 0.4, len * 0.68, wdt * 0.72, depth - 1, color);
        }
      };
      branch(x0 - T * 0.24, y0, -Math.PI / 2 - 0.15, T * 0.2, T * 0.07, 2, "#ff6f61");
      branch(x0 + T * 0.2, y0, -Math.PI / 2 + 0.2, T * 0.17, T * 0.06, 2, "#ff9a4a");
      // A brain coral and a starfish on the glass.
      ctx.fillStyle = "#c76bd8";
      ctx.beginPath();
      ctx.ellipse(x0 + T * 0.02, y0 - T * 0.04, T * 0.13, T * 0.09, 0, Math.PI, 0);
      ctx.fill();
      const sx = x0 + T * 0.27;
      const sy = y0 - T * 0.5;
      const r = T * 0.1;
      ctx.fillStyle = "#ff8a2a";
      ctx.beginPath();
      for (let i = 0; i < 10; i++) {
        const ang = -Math.PI / 2 + (i * Math.PI) / 5;
        const rr = i % 2 === 0 ? r : r * 0.42;
        if (i === 0) ctx.moveTo(sx + Math.cos(ang) * rr, sy + Math.sin(ang) * rr);
        else ctx.lineTo(sx + Math.cos(ang) * rr, sy + Math.sin(ang) * rr);
      }
      ctx.closePath();
      ctx.fill();
    };

    const drawJelly = (j: Jelly) => {
      place(j.u, 0, a);
      const pulse = 1 + 0.12 * Math.sin(time * 2.2 + j.phase);
      const bw = T * 0.17 * pulse;
      const bh = T * 0.12 / pulse;
      const top = a.y - T * 0.12;
      ctx.globalCompositeOperation = "lighter";
      const glow = ctx.createRadialGradient(a.x, top, 0, a.x, top, T * 0.32);
      glow.addColorStop(0, "rgba(255,120,220,0.35)");
      glow.addColorStop(1, "rgba(255,120,220,0)");
      ctx.fillStyle = glow;
      ctx.fillRect(a.x - T * 0.32, top - T * 0.32, T * 0.64, T * 0.64);
      ctx.globalCompositeOperation = "source-over";
      const bell = ctx.createRadialGradient(a.x, top - bh * 0.3, 0, a.x, top, bw);
      bell.addColorStop(0, "rgba(255,235,250,0.9)");
      bell.addColorStop(0.6, "rgba(255,150,225,0.7)");
      bell.addColorStop(1, "rgba(220,90,200,0.35)");
      ctx.fillStyle = bell;
      ctx.beginPath();
      ctx.ellipse(a.x, top, bw, bh, 0, Math.PI, 0);
      ctx.quadraticCurveTo(a.x, top + bh * 0.35, a.x - bw, top);
      ctx.fill();
      ctx.strokeStyle = "rgba(255,170,235,0.55)";
      ctx.lineWidth = 1;
      for (let t = 0; t < 5; t++) {
        const tx = a.x - bw * 0.7 + (t * bw * 1.4) / 4;
        ctx.beginPath();
        ctx.moveTo(tx, top + 1);
        for (let q = 1; q <= 6; q++) ctx.lineTo(tx + Math.sin(time * 3 + t + q * 0.9 + j.phase) * T * 0.025, top + (q * T * 0.3) / 6);
        ctx.stroke();
      }
    };

    const step = (dt: number) => {
      time += dt;
      for (const f of fish) {
        f.next -= dt;
        if (f.next <= 0) {
          f.next = 3 + Math.random() * 6;
          f.latT = (Math.random() * 2 - 1) * (latMax - f.size * 0.3);
          if (Math.random() < 0.25) f.v = -f.v;
        }
        f.lat += (f.latT - f.lat) * Math.min(1, dt * 0.6);
        f.u += f.v * dt;
        if (f.u < T * 0.9 && f.v < 0) f.v = -f.v;
        if (f.u > U - T * 0.9 && f.v > 0) f.v = -f.v;
      }
      for (const j of jellies) {
        j.u += j.v * dt * (0.6 + 0.4 * Math.max(0, Math.sin(time * 2.2 + j.phase)));
        if (j.u < j.lo || j.u > j.hi) j.v = -j.v;
      }
    };

    const draw = () => {
      ctx.setTransform(k, 0, 0, k, 0, 0);
      ctx.clearRect(0, 0, g.width, g.height);
      drawKelp(0, 1);
      drawKelp(U, -1);
      for (const j of jellies) drawJelly(j);
      for (const f of fish) {
        // Facing the way it swims along the tube, kept upright.
        place(f.u, f.lat, a);
        place(f.u + Math.sign(f.v) * 2, f.lat, b);
        const ang = Math.atan2(b.y - a.y, b.x - a.x);
        const s = f.size / 30;
        const w = Math.sin(time * (f.kind === "tetra" ? 14 : 8) + f.phase) * 0.35;
        ctx.save();
        ctx.translate(a.x, a.y);
        if (Math.cos(ang) >= 0) {
          ctx.rotate(ang);
          ctx.scale(s, s);
        } else {
          ctx.rotate(ang - Math.PI);
          ctx.scale(-s, s);
        }
        drawFishShape(ctx, f.kind, w);
        ctx.restore();
      }
      drawCoral(0);
      drawCoral(U);
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

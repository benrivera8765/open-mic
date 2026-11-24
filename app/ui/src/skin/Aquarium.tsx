// SPDX-License-Identifier: GPL-3.0-or-later
// The aquarium cabinet (owner, 2026-11-23): the cabinet's console band becomes a lit
// fish tank, deep blue water with slow light rays from above, sand and pebbles, rocks,
// airstones, swaying plants (tall ribbon weed, broad sword plants, red stem plants and
// grass tufts), fish that swim, bob, wag their tails and turn round (clownfish, blue
// and yellow tangs, angelfish and a school of neon tetras), and streams of bubbles.
// The water colour of the rest of the cabinet is CSS (skin.css).
//
// Cost: two canvases over the console band only. The sand, pebbles, rocks and airstones
// are drawn once per size on the back canvas; the front canvas redraws the rays,
// plants, fish and bubbles at most 24 times a second, with fixed pools (no allocation
// per frame). Stops when `running` is false (hidden, minimised); under reduced motion
// one still frame is drawn.

import { useEffect, useRef } from "preact/hooks";
import { fxLoop, useEffectFps } from "../lib/fxrate";
import { drawFishShape, type FishKind } from "./sea-life";

const FPS = 24;
const MAX_PIXELS = 3840 * 1200;
const MAX_BUBBLES = 90;


interface Fish {
  kind: FishKind;
  x: number;
  y: number;
  baseY: number;
  targetY: number;
  size: number;
  speed: number;
  dir: number;
  /** Current horizontal scale, easing towards dir (a turn passes through 0). */
  turn: number;
  phase: number;
  wag: number;
  /** 0 = at the back (smaller, behind the middle plants), 1 = at the front. */
  depth: number;
  next: number;
  /** Tetras: place in the school relative to the leader. */
  leader: number;
  dx: number;
  dy: number;
}

type PlantKind = "ribbon" | "sword" | "red" | "grass";

interface Plant {
  kind: PlantKind;
  x: number;
  h: number;
  w: number;
  phase: number;
  layer: number;
  color: string;
  tip: string;
  leaves: number;
}

interface Bubble {
  alive: boolean;
  x: number;
  y: number;
  r: number;
  vy: number;
  phase: number;
}

export interface AquariumProps {
  /** False stops the loop (hidden page, minimised window). */
  running: boolean;
  /** False: one still frame (reduced motion). */
  animate: boolean;
  /** Width of the frame and height of the band at its bottom that the tank fills (CSS px). */
  width: number;
  band: number;
  class?: string;
}

function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), a | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function Aquarium({ running, animate, width, band, class: className }: AquariumProps) {
  const fps = useEffectFps(FPS);
  const backRef = useRef<HTMLCanvasElement>(null);
  const frontRef = useRef<HTMLCanvasElement>(null);
  const fishRef = useRef<Fish[]>([]);

  useEffect(() => {
    const back = backRef.current;
    const front = frontRef.current;
    const bctx = back?.getContext("2d");
    const ctx = front?.getContext("2d");
    if (!back || !front || !bctx || !ctx || width < 50 || band < 20) return;
    const W = width;
    const H = band;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    let k = Math.min(dpr, 2);
    if (W * H * k * k > MAX_PIXELS) k = Math.sqrt(MAX_PIXELS / (W * H));
    for (const c of [back, front]) {
      c.width = Math.max(1, Math.round(W * k));
      c.height = Math.max(1, Math.round(H * k));
    }
    const sandH = Math.max(12, H * 0.12);
    const floor = H - sandH * 0.55;
    const scale = Math.max(0.6, H / 210);
    const rnd = seeded(0xa9fa);

    // ---- The back: sand, pebbles, rocks, airstones (drawn once).
    bctx.setTransform(k, 0, 0, k, 0, 0);
    bctx.clearRect(0, 0, W, H);
    const sand = bctx.createLinearGradient(0, H - sandH, 0, H);
    sand.addColorStop(0, "#d8bd84");
    sand.addColorStop(0.5, "#c4a46a");
    sand.addColorStop(1, "#8f7348");
    bctx.fillStyle = sand;
    bctx.beginPath();
    bctx.moveTo(0, H - sandH);
    for (let x = 0; x <= W; x += 40) bctx.lineTo(x, H - sandH + Math.sin(x * 0.013) * sandH * 0.18 + Math.sin(x * 0.041) * sandH * 0.08);
    bctx.lineTo(W, H);
    bctx.lineTo(0, H);
    bctx.closePath();
    bctx.fill();
    const rocks = Math.max(3, Math.round(W / 420));
    for (let i = 0; i < rocks; i++) {
      const rx = ((i + 0.3 + rnd() * 0.4) / rocks) * W;
      const rw = H * (0.18 + rnd() * 0.16);
      const rh = rw * (0.45 + rnd() * 0.25);
      const g = bctx.createRadialGradient(rx - rw * 0.25, H - sandH - rh * 0.3, rw * 0.1, rx, H - sandH, rw);
      g.addColorStop(0, "#7d8a8e");
      g.addColorStop(0.6, "#4a5458");
      g.addColorStop(1, "#262c30");
      bctx.fillStyle = g;
      bctx.beginPath();
      bctx.ellipse(rx, H - sandH * 0.7, rw, rh, 0, Math.PI, 0);
      bctx.closePath();
      bctx.fill();
    }
    const pebbleColors = ["#f4efe6", "#b9b2a6", "#7c7266", "#d26a4a", "#3d6fd8", "#e8d27a", "#5a4a3c"];
    const pebbles = Math.round(W / 6);
    for (let i = 0; i < pebbles; i++) {
      const px = rnd() * W;
      const py = H - sandH * (0.1 + rnd() * 0.75);
      const pr = 1 + rnd() * 2.6 * scale;
      bctx.fillStyle = pebbleColors[Math.floor(rnd() * pebbleColors.length)];
      bctx.globalAlpha = 0.75 + rnd() * 0.25;
      bctx.beginPath();
      bctx.ellipse(px, py, pr * 1.3, pr, rnd() * 3, 0, Math.PI * 2);
      bctx.fill();
    }
    bctx.globalAlpha = 1;
    const stones = [0.16, 0.52, 0.86].map((f) => f * W);
    for (const sx of stones) {
      const g = bctx.createLinearGradient(sx - 8, 0, sx + 8, 0);
      g.addColorStop(0, "#5a6064");
      g.addColorStop(0.5, "#a9b0b4");
      g.addColorStop(1, "#4a5054");
      bctx.fillStyle = g;
      bctx.fillRect(sx - 7 * scale, floor - 6 * scale, 14 * scale, 7 * scale);
    }

    // ---- Plants (placed once per size; swayed every frame).
    const plants: Plant[] = [];
    const greens = ["#1f7a3a", "#2a9446", "#3aa856", "#17663a"];
    const n = Math.max(10, Math.round(W / 70));
    for (let i = 0; i < n; i++) {
      const x = ((i + rnd()) / n) * W;
      const u = rnd();
      const kind: PlantKind = u < 0.38 ? "ribbon" : u < 0.58 ? "sword" : u < 0.72 ? "red" : "grass";
      const layer = kind === "ribbon" ? (rnd() < 0.7 ? 0 : 1) : kind === "grass" ? 2 : 1;
      plants.push({
        kind,
        x,
        h: H * (kind === "ribbon" ? 0.55 + rnd() * 0.4 : kind === "sword" ? 0.35 + rnd() * 0.25 : kind === "red" ? 0.4 + rnd() * 0.3 : 0.12 + rnd() * 0.12),
        w: (kind === "ribbon" ? 4 + rnd() * 3 : 6 + rnd() * 4) * scale,
        phase: rnd() * 6.28,
        layer,
        color: kind === "red" ? "#b8402a" : greens[Math.floor(rnd() * greens.length)],
        tip: kind === "red" ? "#ff7a4a" : "#7ad46a",
        leaves: kind === "sword" ? 5 + Math.floor(rnd() * 3) : kind === "grass" ? 6 + Math.floor(rnd() * 5) : kind === "ribbon" ? 2 + Math.floor(rnd() * 2) : 7,
      });
    }

    // ---- Fish (made once; they keep swimming across resizes).
    if (fishRef.current.length === 0) {
      const make = (kind: FishKind, size: number, depth: number, leader = -1, dx = 0, dy = 0): Fish => {
        const y = H * (0.2 + Math.random() * 0.5);
        return {
          kind,
          x: Math.random() * W,
          y,
          baseY: y,
          targetY: y,
          size,
          speed: (kind === "tetra" ? 46 : kind === "angel" ? 18 : 26 + Math.random() * 18) * (0.8 + Math.random() * 0.4),
          dir: Math.random() < 0.5 ? -1 : 1,
          turn: 1,
          phase: Math.random() * 6.28,
          wag: kind === "tetra" ? 14 : 7 + Math.random() * 3,
          depth,
          next: 2 + Math.random() * 5,
          leader,
          dx,
          dy,
        };
      };
      const fish: Fish[] = [
        make("clown", 34, 1),
        make("clown", 28, 0.3),
        make("blueTang", 40, 1),
        make("yellowTang", 34, 0.7),
        make("yellowTang", 28, 0.2),
        make("angel", 40, 0.5),
        make("blueTang", 30, 0.25),
      ];
      const lead = fish.length;
      fish.push(make("tetra", 15, 0.8));
      for (let i = 1; i < 7; i++) fish.push(make("tetra", 13 + Math.random() * 3, 0.8, lead, -i * 16 - Math.random() * 10, (Math.random() - 0.5) * 30));
      for (const f of fish) f.turn = f.dir;
      fishRef.current = fish;
    }
    const fish = fishRef.current;
    for (const f of fish) {
      f.x = Math.min(f.x, W + 40);
      f.baseY = Math.min(Math.max(f.baseY, H * 0.15), floor - 20);
      f.targetY = f.baseY;
    }

    const bubbles: Bubble[] = [];
    for (let i = 0; i < MAX_BUBBLES; i++) bubbles.push({ alive: false, x: 0, y: 0, r: 0, vy: 0, phase: 0 });
    const nextBubble = stones.map(() => Math.random() * 0.3);
    let time = 0;

    const sway = (p: Plant, f: number) => Math.sin(time * 0.9 + p.phase + f * 2.2) * p.h * 0.1 * Math.pow(f, 1.4);

    const drawPlant = (p: Plant) => {
      const base = floor + 2;
      if (p.kind === "ribbon") {
        for (let b = 0; b < p.leaves; b++) {
          const off = (b - (p.leaves - 1) / 2) * p.w * 1.4;
          const hh = p.h * (1 - b * 0.12);
          const g = ctx.createLinearGradient(0, base, 0, base - hh);
          g.addColorStop(0, "#0f4a26");
          g.addColorStop(1, p.color);
          ctx.fillStyle = g;
          ctx.beginPath();
          const seg = 9;
          for (let j = 0; j <= seg; j++) {
            const f = j / seg;
            const x = p.x + off + sway(p, f + b * 0.3) - (p.w * (1 - f * 0.7)) / 2;
            const y = base - f * hh;
            if (j === 0) ctx.moveTo(x, y);
            else ctx.lineTo(x, y);
          }
          for (let j = seg; j >= 0; j--) {
            const f = j / seg;
            ctx.lineTo(p.x + off + sway(p, f + b * 0.3) + (p.w * (1 - f * 0.7)) / 2, base - f * hh);
          }
          ctx.closePath();
          ctx.fill();
        }
      } else if (p.kind === "sword") {
        for (let i = 0; i < p.leaves; i++) {
          const a = -Math.PI / 2 + ((i / (p.leaves - 1)) - 0.5) * 1.6 + Math.sin(time * 0.8 + p.phase + i) * 0.06;
          const len = p.h * (0.7 + 0.3 * Math.sin(i * 2.1 + p.phase) * 0.5 + 0.15);
          const tx = p.x + Math.cos(a) * len;
          const ty = base + Math.sin(a) * len;
          const nx = -Math.sin(a) * p.w;
          const ny = Math.cos(a) * p.w;
          const mx = (p.x + tx) / 2;
          const my = (base + ty) / 2;
          ctx.fillStyle = p.color;
          ctx.beginPath();
          ctx.moveTo(p.x, base);
          ctx.quadraticCurveTo(mx + nx, my + ny, tx, ty);
          ctx.quadraticCurveTo(mx - nx, my - ny, p.x, base);
          ctx.fill();
          ctx.strokeStyle = "rgba(170,230,140,0.45)";
          ctx.lineWidth = 1;
          ctx.beginPath();
          ctx.moveTo(p.x, base);
          ctx.lineTo(tx, ty);
          ctx.stroke();
        }
      } else if (p.kind === "red") {
        const seg = p.leaves;
        let px = p.x;
        let py = base;
        ctx.strokeStyle = "#7a2a1a";
        ctx.lineWidth = 2;
        for (let j = 1; j <= seg; j++) {
          const f = j / seg;
          const x = p.x + sway(p, f);
          const y = base - f * p.h;
          ctx.beginPath();
          ctx.moveTo(px, py);
          ctx.lineTo(x, y);
          ctx.stroke();
          ctx.fillStyle = j > seg - 2 ? p.tip : p.color;
          for (const s of [-1, 1]) {
            ctx.beginPath();
            ctx.ellipse(x + s * p.w * 0.7, y, p.w * 0.75, p.w * 0.32, s * 0.5, 0, Math.PI * 2);
            ctx.fill();
          }
          px = x;
          py = y;
        }
      } else {
        ctx.strokeStyle = p.color;
        ctx.lineWidth = 2 * scale;
        ctx.lineCap = "round";
        for (let i = 0; i < p.leaves; i++) {
          const ox = (i - p.leaves / 2) * 2.4 * scale;
          const hh = p.h * (0.6 + 0.4 * Math.abs(Math.sin(i * 1.7 + p.phase)));
          ctx.beginPath();
          ctx.moveTo(p.x + ox, base + 2);
          ctx.quadraticCurveTo(p.x + ox + sway(p, 0.5) * 0.5, base - hh * 0.5, p.x + ox * 1.6 + sway(p, 1) + (i - p.leaves / 2) * 1.5, base - hh);
          ctx.stroke();
        }
      }
    };

    const drawFish = (f: Fish) => {
      const s = (f.size / 30) * scale * (0.75 + 0.25 * f.depth);
      const w = Math.sin(time * f.wag + f.phase) * 0.35;
      ctx.save();
      ctx.translate(f.x, f.y);
      ctx.scale(s * f.turn, s);
      ctx.globalAlpha = 0.72 + 0.28 * f.depth;
      drawFishShape(ctx, f.kind, w);
      ctx.restore();
      ctx.globalAlpha = 1;
    };

    const step = (dt: number) => {
      time += dt;
      for (const f of fish) {
        if (f.leader >= 0) {
          const l = fish[f.leader];
          f.dir = l.dir;
          f.x = l.x + f.dx * l.turn * scale + Math.sin(time * 1.3 + f.phase) * 3;
          f.y = l.y + f.dy * scale + Math.sin(time * 1.7 + f.phase) * 3;
          f.turn += (f.dir - f.turn) * Math.min(1, dt * 5);
          continue;
        }
        f.next -= dt;
        if (f.next <= 0) {
          f.next = 3 + Math.random() * 6;
          f.targetY = H * 0.15 + Math.random() * (floor - 30 - H * 0.15);
          if (f.x > 40 && f.x < W - 40 && Math.random() < 0.35) f.dir = -f.dir;
        }
        f.turn += (f.dir - f.turn) * Math.min(1, dt * 4);
        const swimming = Math.abs(f.turn) > 0.5 ? 1 : 0.3;
        f.x += f.dir * f.speed * scale * dt * swimming;
        f.baseY += (f.targetY - f.baseY) * Math.min(1, dt * 0.4);
        f.y = f.baseY + Math.sin(time * 0.9 + f.phase) * 4 * scale;
        if (f.x < -60 && f.dir < 0) {
          f.dir = 1;
          f.turn = 1;
        } else if (f.x > W + 60 && f.dir > 0) {
          f.dir = -1;
          f.turn = -1;
        }
      }
      for (let i = 0; i < stones.length; i++) {
        nextBubble[i] -= dt;
        if (nextBubble[i] <= 0) {
          nextBubble[i] = 0.06 + Math.random() * 0.22;
          const b = bubbles.find((q) => !q.alive);
          if (b) {
            b.alive = true;
            b.x = stones[i] + (Math.random() - 0.5) * 6 * scale;
            b.y = floor - 6 * scale;
            b.r = (1.4 + Math.random() * 3.2) * scale;
            b.vy = (55 + Math.random() * 55) * scale;
            b.phase = Math.random() * 6.28;
          }
        }
      }
      for (const b of bubbles) {
        if (!b.alive) continue;
        b.y -= b.vy * dt;
        b.vy *= 1 + dt * 0.25;
        if (b.y < -10) b.alive = false;
      }
    };

    const draw = () => {
      ctx.setTransform(k, 0, 0, k, 0, 0);
      ctx.clearRect(0, 0, W, H);
      // Light rays from the surface, drifting slowly.
      ctx.globalCompositeOperation = "lighter";
      for (let i = 0; i < 5; i++) {
        const cx = ((i + 0.5) / 5) * W + Math.sin(time * 0.07 + i * 1.9) * W * 0.06;
        const top = 30 * scale + i * 6;
        const bot = 90 * scale + (i % 2) * 40;
        const slant = H * 0.35;
        const g = ctx.createLinearGradient(0, 0, 0, floor);
        g.addColorStop(0, "rgba(190,240,255,0.1)");
        g.addColorStop(1, "rgba(190,240,255,0)");
        ctx.fillStyle = g;
        ctx.globalAlpha = 0.6 + 0.4 * Math.sin(time * 0.5 + i * 2.3);
        ctx.beginPath();
        ctx.moveTo(cx - top / 2, 0);
        ctx.lineTo(cx + top / 2, 0);
        ctx.lineTo(cx + slant + bot / 2, floor);
        ctx.lineTo(cx + slant - bot / 2, floor);
        ctx.closePath();
        ctx.fill();
      }
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
      for (const p of plants) if (p.layer === 0) drawPlant(p);
      for (const f of fish) if (f.depth < 0.5) drawFish(f);
      for (const p of plants) if (p.layer === 1) drawPlant(p);
      for (const f of fish) if (f.depth >= 0.5) drawFish(f);
      for (const p of plants) if (p.layer === 2) drawPlant(p);
      // Bubbles.
      for (const b of bubbles) {
        if (!b.alive) continue;
        const x = b.x + Math.sin(time * 6 + b.phase) * 2;
        ctx.globalAlpha = 0.85;
        ctx.strokeStyle = "rgba(230,250,255,0.8)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(x, b.y, b.r, 0, Math.PI * 2);
        ctx.stroke();
        ctx.fillStyle = "rgba(220,245,255,0.12)";
        ctx.fill();
        ctx.fillStyle = "rgba(255,255,255,0.9)";
        ctx.beginPath();
        ctx.arc(x - b.r * 0.35, b.y - b.r * 0.35, Math.max(0.5, b.r * 0.25), 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.globalAlpha = 1;
    };

    // Start with bubbles already rising.
    for (let i = 0; i < 60; i++) step(0.05);
    draw();
    if (!running || !animate) return;
    let last = performance.now();
    return fxLoop(fps, (now: number) => {
      step(Math.min(0.1, (now - last) / 1000));
      last = now;
      draw();
    });
  }, [running, animate, width, band, fps]);

  return (
    <div class={className} style={`height:${Math.round(band)}px`} aria-hidden="true">
      <canvas ref={backRef} />
      <canvas ref={frontRef} />
    </div>
  );
}

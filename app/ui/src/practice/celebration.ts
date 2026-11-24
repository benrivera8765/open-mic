// SPDX-License-Identifier: GPL-3.0-or-later
// Perfect-phrase celebrations over the practice lane (owner, 2026-11-22: "an out
// bursting of colour ... make it look cool for the camera"). Four looks:
//
//   starburst  a white flash, spinning rays of jukebox colour and a spray of sparks
//   fireworks  three or four shells bursting across the lane, with trails and gravity
//   rings      neon shock-rings racing out from the note, sparkles and a light sweep
//   bloom      big soft blooms of colour bursting outward and a shower of confetti
//   inferno    (2026-11-23, "a bunch of flames") a fireball at the note, flame jets
//              bursting out of it, a wall of fire racing along the foot of the lane,
//              pillars of fire shooting up and a shower of embers
//
// Each one also pops a gold "PERFECT!" over the note (a fiery one for the inferno).
// Flames are drawn from three pre-rendered sprites (white heat, orange, red), stretched
// upward and cross-faded as they cool, so hundreds of them cost a few blits each.
//
// Cost: its own canvas over the lane, drawn only while a burst is alive (about 1.6 s);
// otherwise no loop at all. A fixed pool of particles (no allocation per frame), additive
// blending for the glow instead of blur filters.

import type { CelebrateFreq, CelebrationKind, PitchFx } from "../api/types";

export type BurstKind = Exclude<CelebrationKind, "mix">;
export const BURST_KINDS: readonly BurstKind[] = ["starburst", "fireworks", "rings", "bloom", "inferno"];

/** "Now and then": at most one celebration in this long. */
export const RARE_GAP_MS = 30_000;
/** "More often": a phrase counts at this share of its graded time on pitch. */
export const NEAR_PERFECT_SHARE = 0.72;

/**
 * How often perfect phrases are celebrated (settings ui.look.celebrate_freq): every one,
 * every second or third, or at most once every 30 s. Ask `allow` once per perfect phrase.
 */
export class CelebrationGate {
  private count = 0;
  private lastMs = Number.NEGATIVE_INFINITY;

  allow(freq: CelebrateFreq, nowMs: number): boolean {
    this.count++;
    if (freq === "second") return this.count % 2 === 1;
    if (freq === "third") return this.count % 3 === 1;
    if (freq === "rare") {
      if (nowMs - this.lastMs < RARE_GAP_MS) return false;
      this.lastMs = nowMs;
    }
    return true;
  }
}

/** Jukebox neon colours (DESIGN.md palette) plus white and gold. */
const COLORS = ["#ff3d8b", "#ff8a1f", "#ffd23f", "#4dff6a", "#7dffd8", "#9b6bff", "#ff2a2a", "#ffffff", "#ffc21a"];

const MAX_PARTICLES = 900;
const MAX_SHAPES = 40;

const FLAME_STOPS: readonly (readonly [number, string][])[] = [
  // White heat.
  [
    [0, "rgba(255,255,236,1)"],
    [0.25, "rgba(255,238,160,0.92)"],
    [0.6, "rgba(255,168,56,0.45)"],
    [1, "rgba(255,100,20,0)"],
  ],
  // Orange.
  [
    [0, "rgba(255,206,96,0.95)"],
    [0.3, "rgba(255,138,31,0.8)"],
    [0.65, "rgba(238,66,10,0.35)"],
    [1, "rgba(200,30,0,0)"],
  ],
  // Red, cooling.
  [
    [0, "rgba(255,104,40,0.8)"],
    [0.4, "rgba(206,38,10,0.5)"],
    [1, "rgba(110,8,0,0)"],
  ],
];

/** The three flame sprites (64 px, the hot spot low so a stretched flame has a round base and a tip). */
function flameSprites(): HTMLCanvasElement[] | null {
  if (typeof document === "undefined") return null;
  const out: HTMLCanvasElement[] = [];
  for (const stops of FLAME_STOPS) {
    const c = document.createElement("canvas");
    c.width = 64;
    c.height = 64;
    const ctx = c.getContext("2d");
    if (!ctx) return null;
    const g = ctx.createRadialGradient(32, 40, 0, 32, 36, 31);
    for (const [o, col] of stops) g.addColorStop(o, col);
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, 64, 64);
    out.push(c);
  }
  return out;
}

const EMBER_COLORS = ["#ffd23f", "#ff8a1f", "#ffb02a", "#fff2c0", "#ff5a0a"];

const P = { Spark: 0, Confetti: 1, Ember: 2, Star: 3, Flame: 4, Glyph: 5 } as const;
type P = (typeof P)[keyof typeof P];

interface Particle {
  alive: boolean;
  kind: P;
  x: number;
  y: number;
  px: number;
  py: number;
  vx: number;
  vy: number;
  age: number;
  life: number;
  size: number;
  color: string;
  gravity: number;
  drag: number;
  rot: number;
  vr: number;
  /** Seconds to wait before appearing (staggered firework shells). */
  delay: number;
  /** Music-note glyph (P.Glyph). */
  ch: string;
}

const S = { Flash: 0, Rays: 1, Ring: 2, Bloom: 3, Sweep: 4, Word: 5, Halo: 6 } as const;
type S = (typeof S)[keyof typeof S];

interface Shape {
  alive: boolean;
  kind: S;
  x: number;
  y: number;
  /** Travel direction / spin for blooms and rays. */
  vx: number;
  vy: number;
  age: number;
  life: number;
  delay: number;
  size: number;
  color: string;
  spin: number;
}

function rand(a: number, b: number): number {
  return a + Math.random() * (b - a);
}

function pick<T>(list: readonly T[]): T {
  return list[Math.floor(Math.random() * list.length)];
}

function easeOut(t: number): number {
  const u = 1 - Math.min(1, Math.max(0, t));
  return 1 - u * u * u;
}

/** Elastic pop: overshoots to ~1.15 and settles at 1. */
function pop(t: number): number {
  if (t >= 1) return 1;
  return 1 - Math.cos(t * Math.PI * 2.4) * Math.exp(-t * 5.2) * 0.42;
}

export class CelebrationFx {
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D | null;
  private readonly parts: Particle[] = [];
  private readonly shapes: Shape[] = [];
  private w = 0;
  private h = 0;
  private dpr = 1;
  private raf = 0;
  private last = 0;
  private mixIndex = 0;
  private font = "Oswald, sans-serif";
  /** The theme's glow behind the PERFECT! word, "r, g, b" (read from the canvas's CSS). */
  private glowRgb = "255, 61, 139";
  private flames: HTMLCanvasElement[] | null | undefined;
  /** Bursts fired (read by the screenshot script). */
  fired = 0;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    for (let i = 0; i < MAX_PARTICLES; i++) {
      this.parts.push({ alive: false, kind: P.Spark, x: 0, y: 0, px: 0, py: 0, vx: 0, vy: 0, age: 0, life: 1, size: 1, color: "#fff", gravity: 0, drag: 0, rot: 0, vr: 0, delay: 0, ch: "" });
    }
    for (let i = 0; i < MAX_SHAPES; i++) {
      this.shapes.push({ alive: false, kind: S.Flash, x: 0, y: 0, vx: 0, vy: 0, age: 0, life: 1, delay: 0, size: 1, color: "#fff", spin: 0 });
    }
  }

  setSize(w: number, h: number, dpr: number, font?: string): void {
    this.w = w;
    this.h = h;
    this.dpr = Math.min(2, dpr || 1);
    if (font) this.font = font;
    try {
      const rgb = getComputedStyle(this.canvas).getPropertyValue("--jk-glow-rgb").trim();
      if (/^\d{1,3},\s*\d{1,3},\s*\d{1,3}$/.test(rgb)) this.glowRgb = rgb;
    } catch {
      // Not in a document (tests): keep the pink.
    }
    this.canvas.width = Math.max(1, Math.round(w * this.dpr));
    this.canvas.height = Math.max(1, Math.round(h * this.dpr));
  }

  /** Stops the loop and clears (window hidden, practice ended). */
  stop(): void {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
    for (const p of this.parts) p.alive = false;
    for (const s of this.shapes) s.alive = false;
    this.ctx?.clearRect(0, 0, this.canvas.width, this.canvas.height);
  }

  /** Fires one celebration at (x, y) in lane CSS px. "mix" takes the four in turn. */
  burst(kind: CelebrationKind, x: number, y: number): BurstKind {
    const k: BurstKind = kind === "mix" ? BURST_KINDS[this.mixIndex++ % BURST_KINDS.length] : kind;
    this.fired++;
    if (k === "starburst") this.starburst(x, y);
    else if (k === "fireworks") this.fireworks(x, y);
    else if (k === "rings") this.rings(x, y);
    else if (k === "inferno") this.inferno(x, y);
    else this.bloom(x, y);
    this.shape(S.Word, x, y, 0, 0, k === "inferno" ? 1.5 : 1.25, 0, Math.max(28, Math.min(this.h * (k === "inferno" ? 0.3 : 0.26), 104)), k === "inferno" ? "fire" : "#ffd23f", 0);
    if (!this.raf) {
      this.last = performance.now();
      this.raf = requestAnimationFrame(this.frame);
    }
    return k;
  }

  private trailAcc = 0;
  private haloClock = 0;
  private emberAcc = 0;
  private glowClock = 0;
  private flareClock = 0.4;

  /**
   * The on-pitch effect (owner, 2026-11-23: "sparks ... should emanate from the cursor where
   * your pitch is"): call on every lane frame while the singer is on pitch, with the
   * cursor's position, the seconds since the last call and the bar height (CSS px).
   */
  trail(kind: PitchFx, x: number, y: number, dt: number, h: number): void {
    if (kind === "off" || dt <= 0 || dt > 0.2) return;
    const u = Math.max(6, h);
    if (kind === "flames") {
      this.flameTrail(x, y, dt, u);
    } else if (kind === "rings") {
      this.haloClock -= dt;
      if (this.haloClock <= 0) {
        this.haloClock = 0.17;
        this.shape(S.Halo, x, y, 0, 0, 0.55, 0, u * 1.6, Math.random() < 0.5 ? "#7dffd8" : "#ffd23f", 0);
      }
    } else {
      const rate = kind === "notes" ? 6 : kind === "stars" ? 18 : 70;
      this.trailAcc += rate * dt;
      while (this.trailAcc >= 1) {
        this.trailAcc -= 1;
        const jy = y + rand(-0.25, 0.25) * u;
        if (kind === "sparks") {
          this.particle(P.Spark, x, jy, -rand(90, 320), rand(-220, 220), rand(0.3, 0.65), rand(0.16, 0.28) * u, pick(["#ffd23f", "#ffffff", "#7dffd8", "#ffc21a", "#ff8a1f"]), 320, 2.0);
        } else if (kind === "stars") {
          this.particle(P.Star, x, jy, -rand(20, 130), rand(-120, 120), rand(0.45, 0.85), rand(0.45, 0.8) * u, pick(["#ffd23f", "#ffffff", "#ff9ad5", "#7dffd8"]), 0, 1.6);
        } else {
          const p = this.particle(P.Glyph, x, y - u * 0.3, -rand(25, 70), -rand(35, 80), rand(1.0, 1.4), rand(1.0, 1.4) * u, pick(["#7dffd8", "#ff9ad5", "#ffd23f", "#c9b4ff"]), -10, 0.6);
          if (p) p.ch = pick(["\u266a", "\u266b", "\u266c"]);
        }
      }
    }
    if (!this.raf) {
      this.last = performance.now();
      this.raf = requestAnimationFrame(this.frame);
    }
  }

  /**
   * The on-pitch flames (owner, 2026-11-23: "bigger ... extravagant"): a plume of flame
   * pouring off the cursor and streaming back along the notes, a glowing fireball on the
   * cursor, embers flying off it, and now and then a flare of flame bursting out round it.
   */
  private flameTrail(x: number, y: number, dt: number, u: number): void {
    const B = Math.max(16, u * 1.7);
    this.trailAcc += 95 * dt;
    while (this.trailAcc >= 1) {
      this.trailAcc -= 1;
      this.particle(P.Flame, x + rand(-0.2, 0.2) * B, y + rand(-0.25, 0.25) * B, -rand(40, 210), -rand(60, 230), rand(0.4, 0.85), rand(0.9, 1.8) * B, "", -320, 1.4);
    }
    this.emberAcc += 32 * dt;
    while (this.emberAcc >= 1) {
      this.emberAcc -= 1;
      this.particle(P.Ember, x, y + rand(-0.3, 0.3) * B, -rand(60, 330), -rand(120, 430), rand(0.6, 1.2), rand(1.2, 2.4), pick(EMBER_COLORS), -120, 1.2);
    }
    this.glowClock -= dt;
    if (this.glowClock <= 0) {
      this.glowClock = 0.07;
      this.shape(S.Flash, x, y, 0, 0, 0.22, 0, B * 2.6, "#ff8a1f", 0);
    }
    this.flareClock -= dt;
    if (this.flareClock <= 0) {
      this.flareClock = rand(0.55, 0.9);
      for (let i = 0; i < 14; i++) {
        const a = rand(0, Math.PI * 2);
        const v = rand(140, 340);
        this.particle(P.Flame, x, y, Math.cos(a) * v - 60, Math.sin(a) * v, rand(0.35, 0.6), rand(0.7, 1.2) * B, "", -200, 2.4);
      }
      this.shape(S.Halo, x, y, 0, 0, 0.45, 0, B * 3, "#ffb02a", 0);
    }
  }

  private particle(kind: P, x: number, y: number, vx: number, vy: number, life: number, size: number, color: string, gravity: number, drag: number, delay = 0): Particle | null {
    const p = this.parts.find((q) => !q.alive);
    if (!p) return null;
    p.alive = true;
    p.kind = kind;
    p.x = p.px = x;
    p.y = p.py = y;
    p.vx = vx;
    p.vy = vy;
    p.age = 0;
    p.life = life;
    p.size = size;
    p.color = color;
    p.gravity = gravity;
    p.drag = drag;
    p.rot = rand(0, Math.PI * 2);
    p.vr = rand(-9, 9);
    p.delay = delay;
    return p;
  }

  private shape(kind: S, x: number, y: number, vx: number, vy: number, life: number, delay: number, size: number, color: string, spin: number): void {
    const s = this.shapes.find((q) => !q.alive);
    if (!s) return;
    s.alive = true;
    s.kind = kind;
    s.x = x;
    s.y = y;
    s.vx = vx;
    s.vy = vy;
    s.age = 0;
    s.life = life;
    s.delay = delay;
    s.size = size;
    s.color = color;
    s.spin = spin;
  }

  private starburst(x: number, y: number): void {
    const R = Math.max(this.w, this.h);
    this.shape(S.Flash, x, y, 0, 0, 0.45, 0, this.h * 0.95, "#ffffff", 0);
    this.shape(S.Rays, x, y, 0, 0, 1.1, 0, R * 0.62, "", rand(-1, 1) > 0 ? 0.9 : -0.9);
    for (let i = 0; i < 90; i++) {
      const a = rand(0, Math.PI * 2);
      const v = rand(260, 980);
      this.particle(P.Spark, x, y, Math.cos(a) * v, Math.sin(a) * v, rand(0.55, 1.15), rand(1.4, 3.4), pick(COLORS), 120, 2.4);
    }
  }

  private fireworks(x: number, y: number): void {
    const shells = 4;
    for (let s = 0; s < shells; s++) {
      const sx = s === 0 ? x : rand(this.w * 0.2, this.w * 0.95);
      const sy = s === 0 ? y : rand(this.h * 0.15, this.h * 0.6);
      const delay = s * 0.14;
      const c1 = pick(COLORS);
      const c2 = pick(COLORS);
      const n = 96;
      const v0 = rand(380, 560) * Math.max(0.75, this.h / 400);
      this.shape(S.Flash, sx, sy, 0, 0, 0.42, delay, this.h * 0.55, c1, 0);
      for (let i = 0; i < n; i++) {
        const a = (i / n) * Math.PI * 2 + rand(-0.05, 0.05);
        // Two shells in one: a fast outer ring and a slower inner one.
        const v = v0 * (i % 2 ? rand(0.9, 1.08) : rand(0.45, 0.62));
        this.particle(P.Ember, sx, sy, Math.cos(a) * v, Math.sin(a) * v, rand(1.05, 1.55), rand(2.6, 4.2), i % 3 === 0 ? c2 : c1, 320, 1.6, delay);
      }
    }
  }

  private rings(x: number, y: number): void {
    const R = Math.hypot(Math.max(x, this.w - x), Math.max(y, this.h - y));
    const ringColors = ["#ff3d8b", "#ff8a1f", "#ffd23f", "#7dffd8"];
    ringColors.forEach((c, i) => this.shape(S.Ring, x, y, 0, 0, 1.05, i * 0.085, R * 1.05, c, 0));
    this.shape(S.Flash, x, y, 0, 0, 0.35, 0, this.h * 0.6, "#ffffff", 0);
    this.shape(S.Sweep, x, y, 0, 0, 0.9, 0.05, this.w, "#ffffff", 0);
    for (let i = 0; i < 70; i++) {
      const a = rand(0, Math.PI * 2);
      const v = rand(420, 720);
      this.particle(P.Spark, x, y, Math.cos(a) * v, Math.sin(a) * v, rand(0.5, 0.9), rand(1.2, 2.6), pick(ringColors), 0, 2.8);
    }
  }

  private bloom(x: number, y: number): void {
    const n = 7;
    for (let i = 0; i < n; i++) {
      const a = (i / n) * Math.PI * 2 + rand(-0.3, 0.3);
      const v = rand(0.25, 0.55) * Math.max(this.w * 0.3, this.h);
      this.shape(S.Bloom, x, y, Math.cos(a) * v, Math.sin(a) * v * 0.55, rand(1.1, 1.5), i * 0.03, this.h * rand(0.32, 0.5), COLORS[i % 6], 0);
    }
    this.shape(S.Flash, x, y, 0, 0, 0.4, 0, this.h * 0.7, "#fff4d6", 0);
    for (let i = 0; i < 110; i++) {
      const a = rand(-Math.PI * 0.95, -Math.PI * 0.05);
      const v = rand(300, 760);
      this.particle(P.Confetti, x, y, Math.cos(a) * v * 1.3, Math.sin(a) * v, rand(1.2, 1.7), rand(4, 8), pick(COLORS), 680, 1.5);
    }
  }

  private inferno(x: number, y: number): void {
    const W = this.w;
    const H = this.h;
    const k = Math.max(0.75, H / 400);
    // The fireball at the note and a shock of heat racing out of it.
    this.shape(S.Flash, x, y, 0, 0, 0.55, 0, H * 0.95, "#ff9a2a", 0);
    this.shape(S.Flash, x, y, 0, 0, 0.32, 0, H * 0.45, "#fff2c0", 0);
    this.shape(S.Ring, x, y, 0, 0, 0.85, 0, Math.max(W, H) * 0.6, "#ff6a1a", 0);
    // Flame jets bursting out of the note.
    for (let i = 0; i < 70; i++) {
      const a = rand(0, Math.PI * 2);
      const v = rand(150, 620) * k;
      this.particle(P.Flame, x, y, Math.cos(a) * v, Math.sin(a) * v * 0.8, rand(0.6, 1.1), rand(0.1, 0.2) * H, "", -260, 2.6, rand(0, 0.08));
    }
    // A wall of fire rising along the foot of the lane, racing out from under the note.
    for (let i = 0; i < 110; i++) {
      const fx = rand(0, W);
      const delay = (Math.abs(fx - x) / Math.max(1, W)) * 0.45 + rand(0, 0.25);
      this.particle(P.Flame, fx, H * rand(1.0, 1.05), rand(-30, 30), -rand(260, 620) * k, rand(0.55, 1.0), rand(0.14, 0.3) * H, "", -180, 1.2, delay);
    }
    // Pillars of fire shooting up.
    for (let c = 0; c < 4; c++) {
      const px = rand(W * 0.08, W * 0.92);
      for (let i = 0; i < 24; i++) {
        this.particle(P.Flame, px + rand(-0.015, 0.015) * W, H, rand(-20, 20), -rand(500, 900) * k, rand(0.5, 0.9), rand(0.1, 0.2) * H, "", -200, 0.8, 0.12 + c * 0.08 + i * 0.025);
      }
    }
    // A shower of embers.
    for (let i = 0; i < 140; i++) {
      this.particle(P.Ember, rand(0, W), H * rand(0.6, 1.05), rand(-80, 80), -rand(200, 700) * k, rand(0.9, 1.7), rand(1.4, 3.0), pick(EMBER_COLORS), -60, 1.0, rand(0, 0.6));
    }
    // Heat glowing up from the foot of the lane.
    this.shape(S.Bloom, x, H, 0, -H * 0.2, 1.4, 0.05, H * 0.9, "#ff4a0a", 0);
  }

  private readonly frame = (now: number): void => {
    this.raf = 0;
    const ctx = this.ctx;
    if (!ctx) return;
    const dt = Math.min(0.05, Math.max(0, (now - this.last) / 1000));
    this.last = now;
    const d = this.dpr;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    ctx.setTransform(d, 0, 0, d, 0, 0);
    let alive = 0;

    ctx.globalCompositeOperation = "lighter";
    for (const s of this.shapes) {
      if (!s.alive) continue;
      s.age += dt;
      if (s.age < s.delay) {
        alive++;
        continue;
      }
      const t = (s.age - s.delay) / s.life;
      if (t >= 1) {
        s.alive = false;
        continue;
      }
      alive++;
      if (s.kind === S.Word) continue; // drawn last, on top, in normal blending
      this.drawShape(ctx, s, t);
    }

    for (const p of this.parts) {
      if (!p.alive) continue;
      p.age += dt;
      if (p.age < p.delay) {
        alive++;
        continue;
      }
      const t = (p.age - p.delay) / p.life;
      if (t >= 1) {
        p.alive = false;
        continue;
      }
      alive++;
      p.px = p.x;
      p.py = p.y;
      const k = Math.exp(-p.drag * dt);
      p.vx *= k;
      p.vy = p.vy * k + p.gravity * dt;
      p.x += p.vx * dt;
      p.y += p.vy * dt;
      p.rot += p.vr * dt;
      this.drawParticle(ctx, p, t);
    }

    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    for (const s of this.shapes) {
      if (!s.alive || s.kind !== S.Word || s.age < s.delay) continue;
      this.drawWord(ctx, s, (s.age - s.delay) / s.life);
    }
    ctx.globalAlpha = 1;

    if (alive > 0) this.raf = requestAnimationFrame(this.frame);
    else {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    }
  };

  private drawShape(ctx: CanvasRenderingContext2D, s: Shape, t: number): void {
    if (s.kind === S.Flash) {
      const r = s.size * (0.35 + 0.65 * easeOut(t));
      const g = ctx.createRadialGradient(s.x, s.y, 0, s.x, s.y, r);
      g.addColorStop(0, s.color);
      g.addColorStop(0.25, s.color);
      g.addColorStop(1, "rgba(255,255,255,0)");
      ctx.globalAlpha = (1 - t) * 0.85;
      ctx.fillStyle = g;
      ctx.fillRect(s.x - r, s.y - r, r * 2, r * 2);
    } else if (s.kind === S.Rays) {
      const n = 18;
      const len = s.size * easeOut(Math.min(1, t * 1.6));
      const turn = s.spin * t;
      ctx.globalAlpha = Math.min(1, (1 - t) * 1.4) * 0.6;
      for (let i = 0; i < n; i++) {
        const a = (i / n) * Math.PI * 2 + turn;
        const half = 0.07;
        const g = ctx.createLinearGradient(s.x, s.y, s.x + Math.cos(a) * len, s.y + Math.sin(a) * len);
        const c = COLORS[i % 6];
        g.addColorStop(0, "rgba(255,255,255,0.9)");
        g.addColorStop(0.18, c);
        g.addColorStop(1, "rgba(0,0,0,0)");
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.moveTo(s.x, s.y);
        ctx.lineTo(s.x + Math.cos(a - half) * len, s.y + Math.sin(a - half) * len);
        ctx.lineTo(s.x + Math.cos(a + half) * len, s.y + Math.sin(a + half) * len);
        ctx.closePath();
        ctx.fill();
      }
    } else if (s.kind === S.Ring) {
      const r = s.size * easeOut(t);
      const fade = 1 - t;
      ctx.strokeStyle = s.color;
      ctx.globalAlpha = fade * 0.22;
      ctx.lineWidth = 18 * (1 - t * 0.6);
      ctx.beginPath();
      ctx.arc(s.x, s.y, r, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = fade * 0.95;
      ctx.lineWidth = 4 * (1 - t * 0.5);
      ctx.stroke();
      ctx.strokeStyle = "#ffffff";
      ctx.globalAlpha = fade * 0.7;
      ctx.lineWidth = 1.2;
      ctx.stroke();
    } else if (s.kind === S.Bloom) {
      const e = easeOut(t);
      const cx = s.x + s.vx * e;
      const cy = s.y + s.vy * e;
      const r = s.size * (0.3 + 0.9 * e);
      const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r);
      g.addColorStop(0, s.color);
      g.addColorStop(0.45, s.color);
      g.addColorStop(1, "rgba(0,0,0,0)");
      ctx.globalAlpha = Math.min(1, (1 - t) * 1.6) * 0.55;
      ctx.fillStyle = g;
      ctx.fillRect(cx - r, cy - r, r * 2, r * 2);
    } else if (s.kind === S.Halo) {
      // A soft ring pulsing out of the cursor (on-pitch effect).
      const r = s.size * (0.25 + 0.75 * easeOut(t));
      ctx.strokeStyle = s.color;
      ctx.globalAlpha = (1 - t) * 0.35;
      ctx.lineWidth = 5;
      ctx.beginPath();
      ctx.arc(s.x, s.y, r, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = (1 - t) * 0.9;
      ctx.lineWidth = 1.6;
      ctx.stroke();
    } else if (s.kind === S.Sweep) {
      // A band of light racing across the lane at the note's height.
      const x = -this.w * 0.2 + (this.w * 1.4) * easeOut(t);
      const bw = this.w * 0.16;
      const g = ctx.createLinearGradient(x - bw, 0, x + bw, 0);
      g.addColorStop(0, "rgba(255,255,255,0)");
      g.addColorStop(0.5, "rgba(255,240,200,0.55)");
      g.addColorStop(1, "rgba(255,255,255,0)");
      ctx.globalAlpha = 1 - t;
      ctx.fillStyle = g;
      ctx.fillRect(x - bw, 0, bw * 2, this.h);
    }
  }

  private drawParticle(ctx: CanvasRenderingContext2D, p: Particle, t: number): void {
    const fade = 1 - t * t;
    if (p.kind === P.Confetti) {
      ctx.globalCompositeOperation = "source-over";
      ctx.globalAlpha = Math.min(1, fade * 1.3);
      ctx.fillStyle = p.color;
      const flip = Math.cos(p.rot * 1.7);
      ctx.save();
      ctx.translate(p.x, p.y);
      ctx.rotate(p.rot);
      ctx.fillRect(-p.size / 2, (-p.size * 0.32) * flip, p.size, p.size * 0.64 * flip);
      ctx.restore();
      ctx.globalCompositeOperation = "lighter";
      return;
    }
    if (p.kind === P.Star) {
      // A four-point star, turning and twinkling.
      const r = p.size * (1 - t * 0.5);
      const k = r * 0.3;
      ctx.globalAlpha = fade * (0.6 + 0.4 * Math.sin(p.age * 30 + p.rot * 5));
      ctx.fillStyle = p.color;
      ctx.save();
      ctx.translate(p.x, p.y);
      ctx.rotate(p.rot);
      ctx.beginPath();
      ctx.moveTo(0, -r);
      ctx.lineTo(k, -k);
      ctx.lineTo(r, 0);
      ctx.lineTo(k, k);
      ctx.lineTo(0, r);
      ctx.lineTo(-k, k);
      ctx.lineTo(-r, 0);
      ctx.lineTo(-k, -k);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
      return;
    }
    if (p.kind === P.Flame) {
      if (this.flames === undefined) this.flames = flameSprites();
      const spr = this.flames;
      if (spr) {
        // A flame tongue stretched upward and flickering: white heat, orange, then red as it cools.
        const r = p.size * (t < 0.2 ? 0.6 + 2 * t : 1 - (t - 0.2) * 0.55);
        const hgt = r * (1.7 + 0.5 * Math.sin(p.age * 14 + p.rot * 3));
        const fx = p.x + Math.sin(p.age * 9 + p.rot) * r * 0.08 - r / 2;
        const fy = p.y - hgt * 0.62;
        const a = Math.min(2, t * 3);
        const i = Math.floor(a);
        const f = a - i;
        ctx.globalAlpha = fade * (1 - f);
        ctx.drawImage(spr[i], fx, fy, r, hgt);
        ctx.globalAlpha = fade * f;
        if (f > 0.01) ctx.drawImage(spr[Math.min(2, i + 1)], fx, fy, r, hgt);
        return;
      }
      // No sprites (no document): a round gradient.
      const r = p.size * (1 - t * 0.6);
      const g = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, r);
      g.addColorStop(0, t < 0.35 ? "rgba(255,245,200,0.95)" : "rgba(255,190,80,0.9)");
      g.addColorStop(0.5, t < 0.6 ? "rgba(255,138,31,0.7)" : "rgba(255,70,40,0.6)");
      g.addColorStop(1, "rgba(255,40,30,0)");
      ctx.globalAlpha = fade;
      ctx.fillStyle = g;
      ctx.fillRect(p.x - r, p.y - r, r * 2, r * 2);
      return;
    }
    if (p.kind === P.Glyph) {
      ctx.globalCompositeOperation = "source-over";
      ctx.globalAlpha = Math.min(1, fade * 1.2);
      ctx.fillStyle = p.color;
      ctx.font = `700 ${p.size.toFixed(1)}px "Segoe UI Symbol", "Apple Symbols", sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(p.ch, p.x, p.y + Math.sin(p.age * 5 + p.rot) * 2);
      ctx.globalCompositeOperation = "lighter";
      return;
    }
    // Sparks and embers: a bright streak along the motion (the trail), then a dot.
    const twinkle = p.kind === P.Ember && t > 0.6 ? (Math.sin(p.age * 60 + p.rot * 10) > 0 ? 1 : 0.35) : 1;
    ctx.globalAlpha = fade * twinkle;
    ctx.strokeStyle = p.color;
    ctx.lineCap = "round";
    ctx.lineWidth = p.size;
    ctx.beginPath();
    const trail = p.kind === P.Ember ? 5 : 3;
    const tx = p.x - (p.x - p.px) * trail;
    const ty = p.y - (p.y - p.py) * trail;
    ctx.moveTo(tx, ty);
    ctx.lineTo(p.x, p.y);
    ctx.stroke();
    ctx.globalAlpha = fade * twinkle * 0.9;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(p.x, p.y, p.size * 0.45, 0, Math.PI * 2);
    ctx.fill();
  }

  private drawWord(ctx: CanvasRenderingContext2D, s: Shape, t: number): void {
    const size = s.size * pop(Math.min(1, t * 2.2));
    const alpha = t < 0.7 ? 1 : 1 - (t - 0.7) / 0.3;
    const y = Math.max(size * 0.6, Math.min(this.h - size * 0.4, s.y - size * 0.7 - t * size * 0.35));
    const x = Math.max(size * 2.2, Math.min(this.w - size * 2.2, s.x));
    ctx.save();
    ctx.globalAlpha = Math.max(0, alpha);
    ctx.font = `700 ${size.toFixed(1)}px ${this.font}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    const fire = s.color === "fire";
    ctx.shadowColor = fire ? "rgba(255, 90, 10, 0.95)" : `rgba(${this.glowRgb}, 0.9)`;
    ctx.shadowBlur = size * (fire ? 0.5 : 0.35);
    ctx.lineJoin = "round";
    ctx.lineWidth = Math.max(2, size * 0.08);
    ctx.strokeStyle = fire ? "#3a0800" : "#3a1400";
    ctx.strokeText("PERFECT!", x, y);
    ctx.shadowBlur = 0;
    const g = ctx.createLinearGradient(0, y - size * 0.5, 0, y + size * 0.5);
    if (fire) {
      // White heat on top, through gold and orange to red at the foot.
      g.addColorStop(0, "#fffbe0");
      g.addColorStop(0.3, "#ffe27a");
      g.addColorStop(0.55, "#ffa024");
      g.addColorStop(0.78, "#ff5a0a");
      g.addColorStop(1, "#c02a04");
    } else {
      g.addColorStop(0, "#fffbe6");
      g.addColorStop(0.35, "#ffd77a");
      g.addColorStop(0.52, "#c08a26");
      g.addColorStop(0.62, "#f2c25a");
      g.addColorStop(1, "#fff1bd");
    }
    ctx.fillStyle = g;
    ctx.fillText("PERFECT!", x, y);
    ctx.restore();
  }
}

/** Phrase ends: notes followed by at least this much silence (or the last note) close a phrase. */
export const PHRASE_GAP_S = 0.45;
/** A real singer's phrase counts as perfect at this share of its graded time on pitch. */
export const PERFECT_SHARE = 0.88;

// SPDX-License-Identifier: GPL-3.0-or-later
// Bubble simulation for the bubble tubes (docs/DESIGN.md, "Bubbles").
//
// Bubbles rise along a TubePath (up a column, round the shoulder, over the arch
// to the crest). They vary in size and speed, sometimes come in small clusters,
// grow a little and speed up as they rise (as real bubbles do when the pressure
// drops), and wobble sideways inside the narrow inner tube.
//
// No DOM here and no allocation per step: a fixed pool of bubble records is
// reused, so the animation never makes garbage for the collector to pause on.
// Deterministic for a given seed (unit-tested in bubbles-engine.test.ts).

import type { PathPoint, TubePath } from "./geometry";

/** Hard cap from the work order: at most ~60 bubbles on screen. */
export const MAX_BUBBLES = 60;

export interface Bubble {
  alive: boolean;
  /** Which path (0 = left, 1 = right, ...). */
  path: number;
  /** Arc length travelled (CSS px). May start slightly negative (hidden in the base). */
  s: number;
  /** Base speed (CSS px / s). */
  v0: number;
  /** Radius at the bottom (CSS px). */
  r0: number;
  /** Side-to-side wobble: amplitude as a fraction of the free space, angular speed, phase. */
  wobble: number;
  omega: number;
  phase: number;
  /** Seconds since spawn. */
  age: number;
}

export interface BubbleSprite {
  x: number;
  y: number;
  r: number;
  alpha: number;
}

/** Small, fast, seedable PRNG (mulberry32). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface BubbleFieldOptions {
  /** Upper bound on live bubbles across all paths (clamped to MAX_BUBBLES). */
  maxBubbles?: number;
  /** PRNG seed (tests). */
  seed?: number;
  /** Size scale: 1 at a 900 px tall window. Speeds and radii scale with it. */
  scale?: number;
}

export class BubbleField {
  private paths: TubePath[] = [];
  /** Inner bubble-tube half width (CSS px). */
  private halfWidth = 4;
  private readonly pool: Bubble[];
  private readonly rand: () => number;
  private readonly maxTotal: number;
  private scale: number;
  /** Seconds until the next spawn, per path. */
  private nextSpawn: number[] = [];
  private readonly tmp: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
  private readonly sprite: BubbleSprite = { x: 0, y: 0, r: 0, alpha: 0 };
  private time = 0;

  constructor(options: BubbleFieldOptions = {}) {
    this.maxTotal = Math.max(0, Math.min(MAX_BUBBLES, options.maxBubbles ?? MAX_BUBBLES));
    this.rand = mulberry32(options.seed ?? 0x5eed);
    this.scale = options.scale ?? 1;
    this.pool = [];
    for (let i = 0; i < this.maxTotal; i++) {
      this.pool.push({ alive: false, path: 0, s: 0, v0: 0, r0: 0, wobble: 0, omega: 0, phase: 0, age: 0 });
    }
  }

  /**
   * (Re)attaches the field to new paths, e.g. after a window resize. Live bubbles
   * keep their relative position along their path, so a resize does not reset them.
   */
  setPaths(paths: TubePath[], halfWidth: number, scale = this.scale): void {
    const old = this.paths;
    this.scale = scale;
    this.halfWidth = Math.max(1, halfWidth);
    for (const b of this.pool) {
      if (!b.alive) continue;
      const before = old[b.path];
      const after = paths[b.path];
      if (!before || !after || before.length <= 0) {
        b.alive = false;
        continue;
      }
      b.s = (b.s / before.length) * after.length;
    }
    this.paths = paths;
    while (this.nextSpawn.length < paths.length) this.nextSpawn.push(this.rand() * 0.8);
    this.nextSpawn.length = paths.length;
  }

  /** Removes every bubble (e.g. when the tubes change shape). */
  clear(): void {
    for (const b of this.pool) b.alive = false;
  }

  /** Number of live bubbles. */
  get count(): number {
    let n = 0;
    for (const b of this.pool) if (b.alive) n++;
    return n;
  }

  /** Average seconds a bubble needs to cross a path (for the spawn rate). */
  private travelTime(path: TubePath): number {
    const v = 52 * this.scale * 1.18; // mean base speed x mean acceleration factor
    return path.length / Math.max(1, v);
  }

  private spawn(path: number, sOffset: number, template: Bubble | null): Bubble | null {
    let b: Bubble | null = null;
    for (const p of this.pool) {
      if (!p.alive) {
        b = p;
        break;
      }
    }
    if (!b) return null;
    const hw = this.halfWidth;
    b.alive = true;
    b.path = path;
    b.s = -sOffset;
    b.age = 0;
    if (template) {
      // Cluster member: similar speed and size to its leader, so the group holds together.
      b.v0 = template.v0 * (0.97 + this.rand() * 0.06);
      b.r0 = Math.min(hw * 0.78, template.r0 * (0.75 + this.rand() * 0.5));
    } else {
      b.v0 = (34 + this.rand() * 36) * this.scale;
      // Mostly small bubbles, a few big ones (squared distribution).
      const u = this.rand();
      b.r0 = hw * (0.36 + 0.44 * u * u);
    }
    b.wobble = 0.25 + this.rand() * 0.6;
    b.omega = 2 + this.rand() * 4;
    b.phase = this.rand() * Math.PI * 2;
    return b;
  }

  /**
   * Runs the simulation forward without drawing, so the tubes are already full of
   * bubbles when they first appear (otherwise the first bubbles need ~20 s to
   * reach the arch).
   */
  prewarm(seconds: number): void {
    for (let t = 0; t < seconds; t += 0.1) this.step(0.1);
  }

  /** Advances the simulation by `dt` seconds (clamped to 0.1 s). */
  step(dt: number): void {
    const t = dt > 0.1 ? 0.1 : dt < 0 ? 0 : dt;
    this.time += t;
    const paths = this.paths;
    if (paths.length === 0 || this.maxTotal === 0) return;
    const perPath = Math.max(1, Math.floor(this.maxTotal / paths.length));

    // Move.
    for (const b of this.pool) {
      if (!b.alive) continue;
      const path = paths[b.path];
      const L = path.length;
      const f = L > 0 ? Math.max(0, b.s) / L : 1;
      // Speed up gently as the bubble rises (smoothstep 1.0 -> 1.45).
      const accel = 1 + 0.45 * f * f * (3 - 2 * f);
      b.s += b.v0 * accel * t;
      b.age += t;
      if (b.s >= L) b.alive = false;
    }

    // Spawn: a Poisson process per path whose rate keeps about 85 % of the
    // per-path budget on screen; 1 in 6 spawns is a cluster of 3 to 5.
    for (let p = 0; p < paths.length; p++) {
      this.nextSpawn[p] -= t;
      let guard = 0;
      while (this.nextSpawn[p] <= 0 && guard++ < 8) {
        let live = 0;
        for (const b of this.pool) if (b.alive && b.path === p) live++;
        const rate = (perPath * 0.85) / this.travelTime(paths[p]);
        this.nextSpawn[p] += -Math.log(1 - this.rand() * 0.999) / Math.max(0.05, rate);
        if (live >= perPath) continue;
        const leader = this.spawn(p, this.rand() * 4, null);
        if (!leader) continue;
        if (this.rand() < 1 / 6) {
          const extra = 2 + Math.floor(this.rand() * 3);
          let offset = 0;
          for (let k = 0; k < extra && live + 1 + k < perPath; k++) {
            offset += leader.r0 * (2.2 + this.rand() * 1.6);
            this.spawn(p, offset, leader);
          }
        }
      }
    }
  }

  /**
   * Calls `draw` for every visible bubble with its position, radius and opacity.
   * The sprite object is reused between calls: copy what you keep.
   */
  forEach(draw: (b: BubbleSprite) => void): void {
    const out = this.sprite;
    const p = this.tmp;
    const hw = this.halfWidth;
    for (const b of this.pool) {
      if (!b.alive || b.s < 0) continue;
      const path = this.paths[b.path];
      if (!path) continue;
      const L = path.length;
      const f = L > 0 ? b.s / L : 1;
      const r = Math.min(hw * 0.9, b.r0 * (1 + 0.3 * f));
      const room = Math.max(0, hw - r);
      const lateral = room * b.wobble * Math.sin(b.phase + b.omega * b.age);
      path.at(b.s, p);
      out.x = p.x + p.nx * lateral;
      out.y = p.y + p.ny * lateral;
      out.r = r;
      // Fade in over the first 4 % (they emerge from the base) and out over the
      // last 6 % (they vanish under the crest).
      const fadeIn = f < 0.04 ? f / 0.04 : 1;
      const fadeOut = f > 0.94 ? (1 - f) / 0.06 : 1;
      out.alpha = Math.max(0, Math.min(1, fadeIn * fadeOut));
      draw(out);
    }
  }
}

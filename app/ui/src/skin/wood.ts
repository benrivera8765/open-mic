// SPDX-License-Identifier: GPL-3.0-or-later
// Walnut for the cabinet face, generated once in code (no image files).
//
// A seamless tile of horizontal grain: long stretched value noise for the figure,
// a few wavy growth lines, and fine pores, mapped onto the walnut colours from
// docs/DESIGN.md. Rendered once into a canvas and handed to CSS as a JPEG data URL,
// so every later repaint is a plain bitmap blit. (An SVG feTurbulence background
// would be re-filtered on every repaint by engines that do not composite the
// drifting light separately; measured in Playwright's WebKit for Windows.)

const TILE_W = 640;
const TILE_H = 320;

/** Integer hash -> [0, 1). */
function hash(x: number, y: number, seed: number): number {
  let h = (x * 374761393 + y * 668265263 + seed * 1442695041) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h ^= h >>> 16;
  return (h >>> 0) / 4294967296;
}

/** Smooth value noise on a lattice that wraps every `px` x `py` cells (tileable). */
function noise(x: number, y: number, px: number, py: number, seed: number): number {
  const xi = Math.floor(x);
  const yi = Math.floor(y);
  const fx = x - xi;
  const fy = y - yi;
  const sx = fx * fx * (3 - 2 * fx);
  const sy = fy * fy * (3 - 2 * fy);
  const x0 = ((xi % px) + px) % px;
  const y0 = ((yi % py) + py) % py;
  const x1 = (x0 + 1) % px;
  const y1 = (y0 + 1) % py;
  const a = hash(x0, y0, seed);
  const b = hash(x1, y0, seed);
  const c = hash(x0, y1, seed);
  const d = hash(x1, y1, seed);
  return a + (b - a) * sx + (c - a) * sy + (a - b - c + d) * sx * sy;
}

/** Fractal noise, `cx` x `cy` lattice cells across the tile at the base octave. */
function fbm(u: number, v: number, cx: number, cy: number, octaves: number, seed: number): number {
  let sum = 0;
  let amp = 0.5;
  let norm = 0;
  for (let o = 0; o < octaves; o++) {
    const k = 1 << o;
    sum += amp * noise(u * cx * k, v * cy * k, cx * k, cy * k, seed + o);
    norm += amp;
    amp *= 0.5;
  }
  return sum / norm;
}

const cache: Record<string, string> = {};

/** Wood colours, dark to light (DESIGN.md walnut; the light maple is the owner's 2026-11-22 option). */
const WOODS = {
  walnut: { dark: [44, 26, 14], light: [112, 72, 42], seed: 11 },
  maple: { dark: [158, 108, 62], light: [236, 196, 140], seed: 17 },
} as const;
export type WoodKind = keyof typeof WOODS;

/** CSS `url(...)` of a wood tile, or null where canvas is unavailable (tests). */
export function woodTexture(kind: WoodKind = "walnut"): string | null {
  if (cache[kind]) return cache[kind];
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = TILE_W;
  canvas.height = TILE_H;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(TILE_W, TILE_H);
  const data = img.data;
  const { dark, light, seed } = WOODS[kind];
  const pale = kind === "maple";
  for (let y = 0; y < TILE_H; y++) {
    const v = y / TILE_H;
    for (let x = 0; x < TILE_W; x++) {
      const u = x / TILE_W;
      // Figure: long, stretched along the grain (3 cells across, 20 down).
      const figure = fbm(u, v, 3, 20, 4, seed);
      // Growth lines: bands across the height, wobbling along the length.
      const wobble = fbm(u, v, 2, 4, 3, seed + 12);
      const lines = 0.5 + 0.5 * Math.sin((v * (pale ? 18 : 14) + wobble * 3.2) * Math.PI * 2);
      // Pores: short dark flecks (finer and fainter in maple).
      const pores = fbm(u, v, 40, 160, 2, seed + 26);
      let t = pale
        ? 0.42 + 0.6 * (figure - 0.3) - 0.22 * lines * lines * lines - (pores > 0.66 ? (pores - 0.66) * 0.9 : 0)
        : 0.18 + 0.75 * (figure - 0.25) + 0.16 * lines * lines - (pores > 0.62 ? (pores - 0.62) * 1.6 : 0);
      t = t < 0 ? 0 : t > 1 ? 1 : t;
      const i = (y * TILE_W + x) * 4;
      data[i] = dark[0] + (light[0] - dark[0]) * t;
      data[i + 1] = dark[1] + (light[1] - dark[1]) * t;
      data[i + 2] = dark[2] + (light[2] - dark[2]) * t;
      data[i + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  cache[kind] = `url("${canvas.toDataURL("image/jpeg", 0.9)}")`;
  return cache[kind];
}

/** CSS `url(...)` of the walnut tile, or null where canvas is unavailable (tests). */
export function walnutTexture(): string | null {
  return woodTexture("walnut");
}

const PEARL_W = 512;
const PEARL_H = 256;
let pearlCached: string | null = null;

/**
 * The pearlescent sheen laid over the moving warm light (owner's photo 7): milky
 * mother-of-pearl swirls with a faint play of warm whites, smooth and glossy (no
 * fluting). Transparent PNG, generated once; the colour comes from the glow
 * underneath, so the sheen never repaints.
 */
export function pearlTexture(): string | null {
  if (pearlCached !== null) return pearlCached;
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = PEARL_W;
  canvas.height = PEARL_H;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(PEARL_W, PEARL_H);
  const data = img.data;
  for (let y = 0; y < PEARL_H; y++) {
    const v = y / PEARL_H;
    for (let x = 0; x < PEARL_W; x++) {
      const u = x / PEARL_W;
      // Domain-warped swirls (tileable: the warp is itself tileable noise).
      const q = fbm(u, v, 2, 1, 3, 51);
      const r = fbm(u + 0.4 * q, v + 0.4 * q, 4, 2, 4, 63);
      const swirl = Math.max(0, Math.min(1, (r - 0.38) * 2.4));
      // Play of colour: a slow hue walk through soft pink, lilac, mint and peach.
      const h = (r * 3.1 + q * 1.7) % 1;
      // Warm whites only (peach, rose, champagne): a cool tint turned the warm glow grey.
      const tint = [255, 236 + 14 * Math.cos(6.283 * h), 214 + 26 * Math.cos(6.283 * (h - 0.4))];
      // Smooth, glossy mother of pearl: white sheen on the swirls only (no ribs: the
      // owner found the dark rib valleys looked like bars).
      const lightA = 0.04 + 0.34 * swirl * swirl;
      const shadeA = 0.035 * (1 - swirl);
      const a = Math.min(1, lightA + shadeA);
      const mix = lightA / Math.max(1e-6, a);
      const i = (y * PEARL_W + x) * 4;
      data[i] = tint[0] * mix + 120 * (1 - mix);
      data[i + 1] = tint[1] * mix + 30 * (1 - mix);
      data[i + 2] = tint[2] * mix + 20 * (1 - mix);
      data[i + 3] = Math.round(a * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  pearlCached = `url("${canvas.toDataURL("image/png")}")`;
  return pearlCached;
}

const FORGE_W = 512;
const FORGE_H = 256;
let forgeCached: string | null = null;

/** Distances to the nearest and second-nearest cell point of a tileable Worley grid. */
function worley(u: number, v: number, cx: number, cy: number, seed: number): [number, number] {
  const x = u * cx;
  const y = v * cy;
  const xi = Math.floor(x);
  const yi = Math.floor(y);
  let f1 = 9;
  let f2 = 9;
  for (let dy = -1; dy <= 1; dy++) {
    for (let dx = -1; dx <= 1; dx++) {
      const gx = xi + dx;
      const gy = yi + dy;
      const wx = ((gx % cx) + cx) % cx;
      const wy = ((gy % cy) + cy) % cy;
      const px = gx + 0.12 + 0.76 * hash(wx, wy, seed);
      const py = gy + 0.12 + 0.76 * hash(wx, wy, seed + 7);
      const d = Math.hypot(px - x, py - y);
      if (d < f1) {
        f2 = f1;
        f1 = d;
      } else if (d < f2) {
        f2 = d;
      }
    }
  }
  return [f1, f2];
}

/**
 * The forge cabinet's crust (Molten theme, 2026-11-23): blackened iron-dark slag split
 * by a network of cracks, like cooling lava. The cracks are transparent, so the moving
 * magma glow underneath (PearlGlow with FORGE_PALETTE) shows through them; the crust's
 * edges along each crack are thinner and glow dimly. Transparent PNG, generated once.
 */
export function forgeTexture(): string | null {
  if (forgeCached !== null) return forgeCached;
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = FORGE_W;
  canvas.height = FORGE_H;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(FORGE_W, FORGE_H);
  const data = img.data;
  const smooth = (e0: number, e1: number, x: number) => {
    const t = Math.max(0, Math.min(1, (x - e0) / (e1 - e0)));
    return t * t * (3 - 2 * t);
  };
  for (let y = 0; y < FORGE_H; y++) {
    const v = y / FORGE_H;
    for (let x = 0; x < FORGE_W; x++) {
      const u = x / FORGE_W;
      // Warp the plates a little so the cracks wander instead of running straight.
      const wu = u + 0.05 * (fbm(u, v, 4, 2, 3, 91) - 0.5);
      const wv = v + 0.1 * (fbm(u, v, 4, 2, 3, 97) - 0.5);
      // Big plates and a finer network inside them.
      const [a1, a2] = worley(wu, wv, 7, 3, 101);
      const [b1, b2] = worley(wu, wv, 18, 8, 131);
      const width = 0.05 + 0.05 * fbm(u, v, 6, 3, 2, 151);
      const big = 1 - smooth(0, width, a2 - a1);
      const fine = (1 - smooth(0, width * 0.45, b2 - b1)) * 0.5;
      const crack = Math.max(big, fine);
      // A dimly glowing rim along the big cracks (thin crust).
      const rim = (1 - smooth(width, width * 2.4, a2 - a1)) * (1 - big);
      // Crust: dark, rough, a little warmer and lighter in the middle of each plate.
      const rough = fbm(u, v, 16, 8, 4, 171);
      const dome = Math.min(1, a1 * 0.9);
      const base = 10 + 24 * rough + 8 * (1 - dome);
      let r = base * 1.45;
      let g = base * 0.6;
      let b = base * 0.42;
      // Heated rim: dark red.
      r += rim * 70;
      g += rim * 14;
      const alpha = Math.max(0, Math.min(1, 1 - 0.96 * crack - 0.22 * rim));
      const i = (y * FORGE_W + x) * 4;
      data[i] = Math.min(255, r);
      data[i + 1] = Math.min(255, g);
      data[i + 2] = Math.min(255, b);
      data[i + 3] = Math.round(alpha * 255);
    }
  }
  ctx.putImageData(img, 0, 0);
  forgeCached = `url("${canvas.toDataURL("image/png")}")`;
  return forgeCached;
}

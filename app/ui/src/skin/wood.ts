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

let cached: string | null = null;

/** CSS `url(...)` of the walnut tile, or null where canvas is unavailable (tests). */
export function walnutTexture(): string | null {
  if (cached !== null) return cached;
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = TILE_W;
  canvas.height = TILE_H;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(TILE_W, TILE_H);
  const data = img.data;
  // Walnut, dark to light (DESIGN.md: #3b2414 -> #5a3a22, a little lighter for figure).
  const dark = [44, 26, 14];
  const light = [112, 72, 42];
  for (let y = 0; y < TILE_H; y++) {
    const v = y / TILE_H;
    for (let x = 0; x < TILE_W; x++) {
      const u = x / TILE_W;
      // Figure: long, stretched along the grain (3 cells across, 20 down).
      const figure = fbm(u, v, 3, 20, 4, 11);
      // Growth lines: bands across the height, wobbling along the length.
      const wobble = fbm(u, v, 2, 4, 3, 23);
      const lines = 0.5 + 0.5 * Math.sin((v * 14 + wobble * 3.2) * Math.PI * 2);
      // Pores: short dark flecks.
      const pores = fbm(u, v, 40, 160, 2, 37);
      let t = 0.18 + 0.75 * (figure - 0.25) + 0.16 * lines * lines - (pores > 0.62 ? (pores - 0.62) * 1.6 : 0);
      t = t < 0 ? 0 : t > 1 ? 1 : t;
      const i = (y * TILE_W + x) * 4;
      data[i] = dark[0] + (light[0] - dark[0]) * t;
      data[i + 1] = dark[1] + (light[1] - dark[1]) * t;
      data[i + 2] = dark[2] + (light[2] - dark[2]) * t;
      data[i + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  cached = `url("${canvas.toDataURL("image/jpeg", 0.9)}")`;
  return cached;
}

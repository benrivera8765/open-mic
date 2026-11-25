// SPDX-License-Identifier: GPL-3.0-or-later
// Fragment shaders of the GPU cabinets (CabinetGL.tsx) and GPU pipes (PipeGL.tsx).
//
// Cabinets (D-272, D-275), drawn over the whole cabinet at reduced resolution:
//   forge   flowing lava: red and orange molten rock, yellow streaks, cooling crust
//   ocean   the sea: rows of waves rolling towards the viewer, white foam capping each
//           crest with lace trailing below it, sun glitter, haze towards the top
//   brass   steampunk: riveted brass and copper plates with verdigris, turning gears,
//           steam rising from the floor, flickering gaslight
//   cyber   cyberpunk: a neon city at night (owner's reference photos, 2026-11-24): rows
//           of dark towers with lit windows, floor bands and neon bars, holographic
//           billboards, stacked vertical signs and shop signs at street level, rain, a
//           wet street mirroring it all, and now and then a glitch
//
// Pipes (D-272, D-275), drawn in the pipe's own coordinates (v_flow.x: distance from the
// nearer burner, v_u: distance from the left foot, both px; v_flow.y: -1..1 across):
//   fire    a turbulent flame from both burners to the crest (Molten)
//   water   blue water coursing through from one end to the other: streaks, rolling
//           wave crests, white water, bubbles swept along
//   steam   white steam billowing from both ends through clear glass, condensation
//   data    cyberpunk: cyan and yellow neon walls, data packets racing both ways, a
//           light pulse, glitches

import { NOISE_GLSL } from "./gl";

export type GpuCabinet = "forge" | "ocean" | "brass" | "cyber";
export type GpuPipe = "fire" | "water" | "steam" | "data";

export const GPU_CABINETS: readonly GpuCabinet[] = ["forge", "ocean", "brass", "cyber"];
export const GPU_PIPES: readonly GpuPipe[] = ["fire", "water", "steam", "data"];

const CAB = NOISE_GLSL + `
uniform vec2 u_res;
uniform float u_time;
`;

const PIPE = NOISE_GLSL + `
uniform float u_time;
uniform float u_T;
varying vec2 v_flow;
varying float v_u;
`;

export const CABINET_FS: Record<GpuCabinet, string> = {
  forge: NOISE_GLSL + `
uniform vec2 u_res;
uniform float u_time;

// Nearest crust plate: x = distance to its seed, y = distance to the crack (edge), id in 0..1.
vec2 plates(vec2 x, out float id) {
  vec2 n = floor(x);
  vec2 f = fract(x);
  float f1 = 8.0;
  float f2 = 8.0;
  vec2 cell = vec2(0.0);
  for (int j = -1; j <= 1; j++) {
    for (int i = -1; i <= 1; i++) {
      vec2 g = vec2(float(i), float(j));
      vec2 r = g + hash22(n + g) * 0.8 + 0.1 - f;
      float d = dot(r, r);
      if (d < f1) {
        f2 = f1;
        f1 = d;
        cell = n + g;
      } else if (d < f2) {
        f2 = d;
      }
    }
  }
  id = hash12(cell + 17.0);
  float a = sqrt(f1);
  return vec2(a, sqrt(f2) - a);
}

// Black crust, dark red, red, red-orange, orange, yellow, white heat.
vec3 lavaRamp(float h) {
  h = clamp(h, 0.0, 1.0) * 6.0;
  if (h < 1.0) return mix(vec3(0.07, 0.006, 0.0), vec3(0.32, 0.018, 0.0), h);
  if (h < 2.0) return mix(vec3(0.32, 0.018, 0.0), vec3(0.74, 0.05, 0.0), h - 1.0);
  if (h < 3.0) return mix(vec3(0.74, 0.05, 0.0), vec3(1.0, 0.22, 0.0), h - 2.0);
  if (h < 4.0) return mix(vec3(1.0, 0.22, 0.0), vec3(1.0, 0.46, 0.03), h - 3.0);
  if (h < 5.0) return mix(vec3(1.0, 0.46, 0.03), vec3(1.0, 0.72, 0.2), h - 4.0);
  return mix(vec3(1.0, 0.72, 0.2), vec3(1.0, 0.95, 0.7), h - 5.0);
}

void main() {
  vec2 p = gl_FragCoord.xy / u_res.y * 2.6;
  float t = u_time;
  // The lava creeps down the cabinet and a little sideways.
  vec2 flow = vec2(0.03, 0.065) * t;
  // Folded by its own flow (domain warping): slow large folds, then the finer swirl.
  vec2 q = vec2(fbm3(p * 0.8 + flow * 0.6), fbm3(p * 0.8 + vec2(5.2, 1.3) + flow * 0.6));
  vec2 r = vec2(fbm(p + 2.2 * q + vec2(1.7, 9.2) + 0.045 * t), fbm(p + 2.2 * q + vec2(8.3, 2.8) - 0.04 * t));
  vec2 w = p + 1.6 * r + flow;
  float f = fbm(w * 1.3);
  // Molten heat: red where it is coolest, orange, and yellow streaks where it runs hottest.
  float heat = 0.13 + 0.72 * f;
  float ridge = 1.0 - abs(2.0 * fbm3(w * 2.2 + r * 1.5 - flow * 0.5) - 1.0);
  heat += pow(ridge, 6.0) * 0.27;
  // Cooling crust plates ride on the cooler lava, with glowing cracks round them.
  float id;
  vec2 v = plates(w * 1.9, id);
  float plate = smoothstep(0.36, 0.42, id) * smoothstep(0.6, 0.5, f);
  float inner = smoothstep(0.02, 0.1, v.y);
  float crust = plate * inner;
  // Crust: near black, rough, still glowing a little towards its edges.
  float skin = 0.015 + 0.13 * fbm3(w * 9.0) + 0.14 * (1.0 - smoothstep(0.08, 0.3, v.y));
  heat = mix(heat, skin, crust);
  // The cracks round a plate run hot.
  heat += plate * (1.0 - inner) * 0.22;
  // A slow breath of heat.
  heat *= 0.97 + 0.03 * sin(t * 0.7 + f * 6.0);
  gl_FragColor = vec4(lavaRamp(heat), 1.0);
}
`,

  ocean: CAB + `
// Row j's crest height within its row (0..1), rolling sideways and bobbing.
float crest(float x, float row, float t) {
  float h1 = hash12(vec2(row, 1.3));
  float h2 = hash12(vec2(row, 7.1));
  float dir = h1 < 0.5 ? 1.0 : -1.0;
  return 0.5 + 0.2 * sin(x * (14.0 + 8.0 * h2) + t * (0.6 + 0.5 * h1) * dir + h2 * 6.28)
    + 0.11 * sin(x * (31.0 + 9.0 * h1) - t * 1.1 * dir + h1 * 6.28)
    + 0.14 * (fbm3(vec2(x * 6.0 + t * 0.2 * dir, row * 1.7)) - 0.5);
}
// Colour of wave j at depth d (rows) below its crest.
vec3 waveCol(float j, float d, vec2 uv, float yy, float t) {
  float far = uv.y;
  vec3 deep = mix(vec3(0.0, 0.1, 0.3), vec3(0.04, 0.24, 0.46), far);
  vec3 face = mix(vec3(0.08, 0.52, 0.78), vec3(0.28, 0.64, 0.86), far);
  vec3 col = mix(face, deep, smoothstep(0.0, 0.85, d));
  col += vec3(0.08, 0.18, 0.24) * (fbm3(vec2(uv.x * 10.0, yy * 3.0)) - 0.5);
  // White foam capping the crest, lace trailing below it.
  float foamW = 0.08 + 0.14 * fbm3(vec2(uv.x * 24.0 + t * 0.3, j * 3.7));
  col = mix(col, vec3(0.93, 0.98, 1.0), (1.0 - smoothstep(foamW * 0.55, foamW, d)) * 0.94);
  float lace = smoothstep(0.6, 0.8, fbm(vec2(uv.x * 18.0 + j * 5.0, d * 6.0 - t * 0.2))) * (1.0 - smoothstep(0.1, 0.55, d));
  return mix(col, vec3(0.85, 0.95, 1.0), lace * 0.6);
}
void main() {
  vec2 uv = gl_FragCoord.xy / u_res.y;
  float t = u_time;
  // Rows of waves rolling towards the viewer (down the cabinet), the far ones smaller.
  float H = 0.058;
  float yy = uv.y / H * (1.0 + uv.y * 0.5) + t * 0.12;
  float i = floor(yy);
  float fy = yy - i;
  float ci = crest(uv.x, i, t);
  float c1 = crest(uv.x, i + 1.0, t);
  // Wave i where the pixel is below its crest, else the wave behind it; blended over
  // a pixel or so across the crest line.
  float aa = 1.5 / (H * u_res.y);
  vec3 col = mix(waveCol(i + 1.0, 1.0 + c1 - fy, uv, yy, t), waveCol(i, ci - fy, uv, yy, t), smoothstep(-aa, aa, ci - fy));
  float d = fy < ci ? ci - fy : 1.0 + c1 - fy;
  // Sun glitter on the faces.
  col += vec3(1.0) * step(0.986, hash12(floor(gl_FragCoord.xy * 0.5) + floor(t * 6.0))) * (1.0 - smoothstep(0.0, 0.4, d)) * 0.7;
  // Haze towards the horizon.
  col = mix(col, vec3(0.55, 0.78, 0.92), uv.y * uv.y * 0.22);
  gl_FragColor = vec4(col, 1.0);
}
`,

  brass: CAB + `
vec4 gear(vec2 q, float R, float N, float rot, float px) {
  float r = length(q);
  if (r > R * 1.05) return vec4(0.0);
  float aa = atan(q.y, q.x);
  float a = aa + rot;
  float tooth = smoothstep(-0.3, 0.3, sin(a * N));
  float outer = R * (0.86 + 0.14 * tooth);
  float cov = 1.0 - smoothstep(outer - px, outer + px, r);
  float rim = smoothstep(0.64 * R - px, 0.64 * R + px, r);
  float hub = 1.0 - smoothstep(0.24 * R - px, 0.24 * R + px, r);
  float spoke = 1.0 - smoothstep(0.07 * R - px, 0.07 * R + px, r * abs(sin(a * 3.0)) / 3.0);
  float axle = smoothstep(0.08 * R - px, 0.08 * R + px, r);
  cov *= max(rim, max(hub, spoke)) * axle;
  float lit = 0.5 + 0.5 * cos(aa - 2.3);
  vec3 c = mix(vec3(0.42, 0.28, 0.07), vec3(0.98, 0.8, 0.4), 0.35 + 0.5 * lit);
  c *= 0.9 + 0.1 * sin(r / R * 70.0);
  c += vec3(0.35, 0.28, 0.12) * smoothstep(0.82 * R, outer, r) * lit;
  c *= 1.0 - 0.35 * (1.0 - smoothstep(0.62 * R, 0.7 * R, r)) * (1.0 - hub);
  return vec4(c, cov);
}
void addGear(inout vec3 col, vec2 p, vec2 c, float R, float N, float rot, float px) {
  vec4 sh = gear(p - c - vec2(0.006, -0.009), R, N, rot, px * 5.0);
  col *= 1.0 - 0.55 * sh.a;
  vec4 g = gear(p - c, R, N, rot, px);
  col = mix(col, g.rgb, g.a);
}
void main() {
  float A = u_res.x / u_res.y;
  vec2 p = gl_FragCoord.xy / u_res.y;
  float t = u_time;
  float px = 1.2 / u_res.y;
  // Riveted plates, brass and copper, some gone green with verdigris.
  vec2 g = p * vec2(5.0, 6.0);
  g.x += 0.5 * mod(floor(g.y), 2.0);
  vec2 id = floor(g);
  vec2 f = fract(g);
  float hv = hash12(id);
  vec3 plate = mix(vec3(0.5, 0.26, 0.1), vec3(0.62, 0.45, 0.16), step(0.5, hv));
  plate *= 0.72 + 0.45 * fbm(p * vec2(3.0, 45.0) + hv * 10.0);
  float pat = smoothstep(0.55, 0.8, fbm(p * 6.0 + hv * 3.0)) * step(0.6, hash12(id + 3.0));
  plate = mix(plate, vec3(0.22, 0.45, 0.38), pat * 0.6);
  vec2 e = min(f, 1.0 - f) * vec2(0.2, 1.0 / 6.0);
  float edge = min(e.x, e.y);
  plate *= 0.5 + 0.5 * smoothstep(0.0, 0.008, edge);
  plate += vec3(0.18, 0.13, 0.05) * (1.0 - smoothstep(0.0, 0.004, abs(f.y - 0.985) / 6.0));
  // Rivets along the top and bottom of each plate.
  vec2 d = vec2((fract(f.x * 4.0) - 0.5) * 0.05, (f.y - (f.y < 0.5 ? 0.13 : 0.87)) / 6.0);
  float rr = 0.0065;
  float riv = 1.0 - smoothstep(rr - px, rr + px, length(d));
  vec2 nn = d / rr;
  float dome = 0.5 + 0.5 * clamp(-nn.x * 0.6 + nn.y * 0.8, -1.0, 1.0);
  vec3 rc = vec3(0.86, 0.66, 0.3) * dome + 0.5 * pow(max(0.0, 1.0 - length(nn - vec2(-0.35, 0.4))), 3.0);
  float rsh = (1.0 - smoothstep(rr, rr * 1.9, length(d - vec2(0.002, -0.003)))) * (1.0 - riv);
  vec3 col = plate * (1.0 - 0.45 * rsh);
  col = mix(col, rc, riv);
  // The gears, meshing in pairs.
  addGear(col, p, vec2(0.37 * A, 0.088), 0.085, 19.0, t * 0.4, px);
  addGear(col, p, vec2(0.37 * A + 0.128, 0.045), 0.055, 12.0, -t * 0.4 * 0.085 / 0.055 + 0.2, px);
  addGear(col, p, vec2(0.63 * A, 0.08), 0.095, 21.0, -t * 0.35, px);
  addGear(col, p, vec2(0.63 * A - 0.14, 0.03), 0.06, 13.0, t * 0.35 * 0.095 / 0.06 + 0.3, px);
  addGear(col, p, vec2(0.18 * A, 0.955), 0.07, 15.0, t * 0.3, px);
  addGear(col, p, vec2(0.82 * A, 0.955), 0.07, 15.0, -t * 0.3, px);
  // Steam rising off the floor.
  float st = smoothstep(0.45, 0.85, fbm(vec2(p.x * 2.5, p.y * 3.0 - t * 0.35))) * (1.0 - smoothstep(0.02, 0.32, p.y));
  col = mix(col, vec3(0.86, 0.85, 0.82), st * 0.55);
  // Gaslight: a warm vignette with a little flicker.
  vec2 v = gl_FragCoord.xy / u_res - 0.5;
  col *= (0.96 + 0.04 * sin(t * 7.0) * sin(t * 3.1)) * (0.78 + 0.32 * (1.0 - length(v * vec2(1.0, 1.2))));
  gl_FragColor = vec4(col, 1.0);
}
`,

  cyber: CAB + `
// Neon colours: cyan and teal most, then pink, violet, yellow and red.
vec3 neon(float h) {
  if (h < 0.3) return vec3(0.1, 0.9, 1.0);
  if (h < 0.48) return vec3(0.1, 0.95, 0.72);
  if (h < 0.6) return vec3(1.0, 0.16, 0.58);
  if (h < 0.72) return vec3(0.6, 0.28, 1.0);
  if (h < 0.88) return vec3(1.0, 0.82, 0.2);
  return vec3(1.0, 0.3, 0.16);
}
// Distance from q to the rectangle [lo, hi] (0 inside).
float rectD(vec2 q, vec2 lo, vec2 hi) {
  vec2 d = max(max(lo - q, q - hi), 0.0);
  return length(d);
}
// A neon letter in a unit box: its frame and a few random strokes, as tube glow.
float neonLetter(vec2 q, float seed) {
  float w = 0.09;
  float d = min(min(q.x, 1.0 - q.x), min(q.y, 1.0 - q.y));
  float tube = 1.0 - smoothstep(w * 0.4, w, d);
  for (int k = 0; k < 3; k++) {
    float fk = float(k);
    float y0 = 0.25 + 0.25 * fk;
    if (hash12(vec2(seed, fk)) > 0.45) tube = max(tube, (1.0 - smoothstep(w * 0.4, w, abs(q.y - y0))) * step(0.15, q.x) * step(q.x, 0.85));
    float x0 = 0.3 + 0.4 * fk * 0.5;
    if (hash12(vec2(seed, fk + 5.0)) > 0.55) tube = max(tube, (1.0 - smoothstep(w * 0.4, w, abs(q.x - x0))) * step(0.15, q.y) * step(q.y, 0.85));
  }
  return tube;
}
// One row of towers (layer 0 far .. 2 near): colour and coverage.
vec4 towers(vec2 p, vec2 uv, float dens, float hBase, float hRange, float layer, float t) {
  float x = p.x * dens + layer * 13.7;
  float bi = floor(x);
  float bf = fract(x);
  float bw = 1.0 / dens;
  float hb = hash12(vec2(bi, layer));
  float w0 = 0.06 + 0.12 * hash12(vec2(bi, layer + 3.0));
  float h = hBase + hRange * hb;
  if (bf < w0 || bf > 1.0 - w0 * 0.4 || uv.y > h) return vec4(0.0);
  vec3 col = mix(vec3(0.008, 0.012, 0.022), vec3(0.022, 0.036, 0.055), hash12(vec2(bi, layer + 5.0)));
  col *= 0.75 + 0.45 * smoothstep(w0, w0 + 0.15, bf) * (1.0 - smoothstep(0.75, 1.0, bf));
  // Many small windows, most dark: warm white, cyan, teal, the odd pink.
  vec2 wq = vec2(p.x * (150.0 + 90.0 * layer), uv.y * (170.0 + 70.0 * layer));
  vec2 wid = floor(wq);
  vec2 wf = fract(wq);
  float lit = step(0.62, hash12(wid + bi * 3.1 + layer * 31.0)) * step(0.2, wf.x) * step(wf.x, 0.8) * step(0.25, wf.y) * step(wf.y, 0.75);
  float hw = hash12(wid + 7.0);
  vec3 wc = hw < 0.45 ? vec3(1.0, 0.78, 0.5) : hw < 0.78 ? vec3(0.45, 0.88, 1.0) : hw < 0.93 ? vec3(0.35, 1.0, 0.75) : vec3(1.0, 0.4, 0.7);
  float wb = (0.25 + 0.75 * hash12(wid + 13.0)) * (0.45 + 0.25 * layer);
  float type = hash12(vec2(bi, layer + 7.0));
  if (type < 0.55) {
    col += wc * lit * wb;
  } else if (type < 0.8) {
    // Bands of light round every floor.
    float fy = fract(uv.y * (110.0 + 40.0 * layer));
    col += neon(hash12(vec2(bi, 15.0)) * 0.48) * smoothstep(0.55, 0.7, fy) * (1.0 - smoothstep(0.75, 0.9, fy)) * (0.3 + 0.4 * hash12(vec2(bi, floor(uv.y * 30.0))));
    col += wc * lit * wb * 0.3;
  } else {
    // Neon tubes up the face, glowing.
    float fx = fract(bf * 4.0);
    col += neon(hash12(vec2(bi, 17.0))) * (exp(-pow((fx - 0.5) * 40.0, 2.0)) + 0.22 * exp(-pow((fx - 0.5) * 8.0, 2.0)));
    col += wc * lit * wb * 0.25;
  }
  // A neon strip up one edge, with its glow on the wall.
  if (hash12(vec2(bi, layer + 9.0)) > 0.55) {
    float d = (bf - w0) * bw;
    col += neon(hash12(vec2(bi, 19.0))) * (exp(-pow(d * 900.0, 2.0)) + 0.3 * exp(-d * 140.0));
  }
  // A holographic billboard on the nearer towers: a soft graphic, rows of small text, scanlines.
  if (layer > 0.5 && hash12(vec2(bi, layer + 11.0)) > 0.5) {
    float by = h - 0.1 - 0.3 * hash12(vec2(bi, layer + 12.0));
    float bh = 0.05 + 0.06 * hash12(vec2(bi, layer + 13.0));
    float x0 = w0 + 0.1;
    float x1 = 0.86;
    vec3 c1 = neon(hash12(vec2(bi, 20.0)));
    vec3 c2 = neon(hash12(vec2(bi, 21.0)));
    float dist = rectD(vec2(bf * bw, uv.y), vec2(x0 * bw, by), vec2(x1 * bw, by + bh));
    if (dist <= 0.0) {
      vec2 bu = vec2((bf - x0) / (x1 - x0), (uv.y - by) / bh);
      vec3 ad = mix(c1, c2, smoothstep(0.2, 0.8, 0.5 + 0.5 * sin(bu.x * 4.0 + bu.y * 2.0 + t * 1.2 + hb * 10.0)));
      float blob = 1.0 - smoothstep(0.26, 0.34, length((bu - vec2(0.25 + 0.5 * hash12(vec2(bi, 22.0)), 0.5)) * vec2(1.7, 1.0)));
      ad = mix(ad * 0.6, neon(hash12(vec2(bi, 23.0))) * 1.1, blob * 0.8);
      float row = step(0.6, fract(bu.y * 7.0)) * step(0.45, hash12(floor(vec2(bu.x * 34.0, bu.y * 7.0)) + floor(t * 0.5 + hb * 3.0)));
      ad += vec3(1.0) * row * 0.3 * (1.0 - blob);
      col = ad * (0.85 + 0.15 * sin(uv.y * 1400.0));
    } else {
      col += mix(c1, c2, 0.5) * exp(-dist * 70.0) * 0.45;
    }
  }
  // Street level on the nearest towers: a tall sign of stacked neon letters, a shop sign.
  if (layer > 1.5 && uv.y < 0.22) {
    float flick = 0.4 + 0.6 * step(0.05, hash12(vec2(bi, floor(t * 3.0))));
    if (hash12(vec2(bi, 31.0)) > 0.45) {
      float sx = w0 + 0.04;
      float sw = 0.1;
      vec3 sc = neon(hash12(vec2(bi, 32.0)));
      float dist = rectD(vec2(bf * bw, uv.y), vec2(sx * bw, 0.035), vec2((sx + sw) * bw, 0.19));
      if (dist <= 0.0) {
        vec2 su = vec2((bf - sx) / sw, (uv.y - 0.035) / 0.155 * 5.0);
        float cell = floor(su.y);
        float tube = neonLetter(vec2(su.x, fract(su.y)), cell + bi * 7.0);
        col = mix(vec3(0.02, 0.02, 0.03), sc * 1.2, tube) * flick + sc * 0.12;
      } else {
        col += sc * exp(-dist * 90.0) * 0.5 * flick;
      }
    }
    if (hash12(vec2(bi, 33.0)) > 0.35) {
      float sy = 0.04 + 0.07 * hash12(vec2(bi, 34.0));
      vec3 sc = neon(hash12(vec2(bi, 35.0)));
      float dist = rectD(vec2(bf * bw, uv.y), vec2(0.34 * bw, sy), vec2(0.92 * bw, sy + 0.024));
      if (dist <= 0.0) {
        vec2 su = vec2((bf - 0.34) / 0.58 * 8.0, (uv.y - sy) / 0.024);
        float tube = neonLetter(vec2(fract(su.x), su.y), floor(su.x) + bi * 3.0);
        col = mix(vec3(0.02), sc * 1.2, tube) * flick + sc * 0.1;
      } else {
        col += sc * exp(-dist * 110.0) * 0.4 * flick;
      }
    }
  }
  // Haze swallows the far rows.
  col = mix(col, vec3(0.03, 0.075, 0.11), layer < 0.5 ? 0.55 : layer < 1.5 ? 0.22 : 0.0);
  return vec4(col, 1.0);
}
vec3 city(vec2 uv, float A, float t) {
  vec2 p = vec2(uv.x * A, uv.y);
  // Night sky, lit teal and magenta from below by the city.
  vec3 col = mix(vec3(0.05, 0.1, 0.14), vec3(0.006, 0.01, 0.025), smoothstep(0.35, 1.0, uv.y));
  col += vec3(0.3, 0.05, 0.22) * exp(-pow((uv.x - 0.7) * 3.0, 2.0)) * smoothstep(1.0, 0.6, uv.y) * 0.22;
  vec4 l0 = towers(p, uv, 9.0, 0.86, 0.14, 0.0, t);
  col = mix(col, l0.rgb, l0.a);
  vec4 l1 = towers(p, uv, 6.0, 0.8, 0.22, 1.0, t);
  col = mix(col, l1.rgb, l1.a);
  vec4 l2 = towers(p, uv, 3.5, 0.68, 0.4, 2.0, t);
  col = mix(col, l2.rgb, l2.a);
  // Neon haze drifting low between the towers, teal and magenta.
  float hz = fbm3(vec2(p.x * 1.6 + t * 0.03, uv.y * 3.0 - t * 0.01));
  vec3 hc = mix(vec3(0.0, 0.32, 0.38), vec3(0.34, 0.05, 0.26), smoothstep(0.35, 0.7, fbm3(vec2(p.x * 0.7 - t * 0.02, 2.0))));
  col += hc * hz * hz * (1.0 - smoothstep(0.0, 0.45, uv.y)) * 0.4;
  return col;
}
void main() {
  vec2 uv = gl_FragCoord.xy / u_res;
  float A = u_res.x / u_res.y;
  float t = u_time;
  // Now and then the picture glitches: bands jump sideways.
  float glt = step(0.965, hash12(vec2(floor(t * 5.0), 1.7)));
  float band = step(0.7, hash12(vec2(floor(uv.y * 24.0), floor(t * 30.0))));
  uv.x += glt * band * (hash12(vec2(floor(uv.y * 24.0), floor(t * 10.0))) - 0.5) * 0.05;
  vec3 col;
  float street = 0.035;
  if (uv.y > street) {
    col = city(uv, A, t);
  } else {
    // The wet street mirrors the city, broken up by ripples, with puddle glints.
    float rip = (fbm3(vec2(uv.x * A * 30.0, uv.y * 80.0 + t * 0.6)) - 0.5) * 0.014;
    col = city(vec2(uv.x + rip, 2.0 * street - uv.y + 0.004), A, t) * 0.55;
    col = mix(vec3(0.008, 0.012, 0.018), col, 0.45 + 0.55 * smoothstep(0.0, street, uv.y));
    col += vec3(0.3, 0.55, 0.65) * step(0.93, fbm3(vec2(uv.x * A * 12.0, uv.y * 40.0))) * 0.2;
  }
  // Rain, slanting down through the light.
  vec2 rq = vec2(uv.x * A * 140.0 + uv.y * 26.0, uv.y * 5.0 + t * 7.0);
  float rc = floor(rq.x);
  float ry = fract(rq.y + hash12(vec2(rc, 1.0)) * 10.0);
  col += vec3(0.55, 0.75, 0.9) * step(0.8, hash12(vec2(rc, 3.0))) * smoothstep(0.0, 0.04, ry) * (1.0 - smoothstep(0.04, 0.22, ry)) * 0.18;
  // Film grain and a touch of chromatic fringe.
  col += (hash12(gl_FragCoord.xy + floor(t * 24.0) * 17.0) - 0.5) * 0.035;
  col = mix(col, col.gbr, glt * band * 0.5);
  gl_FragColor = vec4(max(col, vec3(0.0)), 1.0);
}
`,

};

export const PIPE_FS: Record<GpuPipe, string> = {
  fire: NOISE_GLSL + `
uniform float u_time;
uniform float u_T;
varying vec2 v_flow;

// Deep red, red, orange, gold, pale yellow, white heat.
vec3 fireRamp(float i) {
  i = clamp(i, 0.0, 1.0) * 5.0;
  if (i < 1.0) return mix(vec3(0.5, 0.03, 0.0), vec3(0.92, 0.15, 0.01), i);
  if (i < 2.0) return mix(vec3(0.92, 0.15, 0.01), vec3(1.0, 0.42, 0.04), i - 1.0);
  if (i < 3.0) return mix(vec3(1.0, 0.42, 0.04), vec3(1.0, 0.68, 0.16), i - 2.0);
  if (i < 4.0) return mix(vec3(1.0, 0.68, 0.16), vec3(1.0, 0.9, 0.52), i - 3.0);
  return mix(vec3(1.0, 0.9, 0.52), vec3(1.0, 1.0, 0.92), i - 4.0);
}

void main() {
  float y = v_flow.x / u_T;   // pipe widths from the nearer burner
  float x = v_flow.y;          // -1 .. 1 across the pipe
  float t = u_time;
  // Turbulence carried along the pipe towards the crest.
  vec2 a = vec2(x * 1.2, y * 0.38 - t * 1.9);
  float n1 = fbm(a);
  float n2 = fbm(a * 1.9 + vec2(4.7, 1.3) - vec2(0.0, t * 1.1));
  // The body: its centre wanders, its width breathes into tongues.
  float cx = (n2 - 0.5) * 0.7;
  float w = 0.35 + 0.75 * n1;
  float body = 1.0 - smoothstep(0.1, 1.0, abs(x - cx) / w);
  // Licks: finer turbulence eats into the edges and streaks along the flow.
  float lick = fbm(vec2(x * 2.4 + n1 * 2.0, y * 0.9 - t * 3.4));
  float I = body * (0.35 + 1.1 * lick);
  // Pulses of heat running along the pipe.
  I *= 0.6 + 0.7 * smoothstep(0.3, 0.75, fbm3(vec2(y * 0.16 - t * 0.85, 7.3)));
  // Narrower and blue at the burner.
  float root = smoothstep(0.0, 1.8, y);
  I *= mix(0.55, 1.0, root);
  vec3 col = fireRamp(I * 1.2);
  float alpha = smoothstep(0.04, 0.4, I);
  col = mix(col, vec3(0.3, 0.5, 1.0), (1.0 - root) * smoothstep(0.25, 0.8, body) * 0.65);
  // Embers: short sparks racing ahead.
  vec2 eg = vec2(x * 1.6 + 7.0, y * 0.9 - t * 4.2);
  vec2 ci = floor(eg);
  vec2 d = (fract(eg) - (hash22(ci + 3.1) * 0.7 + 0.15)) * vec2(1.0, 0.55);
  float h = hash12(ci);
  float ember = step(0.72, h) * exp(-dot(d, d) * 160.0) * (0.65 + 0.35 * sin(t * 20.0 + h * 50.0));
  col = mix(col, vec3(1.0, 0.86, 0.52), clamp(ember, 0.0, 1.0));
  alpha = max(alpha, clamp(ember, 0.0, 1.0));
  // Soft against the glass walls.
  alpha *= 1.0 - smoothstep(0.84, 1.0, abs(x));
  gl_FragColor = vec4(col * alpha, alpha);
}
`,

  water: PIPE + `
void main() {
  float x = v_flow.y;
  float y = v_u / u_T;
  float t = u_time;
  float rnd = 1.0 - x * x;
  vec3 col = mix(vec3(0.0, 0.1, 0.32), vec3(0.04, 0.42, 0.86), rnd);
  float flow = y * 0.42 - t * 2.4;
  float s1 = fbm(vec2(x * 1.8, flow));
  float s2 = fbm(vec2(x * 3.2 + 4.0, flow * 1.6 - t * 0.7));
  col += vec3(0.15, 0.5, 0.8) * smoothstep(0.5, 0.82, s1) * 0.7;
  // Waves rolling along the pipe.
  float wv = sin(y * 1.1 - t * 3.2 + x * 1.2 + s1 * 3.0);
  col += vec3(0.25, 0.6, 0.8) * smoothstep(0.75, 1.0, wv) * 0.5;
  // White water.
  float foam = smoothstep(0.64, 0.84, s2) * smoothstep(0.2, 0.9, wv * 0.5 + 0.5);
  col = mix(col, vec3(0.88, 0.96, 1.0), foam * 0.8);
  // Bubbles swept along.
  vec2 bq = vec2(x * 1.2 + 5.0, y * 1.2 - t * 3.0);
  vec2 bi = floor(bq);
  vec2 bf = fract(bq) - 0.5 - (hash22(bi) - 0.5) * 0.4;
  float br = 0.12 + 0.13 * hash12(bi + 2.0);
  vec2 bd = vec2(bf.x, bf.y * 2.1) / br;
  float bl = length(bd);
  float has = step(0.6, hash12(bi + 7.0)) * (1.0 - smoothstep(0.95, 1.05, bl));
  float bub = has * (smoothstep(0.55, 0.92, bl) + (1.0 - smoothstep(0.0, 0.35, length(bd - vec2(-0.3, 0.3)))));
  col += vec3(0.8, 0.95, 1.0) * bub * 0.75;
  // The glass's reflection.
  col += vec3(0.7, 0.9, 1.0) * exp(-pow((x + 0.55) * 8.0, 2.0)) * 0.3;
  float alpha = 0.94 * (1.0 - smoothstep(0.86, 1.0, abs(x)));
  gl_FragColor = vec4(min(col, vec3(1.0)) * alpha, alpha);
}
`,

  steam: PIPE + `
void main() {
  float x = v_flow.y;
  float s = v_flow.x / u_T;
  float t = u_time;
  float rnd = 1.0 - x * x;
  // Clear, dark glass: the steam is the only thing in it.
  vec3 col = vec3(0.06, 0.065, 0.075) * (0.6 + 0.4 * rnd);
  // Billows of white steam pouring from both ends to the crest, in puffs.
  float a = fbm(vec2(x * 1.2, s * 0.38 - t * 1.6));
  float b = fbm(vec2(x * 2.3 + 3.0, s * 0.8 - t * 2.4));
  float puff = smoothstep(0.2, 0.7, fbm3(vec2(s * 0.18 - t * 0.85, 3.0)));
  float steam = clamp(smoothstep(0.3, 0.7, a * 0.6 + b * 0.5) * (0.6 + 0.6 * puff), 0.0, 1.0);
  // Soft grey-blue shadows in the folds, bright white where it is thick.
  float fold = fbm3(vec2(x * 3.0 + 1.0, s * 1.2 - t * 2.0));
  vec3 sc = mix(vec3(0.72, 0.76, 0.82), vec3(1.0, 1.0, 0.99), clamp(steam * 0.8 + fold * 0.4, 0.0, 1.0));
  col = mix(col, sc, steam);
  // A soft white glow through the whole pipe.
  col += vec3(0.18, 0.19, 0.2) * rnd * puff;
  // Condensation on the glass.
  vec2 dq = vec2(x * 3.0 + 9.0, s * 1.2 - t * 0.15);
  vec2 di = floor(dq);
  vec2 dp = fract(dq) - 0.5 - (hash22(di) - 0.5) * 0.5;
  col += vec3(1.0) * step(0.7, hash12(di)) * (1.0 - smoothstep(0.05, 0.09, length(dp))) * 0.45;
  float alpha = (0.55 + 0.43 * steam) * (1.0 - smoothstep(0.86, 1.0, abs(x)));
  gl_FragColor = vec4(min(col, vec3(1.0)) * alpha, alpha);
}
`,

  data: PIPE + `
void main() {
  float x = v_flow.y;
  float y = v_u / u_T;
  float t = u_time;
  // A dark teal haze inside.
  vec3 col = vec3(0.0, 0.02, 0.035) + vec3(0.0, 0.05, 0.07) * (1.0 - x * x);
  float flick = 0.82 + 0.18 * step(0.12, hash12(vec2(floor(t * 14.0), 2.0)));
  // Neon walls: cyan on one side, hazard yellow on the other.
  col += vec3(0.1, 0.92, 1.0) * (exp(-pow((x + 0.74) * 14.0, 2.0)) + 0.3 * exp(-pow((x + 0.74) * 4.0, 2.0))) * flick;
  col += vec3(1.0, 0.82, 0.15) * (exp(-pow((x - 0.74) * 16.0, 2.0)) + 0.2 * exp(-pow((x - 0.74) * 4.0, 2.0))) * flick * 0.85;
  // Data packets racing both ways in four lanes: cyan, white, yellow, now and then red or pink.
  for (int k = 0; k < 4; k++) {
    float fk = float(k);
    float lx = -0.45 + 0.3 * fk;
    float dir = mod(fk, 2.0) < 0.5 ? 1.0 : -1.0;
    float ly = y * 0.6 - dir * t * (2.4 + fk * 0.7);
    float cell = floor(ly);
    float f = fract(ly);
    float len = 0.15 + 0.45 * hash12(vec2(cell, fk + 9.0));
    float dash = step(0.45, hash12(vec2(cell, fk))) * step(f, len);
    float bright = dir > 0.0 ? f / len : 1.0 - f / len;
    float hc = hash12(vec2(cell, fk + 4.0));
    vec3 c = hc < 0.45 ? vec3(0.15, 0.95, 1.0) : hc < 0.65 ? vec3(0.9, 1.0, 1.0) : hc < 0.85 ? vec3(1.0, 0.85, 0.2) : hc < 0.93 ? vec3(1.0, 0.25, 0.2) : vec3(1.0, 0.2, 0.65);
    col += c * dash * (0.35 + 0.65 * bright) * exp(-pow((x - lx) * 16.0, 2.0)) * 1.4;
  }
  // A pulse of light racing the whole pipe; glitch bands now and then.
  col += vec3(0.6, 0.95, 1.0) * exp(-pow((fract(y * 0.012 - t * 0.25) - 0.5) * 30.0, 2.0)) * 0.5 * (1.0 - x * x);
  float gl2 = step(0.95, hash12(vec2(floor(t * 6.0), 5.0))) * step(0.6, hash12(vec2(floor(y * 0.5), floor(t * 20.0))));
  col = mix(col, col.brg * 1.3, gl2);
  col *= 0.9 + 0.1 * sin(y * 40.0);
  float alpha = clamp(0.62 + max(col.r, max(col.g, col.b)), 0.0, 1.0) * (1.0 - smoothstep(0.9, 1.0, abs(x)));
  gl_FragColor = vec4(min(col, vec3(1.0)) * alpha, alpha);
}
`,

};

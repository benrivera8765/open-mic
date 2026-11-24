// SPDX-License-Identifier: GPL-3.0-or-later
// Small WebGL 1 helpers for the GPU effects (D-272, D-275): the GPU cabinets
// (CabinetGL.tsx) and pipes (PipeGL.tsx). WebGL 1 only (WebView2 and
// WKWebView both have it); every caller falls back to its 2D canvas version when a
// context or a shader cannot be made, or the context is lost.

import { fxLoop } from "../lib/fxrate";

export function glContext(canvas: HTMLCanvasElement, alpha: boolean): WebGLRenderingContext | null {
  try {
    const gl = canvas.getContext("webgl", {
      alpha,
      premultipliedAlpha: true,
      antialias: false,
      depth: false,
      stencil: false,
      preserveDrawingBuffer: false,
      powerPreference: "low-power",
    });
    return (gl as WebGLRenderingContext | null) ?? null;
  } catch {
    return null;
  }
}

function shader(gl: WebGLRenderingContext, type: number, src: string): WebGLShader | null {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS) && !gl.isContextLost()) {
    console.warn("jk-gl: shader did not compile", gl.getShaderInfoLog(s));
    gl.deleteShader(s);
    return null;
  }
  return s;
}

/** A linked program, or null (the caller falls back to its 2D version). */
export function glProgram(gl: WebGLRenderingContext, vs: string, fs: string): WebGLProgram | null {
  const v = shader(gl, gl.VERTEX_SHADER, vs);
  const f = shader(gl, gl.FRAGMENT_SHADER, fs);
  if (!v || !f) return null;
  const p = gl.createProgram();
  if (!p) return null;
  gl.attachShader(p, v);
  gl.attachShader(p, f);
  gl.linkProgram(p);
  gl.deleteShader(v);
  gl.deleteShader(f);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS) && !gl.isContextLost()) {
    console.warn("jk-gl: program did not link", gl.getProgramInfoLog(p));
    gl.deleteProgram(p);
    return null;
  }
  return p;
}

/** Fragment shader preamble: precision, hashes, gradient noise and fractal noise (all 0..1 or -1..1 as noted). */
export const NOISE_GLSL = `
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
float hash12(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
vec2 hash22(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.xx + p3.yz) * p3.zy);
}
// Gradient noise, about -0.7..0.7.
float gnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
  float a = dot(hash22(i) * 2.0 - 1.0, f);
  float b = dot(hash22(i + vec2(1.0, 0.0)) * 2.0 - 1.0, f - vec2(1.0, 0.0));
  float c = dot(hash22(i + vec2(0.0, 1.0)) * 2.0 - 1.0, f - vec2(0.0, 1.0));
  float d = dot(hash22(i + vec2(1.0, 1.0)) * 2.0 - 1.0, f - vec2(1.0, 1.0));
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
// Fractal noise, 0..1 (centred on 0.5).
float fbm(vec2 p) {
  float s = 0.0;
  float a = 0.5;
  mat2 m = mat2(1.6, 1.2, -1.2, 1.6);
  for (int i = 0; i < 5; i++) {
    s += a * gnoise(p);
    p = m * p;
    a *= 0.5;
  }
  return clamp(0.5 + s * 0.85, 0.0, 1.0);
}
// Fewer octaves for the cheap parts.
float fbm3(vec2 p) {
  float s = 0.0;
  float a = 0.5;
  mat2 m = mat2(1.6, 1.2, -1.2, 1.6);
  for (int i = 0; i < 3; i++) {
    s += a * gnoise(p);
    p = m * p;
    a *= 0.5;
  }
  return clamp(0.5 + s * 0.95, 0.0, 1.0);
}
`;

/** Draws at most `fps` frames a second (lib/fxrate fxLoop) with the seconds since the start; returns the stop function. */
export function frameLoop(fps: number, draw: (seconds: number) => void): () => void {
  const start = performance.now();
  return fxLoop(fps, (now) => draw(Math.max(0, now - start) / 1000));
}

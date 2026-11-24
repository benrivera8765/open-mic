// SPDX-License-Identifier: GPL-3.0-or-later
// A glass pipe whose contents are drawn on the GPU (D-272, D-275): Molten's fire, water
// coursing through, steam, cyberpunk data, diesel exhaust, Frutiger Aero liquid
// (shaders.ts PIPE_FS). FireTube.tsx is the fire's 2D fallback.
//
// The pipe is one triangle strip from the foot of the left column, over the arch, to the
// foot of the right column. Each vertex carries its distance from the nearer foot and
// from the left foot (along the pipe) and where it sits across the pipe, so a shader
// works in the pipe's own coordinates and follows every bend: fire and steam rise from
// both feet to the crest, water and the others run from one end to the other.
//
// Cost: only the pipe's pixels are shaded, 30 frames a second by default (the frame rate
// setting, D-274, raises it), one draw call. Stops when hidden or minimised; a still
// frame under reduced motion. Where WebGL is missing or the context is lost, `onFail`
// lets the frame fall back to its 2D look.

import { useEffect, useRef, useState } from "preact/hooks";
import { useEffectFps } from "../lib/fxrate";
import { frameLoop, glContext, glProgram } from "./gl";
import { bubbleTrack, type FrameGeometry, type PathPoint } from "./geometry";
import { PIPE_FS, type GpuPipe } from "./shaders";

const FPS = 30;
const MAX_CANVAS_PIXELS = 3840 * 2160;
/** Floats per vertex: x, y (CSS px), distance from the nearer foot, across (-1..1), distance from the left foot. */
export const PIPE_STRIDE = 5;

const VS = `
attribute vec2 a_pos;
attribute vec2 a_flow;
attribute float a_u;
uniform vec2 u_size;
varying vec2 v_flow;
varying float v_u;
void main() {
  v_flow = a_flow;
  v_u = a_u;
  vec2 c = a_pos / u_size * 2.0 - 1.0;
  gl_Position = vec4(c.x, -c.y, 0.0, 1.0);
}
`;

/** Vertices of the pipe strip (PIPE_STRIDE floats each). */
export function pipeStrip(g: FrameGeometry): Float32Array {
  const left = bubbleTrack(g, "left");
  const right = bubbleTrack(g, "right");
  const L = left.length;
  const U = 2 * L;
  const half = g.tube * 0.48;
  const step = Math.max(3, g.tube * 0.2);
  const n = Math.max(2, Math.ceil(U / step) + 1);
  const out = new Float32Array(n * 2 * PIPE_STRIDE);
  const pt: PathPoint = { x: 0, y: 0, nx: 0, ny: 0 };
  for (let i = 0; i < n; i++) {
    const u = Math.min(U, (i * U) / (n - 1));
    if (u <= L) left.at(u, pt);
    else right.at(Math.max(0, U - u), pt);
    const s = Math.min(u, U - u);
    for (let side = 0; side < 2; side++) {
      const lat = side === 0 ? -1 : 1;
      const o = (i * 2 + side) * PIPE_STRIDE;
      out[o] = pt.x + pt.nx * half * lat;
      out[o + 1] = pt.y + pt.ny * half * lat;
      out[o + 2] = s;
      out[o + 3] = lat;
      out[o + 4] = u;
    }
  }
  return out;
}

export interface PipeGLProps {
  kind: GpuPipe;
  geometry: FrameGeometry;
  /** False: draw one still frame and stop. */
  running: boolean;
  /** WebGL is missing or was lost: the frame falls back to its 2D look. */
  onFail: () => void;
  class?: string;
}

export function PipeGL({ kind, geometry, running, onFail, class: className }: PipeGLProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fps = useEffectFps(FPS);
  const timeRef = useRef(17);
  const failRef = useRef(onFail);
  failRef.current = onFail;
  /** Bumped when a lost context comes back, to set it up again. */
  const [epoch, setEpoch] = useState(0);
  const glRef = useRef<WebGLRenderingContext | null>(null);
  const aliveRef = useRef(true);
  // On unmount, give the context back at once (browsers keep only about 16 alive) instead
  // of waiting for garbage collection.
  useEffect(
    () => () => {
      aliveRef.current = false;
      glRef.current?.getExtension("WEBGL_lose_context")?.loseContext();
      glRef.current = null;
      const c = canvasRef.current;
      if (c) {
        c.width = 0;
        c.height = 0;
      }
    },
    [],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const g = geometry;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    let k = Math.min(dpr, 2);
    if (g.width * g.height * k * k > MAX_CANVAS_PIXELS) k = Math.sqrt(MAX_CANVAS_PIXELS / (g.width * g.height));
    canvas.width = Math.max(1, Math.round(g.width * k));
    canvas.height = Math.max(1, Math.round(g.height * k));
    const gl = glContext(canvas, true);
    const prog = gl ? glProgram(gl, VS, PIPE_FS[kind]) : null;
    if (!gl || !prog) {
      failRef.current();
      return;
    }
    glRef.current = gl;
    const verts = pipeStrip(g);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, verts, gl.STATIC_DRAW);
    gl.useProgram(prog);
    const bytes = PIPE_STRIDE * 4;
    const aPos = gl.getAttribLocation(prog, "a_pos");
    const aFlow = gl.getAttribLocation(prog, "a_flow");
    const aU = gl.getAttribLocation(prog, "a_u");
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, bytes, 0);
    gl.enableVertexAttribArray(aFlow);
    gl.vertexAttribPointer(aFlow, 2, gl.FLOAT, false, bytes, 8);
    if (aU >= 0) {
      gl.enableVertexAttribArray(aU);
      gl.vertexAttribPointer(aU, 1, gl.FLOAT, false, bytes, 16);
    }
    gl.uniform2f(gl.getUniformLocation(prog, "u_size"), g.width, g.height);
    gl.uniform1f(gl.getUniformLocation(prog, "u_T"), g.tube);
    const uTime = gl.getUniformLocation(prog, "u_time");
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.disable(gl.BLEND);
    gl.clearColor(0, 0, 0, 0);
    const count = verts.length / PIPE_STRIDE;
    const t0 = timeRef.current;
    const draw = (s: number) => {
      const t = (t0 + s) % 3600;
      timeRef.current = t;
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform1f(uTime, t);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, count);
    };
    let stop = () => {};
    // A lost context (driver reset, GPU switch) usually comes back: wait for it a few
    // seconds and set up again; only if it does not, fall back to the 2D look.
    let giveUp = 0;
    const lost = (e: Event) => {
      e.preventDefault();
      stop();
      if (!aliveRef.current) return;
      giveUp = window.setTimeout(() => failRef.current(), 3000);
    };
    const restored = () => {
      clearTimeout(giveUp);
      if (aliveRef.current) setEpoch((n) => n + 1);
    };
    canvas.addEventListener("webglcontextlost", lost);
    canvas.addEventListener("webglcontextrestored", restored);
    if (running) stop = frameLoop(fps, draw);
    else draw(0);
    return () => {
      stop();
      clearTimeout(giveUp);
      canvas.removeEventListener("webglcontextlost", lost);
      canvas.removeEventListener("webglcontextrestored", restored);
      if (!gl.isContextLost()) {
        gl.deleteBuffer(buf);
        gl.deleteProgram(prog);
      }
    };
  }, [kind, running, geometry, fps, epoch]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

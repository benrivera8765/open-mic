// SPDX-License-Identifier: GPL-3.0-or-later
// A cabinet drawn on the GPU (D-272, D-275): one WebGL fragment shader over the whole
// cabinet (shaders.ts CABINET_FS: Molten's flowing lava, the ocean, steampunk brass and
// gears, the cyberpunk city, the dieselpunk deco panels, the Frutiger Aero sky).
//
// Cost: rendered at half the window's resolution (device pixel ratio capped at 1.5) and
// stretched by the compositor, 24 frames a second by default (the frame rate setting,
// D-274, raises it), one draw call. Stops when hidden or minimised; a still frame under
// reduced motion. Where WebGL is missing or the context is lost, `onFail` lets the frame
// fall back to the cabinet's 2D look.

import { useEffect, useRef } from "preact/hooks";
import { useEffectFps } from "../lib/fxrate";
import { frameLoop, glContext, glProgram } from "./gl";
import { CABINET_FS, type GpuCabinet } from "./shaders";

const FPS = 24;
/** Buffer pixels per CSS pixel (before the device pixel ratio, itself capped at 1.5). */
const RES = 0.5;

const VS = `
attribute vec2 a_pos;
void main() {
  gl_Position = vec4(a_pos, 0.0, 1.0);
}
`;

export interface CabinetGLProps {
  kind: GpuCabinet;
  /** Size of the cabinet in CSS px (the canvas is stretched over it). */
  width: number;
  height: number;
  /** False stops the loop (hidden page, minimised window). */
  running: boolean;
  /** False: one still frame (reduced motion). */
  animate: boolean;
  /** WebGL is missing or was lost: the frame falls back to the 2D cabinet. */
  onFail: () => void;
  class?: string;
}

export function CabinetGL({ kind, width, height, running, animate, onFail, class: className }: CabinetGLProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fps = useEffectFps(FPS);
  /** Time carried across pauses and resizes, so the motion resumes where it stopped. */
  const timeRef = useRef(41);
  const failRef = useRef(onFail);
  failRef.current = onFail;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || width < 2 || height < 2) return;
    const dpr = typeof devicePixelRatio === "number" && devicePixelRatio > 0 ? devicePixelRatio : 1;
    const k = Math.min(dpr, 1.5) * RES;
    canvas.width = Math.max(2, Math.round(width * k));
    canvas.height = Math.max(2, Math.round(height * k));
    const gl = glContext(canvas, false);
    const prog = gl ? glProgram(gl, VS, CABINET_FS[kind]) : null;
    if (!gl || !prog) {
      failRef.current();
      return;
    }
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    gl.useProgram(prog);
    const loc = gl.getAttribLocation(prog, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.uniform2f(gl.getUniformLocation(prog, "u_res"), canvas.width, canvas.height);
    const uTime = gl.getUniformLocation(prog, "u_time");
    const t0 = timeRef.current;
    const draw = (s: number) => {
      const t = (t0 + s) % 3600;
      timeRef.current = t;
      gl.uniform1f(uTime, t);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    };
    let stop = () => {};
    const lost = (e: Event) => {
      e.preventDefault();
      stop();
      failRef.current();
    };
    canvas.addEventListener("webglcontextlost", lost);
    if (running && animate) stop = frameLoop(fps, draw);
    else draw(0);
    return () => {
      stop();
      canvas.removeEventListener("webglcontextlost", lost);
      if (!gl.isContextLost()) {
        gl.deleteBuffer(buf);
        gl.deleteProgram(prog);
      }
    };
  }, [kind, width, height, running, animate, fps]);

  return <canvas ref={canvasRef} class={className} aria-hidden="true" />;
}

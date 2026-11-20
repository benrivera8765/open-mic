// SPDX-License-Identifier: GPL-3.0-or-later
// The practice lane on screen: owns the canvas, the layout (lane.ts), the renderer
// (lanerenderer.ts) and the frame loop. Same rules as the lyric displays (frameloop.ts):
// driven by the playback clock, at most 60 frames a second, stopped when nothing moves and
// while the window is hidden or minimised. It also wakes up for practice://live batches
// (the marker moves while the song is paused, WO2 12.2) and key-heard changes.
//
// Debug hooks for the screenshot script and the real-app checks: `window.__jkLane`
// (stats, the layout's numbers, the last frame, verify(), probe()).
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Lyrics, NoteSpelling, OctaveMode, PracticeGuide } from "../api/types";
import { clock, getApiSync, onClockKick } from "../state/app";
import { keys, live, liveMs, onPracticeKick } from "../state/practice";
import { FrameLoop, type MinimisedSource } from "../ticker/frameloop";
import type { TickerStats } from "../ticker/renderer";
import { type LaneFrame, type LaneLayout, layoutLane, type Measure } from "./lane";
import { LaneRenderer, type LaneStyle } from "./lanerenderer";
import { rasterBench } from "./lanecost";
import { type LaneScenario, verifyLane } from "./laneverify";

export interface PracticeLaneProps {
  guide: PracticeGuide;
  lyrics: Lyrics | null;
  duration: number;
  /** The lane's now = clock position - this (the device offset only, APP_API 12.7), s. */
  offsetS: number;
  pianoSide: "left" | "right";
  words: boolean;
  simple: boolean;
  reduceMotion: boolean;
  watchOnly: boolean;
  octaveMode: OctaveMode;
  spelling: NoteSpelling;
  colors: { sung: string; upcoming: string; marker: string };
}

declare global {
  interface Window {
    __jkLane?: {
      stats: TickerStats;
      layout: () => Pick<LaneLayout, "barX" | "barW" | "barTop" | "barBottom" | "nowX" | "pxPerSec" | "rowH" | "rows" | "barH" | "mode" | "headH" | "tailW" | "pianoW" | "wordRows"> & { width: number; height: number };
      last: () => LaneFrame | null;
      verify: (sc: LaneScenario) => ReturnType<typeof verifyLane>;
      /**
       * Draws `frames` frames at `step` s from `from` without the clock (cost measurement).
       * `raster`: on an offscreen CPU canvas of the same size, each frame rasterised inside
       * the timed span (lanecost.ts); otherwise on the lane's own canvas (commands only).
       */
      bench: (from: number, frames: number, step: number, simple?: boolean, raster?: boolean) => { avg: number; p95: number; max: number; frames: number };
    };
  }
}

function windowMinimisedSource(): MinimisedSource | undefined {
  try {
    const api = getApiSync();
    return api.onWindowMinimised ? (cb) => api.onWindowMinimised!(cb) : undefined;
  } catch {
    return undefined;
  }
}

export function PracticeLane(props: PracticeLaneProps) {
  const { guide, lyrics, duration, offsetS, pianoSide, words, simple, reduceMotion, watchOnly, octaveMode, spelling, colors } = props;
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<LaneRenderer | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0, dpr: 1, winW: 1440, winH: 900 });
  const [fontEpoch, setFontEpoch] = useState(0);
  const offsetRef = useRef(offsetS);
  offsetRef.current = offsetS;
  // Inputs the frame reads that change without a new layout.
  const dyn = useRef({ watchOnly, octaveMode, spelling, reduceMotion, simple });
  dyn.current = { watchOnly, octaveMode, spelling, reduceMotion, simple };

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setSize({ w: Math.round(r.width), h: Math.round(r.height), dpr: window.devicePixelRatio || 1, winW: window.innerWidth, winH: window.innerHeight });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const fonts = document.fonts;
    if (!fonts) return;
    const bump = () => setFontEpoch((n) => n + 1);
    void fonts.ready.then(bump);
    fonts.addEventListener?.("loadingdone", bump);
    return () => fonts.removeEventListener?.("loadingdone", bump);
  }, []);

  const style: LaneStyle | null = useMemo(() => {
    const el = wrapRef.current;
    if (!el) return null;
    const cs = getComputedStyle(el);
    return {
      sung: colors.sung,
      marker: colors.marker,
      upcoming: colors.upcoming,
      bar: cs.getPropertyValue("--lane-bar").trim() || "#ff3d8b",
      wordFont: cs.getPropertyValue("--ticker-font").trim() || "system-ui, sans-serif",
      wordWeight: cs.getPropertyValue("--lane-word-weight").trim() || "700",
      labelFont: cs.getPropertyValue("--lane-label-font").trim() || "system-ui, sans-serif",
    };
  }, [colors.sung, colors.marker, colors.upcoming, size.w > 0, fontEpoch]);

  const layout: LaneLayout | null = useMemo(() => {
    if (!style || size.w <= 0 || size.h <= 0) return null;
    const scratch = document.createElement("canvas").getContext("2d");
    if (!scratch) return null;
    const measure: Measure = (text, px) => {
      scratch.font = `${style.wordWeight} ${px}px ${style.wordFont}`;
      return scratch.measureText(text).width;
    };
    const scale = Math.min(size.winW / 1440, size.winH / 900);
    return layoutLane(guide, lyrics, duration, { width: size.w, height: size.h, pianoSide, words, scale, dpr: size.dpr }, measure);
  }, [guide, lyrics, duration, size.w, size.h, size.dpr, size.winW, size.winH, pianoSide, words, style, fontEpoch]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    try {
      rendererRef.current = new LaneRenderer(canvas);
    } catch {
      rendererRef.current = null;
    }
    return () => {
      rendererRef.current = null;
    };
  }, []);

  const loopRef = useRef<FrameLoop | null>(null);
  if (!loopRef.current) {
    loopRef.current = new FrameLoop({
      requestFrame: (cb) => requestAnimationFrame(cb),
      cancelFrame: (id) => cancelAnimationFrame(id),
      doc: typeof document === "undefined" ? null : document,
      onWindowMinimised: windowMinimisedSource(),
    });
  }
  useEffect(() => {
    const fl = loopRef.current!;
    fl.attach();
    return () => fl.detach();
  }, []);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r || !layout || !style) return;
    r.setSize(size.w, size.h, size.dpr);
    r.setLayout(layout, style, simple);
    const inputsAt = (now: number, t: number) => ({
      t,
      nowMs: now,
      keys: keys.view(),
      spelling: dyn.current.spelling,
      octaveMode: dyn.current.octaveMode,
      live,
      liveMs,
      watchOnly: dyn.current.watchOnly,
      reduceMotion: dyn.current.reduceMotion,
      simple: dyn.current.simple,
    });
    window.__jkLane = {
      stats: r.stats,
      layout: () => ({
        width: size.w,
        height: size.h,
        barX: layout.barX,
        barW: layout.barW,
        barTop: layout.barTop,
        barBottom: layout.barBottom,
        nowX: layout.nowX,
        pxPerSec: layout.pxPerSec,
        rowH: layout.rowH,
        rows: layout.rows,
        barH: layout.barH,
        mode: layout.mode,
        headH: layout.headH,
        tailW: layout.tailW,
        pianoW: layout.pianoW,
        wordRows: layout.wordRows,
      }),
      last: () => r.last,
      verify: (sc) => verifyLane(layout, guide, lyrics, sc),
      bench: (from, frames, step, simpleLook, raster) => {
        const look = simpleLook ?? dyn.current.simple;
        if (raster) {
          const res = rasterBench(layout, style, look, size.dpr, frames, (k) => ({ ...inputsAt(performance.now(), from + k * step), simple: look }));
          return res ?? { avg: Number.NaN, p95: Number.NaN, max: Number.NaN, frames: 0 };
        }
        if (simpleLook !== undefined) r.setLayout(layout, style, simpleLook);
        const times: number[] = [];
        for (let k = 0; k < frames; k++) {
          const t0 = performance.now();
          r.draw({ ...inputsAt(performance.now(), from + k * step), simple: look });
          times.push(performance.now() - t0);
        }
        if (simpleLook !== undefined) r.setLayout(layout, style, dyn.current.simple);
        times.sort((a, b) => a - b);
        const avg = times.reduce((a, b) => a + b, 0) / Math.max(1, times.length);
        return { avg, p95: times[Math.floor(times.length * 0.95)] ?? 0, max: times[times.length - 1] ?? 0, frames: times.length };
      },
    };
    const fl = loopRef.current!;
    fl.draw = (now) => {
      const t = clock.position(now) - offsetRef.current;
      const f = r.draw(inputsAt(now, t));
      // Keep drawing while the clock runs, a slide is under way, or the marker is live.
      return clock.isAnimating(now) || !!f?.animating || now - liveMs < 400;
    };
    fl.kick();
    const off1 = onClockKick(fl.kick);
    const off2 = onPracticeKick(fl.kick);
    return () => {
      off1();
      off2();
      fl.draw = null;
      fl.stop();
    };
  }, [layout, style, size.w, size.h, size.dpr, simple, guide, lyrics]);

  // Settings that need only a new frame.
  useEffect(() => loopRef.current?.kick(), [offsetS, watchOnly, octaveMode, spelling, reduceMotion]);

  return (
    <div class={`practice-lane practice-lane--piano-${pianoSide}`} ref={wrapRef}>
      <canvas class="practice-lane__canvas" ref={canvasRef} aria-hidden="true" />
    </div>
  );
}

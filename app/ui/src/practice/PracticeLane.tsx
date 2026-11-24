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
import type { CelebrateFreq, CelebrationKind, Lyrics, NoteSpelling, OctaveMode, PitchFx, PracticeGuide } from "../api/types";
import { clock, getApiSync, onClockKick } from "../state/app";
import { keys, live, liveMs, onPracticeKick } from "../state/practice";
import { FrameLoop, type MinimisedSource } from "../ticker/frameloop";
import type { TickerStats } from "../ticker/renderer";
import { barOf, firstNoteFrom, type LaneFrame, type LaneLayout, layoutLane, type Measure } from "./lane";
import { CelebrationFx, CelebrationGate, NEAR_PERFECT_SHARE, PERFECT_SHARE, PHRASE_GAP_S } from "./celebration";
import { LaneRenderer, type LaneStyle } from "./lanerenderer";
import { rasterBench } from "./lanecost";
import { type LaneScenario, verifyLane } from "./laneverify";

export interface PracticeLaneProps {
  guide: PracticeGuide;
  lyrics: Lyrics | null;
  /** The song's lyrics offset (s): the word strip matches the lyrics shown below. */
  lyricsOffsetS?: number;
  duration: number;
  /** The lane's now = clock position - this (the device offset only, APP_API 12.7), s. */
  offsetS: number;
  pianoSide: "left" | "right";
  words: boolean;
  simple: boolean;
  reduceMotion: boolean;
  watchOnly: boolean;
  /** Watch only: a perfect voice follows the melody (settings ui.look.watch_follow). */
  follow?: boolean;
  /** Celebrate perfect phrases (ui.look.celebrate), and how. */
  celebrate?: boolean;
  celebration?: CelebrationKind;
  /** How often (ui.look.celebrate_freq); every perfect phrase when absent. */
  celebrateFreq?: CelebrateFreq;
  /** The effect off the pitch cursor while on pitch (ui.look.pitch_fx). */
  pitchFx?: PitchFx;
  octaveMode: OctaveMode;
  spelling: NoteSpelling;
  colors: { sung: string; upcoming: string; marker: string };
  /** Note bar colour (ui.look.lane_bars); the stylesheet's --lane-bar when absent. */
  barColor?: string;
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
  const { guide, lyrics, lyricsOffsetS = 0, duration, offsetS, pianoSide, words, simple, reduceMotion, watchOnly, octaveMode, spelling, colors } = props;
  const follow = props.follow ?? false;
  const celebrate = props.celebrate ?? false;
  const celebration = props.celebration ?? "mix";
  const pitchFx = props.pitchFx ?? "off";
  const celebrateFreq = props.celebrateFreq ?? "every";
  const gateRef = useRef(new CelebrationGate());
  /** performance.now() of the last frame that could emit the on-pitch effect. */
  const fxMsRef = useRef(0);
  const fxCanvasRef = useRef<HTMLCanvasElement>(null);
  const fxRef = useRef<CelebrationFx | null>(null);
  /** Lane time of the previous frame, to find the phrase ends passed since. */
  const lastTRef = useRef(Number.NaN);
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<LaneRenderer | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0, dpr: 1, winW: 1440, winH: 900 });
  const [fontEpoch, setFontEpoch] = useState(0);
  const offsetRef = useRef(offsetS);
  offsetRef.current = offsetS;
  // Inputs the frame reads that change without a new layout.
  const dyn = useRef({ watchOnly, octaveMode, spelling, reduceMotion, simple, follow, celebrate, celebration, pitchFx, celebrateFreq });
  dyn.current = { watchOnly, octaveMode, spelling, reduceMotion, simple, follow, celebrate, celebration, pitchFx, celebrateFreq };

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
      bar: props.barColor || cs.getPropertyValue("--lane-bar").trim() || "#ff3d8b",
      wordFont: cs.getPropertyValue("--ticker-font").trim() || "system-ui, sans-serif",
      wordWeight: cs.getPropertyValue("--lane-word-weight").trim() || "700",
      labelFont: cs.getPropertyValue("--lane-label-font").trim() || "system-ui, sans-serif",
    };
  }, [colors.sung, colors.marker, colors.upcoming, props.barColor, size.w > 0, fontEpoch]);

  const layout: LaneLayout | null = useMemo(() => {
    if (!style || size.w <= 0 || size.h <= 0) return null;
    const scratch = document.createElement("canvas").getContext("2d");
    if (!scratch) return null;
    const measure: Measure = (text, px) => {
      scratch.font = `${style.wordWeight} ${px}px ${style.wordFont}`;
      return scratch.measureText(text).width;
    };
    const scale = Math.min(size.winW / 1440, size.winH / 900);
    return layoutLane(guide, lyrics, duration, { width: size.w, height: size.h, pianoSide, words, scale, dpr: size.dpr }, measure, lyricsOffsetS);
  }, [guide, lyrics, lyricsOffsetS, duration, size.w, size.h, size.dpr, size.winW, size.winH, pianoSide, words, style, fontEpoch]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    try {
      rendererRef.current = new LaneRenderer(canvas);
    } catch {
      rendererRef.current = null;
    }
    const fxCanvas = fxCanvasRef.current;
    if (fxCanvas) {
      try {
        fxRef.current = new CelebrationFx(fxCanvas);
        (window as unknown as { __jkCelebrate?: unknown }).__jkCelebrate = fxRef.current;
      } catch {
        fxRef.current = null;
      }
    }
    return () => {
      rendererRef.current = null;
      fxRef.current?.stop();
      fxRef.current = null;
    };
  }, []);

  // Celebrations: off -> stop any burst still flying.
  useEffect(() => {
    if (!celebrate || reduceMotion) fxRef.current?.stop();
  }, [celebrate, reduceMotion]);

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
    fxRef.current?.setSize(size.w, size.h, size.dpr, style.labelFont);
    lastTRef.current = Number.NaN;
    // Lyric line starts (song time): a phrase also ends where the next note starts a new line,
    // since many songs run their lines together without a rest.
    const lineStarts = (lyrics?.lines ?? []).map((ln) => ln.start + lyricsOffsetS).sort((a, b) => a - b);
    const lineOf = (t: number) => {
      let lo = 0;
      let hi = lineStarts.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (lineStarts[mid] <= t + 0.08) lo = mid + 1;
        else hi = mid;
      }
      return lo;
    };
    /** True when notes a and b (a before b) belong to different phrases. */
    const breakBetween = (a: { start: number; end: number }, b: { start: number }) => b.start - a.end >= PHRASE_GAP_S || lineOf(b.start) > lineOf(a.start);
    /** Fires a celebration when a phrase that was sung perfectly (or followed in watch only) ends. */
    const celebrateAt = (t: number, f: LaneFrame | null) => {
      const prev = lastTRef.current;
      lastTRef.current = t;
      const d = dyn.current;
      const fx = fxRef.current;
      if (!fx || !f || !d.celebrate || d.reduceMotion) return;
      const followOn = d.watchOnly && d.follow;
      if (d.watchOnly && !followOn) return;
      // Only while playing forward at normal speed: never on a seek or a jump.
      if (!(t > prev) || t - prev > 0.5) return;
      const notes = layout.notes;
      for (let k = firstNoteFrom(layout, prev); k < notes.length && notes[k].end <= t; k++) {
        const n = notes[k];
        if (n.end <= prev) continue;
        const next = notes[k + 1];
        if (next && !breakBetween(n, next)) continue;
        // The phrase: back from this note to the previous rest.
        let graded = 0;
        let onPitch = 0;
        for (let q = k; q >= 0; q--) {
          const m = notes[q];
          if (q < k && breakBetween(m, notes[q + 1])) break;
          if (m.kind !== "pitched" || !m.graded || m.dim) continue;
          graded += m.end - m.start;
          if (followOn) continue;
          const h = live?.hitsOf(m.i);
          if (!h) continue;
          for (let j = 0; j + 2 < h.length; j += 3) {
            if (h[j + 2] !== 0) continue;
            onPitch += Math.max(0, Math.min(h[j + 1], m.end) - Math.max(h[j], m.start));
          }
        }
        if (graded < 0.5) continue;
        if (!followOn && onPitch / graded < (d.celebrateFreq === "often" ? NEAR_PERFECT_SHARE : PERFECT_SHARE)) continue;
        if (gateRef.current.allow(d.celebrateFreq, performance.now())) fx.burst(d.celebration, layout.nowX, barOf(layout, f, k).y);
        break;
      }
    };
    const inputsAt = (now: number, t: number) => ({
      t,
      nowMs: now,
      keys: keys.view(),
      spelling: dyn.current.spelling,
      octaveMode: dyn.current.octaveMode,
      live,
      liveMs,
      watchOnly: dyn.current.watchOnly,
      follow: dyn.current.follow,
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
      celebrateAt(t, f);
      // On pitch on a graded note: the effect comes off the cursor (owner, 2026-11-23).
      const dtFx = (now - fxMsRef.current) / 1000;
      fxMsRef.current = now;
      const fx = fxRef.current;
      const kind = dyn.current.pitchFx;
      const sg = f?.singer;
      if (fx && f && sg && kind !== "off" && !dyn.current.reduceMotion && sg.edge === 0 && sg.cue === 0 && f.target?.graded) {
        fx.trail(kind, layout.nowX, sg.y, dtFx, layout.barH);
      }
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
  }, [layout, style, size.w, size.h, size.dpr, simple, guide, lyrics, lyricsOffsetS]);

  // Settings that need only a new frame.
  useEffect(() => loopRef.current?.kick(), [offsetS, watchOnly, follow, octaveMode, spelling, reduceMotion]);

  return (
    <div class={`practice-lane practice-lane--piano-${pianoSide}`} ref={wrapRef}>
      <canvas class="practice-lane__canvas" ref={canvasRef} aria-hidden="true" />
      <canvas class="practice-lane__fx" ref={fxCanvasRef} aria-hidden="true" />
    </div>
  );
}

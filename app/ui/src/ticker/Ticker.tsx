// SPDX-License-Identifier: GPL-3.0-or-later
// The lyrics ticker component: owns the canvas, the layout (layout.ts), the renderer
// (renderer.ts) and the frame loop.
//
// Frame loop rules (WORKING ORDER 4.4 / 4.7, the fanless MacBook Air):
//   - driven by the playback clock (state/app.ts `clock`), never by a timer;
//   - at most 60 frames per second, even on 120 Hz displays;
//   - stops completely when nothing moves (paused, no glide in progress) and while
//     the window is hidden or minimised (frameloop.ts); clock events, seeks, resizes
//     and setting changes restart it.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Lyrics, LyricsMotion } from "../api/types";
import { lineIndexAt, lineText } from "../lib/lyrics";
import { clock, getApiSync, onClockKick } from "../state/app";
import { FrameLoop, type MinimisedSource } from "./frameloop";
import { layoutTicker, type Measure, type TickerLayout } from "./layout";
import { TickerRenderer, type TickerStats, type TickerStyle } from "./renderer";
import { type TickerVerifyReport, verifyTickerLayout } from "./verify";
import type { PageProbe } from "./PageLyrics";

export interface TickerProps {
  lyrics: Lyrics;
  /** Total lyric offset in seconds: (song offset + device offset) / 1000. */
  offsetS: number;
  /** settings.ui.lyrics_text_size: CSS px at a 1440x900 window. */
  textSize: number;
  /** settings.ui.marker_position, 0..1. */
  marker: number;
  /** settings.ui.lyrics_motion: "word" (smooth, paced by word; default) or "letter" (D-216). */
  pace: LyricsMotion;
  colors: { sung: string; upcoming: string; marker: string };
}

/** The native window's minimised signal (Tauri), or undefined (browser mock, tests). */
function windowMinimisedSource(): MinimisedSource | undefined {
  try {
    const api = getApiSync();
    return api.onWindowMinimised ? (cb) => api.onWindowMinimised!(cb) : undefined;
  } catch {
    return undefined; // no API yet
  }
}

/** One drawn frame, as recorded by `window.__jkTicker.trace`. */
export interface TickerFrame {
  /** performance.now() of the frame, ms. */
  now: number;
  /** Lyric time drawn, s. */
  t: number;
  /** Strip position under the marker, CSS px. */
  x: number;
  /** Screen x of the strip's origin (marker x - strip x), CSS px: what moves on screen. Page display: the wipe edge's x. */
  offset: number;
  /** Page display: the line in the current slot (-1 in rests). */
  line?: number;
  /** Page display: the progress needle's x as drawn (NaN when none was drawn), and the x where its row was split between the sung and the upcoming colour (NaN when the row was drawn in one colour). */
  needle?: number;
  split?: number;
  /** Page display: the needle's row (-1 without a needle) and the scale of the line in the current slot (1 once it has settled). */
  needleRow?: number;
  scale?: number;
  /** Page display: the wipe edge's x from the current line's motion, clamped to its text and scaled with it (NaN without a line in the slot). */
  edge?: number;
}

/** Frame trace for the real-app scripts; off (and free) unless a script starts it. */
const trace = { on: false, max: 0, frames: [] as TickerFrame[] };

/** Debug/perf hook read by the headless screenshot script (scripts/screenshots.mjs) and the real-app scripts. */
declare global {
  interface Window {
    __jkTicker?: {
      stats: TickerStats;
      pace: LyricsMotion;
      /** Which display drew it: the scrolling ticker or the page display (PageLyrics.tsx). */
      display?: "scroll" | "page";
      fontPx: number;
      fontScale: number;
      minLineScale: number;
      denseLines: number;
      glyphs: number;
      worstLookahead: number;
      /** Runs the letter (or, for the word pace, word) under-the-marker check on the live layout (real fonts). */
      verify: () => TickerVerifyReport;
      /** Records every drawn frame from now on (at most `maxFrames`, default 10 minutes at 60 fps). */
      traceStart: (maxFrames?: number) => void;
      /** Stops recording and returns the frames recorded. */
      traceTake: () => TickerFrame[];
      /** Page display: what the wipe shows at lyric time t (default: now). */
      probe?: (t?: number) => PageProbe;
      /** Page display: the lines as laid out (size, rows, schedule, words). */
      pageLines?: () => unknown[];
    };
  }
}

export function Ticker({ lyrics, offsetS, textSize, marker, pace, colors }: TickerProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<TickerRenderer | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0, dpr: 1, winW: 1440, winH: 900 });
  const [fontEpoch, setFontEpoch] = useState(0);
  const [lineIdx, setLineIdx] = useState(-1);
  const [noCanvas, setNoCanvas] = useState(false);
  const offsetRef = useRef(offsetS);
  offsetRef.current = offsetS;
  const lineRef = useRef(-1);

  // Size and pixel ratio (ResizeObserver + a resolution media query for DPR changes).
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
    let mq: MediaQueryList | null = null;
    const onDpr = () => {
      measure();
      mq?.removeEventListener("change", onDpr);
      mq = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      mq.addEventListener("change", onDpr);
    };
    onDpr();
    return () => {
      ro.disconnect();
      mq?.removeEventListener("change", onDpr);
    };
  }, []);

  // Web fonts: measure again once they have loaded.
  useEffect(() => {
    const fonts = document.fonts;
    if (!fonts) return;
    const bump = () => setFontEpoch((n) => n + 1);
    void fonts.ready.then(bump);
    fonts.addEventListener?.("loadingdone", bump);
    return () => fonts.removeEventListener?.("loadingdone", bump);
  }, []);

  // Font family and weight come from CSS (--ticker-font, --ticker-weight), so a skin can change them.
  const style: TickerStyle | null = useMemo(() => {
    const el = wrapRef.current;
    if (!el) return null;
    const cs = getComputedStyle(el);
    return {
      fontFamily: cs.getPropertyValue("--ticker-font").trim() || "system-ui, -apple-system, 'Segoe UI', sans-serif",
      fontWeight: cs.getPropertyValue("--ticker-weight").trim() || "700",
      sung: colors.sung,
      upcoming: colors.upcoming,
      marker: colors.marker,
    };
    // size: the element exists once it has a size; fontEpoch: CSS may have changed.
  }, [colors.sung, colors.upcoming, colors.marker, size.w > 0, fontEpoch]);

  const layout: TickerLayout | null = useMemo(() => {
    if (!style || size.w <= 0) return null;
    const scratch = document.createElement("canvas").getContext("2d");
    if (!scratch) return null;
    const measure: Measure = (text, px) => {
      scratch.font = `${style.fontWeight} ${px}px ${style.fontFamily}`;
      return scratch.measureText(text).width;
    };
    const scale = Math.min(size.winW / 1440, size.winH / 900);
    return layoutTicker(lyrics, { width: size.w, marker, fontPx: Math.max(12, textSize * scale), pace }, measure);
  }, [lyrics, size.w, size.winW, size.winH, marker, textSize, pace, style, fontEpoch]);

  // Renderer and frame loop.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    try {
      rendererRef.current = new TickerRenderer(canvas);
    } catch {
      // No Canvas 2D (should not happen in a supported web view): show plain text instead.
      setNoCanvas(true);
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
  // Follows page visibility and the window's minimised state while mounted.
  useEffect(() => {
    const fl = loopRef.current!;
    fl.attach();
    return () => fl.detach();
  }, []);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r || !layout || !style) return;
    r.setSize(size.w, size.h, size.dpr);
    r.setLayout(layout, style);
    window.__jkTicker = {
      stats: r.stats,
      pace: layout.pace,
      display: "scroll",
      fontPx: layout.fontPx,
      fontScale: layout.fontScale,
      minLineScale: layout.minLineScale,
      denseLines: layout.denseLines,
      glyphs: layout.glyphs.length,
      worstLookahead: layout.worstLookahead,
      verify: () => verifyTickerLayout(layout),
      traceStart: (maxFrames = 36000) => {
        trace.frames = [];
        trace.max = maxFrames;
        trace.on = true;
      },
      traceTake: () => {
        trace.on = false;
        const out = trace.frames;
        trace.frames = [];
        return out;
      },
    };
    const fl = loopRef.current!;
    fl.draw = (now) => {
      const t = clock.position(now) - offsetRef.current;
      r.draw(t);
      if (trace.on && trace.frames.length < trace.max) {
        const x = layout.motion.at(t);
        trace.frames.push({ now, t, x, offset: size.w * layout.marker - x });
      }
      // Screen-reader line (changes once per line, not per frame).
      const li = lineIndexAt(lyrics, t);
      if (li !== lineRef.current) {
        lineRef.current = li;
        setLineIdx(li);
      }
      return clock.isAnimating(now);
    };
    fl.kick();
    const off = onClockKick(fl.kick);
    return () => {
      off();
      fl.draw = null;
      fl.stop();
    };
  }, [layout, style, size.w, size.h, size.dpr, lyrics]);

  // Offset changes move the text while paused: draw again.
  useEffect(() => loopRef.current?.kick(), [offsetS]);

  // Without a canvas, follow the current line with a slow timer instead of the frame loop.
  useEffect(() => {
    if (!noCanvas) return;
    const h = setInterval(() => setLineIdx(lineIndexAt(lyrics, clock.position(performance.now()) - offsetRef.current)), 250);
    return () => clearInterval(h);
  }, [noCanvas, lyrics]);

  const current = lineIdx >= 0 ? lyrics.lines[lineIdx] : null;
  return (
    <div class="ticker" ref={wrapRef}>
      <canvas class="ticker__canvas" ref={canvasRef} aria-hidden="true" />
      <p class={noCanvas ? "ticker__fallback" : "visually-hidden"} aria-live="polite">
        {current ? lineText(current) : ""}
      </p>
    </div>
  );
}

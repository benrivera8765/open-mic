// SPDX-License-Identifier: GPL-3.0-or-later
// The lyrics ticker component: owns the canvas, the layout (layout.ts), the renderer
// (renderer.ts) and the frame loop.
//
// Frame loop rules (WORKING ORDER 4.4 / 4.7, the fanless MacBook Air):
//   - driven by the playback clock (state/app.ts `clock`), never by a timer;
//   - at most 60 frames per second, even on 120 Hz displays;
//   - stops completely when nothing moves (paused, no glide in progress) and while
//     the window is hidden; clock events, seeks, resizes and setting changes restart it.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Lyrics } from "../api/types";
import { lineIndexAt, lineText } from "../lib/lyrics";
import { clock, onClockKick } from "../state/app";
import { layoutTicker, type Measure, type TickerLayout } from "./layout";
import { TickerRenderer, type TickerStats, type TickerStyle } from "./renderer";
import { type TickerVerifyReport, verifyTickerLayout } from "./verify";

export interface TickerProps {
  lyrics: Lyrics;
  /** Total lyric offset in seconds: (song offset + device offset) / 1000. */
  offsetS: number;
  /** settings.ui.lyrics_text_size: CSS px at a 1440x900 window. */
  textSize: number;
  /** settings.ui.marker_position, 0..1. */
  marker: number;
  colors: { sung: string; upcoming: string; marker: string };
}

const FRAME_MS = 1000 / 60;
const MIN_FRAME_MS = FRAME_MS - 2; // 60 fps cap with a little slack for timer jitter

/** Debug/perf hook read by the headless screenshot script (scripts/screenshots.mjs). */
declare global {
  interface Window {
    __jkTicker?: {
      stats: TickerStats;
      fontPx: number;
      fontScale: number;
      minLineScale: number;
      denseLines: number;
      glyphs: number;
      worstLookahead: number;
      /** Runs the letter-under-the-marker check on the live layout (real fonts). */
      verify: () => TickerVerifyReport;
    };
  }
}

export function Ticker({ lyrics, offsetS, textSize, marker, colors }: TickerProps) {
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
    return layoutTicker(lyrics, { width: size.w, marker, fontPx: Math.max(12, textSize * scale) }, measure);
  }, [lyrics, size.w, size.winW, size.winH, marker, textSize, style, fontEpoch]);

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

  const loop = useRef({ raf: 0, last: 0 });
  const kickRef = useRef<() => void>(() => undefined);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r || !layout || !style) return;
    r.setSize(size.w, size.h, size.dpr);
    r.setLayout(layout, style);
    window.__jkTicker = {
      stats: r.stats,
      fontPx: layout.fontPx,
      fontScale: layout.fontScale,
      minLineScale: layout.minLineScale,
      denseLines: layout.denseLines,
      glyphs: layout.glyphs.length,
      worstLookahead: layout.worstLookahead,
      verify: () => verifyTickerLayout(layout),
    };
    const st = loop.current;
    const frame = (now: number) => {
      st.raf = 0;
      if (document.hidden) return;
      if (now - st.last < MIN_FRAME_MS) {
        st.raf = requestAnimationFrame(frame);
        return;
      }
      // Advance the 60 fps schedule by whole frame periods instead of jumping to `now`:
      // with `st.last = now` a 75 Hz screen drew every second refresh (37.5 fps) and a
      // 144 Hz one 48 fps; this keeps 60 fps there too and never more on average.
      st.last += FRAME_MS * Math.max(1, Math.floor((now - st.last) / FRAME_MS));
      const t = clock.position(now) - offsetRef.current;
      r.draw(t);
      // Screen-reader line (changes once per line, not per frame).
      const li = lineIndexAt(lyrics, t);
      if (li !== lineRef.current) {
        lineRef.current = li;
        setLineIdx(li);
      }
      if (clock.isAnimating(now)) st.raf = requestAnimationFrame(frame);
    };
    const kick = () => {
      if (!st.raf && !document.hidden) st.raf = requestAnimationFrame(frame);
    };
    kickRef.current = kick;
    kick();
    const off = onClockKick(kick);
    const onVis = () => {
      if (document.hidden) {
        cancelAnimationFrame(st.raf);
        st.raf = 0;
      } else kick();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      off();
      document.removeEventListener("visibilitychange", onVis);
      cancelAnimationFrame(st.raf);
      st.raf = 0;
    };
  }, [layout, style, size.w, size.h, size.dpr, lyrics]);

  // Offset changes move the text while paused: draw again.
  useEffect(() => kickRef.current(), [offsetS]);

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

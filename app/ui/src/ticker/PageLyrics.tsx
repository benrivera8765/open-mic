// SPDX-License-Identifier: GPL-3.0-or-later
// The page display of the lyrics (settings.ui.lyrics_display = "page", D-218): owns the
// canvas, the layout (page.ts), the renderer (pagerenderer.ts) and the frame loop. Same
// frame-loop rules as the ticker (Ticker.tsx): driven by the playback clock, at most 60
// frames per second, stopped when nothing moves and while the window is hidden or
// minimised (frameloop.ts).
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Lyrics, LyricsMotion } from "../api/types";
import { lineIndexAt, lineText } from "../lib/lyrics";
import { clock, getApiSync, onClockKick } from "../state/app";
import { FrameLoop, type MinimisedSource } from "./frameloop";
import type { Measure } from "./layout";
import { currentIndexAt, layoutPage, type PageLayout, pageFrame } from "./page";
import { PageRenderer, type PageStyle } from "./pagerenderer";
import { verifyPage } from "./pageverify";
import type { TickerFrame } from "./Ticker";

export interface PageLyricsProps {
  lyrics: Lyrics;
  /** Total lyric offset in seconds: (song offset + device offset) / 1000. */
  offsetS: number;
  /** settings.ui.lyrics_text_size: CSS px at a 1440x900 window. */
  textSize: number;
  /** settings.ui.lyrics_motion: how the wipe moves ("word", default, or "letter"). */
  pace: LyricsMotion;
  /** settings.ui.show_next_line. */
  showNext: boolean;
  colors: { sung: string; upcoming: string };
}

function windowMinimisedSource(): MinimisedSource | undefined {
  try {
    const api = getApiSync();
    return api.onWindowMinimised ? (cb) => api.onWindowMinimised!(cb) : undefined;
  } catch {
    return undefined;
  }
}

const trace = { on: false, max: 0, frames: [] as TickerFrame[] };

/** What the wipe shows at lyric time t (for the real-app scripts' live check). */
export interface PageProbe {
  t: number;
  /** Line in the current slot, or -1. */
  line: number;
  text: string | null;
  /** Wipe position (reading order, CSS px of the line) and its x on screen. */
  wipe: number;
  edgeX: number | null;
  /** The word the wipe edge is on (or the space after it), and the word being sung by the timing (index in the line, text). */
  edgeWord: number;
  wordAtEdge: string | null;
  sungWord: number;
  wordSung: string | null;
  /** The sung word's start and end (s). */
  sungStart: number | null;
  sungEnd: number | null;
}

export function probePage(layout: PageLayout, t: number): PageProbe {
  const f = pageFrame(layout, t);
  const line = f.current >= 0 ? f.current : currentIndexAt(layout, t);
  const L = line >= 0 ? layout.lines[line] : null;
  if (!L) return { t, line: -1, text: null, wipe: 0, edgeX: null, edgeWord: -1, wordAtEdge: null, sungWord: -1, wordSung: null, sungStart: null, sungEnd: null };
  const wipe = L.motion.at(t);
  const word = (k: number) =>
    L.glyphs
      .slice(L.words[k].first, L.words[k].last + 1)
      .map((g) => g.g)
      .join("");
  let edgeWord = -1;
  let sungWord = -1;
  L.words.forEach((w, k) => {
    const nextA = k + 1 < L.words.length ? L.words[k + 1].a : L.uEnd;
    if (wipe >= w.a && wipe < nextA) edgeWord = k;
    if (t >= w.start && t < w.end) sungWord = k;
  });
  let edgeX: number | null = null;
  for (const row of L.rows) if (wipe >= row.u0 && wipe <= row.u1) edgeX = row.left + (wipe - row.u0);
  const sw = sungWord >= 0 ? L.words[sungWord] : null;
  return { t, line, text: L.text, wipe, edgeX, edgeWord, wordAtEdge: edgeWord >= 0 ? word(edgeWord) : null, sungWord, wordSung: sw ? word(sungWord) : null, sungStart: sw ? sw.start : null, sungEnd: sw ? sw.end : null };
}

export function PageLyrics({ lyrics, offsetS, textSize, pace, showNext, colors }: PageLyricsProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<PageRenderer | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0, dpr: 1, winW: 1440, winH: 900 });
  const [fontEpoch, setFontEpoch] = useState(0);
  const [lineIdx, setLineIdx] = useState(-1);
  const [noCanvas, setNoCanvas] = useState(false);
  const offsetRef = useRef(offsetS);
  offsetRef.current = offsetS;
  const lineRef = useRef(-1);

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

  useEffect(() => {
    const fonts = document.fonts;
    if (!fonts) return;
    const bump = () => setFontEpoch((n) => n + 1);
    void fonts.ready.then(bump);
    fonts.addEventListener?.("loadingdone", bump);
    return () => fonts.removeEventListener?.("loadingdone", bump);
  }, []);

  // Font family and weight from CSS (--ticker-font, --page-weight / --ticker-weight), so a skin can change them.
  const style: PageStyle | null = useMemo(() => {
    const el = wrapRef.current;
    if (!el) return null;
    const cs = getComputedStyle(el);
    return {
      fontFamily: cs.getPropertyValue("--ticker-font").trim() || "system-ui, -apple-system, 'Segoe UI', sans-serif",
      fontWeight: cs.getPropertyValue("--page-weight").trim() || cs.getPropertyValue("--ticker-weight").trim() || "700",
      sung: colors.sung,
      upcoming: colors.upcoming,
      outline: cs.getPropertyValue("--page-outline").trim() || "rgba(8, 6, 14, 0.92)",
      shadow: cs.getPropertyValue("--page-shadow").trim() || "rgba(0, 0, 0, 0.55)",
    };
  }, [colors.sung, colors.upcoming, size.w > 0, fontEpoch]);

  const layout: PageLayout | null = useMemo(() => {
    if (!style || size.w <= 0 || size.h <= 0) return null;
    const scratch = document.createElement("canvas").getContext("2d");
    if (!scratch) return null;
    const measure: Measure = (text, px) => {
      scratch.font = `${style.fontWeight} ${px}px ${style.fontFamily}`;
      return scratch.measureText(text).width;
    };
    const scale = Math.min(size.winW / 1440, size.winH / 900);
    return layoutPage(lyrics, { width: size.w, height: size.h, fontPx: Math.max(12, textSize * scale), pace, showNext }, measure);
  }, [lyrics, size.w, size.h, size.winW, size.winH, textSize, pace, showNext, style, fontEpoch]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    try {
      rendererRef.current = new PageRenderer(canvas);
    } catch {
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
      display: "page",
      fontPx: layout.fontPx,
      fontScale: layout.fontScale,
      minLineScale: layout.minLineScale,
      denseLines: layout.shrunkLines + layout.wrappedLines,
      glyphs: layout.glyphCount,
      worstLookahead: NaN,
      verify: () => verifyPage(layout),
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
      probe: (t?: number) => probePage(layout, t ?? clock.position(performance.now()) - offsetRef.current),
      pageLines: () =>
        layout.lines.map((L) => ({
          index: L.index,
          text: L.text,
          px: L.px,
          rows: L.rows.length,
          shrunk: L.shrunk,
          wrapped: L.wrapped,
          start: L.start,
          end: L.end,
          current: L.current,
          leave: L.leave,
          previewFrom: L.previewFrom,
          leadIn: L.leadIn,
          breakBefore: L.breakBefore,
          sourceLines: L.sourceLines,
          width: L.width,
          words: L.words.map((w) => ({ start: w.start, end: w.end, a: w.a, b: w.b, text: L.glyphs.slice(w.first, w.last + 1).map((g) => g.g).join("") })),
        })),
    };
    const fl = loopRef.current!;
    fl.draw = (now) => {
      const t = clock.position(now) - offsetRef.current;
      const f = r.draw(t);
      if (trace.on && trace.frames.length < trace.max && f) {
        const L = f.current >= 0 ? layout.lines[f.current] : null;
        const x = L ? L.motion.at(t) : NaN;
        let edge = NaN;
        if (L) for (const row of L.rows) if (x >= row.u0 && x <= row.u1) edge = row.left + (x - row.u0);
        trace.frames.push({ now, t, x, offset: edge, line: f.current });
      }
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

  useEffect(() => loopRef.current?.kick(), [offsetS]);

  useEffect(() => {
    if (!noCanvas) return;
    const h = setInterval(() => setLineIdx(lineIndexAt(lyrics, clock.position(performance.now()) - offsetRef.current)), 250);
    return () => clearInterval(h);
  }, [noCanvas, lyrics]);

  const current = lineIdx >= 0 ? lyrics.lines[lineIdx] : null;
  return (
    <div class="ticker page-lyrics" ref={wrapRef}>
      <canvas class="ticker__canvas" ref={canvasRef} aria-hidden="true" />
      <p class={noCanvas ? "ticker__fallback" : "visually-hidden"} aria-live="polite">
        {current ? lineText(current) : ""}
      </p>
    </div>
  );
}

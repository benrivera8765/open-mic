// SPDX-License-Identifier: GPL-3.0-or-later
// The jukebox frame round the whole window (WORK_ORDER 4.7, docs/DESIGN.md).
//
//   - an arched crest with a glowing translucent tube and an inner bubble tube,
//     a crest housing in the middle (the `crest` slot, normally the app Badge);
//   - a glowing column each side with three chrome rings and red enamel at the
//     top and a walnut base at the bottom;
//   - the screen (children) with a curved-glass sheen and two vertical light strips;
//   - the console along the bottom (the `console` slot: transport, key dial,
//     mixer) between two lattice grilles with compass-star medallions.
//
// How it stays cheap (MacBook Air: fanless, 8 GB):
//   - Layout is CSS grid sized with clamp(); JS only measures it (ResizeObserver)
//     and derives the decoration from one curve (geometry.ts).
//   - Static decoration (tube shading, rims, rings, bases, the room) is SVG
//     *images* built once per window size (frame-art.ts), and the walnut is a
//     bitmap generated once (wood.ts), so a repaint is a blit, never a re-filter.
//   - The drifting light is gradient strips inside fixed, masked regions (arch and
//     columns), moved with Web Animations `transform` animations: composited, no
//     repainted gradients, no animated blur. One palette period per 40 s, stepped
//     12 times a second (DRIFT_FPS). Nothing animates inside the screen.
//   - Bubbles: one canvas, <= 30 fps, <= 60 bubbles (Bubbles.tsx).
//   - Everything stops when the page is hidden or `paused`; motion "reduce"
//     stops the drift and removes the bubbles.

import type { ComponentChildren } from "preact";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "preact/hooks";
import { Bubbles } from "./Bubbles";
import { CrestOrnament } from "./CrestOrnament";
import { roomSvg, svgUrl, trimSvg, tubeSvg } from "./frame-art";
import { centreLinePath, computeFrameGeometry, type FrameGeometry } from "./geometry";
import { Grille } from "./Grille";
import { Medallion } from "./Medallion";
import { usePageVisible, type Motion } from "./motion";
import { walnutTexture } from "./wood";

export interface JukeboxFrameProps {
  /** Screen content (library, stage, ...). */
  children?: ComponentChildren;
  /** Console content (transport, KeyDial, Faders). */
  console: ComponentChildren;
  /** Crest content; the default music-note ornament when absent. Pass <Badge/>. */
  crest?: ComponentChildren;
  /** Bubble tubes on/off (app setting ui.bubbles). */
  bubbles: boolean;
  /** Resolved motion setting (see useResolvedMotion). "reduce" = no drift, no bubbles. */
  motion: Motion;
  /** Extra pause signal from the host, e.g. the native window is minimised or occluded. */
  paused?: boolean;
  class?: string;
}

/** One full trip through the palette, as asked for in DESIGN.md. */
export const LIGHT_CYCLE_MS = 40_000;

/**
 * The drift advances in steps, DRIFT_FPS times a second, instead of every display
 * frame. It moves ~45 px/s through a smooth gradient, so a 12 Hz step (~4 px on a
 * 1440 px window) is invisible, but the compositor only has to redraw the tubes
 * 12 times a second instead of 60 (measured in docs: tools/measure-skin.mjs).
 */
export const DRIFT_FPS = 12;

/** Palette stops along one period (DESIGN.md "Light and colour"). Warm colours get more room, as in photo 1. */
const PALETTE: readonly (readonly [string, number])[] = [
  ["#ff3d8b", 0],
  ["#ff2a2a", 0.13],
  ["#ff8a1f", 0.28],
  ["#ffd23f", 0.42],
  ["#4dff6a", 0.57],
  ["#7dffd8", 0.7],
  ["#9b6bff", 0.84],
  ["#ff3d8b", 1],
];

/** Repeating palette gradient with period `period` px, shifted by `phase` px. */
export function paletteGradient(angleDeg: number, period: number, phase: number): string {
  const ph = ((phase % period) + period) % period;
  const stops = PALETTE.map(([c, f]) => `${c} ${(f * period - ph).toFixed(1)}px`).join(", ");
  return `repeating-linear-gradient(${angleDeg}deg, ${stops})`;
}

function px(v: number): string {
  return `${Math.round(v * 100) / 100}px`;
}

/** CSS declarations for a mask image, with the -webkit- prefix Safari 16.4 still wants. */
function maskCss(image: string, size?: string): string {
  const sz = size ? `-webkit-mask-size:${size};mask-size:${size};` : "";
  return `-webkit-mask-image:${image};mask-image:${image};-webkit-mask-repeat:no-repeat;mask-repeat:no-repeat;${sz}`;
}

/** Bloom width round the tubes. */
function bloomOf(g: FrameGeometry): number {
  return Math.max(4, Math.min(56, g.tube * 0.42));
}

/** Mask for the arch light region (left half; the right half is its mirror image). */
function archMask(g: FrameGeometry): string {
  const w = g.width / 2;
  const h = g.junctionY;
  const B = bloomOf(g);
  const d = centreLinePath(g, 0);
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' width='${w}' height='${h}' viewBox='0 0 ${w} ${h}'>` +
    `<filter id='b' x='-10%' y='-10%' width='120%' height='120%'><feGaussianBlur stdDeviation='${(B * 0.45).toFixed(2)}'/></filter>` +
    `<path d='${d}' fill='none' stroke='#fff' stroke-opacity='.55' stroke-width='${(g.tube + 2 * B).toFixed(2)}' filter='url(#b)'/>` +
    `<path d='${d}' fill='none' stroke='#fff' stroke-width='${g.tube.toFixed(2)}'/>` +
    `</svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

/** Mask for a column light region: a horizontal profile across the tube and its bloom. */
function columnMask(g: FrameGeometry): string {
  const B = bloomOf(g);
  const a = g.margin;
  const b = g.margin + g.tube;
  const edge = Math.max(0, 0.55 * (1 - a / B));
  return (
    `linear-gradient(90deg, rgba(0,0,0,${edge.toFixed(2)}) 0px, #000 ${px(a)}, #000 ${px(b)}, ` +
    `rgba(0,0,0,.5) ${px(b + 0.5)}, rgba(0,0,0,.16) ${px(b + B * 0.45)}, rgba(0,0,0,0) ${px(b + B)})`
  );
}

/** The light for the left half of the frame (rendered twice: plain and mirrored). */
function LightHalf({ g, mirrored }: { g: FrameGeometry; mirrored: boolean }) {
  const P = g.lightPeriod;
  const half = g.width / 2;
  const B = bloomOf(g);
  const colW = g.margin + g.tube + B;
  const colH = Math.max(0, g.baseTop - g.junctionY + 2);
  // Colour continuity: the column top shows the colour the arch has at the shoulder.
  const shoulderPhase = half - g.xL;
  return (
    <div
      class={`jk-light-half${mirrored ? " jk-light-half--mirror" : ""}`}
      style={`width:${px(half)};height:${px(g.height)}`}
      aria-hidden="true"
    >
      <div
        class="jk-light-region"
        style={`left:0;top:0;width:${px(half)};height:${px(g.junctionY)};${maskCss(archMask(g))}`}
      >
        <div
          class="jk-light-strip"
          data-jk-drift="x"
          style={`right:${px(-P)};top:0;width:${px(half + P)};height:100%;background-image:${paletteGradient(270, P, 0)}`}
        />
      </div>
      <div
        class="jk-light-region"
        style={`left:0;top:${px(g.junctionY)};width:${px(colW)};height:${px(colH)};${maskCss(columnMask(g))}`}
      >
        <div
          class="jk-light-strip"
          data-jk-drift="y"
          style={`left:0;top:${px(-P)};width:100%;height:${px(colH + P)};background-image:${paletteGradient(180, P, shoulderPhase)}`}
        />
      </div>
    </div>
  );
}

/** Size and place of the crest slot. */
function crestBox(g: FrameGeometry, screenTop: number) {
  const h = Math.max(24, Math.min(screenTop + g.tube * 0.12, g.tube * 1.55 + g.margin));
  const w = h * 2.5;
  const top = Math.max(0, g.margin * 0.3);
  return { x: g.width / 2 - w / 2, y: top, w, h };
}

interface Measured {
  g: FrameGeometry;
  screenTop: number;
  consoleH: number;
}

function sameMeasure(a: Measured | null, b: Measured): boolean {
  if (!a) return false;
  const x = a.g;
  const y = b.g;
  return (
    x.width === y.width &&
    x.height === y.height &&
    x.xL === y.xL &&
    x.cornerR === y.cornerR &&
    x.cornerCy === y.cornerCy &&
    x.baseTop === y.baseTop &&
    a.screenTop === b.screenTop &&
    a.consoleH === b.consoleH
  );
}

export function JukeboxFrame({ children, console: consoleContent, crest, bubbles, motion, paused = false, class: className }: JukeboxFrameProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const screenRef = useRef<HTMLElement>(null);
  const consoleRef = useRef<HTMLElement>(null);
  const [measured, setMeasured] = useState<Measured | null>(null);
  const visible = usePageVisible();
  const animate = motion === "full";
  const running = visible && !paused;
  /** Drift phase (0..1) carried across re-creations of the animations. */
  const phaseRef = useRef(0);
  const animsRef = useRef<Animation[]>([]);

  // Measure the CSS grid and derive the decoration geometry.
  useLayoutEffect(() => {
    const root = rootRef.current;
    const screen = screenRef.current;
    const consoleEl = consoleRef.current;
    if (!root || !screen || !consoleEl) return;
    let raf = 0;
    const measure = () => {
      raf = 0;
      const r = root.getBoundingClientRect();
      const s = screen.getBoundingClientRect();
      const c = consoleEl.getBoundingClientRect();
      if (r.width < 50 || r.height < 50) return;
      const radius = parseFloat(getComputedStyle(screen).borderTopLeftRadius) || 0;
      const g = computeFrameGeometry({
        width: Math.round(r.width),
        height: Math.round(r.height),
        screen: { x: s.left - r.left, y: s.top - r.top, w: s.width, h: s.height },
        screenRadius: radius,
        consoleTop: c.top - r.top,
      });
      const next = { g, screenTop: s.top - r.top, consoleH: Math.round(r.bottom - c.top) };
      setMeasured((prev) => (sameMeasure(prev, next) ? prev : next));
    };
    measure();
    const ro = new ResizeObserver(() => {
      if (!raf) raf = requestAnimationFrame(measure);
    });
    ro.observe(root);
    return () => {
      ro.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
  }, []);

  const g = measured?.g ?? null;

  // The light drift: (re)create one Web Animation per strip whenever the geometry
  // changes, all on the same phase, so colours stay continuous between regions.
  useEffect(() => {
    const root = rootRef.current;
    for (const a of animsRef.current) {
      if (a.currentTime !== null) phaseRef.current = (Number(a.currentTime) % LIGHT_CYCLE_MS) / LIGHT_CYCLE_MS;
      a.cancel();
    }
    animsRef.current = [];
    if (!root || !g || !animate) return;
    const P = g.lightPeriod;
    const strips = root.querySelectorAll<HTMLElement>("[data-jk-drift]");
    const anims: Animation[] = [];
    strips.forEach((el) => {
      const axis = el.dataset.jkDrift;
      const to = axis === "x" ? `translate3d(${-P}px,0,0)` : `translate3d(0,${P}px,0)`;
      if (typeof el.animate !== "function") return;
      const anim = el.animate([{ transform: "translate3d(0,0,0)" }, { transform: to }], {
        duration: LIGHT_CYCLE_MS,
        iterations: Infinity,
        easing: `steps(${Math.round((LIGHT_CYCLE_MS / 1000) * DRIFT_FPS)})`,
      });
      anim.currentTime = phaseRef.current * LIGHT_CYCLE_MS;
      anims.push(anim);
    });
    animsRef.current = anims;
    return () => {
      for (const a of anims) {
        if (a.currentTime !== null) phaseRef.current = (Number(a.currentTime) % LIGHT_CYCLE_MS) / LIGHT_CYCLE_MS;
        a.cancel();
      }
      animsRef.current = [];
    };
  }, [g, animate]);

  // Pause / resume with visibility and the host's paused flag.
  useEffect(() => {
    for (const a of animsRef.current) {
      if (running && a.playState !== "running") a.play();
      else if (!running && a.playState === "running") a.pause();
    }
  }, [running, g, animate]);

  const T = g?.tube ?? 0;
  const crestRect = g && measured ? crestBox(g, measured.screenTop) : null;
  // Static artwork as SVG images, rendered once per window size (frame-art.ts).
  const art = useMemo(
    () => (g && crestRect ? { room: svgUrl(roomSvg(g)), tubes: svgUrl(tubeSvg(g)), trim: svgUrl(trimSvg(g, crestRect.w)) } : null),
    [g, measured], // crestRect is derived from these two
  );
  // Generated once per page (about 20 ms), then cached.
  const wood = useMemo(() => walnutTexture(), []);
  const P = g?.lightPeriod ?? 1000;
  // Diamond size follows the console height, so the lattice scales with the window.
  const grilleCell = measured ? Math.max(16, Math.min(64, measured.consoleH * 0.17)) : 26;

  return (
    <div
      ref={rootRef}
      class={`jk-frame${className ? ` ${className}` : ""}`}
      data-jk-motion={motion}
      data-jk-running={running ? "true" : "false"}
      style={g ? `--jk-tube:${px(T)};--jk-light-period:${px(P)}` : undefined}
    >
      <div class="jk-frame-grid">
        <div class="jk-frame-wood" aria-hidden="true" style={wood ? `--jk-wood:${wood}` : undefined} />
        {art && <img class="jk-frame-layer jk-frame-room" src={art.room} alt="" aria-hidden="true" draggable={false} />}
        {g && (
          <div class="jk-frame-layer jk-light" aria-hidden="true">
            <LightHalf g={g} mirrored={false} />
            <LightHalf g={g} mirrored={true} />
          </div>
        )}
        {art && <img class="jk-frame-layer jk-frame-tubes" src={art.tubes} alt="" aria-hidden="true" draggable={false} />}
        {g && bubbles && animate && <Bubbles geometry={g} running={running} class="jk-frame-layer jk-bubbles" />}
        {art && <img class="jk-frame-layer jk-frame-trim" src={art.trim} alt="" aria-hidden="true" draggable={false} />}
        <section ref={screenRef} class="jk-screen">
          <div class="jk-screen-glass">
            {/* Static light strips (photo 4). Still on purpose: nothing animates inside
                the screen, where the lyrics ticker runs. */}
            <div class="jk-screen-strip jk-screen-strip--left" aria-hidden="true" />
            <div class="jk-screen-content">{children}</div>
            <div class="jk-screen-strip jk-screen-strip--right" aria-hidden="true" />
            <div class="jk-screen-sheen" aria-hidden="true" />
          </div>
        </section>
        <footer ref={consoleRef} class="jk-console">
          <div class="jk-console-rail" aria-hidden="true" />
          <div class="jk-console-wing jk-console-wing--left" aria-hidden="true">
            <Grille class="jk-console-grille" cell={grilleCell} />
            <Medallion class="jk-console-medallion" />
          </div>
          <div class="jk-console-slot">{consoleContent}</div>
          <div class="jk-console-wing jk-console-wing--right" aria-hidden="true">
            <Grille class="jk-console-grille" cell={grilleCell} />
            <Medallion class="jk-console-medallion" />
          </div>
        </footer>
        {crestRect && (
          <div
            class="jk-frame-crest"
            style={`left:${px(crestRect.x)};top:${px(crestRect.y)};width:${px(crestRect.w)};height:${px(crestRect.h)}`}
          >
            {crest ?? <CrestOrnament />}
          </div>
        )}
      </div>
    </div>
  );
}

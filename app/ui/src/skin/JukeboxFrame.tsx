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
import { Aquarium } from "./Aquarium";
import { Bubbles } from "./Bubbles";
import { CrestOrnament } from "./CrestOrnament";
import { FireTube } from "./FireTube";
import { CabinetGL } from "./CabinetGL";
import { PipeGL } from "./PipeGL";
import { GPU_CABINETS, GPU_PIPES, type GpuCabinet, type GpuPipe } from "./shaders";
import { type AccentName, type CabinetName, type FrameStyleName, GLASS_PIPES, type MetalName, pilasterChannels, roomSvg, svgUrl, trimSvg, tubeSvg } from "./frame-art";
import { centreLinePath, computeFrameGeometry, type FrameGeometry } from "./geometry";
import { Grille } from "./Grille";
import { LavaLamp } from "./LavaLamp";
import { Lightning } from "./Lightning";
import { Medallion } from "./Medallion";
import { OceanTube } from "./OceanTube";
import { usePageVisible, type Motion } from "./motion";
import { useEffectFps } from "../lib/fxrate";
import { FORGE_PALETTE, PearlGlow } from "./PearlGlow";
import { forgeTexture, pearlTexture, woodTexture } from "./wood";

/** The owner's customisation of the cabinet (settings ui.look, 2026-11-22). */
export interface FrameLook {
  /** Cabinet face: animated pearlescent glow, dark walnut, light maple, an aquarium, or a forge (Molten). */
  cabinet: CabinetName;
  /** Border round the screen: neon bubble tubes or pearl pilasters (photo 7). */
  frame: FrameStyleName;
  /** Bands between the chrome rings: silver chrome, red enamel or iron (the inner trim takes iron too). */
  accents: AccentName;
  /** Speaker lattice bars (the surround is the warm metal). */
  lattice: "silver" | "gold" | "iron";
  /** Thin silver (or iron) line inside the warm metal (plates, dial, grilles, medallions). */
  innerTrim: boolean;
  /** The warm metal of rims, rings, bezels and plates: gold chrome or bronze. */
  metal?: MetalName;
  /** Glow colour of lit plates, faders, focus rings and the speaker cloth. */
  glow?: "pink" | "ember" | "aqua" | "violet" | "amber";
}

export const DEFAULT_FRAME_LOOK: FrameLook = { cabinet: "walnut", frame: "neon", accents: "red", lattice: "gold", innerTrim: false, metal: "gold", glow: "pink" };

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
  /** Cabinet customisation; the original walnut / neon / red / gold look when absent. */
  look?: FrameLook;
  /**
   * GPU effects (D-272; Windows, settings ui.look.gpu_fx): the forge cabinet as flowing
   * lava and the fire pipe drawn with WebGL. Each falls back to its 2D version by itself
   * when WebGL is missing or lost.
   */
  gpu?: boolean;
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

/**
 * The pilasters' pearl (owner, 2026-11-23: "use all the colors"): every jukebox colour
 * in a pearly tint, pink, coral, orange, yellow, green, mint and lilac, drifting faster
 * and in shorter bands than the neon tubes so the panel is always changing.
 */
const PEARL_PALETTE: readonly (readonly [string, number])[] = [
  ["#ffa6cc", 0],
  ["#ff8a80", 0.14],
  ["#ffb870", 0.28],
  ["#ffe58a", 0.42],
  ["#b4f5a8", 0.56],
  ["#aaf3ea", 0.7],
  ["#c9b4ff", 0.85],
  ["#ffa6cc", 1],
];

/** Period (px) and duration (ms) of the light drift for a frame style. */
function lightTiming(g: FrameGeometry, style: FrameStyleName): { P: number; cycle: number } {
  if (style === "pilaster") return { P: Math.round(g.lightPeriod * 0.55), cycle: 22_000 };
  // Fire: short, quick bands of flame light racing along the pipe; water and data race too.
  if (style === "fire") return { P: Math.round(g.lightPeriod * 0.4), cycle: 7_000 };
  if (style === "water" || style === "data") return { P: Math.round(g.lightPeriod * 0.5), cycle: 9_000 };
  if (style === "steam") return { P: Math.round(g.lightPeriod * 0.6), cycle: 14_000 };
  return { P: g.lightPeriod, cycle: LIGHT_CYCLE_MS };
}

/** The fire pipe's glow (the flame itself is FireTube.tsx): reds, oranges and gold. */
const FIRE_PALETTE: readonly (readonly [string, number])[] = [
  ["#ff3a0a", 0],
  ["#ff7a12", 0.18],
  ["#ffb42a", 0.34],
  ["#ff5a0a", 0.5],
  ["#c41c04", 0.68],
  ["#ff8a1a", 0.84],
  ["#ff3a0a", 1],
];

/** The lava lamp's liquid: deep ruby, magenta and violet, lit from below (the wax is drawn on top). */
const LAVA_PALETTE: readonly (readonly [string, number])[] = [
  ["#7a1050", 0],
  ["#a8185a", 0.2],
  ["#c8204a", 0.38],
  ["#901a7a", 0.58],
  ["#52127e", 0.78],
  ["#7a1050", 1],
];

/** The lightning tube's water: deep, clear blues drifting gently (owner: darker than the first light blue). */
const WATER_PALETTE: readonly (readonly [string, number])[] = [
  ["#1858b8", 0],
  ["#2470d4", 0.2],
  ["#1450a8", 0.4],
  ["#2c80e2", 0.6],
  ["#1a5cc0", 0.8],
  ["#1858b8", 1],
];

/** The ocean tube's water: tropical teal and sea blue. */
const OCEAN_PALETTE: readonly (readonly [string, number])[] = [
  ["#0a7ab0", 0],
  ["#0e98c4", 0.2],
  ["#13b4c8", 0.4],
  ["#0b86b8", 0.6],
  ["#0a68a0", 0.8],
  ["#0a7ab0", 1],
];

type Palette = readonly (readonly [string, number])[];

/** The glow under the GPU pipes' glass (D-275), by what is inside. */
const GLASS_PALETTES: Partial<Record<FrameStyleName, Palette>> = {
  water: [
    ["#0a5ad8", 0],
    ["#1a8ff0", 0.25],
    ["#36c8ff", 0.5],
    ["#0a6ae0", 0.75],
    ["#0a5ad8", 1],
  ],
  steam: [
    ["#c8ced6", 0],
    ["#ffffff", 0.3],
    ["#aab2bc", 0.55],
    ["#f2ece2", 0.8],
    ["#c8ced6", 1],
  ],
  data: [
    ["#0a8ab0", 0],
    ["#2ae0ff", 0.3],
    ["#ffd23a", 0.45],
    ["#1a6aff", 0.65],
    ["#2ae0ff", 0.85],
    ["#0a8ab0", 1],
  ],
};

function paletteFor(style: FrameStyleName): Palette {
  const glass = GLASS_PALETTES[style];
  if (glass) return glass;
  return style === "pilaster"
    ? PEARL_PALETTE
    : style === "lava"
      ? LAVA_PALETTE
      : style === "light"
        ? WATER_PALETTE
        : style === "ocean"
          ? OCEAN_PALETTE
          : style === "fire"
            ? FIRE_PALETTE
            : PALETTE;
}

/** Repeating palette gradient with period `period` px, shifted by `phase` px. */
export function paletteGradient(angleDeg: number, period: number, phase: number, palette: readonly (readonly [string, number])[] = PALETTE): string {
  const ph = ((phase % period) + period) % period;
  const stops = palette.map(([c, f]) => `${c} ${(f * period - ph).toFixed(1)}px`).join(", ");
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
function LightHalf({ g, mirrored, style }: { g: FrameGeometry; mirrored: boolean; style: FrameStyleName }) {
  const pal = paletteFor(style);
  const P = lightTiming(g, style).P;
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
          style={`right:${px(-P)};top:0;width:${px(half + P)};height:100%;background-image:${paletteGradient(270, P, 0, pal)}`}
        />
      </div>
      <div
        class="jk-light-region"
        style={`left:0;top:${px(g.junctionY)};width:${px(colW)};height:${px(colH)};${maskCss(columnMask(g))}`}
      >
        <div
          class="jk-light-strip"
          data-jk-drift="y"
          style={`left:0;top:${px(-P)};width:100%;height:${px(colH + P)};background-image:${paletteGradient(180, P, shoulderPhase, pal)}`}
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

export function JukeboxFrame({ children, console: consoleContent, crest, bubbles, motion, paused = false, look = DEFAULT_FRAME_LOOK, gpu = false, class: className }: JukeboxFrameProps) {
  const pilaster = look.frame === "pilaster";
  const rootRef = useRef<HTMLDivElement>(null);
  const screenRef = useRef<HTMLElement>(null);
  const consoleRef = useRef<HTMLElement>(null);
  const [measured, setMeasured] = useState<Measured | null>(null);
  /** 0 = standard rates; otherwise the frame rate setting (D-274). */
  const fxCap = useEffectFps(0);
  /**
   * GPU effects that could not start or whose context was lost for good: that kind only
   * shows its 2D version (the other themes keep theirs). Turning "GPU lava and fire" off
   * and on again gives every one another try.
   */
  const [cabFailed, setCabFailed] = useState<ReadonlySet<string>>(() => new Set());
  const [pipeFailed, setPipeFailed] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    if (!gpu) {
      setCabFailed((s) => (s.size ? new Set() : s));
      setPipeFailed((s) => (s.size ? new Set() : s));
    }
  }, [gpu]);
  const gpuCabinet = (GPU_CABINETS as readonly string[]).includes(look.cabinet) ? (look.cabinet as GpuCabinet) : null;
  const gpuPipe = (GPU_PIPES as readonly string[]).includes(look.frame) ? (look.frame as GpuPipe) : null;
  const lavaFailed = gpuCabinet ? cabFailed.has(gpuCabinet) : false;
  const fireFailed = gpuPipe ? pipeFailed.has(gpuPipe) : false;
  const glassPipe = !!GLASS_PIPES[look.frame];
  const visible = usePageVisible();
  const animate = motion === "full";
  const running = visible && !paused;
  /** Drift phase (0..1) carried across re-creations of the animations. */
  const phaseRef = useRef(0);
  /** Duration of the drift animations now running (it differs per frame style). */
  const cycleRef = useRef(LIGHT_CYCLE_MS);
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
    // The screen changes size on its own when the frame style widens the side columns.
    ro.observe(screen);
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
      if (a.currentTime !== null) phaseRef.current = (Number(a.currentTime) % cycleRef.current) / cycleRef.current;
      a.cancel();
    }
    animsRef.current = [];
    if (!root || !g || !animate) return;
    const { P, cycle } = lightTiming(g, look.frame);
    cycleRef.current = cycle;
    const strips = root.querySelectorAll<HTMLElement>("[data-jk-drift]");
    const anims: Animation[] = [];
    strips.forEach((el) => {
      const axis = el.dataset.jkDrift;
      const to = axis === "x" ? `translate3d(${-P}px,0,0)` : `translate3d(0,${P}px,0)`;
      if (typeof el.animate !== "function") return;
      const anim = el.animate([{ transform: "translate3d(0,0,0)" }, { transform: to }], {
        duration: cycle,
        iterations: Infinity,
        easing: fxCap > 0 ? "linear" : `steps(${Math.round((cycle / 1000) * DRIFT_FPS)})`,
      });
      anim.currentTime = phaseRef.current * cycle;
      anims.push(anim);
    });
    animsRef.current = anims;
    return () => {
      for (const a of anims) {
        if (a.currentTime !== null) phaseRef.current = (Number(a.currentTime) % cycle) / cycle;
        a.cancel();
      }
      animsRef.current = [];
    };
  }, [g, animate, look.frame, fxCap]);

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
    () =>
      g && crestRect
        ? {
            room: svgUrl(roomSvg(g, glassPipe ? "fire" : look.frame === "light" || look.frame === "ocean")),
            tubes: svgUrl(tubeSvg(g, { style: look.frame })),
            trim: svgUrl(trimSvg(g, crestRect.w, { style: look.frame, accents: look.accents, cabinet: look.cabinet, metal: look.metal })),
          }
        : null,
    [g, measured, look.frame, look.accents, look.cabinet, look.metal], // crestRect is derived from g and measured
  );
  // Generated once per page per finish (about 20-60 ms), then cached.
  const wood = useMemo(() => (look.cabinet === "walnut" || look.cabinet === "maple" ? woodTexture(look.cabinet) : null), [look.cabinet]);
  const pearl = useMemo(() => (look.cabinet === "pearl" ? pearlTexture() : null), [look.cabinet]);
  const forge = useMemo(() => (look.cabinet === "forge" ? forgeTexture() : null), [look.cabinet]);
  const glow = look.glow ?? "pink";
  const metal = look.metal ?? "gold";
  // Small bubbles: the pilaster's two tubes, or spread through the lightning tube's water.
  // ... and, without the GPU, through the water pipe (its 2D look).
  const lightning = look.frame === "light" || look.frame === "ocean" || (!(gpu && !fireFailed) && look.frame === "water");
  const channels = useMemo(
    () => (g && pilaster ? pilasterChannels(g) : g && lightning ? { offset: g.tube * 0.2, width: Math.max(4, g.tube * 0.12) } : null),
    [g, pilaster, lightning],
  );
  const P = g?.lightPeriod ?? 1000;
  // Diamond size follows the console height, so the lattice scales with the window.
  const grilleCell = measured ? Math.max(16, Math.min(64, measured.consoleH * 0.17)) : 26;

  return (
    <div
      ref={rootRef}
      class={`jk-frame${className ? ` ${className}` : ""}`}
      data-jk-motion={motion}
      data-jk-running={running ? "true" : "false"}
      data-jk-cabinet={look.cabinet}
      data-jk-frame-style={look.frame}
      data-jk-lattice={look.lattice}
      data-jk-trim={look.innerTrim ? "silver" : "none"}
      data-jk-metal={metal}
      data-jk-accents={look.accents}
      data-jk-glow={glow}
      data-jk-gpu={gpu && !lavaFailed ? "on" : "off"}
      style={g ? `--jk-tube:${px(T)};--jk-light-period:${px(P)}` : undefined}
    >
      <div class="jk-frame-grid">
        <div class="jk-frame-wood" aria-hidden="true" style={wood ? `--jk-wood:${wood}` : undefined}>
          {look.cabinet === "pearl" && (
            <>
              <PearlGlow class="jk-pearl-glow" running={running} animate={animate} />
              <div class="jk-pearl-sheen" style={pearl ? `--jk-pearl:${pearl}` : undefined} />
            </>
          )}
          {gpuCabinet && gpu && !lavaFailed && g && (
            <CabinetGL kind={gpuCabinet} class="jk-lava" width={g.width} height={g.height} running={running} animate={animate} onFail={() => setCabFailed((s) => new Set(s).add(gpuCabinet))} />
          )}
          {look.cabinet === "forge" && !(gpu && !lavaFailed) && (
            <>
              <PearlGlow class="jk-pearl-glow" running={running} animate={animate} palette={FORGE_PALETTE} cycleMs={26_000} />
              <div class="jk-forge-crust" style={forge ? `--jk-forge:${forge}` : undefined} />
            </>
          )}
          {look.cabinet === "aquarium" && g && measured && (
            <Aquarium class="jk-aquarium" running={running} animate={animate} width={g.width} band={measured.consoleH * 1.04} />
          )}
        </div>
        {art && <img class="jk-frame-layer jk-frame-room" src={art.room} alt="" aria-hidden="true" draggable={false} />}
        {g && (
          <div class="jk-frame-layer jk-light" aria-hidden="true">
            <LightHalf g={g} mirrored={false} style={look.frame} />
            <LightHalf g={g} mirrored={true} style={look.frame} />
          </div>
        )}
        {art && <img class="jk-frame-layer jk-frame-tubes" src={art.tubes} alt="" aria-hidden="true" draggable={false} />}
        {g && bubbles && animate && (look.frame === "neon" || pilaster || lightning) && (
          <Bubbles geometry={g} running={running} channels={channels} class="jk-frame-layer jk-bubbles" />
        )}
        {g && look.frame === "lava" && <LavaLamp geometry={g} running={running && animate} class="jk-frame-layer jk-bubbles" />}
        {g && look.frame === "light" && <Lightning geometry={g} running={running && animate} class="jk-frame-layer jk-bubbles" />}
        {g && look.frame === "ocean" && <OceanTube geometry={g} running={running && animate} class="jk-frame-layer jk-bubbles" />}
        {g && gpuPipe && gpu && !fireFailed && (
          <PipeGL kind={gpuPipe} geometry={g} running={running && animate} onFail={() => setPipeFailed((s) => new Set(s).add(gpuPipe))} class="jk-frame-layer jk-bubbles" />
        )}
        {g && look.frame === "fire" && !(gpu && !fireFailed) && <FireTube geometry={g} running={running && animate} class="jk-frame-layer jk-bubbles" />}
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
            <div class="jk-console-trim">
              <Grille class="jk-console-grille" cell={grilleCell} bars={look.lattice} cloth={glow} />
            </div>
            <Medallion class="jk-console-medallion" trim={look.innerTrim} metal={metal} trimFinish={look.accents === "iron" || look.accents === "copper" ? look.accents : "silver"} enamel={glow} />
          </div>
          <div class="jk-console-slot">{consoleContent}</div>
          <div class="jk-console-wing jk-console-wing--right" aria-hidden="true">
            <div class="jk-console-trim">
              <Grille class="jk-console-grille" cell={grilleCell} bars={look.lattice} cloth={glow} />
            </div>
            <Medallion class="jk-console-medallion" trim={look.innerTrim} metal={metal} trimFinish={look.accents === "iron" || look.accents === "copper" ? look.accents : "silver"} enamel={glow} />
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

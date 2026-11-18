# Jukebox skin (`app/ui/src/skin`)

The bubbler-jukebox look from `docs/DESIGN.md`, as a self-contained layer of Preact
components. Nothing in here talks to the Rust core; the integrator passes data and
callbacks in. Importing `./skin` loads `skin.css` and the bundled fonts.

Everything is drawn in code (SVG, CSS gradients, one canvas). No images from the
reference photos, no maker's name, logo or badge shape. GPL-3.0-or-later; fonts
SIL OFL 1.1 (see `src/assets/fonts/*/OFL.txt` and `SOURCE.txt`).

Try it: `npm run dev` in `app/ui`, open `http://localhost:5173/skin-demo.html`
(`?view=library|stage|gallery&bubbles=0|1&motion=system|full|reduce&paused=1&hud=0`;
press D to hide the demo controls). `skin-demo.html` is served by the dev server
only; it is not part of `npm run build`.

## Components

| Export | Props | Notes |
|---|---|---|
| `JukeboxFrame` | `children` (screen), `console`, `crest?`, `bubbles: boolean`, `motion: "full" \| "reduce"`, `paused?`, `class?` | The whole window frame. Give it a box (it fills 100% x 100% of its parent). |
| `KeyDial` | `semitones`, `min?` (-12), `max?` (12), `label` ("A major -> B major (+2)"), `onChange(n)`, `disabled?` | 270-degree sweep, one detent per semitone, spring pointer, knurled ring turns with the value, -/+ plates. Drag round the ring (or up/down near the centre), wheel, keys: arrows, `[` `]`, `+` `-`, PageUp/PageDown (2), Home/End, `0` = original key; double-click = original key. **Commits once the hand stops** (release, or 160 ms after the last step) because each key change rebuilds the pitch shifter (DECISIONS D-31); the face shows the pending value meanwhile. `role="slider"` with `aria-valuetext` = the label. |
| `PlateButton` | `label` (text or node), `onClick?`, `active?`, `disabled?`, `title?`, `ariaLabel?`, `size?: "sm"\|"md"\|"lg"` | Chrome bezel, black glass, white condensed caps; `active` lights it pink. `active` undefined = plain push button (no `aria-pressed`). Class `jk-plate--danger` lights the glass red (a plate that deletes), `jk-plate--danger-hover` only under the pointer or focus. |
| `TransportIcon` | `name: "play"\|"pause"\|"stop"\|"restart"\|"prev"\|"next"\|"record"\|"back"\|"forward"\|"trash"` | Drawn icons for plate labels (Unicode media symbols render as colour emoji in some WebKit builds). |
| `Fader` | `value` 0..1, `onChange(v)`, `label`, `disabled?`, `title?`, `valueText?`, `orientation?` | Fires continuously while dragging (mixer changes are cheap). Arrows 0.02, PageUp/PageDown 0.1, Home/End, wheel. |
| `TitleStripCard` | `title`, `artist`, `keyName?`, `duration` (s or string), `code`, `cover?` (URL), `selected?`, `onSelect()`, `note?`, `disabled?`, `checked?` | A `<button>`. Draws a record when there is no cover. `checked` set (the Library's Select mode, D-230): a checkbox (`role="checkbox"`), with a selector lamp on the chrome tab that lights pink when ticked. |
| `TitleStripRack` | `children`, `label?` | Chrome-framed grid of strips: 2 columns on a laptop, more on 4K, 1 when narrow (no media queries). |
| `Badge` | `name?`, `title?` | The app's own badge. Pass `name={appInfo.name}`; without it the name is read from `<meta name="application-name">` or the `<title>` (both filled from `branding.json` by the Vite plugin). |
| `Medallion`, `Grille`, `CrestOrnament` | `class?` (+ `title?`, `cell?`, `lit?`) | Ornaments; the frame uses them itself. |
| `Bubbles` | `geometry`, `running`, `maxBubbles?` | Used by the frame; exported for tests. `bubbleStats` = live counters. |
| `useResolvedMotion(setting)` | `"system"\|"reduce"\|"full"` -> `"full"\|"reduce"` | Follows `prefers-reduced-motion` live when the setting is `"system"`. |
| `usePageVisible()` | | `document.visibilityState`, live. |

CSS helpers: `.jk-glass-input` (search box as a black-glass plate), `.jk-lyrics-text`
(the lyrics face with ligatures and contextual alternates off). Tokens on `:root`:
`--jk-pink` ... `--jk-violet`, `--jk-enamel`, `--jk-cream`, `--jk-screen-bg`,
`--jk-chrome`, `--jk-glass-face`, `--jk-font-display` (Oswald), `--jk-font-lyrics`
(Atkinson Hyperlegible Next), `--jk-font-script` (Pacifico, badge only). Inside the
frame: `--jk-screen-r` (top-corner radius of the screen; pad content near the top
corners by about `0.3 * var(--jk-screen-r)`), `--jk-console-h`.

## How the app uses it

Wired in by the integrator (2026-11-17): `App.tsx` `SkinApp` renders the app inside
`JukeboxFrame` when `settings.ui.skin` is on (crest = `Badge name={appInfo.name}`,
`bubbles` = `settings.ui.bubbles`, `motion` = `useResolvedMotion(settings.ui.motion)`,
`paused` = the native window is minimised, from Tauri). The console is
`components/Jukebox.tsx` (transport plates + seek bar, `KeyDial`, Guide / Backing /
Master faders, a clip lamp); the library uses `TitleStripRack` + `TitleStripCard`
(`components/Library.tsx`, `<Library skin />`); `src/skin-app.css` restyles the plain
screens for the dark screen and points `--ticker-font` at the bundled lyrics face.
The sketch below is the original wiring plan, kept for reference.

## Wiring it into the app (original plan)

```tsx
import { Badge, Fader, JukeboxFrame, KeyDial, PlateButton, TransportIcon, useResolvedMotion } from "./skin";

const motion = useResolvedMotion(settings.ui.motion);           // AppSettings.ui.motion
return settings.ui.skin ? (
  <JukeboxFrame
    crest={<Badge name={appInfo.name} />}
    bubbles={settings.ui.bubbles}
    motion={motion}
    paused={windowMinimised}                                    // see below
    console={
      <>
        <PlateButton label={<TransportIcon name="play" />} ariaLabel="Play" active={playing} onClick={toggle} />
        <KeyDial semitones={song.settings.semitones} label={player.key_label ?? ""}
                 onChange={(n) => player_set({ semitones: n })} disabled={!song} />
        <Fader label="Guide" value={song.settings.guide_vocal} onChange={(v) => player_set({ guide_vocal: v })} />
      </>
    }>
    {screen /* library (TitleStripRack + TitleStripCard) or the stage */}
  </JukeboxFrame>
) : plainUi;
```

- The frame needs a sized parent: `html, body, #app { height: 100% }`.
- `paused`: the frame already stops when `document.visibilityState` is `"hidden"`.
  Whether a minimised Tauri window reports that depends on the web view (WebView2
  reports it only when the host hides its controller; not verified for wry/WKWebView
  here). Pass `paused` from Tauri's window events (minimised / occluded) so the drift
  and bubbles stop there for certain.
- Fonts load with `font-display: block` from local files. The ticker should
  `await document.fonts.load('600 64px "Atkinson Hyperlegible Next"')` before measuring
  glyph positions, and use `.jk-lyrics-text` (or the same font settings on its canvas).
- `motion="reduce"` stops the drift (static colours), removes the bubbles, and turns
  off the dial's spring and pops. Set `data-jk-motion="reduce"` on any ancestor to do
  the same for components used outside the frame.

## How it stays cheap

- Layout is a CSS grid sized with `clamp()` (container units, with vw/vh fallback);
  JS only measures it (ResizeObserver) and derives every curve from one centre line
  (`geometry.ts`, unit-tested).
- Static decoration (tube shading, rims, rings, bases, crest clamps, the dark room)
  is built as standalone SVG *images* once per window size (`frame-art.ts`), and the
  walnut is a bitmap tile generated once in a canvas (`wood.ts`). Engines keep the
  raster of an image, so a repaint is a blit; inline SVG with blur filters (and an
  `feTurbulence` wood tile) was re-filtered on every drift step in Playwright's
  WebKit for Windows, which dropped that page to 3-4 frames/s.
- The drifting light: gradient strips inside fixed, masked regions (arch halves and
  columns) moved by Web Animations `transform` (composited). One palette period
  per 40 s, advanced in 12 steps per second (`DRIFT_FPS`), so the compositor
  redraws the tubes 12 times a second instead of every display frame (measured with
  only the easing changed, software-composited Chromium, drift only: 47 % of one
  core with linear easing, 8.7 % stepped; short 4-5 s trials). The light strips inside the screen are static, so nothing animates where
  the lyrics ticker runs.
- Bubbles: one canvas, at most 30 frames and 60 bubbles, sprites cached per radius,
  only the previous frame's bubble rectangles are cleared, no allocation per frame.
- Hidden page or `paused`: animations paused, bubble loop stopped (no timer, no rAF).

### Measured (2026-11-17, this PC, demo stage view, 1440x900 @2x, 15 s windows)

`node src/skin/tools/measure-skin.mjs <playwright-core dir> <out dir> 15 both`
(results in `measure.json` next to the screenshots, `../jukeoke-build/screenshots/skin/`).
"gpu" = Chromium in new headless mode compositing on the RTX 5070 Ti (like WebView2);
"software" = headless shell with SwiftShader (no GPU, worst case). CPU is % of one
core for the whole browser process tree; main thread = renderer main thread busy.

| Scenario | gpu: main / tree | software: main / tree | bubbles |
|---|---|---|---|
| bubbles + drift | 1.8 % / 14.8 % | 3.7 % / 43.1 % | 30.0 fps, draw 0.10 ms avg, 0.9 ms max |
| drift only (bubbles off) | 0.02 % / 5.1 % | 0.02 % / 9.7 % | - |
| motion reduce | 0.00 % / 0.8 % | 0.00 % / 0.0 % | - |
| window really minimised | 0.08 % / 2.4 % | 0.17 % / 3.6 % | 1.0 fps (browser stops frames) |
| visibilitychange -> hidden | 0.00 % / 0.3 % | 0.00 % / 0.1 % | 0 fps; 4/4 drift animations paused |

Frame pacing while animating: rAF intervals p50 4.2 ms / p99 4.3 ms (gpu, headless
ticks at 240 Hz), 16.7 / 16.8 ms (software, 60 Hz): no long frames. The bubble canvas
is the largest cost (mostly compositing a full-window canvas 30 times a second);
the drift alone is cheap. These are Chromium numbers: Safari on the MacBook could not
be measured here (see below).

`node src/skin/tools/interact-skin.mjs <playwright-core dir> <demo url> [chromium|webkit]`
drives the controls (dial keys, wheel, drag, end stops, commit, plates, fader, strip
selection, bubbles/motion/paused switches); it passes in Chromium and in Playwright's
WebKit.

WebKit note: Playwright's WebKit for Windows (WebKit 27.2 UA, not Safari 16.4, and a
different graphics backend) renders the skin correctly but slowly: page rendering fell
to 12 frames/s with the drift on and ~7 with bubbles too (52 with motion reduce),
because it repaints instead of compositing. That is not how Safari on macOS draws, but
it is a reason to check the MacBook: bubble fps and CPU belong in the owner's checklist.

## Browser baseline (Safari 16.4 / WebView2)

| Feature | Safari | Use / fallback |
|---|---|---|
| `clamp()`, `min()` | 13.1 | sizes |
| container queries, `cqw`/`cqh` | 16.0 | frame sizes; vw/vh fallback under `@supports not` |
| `conic-gradient`, `repeating-conic-gradient` | 12.1 | dial chrome and knurling |
| `mask-image` | prefixed in WebKit | set inline with both `-webkit-mask-*` and `mask-*` |
| Web Animations `element.animate`, `getAnimations` | 13.1 | light drift (`steps()` easing) |
| `ResizeObserver` | 13.1 | geometry |
| Pointer events + `setPointerCapture` | 13 | dial, faders |
| `:focus-visible` | 15.4 | focus rings |
| `aspect-ratio` | 15 | medallion |
| variable woff2 with a weight range | 11 | Oswald, Atkinson Hyperlegible Next |
| `MediaQueryList.addEventListener` | 14 | reduced-motion follow |

Not used: `backdrop-filter`, CSS nesting (16.5), `color-mix()`, `offset-path`,
`@property`, `:has()`, `OffscreenCanvas`, `ctx.roundRect`. The built demo passes
`scripts/check-dist-syntax.mjs` (ES2022) and `scripts/check-dist-css.mjs`.
Rendered in Playwright's WebKit (a recent WebKit, not 16.4) as well as Chromium.

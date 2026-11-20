# Practice mode: the screen (Work Order 2, Stage 4)

Work Order 2 sections 4.5 (the practice screen), 4.6 ("The picture follows", the names),
4.7 (the result screen), 4.4 (input picker, levels, permission) and the section 9 row "The
screen". Built against docs/APP_API.md section 12 with the browser mock
(`app/ui/src/api/mock.ts` + `mock-practice.ts`); the real core is wired in a later stage.
Design: docs/DESIGN.md "Practice mode". Measured 2026-11-19 on the build PC (Ryzen 7
9700X, RTX 5070 Ti, Windows 11) in headless Microsoft Edge (Chromium 157.0.4341.60, the
WebView2 engine) driven by playwright-core. Three other workstreams were building and
benchmarking on the same PC at the same time, so the timings below carry noise (two runs
are quoted where they differ). Stage 7 merges this file into `docs/benchmarks/practice.md`.

Evidence (outside the repository): `../jukeoke-build/practice-ui/screens/` (23 PNGs,
`report.json`, `run.log` = the final run; `run-earlier-busy-machine.log` = an earlier run
on a busier machine), `../jukeoke-build/practice-ui/tools/` (the development scripts
behind section 3.3: `raster-ab.mjs`, `cost-breakdown.mjs`, `zoom-shot.mjs`, and the
renderer before the device-pixel change). Only the public-domain demo songs of the mock
(Twinkle, Twinkle, Little Star; Frère Jacques; Row, Row, Row Your Boat) and the generated
demo song's guide were used; no other lyrics were read or written.

## 1. The whole-song screen check (section 9 "The screen")

`laneverify.ts` computes, independently of `lane.ts`'s helpers (its own name table, the
APP_API 12.1 rule written out), at every sampled moment of a song: each bar in view is on
its note's key row at its note's height (within 0.5 px), the key is named right, each word
sits under its note and is its note's word, the lit key is the note due now, the marker
is at the sung pitch's height on the right key (or pinned at the edge), the higher / lower
cue is right, octaves are forgiven unless strict, the marker hides with nobody singing.

**Unit tests** (`cd app/ui && npx vitest run src/practice`: 3 files, 69 tests, all pass):

| test file | what it covers |
|---|---|
| `lane.test.ts` | the demo song's real guide (Rust builder output, `src/practice/fixtures/demo-song.practice.json`) across the whole song, every 0.05 s, at 6 lane sizes (1200x270, 1070x240, 3000x680 at scale 2.4, 1360x470 piano right at dpr 2, 1070x240 at dpr 1.25, 1200x420 without words) x keys 0, -5, +3 and octave shifts (-3 with octave -1, +2 with octave +1): 0 errors; > 500 bars, > 400 lit keys, > 200 words checked per case; >= 2 s look-ahead, now line at 25-33 %, >= 12 rows; the mock's Twinkle (A major, +2 saved) and Row (-14 cent tuning, spoken and low-confidence notes) at every key -12..12 with octave shifts, names spelled with the key; the range rule (a 5-semitone melody still shows 13 rows; a three-octave melody scrolls, never more than 0.25 row per frame); the key HEARD moving the bars (pending change at 3.28 s for a dial touch at 3.0 s: bars move from 3.28 s on, slide in 0.18 s, instant under reduce-motion); marker honesty; lit parts staying lit; reduce-motion and the simple lane drawing no tail |
| `lanerenderer.test.ts` | the renderer with a recording 2D context, whole song every 0.25 s at dpr 1, 1.25, 1.5 and 2 x keys 0, -5, +3, -3/oct -1: every bar's blit lands within half a device pixel of the pitch rule, its piano key exactly level with it (< 0.01 px), every cached bitmap copied 1:1 onto whole device pixels; a key change and the slide redraw no tile; no tail under reduce-motion and the simple lane |
| `practice-ui.test.tsx` | the screens with the mock core: practice off leaves the singing screen unchanged; the plate and `P`; transport, key dial and mixer keep working; leaving the singing screen closes the microphone; the macOS explanation before the system asks; watch only; the Windows privacy switch named with steps; the input picker (kinds, Bluetooth warning, a meter per channel, voice channel saved); music in the input; the result screen; practice data not ready (built on the spot, failure with Try again); the key heard |

Whole UI suite: `cd app/ui && npx vitest run`: 25 files, 527 tests, all pass.

**In a real browser with the real fonts** (`node app/ui/scripts/practice-screens.mjs`,
`window.__jkLane.verify` in headless Edge, the demo song's guide, whole song every 0.1 s):

| window | key | octave | bars | words | lit keys | marker | errors | worst bar offset | look-ahead | now line | rows |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1280x800 | 0, -5, +3 | 0 | 6183 each | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.6 |
| 1280x800 | -3 | -1 | 6183 | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.6 |
| 1440x900 | 0, -5, +3 | 0 | 6183 each | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.7 |
| 1440x900 | -3 | -1 | 6183 | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.7 |
| 3840x2160 | 0, -5, +3 | 0 | 6183 each | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.5 |
| 3840x2160 | -3 | -1 | 6183 | 4724 | 767 | 767 | 0 | 0.00 px | 3.00 s | 30 % | 13.5 |

## 2. Fit (WO2 4.5 "It must fit")

The lane's canvas with the jukebox frame on, from the same run: 1220x318 at 1440x900
(rows 20 px, bars 14.4 px, marker head 25.9 px), 1085x282 at 1280x800 (18 / 13.0 / 23.3),
3261x776 at 3840x2160 (50 / 36 / 64.8), 1220x478 with the lyrics below switched off; the
usual lyrics display takes the rest of the lyrics area below it. No horizontal overflow
and no page errors in any of the 23 screenshots.

## 3. Drawing cost (WO2 4.5 budget: 4 ms a frame on average for both canvases)

### 3.1 Three measures

- **Loops**: the lane's and the lyrics display's own frame time (frame function and the
  drawing commands), from their `stats`, over 8 s of the demo song playing with the
  simulated singer; their frame rate (both are capped at 60).
- **Main thread**: the whole page's main-thread busy time per displayed frame over the
  same 8 s (CDP `Performance.getMetrics` TaskDuration: script, style, layout, paint and
  the canvases' flush), with practice on and, as the baseline, the ordinary singing
  screen with practice off.
- **Lane CPU-rasterised**: 600 frames of the demo song on an offscreen canvas of the
  lane's size kept on the CPU (`willReadFrequently`), each frame followed by a one-pixel
  read so its rasterisation is inside the timed span (`lanecost.ts`; the self-check runs
  the same code). On a GPU canvas the same read times the wait for the GPU (3-10 ms here,
  most of it the readback stall, which the real loop never has), so the CPU raster is
  used as the repeatable upper bound.

The **software** rows run a second browser with the GPU switched off
(`--disable-gpu --disable-gpu-compositing --disable-accelerated-2d-canvas`): everything,
the jukebox frame and bubbles included, rasterised by Skia on the CPU, the worst case
for a web view that draws canvases on the CPU.

### 3.2 Results (final run, `run.log`; ms per frame, average)

| case | loops: lane + lyrics | main thread | loops fps | lane CPU-rasterised avg / p95 | simple lane |
|---|---|---|---|---|---|
| 1440x900 | 0.20 + 0.12 = **0.32** | 0.52 (practice off: 0.37) | 59.3 | **0.69** / 0.80 | 0.68 |
| 1280x800 | 0.23 + 0.06 = **0.29** | 0.45 | 60.0 | **0.51** / 0.60 | 0.50 |
| 1440x900 at dpr 2 | 0.19 + 0.12 = 0.31 | 0.51 | 60.1 | 2.81 / 3.00 | 2.81 |
| 3840x2160 (dpr 1) | 0.22 + 0.12 = 0.34 | 0.54 | 57.0 | 5.20 / 5.90 | 4.81 |
| 1440x900, simple lane on | 0.15 + 0.11 = 0.26 | 0.45 | 60.1 | 0.57 / 0.70 | 0.56 |
| 1440x900 software | 0.27 + 0.07 = 0.34 | **2.51** (practice off: 1.34) | 60.0 | 0.57 / 0.70 | 0.58 |
| 1440x900 dpr 2 software | 0.23 + 0.12 = 0.35 | 6.13 | 60.0 | 2.43 / 2.90 | 2.00 |
| 1440x900 dpr 2 software, simple lane | 0.18 + 0.10 = 0.28 | 4.76 | 60.1 | 1.45 / 1.90 | 1.49 |

On a quieter moment of the PC (the development A/B of 3.3) the lane CPU-rasterised was
0.38 ms (1440x900), 1.37 ms (dpr 2) and 2.31 ms (4K); `run-earlier-busy-machine.log` shows
the same run on a busier moment (software rows below 60 fps with practice off too).

**Verdict for the budget.** At the WO2 window sizes the lane and the lyrics together take
0.3 ms of frame-loop time and the whole page 0.5 ms of main-thread time per frame (practice
adds about 0.15 ms over the ordinary singing screen); rasterised on the CPU the lane alone
is 0.5-0.7 ms. With the GPU off entirely the page still holds 60 fps at 1440x900 with
2.5 ms of main-thread time (practice adds 1.2 ms). All within the 4 ms. Not within it:
Retina without a GPU (6.1 ms for the page, 4.8 ms with the simple lane; still 60 fps
here), and a 4K window at dpr 1 rasterised on the CPU (5.2 ms for the lane). Both web
views the app uses (WebView2, WKWebView) draw canvases on the GPU by default; the M1 has
not been measured, which is what the self-check's lane item is for.

**Correction (independent verification, 2026-11-19).** The "main thread" column above is
per *displayed* frame, and headless Edge runs `requestAnimationFrame` at about 170 Hz with
the GPU on (`displayFrames` in `report.json`: about 1,390 in 8 s), while the drawing loops
run at 60. Per 60-fps frame of the loops, which is what the budget means, the GPU rows are
about 2.9 times higher: recomputed from `report.json` (busy time / loop frames), 1440x900
**1.45 ms** (practice off 1.06; the verifier's re-run 1.45 and 0.89), 1280x800 1.31
(re-run 0.92), dpr 2 1.46, 4K 1.56, simple lane 1.29. Practice thus adds about 0.4-0.55 ms
per frame, not 0.15. The software rows already ran at 60 Hz and are unchanged. Still
within the 4 ms everywhere it was within it. `practice-screens.mjs` now divides by the
loops' frames (`mainThreadMsPerFrame`) and keeps the old figure as
`mainThreadMsPerDisplayFrame`. The verifier's re-run (lane tile pre-render now counted in
the lane loop's time) gave loops 0.31 / 0.22 ms, lane CPU-rasterised 0.73 / 0.32 ms
(1440x900 / 1280x800), 4K 2.31 ms, dpr 2 software 4.62 ms for the page (4.10 simple).

### 3.3 What made it cheap (development A/B, `tools/`)

The lane draws its bars, words, key rows, piano keys, needle, marker and glows once into
cached bitmaps and copies them each frame (WO2 trap 13); a key change or a slide redraws
nothing (renderer test). Two changes measured during this stage:

- **Whole device pixels.** A copy at a fractional position is filtered, and a filtered
  copy costs about 3x more to rasterise on the CPU. The row height is now a whole, even
  number of device pixels (`lane.ts`), and every cached bitmap is copied 1:1 onto whole
  device pixels, one rounding per frame for the time axis and one for the pitch rows, so a
  bar stays exactly level with its piano key (renderer test above). Lane CPU-rasterised,
  `raster-ab.mjs`, same moment, before -> after: 1440x900 1.45 -> 0.43 ms, dpr 2 5.15 ->
  1.41 ms, 4K 9.03 -> 3.86 ms (then 0.38 / 1.37 / 2.31 ms with the row quantisation).
  The bars move in whole device pixels (at most half a device pixel from the exact frame;
  a quarter of a CSS pixel on Retina).
- **No blur per frame.** The two lit piano keys were drawn with a shadow blur every frame;
  they are now sprites drawn once with their glow. The countdown's glow is two faint
  strokes under the letters. The tubes keep their blurred glow, drawn once per tile: a
  layered glow instead measured the same in the CPU-raster bench and looked banded.

Per-part breakdown before these changes (`cost-breakdown.mjs`, CPU raster, 1440x900):
1.44 ms in all, of which the key rows 0.6 ms and the bar tiles 0.64 ms (both filtered
copies over the whole bar area), every other part about 0.1 ms.

## 4. Screenshots (with the jukebox skin on unless named "plain")

`practice-1440x900`, `-1280x800`, `-3840x2160`, `-1440x900-retina` (singing, the marker
on the note and off it with HIGHER, lit parts, words, lyrics below), `-piano-right`,
`-row-tuning-spoken` (-14 cent tuning, slides, fast notes), `-octave-down` (Auto has moved
the lane an octave down; keys named D♯3...), `-key-down-3` (A major -> F♯ major: bars,
names and the lit key three semitones lower), `-break-countdown`, `-watch-only-no-mic`,
`-permission-macos` (the app's explanation before the system asks), `-denied-windows-privacy`,
`-picker` (USB mixer recommended, built-in, Bluetooth with its warning; a meter per channel),
`-music-in-input`, `-preparing`, `-not-ready`, `-simple-reduce` (simple lane + reduce
motion: plain marker, no tail, no glows), `-lane-only`, `-plain` (skin off), and the
result screen `practice-result-1440x900`, `-1280x800`, `-3840x2160` (the breakdown in two
columns from 1280x800 up), `-result-music` (marked less certain).

## 5. Not measured here

- The real core: everything above runs on the browser mock (APP_API 12 as specified). The
  marker's sound-to-screen delay, the 20-minute real-app run with practice on and the
  allocation test belong to the stages that wire the core.
- WebView2 inside the app and WKWebView on the MacBook Air: not measured. The self-check
  reports the lane's CPU-raster cost in the real web view (`selfcheck_run(..., lane)` from
  the Self-check window; in `--selfcheck` runs the core is to evaluate
  `window.__jkSelfcheckLane()` and receive `selfcheck_lane`, not yet implemented in the
  core). In a web view without `willReadFrequently` the figure is an over-estimate (it
  then includes the GPU readback).
- Nobody has sung into it: the singer on screen is the mock's scripted one.

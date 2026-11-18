# Jukeoke architecture

This is the shared map every part of the project builds against. The two
interface contracts are separate documents and win over anything here:

- `docs/SONG_FORMAT.md` - the `.jksong` file.
- `docs/STUDIO_PROTOCOL.md` - messages between the app and the Studio engine.

The product requirements are in `../WORK_ORDER.md` (outside the repo). Decisions
and their reasons are in `DECISIONS.md`.

## 1. Processes

```
+---------------------------- Jukeoke app (one native process) ----------------------------+
|  Web UI (Preact + TypeScript, in the system web view; jukebox skin on top, optional)     |
|     |  Tauri commands + events (JSON, docs/APP_API.md). UI never touches audio samples.  |
|  Rust core (Tauri 2, app/src-tauri; section 11)                                          |
|     - library scan + watcher, settings, song file I/O   (crate jk-format)                |
|     - audio engine: decode -> mix -> pitch -> resample -> device   (jk-engine, jk-dsp)   |
|     - recorder: WASAPI loopback / ScreenCaptureKit  (jk-recorder)                        |
|     - Studio supervisor: spawns + talks to the Studio process (JSON lines over stdio)    |
|     - export, self-check, start-up error dialog, portable paths                          |
+------------------------------------------------------------------------------------------+
            | stdin/stdout JSON lines (docs/STUDIO_PROTOCOL.md), Windows only
+------------------- Studio engine (separate Python process, started on demand) -----------+
|  decode (PyAV) -> separate (PyTorch CUDA) -> lyrics -> align -> analyse -> write .jksong |
+------------------------------------------------------------------------------------------+
```

The Mac build has no Studio. The player never imports Python.

## 2. Repository layout

```
Cargo.toml                 Rust workspace (members: crates/*, app/src-tauri)
.cargo/config.toml         target + linker settings (Windows GNU toolchain, static runtime)
branding.json              THE ONLY place the app name / ids live (see section 9)
crates/
  jk-format/               .jksong reader + writer, lyrics model, settings slots. No audio deps.
  jk-rubberband/           vendored Rubber Band Library (GPL) + safe Rust wrapper
  jk-dsp/                  deterministic DSP: mix bus, pitch path, drum delay, resampler,
                           offline renderer. No threads, no I/O, no OS calls.
  jk-engine/               real-time player (`api.rs` is its contract with the app): stem
                           reader, render-ahead worker, house-keeping thread, device
                           output (WASAPI / Core Audio / null), playback clock, export,
                           FLAC writer, generated demo song
  jk-recorder/             system-audio capture: WASAPI loopback (Windows),
                           ScreenCaptureKit (macOS 13+), writes float WAV / RF64
  jk-cli/                  `jukeoke-cli`: info, verify, render, export, play, seek-test,
                           devices, make-demo-song, make-test-song, gen-test-stems,
                           render-stems, bench, ci-render
app/
  src-tauri/               the Tauri app crate `jukeoke` (layout in its README.md):
                           core.rs + commands/ (APP_API), playback, library, settings,
                           export, recorder, studio (supervisor), selfcheck, startup,
                           portable; tests/fake_studio.py; resources/ (demo song)
  ui/                      Vite + Preact + TypeScript front end: src/api (types, Tauri
                           client, browser mock), state/, components/, lib/ (clock, keys),
                           ticker/, skin/ (jukebox look), scripts/ (baseline checks,
                           screenshots, real-app checks)
studio/                    Python package `jukeoke_studio` (pyproject.toml, uv.lock)
docs/                      specs, architecture, user docs, benchmarks/
scripts/                   build, dist, test and dev-environment scripts
tests/                     cross-component tests (conformance/) and generated fixtures
                           (fixtures/generated: demo-song.jksong, tiny.jksong)
.github/workflows/         CI (Windows + macOS) and an untriggered release workflow
```

Outside the repo, next to it: `../jukeoke-build/` holds heavy local state (Python
runtimes, the Studio venv, model cache, evaluation datasets, benchmark audio). It is
never committed. `../reference-images/` and `../test-songs/` are never committed.

## 3. Toolchains

- Rust stable, host `x86_64-pc-windows-gnu`, C/C++ from WinLibs GCC (MSVCRT, POSIX
  threads). Link libgcc / libstdc++ / winpthread statically, so `Jukeoke.exe` depends
  only on DLLs that ship with Windows 11. Verify with `objdump -p <exe> | grep "DLL Name"`.
- macOS: `aarch64-apple-darwin`, `MACOSX_DEPLOYMENT_TARGET=13.0` for Rust, C, C++ and
  Objective-C. On Windows we can only `cargo check --target aarch64-apple-darwin`
  (type-checks, no link); the real build runs in CI or `scripts/build-mac.sh`.
- Floating point: compile C/C++ with `-ffp-contract=off`, never `-ffast-math`, on every
  platform, so Windows (x86-64) and Mac (ARM64) produce the same samples within float
  tolerance. Rust does not contract FMAs by default; do not use `mul_add` in DSP paths
  unless both platforms take the same path.
- Python 3.12 (python-build-standalone via uv), PyTorch 2.14.1+cu130 (verified on the
  RTX 5070 Ti, sm_120). `source scripts/devenv.sh` puts everything on PATH for a shell.
- UI: Node 25 / npm, Vite, Preact, TypeScript. Build target `safari16.4` + `chrome120`.

## 4. Audio engine (jk-engine + jk-dsp)

Goal order: fidelity, no dropouts, determinism across platforms, low CPU.

**Signal path, per song** (all 32-bit float, at the song's native rate until the last step):

```
stems (24-bit FLAC in the song file, x stored gain)
  lead_vocal  x guide level (0..1, default 0)  --\
  backing     x on/off x level                 ---+--> PITCH BUS --> Rubber Band R3 (real-time,
  other       x level                          --/                   OptionEngineFiner,
                                                                     OptionChannelsTogether)
  drums       x level --> DRUM BUS --> delay line = pitch path latency (to the sample)
                          (or into the PITCH BUS when "shift drums" is on)
PITCH BUS out + DRUM BUS out --> resampler (only if device rate != song rate)
  --> output ring --> device callback x master volume (smoothed)
```

- **Key shift 0 = bit-transparent path.** At 0 semitones the pitch bus bypasses Rubber
  Band through a delay line of the same latency, so the output equals the plain mix
  (measured: difference 0).
- **Unknown stem roles** (newer files): pitched in-mix stems are summed at unity into an
  extra pitch-bus input, unpitched ones into an extra drum-bus input; stems not in the
  mix are not decoded. With no extras the output is bit-identical (D-62).
- **Key changes** (DECISIONS D-31, D-60): each key uses its own fixed-pitch R3 instance.
  Instances are built ahead on the house-keeping thread and handed to the render worker
  through a lock-free ring (2 spares); the worker primes the new one with `prime_len`
  frames (4096 at 44.1/48 kHz) of the real audio before the change and crossfades
  (40 ms, equal-power) from the old path to the new one. Rubber Band is idle at 0. The
  change is heard about 280 ms after the click. The skin's key dial commits 160 ms
  after the hand stops, so spinning it does not build one instance per detent.
- **Render ahead and cache** (WORK_ORDER 4.5 last bullet; crates/jk-engine/src/keycache.rs,
  jk-dsp's cached route). Mode `realtime` is the path above. Mode `prerender` (setting
  `audio.key_mode`; "auto" picks it when Rubber Band measured below 3x real time on this
  computer and rate, or before any measurement) renders the pitch path (the graph with
  `pitched_only`: the shifted mix of lead x guide + backing + other, + drums with "shift
  drums") on a below-normal-priority thread into `<Settings>/cache/keys/` (raw f32, one
  file per song + key + pitched levels, `index.json`, LRU eviction above
  `audio.key_cache_mb`, partial files deleted at the next start). Rendering starts at the
  play position (cells of 32768 frames; each run starts like a real-time seek: reset +
  priming), then fills the rest of the song; where two runs meet, the earlier one
  crossfades (40 ms equal power) into the later one's pre-roll, so the file never jumps
  and written cells are never rewritten. Control (house thread) keeps the previous key
  until 5 s are rendered ahead with a renderer above 1.1x real time (else to the end of
  the song), then hands the worker a feed (a 2 s lock-free ring filled by a reader
  thread) and the worker's graph crossfades to its `Cached` route: cached frames + the
  live drum bus, the same arithmetic as the real-time route, so the output is
  bit-identical for the same start point (tests/keycache.rs). Seeks start on the cached
  key when it is ready there, otherwise on the previous key, and move the renderer
  there first. A missing cache block fades to the bypass before the gap (never seen in
  tests). Mixer levels: the drum level stays live; another level renders a new entry
  while the old mix plays (per-stem caches were rejected: shifting stems separately is
  not the same waveform as shifting the mix, residual about +1 dB, at 3x the CPU).
  Disk: about 90 MB per key for a 4-minute 48 kHz song; memory: 2 s rings + one
  renderer graph. The render worker and the callback still never allocate or lock.
- **One latency for the whole graph.** Every path is delayed to the same total latency L
  (longest R3 FFT x 9/8: 4608 frames at 44.1/48 kHz). Drums and the bypass path are
  delayed by exactly L; per-key correction tables remove R3's systematic transient bias.
  The clock subtracts L. Proven by the impulse tests in docs/benchmarks/dsp.md.
- **Threads** (crates/jk-engine/src/engine.rs has the diagram).
  (0) Control (API calls, under a mutex audio threads never touch): opens songs and
  devices, allocates rings and graphs, hands them to the worker as boxed commands.
  (1) Stem reader, one per song: decodes all stems from the song file (stored gains
  applied) into one SPSC ring holding about 4 s, so a slow USB stick cannot starve
  playback. Each load or seek gets a new ring (no flushing of shared rings).
  (2) Render worker (MMCSS "Pro Audio" on Windows, QoS user-interactive on macOS): runs
  the jk-dsp graph and the output resampler in 512-frame blocks, keeps the output ring
  about 150 ms ahead, measures output peak and clipping (no limiter). It never
  allocates or frees.
  (3) Device filler (the callback): copies from the output ring, applies fades and the
  master volume (an atomic, ramped), keeps the clock. No allocation, no locks, no
  syscalls besides the audio API's own, no logging. Underruns write silence and bump an
  atomic counter.
  (4) House-keeping thread (low priority): builds Rubber Band instances, frees replaced
  objects that come back through a garbage ring, and turns callback, reader and device
  flags into events (ended, device lost, errors).
- **Parameters** go UI -> Rust command -> engine control -> SPSC command ring -> render
  worker. A change while paused re-renders the buffered audio (an in-place seek to the
  current position), so play starts with the new mix. Pending parameters are dropped on
  song load and unload.
- **Playback clock** (D-59). The filler keeps a log of what it wrote as spans (song
  audio from position p, or silence while paused at p), walks back by the latency the
  device reports (frames queued + stream latency) and publishes the song time of the
  sample now leaving the DAC with the callback's timestamp (`Snapshot.position_ns`).
  This holds across pause, seek, underrun and song end. The app's clock thread emits
  `playback://clock` about 30 times per second (absolute 33.3 ms deadlines), with the
  position extrapolated to the emit time (at most 100 ms). The UI (app/ui/src/lib/
  clock.ts) interpolates with `performance.now()` and re-anchors on every event:
  errors under 250 ms are absorbed by up to 5 % speed change, larger ones snap;
  extrapolation stops 250 ms after the last event; a pause or seek the UI asked for
  freezes or holds the display until the engine confirms. It never runs a free timer.
- **Seek / song change** (D-58): the device stream keeps running. Control bumps a
  generation counter and fills a new stem ring from a sample-accurate decoder seek that
  starts `prime_len` frames (the longest R3 FFT: 4096 at 44.1/48 kHz) before the target
  (real audio primes Rubber Band: no zero-padding pops); the worker resets the graph,
  discards output before the target and writes a `Start` marker into a second SPSC ring
  (`End` after the song's last frame); the filler fades the old audio out over 10 ms,
  drops the rest of that generation, waits until 50 ms of the new one is buffered and
  fades it in over 10 ms. Seek latency about 40-49 ms at the DAC.
- **Devices** (D-64). The output stream stays open from start-up until a device switch,
  playing silence when idle. Windows: WASAPI via the `wasapi` crate, shared mode at the
  device mix format, optional exclusive mode at the song rate. macOS: Core Audio via
  `cpal` 0.16, device nominal rate. A `null` / `null:<rate>` device consumes audio in
  real time silently (tests, headless self-check). Opening tries the chosen device as
  asked, then in shared mode, then the default (exclusive, then shared), and reports
  `exclusive_unavailable` when it fell back. A missing device never fails start-up;
  device loss pauses, keeps the position and emits an event; play reopens.
- **Resampler.** Used only when device rate != song rate. High-quality windowed sinc
  (`rubato`, long filter). Record each place a resample happens in DECISIONS.md.
- **Memory.** Never decode whole songs. Budget for a playing song with bubbles on:
  < 1 GB for all processes on the Mac (target < 300 MB for the Rust process). Measured
  on Windows: 49.8 MB peak working set for a 4-minute stereo song; 63.9 MB in the
  20-minute soak (docs/benchmarks/playback.md).
- **Offline render** (`jk-dsp::render`, `jk_engine::render_offline`) runs the exact same
  graph single-threaded (Rubber Band built inline, identical output) for export, CI
  cross-platform comparison and tests. Export (D-67): Wav32f is the render bit for bit;
  Flac24 is scaled down only when the render exceeds full scale, and the applied gain is
  reported to the UI.

## 5. Studio engine (studio/, Python)

- Runs only on Windows with a CUDA GPU (capabilities are probed at startup, section 7).
- Package `jukeoke_studio`; entry `python -I -m jukeoke_studio serve` speaks the
  protocol on stdio; `python -I -m jukeoke_studio make <file>` runs one song from the
  command line (used by tests and benchmarks).
- Stages: decode (PyAV, float32, native rate, tags + cover) -> separate -> lyrics ->
  align -> analyse (key, tempo) -> write `.jksong` (Python writer in
  `jukeoke_studio/jksong.py`, conformance-tested against the Rust reader).
- Separation: details and model choices in `docs/benchmarks/separation.md` and
  `MODELS.md`. Models run in fp32 unless a benchmark shows a faster mode is identical.
  If a model needs 44.1 kHz and the source is not 44.1 kHz, only the model's input and
  its estimate are resampled (soxr VHQ); the original audio is never resampled, and the
  residual stem is computed at the native rate.
- Stem definition ("full" preset, default): lead, backing, drums from the models;
  `other = original - lead - backing - drums`, so all stems at unity sum back to the
  original within 24-bit quantisation.
- Relocatable runtime: a python-build-standalone interpreter with packages installed
  into it directly (no venv, no absolute paths), launched with `-I`. Model weights
  live in `<root>/Models/`, downloaded on first use, never committed.

## 6. UI (app/ui)

- Preact + TypeScript + Vite. All core calls go through one `Api` interface
  (`src/api/client.ts`): `TauriApi` inside the app, `MockApi` (demo library, simulated
  clock) in a plain browser and in tests. State in `src/state/`, screens in
  `src/components/` (Library, Stage, Mixer, Recorder, Studio, SyncEditor, Settings,
  overlays), the clock model in `src/lib/clock.ts`.
- Plain functional screens first; the jukebox skin (`src/skin/`, a self-contained module
  that never talks to the core) wraps them when `settings.ui.skin` is on (default):
  `App.tsx` draws the window as a `JukeboxFrame` (badge on the crest, plate navigation
  along the top of the screen, transport plates + `KeyDial` + Guide/Backing/Master
  faders in the console, library as title-strip cards); `src/skin-app.css` restyles the
  plain screens for the dark screen. Skin off = the plain layout the tests use.
- Baseline browser: Safari 16.4 (WKWebView on macOS 13.6.9) and WebView2 (Chromium) on
  Windows. No WebGPU. Any newer CSS/JS feature needs a check and a fallback. `npm run
  build` fails unless every emitted JS file parses as ES2022 and the CSS passes a
  caniuse check against Safari 16.4.
- Lyrics display (`settings.ui.lyrics_display`, D-218): `page` (default since the owner's
  request of 18 Nov 2026: "the phrase of the lyrics to appear on screen, stay static,
  while the progress bar goes over which word is being sung") or `scroll` (the ticker
  below, unchanged). Stage and Sync editor draw the same one.
- Page display (`src/ticker/page.ts` layout and timing, pure and unit-tested;
  `pageverify.ts` its checks; `pagerenderer.ts`; `PageLyrics.tsx`): Canvas 2D, the
  current line static, centred, bold (Atkinson Hyperlegible Next 800 in the skin) with a
  dark outline and shadow; the next line under it at 0.62x and 62 % opacity
  (`ui.show_next_line`). A colour wipe (sung colour left of the edge, upcoming right,
  default gold `#ffc21a` / white `#f4f4f4`) sweeps through the line in reading order. The
  edge's position is the line's own motion built with the ticker's pacing unchanged
  (`layout.ts` `wordKnots` + `wordmotion.ts` `RampMotion` for the word pace, the letter
  cells + `MonotoneSpline` for `lyrics_motion = "letter"`), on the line laid out as a
  one-row strip, with a 0.3 s run-up and run-out outside the text; the same contract
  (`verify.ts`) is checked on every line. A line too wide shrinks to 0.7x, then wraps onto
  two (at most three) balanced rows; a short area shrinks the song. Short lines are
  joined into phrases (only across pauses under 1 s, while the phrase fits two rows) until
  each display line is previewed at least 2 s before it is sung (with the preview on).
  Line changes start within 0.05 s of the last word (0.2 s: the next line moves up and
  grows, the old one moves up and fades); only after a display line shorter than the
  change does the next change start later (at most 0.25 s), and a line without a preview
  never waits for it past 0.2 s before its own first word. After a pause of 1 s or more a lead-in bar left of the line drains
  over up to 3 s and is gone exactly at the first word; rests of 4 s or more clear the
  screen and show the countdown, the line returns 3 s before it is sung. Every frame is
  `pageFrame(layout, t)`, a pure function of the lyric time. Each row is pre-rendered
  once per colour (and once at the preview size) in device pixels, a few lines kept
  (LRU), the line after the preview rendered ahead; a frame is two clipped blits per row.
  Same frame loop as the ticker (60 fps cap, stopped when idle, hidden or minimised).
- Progress needle on the page display (`settings.ui.show_marker`, default on, D-219; the
  owner's request of 18 Nov 2026 to bring back "the bar ... that would mark the progress
  of where the song is"): the ticker's marker needle (`renderer.ts` `drawNeedle`, shared;
  colour `colors.marker`; sized in em, the ticker's 3 px at 56 px) rides the wipe edge.
  `pageFrame(...).needle` (`page.ts` point 6) puts it on the line in the current slot, on
  the row being wiped (`needleRow`: the last row the wipe has entered), at that row's
  sung width; the renderer draws it last, at the x where it split that row between the
  sung and the upcoming bitmap (same expression, same snapped origin), over the row's band
  plus 0.12 em, with the letters' shadow. Before a line's first word it waits at the first
  letter, after the last word at the end; at a row break it moves to the next row's start
  when the wipe enters it; at a line change it rides the next line up into place; it
  fades out and in with the lines around a rest with the countdown and is hidden in the
  rest. No layout work per frame; on/off and colour need no new bitmaps.
  `pageverify.ts` (n) checks it on every sampled frame (on the boundary within 0.5 px,
  right line and row, parked in lead-ins, hidden in rests, inside the area); the
  scrolling ticker keeps its fixed marker.
- Lyrics ticker (`src/ticker/`): Canvas 2D. Each grapheme cluster is measured and drawn
  individually at a precomputed x position (ligatures and contextual alternates off).
  The marker stays fixed and the text slides under it (D-04); how the text is paced
  under the marker is `settings.ui.lyrics_motion` (D-216):
  - `word` (default since the owner's request of 18 Nov 2026, "word by word instead of
    jumping"): word-level knots (`layout.ts` `wordKnots`): a word's first letter
    reaches the marker when the word starts, give or take 40 ms; the word carries the
    space after it at its own pace, so the text never dashes across a space; in a short
    pause the word just sung stays at the marker up to 0.25 s longer, a longer pause
    slows the text down. A knot moves (within the 40 ms) only where the speeds on its
    two sides would differ by more than 2x. Between the knots, `wordmotion.ts`
    `RampMotion`: one steady speed per interval (a held word crawls, quick words pass
    fast), blended into the next by a smoothstep ramp reaching up to 0.35 s into each
    neighbour, the plateau speeds solved (tridiagonal) so every knot is passed exactly.
    Speed and acceleration are continuous, the speed is never negative and never zero
    while a line is sung. The colour split at the marker becomes a smooth wipe
    through each word.
  - `letter` (the previous behaviour, unchanged): each letter owns a cell (glyph box
    plus at most half a neighbouring separator); the time-to-x mapping is a monotone
    cubic (Steffen) through every cell edge, so the marker can never leave the letter
    being sung (D-94, D-95); the text surges on short consonants.
  Both paces share the placement, the breaks and the lookahead rule: gaps are sized so
  the next line enters 3 s before it is sung; the font shrinks (to 0.6x at most, single
  dense lines to 0.45x) to keep 2 s of text ahead (D-96, D-167). Text is pre-rendered into 1024 CSS-px tiles per colour (sung and
  upcoming, LRU cache); each frame blits them with a clip at the marker. Capped at
  60 fps, stopped when idle, when the page is hidden, or when the native window is
  minimised (Tauri's resize event + `isMinimized`, as for the skin; WebView2 keeps a
  minimised page "visible"; `src/ticker/frameloop.ts`).
- Skin animation (D-104, D-107): the light drift is a Web Animations transform on
  gradient strips inside masked regions, stepped 12 times a second, one palette cycle per
  40 s; static art is SVG/JPEG images (cached rasters, no filters re-run on repaint);
  bubbles are one canvas at <= 30 fps and <= 60 bubbles. Both stop when the page is
  hidden or the native window is minimised, and under reduce-motion (setting or OS
  preference); bubbles have their own switch. Nothing animates inside the screen except
  the ticker.
- Fonts are bundled (SIL OFL): Oswald (display), Atkinson Hyperlegible Next (lyrics),
  Pacifico (badge). Nothing is loaded from the network.
- No audio in the web view, ever.

## 7. Capability detection

At startup the Rust core works out: OS and version, RAM, whether a Studio runtime is
present next to the app, and (by asking the Studio `probe` command, with a timeout)
whether CUDA works and how much VRAM there is. Missing pieces make Studio features
show as unavailable with a plain explanation; playback never depends on them. The probe
runs once in the background after the window is up (start timeout 60 s); until it
answers, `capabilities.studio.checking` is true and the reason reads "Still checking
whether this computer can run the Studio...". A missing output device never stops
start-up either (the engine reports `no_device` on play).

## 8. Portable mode and paths

The app finds its root by walking up from the executable: on Windows the exe is in
`<root>/Windows/`, on macOS the bundle is in `<root>/Mac/`. If `<root>/Library` exists
(or `<root>/portable.txt`), the app is in portable mode and every write goes under
`<root>`: `Library/`, `Recordings/`, `Exports/`, `Models/`, `Settings/` (settings.json,
recordings-index.json, `logs/`, self-check reports, and the WebView2 user-data folder
`webview-windows/` on Windows). Nothing uses an absolute path that survives a restart;
paths are stored relative to `<root>`. The mode is one of `portable`, `dev` (data in
`$JK_BUILD/dev-data`), `user` (per-user app data folder) or `translocated` (D-40, D-87).

- WebView2 profile (D-86): kept between runs, renamed aside, deleted and the window
  retried once if window creation fails; `WEBVIEW2_USER_DATA_FOLDER` is removed from the
  environment at start so it cannot move the profile off the stick. macOS uses a
  non-persistent WKWebView store.
- macOS translocation (D-87): a translocated (quarantined) app runs with its data in a
  temporary folder and asks for the stick's folder (`root_choose`); the choice is
  remembered in `~/Library/Application Support/org.jukeoke.app/root.txt`, the only
  write outside the stick.
- Windows itself records a few entries for any exe (WebView2 EdgeUpdate `lastrun`,
  Explorer MuiCache, the audio service's PolicyConfig PropertyStore); the portable
  audit lists them as OS-written (D-41, D-88).

Writes of song files and settings are atomic (unique temp file, fsync, rename with
write-through on Windows, parent-directory fsync on Unix; D-81), except the song file's
settings slots, which are designed for in-place A/B updates (see the spec). Library
scans ignore `._*` AppleDouble files, other dotfiles and unfinished `*.tmp-*` writes.

## 9. Naming

`branding.json` holds the product name, bundle identifier, file extension and
display strings. Build scripts and code read it; no other file hard-codes "Jukeoke"
except where a tool forces a literal (those places are listed in `branding.json`
under `"also_edit"` so a rename is a checklist, not a search).

## 10. Testing (maps to WORK_ORDER section 9)

- Rust: `cargo test --workspace` (unit + DSP property tests: pitch accuracy in cents,
  duration, impulse alignment, transparency at 0, determinism of offline render; engine
  tests on the silent `null` device; app-core tests against fake engines and a fake
  Studio, `app/src-tauri/tests/fake_studio.py`).
- Python: `pytest studio/tests` (pipeline pieces with generated audio; GPU tests marked).
- UI: `npm test` (vitest + happy-dom: ticker letter-under-marker check across whole
  songs, including the demo song, at several window sizes; UI flows on the mock; skin
  units), `npm run screenshots` (headless Edge via playwright-core), and
  `npm run real-app-check` / `real-app-screens` (the real exe driven over WebView2's
  DevTools port, null device, master 0).
- App: `jukeoke --smoke-test` (window + UI round trip, exit 0/1/2/3) and
  `jukeoke --selfcheck --silent` (the WORK_ORDER 4.10 self-check, exit 0/1).
- Cross-component: `tests/` (Python writer -> Rust reader conformance, end-to-end
  Studio -> app render) and the portable-mode write audit
  (`app/scripts/portable-smoke-test.sh`, including a run from a `subst` drive letter).
- Every audio claim has a measured number. Results go to `docs/benchmarks/`
  (dsp.md, playback.md, recorder.md, alignment.md, analysis.md, separation.md).
- Nothing may play aloud above -60 dBFS in tests; device tests play digital silence.

## 11. App core (app/src-tauri)

`Core` (core.rs) holds everything the commands use and is tested without a window;
`commands/` are thin async wrappers (one file per APP_API area) that run on Tauri's
blocking pool, never on the main thread. The engine is reached through `AudioEngine` /
`Renderer` traits so tests can use fakes.

- **Playback** (playback.rs): load / play / seek / set; per-song settings saved to the
  song's slot debounced 1 s and flushed on song change and exit; the clock thread
  (section 4) also emits `playback://state` and `playback://error`.
- **Settings** (settings.rs): `<root>/Settings/settings.json`, deep-merged, clamped,
  saved atomically after 300 ms; a bad value falls back alone, unknown keys are kept.
- **Library** (library.rs): recursive scan, summaries cached by (size, mtime), cover
  thumbnails cached in memory, a recursive file watcher coalescing bursts over 400 ms
  into `library://changed`, free space of the Library's volume.
- **Export** (export.rs): `render_offline` on a worker thread into `<root>/Exports/`,
  progress events, cancel.
- **Recorder** (recorder.rs): jk-recorder session, `recorder://level` about 20 times a
  second, recordings index with the clip flag.
- **Studio supervisor** (studio.rs, D-83): starts the Studio on demand
  (`<root>/Windows/Studio/python/python.exe -I -X utf8 -m jukeoke_studio serve`, with
  `JUKEOKE_MODELS_DIR=<root>/Models`), JSON lines over stdio, request timeouts, a job
  queue whose ids the app chooses, re-send after a crash, give up after 3 crashes in
  10 minutes, stop after 10 minutes idle, log in `Settings/logs/studio.log`.
- **Self-check** (selfcheck.rs, D-84): System, Web engine, Memory, Output device, Test
  tone (-60 dBFS peak, or digital silence with `--silent`), Recorder (3 s capture),
  Demo song (bundled resource, rendered offline), Key change (+2 / -5 within 5 cents).
  Report `Settings/selfcheck-<local time>.txt`; also `--selfcheck` from the command line.
- **Start-up errors** (startup.rs, D-85): written to `Settings/logs/startup-error.txt`
  and shown in a native dialog, exit 1; never a panic inside Tauri.
- **Portable paths** (portable.rs): section 8.

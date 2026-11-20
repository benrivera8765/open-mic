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

Practice mode (Work Order 2, section 12) adds microphone capture (jk-recorder::input) and
pitch analysis (jk-pitch) to the core, on both systems; the Studio is not involved.

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
  jk-pitch/                practice mode DSP (section 12): guide builder, live pitch
                           tracker, own-playback echo detection/removal, scorer. Pure
                           functions: no threads, no I/O, no OS calls.
  jk-recorder/             system-audio capture: WASAPI loopback (Windows),
                           ScreenCaptureKit (macOS 13+), writes float WAV / RF64;
                           microphone input for practice mode (`input`, D-240);
                           media-player link (Windows media sessions, `media`),
                           recording sessions + splitter (`session`), preview player
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
`<root>`: `Library/`, `Recordings/`, `Exports/`, `Models/`, `Deleted/`, `Settings/` (settings.json,
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

`Deleted/` (D-230, the owner's request of 18 Nov 2026) is the app's own recycle bin on
the stick, a sibling of `Library/`, created on the first delete: files deleted from a USB
stick bypass the Windows Recycle Bin (and a Mac keeps them in a hidden `.Trashes` folder
only until that Mac empties its Trash), so "delete" moves the song file there, and
`Deleted/index.json` remembers where each came from. The Library scan and
watcher never look inside it. The space it takes is shown in the Library header until the
songs are deleted for good.

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
- **Deleted songs** (deleted.rs, D-230): `library_delete` first releases the song if it
  is loaded (`Player::release_during`: no current song, engine paused and unloaded, then the
  pending slot save flushed; the move runs under the load lock, D-233), waits until nobody holds the file (Windows: an open with no
  sharing succeeds, at most 2 s), then moves it into `<root>/Deleted/` with a unique
  name (`fs_atomic::rename_new`, never replacing; across volumes a verified copy, then the
  original removed, else the copy removed). `Deleted/index.json` gets the entry, marked
  with the operation in progress, before the move and settled after; `DeletedSongs::list`
  (at start-up and before every operation) repairs an interrupted delete or restore
  without ever removing the only copy of a song. Restore goes back to the original place
  or a free name next to it; purge removes the file for good.
- **Export** (export.rs): `render_offline` on a worker thread into `<root>/Exports/`,
  progress events, cancel.
- **Recorder** (recorder.rs, recsession.rs, medialink.rs): jk-recorder session,
  `recorder://level` about 20 times a second, recordings index with the clip flag. Three
  modes share the one capture (the owner's request of 18 Nov 2026, D-221..D-228):
  - *plain*: the Record button.
  - *record_and_play*: the capture starts, the app waits until its first buffer is in
    (`Recorder::has_audio`, at most 3 s), then tells the chosen media player to play
    (Windows' Global System Media Transport Controls, `jk_recorder::media`, through the
    `MediaLink` trait; tests use a fake that never touches a real app). "Stop & pause"
    stops the capture, then pauses that player.
  - *session* ("Record songs separately"): one continuous master in
    `Recordings/<date time>/master.wav`; a `MediaWatch` reports every track or playback
    change of the player with the instant it was noticed; the instant is placed on the
    master's timeline by `CapturePipeline::timeline_frame_at` (anchored on the newest
    captured block, so device-clock drift does not accumulate), and a `MarkerBuilder`
    turns the reports into markers (new title/artist = new track; empty metadata
    ignored; repeat = position back to the start). `session.json` is saved atomically on
    every change; the cover is fetched 0.7 s and 2.5 s after a change, only while the
    player still shows that song. A monitor thread stops the session after the player
    has been paused/stopped for the chosen time (default 30 s), after the song limit,
    or when the player closes. Then `jk_recorder::session::split` runs on a background
    thread: each track change is cut in the middle of the nearest run of near-digital
    silence (all channels below -70 dBFS for at least 100 ms) within 2.5 s of the event
    and no further than half-way to the neighbouring changes (D-229), or at the event
    ("uncertain") without one; "playback ended" keeps at most 1 s of the
    silence; parts are copied sample for sample into float WAVs (temporary file,
    flushed, renamed), read back and hash-checked against the master; songs get a
    sidecar JSON and their cover; advertisements, the tail after playback ended and skips
    under 5 s go to `_excluded/`; the master is deleted only if every part matched and
    the parts cover it exactly. Sessions a crash or a closed app left behind are finished
    at the next start (`wav::repair` of the master, then the split; files already
    written and matching are kept).
  Recorded songs are previewed natively (`jk_recorder::preview`, WASAPI shared mode at
  the file's format; the web view never plays audio). The Studio picks up a recorded
  song's sidecar in `studio_inspect` and as its cover in `studio_submit`.
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

## 12. Practice mode (Work Order 2)

The singer sings into a microphone; the singing screen shows the guide's notes as bars
in a lane with a piano keyboard and a marker at the pitch sung; the take is graded at
the end. Contracts: docs/SONG_FORMAT.md sections 7.1, 10, 11 (guide and practice data),
docs/STUDIO_PROTOCOL.md section 12 (guide stage), docs/APP_API.md section 12 (commands,
events, the pitch/key/octave rule). Decisions D-234..D-240 and the Stage 0 reports in
`docs/benchmarks/practice-*.md`.

### 12.1 Where each piece lives

| Piece | Where | Threads / I/O |
|---|---|---|
| Guide builder (pYIN curve -> notes, octave check, tuning; D-236) | `jk-pitch::guide` | none (pure) |
| Live tracker (MPM, 10 ms hop, one channel; D-238) | `jk-pitch::tracker` | none |
| Own-playback detection and removal ("music in the input") | `jk-pitch::echo` | none |
| Scorer (per-note credit, bands, tendency, breakdown, octave decision) | `jk-pitch::score` | none |
| Microphone capture (`InputSource`; WASAPI / cpal 0.16; rings, meters; D-240) | `jk-recorder::input` | OS capture thread |
| Key heard, `key_at` history (D-239); output tap for the echo reference (Stage 3) | `jk-engine` | render worker, house thread |
| Guide chunks, practice slot data, raw-copy rewrite | `jk-format` (+ `jksong.py`) | file I/O |
| Guide queue (new songs after `job.done`, "Add practice data", on-the-spot builds) | app core `guide.rs` | one builder thread |
| Practice session (open input, analysis thread, results to the slot, events) | app core `practice.rs` | analysis thread |
| Lane and result screen (pure layout + verify + Canvas 2D renderer) | `app/ui/src/practice/` | the existing frame loop |
| `jukeoke-cli guide add/show`, `verify` additions | `jk-cli` | |

`jk-pitch` has no threads, no I/O and no OS calls, so the same code runs on Windows and
the Mac and every piece is tested offline with generated input (WO2 section 6). The UI
only draws.

### 12.2 Data flow

```
microphone / mixer (USB)                               engine render worker
   | OS capture callback: copy samples + host-clock       | output tap: copy of the output mix
   | stamp into lock-free rings, nothing else (D-240)     | + song frame into a lock-free ring
   v                                                      v  (no allocation, no lock; rt_alloc test)
 input rings (2 s) ------------------> practice analysis thread "jk-practice" <---- tap ring
                                         1. read blocks; meters per channel (mic://levels)
                                         2. take the chosen channel (never a sum)
                                         3. echo: reference (tap, resampled to the input rate,
                                            aligned by the two clocks) vs input -> detect,
                                            measure delay/level, remove when stable;
                                            Trust per stretch (Clean/Singer/Music/Unknown)
                                         4. tracker on the cleaned channel -> PitchEvent
                                            {time, ready_at, hz|None, clarity, level};
                                            never told the expected note (WO2 trap 6)
                                         5. map event.time -> host time -> song time + segment
                                            (section 12.3); pitch in a Music stretch -> None
                                         6. scorer: target = guide note + key_at(t, segment)
                                            + 12 x octave (APP_API 12.1); per-note credit,
                                            octave decision (auto), running totals
                                         7. every 16 ms: practice://live batch
                                            {pitch[], hits[], done[]}; practice://state on change
                                         8. take end: PracticeResult -> song slot (practice key)
                                            -> practice://result
                                                      |
                                                      v
                       UI: lane = pure function (layout, clock time, key heard/pending,
                       octave, guide, recent pitch values) -> frame; Canvas 2D in the
                       existing 60 fps loop (bars drawn once and moved, WO2 trap 13)
```

Nothing from the analysis thread reaches the output: with practice on, the engine's
output is bit-identical (WO2 section 5). Microphone samples live only in the rings and
the analysis thread's fixed buffers; nothing is written or sent.

The guide reaches the session through `practice_guide` (notes, spans, curve, tuning) when
practice starts; the lane draws from it and from the clock, the core grades from the
same data, so drawing and grading cannot disagree.

### 12.3 Clocks

Three clocks meet here (WO2 trap 4: the input and output devices drift apart, up to
about 24 ms over a song):

- **Host clock**: QPC on Windows, `mach_absolute_time` on the Mac. Every input block is
  stamped with the host time of its first frame by the capture thread (WASAPI's QPC
  position; cpal's `mHostTime`), never by counting samples. The engine's `now_ns` is
  `std::time::Instant` from a process epoch, which is the same QPC / mach clock: the
  core measures the epoch offset once (both clocks read back to back) and the self-check
  confirms it.
- **Playback (DAC) clock**: the engine's `Snapshot` pairs `position_s` (song time at the
  DAC, the declared output latency removed) with `position_ns` and the `segment`
  (published together by a sequence lock, D-239). The analysis thread keeps the last
  2 s of these anchors (sampled every 5 ms by the house thread or on every snapshot) and
  maps a capture host time to (segment, song time) by interpolating between the anchors
  on either side, so drift never accumulates (each anchor is a fresh device callback).
- **Input sample clock**: the tracker's `time` is in input samples; with the block
  stamps it becomes a host time (frame offset / the input rate, from the newest block's
  stamp backwards), minus the input latency the device declares.

`song_time = dac_song_time(capture_host_time) - latency.applied_ms` (APP_API 12.7). When
the app's music comes back in the input, the echo stage measures how late it arrives
relative to the declared latencies; that residual is exactly the hidden output + input
delay the singer's timing needs (the singer hears the music by the same path), so it is
applied automatically (WO2 4.10 "the delay comes for free"). Otherwise the rig check's
measurement, else the manual offsets (the input's, plus the output device's offset the
lyrics already use), else 0, and the scorer's timing tolerance absorbs the rest. Every
measured delay is this residual and is never added to the declared latencies twice
(APP_API 12.7 "Timing").

**The echo reference in the app** (app core `src/practice/align.rs`, Stage 3b). The output
tap's pieces are stamped with the host time each leaves the DAC, but a timer-driven device
thread wakes with a few milliseconds of jitter, so the reference is lined up once through a
least-squares line over the last ~2 s of stamps and then read continuously (the drift
between the two devices reaches the canceller as the slow slide it follows), lined up again
only after a discontinuity. It is handed 20 ms early (`reference_lead_s`: a return that
comes back up to 20 ms sooner than the declared latencies predict is still found); an input
block waits (at most 120 ms) until the tap holds its reference, which adds roughly
`20 ms - output latency` (usually nothing) to the marker's delay. The tap is converted to
the input rate with the engine's own resampler when the rates differ.

**Key and segment.** Each tracker value is graded against `key_at(song_time, segment)`
from the engine's bounded history, so a note sung before a key change keeps the key it
was sung in, in both key modes (in render ahead the switch is the crossover onto the
cached key, D-239). The lane uses the clock's `key_heard` / `key_pending.at_s`, so bars
move when the key is heard, not when the dial is turned. A seek, device switch or reload
starts a new segment, and values are never mapped across it.

### 12.4 Threads and real-time safety

- OS capture callback (WASAPI event thread, MMCSS "Pro Audio"; Core Audio's HAL thread
  through cpal on the Mac): copies samples and one stamp per block into `rtrb` rings,
  nothing else; a full ring drops the whole block and flags the next. 0 allocations
  measured (D-240). Known on the Mac only: cpal 0.16's input wrapper may grow its own
  buffer when the callback size changes (practice-mic.md, "Not done").
- Engine render worker: one extra lock-free push of the output block into the tap ring
  while practice is on (fixed capacity, drop on full; the `rt_alloc` test keeps covering
  it). The device callback is unchanged.
- Practice analysis thread (one per session, normal priority, never an audio thread):
  everything in 12.2 steps 1-8. Budget on the build PC: tracker 1.0 ms per second of
  audio (practice-tracker.md section 8), echo removal a few ms per second; the M1 is
  estimated at 2-3 times that. It allocates only at session start and on a device
  change.
- Guide builder thread (one build at a time; below normal priority for the batch and new
  songs, normal for a one-song `guide_build` someone waits for, never above normal): about
  1.2 s per 4-minute song on the build PC (estimated 1.7-2 s on the M1), then the atomic
  rewrite.
- Slot writes (practice settings, exclusions, results) go through the existing debounced
  per-song save (section 11 "Playback"), never from the analysis thread directly.

Memory: rings 2 s of input, the tap ring about 1 s, the guide (about 0.4 MB on disk per
4-minute song, a few MB in memory with the curve), the tracker's and canceller's fixed
buffers: a few tens of MB at most, inside the 1 GB budget (WO2 4.8).

### 12.5 What runs on the Mac

Everything in 12.1 except the Studio: the guide builder (so the Mac can add a guide to an
old song with no Studio), capture through cpal 0.16 with the AVFoundation permission
check before opening (macOS hands a refused app silence, not an error), the tracker, echo
handling, scorer and lane. The bundle carries `NSMicrophoneUsageDescription`
(app/src-tauri/Info.plist, checked by `scripts/check-macos-infoplist.sh`); no audio-input
entitlement is needed while the app is signed without the hardened runtime, and
build-mac.sh fails if that changes (D-240). Apple's voice-processing I/O unit is never
used (it would lower and reshape the app's own playback). Nothing newer than macOS 13.0
is called. All of this is type-checked on Windows only; built but not verified until a
Mac report comes back.

### 12.6 Failure paths

- No guide: built on the spot (any computer); if that fails, Practice says why and offers
  to try again; the song plays normally (STUDIO_PROTOCOL 12.2).
- No input, no permission, input unplugged mid-song: watch only (notes and keys, no
  marker, no grade), with the reason and, for permission, the exact steps; the device is
  reopened when it comes back; the take is flagged `input_lost`.
- Music coming back that cannot be removed well (a wandering Bluetooth delay): the
  warning, the marker, and no grade (APP_API 12.5).
- A guide that cannot be written (read-only stick, full stick, file in use): used from
  memory for the session, with the reason; the song file is never left half-written
  (SONG_FORMAT 7.1).

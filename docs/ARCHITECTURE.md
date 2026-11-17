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
|  Web UI (Preact + TypeScript, in the system web view)                                    |
|     |  Tauri commands + events (JSON). UI never touches audio samples.                   |
|  Rust core (Tauri 2)                                                                     |
|     - library scan, settings, song file I/O         (crate jk-format)                    |
|     - audio engine: decode -> mix -> pitch -> resample -> device   (jk-engine, jk-dsp)   |
|     - recorder: WASAPI loopback / ScreenCaptureKit  (jk-recorder)                        |
|     - Studio supervisor: spawns + talks to the Studio process (JSON lines over stdio)    |
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
  jk-engine/               real-time player: stem decoders, render-ahead worker, device
                           output (WASAPI / Core Audio), playback clock, underrun counter
  jk-recorder/             system-audio capture: WASAPI loopback (Windows),
                           ScreenCaptureKit (macOS 13+), writes float WAV / RF64
  jk-cli/                  `jukeoke-cli`: info, verify, render (offline), selfcheck, bench
app/
  src-tauri/               the Tauri app crate `jukeoke` (commands, events, supervisor)
  ui/                      Vite + Preact + TypeScript front end
studio/                    Python package `jukeoke_studio` (pyproject.toml, uv.lock)
docs/                      specs, architecture, user docs
scripts/                   build, dist, test and dev-environment scripts
tests/                     cross-component tests and generated fixtures
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
- **Key changes** (DECISIONS D-31): each key uses its own fixed-pitch R3 instance. A change
  builds a fresh instance in the render worker, primes it with 4096 frames of the real
  audio before the change, and crossfades (40 ms, equal-power) from the old path to the
  new one. Rubber Band is idle at 0. The change is heard about 280 ms after the click.
- **One latency for the whole graph.** Every path is delayed to the same total latency L
  (longest R3 FFT x 9/8: 4608 frames at 44.1/48 kHz). Drums and the bypass path are
  delayed by exactly L; per-key correction tables remove R3's systematic transient bias.
  The clock subtracts L. Proven by the impulse tests in docs/benchmarks/dsp.md.
- **Threads.** (1) Stem readers: one I/O thread decodes FLAC ahead into per-stem SPSC
  rings holding at least 3 s, so a slow USB stick cannot starve playback.
  (2) Render worker (raised priority: MMCSS "Pro Audio" on Windows, QoS user-interactive
  on macOS): pulls equal-length blocks from the stem rings, mixes, pitch-shifts,
  resamples, and pushes into the output ring, keeping it about 150 ms ahead.
  (3) Device callback: copies from the output ring, applies master volume, advances the
  clock. No allocation, no locks, no syscalls, no logging in the callback. Underruns
  write silence and bump an atomic counter.
- **Parameters** go UI -> Rust command -> lock-free queue -> render worker. Master volume
  is an atomic read by the callback (smoothed per block).
- **Playback clock.** Each block in the output ring carries the song-frame index of its
  first frame. The callback publishes (song_frame_at_dac, device_frames_total,
  host_time). song_time_at_dac = frame of the sample now leaving the DAC, i.e. the frame
  just handed over minus the device's reported output latency. The UI receives
  `playback://clock` events about 30 times per second and interpolates between them
  with `performance.now()`, re-anchoring on every event. It never runs a free timer.
- **Seek / song change:** bump a generation counter, flush rings, re-seek decoders,
  re-prime Rubber Band with the real audio before the seek point (no zero-padding
  pops), 10 ms fade-in.
- **Devices.** Windows: WASAPI via the `wasapi` crate, shared mode at the device mix
  rate, optional exclusive mode. macOS: Core Audio via `cpal` (or objc2 Core Audio),
  device nominal rate. Output device picker lists devices; device loss is handled by
  pausing, not crashing.
- **Resampler.** Used only when device rate != song rate. High-quality windowed sinc
  (`rubato`, long filter). Record each place a resample happens in DECISIONS.md.
- **Memory.** Never decode whole songs. Budget for a playing song with bubbles on:
  < 1 GB for all processes on the Mac (target < 300 MB for the Rust process).
- **Offline render** (`jk-dsp::render`) runs the exact same graph single-threaded for
  export, CI cross-platform comparison and tests. Same code, same results.

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

- Preact + TypeScript + Vite. Plain functional screens first; the jukebox skin is a
  separate layer (`app/ui/src/skin/`) that wraps the same components.
- Baseline browser: Safari 16.4 (WKWebView on macOS 13.6.9) and WebView2 (Chromium) on
  Windows. No WebGPU. Any newer CSS/JS feature needs a check and a fallback.
- Lyrics ticker: Canvas 2D. Each grapheme cluster is measured and drawn individually at
  a precomputed x position (so ligatures and kerning cannot move letters away from the
  timing data). The text strip is pre-rendered once per line set into offscreen tiles
  (sung colour and upcoming colour); each frame blits with a translation and a clip at
  the marker. Frame rate capped (60 fps playing, animation paused when hidden).
- Bubbles and glow: CSS transforms / opacity and a single low-rate canvas, off when the
  window is hidden, off under `prefers-reduced-motion`, switchable in settings.
- No audio in the web view, ever.

## 7. Capability detection

At startup the Rust core works out: OS and version, RAM, whether a Studio runtime is
present next to the app, and (by asking the Studio `probe` command, with a timeout)
whether CUDA works and how much VRAM there is. Missing pieces make Studio features
show as unavailable with a plain explanation; playback never depends on them.

## 8. Portable mode and paths

The app finds its root by walking up from the executable: on Windows the exe is in
`<root>/Windows/`, on macOS the bundle is in `<root>/Mac/`. If `<root>/Library` exists
(or `<root>/portable.txt`), the app is in portable mode and every write goes under
`<root>`: `Library/`, `Recordings/`, `Models/`, `Settings/` (including the WebView2
user-data folder on Windows). Nothing uses an absolute path that survives a restart;
paths are stored relative to `<root>`. On macOS, a translocated (quarantined) app is
detected and the user is asked to pick the library folder instead.

Writes of song files and settings are atomic (temp file + fsync + rename), except the
song file's settings slots, which are designed for in-place A/B updates (see the spec).
Library scans ignore `._*` AppleDouble files and other dotfiles.

## 9. Naming

`branding.json` holds the product name, bundle identifier, file extension and
display strings. Build scripts and code read it; no other file hard-codes "Jukeoke"
except where a tool forces a literal (those places are listed in `branding.json`
under `"also_edit"` so a rename is a checklist, not a search).

## 10. Testing (maps to WORK_ORDER section 9)

- Rust: `cargo test --workspace` (unit + DSP property tests: pitch accuracy in cents,
  duration, impulse alignment, transparency at 0, determinism of offline render).
- Python: `pytest studio/tests` (pipeline pieces with generated audio; GPU tests marked).
- UI: `npm test` (ticker letter-under-marker check across a whole song at several
  window sizes, using a headless canvas measurement shim), plus screenshots.
- Cross-component: `tests/` (Python writer -> Rust reader conformance, end-to-end
  Studio -> app render, portable-mode write audit).
- Every audio claim has a measured number. Results go to `docs/benchmarks/`.

# Test results (local sweep, CI equivalent)

**Run:** 18 November 2026, 02:04 to about 03:35 (local time), on the build PC: the final
evidence pass after the gap-closing work (key cache, cover correction, minimised ticker,
recorder honesty, Mac CI hardening, licence texts). The previous sweep (17-18 November,
23:25-00:35) is superseded; its fixes are kept in section 5.1.
**Code:** commit `1703c7a` plus uncommitted work (about 80 files changed or new, `git
status --short`). Nothing is committed.
**PC:** Windows 11 26200, Ryzen 7 9700X, 32 GB, RTX 5070 Ti 16 GB (driver 621.24), NVMe SSD.
**Tools:** rustc 1.100.0 (x86_64-pc-windows-gnu, WinLibs GCC 16.2), Node 25.6.1 / npm 11.9.0,
Studio dev venv Python 3.12.16 + torch 2.14.1+cu130 + pytest 9.2.0, ShellCheck 0.11.0,
actionlint 1.7.12. `CARGO_TARGET_DIR=$JK_BUILD/target-final`.

GitHub is deferred by the owner, so `.github/workflows/ci.yml` has **never run**. This sweep
runs the same suites on the build PC, plus the GPU tests that CI cannot run, plus a
type-check of the Mac code. Section 6 lists what CI would run on macOS that has not run
anywhere.

To run it again (Git Bash, from the repository root):

```sh
source scripts/devenv.sh
bash scripts/make-dist.sh                      # builds dist/, about 5.5 min
bash scripts/local-ci.sh                       # every suite below, about 20 min
bash scripts/prove-dist.sh <a short CC audio excerpt>   # proves dist/ from a copy, about 15 min
```

Logs are kept outside the repository: `$JK_BUILD/test-sweep/20261118-025253/` (full sweep)
and `.../20261118-031124/` (Rust and Mac re-run), `$JK_BUILD/dist-build/20261118-022427/`,
`$JK_BUILD/dist-proof/20261118-031615/`, and `$JK_BUILD/final/` (this pass's own runs).

## 1. Summary

Every suite passes. One Rust test file failed in the first full run and was fixed (section
5, item 3); the Rust suite was then re-run in full. The remaining finding is the same 13
clippy style warnings, all in `crates/jk-format`. CI does not fail on clippy warnings.

| Suite | Command | Result | Time |
|---|---|---|---|
| UI | `cd app/ui && npm ci && npm run build && npm test` | build OK (tsc, Vite, every JS file parses as ES2022, CSS check against Safari 16.4 and Chrome 120); vitest **15 files, 170 passed, 0 failed** | 14 s |
| Rust | `cargo test --workspace --release --locked` | **194 passed, 0 failed, 0 ignored**, in 34 test binaries (doc tests included); re-run at 03:11 after the fix of section 5 item 3. The first run (02:53) stopped at `tests/keycache.rs` (43 passed, 1 failed, 12 binaries) | 281 s |
| Clippy | `cargo clippy --workspace --locked` | 0 errors, **13 warnings**, all in `jk-format` (`verify.rs` 6, `writer.rs` 2, `flac.rs`, `header.rs`, `lyrics.rs`; the same 13 as before) | 88 s |
| Mac type-check | `cargo check --target aarch64-apple-darwin --workspace --locked` (Zig as C compiler) | all 7 crates; **0 warnings, 0 errors** after the fix of section 5 item 4 (the first run had 1 warning, in `app/src-tauri/src/keymode.rs`). Rubber Band's C++ is skipped by design (no Apple SDK here) | 101 s |
| Studio, CPU | `studio-venv python -m pytest tests -q -rs` (in `studio/`) | **229 passed, 1 skipped, 24 deselected** (the skipped test is the opt-in network test; the deselected ones are the GPU tests) | 108 s |
| Studio, GPU | `studio-venv python -m pytest -m gpu tests -q -rs` | **24 passed** | 196 s |
| Song format conformance | `bash tests/conformance/run.sh` | **PASS**, 92 checks ok | 19 s |
| Portable smoke test | `bash app/scripts/portable-smoke-test.sh dist/Windows/Jukeoke.exe` | **RESULT: OK** (normal path, a `subst` drive letter, a forced start-up failure, nothing named jukeoke in %LOCALAPPDATA%, %APPDATA% or HKCU) | 279 s |
| Lint | ShellCheck on all 29 `*.sh` files; actionlint on the 2 workflows | **0 findings** each | 3 s |
| Offline renders, Windows half of the cross-platform comparison | `bash scripts/ci-render.sh <dir>`, then `scripts/ci-compare-renders.sh` against the Stage 0 Windows renders (`$JK_BUILD/ci-render-win`) | **18 renders, all byte-identical** to the earlier ones (max difference 0), after the Rubber Band patch of section 5 item 1 | 8 s |
| Real-time allocations | `cargo test --release -p jk-engine --test rt_alloc -- --nocapture` (also part of the Rust row) | **0 allocations and 0 frees** on the render worker and the device thread in every phase; positive controls seen; also on the real WASAPI device at digital silence (docs/benchmarks/playback.md section 11) | 27 s |
| 20 minutes in the real app | `node app/ui/scripts/real-app-soak.mjs ... 20 14 30` | **0 underruns, 0 device starved**, worker lead never below 140 ms, ticker 60.000 fps with 0 frames missing, 0 display frames over 25 ms; whole app 0.55 GB resident (playback.md section 12) | 20 min |
| Karaoke mix loudness and stereo image | `studio-venv python -I studio/bench/karaoke_image.py` | MUSDB: every track within 0.3 LU, correlation within 0.013; ccMixter default: median -0.05 LU, worst -2.99 LU on the known hard track (separation.md 6.9) | 46 s |
| First-run model re-hash | `studio-venv python -I studio/bench/model_hash_time.py dist/Models` | hash 1,820 MB/s, uncached read 3,126 MB/s; estimate 21-40 s on a USB 3 stick, about 129 s on USB 2 (section 7) | 6 s |
| dist build | `bash scripts/make-dist.sh` | exit 0; layout, size and checks in section 2 | 5 min 30 s |
| dist proof | `bash scripts/prove-dist.sh cortez-feel-stripped-30s.wav` | **9 of 9 steps passed**, 0 failed (section 3): smoke test 8/8, self-check 8/8 PASSED, runtime check 26/0, a real Studio job in 54.3 s with `verify --strict` PASS, 5/5 Library plays, 0 per-user entries written by the app or the Studio | 905 s |

Notes on the Rust row:

- `real_studio_in_dev_mode` reports success when it skips, so "passed" alone does not prove
  it ran; the Studio stream ran it with output (`studio_supervisor` 15 passed, including
  the real Studio in dev mode).
- The run is `--release`; CI runs the debug profile.
- `tests/rt_alloc.rs` (new) and the key-cache tests (new) are in it.

## 2. The dist/ folder

`bash scripts/make-dist.sh` builds everything from this checkout into `dist/`, which git
ignores. It was run three times in this pass: at 02:04 (before the Rubber Band patch), at
02:22 (stopped on purpose during the app build, to add `player_diagnostics`), and at
02:24-02:28 (the final `dist/`, exit 0). Its log: `$JK_BUILD/final/make-dist-3.log`.

```
dist/START-HERE.txt          what each folder is, copying to an exFAT stick, first launch on
                             Windows and on the Mac, where the self-check report goes
dist/Windows/Jukeoke.exe     release app, stripped, 12,693,006 bytes, 21 imported DLLs, all part of Windows
dist/Windows/resources/      demo-song.jksong (the self-check's song)
dist/Windows/Studio/         relocatable Studio runtime, built from the current studio/
dist/Mac/README.txt          how to get Jukeoke.app (build-mac.sh on the MacBook, or the CI artifact)
dist/Library/demo-song.jksong
dist/Recordings/README.txt
dist/Models/                 default models (preset full + alignment + transcription), manifest.json,
                             SHA256SUMS.txt, README.txt
dist/Settings/README.txt     nothing else: no settings, no WebView2 profile
dist/Licenses/               LICENSE.txt, THIRD_PARTY_LICENSES.md, MODELS.md, the fonts' OFL.txt,
                             python/ (ctranslate2, flatbuffers, tokenizers), and third-party/:
                             the licence and notice files of every linked Rust crate, the bundled
                             npm packages, the Python packages, Rubber Band (6 files, 680 KB)
```

What `make-dist.sh` checked on the final run:

- **Foreign files:** none in `dist/`.
- **The app:** `app/scripts/build-windows.sh` ran the UI build and tests (15 files, 170
  passed), the DLL check (21 imported DLLs, all part of Windows) and the build-machine
  path check (user folder and repository path, both slashes: 0 hits in the exe).
- **The Studio runtime:** `check-studio-runtime.sh --static-only`: 11 passed or
  informational, 0 failed, **not stale** (runtime, `runtime.json` and `studio/` all at
  tree hash `5365d2b05c18`); no build-machine path in any of the 18,769 files (4.28 GB,
  UTF-8 and UTF-16LE); no symlinks or junctions; longest path below the stick root 167
  characters.
- **The models:** 15 files kept (already in place) and re-hashed by the Studio's own code
  (3,819,949,511 bytes in 2.3 s); nothing can be downloaded.
- **Licences:** `THIRD_PARTY_LICENSES.md` and `third_party_licenses/` match this build
  apart from the runtime's build time; `Licenses/third-party/` 6 files, 680 KB.
- **Final checks:** OK (`Settings/` holds only README.txt, no WebView2 profile, no
  build-machine path in any small file, the demo song byte-identical to the fixture).

### Size against the budget (WORK_ORDER 4.8: apps + models under 12 GB, ideally under 9 GB)

`scripts/dist/dist_size.py` estimates the space on exFAT: every file takes whole clusters,
and every folder takes at least one.

| Folder | Bytes | Files | Folders | exFAT, 32 KB clusters | exFAT, 128 KB clusters |
|---|---|---|---|---|---|
| Windows/ | 4,295,792,680 | 18,771 | 2,343 | 4.810 GB | 6.711 GB |
| &nbsp;&nbsp;Windows/Jukeoke.exe | 12,693,006 | 1 | 0 | 0.013 GB | 0.013 GB |
| &nbsp;&nbsp;Windows/resources/ | 2,425,490 | 1 | 1 | 0.002 GB | 0.003 GB |
| &nbsp;&nbsp;Windows/Studio/ | 4,280,674,184 | 18,769 | 2,341 | 4.794 GB | 6.696 GB |
| Models/ | 3,819,962,697 | 18 | 8 | 3.821 GB | 3.823 GB |
| Library/ | 2,425,490 | 1 | 1 | 0.002 GB | 0.003 GB |
| Licenses/ | 812,642 | 19 | 7 | 0.002 GB | 0.004 GB |
| Mac/ (README only) | 2,812 | 1 | 1 | 0.000 GB | 0.000 GB |
| Recordings/, Settings/, START-HERE.txt | 7,140 | 3 | 2 | 0.000 GB | 0.000 GB |
| **Total** | **8,119,003,461** | **18,813** | **2,363** | **8.634 GB** (8.04 GiB) | **10.542 GB** |

- **On a 32 GB stick** (Windows formats it with 32 KB clusters), apps plus models take
  **8.63 GB: under the 9 GB target and the 12 GB limit**. The NVIDIA compute cache that
  the first Studio song writes into `Settings/cache` adds about 0.24 GB.
- **On a larger stick with the default 128 KB clusters**, it is 10.54 GB: under 12 GB, but
  over 9 GB, because of the Studio's 18,769 small files. START-HERE.txt tells the owner to
  choose 32 KB clusters.
- **The Mac app is not included**, because it cannot be built here.
- **The key cache** (`Settings/cache/keys`, prerender mode) is capped at 1 GiB by default
  (D-179); it is empty on a fresh stick.
- **Licences of the bundled weights:** the owner's own stick only; a public release
  leaves `Models/` empty (WORK_ORDER 10).

## 3. Proof from a copy under another drive letter

`bash scripts/prove-dist.sh $JK_BUILD/dist-proof/inputs/cortez-feel-stripped-30s.wav`, run
2026-11-18 03:16-03:31 on the final `dist/`. Logs: `$JK_BUILD/dist-proof/20261118-031615/`
(and `$JK_BUILD/final/prove-dist.log`). The input is a 30.0 s excerpt (18-48 s) of "Feel
(Stripped)" by Cortez (CC BY, JamendoLyrics), decoded to 32-bit float with no other
processing. It is never in the repository.

| # | Step | Result |
|---|---|---|
| 1 | Snapshot of %USERPROFILE%\.cache, %APPDATA%, %LOCALAPPDATA% and HKCU (`scripts/user-dirs-audit.py`) | 1,181,519 entries, 309 s |
| 2 | `dist/` copied to a fresh folder (8,119,003,461 bytes, 113 s). **`dist/` renamed to `dist.hidden` for the whole run**; the copy mapped with `subst Q:` | ok |
| 3 | `Q:\Windows\Jukeoke.exe --smoke-test` | exit 0; **8 of 8 checks ok**; 3 s |
| 4 | `Q:\Windows\Jukeoke.exe --selfcheck --silent` (default output device, digital silence) | exit 0, `result: PASSED`, **8 of 8 items OK**: Realtek 48 kHz shared, clock ratio 1.000, 0 underruns, output peak -inf dBFS; recorder 3.014 s for 3 s, 0 dropped frames; demo song rendered; key change +2 / -5 at +0.000 cents with exact length, and the new line "Rubber Band runs 8.6x real time at 48000 Hz here, so "auto" changes keys in real time (threshold 3x)". 8 s |
| 5 | `scripts/check-studio-runtime.sh Q:/ --sentinel-profile <empty dir>` (static and GPU checks) | **26 passed or informational, 0 failed**, 108 s; 0 files in the sentinel profile |
| 6 | One real Studio job through the app, over WebView2 DevTools (`real-app-e2e.mjs queue`, null output device, master 0), input copied to `Q:\Recordings\` first, lyrics `auto` | Studio available 1.9 s after launch, models present. **Job done in 54.3 s**: decode 0.03, separate 46.1, lyrics (Whisper transcription) 3.1, align 2.7, analyse 1.0, write 1.3. 7 transcribed lines (181 graphemes), saved as `transcribed`, unchecked. `jukeoke-cli verify --strict`: **PASS, 0 errors, 0 warnings** |
| 7 | Every Library song opened and played on the null device (`real-app-e2e.mjs stage`) | **5 of 5 PASS**. New song: 0 wrong letters across 2,167 samples, min lookahead 2.08 s, playing, 0 underruns. Demo song: 0 wrong letters across 2,161 samples, min lookahead 2.12 s, 0 underruns. No page errors |
| 8 | Second snapshot and diff | **0 entries written by the app or the Studio.** Registry: only the D-41 / D-88 entries Windows writes for any program (2 MuiCache values, 1 audio PolicyConfig key with its value). Other changes: 6 by the OS, 13 by the test tooling, 77 by other programs |
| 9 | Logs copied out, `subst Q: /D`, copy deleted, `dist` renamed back | done; **total 905 s, 0 failed** |

Written on the copy by the run: `Settings/cache` 240.6 MB (the NVIDIA compute cache of the
first Studio song), `Settings/webview-windows` 11.1 MB, the self-check report, the smoke
report and screenshot, `settings.json`, logs.

Not covered by this proof:

- **No real exFAT USB stick.** A `subst` drive is the same NTFS volume, so the Studio
  trusted the model hashes recorded at build time and did not re-hash; section 7 measures
  and estimates that cost.
- **The separation time is not a benchmark** (46 s for a 30 s excerpt: a just-started
  Studio, the karaoke model's overlap of 8 on a short input). The 4-minute numbers are in
  docs/benchmarks/end-to-end.md.

## 4. What this sweep does not prove

- **Nothing ran in Safari / WKWebView or on macOS.** The UI build checks syntax and CSS
  support against Safari 16.4 statically; the Mac code is type-checked only.
- **No run on a clean Windows 11 PC, a clean CI runner or a real exFAT USB stick.** This PC
  has the Visual C++ redistributable and the WebView2 runtime; section 7 estimates the
  first-run model re-hash on a stick instead of measuring it.
- **The Studio tests used the existing dev venv,** not a fresh `uv sync --frozen` as CI
  would. **Node versions differ:** CI pins Node 24; this PC has 25.6.1.
- **Re-run in this pass:** the 20-minute playback run, now in the real app with bubbles,
  ticker and CPU load (playback.md 12), the allocation test (playback.md 11), and the
  offline renders. **Not re-run:** the separation SDR, alignment and recorder
  benchmarks; their numbers in docs/benchmarks/ still apply (the DSP output is
  byte-identical, section 1, and the Studio's separation code did not change).
- **Real-app items not driven:** the Studio form's cover row with the native dialog, the
  ticker stopping in a really minimised window, the prerender key mode on a real song,
  and the "Preparing key" plate's layout under the skin's key dial (unit and mock tests
  only).

## 5. Fixes made during the final evidence pass (18 November)

1. **Rubber Band allocated on the render worker** (WORK_ORDER trap 11). Found by the new
   allocation test (`crates/jk-engine/tests/rt_alloc.rs`, docs/benchmarks/playback.md
   section 11): every new stretcher's first `process()` allocated 568 bytes and freed the
   buffer reserved by its constructor, and one key at 44.1 kHz grew the resampler's phase
   table by 1.5 MB, all inside `BQResampler::state_for_ratio`, on every key change, every
   seek at a shifted key and every "shift drums" toggle. The vendored
   `crates/jk-rubberband/vendor/rubberband/src/common/BQResampler.cpp` now reuses the
   reserved buffer (`assign()`) and reserves the phase table for `rational_max + 16`
   entries; the patch is marked in the file (GPL notice of changes) and in
   `vendor/VENDORED.md` (D-202). Output unchanged: the 18 golden renders are
   byte-identical to the earlier Windows renders, and the jk-rubberband, jk-dsp and
   jk-engine tests pass.
2. **The engine's worker lead could not be read from the real app.** New command
   `player_diagnostics()` (docs/APP_API.md sections 3 and 11; D-203), used by the soak.
3. **`crates/jk-engine/tests/keycache.rs` failed when its tests ran in parallel** under
   CPU load ("timed out after 5 s waiting for cached route" in the full sweep; 4 of 5
   parallel runs failed while other builds ran; serial runs passed, which is how the key
   cache stream ran them). Two of its tests play on the null device in "fast" mode (as
   fast as the engine renders), where the worker can outrun a cache feed starved of CPU
   by another test's renderer; real-time device pacing (a 2 s feed ring against 150 ms of
   render-ahead) does not do that. The three tests now take a file-wide lock and run one
   at a time; then 3 of 3 runs passed with the GPU suite running at the same time, and the
   full Rust suite passed. Not a change to the engine.
4. **A macOS-only compiler warning** in `app/src-tauri/src/keymode.rs` (`let mut parts`
   is only mutated on Windows): `#[cfg_attr(not(windows), allow(unused_mut))]`. The
   Windows build is unchanged, so `dist/` was not rebuilt for it.
5. **New tooling:** `app/ui/scripts/real-app-soak.mjs` (soak driver) and
   `app/ui/scripts/soak-memory.ps1` (memory sampler over the process tree);
   `studio/bench/karaoke_image.py` (separation.md 6.9); `studio/bench/model_hash_time.py`
   (section 7).

The three gap streams' code otherwise built and passed every suite on the first run.

### 5.1 Fixes of the previous pass (17-18 November), kept for the record

1. **The shipped exe contained the build PC's user name.** The e2e-stick exe held the user
   folder `C:\Users\<name>` 413 times:
   - 410 times under `.cargo\registry\...` (panic locations of dependencies);
   - once in the source tree's path (`env!("CARGO_MANIFEST_DIR")`, used by two
     developer fallbacks);
   - twice in the repository path with forward slashes (Rubber Band's `__FILE__`).

   Fixes:
   - `app/scripts/build-windows.sh` passes `--remap-path-prefix` for the user folder, the
     repository, the target folder and CARGO_HOME. It uses `cargo --config`, which cargo
     adds to `.cargo/config.toml`'s target rustflags; RUSTFLAGS would replace them.
   - `crates/jk-rubberband/build.rs` adds `-ffile-prefix-map` (via `flag_if_supported`).
   - `app/src-tauri/src/portable.rs` has a new `SOURCE_DIR` constant, which is `None` when
     `JK_DIST_BUILD=1` at compile time. `lib.rs` (demo song fallback) and `studio.rs`
     (dev-mode Studio) use it. Dev and test builds behave as before.
   - The build now fails if the exe contains the user folder or the repository path, in
     any case, with either slash, UTF-8 or UTF-16 (checked with Node, because MSYS
     `grep -i` aborts with exit 134 on this binary).

   Positive control: the check gives 416 hits on the old exe (its patterns overlap) and 0
   on the new one.

   Since then (18 November, Mac CI stream, D-199): `scripts/build-mac.sh` applies the same
   remap and fails if a build path is left in the bundle (`check-macos-paths.sh`). It has
   never run on a Mac.
2. **`scripts/check-studio-runtime.sh` wrote into `<root>/Settings/tmp/runtime-check`** even
   when `--out` pointed elsewhere, which would have put state into `dist/Settings`. It now
   writes its orchestrator into the `--out` folder.
3. **`app/scripts/portable-smoke-test.sh` contained a literal carriage return** inside
   `tr -d '<CR>'`. Because of it, git stored the script as binary (`i/-text`, so no
   line-ending normalisation), the working copy had CRLF, and ShellCheck reported 188
   errors. It is now the two-character escape `tr -d '\r'`, with LF endings.
4. **Lint:**
   - `scripts/devenv.sh`: `# shellcheck shell=bash`, an SPDX header, and no
     `export X="$(...)"`.
   - `app/scripts/check-mac.sh`: assign, then export.
   - `tests/conformance/run.sh`: `if`/`else` instead of `A && B || C`.

   Re-tested: conformance PASS, `check-mac.sh` exit 0, `devenv.sh` sets the same
   variables.
5. **`scripts/ci-render.sh` ignored `CARGO_TARGET_DIR`.** It looked only in
   `target/release` and "skipped" with a warning, so with a target folder set the
   cross-platform comparison would silently not happen. It now uses
   `${CARGO_TARGET_DIR:-target}/release`.

New scripts:

- `scripts/make-dist.sh` builds the dist, with helpers in `scripts/dist/`: the model copier
  and verifier, the size report, the text templates, and the licence texts of the three
  Python packages that ship none.
- `scripts/prove-dist.sh` runs the proof in section 3.
- `scripts/local-ci.sh` runs this sweep.

## 6. What CI would additionally run on macOS, and that has NOT run anywhere

None of the following has run on any Mac or any macOS runner. The Mac code has only been
type-checked from Windows (`cargo check --target aarch64-apple-darwin`, section 1), which
compiles no Objective-C or C++ for real, links nothing and runs nothing.

**What a green run would prove (CI hardening, 2026-11-18).** Every step below fails when
its evidence is missing: no app bundle, no self-check report, an unfinished self-check,
no renders on either platform. Earlier versions skipped with exit 0 in those cases. The
only tolerated gap is the Screen Recording permission, which a runner may be unable to
grant. In that case the self-check's Recorder item (only when it failed on the
permission) and the ScreenCaptureKit probe (only "permission not granted") pass with a
warning that the job summary repeats in bold. A step can be switched off only
explicitly (`JK_SKIP_EXFAT`, `JK_SKIP_SCK`, `JK_SKIP_RENDER`, `JK_SKIP_RENDER_COMPARE`, set
to a reason); `ci.yml` sets none. Evidence steps run even after an earlier step failed,
so one red step does not hide the rest.

The failure paths were tested on Windows with fake inputs:

- `bash scripts/tests/test-ci-scripts.sh`: 74 passed, 0 failed (Git Bash);
- `python3 -I scripts/tests/test_check_macos_availability.py`: 20 tests, OK.

CI runs both first on the macOS runner, which also checks the scripts under bash 3.2
and the BSD tools.

From `.github/workflows/ci.yml`, job `macos` (macos-15 arm64, deployment target 13.0):

1. **The two test files above**, on bash 3.2 and BSD tools.
2. **`scripts/ci-test-crates.sh`**: the Rust tests of every crate on Apple Silicon. This
   covers Rubber Band compiled for ARM64 with Apple clang, jk-dsp accuracy and impulse
   tests on ARM64, jk-engine on Core Audio (cpal 0.16) and on the null device, jk-format,
   and jk-recorder's ScreenCaptureKit code.
3. **`scripts/build-mac.sh`**, the same script the owner runs on the MacBook:
   - UI build;
   - `cargo build --release` for aarch64-apple-darwin, so the first real compile of the
     Objective-C, C++ and Rust Mac code, and the first link;
   - the home folder, checkout, target folder and CARGO_HOME remapped with
     `--remap-path-prefix`, plus `JK_DIST_BUILD=1` and a debug-info strip;
   - assembling `Jukeoke.app`;
   - ad-hoc signing and `codesign --verify --deep --strict`;
   - **`scripts/check-macos-minos.sh`**: every binary and bundled library declares macOS
     13.0 or lower;
   - **`scripts/check-macos-paths.sh`**: the build fails if the runner's home folder or
     the checkout path appears in any file of the bundle or in `sck_probe`.
4. **`scripts/check-macos-availability.sh`** on `dist/Mac` (trap 14, D-37). It parses the
   SDK headers for C functions, constants, Objective-C classes and selectors introduced
   after macOS 13.0, then reads every Mach-O with `nm -m -u`, `otool` selector references
   and C strings. It fails on any strong reference to a newer symbol or class; weak
   references, selectors and names looked up at run time are listed for review. **It has
   never run on a real SDK:** the header parser ran on Zig's bundled copy of Apple's
   `usr/include` (574 headers, 73 declarations newer than 13.0 found), and the framework
   headers have only been seen as test snippets (docs/MAC.md section 11).
5. **`cargo test -p jukeoke` on macOS**: the app core tests, including the macOS portable
   paths and translocation handling.
6. **`scripts/ci-mac-audio-device.sh`**: installs the BlackHole virtual output device only
   if the runner has no audio output device.
7. **`scripts/ci-exfat-launch.sh`**:
   - an exFAT disk image laid out like the stick, with the signature and minimum-macOS
     checks repeated on the stick copy;
   - Screen Recording approved for the stick copy in the runner's TCC database;
   - the real self-check, `--selfcheck --silent`, launched from the stick both directly
     (exit code) and through `open`;
   - the UI smoke test `--smoke-test`;
   - crash reports collected.

   Each self-check report is judged by `scripts/ci-selfcheck-verdict.sh`: all eight items
   OK, report and exit code consistent.
8. **`scripts/ci-sck-probe.sh`**: a first ScreenCaptureKit system-audio capture. It
   passes only on a full capture with a matching sample count and the tone coming back,
   or warns on "permission not granted".
9. **`scripts/ci-render.sh` on macOS and the `compare-renders` job**: macOS renders against
   Windows renders, within float tolerance (WORK_ORDER 9, "Same audio on both systems").
   The job fails if either platform's renders are missing. Only the Windows half has run
   (section 1).
10. **The `release.yml` macOS job**, a manual release build with the same exFAT
    self-check. It is never triggered. The release archives of both platforms now carry
    `third_party_licenses/`.

Beyond CI, only the owner's MacBook (macOS 13.6.9, 8 GB) can show these, and none has
been tested:

- the app launching on Ventura;
- WKWebView rendering of the skin and ticker;
- the Screen Recording prompt and a real capture;
- memory against the 1 GB budget;
- 20 minutes of sustained playback with key change and bubbles;
- the self-check report from the real machine.

The CI runner is macOS 15, so even a green CI run would not prove macOS 13.

The Windows CI job (`windows-latest`) has also never run. Its suites are the ones in
section 1, so the only gap there is the clean runner.

## 7. First-run model re-hash on a new drive (measured here, estimated for USB sticks)

The Studio trusts a model file only after hashing it on the drive it is on (D-172: the
record in `Models/manifest.json` holds size, mtime and the volume serial). So the first
Studio job after the stick is plugged into a PC, or after the folder is copied to another
drive, hashes the model files it needs once. Section 3's proof could not show that cost,
because a `subst` drive has the same volume serial.

```
studio-venv python -I studio/bench/model_hash_time.py dist/Models --json $JK_BUILD/final/model_hash_time.json
```

Measured on this PC's NVMe drive, the 15 files of `dist/Models` (3,819,949,511 bytes), with
the Studio's own hash function (`modelzoo.download.sha256_of`, SHA-256 over 4 MiB reads):

| Measure | Result |
|---|---|
| Hash speed, files in the OS cache (CPU bound: the limit of any drive) | 2.10 s = **1,820 MB/s** |
| Sequential read, OS cache bypassed (FILE_FLAG_NO_BUFFERING) | 1.22 s = **3,126 MB/s** |
| Uncached read with every block hashed as it arrives (a first run on this drive) | **2.62 s** |

Estimate for a stick, with the Studio's serial read-then-hash loop
(time = bytes / read speed + bytes / 1,820 MB/s); USB 3 sticks typically read 100-200
MB/s, USB 2 about 30 MB/s:

| Files hashed | Bytes | USB 2, 30 MB/s | USB 3, 100 MB/s | 150 MB/s | 200 MB/s |
|---|---|---|---|---|---|
| Separation, preset full (3 files) | 936 MB | 32 s | 10 s | 7 s | 5 s |
| Alignment (wav2vec2) | 1,262 MB | 43 s | 13 s | 9 s | 7 s |
| Transcription (Whisper turbo; only when the lyrics are transcribed) | 1,622 MB | 55 s | 17 s | 12 s | 9 s |
| **All 15 files** (a first song without lyrics) | **3,820 MB** | **129 s** | **40 s** | **28 s** | **21 s** |

So the first song on a new stick waits roughly 20-40 s longer on a USB 3 stick (up to
about 2 minutes on USB 2), once; later songs on the same drive skip it. The model load
after the check reads the same files again; whether the OS cache serves that read on a
stick was not measured.
**Not measured on a real stick** (none available); the stick's own read speed is the
assumption.

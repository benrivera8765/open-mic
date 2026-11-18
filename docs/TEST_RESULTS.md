# Test results (local sweep, CI equivalent)

**Latest re-run:** 18 November 2026, 05:18-05:51, after the preset-download pass (section
8): `CARGO_TARGET_DIR=$JK_BUILD/target-presets bash scripts/local-ci.sh`, exit 0, log
`$JK_BUILD/test-sweep/20261118-051847/`. Its counts are in the table of section 1 (the
earlier figures are kept in brackets); `dist/` was rebuilt and proved again (sections 2
and 3). Everything below that is not marked as re-run is from the run described next.
An independent verification followed (06:00-06:35, section 8.1): two corrections (D-215),
vitest 16 files, 179 passed; `dist/` rebuilt and proved again with them.

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
| UI | `cd app/ui && npm ci && npm run build && npm test` | build OK (tsc, Vite, every JS file parses as ES2022, CSS check against Safari 16.4 and Chrome 120); vitest **16 files, 178 passed, 0 failed** (re-run; was 15 files, 170) | 16 s |
| Rust | `cargo test --workspace --release --locked` | **196 passed, 0 failed, 0 ignored**, in 34 test binaries (re-run; was 194: the two new `studio_supervisor` preset tests). Earlier: re-run at 03:11 after the fix of section 5 item 3; the first run (02:53) stopped at `tests/keycache.rs` (43 passed, 1 failed, 12 binaries) | 329 s |
| Clippy | `cargo clippy --workspace --locked` | 0 errors, **13 warnings**, all in `jk-format` (`verify.rs` 6, `writer.rs` 2, `flac.rs`, `header.rs`, `lyrics.rs`; the same 13 as before). The re-run found a 14th, a collapsible `if` in this pass's `app/src-tauri/src/studio.rs`; after the fix `local-ci.sh --only clippy` (log `$JK_BUILD/test-sweep/20261118-053751/`): **13 warnings, all in `jk-format`, 0 errors**, and `studio_supervisor` 17 passed again | 88 s |
| Mac type-check | `cargo check --target aarch64-apple-darwin --workspace --locked` (Zig as C compiler) | all 7 crates; **0 warnings, 0 errors** (re-run: the same) after the fix of section 5 item 4 (the first run had 1 warning, in `app/src-tauri/src/keymode.rs`). Rubber Band's C++ is skipped by design (no Apple SDK here) | 101 s |
| Studio, CPU | `studio-venv python -m pytest tests -q -rs` (in `studio/`) | **230 passed, 1 skipped, 24 deselected** (re-run; was 229: the new preset download test) (the skipped test is the opt-in network test; the deselected ones are the GPU tests) | 108 s |
| Studio, GPU | `studio-venv python -m pytest -m gpu tests -q -rs` | **24 passed** (re-run: the same) | 193 s |
| Song format conformance | `bash tests/conformance/run.sh` | **PASS**, 92 checks ok (re-run: the same) | 22 s |
| Portable smoke test | `bash app/scripts/portable-smoke-test.sh dist/Windows/Jukeoke.exe` | **RESULT: OK** (re-run: OK, 275 s, on the exe built before the clippy fix; the final exe passed the dist proof's smoke test, section 3) (normal path, a `subst` drive letter, a forced start-up failure, nothing named jukeoke in %LOCALAPPDATA%, %APPDATA% or HKCU) | 279 s |
| Lint | ShellCheck on all 29 `*.sh` files; actionlint on the 2 workflows | **0 findings** each (re-run: the same) | 3 s |
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

**Re-built 18 November 05:39-05:45** after the preset-download pass (section 8), with the
final code: `CARGO_TARGET_DIR=$JK_BUILD/target-presets bash scripts/make-dist.sh`, exit 0,
log `$JK_BUILD/dist-build/20261118-053958/`. UI tests 178 passed in the build, 21 imported
DLLs all part of Windows, runtime check not stale (tree hash `fb9d6831883f`), the same 15
model files re-hashed, final checks OK; exe 12,694,030 bytes; total 8,119,009,393 bytes,
8.634 GB on exFAT with 32 KB clusters, 10.542 GB with 128 KB. `Models/README.txt` now says
what the app does: the Clean and Permissive models are not included and are downloaded
through the Studio form when the user presses its button (template in
`scripts/dist/dist_models.py`). **Re-built again 06:14-06:19** with the corrections of
section 8.1 (log `$JK_BUILD/dist-build/20261118-061406/`, full output
`$JK_BUILD/verify-final/make-dist.log`): exit 0, UI tests 179 passed in the build, 21 DLLs
all part of Windows, runtime not stale (tree hash `228161fd1314`, changed by `presets.py`),
15 model files re-hashed, final checks OK; total 8,119,009,490 bytes, 8.634 GB on exFAT
with 32 KB clusters, 10.542 GB with 128 KB. The figures below are from the earlier build.

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

**Re-run 18 November 05:44-05:59** on the re-built `dist/`:
`bash scripts/prove-dist.sh $JK_BUILD/dist-proof/inputs/cortez-feel-stripped-30s.wav`, log
`$JK_BUILD/dist-proof/20261118-054430/`: **every step passed, 0 failed, 859 s**: smoke
test 8 of 8, self-check 8 of 8 PASSED, runtime check 26 passed or informational / 0 failed,
a real Studio job through the app in 54.2 s with `verify --strict` PASS (0 errors, 0
warnings), 5 of 5 Library plays (min lookahead 2.08 s and 2.12 s, 0 underruns), 0 entries
written by the app or the Studio in the per-user folders. **Re-run again 06:20-06:34** on the
dist re-built with the section 8.1 corrections (log `$JK_BUILD/dist-proof/20261118-062022/`):
the same steps, **0 failed, 841 s** (copy 8,119,009,490 bytes; smoke 8/8, self-check 8/8
PASSED, runtime check 26/0, Studio job 54.3 s with `verify --strict` PASS, 5/5 plays, 0
per-user entries by the app or the Studio). The table below is the earlier run.

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

## 8. Preset downloads and never-clicked features in the real app (18 November, 05:00-06:00)

Code changed in this pass: the Studio form asks for and downloads the models of the preset
chosen in it (D-209), new preset hints (D-210), key-preparation progress that never steps
back (D-211). Tests added: `app/ui/src/components/studio-models.test.tsx` (7 tests),
`app/ui/src/key-prepare.test.tsx` (+1 test, +4 assertions in the note test),
`app/src-tauri/tests/studio_supervisor.rs` (`models_follow_the_chosen_preset`,
`a_permissive_download_does_not_mark_the_default_models_present`; the fake Studio now
answers per preset), `studio/tests/test_server.py::test_models_status_and_download_follow_the_preset`
(a real Studio process, file:// stand-ins for the weights, `.part` + SHA-256 + manifest).
Before the sweep: `npx vitest run` 16 files, 178 passed; `cargo test --release -p jukeoke
--test studio_supervisor` 17 passed (was 15); `pytest tests/test_server.py -k
"follow_the_preset or models_missing or shutdown_during"` 3 passed.

**Method.** `bash scripts/make-dist.sh` (exit 0, `$JK_BUILD/presets/make-dist.log`), then a
copy of `dist/` (8,119,009,393 bytes; `Models/` holds the default set only) mapped with
`subst Q:`, and `node app/ui/scripts/real-app-features.mjs Q:/Windows/Jukeoke.exe Q:/ <out>
Q:\inputs\cortez-feel-stripped-30s.wav Q:\inputs\cover-test.jpg`: the release exe, driven
over WebView2 DevTools, with `Settings/settings.json` set to the null output device
("No audio output (null device)", 48 kHz) and master 0 before the start. Input: the 30 s CC
BY excerpt of section 3; cover: a 600x600 JPEG drawn for this test (20,121 bytes). Logs and
screenshots: `$JK_BUILD/presets/features-20261118-feat2/` (run 2, the one reported below),
`.../features-20261118-feat1/` (run 1, stopped at the cover step: see "Cover" below). Each
song was then checked with `jukeoke-cli verify --strict` and read back by an independent
reader (`jukeoke_studio.jksong`, studio venv); the downloaded files were hashed again.

| Feature | Result |
|---|---|
| Models before | `studio_models_status`: full all present; clean missing `bs_inst_resurrection_unwa` (204,483,033 B, "none published"); permissive missing `mel_roformer_kim` (913,106,900 B, MIT) |
| **Clean preset, download through the form** | Choosing "Clean" in the form showed, under the preset choice: "One model for the Clean preset is not on this drive yet: 204 MB to download, once. Nothing is downloaded until you press the button. BS-RoFormer Resurrection Inst (unwa), 204 MB (needed for every song): licence: none published; commercial use unclear" and the button "Download 204 MB" (screenshot `studio-clean-notice.png`). Pressing it downloaded from the pinned URL `https://huggingface.co/pcunwa/BS-Roformer-Resurrection/resolve/8fc1a62f.../BS-Roformer-Resurrection-Inst.ckpt` (real network): run 1 106.8 s with 50 progress events, run 2 3.5 s with 7; then the notice went away and the toast said "Models for the Clean preset downloaded and checked." Re-hashed afterwards: SHA-256 `16311025...a7bac7a15` and size equal to the registry; manifest entry recorded with the URL and "licence: none published"; no `.part` file left |
| **Clean song** | Added to the queue with preset clean, lyrics automatic: done; `Cortez - Feel (Stripped) excerpt, Clean.jksong`, manifest `preset: clean`, stems sum to source, 7 transcribed lines; `jukeoke-cli verify --strict`: **PASS, 0 errors, 0 warnings** |
| **Permissive preset, download and song** | Notice: "One model for the Permissive licences only preset ... 913 MB ... Mel-Band RoFormer vocals (Kimberley Jensen), 913 MB (needed for every song): licence: MIT; commercial use allowed"; download 13.6 s, 25 progress events, from `https://huggingface.co/KimberleyJSN/melbandroformer/resolve/ac9b0614.../MelBandRoformer.ckpt`, SHA-256 `87201f4d...7c559e` equal to the registry. Song done, `preset: permissive`, `verify --strict` **PASS, 0 errors, 0 warnings**. Afterwards `models_present` (default preset) was still true |
| **Recorder** (4a) | The demo song played on the null device at master 0 while the Recorder captured the real default device (Realtek, 48 kHz shared) for 10.5 s between the two button clicks: `Recording 2026-11-18 05-15-47.wav`, IEEE float (format tag 3), 2 channels, 48,000 Hz, 32-bit, 482,384 frames = 10.050 s; peak -120.5 dBFS (silence plus the driver's processing floor, see recorder.md); the Recordings list showed the same length, rate and channels. "Make a karaoke song from this" switched to the Studio with that file in the form; with "No lyrics" it was queued and finished (`Recording 2026-11-18 05-15-47.jksong`, `verify --strict` PASS). Run 1 gave the same (10.057 s) |
| **Drag-and-drop** (4d) | A real OS drag-and-drop: `app/ui/scripts/ole-drop.ps1` starts a Windows OLE drag (DoDragDrop, CF_HDROP file list, as Explorer does) under a synthesized button press and moves the real pointer onto the app's window. The app received `tauri://drag-enter` and `tauri://drag-drop` with the path, the core's `WindowEvent::DragDrop` handler emitted `studio://dropped {"paths":["Q:\inputs\cortez-feel-stripped-30s.wav"]}` and the file appeared in the Studio form. 3 attempts: run 1 and a separate `--drop-only` run succeeded; in run 2 the helper found another window over the app at the drop point and cancelled the drag with Escape (its safety rule; the run then emitted `studio://dropped` from the page to continue). The helper now raises the app's window first (topmost, then normal), which the `--drop-only` run used. Not covered: dragging from Explorer itself (the drag source was the helper's window, with the same data format), several files at once, and dropping onto an inactive or minimised window |
| **Cover** (4c) | The native file dialog cannot be automated. Its answer was supplied at the IPC boundary: `__TAURI_INTERNALS__.invoke` cannot be replaced (not writable, not configurable), so the page's `fetch` answered only `http://ipc.localhost/pick_cover_image` with the JPEG's path, as the core does (run 1 stopped here when it tried to replace `invoke`). Everything else was the real path: the click on "Choose a cover picture…", the UI's `pick()`, two `studio_cover_preview` calls (the form showed "Your picture: cover-test.jpg" and a JPEG thumbnail), `studio_submit` with `metadata.cover_path`, and the Studio writing it: the Clean song's `cover` chunk is `image/jpeg`, 20,121 bytes, SHA-256 equal to the file's (`75ce7452...a97c7`) |
| **Sync editor "Re-align with Studio"** (4b) | On the Clean song (transcribed lyrics): line 1 "Please don't stand over here" changed to "Please jukebox stand over here" in the editor, "Re-align with Studio" (1.0 s, toast "Re-aligned. Check it, then save."), "Save into the song". Read back from the file: the active user-state slot holds `lyrics_override` (7 lines, source "transcribed", unchecked) whose line 1 is "Please jukebox stand over here" with "jukebox" timed 8.42-9.32 s (the replaced "don't" was 8.68-9.32 s); `jukeoke-cli info` shows the override and `verify --strict` still passes. Observation: the word before it, "Please", moved from 7.64-8.68 s to 1.58-8.42 s (alignment confidence 0.16): "jukebox" is not what is sung there, so the aligner stretched its neighbour; a real correction was not tried |
| Page errors | none in either run |

Not covered here: the native cover dialog itself, a drop from Explorer, the ticker in a
minimised window, and anything audible (nothing was played aloud).

The drive used the `dist/` built at 05:04-05:10. After it, clippy flagged a collapsible
`if` in this pass's `models_status` (`app/src-tauri/src/studio.rs`); the fix has the same
behaviour (`studio_supervisor` 17 passed again, clippy back to the 13 `jk-format`
warnings), and `dist/` was rebuilt and proved again with it (sections 2 and 3). The full
sweep of section 1 ran between the two builds.

## 8.1 Independent verification of section 8 (18 November, 06:00-06:35)

Re-run first: `npx vitest run` 16 files, 178 passed; `cargo test --release --locked -p
jukeoke --test studio_supervisor` (`target-presets`) 17 passed, including both preset tests;
`pytest tests/test_server.py -k "follow_the_preset or models_missing or shutdown_during"` 3
passed. Counts in sections 1-3 and 8 were checked against their logs (`test-sweep/20261118-051847`
and `-053751`, `presets/make-dist2.log`, `dist-proof/20261118-054430`): they match.

**Release app, the dist of 05:39-05:45**, copied fresh (8,119,009,393 bytes), `subst Q:`,
null device at master 0, driven over WebView2 DevTools (verifier scripts, not in the repo;
output `$JK_BUILD/verify-final/`):

| Check | Result |
|---|---|
| Clean with its weights absent | `studio_models_status`: full all present; clean missing only `bs_inst_resurrection_unwa` (204,483,033 B, "none published", unclear); `Q:/Models/bs_roformer_resurrection_inst.ckpt` absent. In the form: Full shows no notice; Permissive "... 913 MB ... licence: MIT; commercial use allowed"; Clean "One model for the Clean preset is not on this drive yet: 204 MB to download, once. Nothing is downloaded until you press the button. BS-RoFormer Resurrection Inst (unwa), 204 MB (needed for every song): licence: none published; commercial use unclear", button "Download 204 MB". Before the click: no `studio_models_download` request, 0 `studio://models` events, no file, no `.part`. The form asked `studio_models_status` with `{"preset":"clean"}` / `"permissive"` / `"full"` |
| Download path | The already-downloaded file (`$JK_BUILD/models-cache`, same size and SHA-256 as the registry) was copied in after the notice appeared, so nothing was fetched a second time. The button then sent `studio_models_download {"preset":"clean"}`; the notice went away 0.08 s later; `studio_models_status` clean: all present; `models_present` (default) still true. The toast said "downloaded and checked" although nothing was downloaded or hashed at that moment (see below) |
| Clean song | Queued from the form (lyrics automatic): done in 62.8 s; manifest `preset: clean`, stems sum to source; the Studio hashed the copied weights at first use (`Models/manifest.json`: SHA-256 `16311025...a7bac7a15`, URL pinned to revision `8fc1a62f...`, "none published"); `jukeoke-cli verify --strict`: PASS, 0 errors, 0 warnings |
| Re-align, a genuine correction | The transcription's last line was "Cause I'm not her eye"; the JamendoLyrics annotation says "cause i'm not who i (should be)". Corrected to "Cause I'm not who I" in the Sync editor, "Re-align with Studio" (1.3 s), "Save into the song". Read back from the file: `lyrics_override` line 6 = Cause 28.26, I'm 28.56, not 28.74, who 29.02, I 29.48 s against annotated onsets (excerpt time) 28.29, 28.46, 28.73, 29.07, 29.34 s: errors -0.03, +0.10, +0.01, -0.05, +0.14 s (confidence 0.09-0.74); lines 0-5 kept their times; `verify --strict` PASS |
| Key preparation progress (prerender, null device) | Sampled every 15 ms, raw `key_preparing` vs the note: the core's figure stepped down 20 times (always 1 point, between cache cells), the note 0 times. But after a seek while a key was being prepared, the core restarted from 0 % and the note kept the old figure until passed: 41 % held for 0.5 s after a seek at 41 %, 61 % for 0.5 s after a seek at 61 % (longer on slower computers). Fixed (D-215, below) |
| Preset hints against separation.md | Clean and Permissive match. Full said "the band is left exactly as recorded" and "leaves the least of the singer of the three": separation.md 5.4 measures Full 0.61 / 1.05 dB low at 4-8 / 8-16 kHz (Clean 0.28 / 0.49), and Permissive was never measured against Full on lead residue. Fixed (D-215) |

**Corrections made (D-215):** the Full hint in `Studio.tsx`, `PRESET_INFO` and README now
says the band is not rebuilt, a little cymbal brightness can go with the voice, and Full
leaves less of the singer than Clean; `monotonicPrepare` shows a drop of more than 2 points
as it is within 1 s after a seek the UI asked for (`state/app.ts` `msSinceSeek`), and still
holds the 1-point dips. Tests: `studio-models.test.tsx` (Full wording), `key-prepare.test.tsx`
(+1 test). `npx vitest run`: **16 files, 179 passed**; `pytest tests/test_registry.py
tests/test_server.py -k "preset or registry"` 28 passed. `dist/` rebuilt (section 2) and
proved again (section 3).

**The rebuilt dist in the release app** (fresh copy, `subst Q:`, null device, master 0): the
three hints read as above; Clean with its weights absent shows the same notice. With
prerender on, a key change and a seek through the UI's own shortcut while the key was half
prepared (ArrowRight at 42 %, ArrowLeft at 61 %): the note went 42 % -> 0 % and 61 % -> 0 %
with the core, and still held its 1- and 2-point dips (25/24, 52/51, 20/19, 21/19).

Not covered by this check: a real second download of the Clean weights (the file was copied
in; the real download is section 8's), a seek the core makes by itself, and anything audible.
The toast after a download that found every file already present still says "downloaded and
checked" (the file is hashed at first use instead); left as it is.

## 9. Recording with the music player in the real app (18 November, 15:16-15:35)

The owner's request (DECISIONS D-221..D-228). Release exe built with
`app/scripts/build-windows.sh` into a portable folder under `$JK_BUILD/recsessions-app/`
(with a copy of the Studio runtime and models), driven over WebView2's DevTools by
`app/ui/scripts/real-app-record-sessions.mjs`. The media player is a test player the
script makes itself: a page in its own Microsoft Edge window (throwaway profile) playing
generated tones at -60.2 dBFS peak (`$JK_BUILD/recsessions-live/gen-player.mjs`: five
12 s tracks and a 6 s "Advertisement"; gaps of 1.5, 1.0 and 0.8 s of digital silence;
track 3 to 4 gapless; track 4, reported as 24 s, skipped by the page after 12 s), announced
through `navigator.mediaSession`, so Windows lists it like any music app. The owner's
Spotify and Firefox sessions were listed (read only) and never played or paused; the app
played nothing itself (null device, master 0). Three runs (run 3, final, final2 on the
exe that replaced dist/Windows/Jukeoke.exe), 20 of 20 checks passed in the last two:

| Check | Result |
|---|---|
| Record & play | capture first, then play: first sound 150.3 / 140.9 / 145.3 ms into the recording, digital silence before it; "Stop & pause" paused the player 4.05-4.10 s in |
| Songs separately | stopped by itself 6 s after the player paused; 5 songs named after the player's titles, covers saved; the advertisement and the tail after playback in `_excluded/` |
| Cuts at gaps | 0.0, 0.0 and -0.1 ms from the middle of the 1.5 / 1.0 / 0.8 s gaps, every run |
| Cuts without a gap | gapless join 37.5-38 ms before the true change, skip 96.5-111.5 ms before it (Goertzel on the master); both marked "uncertain" in the song list |
| Playback ended | 999.9 ms of the silence kept with the last song |
| Flags | the skipped track partial ("shorter than the track"), the others complete |
| Bit for bit | the 7 part files joined = the master (hard-linked before the app deleted it), 3,631,782 / 3,636,642 / 3,637,936 frames; every part verified by the app |
| Player timing | track events noticed 3-20 ms before the sound reached the capture (skip 83-91 ms); the player's timeline put track starts a steady 38 ms early |
| Studio | "Make karaoke songs from all complete songs (4)" queued 4 jobs; the first finished in 56.6 / 33.9 / 47.5 s as "Jukeoke Test Player - Test Track 1" with the player's cover; the other 3 cancelled |

Also on the final exe: portable smoke test RESULT OK; `cargo test -p jk-recorder -p jukeoke`
all passed (jk-recorder 57 + examples 3; app 43 unit + 12 + 2 + 9 + 1 + 17 + 3 + 1
integration); `npm run build` and `npm test` 440 passed. Screenshots (plain look and
jukebox skin) in `$JK_BUILD/recsessions-live/final2/`.

## 9.1 Independent verification of section 9 (18 November, 15:40-16:10)

`cargo test -p jk-recorder -p jukeoke` re-run: all passed before any change. The verifier's
own test player (`$JK_BUILD/verify-recsessions-live/gen-verify-player.mjs`, generated tones
at -60.2 dBFS, its own Edge window and throwaway profile) covers what section 9 did not: a
track skipped 1.8 s after a gap, a track with no artist (reported as 200 s), a title with
`: " ? < / > * |`, a 3 s pause and resume inside a track, a cover that fails to load (404),
and the player closed in the middle of a song. Driven by
`$JK_BUILD/verify-recsessions-live/verify-live.mjs` in a portable folder; the owner's
Spotify and Firefox sessions were listed (read only) and never played or paused.

| Check | Exe that was in dist/ | Fixed exe (now in dist/) |
|---|---|---|
| Record & play, first sound | 134.4 ms into the recording | 143.7 ms |
| Parts joined = master, and each part = its master slice (own byte comparison) | yes | yes (album 3,003,418 frames; closed-player session 512,960) |
| Gap cuts | 0 / 0 / -0.1 / -0.1 ms from the gap middles | the same |
| The skip 1.8 s after a gap | boundary pulled into the earlier gap: the "skipped" part was 0.15 s of silence, the next song began with 2.0 s of the skipped song, start marked "certain" | skipped part holds the skipped song (2.07 s); the next song starts 95 ms early, marked "uncertain" |
| No artist, odd title | `03 No Artist Long.wav` (partial); `02 ... Odd - 'Name' (1-2) -.wav`, sidecar keeps the title | the same |
| Pause and resume inside a song | | paused 2.99 s reported, song complete |
| Cover that failed to load | | no cover for that song (not another song's); the others saved |
| Player closed mid-song | | stopped by itself ("player_closed"), markers stopped + gone, the song partial, the silence after it excluded |
| `studio_inspect` of a recorded song | (title = the file name: not tested live, see D-229) | "Verify One", `Odd: "Name"? <1/2> *|`, artist and cover from the sidecar |

Both fixes are in DECISIONS D-229. After them: `cargo test -p jk-recorder -p jukeoke` all
passed (jk-recorder 61), clippy clean on the changed crates, `app/scripts/build-windows.sh`
(UI tests 440 passed, no build-machine paths, Windows DLLs only), portable smoke test RESULT
OK. The fixed exe replaced dist/Windows/Jukeoke.exe while the app was not running.

A verifier's slip, undone: the first run with the old exe was started from a folder without
`Library/`, so the app was not in portable mode and created `%LOCALAPPDATA%\org.jukeoke.app`
(15:56:59; it did not exist before). That folder, holding only that run's test files, was
moved to `$JK_BUILD/verify-recsessions-live/old-run-appdata/`; `%LOCALAPPDATA%` and
`%APPDATA%` hold no Jukeoke folder now, and the runs in the table used portable folders.

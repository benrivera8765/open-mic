# Test results (local sweep, CI equivalent)

**Run:** 17 November 2026, 23:25, to 18 November 2026, 00:35 (local time), on the build PC.
**Code:** commit `24ff195` plus uncommitted work: the final fixes, the licences and docs,
and this packaging pass. 45 files changed or new. Nothing is committed.
**PC:** Windows 11 26200, Ryzen 7 9700X, 32 GB, RTX 5070 Ti 16 GB (driver 621.24), NVMe SSD.
**Tools:** rustc 1.100.0 (x86_64-pc-windows-gnu, WinLibs GCC 16.2), Node 25.6.1 / npm 11.9.0,
Studio dev venv Python 3.12.16 + torch 2.14.1+cu130 + pytest 9.2.0, ShellCheck 0.11.0,
actionlint 1.7.12.

GitHub is deferred by the owner, so `.github/workflows/ci.yml` has **never run**. This sweep
runs the same suites on the build PC, plus the GPU tests that CI cannot run, plus a
type-check of the Mac code. Section 6 lists what CI would run on macOS that has not run
anywhere.

To run it again (Git Bash, from the repository root):

```sh
source scripts/devenv.sh
bash scripts/local-ci.sh                       # every suite below, about 20 min
bash scripts/make-dist.sh                      # builds dist/, about 4.5 min
bash scripts/prove-dist.sh <a short CC audio excerpt>   # proves dist/ from a copy, about 15 min
```

Logs are kept outside the repository, in `$JK_BUILD/test-sweep/`, `$JK_BUILD/dist-build/` and
`$JK_BUILD/dist-proof/`.

## 1. Summary

Every suite passes. The one finding is 13 clippy style warnings, all in `crates/jk-format`.
CI does not fail on clippy warnings.

| Suite | Command | Result | Time |
|---|---|---|---|
| UI | `cd app/ui && npm ci && npm run build && npm test` | build OK (tsc, Vite, every JS file parses as ES2022, CSS check against Safari 16.4 and Chrome 120); vitest **11 files, 159 passed, 0 failed** | 15 s |
| Rust | `cargo test --workspace --release --locked` | **176 passed, 0 failed, 0 ignored**, in 31 test binaries (doc tests included) | 245 s, build included |
| Clippy | `cargo clippy --workspace --locked` | 0 errors, **13 warnings**, all in `jk-format`: 8 `collapsible_if`, 4 `manual_is_multiple_of`, 1 `chunks_exact` with a constant size | 88 s |
| Mac type-check | `cargo check --target aarch64-apple-darwin --workspace --locked` (Zig as C compiler, as in `app/scripts/check-mac.sh`) | all 7 crates (jk-format, jk-rubberband, jk-dsp, jk-engine, jk-recorder, jk-cli, jukeoke); **0 warnings, 0 errors**. Rubber Band's C++ is skipped by design (no Apple SDK here) | 114 s |
| Studio, CPU | `studio-venv python -m pytest tests -q -rs` (in `studio/`) | **223 passed, 1 skipped, 24 deselected**. The skipped test is the opt-in network test (`JK_NETWORK_TESTS`); the deselected ones are the GPU tests | 94 s |
| Studio, GPU | `studio-venv python -m pytest -m gpu tests -q -rs` | **24 passed** | 199 s |
| Song format conformance | `bash tests/conformance/run.sh` | **PASS**, 92 checks ok (Python writer against Rust reader and writer, damaged files, committed fixture) | 23 s (8 s on the re-run) |
| Portable smoke test | `bash app/scripts/portable-smoke-test.sh dist/Windows/Jukeoke.exe` | **RESULT: OK**: normal path, a `subst` drive letter (J:), a forced start-up failure (exit 1 with `startup-error.txt`), and nothing named jukeoke in %LOCALAPPDATA%, %APPDATA% or HKCU | 279 s (255 s on the re-run) |
| Offline renders, Windows half of the cross-platform comparison | `bash scripts/ci-render.sh <dir>`, then `scripts/ci-compare-renders.sh` against the Stage 0 Windows renders of 17 Nov 07:26 | **18 renders, all bit-identical** to the earlier ones (max difference 0) | 8 s |
| Lint | ShellCheck on all 24 `*.sh` files; actionlint on the 2 workflows | **0 findings** each, after the fixes in section 5 (ShellCheck found 195 before) | 2 s |
| dist build | `bash scripts/make-dist.sh` | exit 0; layout, size and checks in section 2 | 4 min 32 s |
| dist proof | `bash scripts/prove-dist.sh cortez-feel-stripped-30s.wav` | **10 of 10 steps passed** (section 3) | 891 s |

Notes on the Rust row:

- `real_studio_in_dev_mode` reports success when it skips, so "passed" alone does not prove
  it ran. Run again with `--nocapture`, it printed `real Studio start + probe 10.9 s`,
  `available: true`, RTX 5070 Ti, 16275 MB.
- The run is `--release`, as this task asked; CI runs the debug profile. The Rust changes
  of section 5 (fix 1) were already in place for this run.

The final re-runs after the script fixes (conformance, portable, lint) are in
`$JK_BUILD/test-sweep/20261118-001537/`. The first full run is in `.../20261117-235615/`.

## 2. The dist/ folder

`bash scripts/make-dist.sh` builds everything from this checkout into `dist/`, which git
ignores (`git status --ignored` shows `!! dist/`).

```
dist/START-HERE.txt          what each folder is, copying to an exFAT stick, first launch on
                             Windows and on the Mac, where the self-check report goes
dist/Windows/Jukeoke.exe     release app, stripped, 12,422,158 bytes, 21 imported DLLs, all part of Windows
dist/Windows/resources/      demo-song.jksong (the self-check's song)
dist/Windows/Studio/         relocatable Studio runtime, built from the current studio/
dist/Mac/README.txt          how to get Jukeoke.app (build-mac.sh on the MacBook, or the CI artifact)
dist/Library/demo-song.jksong
dist/Recordings/README.txt
dist/Models/                 default models (preset full + alignment + transcription), manifest.json,
                             SHA256SUMS.txt, README.txt
dist/Settings/README.txt     nothing else: no settings, no WebView2 profile
dist/Licenses/               LICENSE.txt, THIRD_PARTY_LICENSES.md, MODELS.md, the three fonts' OFL.txt,
                             licence texts of ctranslate2, flatbuffers and tokenizers
```

What `make-dist.sh` checks on every run:

- **Foreign files.** It first looks for files it did not put there (songs, recordings,
  settings, a WebView2 profile from running the app inside `dist/`). If it finds any, it
  stops and lists them, unless given `--force-clean`. Tested: `Settings/settings.json` and
  `Settings/webview-windows` gave exit 1 and both were listed.
- **The app.** `app/scripts/build-windows.sh` runs the UI tests (159 passed), checks the
  DLL imports, and checks that the exe contains no path from the build PC (0 found).
- **The Studio runtime.** `check-studio-runtime.sh --static-only` gives 11 passed, 0
  failed, and **not stale**: the runtime, `runtime.json` and `studio/` all have tree hash
  `24b8163194c0`. The check also finds no build-machine path in any of the 18,769 files
  (4.28 GB, UTF-8 and UTF-16LE).
- **The models.** 15 files are copied, and the Studio's own code hashes every one against
  its registry (`ensure_model(verify=True, allow_download=False)` and
  `hfmodels.verify(force=True)`). Nothing can be downloaded. Afterwards `sha256sum -c
  SHA256SUMS.txt` gives 15 of 15 OK.
- **THIRD_PARTY_LICENSES.md.** It is regenerated for the dist runtime. It is identical to
  the committed file apart from the line that gives the runtime's build time.
- **Final checks.** `Settings/` holds only README.txt, there is no WebView2 profile, and no
  build-machine path appears in any small file. The demo song in `Library/` is
  byte-identical to `tests/fixtures/generated/demo-song.jksong`.

**Re-runnable.** It was run four times; the first stopped at the exe path check (section
5, fix 1). The third run left all 15 model files in place ("kept"), re-hashed them, and
gave the same layout and size as the second to within 30 bytes. Its output is the dist
that was proven (section 3). The fourth is the final `dist/`. Between
the proven dist and the final one, 9 files differ:

- The exe differs only in the PE link timestamp and the PE checksum. A rebuild compared
  byte by byte differs in exactly those 4 bytes (`cmp -l`), so the build is reproducible
  apart from that.
- `START-HERE.txt` differs because its exFAT advice was improved.
- `Models/manifest.json` differs in its verification times.
- 6 Studio runtime files differ, all benign:
  - `runtime.json` and `runtime-build.json` (the build records);
  - `RECORD` and `uv_cache.json` of jukeoke_studio (timestamps);
  - two `.pyc` files built from identical wheel sources. docs/STUDIO_RUNTIME.md
    section 3 traces one of them to the per-process hash seed; the second one
    (`sklearn/pipeline`) is new this time.

### Size against the budget (WORK_ORDER 4.8: apps + models under 12 GB, ideally under 9 GB)

`scripts/dist/dist_size.py` estimates the space on exFAT: every file takes whole clusters,
and every folder takes at least one.

| Folder | Bytes | Files | Folders | exFAT, 32 KB clusters | exFAT, 128 KB clusters |
|---|---|---|---|---|---|
| Windows/ | 4,295,512,921 | 18,771 | 2,343 | 4.809 GB | 6.711 GB |
| &nbsp;&nbsp;Windows/Jukeoke.exe | 12,422,158 | 1 | 0 | 0.012 GB | 0.012 GB |
| &nbsp;&nbsp;Windows/resources/ | 2,425,490 | 1 | 1 | 0.002 GB | 0.003 GB |
| &nbsp;&nbsp;Windows/Studio/ | 4,280,665,273 | 18,769 | 2,341 | 4.794 GB | 6.696 GB |
| Models/ | 3,819,962,697 | 18 | 8 | 3.821 GB | 3.823 GB |
| Library/ | 2,425,490 | 1 | 1 | 0.002 GB | 0.003 GB |
| Licenses/ | 127,232 | 13 | 6 | 0.001 GB | 0.002 GB |
| Mac/ (README only) | 2,812 | 1 | 1 | 0.000 GB | 0.000 GB |
| Recordings/, Settings/, START-HERE.txt | 6,993 | 3 | 2 | 0.000 GB | 0.000 GB |
| **Total** | **8,118,038,145** | **18,807** | **2,362** | **8.633 GB** (8.04 GiB) | **10.540 GB** |

- **On a 32 GB stick** (Windows formats it with 32 KB clusters), apps plus models take
  **8.63 GB: under the 9 GB target and the 12 GB limit**. With the NVIDIA compute cache that
  the first Studio song writes into `Settings/cache` (235.5 MB measured in the proof run),
  it is about 8.87 GB.
- **On a larger stick with the default 128 KB clusters**, it is 10.54 GB: under 12 GB, but
  over 9 GB, because of the Studio's 18,769 small files. START-HERE.txt therefore tells the
  owner to choose 32 KB clusters when formatting.
- **The Mac app is not included**, because it cannot be built here. A Tauri app is
  typically tens of MB, so it does not change the verdict, but it has never been measured.
- **Licences.** These are the owner's own stick contents. The three default separation
  weights are GPL-3.0 and Apache-2.0, Whisper turbo is MIT and wav2vec2 is Apache-2.0, so
  copying them along is allowed. The `clean` preset's unlicensed model is not on the
  stick. WORK_ORDER 10 says weights are "never bundled" in a *release*, so a public
  download should leave `Models/` empty (the app downloads the models on request).

## 3. Proof from a copy under another drive letter

`bash scripts/prove-dist.sh $JK_BUILD/dist-proof/inputs/cortez-feel-stripped-30s.wav`, run
2026-11-17 23:39-23:55. Logs are in `$JK_BUILD/dist-proof/20261117-233951/`. The input is a
30.0 s excerpt (18-48 s) of "Feel (Stripped)" by Cortez (CC BY, JamendoLyrics), decoded to
32-bit float with no other processing. It is never in the repository.

| # | Step | Result |
|---|---|---|
| 1 | Snapshot of %USERPROFILE%\.cache, %APPDATA%, %LOCALAPPDATA% and HKCU (`scripts/user-dirs-audit.py`) | 1,175,725 entries, 323 s |
| 2 | `dist/` copied to a fresh folder (69 s, 8,118,037,953 bytes). **`dist/` renamed to `dist.hidden` for the whole run**, so a hidden reference to the build location would fail. The copy mapped with `subst Q:` | ok |
| 3 | `Q:\Windows\Jukeoke.exe --smoke-test` | exit 0; **8 of 8 checks ok**: portable mode, root `Q:\`, WebView2 profile in `Q:\Settings\webview-windows`, UI rendered; 2.1 s |
| 4 | `Q:\Windows\Jukeoke.exe --selfcheck --silent` (default output device, digital silence) | exit 0, `result: PASSED`, **8 of 8 items OK**: Realtek 48 kHz shared, clock ratio 1.000, 0 underruns, output peak -inf dBFS; recorder 3.016 s for 3 s; demo song rendered; key change +2 / -5 at +0.000 cents with exact length. 6.0 s |
| 5 | `scripts/check-studio-runtime.sh Q:/ --sentinel-profile <empty dir>` (static and GPU checks) | **26 passed or informational, 0 failed**, 100 s. Highlights below the table |
| 6 | One real Studio job through the app, over WebView2 DevTools (`real-app-e2e.mjs queue`, null output device, master 0), input copied to `Q:\Recordings\` first, lyrics `auto` | Studio available 1.9 s after launch, models present. **Job done in 54.1 s**: decode 0.03, separate 46.0, lyrics (Whisper transcription) 3.0, align 2.8, analyse 1.0, write 1.3. The song has 7 transcribed lines, saved as `transcribed`, unchecked. `jukeoke-cli verify --strict`: **PASS, 0 errors, 0 warnings** |
| 7 | Every Library song opened and played on the null device (`real-app-e2e.mjs stage`) | **5 of 5 PASS**. New song: 0 wrong letters across 2,167 samples, min lookahead 2.08 s, playing, 0 underruns. Demo song: 0 wrong letters across 2,161 samples, min lookahead 2.12 s, playing, 0 underruns. No page errors |
| 8 | Second snapshot and diff | **0 entries written by the app or the Studio.** Registry: only the D-41 / D-88 entries Windows writes for any program (2 MuiCache values, 1 audio PolicyConfig key). Other changes: 6 OS (Credentials, WebCache, Remote Desktop trace, Recent), 18 by the test tooling (the Claude app), 83 by other programs (Firefox 40, OneDrive 15, a Codex app 7, TokenBroker 5, Docker 3, IdentityCache 3, OneAuth 3, Discord 5, Steam 2) |
| 9 | Logs copied out, `subst Q: /D`, copy deleted, `dist` renamed back | `subst` lists no mapping afterwards; the copy is gone |

Highlights of the runtime check in step 5:

- torch: CUDA matmul relative error 7.8e-7, sm 12.0.
- Both CTC model families run on the GPU.
- cuBLAS 12 SGEMM relative error 2.2e-7, and Whisper runs on CUDA.
- librosa finds C major; av decodes 24-bit FLAC bit-exact.
- Protocol: hello 0.1 s, probe 3.5 s, all models present.
- The Studio start-up points all 15 cache and temp variables at the stick (root `Q:\`).
- 337 modules loaded: 263 from the runtime, 63 from Windows, 11 from the NVIDIA driver,
  0 from anywhere else.
- Nothing was written into the runtime, and **0 files** appeared in the sentinel profile.
- Written on the copy: `Settings/cache/cuda` 235.5 MB, numba 0.2 MB.

Not covered by this proof:

- **No real exFAT USB stick.** A `subst` drive is the same NTFS volume. Because the volume
  serial is the same, the Studio trusted the model hashes recorded at build time and did
  not re-hash. On a real stick it hashes each default model file once (about 3.8 GB) before
  its first job, and that cost has not been measured on a stick.
- **The separation time is not a benchmark.** 46 s for a 30 s excerpt reflects a
  just-started Studio and the karaoke model's overlap of 8 on a short input. The 4-minute
  numbers are in docs/benchmarks/end-to-end.md.

## 4. What this sweep does not prove

- **Nothing ran in Safari / WKWebView or on macOS.** The UI build checks syntax and CSS
  support against Safari 16.4 statically.
- **No run on a clean Windows 11 PC or a clean CI runner.** This PC has the Visual C++
  redistributable and the WebView2 runtime. The exe's imports are checked (Windows DLLs
  only), and the Studio's imports are checked too (it uses its own VC runtime), but no PC
  without them was tried.
- **The Studio tests used the existing dev venv,** not a fresh `uv sync --frozen` as CI
  would.
- **Node versions differ.** CI pins Node 24; this PC has 25.6.1.
- **The full WORK_ORDER 9 benchmarks** (separation SDR, alignment, 20-minute soak, recorder)
  were not re-run. Their numbers are in docs/benchmarks/ and still apply: the DSP output
  is bit-identical (section 1, offline renders).

## 5. Fixes made during this pass (build and packaging)

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

   Not fixed: `scripts/build-mac.sh` has no such remap (it cannot be tested here), so a
   Mac build will contain the Mac user's home path.
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

From `.github/workflows/ci.yml`, job `macos` (macos-15 arm64, deployment target 13.0):

1. **`scripts/ci-test-crates.sh`**: the Rust tests of every crate on Apple Silicon. This
   covers Rubber Band compiled for ARM64 with Apple clang, jk-dsp accuracy and impulse
   tests on ARM64, jk-engine on Core Audio (cpal 0.16) and on the null device, jk-format,
   and jk-recorder's ScreenCaptureKit code.
2. **`scripts/build-mac.sh`**, the same script the owner runs on the MacBook:
   - UI build;
   - `cargo build --release` for aarch64-apple-darwin, so the first real compile of the
     Objective-C, C++ and Rust Mac code, and the first link;
   - assembling `Jukeoke.app`;
   - ad-hoc signing and `codesign --verify --deep --strict`;
   - **`scripts/check-macos-minos.sh`**: every binary and bundled library declares macOS
     13.0 or lower.
3. **`cargo test -p jukeoke` on macOS**: the app core tests, including the macOS portable
   paths and translocation handling.
4. **`scripts/ci-exfat-launch.sh`**:
   - an exFAT disk image laid out like the stick, the signature and minimum-macOS check
     repeated on the stick copy;
   - launching the app from there, directly and through `open`, in self-check mode;
   - collecting crash reports.

   Note: `JK_SELFCHECK_ARGS` there still defaults to `--smoke-test`, not `--selfcheck`
   (docs/MAC.md section 9).
5. **`scripts/ci-sck-probe.sh`**: a first ScreenCaptureKit system-audio capture. The runner
   may not be able to grant the Screen Recording permission.
6. **`scripts/ci-render.sh` on macOS and the `compare-renders` job**: macOS renders against
   Windows renders, within float tolerance (WORK_ORDER 9, "Same audio on both systems").
   Only the Windows half has run (section 1).
7. **The `release.yml` macOS job**, a manual release build. It is never triggered.

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

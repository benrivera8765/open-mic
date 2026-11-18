# Progress

Read this first in every session. Then DECISIONS.md, then docs/ARCHITECTURE.md.

## How to resume

- Project root: `C:\Users\benri\Downloads\jukebox-karaoke-work-order\jukebox-karaoke-work-order\jukeoke`
  (work order: `..\WORK_ORDER.md`; heavy local state: `..\jukeoke-build\`).
- In Git Bash: `source scripts/devenv.sh` puts cargo, gcc (WinLibs), uv and gh on PATH.
- Studio dev venv: `$JK_BUILD/studio-venv/Scripts/python.exe` (Python 3.12, torch 2.14.1+cu130).
- GitHub: deferred by the owner until the project is finished. Local git only. Do not ask for a login.
- `../test-songs/` was empty on 2026-11-17 06:00. Check again; if the owner adds songs, run them end to end.

## Done and verified

- 2026-11-17 Toolchains: rustc 1.100.0 (x86_64-pc-windows-gnu), WinLibs GCC 16.2 (MSVCRT), uv 0.13.8, CPython 3.12.16, Node 25.6.1.
- 2026-11-17 GPU: torch 2.14.1+cu130 runs matmul + STFT on the RTX 5070 Ti (sm_120). WO trap 1 cleared.
- 2026-11-17 Contracts: docs/SONG_FORMAT.md v1.0, docs/STUDIO_PROTOCOL.md v1, docs/APP_API.md v1. Design brief docs/DESIGN.md.
- 2026-11-17 Stage 0 (commit 215b775), each independently re-verified:
  - Audio engine prototype: Rubber Band 4.0.0 R3; pitch error <= 0.0015 cents for -12..+12; 0-semitone path bit-exact; no clicks on key change; drum path offset 0, band-vs-drum mean <= 0.31 ms; 60 s play with 14 busy threads: 0 underruns; clock follows the device (-21 ppm); exe imports only Windows DLLs. docs/benchmarks/dsp.md.
  - Tauri 2 shell on the GNU target: launches, renders in WebView2, portable from a subst drive letter, own WebView2 loader, 6.0 MB exe.
  - Song file library jk-format + Python jksong.py, cross-tested (tests/conformance).
  - Mac: CI workflows, build-mac.sh, minos check, ScreenCaptureKit recorder: TYPE-CHECKED ONLY (no Mac, no GitHub).
- 2026-11-17 Lyrics/alignment/analysis (commit 92d3d25, verified): JamendoLyrics 20 songs, word-onset median 0.036 s, 94-95 % within 0.3 s; wav2vec2-large-960h-lv60-self (Apache-2.0); faster-whisper large-v3-turbo transcription.

- 2026-11-17 Stage 1-6 player side (commit 2ae4cce), independently verified: Engine API on .jksong (in-place sample-accurate seek ~40-50 ms, device switch/loss, export Wav32f bit-identical / Flac24 within 0.5 LSB); 20-min run with 14 busy threads 0 underruns, 49 MB; WASAPI loopback recorder (start/stop in silence covered; not bit-identical because this PC's Realtek driver applies EQ/speaker effects; residual about -125 dBFS after a linear fit); app core implements docs/APP_API.md with a fake-Studio-tested supervisor, self-check 8/8 (silent), native startup error dialog; UI + canvas ticker (0 wrong letters across the demo song at 4 sizes, lookahead >= 2.24 s) + sync editor + recorder/studio screens; jukebox skin wired in (screenshots in ../jukeoke-build/screenshots/). Tests: 170 Rust workspace, 113 UI.

- 2026-11-17 Separation (commit 512126d), verified: default "full" preset (anvuew ft1 vocals GPL-3.0, anvuew karaoke GPL-3.0, SYH99999 drums Apache-2.0); 4:04 song ~155 s, 4.1 GB VRAM; stems sum back bit-exactly; fp32 only. docs/benchmarks/separation.md, MODELS.md.
- 2026-11-17 Fixes (commit de51f7e): export gain notice in the UI; jk-format seek fix (symphonia stale parser, 1336 seeks bit-exact); DECISIONS D-57..D-121, ARCHITECTURE and APP_API synced with the code.
- 2026-11-17 Studio pipeline + protocol server (NOT committed; tested by the stream, awaiting the workflow's verifiers): `python -I -m jukeoke_studio serve` implements docs/STUDIO_PROTOCOL.md (additions in its section 11) and `make` runs one song; verifier findings fixed (no silent downloads, 48 kHz single conversion each way, real `permissive` preset, .th re-hash before unpickling, realign drops only infeasible anchors, auto-lyrics fall-through, `lyrics.source`); portable caches into `<root>/Settings/cache` (Studio + supervisor `--root`/env); DECISIONS D-140..D-158. Evidence: studio pytest 207 passed (CPU, stand-in models over a real pipe), GPU tests on a CC BY 30 s excerpt (every song `jukeoke-cli verify --strict`), Rust studio_supervisor 15 passed incl. the real Studio in dev mode; 4:00.6 CC BY song 193 s (lyrics given) / 229 s (transcribed), per stage in D-158; per-user folders unchanged during both runs. Re-run 2026-11-17 20:12-20:25 on the unchanged code: CPU pytest 208 passed / 1 skipped (88.6 s), `pytest -m gpu` 24 passed (212 s), Rust studio_supervisor 15 passed (real Studio start + probe 8.9 s), both 4-minute songs `verify --strict` PASS. Open for other streams: jk-format verify and SONG_FORMAT 4.1 / APP_API / UI `StudioPreset` must learn the preset value `permissive`.

## In flight

- Workflow `jukeoke-studio` (wf_7ffce523-0c5): Studio `serve` protocol server + job pipeline + verifier fixes (no silent downloads, 48 kHz single resample, permissive preset, cache redirection into the stick); relocatable runtime build script + size budget; then end-to-end through the real app on 3 CC songs (+48 kHz, MP4, corrupt file), weak-computer tests, portable audit; then two verifiers.

## Next

- (in flight in jukeoke-studio) Studio stage: merge Python deps into studio/pyproject.toml (list in the lyrics build report: regex, mutagen, av, transformers, faster-whisper, nvidia-cublas-cu12 (win32), librosa, soxr, numpy, soundfile); add `lyrics.source_hint` to job.submit so lookup results are recorded as `online:lrclib` (protocol gap); realign should drop only the infeasible anchor, not all; resolve_lyrics('auto') should fall through when given LRC/text normalises to nothing; register models in MODELS.md (wav2vec2-large-960h-lv60-self default, HuBERT option, faster-whisper large-v3-turbo default, large-v3 optional); pipeline orchestration (PyAV decode, tags/cover, separate, lyrics, align, analyse, write), protocol server (docs/STUDIO_PROTOCOL.md), model registry + downloads with sha256, relocatable Python runtime in dist/Windows/Studio, test-songs end to end (folder was empty), speed for a 4-min song.
- Stage 7: dist/ layout (WO 4.8), size budget, portable audit, weak-computer tests (GPU hidden, Studio removed), README, CONTRIBUTING, THIRD_PARTY_LICENSES.md, MODELS.md final, listening + MacBook checklists, HANDOFF.md.
- Open from Stage 1 verifiers: Studio must redirect per-user caches (HF_HOME, TORCH_HOME, XDG_CACHE_HOME, CUDA_CACHE_PATH, TMP/TEMP, MPLCONFIGDIR) into the stick in portable mode; THIRD_PARTY_LICENSES.md must be created (Tauri crates, npm packages, Rubber Band GPL, rubato, symphonia MPL, three OFL fonts, Python deps incl. nvidia-cublas EULA); an idle leftover Vite dev server (port 5191, PID 4556) from the skin stream could not be stopped by the agent (permission denied) - harmless.
- Stage 0 verifier minor items still open: recorder CI probe flags, ci-compare-renders single-platform pass, THIRD_PARTY_LICENSES.md missing.

## Notes for a fresh session

- 2026-11-17 18:28-20:10: usage limit hit again during the Studio workflow; both build agents restarted at 20:11 and resume from files on disk.
- 2026-11-17 12:10-15:10: the account's 5-hour usage limit was hit; every running agent stopped and the workflows restarted them after the reset. Keep workflows to a few agents at a time and avoid re-running finished benchmarks. Separation stream is on its third attempt (it resumes from files on disk).
- `.gitignore` ignores `Models/` and Windows git is case-insensitive, so no source folder may be called `models/` (the Studio's model registry lives in `studio/jukeoke_studio/modelzoo/`).

- No Visual Studio on this PC, and no admin approval available: stay on the GNU toolchain (DECISIONS D-10).

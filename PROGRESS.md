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
- 2026-11-17 Lyrics/alignment (not yet committed, verifier was re-running): JamendoLyrics 20 songs, word-onset median 0.036 s, 94-95 % within 0.3 s; wav2vec2-large-960h-lv60-self (Apache-2.0); faster-whisper large-v3-turbo transcription.

## In flight

- Workflow `jukeoke-stage0` (wf_6c491af5-270): only the separation build is still running (restarted after an API error at 08:26); then its verifier. Commits pending: studio/, MODELS.md, docs/benchmarks/separation.md.
- Workflow `jukeoke-format-align` (wf_6acf924b-7ee): lyrics verifier running (restarted). Commit pending: studio/ lyrics files, docs/benchmarks/alignment.md; docs/benchmarks/analysis.md content is in the build report's next_steps (a hook blocked the subagent from writing it): write it from there.
- Workflow `jukeoke-stage1-app` (wf_811279e5-4e3): engine on .jksong + export + demo song + 20-min run; Windows WASAPI loopback recorder; app core implementing docs/APP_API.md (+ fake Studio, self-check, startup error dialog); functional UI + lyrics ticker + sync editor; jukebox skin; then integrator + verifiers.

## Next

- Studio stage (after separation + lyrics land): pipeline orchestration (PyAV decode, tags/cover, separate, lyrics, align, analyse, write), protocol server (docs/STUDIO_PROTOCOL.md), model registry + downloads with sha256, relocatable Python runtime in dist/Windows/Studio, test-songs end to end (folder was empty), speed for a 4-min song.
- Stage 7: dist/ layout (WO 4.8), size budget, portable audit, weak-computer tests (GPU hidden, Studio removed), README, CONTRIBUTING, THIRD_PARTY_LICENSES.md, MODELS.md final, listening + MacBook checklists, HANDOFF.md.
- Stage 0 verifier minor items still open: recorder CI probe flags, ci-compare-renders single-platform pass, THIRD_PARTY_LICENSES.md missing.

## Notes for a fresh session

- No Visual Studio on this PC, and no admin approval available: stay on the GNU toolchain (DECISIONS D-10).

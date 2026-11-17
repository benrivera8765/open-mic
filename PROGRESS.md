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

- 2026-11-17 Contracts written: docs/SONG_FORMAT.md (v1.0), docs/STUDIO_PROTOCOL.md (v1). Design brief: docs/DESIGN.md (from the four reference photos).

## In flight

- Workflow `jukeoke-stage0` (run wf_6c491af5-270): separation benchmark (studio/), audio engine prototype (crates/jk-rubberband, jk-dsp, jk-engine, jk-cli), Mac pipeline (.github/, scripts/build-mac.sh, crates/jk-recorder), Tauri shell on GNU (app/). Each has an independent verifier.
- Workflow `jukeoke-format-align` (run wf_6acf924b-7ee): crates/jk-format + studio/jukeoke_studio/jksong.py + tests/conformance; lyrics/LRC/transcribe/align/analyse + JamendoLyrics benchmark.
- If a session restarts and these runs are gone, check which files exist and resume the missing streams (the workflow scripts are under ~/.claude/projects/.../workflows/scripts/).

## Next

- Stage 1 to 7 per WO section 8.

## Notes for a fresh session

- No Visual Studio on this PC, and no admin approval available: stay on the GNU toolchain (DECISIONS D-10).

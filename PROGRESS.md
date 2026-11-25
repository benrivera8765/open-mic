# Progress

Read this first in every session. Then DECISIONS.md, then docs/ARCHITECTURE.md.

## How to resume

- Project root: the `jukeoke` folder next to `WORK_ORDER.md` (on the build PC it sits under `%USERPROFILE%\Downloads\jukebox-karaoke-work-order\jukebox-karaoke-work-order\`).
  (work order: `..\WORK_ORDER.md`; heavy local state: `..\jukeoke-build\`).
- In Git Bash: `source scripts/devenv.sh` puts cargo, gcc (WinLibs), uv and gh on PATH.
- Studio dev venv: `$JK_BUILD/studio-venv/Scripts/python.exe` (Python 3.12, torch 2.14.1+cu130).
- GitHub: deferred by the owner until the project is finished. Local git only. Do not ask for a login.
- `../test-songs/` was empty on 2026-11-17 06:00. Check again; if the owner adds songs, run them end to end.

## Done and verified

- 2026-11-17 Toolchains: rustc 1.100.0 (x86_64-pc-windows-gnu), WinLibs GCC 16.2 (MSVCRT), uv 0.13.8, CPython 3.12.16, Node 25.6.1.
- 2026-11-17 GPU: torch 2.14.1+cu130 runs matmul + STFT on the RTX 5070 Ti (sm_120). WO trap 1 cleared.
- 2026-11-17 Contracts: docs/SONG_FORMAT.md v1.0, docs/STUDIO_PROTOCOL.md v1, docs/APP_API.md v1. Design brief docs/DESIGN.md.
- 2026-11-17 Stage 0 (commit faf62ff), each independently re-verified:
  - Audio engine prototype: Rubber Band 4.0.0 R3; pitch error <= 0.0015 cents for -12..+12; 0-semitone path bit-exact; no clicks on key change; drum path offset 0, band-vs-drum mean <= 0.31 ms; 60 s play with 14 busy threads: 0 underruns; clock follows the device (-21 ppm); exe imports only Windows DLLs. docs/benchmarks/dsp.md.
  - Tauri 2 shell on the GNU target: launches, renders in WebView2, portable from a subst drive letter, own WebView2 loader, 6.0 MB exe.
  - Song file library jk-format + Python jksong.py, cross-tested (tests/conformance).
  - Mac: CI workflows, build-mac.sh, minos check, ScreenCaptureKit recorder: TYPE-CHECKED ONLY (no Mac, no GitHub).
- 2026-11-17 Lyrics/alignment/analysis (commit b8ad94e, verified): JamendoLyrics 20 songs, word-onset median 0.036 s, 94-95 % within 0.3 s; wav2vec2-large-960h-lv60-self (Apache-2.0); faster-whisper large-v3-turbo transcription.

- 2026-11-17 Stage 1-6 player side (commit f0e6c1e), independently verified: Engine API on .jksong (in-place sample-accurate seek ~40-50 ms, device switch/loss, export Wav32f bit-identical / Flac24 within 0.5 LSB); 20-min run with 14 busy threads 0 underruns, 49 MB; WASAPI loopback recorder (start/stop in silence covered; not bit-identical because this PC's Realtek driver applies EQ/speaker effects; residual about -125 dBFS after a linear fit); app core implements docs/APP_API.md with a fake-Studio-tested supervisor, self-check 8/8 (silent), native startup error dialog; UI + canvas ticker (0 wrong letters across the demo song at 4 sizes, lookahead >= 2.24 s) + sync editor + recorder/studio screens; jukebox skin wired in (screenshots in ../jukeoke-build/screenshots/). Tests: 170 Rust workspace, 113 UI.

- 2026-11-17 Separation (commit 87e5b01), verified: default "full" preset (anvuew ft1 vocals GPL-3.0, anvuew karaoke GPL-3.0, SYH99999 drums Apache-2.0); 4:04 song ~155 s, 4.1 GB VRAM; stems sum back bit-exactly; fp32 only. docs/benchmarks/separation.md, MODELS.md.
- 2026-11-17 Fixes (commit 1c61206): export gain notice in the UI; jk-format seek fix (symphonia stale parser, 1336 seeks bit-exact); DECISIONS D-57..D-121, ARCHITECTURE and APP_API synced with the code.
- 2026-11-17 Studio pipeline + protocol server (NOT committed; tested by the stream, awaiting the workflow's verifiers): `python -I -m jukeoke_studio serve` implements docs/STUDIO_PROTOCOL.md (additions in its section 11) and `make` runs one song; verifier findings fixed (no silent downloads, 48 kHz single conversion each way, real `permissive` preset, .th re-hash before unpickling, realign drops only infeasible anchors, auto-lyrics fall-through, `lyrics.source`); portable caches into `<root>/Settings/cache` (Studio + supervisor `--root`/env); DECISIONS D-140..D-158. Evidence: studio pytest 207 passed (CPU, stand-in models over a real pipe), GPU tests on a CC BY 30 s excerpt (every song `jukeoke-cli verify --strict`), Rust studio_supervisor 15 passed incl. the real Studio in dev mode; 4:00.6 CC BY song 193 s (lyrics given) / 229 s (transcribed), per stage in D-158; per-user folders unchanged during both runs. Re-run 2026-11-17 20:12-20:25 on the unchanged code: CPU pytest 208 passed / 1 skipped (88.6 s), `pytest -m gpu` 24 passed (212 s), Rust studio_supervisor 15 passed (real Studio start + probe 8.9 s), both 4-minute songs `verify --strict` PASS. The preset value `permissive` in jk-format verify, SONG_FORMAT 4.1, APP_API and the UI: done by the end-to-end stream (D-161).

- 2026-11-17 End to end through the real app (NOT committed; docs/benchmarks/end-to-end.md, evidence in `$JK_BUILD/e2e/`):
  - Setup: release exe + rebuilt Studio runtime + default models on a 4.8-layout stick (`$JK_BUILD/e2e-stick`, 8.79 GB with 3 songs), driven over WebView2 DevTools (`app/ui/scripts/real-app-e2e.mjs`), null device, master 0.
  - Queue: 3 CC songs (48 kHz FLAC, float WAV, MP4/AAC) and a corrupt file. The corrupt file failed with a plain message and the queue went on. All 3 songs pass `verify --strict` (0 warnings).
  - Speed: 4-minute songs 174.6 / 175.1 s; Crowdpleaser (3:12.8) 131.0 s, 153.3 s on a repeat that gave bit-identical output.
  - Word onsets vs JamendoLyrics: median 0.045 / 0.033 s, worst 0.20 s, 100 % within 0.3 s.
  - Renders at 0/+2/-5: exact length, expected chroma shift.
  - Residual on the ccMixter song: lead -6.0 dB with backing on, -20.8 dB off; that song is one of the benchmark's hardest.
  - Ticker: 0 wrong letters. Lookahead on the rap song is 1.46 s (font at its 0.6x floor; WO 4.4 wants 2 s; owner decision).
  - Weak computers: GPU hidden, Studio folder removed, a python.exe that hangs, one that crashes. In each, the window is ready in 0.6 s, the song plays and a plain reason is shown (64.6 s for the hang, at most 0.9 s otherwise).
  - Audit: nothing from the app or Studio in per-user folders. Exceptions: the D-41 MuiCache values, and Direct3D/NVIDIA shader-cache writes that could not be attributed.
  - Fixed: the UI Studio screen stuck on "checking" (D-159); the corrupt-file message (D-160); the `permissive` preset in verify, docs and UI (D-161).
- 2026-11-17 21:40-22:10 Independent verifier, Studio pipeline + protocol + end to end (NOT committed; evidence in `$JK_BUILD/verify-studio/`):
  - Studio CPU pytest: 208 passed / 1 skipped before the fixes, 209 after.
  - GPU run of the stick's packaged runtime over the protocol: 0 non-JSON stdout lines in 357; cancel in separation answered in 1.13 s with no temporary files left; the corrupt file failed `decode_failed` and the next job started at once; transcribed lyrics were saved as `transcribed`, `checked: false`; an empty models folder gave `models_missing`, created nothing, and an HTTP(S) proxy trap saw 0 connections over the whole run.
  - Feel (Stripped) 48 kHz: 188.9 s; `verify --strict` PASS with 0 warnings; provenance model hashes equal the bytes on disk; resampling 48->44.1 once and back three times; output bit-identical to the e2e song.
  - Fixed: VRAM held after a failed or cancelled job (D-163), realign anchor dropping (D-164), the UI's online-lyrics label (D-165), two small races (D-166). UI vitest 122 passed, tsc clean.
  - Stale after these fixes: `$JK_BUILD/e2e-stick/Windows/Studio` (built before them) and its `Jukeoke.exe`. Rebuild both before the next stick or dist build. The Rust studio_supervisor tests were not re-run by the verifier (no Rust code changed).

- 2026-11-17 Studio stage (commit 1af4db1), verified twice: `serve` protocol server + pipeline; relocatable runtime runs from any drive letter; apps+runtime+models 8.66 GB; end to end through the real app on 3 CC songs (48 kHz FLAC, float WAV, MP4) + corrupt file: ~3 min per 4-min song, word onsets median 0.033-0.045 s, 100 % within 0.3 s; weak-computer cases explain and never hang; nothing written to per-user folders. docs/benchmarks/end-to-end.md, docs/STUDIO_RUNTIME.md.

- 2026-11-18 Stage 7 (commit 3049134): dist/ (8.63 GB on exFAT with 32 KB clusters; proven from a subst copy: smoke 8/8, self-check 8/8, real Studio job, 0 per-user writes); shipped exe free of build-machine paths; THIRD_PARTY_LICENSES.md, README, CONTRIBUTING, screenshots; ticker >= 2.0 s lookahead on all test songs; local CI sweep all green (docs/TEST_RESULTS.md); completeness critic table docs/WORK_ORDER_STATUS.md; HANDOFF.md draft.

- 2026-11-18 Gap closing (commit ede47fa), verified twice: key-change render-ahead-and-cache mode (WO 4.5), Rubber Band allocation on the render worker fixed (RT path 0 allocations), cover-art correction, ticker pause when minimised, Mac CI evidence steps fail on missing evidence + macOS 13 availability audit + build-mac.sh path remap, full licence texts in dist/Licenses, 20-min real-app soak (0 underruns, ticker 60 fps, whole app ~0.55-0.61 GB on Windows), karaoke loudness/stereo image measured. Local CI all green (docs/TEST_RESULTS.md).
- 2026-11-18 HANDOFF.md written. Everything that can be done unattended on this PC is done.

- 2026-11-18 HANDOFF.md independently reviewed against the evidence and corrected (commit 9bad430). Last fixes (commit c709a35): Clean/Permissive model downloads from the app with licence shown first, accurate preset hints, key-progress display, real-app runs of recording -> karaoke, re-align, cover replacement, OLE drag-and-drop; dist/ rebuilt and proven again; local CI all green.


### Owner changes, 18 November 2026 (each built, independently verified, committed)

- Lyrics ticker paced smoothly by word (commit e29f161, D-216/D-217): `ui.lyrics_motion` word (default) | letter.
- Static-line lyrics with a gold word-paced wipe, lead-in bar and next-line preview (commit 0ac3097, D-218): `ui.lyrics_display` page (default) | scroll; default colours gold #ffc21a / white #f4f4f4.
- Progress bar (needle) riding the wipe edge in the static-line display (commit 3cba37e, D-219/D-220): `ui.show_marker`.
- Record & play via Windows media controls, and "Record songs separately" from a playlist split at track changes (commit 502ec6b, D-221..D-229). Windows only. Never tested against the real Spotify.
- Delete songs from the Library, with Deleted songs (restore / delete for good) and multi-select (commit ef62188, D-230..D-233).
- The owner made their own song (dist/Library) and a recording (dist/Recordings) and has settings in dist/Settings. Never use `make-dist.sh --force-clean`; replace only rebuilt files in dist/.

## In flight

- Nothing. 2026-11-23 evening: Settings > Themes (Deluxe, Classic, Ocean, Electric, Molten; each option still independent, click again to reset) and the Molten theme: fire pipe, forge cabinet, bronze, iron, magma badge, ember glow and note bars (D-271); on Windows the forge is flowing lava and the pipe flame is drawn on the GPU (D-272); five more themes, Water, Steampunk, Cyberpunk, Dieselpunk and Frutiger Aero, each with its own GPU cabinet and pipe (D-275); Steampunk steam and the Cyberpunk city reworked (D-276); pre-launch fixes and a README for GitHub (D-277); Dieselpunk and Frutiger Aero removed before release, eight themes remain (D-278). 2026-11-25: practice bars follow the words: no bars where nothing is sung, none held into the singer's echo (D-280). The Recorder folds away songs already made into karaoke songs or hidden, and whole finished sessions (D-281). 2026-11-22 evening, jukebox customisation (DECISIONS D-256..D-259): Settings > Customize the jukebox (pearl / walnut / maple cabinet, neon / pearl-pilaster border, silver or red bands, lapis / red / silver badge with gold or cream lettering, silver or gold lattice, silver inner trim), Watch only that follows the singer, perfect-phrase celebrations (4 looks). 2026-11-23: pilaster closer to photo 7 and column bases follow the cabinet (D-260); liquid pilaster tubes, glossy pearl, Lava lamp and Lighting tube borders, lapis badge frame (D-261); lightning (plasma) tube and a rising, natural lava lamp (D-262); full-colour pilaster pearl with amber tubes kept amber (D-263); lightning tube as clear blue water with one continuous Tesla-coil arc end to end (D-264, D-265); deeper water, two arcs, and an aquarium cabinet (D-266); ocean-tube border to match it (D-267); practice melody octave alignment for every song, old ones included (D-268); melody builder 0.3.0 chooses octaves from spectrum + melody, all library songs rebuilt (D-269); lane polish: words joined, lit slides, labels under their bars, 5-s skip icons, on-pitch cursor effects (D-270). UI tests 539 pass; checked in the browser mock (screenshots), not yet in the real app with a real song.
- Earlier on 2026-11-22: owner changes done (DECISIONS D-251..D-254): practice bars on piano keys with slivers folded in (jk-guide singable, used for lane and grade), every lyric word under the lane, no faded/violet bars, delete recorded sessions, paste-your-own lyrics in the Recorder dialog and for waiting queue songs, gold chrome, renamed Open Mic (dist/Windows/Open Mic.exe replaces Jukeoke.exe).
- Earlier: WORK ORDER 2 (Practice Mode) finished 2026-11-20 (commits up to the "WO2 finish" commit): practice mode in the app on Windows (microphone, note lane + piano keys + marker, grading, results/history), guides in every song in dist/Library (checksum proof: only guide chunks added; 385 other owner files unchanged), new dist/Windows/Jukeoke.exe, docs/benchmarks/practice.md, HANDOFF section 10 (three classes, accepted limitations D-250, first-practice + MacBook checklists), local CI all green (ui 529, rust 439, studio 251+24, conformance, portable, lint). Mac: built but not verified. Nobody has sung into it. The stopped echo fix 2 partial work is in `git stash` (two entries) and ../jukeoke-build/echo-fix2-partial/.

## Next (needs the owner; see HANDOFF.md section 8)

- GitHub login (`gh auth login`), then a session: create the private repo, push, run CI, read the macOS job (build, availability audit REVIEW lines, exFAT self-check, SCK probe, Windows-vs-Mac render comparison) and fix what it finds.
- MacBook: build the app (`bash scripts/build-mac.sh`) or take the CI artifact, run the self-check, bring the stick back; a session reads `Settings/selfcheck-*.txt`.
- Listening checklist results (HANDOFF section 6).
- Licence decisions: the Studio runtime with NVIDIA libraries + GPL code; the unlicensed "clean" preset model; 7 crates' missing notices; git history (user-folder path in 17 commits, author name).
- Songs in `test-songs/` to run end to end.

## Notes for a fresh session

- 2026-11-20 ~03:25 -> ~05:30: usage limit again; workflows restarted their agents automatically. The standalone tracker copy finished and its result landed in wf_171f937d-ff8 as build:q-tracker (same id), so that workflow is NOT hung after all: its q-tracker verifier is running.

- 2026-11-20 ~00:57: relaying a message to a workflow agent by its id RESUMED A DUPLICATE COPY of it as a standalone background agent (a169eb8e0854df3fb, transcript in subagents/ not subagents/workflows/). TaskStop on that id stopped the WORKFLOW's instance instead; the standalone copy kept working and is now the tracker quality builder (it has the full context and the demo-low-key regression report). The relaunch wf_8bd71af6-311 was stopped at 01:49 to keep one editor. When the standalone copy reports (task notification), run an independent tracker verifier. Lesson: never SendMessage to workflow agents. The quality-round workflow wf_171f937d-ff8 will never finish (its tracker item is dead): harvest the score and echo results from its journal, then TaskStop it.

- 2026-11-19 ~23:05 -> 2026-11-20 ~00:30: usage limit again; the Stage 3b builder and the 3 quality-round builders were restarted automatically by their workflows (they resume from files on disk).

- 2026-11-17 18:28-20:10: usage limit hit again during the Studio workflow; both build agents restarted at 20:11 and resume from files on disk.
- 2026-11-17 12:10-15:10: the account's 5-hour usage limit was hit; every running agent stopped and the workflows restarted them after the reset. Keep workflows to a few agents at a time and avoid re-running finished benchmarks. Separation stream is on its third attempt (it resumes from files on disk).
- `.gitignore` ignores `Models/` and Windows git is case-insensitive, so no source folder may be called `models/` (the Studio's model registry lives in `studio/jukeoke_studio/modelzoo/`).

- No Visual Studio on this PC, and no admin approval available: stay on the GNU toolchain (DECISIONS D-10).

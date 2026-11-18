# Work order status

Completeness review, 18 November 2026. Each requirement in `../WORK_ORDER.md` sections 2,
4.1-4.10, 5, 7 (all 14 traps), 9 (every evidence row), 10, 11 and 13 is checked against
the repository, `docs/benchmarks/*`, `docs/TEST_RESULTS.md`, the logs in `$JK_BUILD`
(`../jukeoke-build`) and the code. Section 3 is covered briefly at the end.

**Statuses**

- **DONE+VERIFIED**: a test or measurement exists and is cited. Unless the row says
  otherwise, it was measured on the Windows build PC only.
- **BUILT-NOT-VERIFIED**: the code or document exists, but nothing has proven it works.
  The row says why (no Mac, no GitHub, no exFAT stick, no clean PC, needs ears).
- **PARTIAL**: some of the requirement is done; the row says what is missing.
- **NOT DONE**: nothing in the repository meets it.

"Mac" rows mean: the code is written and passes `cargo check --target aarch64-apple-darwin`
(TEST_RESULTS section 1). That check **compiles no Objective-C or C++, links nothing and
runs nothing**. The Mac app has never been built.

**Update, 18 November 2026 (final evidence pass).** Rows changed by the gap-closing work and re-verified by the final local CI run, the real-app soak, the allocation test and the separation measurement are marked "18 Nov" in their evidence. Sections 16 and 17 below are the review's original text.

## 0. What this review re-checked itself

This review did not re-run any benchmark. It checked these facts directly:

| Check | Command / file | Result |
|---|---|---|
| dist size and file count | `du -sb dist; find dist -type f \| wc -l` | 8,118,038,145 bytes, 18,807 files (same as TEST_RESULTS section 2) |
| Test sweep numbers | `$JK_BUILD/test-sweep/20261117-235615/summary.tsv`; sum of `test result` lines in `rust.log`; tails of `ui.log`, `studio-*.log` | UI 159 passed; Rust 176 passed, 0 failed, 0 ignored; Studio CPU 223 passed, 1 skipped (network test); GPU 24 passed; conformance PASS; portable OK. The first lint run had 195 findings; the re-runs `20261118-001537` and `-003541` had 0 |
| Windows render determinism | `$JK_BUILD/test-sweep/ci-compare-vs-earlier.log` | 18 files, 0 failures, max diff 0 |
| dist proof | `$JK_BUILD/dist-proof/20261117-233951/summary.txt`, `smoke-verify.log`, `selfcheck-2026-11-17_23-47-12.txt` | `total 891 s, 0 failed`; smoke test 8 `ok`; self-check `result: PASSED`, 8 `[ OK ]` items |
| Bubbles + ticker in the real app | `$JK_BUILD/screenshots/app/real-app-screens.json` `perf` | **5 s** run: ticker 60 fps, bubbles 30 fps (57 alive), underruns 0, **no CPU load** |
| Media in git history | `git log --all --name-only` filtered for audio, image and song extensions | only app icons (png) and the 2 generated fixtures (`demo-song.jksong`, `tiny.jksong`); untracked: 6 screenshots, attributed in `docs/screenshots/README.md` |
| Brand names | `git grep -i -E "wurlitzer\|seeburg\|rock-?ola\|rowe ami"`, plus the same over untracked files | no hits |
| test-songs | `ls ../test-songs` | only `PUT-SONGS-HERE.txt` |
| Real-time allocation checks | `grep -rln "global_allocator"` over crates and app | **Updated 18 Nov:** `crates/jk-engine/tests/rt_alloc.rs` now counts Rust and C/C++ allocations per thread; 0 on the render worker and the device thread across play, every key, seeks, mixer, pause, song change and prerender (playback.md 11) |
| Check of imported Mac APIs | grep for `nm`, `dyld_info`, symbol checks in `scripts/`, `.github/` and `docs/MAC.md` | none. `check-macos-minos.sh` reads only the minimum-OS load command |

## 1. Section 2: the priority order

| # | Priority | Status | Evidence |
|---|---|---|---|
| 1 | Audio quality, no fidelity shortcut | DONE+VERIFIED (Windows), with documented deviations | fp32 separation with TF32 off (separation.md 6.1); stems sum to the source within 5.96e-8, exact for 24-bit sources (separation.md 6.3; end-to-end.md 5); bit-transparent at key 0 (dsp.md 3); export Wav32f bit-identical, Flac24 within 0.5 LSB (playback.md 6). Deviations: resampling more than once for 48 kHz sources, and Rubber Band's internal resampler (section 6 of this file, §5 row 3). Nobody has listened yet: HANDOFF section 6 is the listening checklist |
| 2 | Mac plays Windows-made songs the same | **BUILT-NOT-VERIFIED: written and type-checked only; never compiled, linked or run** | No Mac and no GitHub (D-09). No Mac build, no Mac render, no minimum-macOS check and no Mac self-check exists. TEST_RESULTS section 6 lists everything that has not run |
| 3 | Key change | DONE+VERIFIED (Windows) | Pitch error at most 0.0015 cents for -12..+12, exact length (dsp.md 1); no click (d2 ratio at most 1.26); drums unshifted. **18 Nov:** the render-ahead-and-cache fallback exists and is tested (D-176..D-186; `tests/keycache.rs`, part of the local CI Rust suite, 194 passed, 0 failed in 34 binaries); a 20-minute real-app run with key changes every 30 s had 0 underruns and 0 device-starved events (playback.md 12). Mac not run |
| 4 | Lyrics accuracy and the marker | DONE+VERIFIED | Word onsets: median 0.036 s over 20 JamendoLyrics songs (alignment.md); 0.033 / 0.045 s through the real app (end-to-end.md 5.2); 0 wrong letters in every ticker run |
| 5 | Recorder | DONE+VERIFIED (Windows) / BUILT-NOT-VERIFIED (Mac) | recorder.md; the macOS ScreenCaptureKit backend has never run |
| 6 | USB portability | PARTIAL | Windows: tested from another drive letter (`subst Q:`) and from a copy (dist proof). Not tested: a real exFAT stick, a clean PC, and the Mac (the exFAT launch exists only as a CI script) |
| 7 | Jukebox look and bubbles | DONE (Windows WebView2) / BUILT-NOT-VERIFIED (WKWebView) | Skin components in `app/ui/src/skin/`; screenshots at 1280x800, 1440x900 and 3840x2160 in `$JK_BUILD/screenshots/app/`; never rendered in Safari or WKWebView |

## 2. Section 4.1: Studio

| Requirement | Status | Evidence |
|---|---|---|
| Input: mp3 m4a flac wav ogg opus aiff wma, and mp4 mkv mov webm avi | DONE+VERIFIED | `studio/tests/test_media.py::test_decode_every_container` covers all 13 containers (wav at 16 and 24 bit, so 14 test cases): native rate, lossless inputs decoded exactly. Part of the CPU sweep (223 passed). Real app: 48 kHz FLAC, float WAV and MP4 (end-to-end.md 3) |
| Drag-and-drop or file picker | DONE+VERIFIED (Windows; drop from a helper window, not Explorer) | `lib.rs:140` turns `WindowEvent::DragDrop` into `studio://dropped`, which `state/app.ts:188` handles. **18 Nov:** a real OS (OLE, CF_HDROP) drag-and-drop onto the release app from a portable copy reached `tauri://drag-drop`, the core's handler and the Studio form, in 2 of 3 attempts (the third was cancelled by the helper's own safety check; TEST_RESULTS section 8, `app/ui/scripts/ole-drop.ps1`). Not tried: dragging from Explorer itself, several files at once. The native file picker was not driven |
| Recorder captures go into the queue | DONE+VERIFIED | **18 Nov, real app:** a 10.05 s recording (32-bit float, 2 ch, 48 kHz, digital silence) made with the Recorder's buttons, then "Make a karaoke song from this" opened it in the Studio form; queued and done, `verify --strict` PASS (TEST_RESULTS section 8) |
| Queue several files; one bad file does not stop the queue | DONE+VERIFIED | `corrupt.m4a` failed in 0.04 s and the next job started in the same millisecond (end-to-end.md 3); verifier run in D-163 to D-166 |
| Decode to float at the original rate | DONE+VERIFIED | D-143; `test_media.py` (exact for 16/24-bit); 48 kHz song kept at 48 kHz (end-to-end.md 5) |
| Read title, artist, cover; the user can correct them | DONE+VERIFIED (tests) | Title, artist and album editable (`Studio.tsx`, `saveMetadata`). **18 Nov:** the cover can be replaced, removed or kept (`metadata.cover_path`, D-187..D-191), tested by `test_server.py::test_cover_override_replace_remove_and_refuse`, `test_media.py` and `studio-cover.test.tsx` (local CI: Studio CPU 229 passed, 1 skipped, UI 15 files, 170 passed). **Independent check, 18 Nov:** a real GPU job submitted through the release app's `studio_submit` with `metadata.cover_path` set to a 256x256 PNG produced a song whose cover chunk is that file byte for byte (SHA-256 8ec387f1…, `image/png`, `metadata_source.cover` "user", `verify --strict` PASS); `studio_cover_preview` previewed the PNG and refused `settings.json`. **18 Nov, through the form in the real app:** "Choose a cover picture…" with the dialog's answer supplied at the IPC boundary (the dialog itself cannot be automated); thumbnail shown, and the Clean song's cover chunk is the 20,121-byte JPEG byte for byte (TEST_RESULTS section 8). The native dialog itself was not driven |
| Separate lead, backing, drums, other; default mix = everything but the lead | DONE+VERIFIED | D-114; separation.md; `in_karaoke_mix` (SONG_FORMAT 4) |
| Lyrics order: user text/.lrc, embedded, online lookup on request, transcription | DONE+VERIFIED (offline sources) / BUILT-NOT-VERIFIED (online) | `lyrics.resolve_lyrics` docstring: auto = lrc, text, embedded, transcribe; the lookup is a button only (`Studio.tsx:278`). The real LRCLIB endpoint was never called: `test_lyrics_sources.py:198` skips without `JK_NETWORK_TESTS` |
| Always show the text; label transcribed lyrics unchecked | DONE+VERIFIED | e2e: saved `transcribed`, `checked: false`, and the Stage shows a notice (end-to-end.md 5.2); dist proof song `transcribed`, unchecked |
| Align every line, word and letter | DONE+VERIFIED (words) / tests only (letters) | alignment.md: median 0.036 s, 94.2 % within 0.3 s. No dataset has letter timings: letters are checked by synthetic tests (`test_align_ctc.py`) |
| Key and tempo analysis with key names | DONE+VERIFIED, with limits | analysis.md: exact key 66.7 % on synthetic material, 59-80 % on GuitarSet; full-band songs not measured; the key is shown as a suggestion (D-55) |
| Write the song file | DONE+VERIFIED | `jukeoke-cli verify --strict`: PASS with 0 errors and 0 warnings for the 3 e2e songs and the dist proof song |
| No ghost singer, no dull cymbals or hollow mids, backing kept, stereo image and loudness unchanged | **PARTIAL** | Measured: mean lead residue -15.9 dB, band energy within 0.4 dB at 8-16 kHz (separation.md 5). On a hard song the residue is -6.0 dB with backing on (end-to-end.md 5.3). **18 Nov, stereo image and loudness measured** (separation.md 6.9): on MUSDB18-HQ (50 tracks, all vocals removed) every karaoke mix is within 0.3 LU of the true instrumental's integrated loudness, inter-channel correlation within 0.013 and side-to-mid ratio within 0.8 dB; on the 18 ccMixter mixes with the default preset the median loudness difference is -0.05 LU, correlation within 0.015 on every track and side-to-mid within 0.5 dB on 13 of 15 stereo tracks, worst -2.99 LU / +1.04 dB on tracks whose lead/backing split is hard. Ears not yet used; stereo image not on the listening checklist |
| Measure "full" and "clean", pick a default, offer the other | DONE+VERIFIED | Karaoke-mix SDR 18.10 vs 16.24 dB, residue -15.9 vs -14.8 dB (D-115). "clean" is a preset; its model publishes no licence (MODELS.md). **18 Nov:** the form's preset hints now say what was measured (D-210; the Full hint corrected in D-215, TEST_RESULTS 8.1), and choosing Clean or Permissive offers that preset's missing model with its size and licence ("licence: none published; commercial use unclear" for Clean) and downloads it on request (D-209); proved in the release app: both downloads from the pinned URLs with SHA-256 checked, one Clean and one Permissive song, `verify --strict` PASS (TEST_RESULTS section 8) |
| Speed: a few minutes for 4 minutes, never trading quality | DONE+VERIFIED | 174.6 / 175.1 s through the app; 188.9 s re-measured by the verifier (end-to-end.md 4), in fp32 |
| Sync editor: global offset, nudge, drag, tap-along, text correction; Studio realign; manual tools on the Mac | DONE+VERIFIED (Windows UI tests) / PARTIAL | Every tool is in `SyncEditor.tsx`, covered by `ui.test.tsx`; realign is covered by protocol and Studio tests (D-154, D-164). **18 Nov, real app:** one word changed, "Re-align with Studio", "Save into the song": the song's slot holds `lyrics_override` with the new word timed (TEST_RESULTS section 8). Mac not run |

## 3. Section 4.2: the song file

| Requirement | Status | Evidence |
|---|---|---|
| One self-contained file (`.jksong`), safe on USB, AirDrop, cloud and email | DONE+VERIFIED (format) | Custom chunked container with CRCs (D-21, SONG_FORMAT); conformance suite 92 checks PASS (Python writer against Rust reader and writer, damaged files). AirDrop and cloud transfer not tried (single file by construction) |
| Stems lossless | DONE+VERIFIED | 24-bit FLAC with a stored gain (D-22); FLAC decoded back with MD5 checks; float model output is quantised to 24 bit (about -144 dBFS), which section 5 allows for integer formats |
| Lyrics with line, word and letter timings | DONE+VERIFIED | SONG_FORMAT `chars` arrays; `jk-format` `Grapheme` |
| Title, artist, duration, cover, key, tempo | DONE+VERIFIED | SONG_FORMAT 4.1; e2e songs carry key and tempo (end-to-end.md 5) |
| Singer's settings (key shift, mix, offset) travel with the file | DONE+VERIFIED (Windows) | A/B settings slots (D-21, SONG_FORMAT `semitones`, `guide_vocal`, `lyrics_offset_ms`); app writes them debounced (D-80). Reading them on a Mac has never run |
| Provenance: models and versions, source hash, app version | DONE+VERIFIED | SONG_FORMAT 4.3; the verifier found the model hashes equal to the bytes on disk (PROGRESS, verifier run) |
| Format version; unknown fields ignored | DONE+VERIFIED | `format_version` "1.0"; unknown keys kept at every level (D-46); unknown stem roles routed (D-62, playback.md 5) |
| Spec in `docs/SONG_FORMAT.md` | DONE | 295 lines; two independent implementations (D-26) |

## 4. Section 4.3: player

| Requirement | Status | Evidence |
|---|---|---|
| Searchable, sortable library with cover art | DONE+VERIFIED (Windows) | `Library.tsx` (search, 4 sorts, covers); UI tests; library screenshot |
| Play, pause, seek, restart | DONE+VERIFIED | Seeks sample-exact, 40-50 ms (playback.md 4) |
| Mixer: backing on/off and level, guide 0-100 % (default off), master | DONE+VERIFIED | `Mixer.tsx`; `guide_vocal: 0.0` default (SONG_FORMAT 253); engine tests |
| Output device picker | DONE+VERIFIED (Windows) | `Mixer.tsx:46`; `device_switch` example: every switch OK, 0 underruns (playback.md 7) |
| Full-screen stage, keyboard shortcuts | DONE (Windows) | `Stage.tsx` full-screen mode; D-100; UI tests |
| Per-song settings remembered | DONE+VERIFIED (Windows) | D-80; core tests |
| Lyrics timing offset (Bluetooth) | DONE / BUILT-NOT-VERIFIED (Bluetooth) | Per-song and per-device offsets (`SyncEditor.tsx:76,90`, D-91); no Bluetooth device was tested (HANDOFF section 3) |
| Export backing track, lossless | DONE+VERIFIED | Wav32f bit-identical to the render; Flac24 within half a 24-bit LSB; gain lowered only when it would clip, and reported (playback.md 6, D-67) |

## 5. Section 4.4: lyrics ticker

| Requirement | Status | Evidence |
|---|---|---|
| One large line moving right to left; marker fixed about a third in | DONE+VERIFIED | D-04, D-94; default marker 0.33 (D-91); screenshots |
| Sung letters change colour; upcoming letters readable at least 2-3 s ahead | **PARTIAL** | At least 2.0 s is reached on every test song only by shrinking dense lines down to 0.45x (25.2 px at 1440x900; D-167). D-167 states that songs denser than Crowdpleaser fall below 2 s again (15 of 18 generated rap variants pass). The work order asks for 2-3 s; 3 s is never the floor. Whether 0.45x is readable needs the owner's eyes. The 0.45x result for Crowdpleaser comes from a width table in vitest, not from a live real-app run after the fix (the live run before the fix measured 1.46 s, end-to-end.md 6) |
| Continuous motion; the marker moves through a held letter | DONE+VERIFIED | Steffen monotone cubic through every cell edge (D-95); 0 backwards and 0 too-fast samples |
| Breaks: countdown, next line enters early enough | DONE+VERIFIED | Countdown for rests of 4 s or more, entry 3 s ahead (D-96, `layout.ts:73`) |
| Clock = audio that reached the device, never a timer | DONE+VERIFIED | Span log minus device latency (D-59); clock -20.92 ppm against the device's -20.82 ppm over 20 minutes (playback.md 2); UI re-anchors on every clock event (D-93) |
| Settings: text size, marker position, colours, offset | DONE | `Settings.tsx:54-69`, offsets in `SyncEditor.tsx` |
| Variable, smoothed speed without the marker leaving the letter | DONE+VERIFIED | 0 wrong letters at 1280x800, 1440x900, 2560x1440 and 3840x2160 (`ticker.test.ts:166-169`) and in every real-app run (`real-app-screens.json`, dist proof step 7) |
| Letter = grapheme cluster | DONE (English verified) | `jk-format/src/lyrics.rs:3`; `align.py` grapheme mapping; other scripts not measured |

## 6. Section 4.5: key change

| Requirement | Status | Evidence |
|---|---|---|
| At least ±12 semitones, tempo unchanged | DONE+VERIFIED | 25 steps x 4 tones, max 0.0015 cents, exact length (dsp.md 1) |
| Original and resulting key by name ("A major → B major (+2)") | DONE+VERIFIED | `describe_shift` test (analysis.md); `docs/screenshots/key-dial.png` shows "C major → D♭ major (+1)" |
| No stop, no click when changing during playback | DONE+VERIFIED | 40 ms equal-power crossfade; largest step 1.31x and largest curvature 1.26x the signal's own (dsp.md 4); heard about 280 ms after the click (D-34) |
| Key saved with the song and carried to the Mac | DONE (Windows) / BUILT-NOT-VERIFIED (Mac) | Settings slot `semitones` |
| Best-sounding algorithm | DONE | Rubber Band R3 (OptionEngineFiner), HighQuality pitch (D-30). Its internal resampler is "FastestTolerable" (about 90 dB) and cannot be changed without patching (D-35) |
| Drums not shifted by default, with a setting | DONE+VERIFIED | Drum path offset 0 on every impulse (dsp.md 2); "Shift drums with the key" toggle (`Mixer.tsx:132`) |
| Windows and Mac give the same result, same DSP code, proven | **BUILT-NOT-VERIFIED** | Same code (built-in FFT and resampler forced, D-30); golden files and a 1e-5 tolerance defined (dsp.md 6); Windows rerun bit-identical (18/18). The macOS half has never run |
| If real-time shifting would glitch, render ahead and cache instead | DONE+VERIFIED (Windows) | **18 Nov:** `audio.key_mode` auto/realtime/prerender (D-176..D-186). `tests/keycache.rs`: prerendered playback bit-identical to the real-time path, seeks into cached and unrendered parts, reuse across sessions, and a simulated too-slow computer (renderer 0.71x) plays with 0 underruns where the real-time path underruns; `tests/rt_alloc.rs`: the worker does not allocate in prerender mode (playback.md 11). Both in the local CI Rust suite (194 passed, 0 failed in 34 binaries). **Independent check, 18 Nov** (logs in `$JK_BUILD/verify-keycache`): the release app in prerender mode on the null device with the real 4-minute song showed "Preparing key +2… 0%" and switched after 1.1 s, seeks kept the key, 0 underruns and 0 cache misses, cache files only in `<root>/Settings/cache/keys`; on the engine the switch is the key-0 path bit for bit, then a 40 ms equal-power crossfade (exact to the sample), then the real-time path's own output for the same start cell bit for bit (96,000 frames); a key change after the end of the song now prepares from the restart point (fixed). Open: the cap is exceeded by up to 5 entries of the loaded song (462 MB on a 200 MB cap) and one unexplained cache miss in 10 full runs of `tests/keycache.rs`. **18 Nov:** the progress shown no longer steps back (the UI keeps the highest figure per key change, D-211, `key-prepare.test.tsx`; after a seek it follows the core's restart, D-215, checked in the release app, TEST_RESULTS 8.1); the core's own figure can still dip by about 1 %. Not run: the M1's Rubber Band factor (the self-check now measures it) |

## 7. Section 4.6: recorder

| Requirement | Status | Evidence |
|---|---|---|
| Device's own rate and format, no resampling, no processing | DONE+VERIFIED (Windows) | 48000 Hz = 48000 Hz on every run; packet to file bit-exact (recorder.md summary). Captures the shared mix format (32-bit float); the device's 24-bit hardware format fits inside it exactly |
| Lossless (FLAC or float WAV) into `Recordings/` | DONE+VERIFIED | Float WAV, RF64 above 4 GiB (D-45, D-90); self-check capture |
| Level meter, elapsed time, capture format, clip warning | DONE | `Recorder.tsx:19-145`; core statistics in the self-check report |
| "Make a karaoke song from this" | DONE+VERIFIED (Windows) | `Recorder.tsx:173`; clicked in the release app on 18 Nov, the recording's job finished (TEST_RESULTS section 8) |
| No extra drivers or installs | DONE+VERIFIED | WASAPI loopback; exe imports only Windows DLLs (TEST_RESULTS section 2) |
| Honest about the ceiling, in the UI and the docs | DONE (tests) | Ceiling stated (`Recorder.tsx`, `windows.rs`). **18 Nov:** a platform sentence before recording (Windows: "Audio enhancements" are in the capture, turn them off for an exact copy; Mac: the ScreenCaptureKit format) and in the README (D-193); `recorder-honesty.test.tsx` in the UI suite (15 files, 170 passed). Bit-identity with enhancements off is still unverified (changing system sound settings is not allowed) |
| Bit-transparency reported | DONE+VERIFIED | Not bit-identical on this PC (max diff 7.98e-4 on a 9.77e-4 signal; residual about -125 dBFS RMS after a linear fit); cause measured |
| Trap 5 handled (silence) | DONE+VERIFIED | §8 trap 5 below |
| macOS: ScreenCaptureKit, one-time permission, works on macOS 13 | BUILT-NOT-VERIFIED | `jk-recorder/src/platform/macos/mod.rs` (`available!(macos = 13.0)`); its object files show minos 13.0 (MAC.md table); never linked or run |

## 8. Section 4.7: the jukebox look

| Requirement | Status | Evidence |
|---|---|---|
| Arched crest, glowing columns, console with grille, medallion and transport | DONE (Windows) | `skin/JukeboxFrame.tsx`, `Grille.tsx`, `Medallion.tsx`, `CrestOrnament.tsx`; screenshots |
| Light slowly drifting through the palette | DONE | D-104 (one cycle per 40 s, 12 Hz steps) |
| Chrome, walnut, red enamel | DONE | D-105, `wood.ts`, `frame-art.ts` |
| Library as title-strip cards | DONE | `TitleStripCard.tsx`; `docs/screenshots/library.png` |
| Physical-looking controls; key control prominent | DONE | `KeyDial.tsx` (about 104 px at 1440x900, D-103), `PlateButton.tsx`, `Fader.tsx` |
| Bubbles in tubes round the frame; can be switched off; follows reduce-motion | DONE+VERIFIED (Chromium) | D-107; `App.tsx:124` `prefers-reduced-motion`; skin README measurements |
| Original badge, no brand | DONE+VERIFIED | `Badge.tsx`; brand grep: no hits |
| Drawn in code; photos not shipped or committed | DONE+VERIFIED | git history scan (section 0); `reference-images/` outside the repo and in `.gitignore` |
| Scales from a 13-inch MacBook Air to 4K | DONE (WebView2) / BUILT-NOT-VERIFIED (WKWebView) | Real-app captures at 1280x800, 1440x900 and 3840x2160 (`$JK_BUILD/screenshots/app/`) |
| Low idle CPU/GPU, capped frame rate, pause when hidden | DONE (WebView2, unit-tested) / BUILT-NOT-VERIFIED (WKWebView) | Chromium: bubbles + drift cost 1.8 % main / 14.8 % tree (skin README); ticker capped at 60 fps, bubbles at 30 (60.000 and 29.3 fps over 20 minutes in the real app, playback.md 12). **18 Nov:** the ticker's frame loop also stops while the native window is minimised (D-192), tested by `frameloop.test.ts` (UI suite 15 files, 170 passed); not measured with a real minimised window. Not measured in WKWebView |
| Animation never causes an audio glitch | DONE+VERIFIED (Windows) | **18 Nov:** 20 minutes in the real app with skin, bubbles and ticker on, key changes every 30 s and 14 busy threads: 0 underruns, 0 device starved, render lead never below 140 ms, ticker 60.000 fps with 0 frames missing, 0 display frames over 25 ms (playback.md 12). Mac not run |

## 9. Section 4.8: portability

| Requirement | Status | Evidence |
|---|---|---|
| Layout `Windows/ Mac/ Library/ Recordings/ Models/ Settings/` | **PARTIAL** | `dist/` has all of them plus `Licenses/` and START-HERE.txt; `dist/Mac/` holds only README.txt: there is no Mac app |
| No installer, no admin rights | DONE+VERIFIED | Double-click exe; dist proof from a copy |
| Portable mode writes nothing to the registry or per-user folders | DONE+VERIFIED, with stated exceptions | dist proof step 8: 0 entries from the app or the Studio. Windows itself writes MuiCache and audio PolicyConfig for any program (D-41, D-88). Shader-cache writes could not be attributed (end-to-end.md 9). On a Mac, the translocated case writes `~/Library/Application Support/org.jukeoke.app/root.txt` (D-87); WKWebView's own caches unverified |
| Launches on a clean Windows 11 PC | BUILT-NOT-VERIFIED | Exe imports 21 Windows DLLs only; the Studio has its own VC runtime. No clean PC or clean runner was tried (TEST_RESULTS section 4) |
| Nothing depends on an absolute path or the drive letter | DONE+VERIFIED | dist copy under `subst Q:` with `dist/` renamed away (TEST_RESULTS section 3); portable smoke test from `J:` |
| Size: apps + models under 12 GB, ideally under 9 GB; remaining space shown | DONE+VERIFIED (Windows half) | 8.633 GB on exFAT with 32 KB clusters, 10.540 GB with 128 KB clusters (TEST_RESULTS section 2; the byte count is re-checked in section 0). **The Mac app is not included and has never been measured.** Free space shown (`Library.tsx:269`) |
| Works when copied to an internal drive | DONE+VERIFIED | The dist proof ran from a copy on the internal NVMe drive |
| No dropouts from a slow stick | BUILT-NOT-VERIFIED | 4 s stem ring (ARCHITECTURE 4). No slow stick and no exFAT stick tested. The first Studio job on a real stick re-hashes about 3.8 GB of models; that time is unmeasured (TEST_RESULTS section 3) |

## 10. Section 4.9: weaker computers

| Requirement | Status | Evidence |
|---|---|---|
| Detect capability; without a GPU, memory or Studio, the app opens, plays and explains; never crashes or hangs | DONE+VERIFIED | GPU hidden: reason after 0.87 s; Studio removed: 0.59 s; hanging Python: 64.6 s, then killed; crashing Python: 0.61 s; each case plays and refuses submit in 4 ms (end-to-end.md 7). The "not enough memory" branch (11264 MiB threshold, D-142) and a real weak PC were not exercised |
| Mac: player, key change, manual sync tools, recorder; no Studio | BUILT-NOT-VERIFIED | Shared UI and engine code; type-checked only |

## 11. Section 4.10: the owner's Mac

| Requirement | Status | Evidence |
|---|---|---|
| Apple Silicon, macOS 13.0 minimum | BUILT-NOT-VERIFIED | `build-mac.sh:67` sets `MACOSX_DEPLOYMENT_TARGET`; `tauri.conf.json` `minimumSystemVersion` "13.0" |
| Recorder through ScreenCaptureKit on macOS 13 | BUILT-NOT-VERIFIED | §7 last row |
| SCK details: tiny discarded video, capture rate that avoids a conversion, real format shown, docs on the conversion-free output setting, permission wording explained in the app | BUILT-NOT-VERIFIED | D-45 (2x2 px at 1 fps; device rate if SCK supports it, else 48 kHz); MAC.md 3 and README.md:84 (set the output to 48,000 Hz); `Recorder.tsx:39-68` explains "Screen Recording" before the system prompt |
| WebKit baseline Safari 16.4, checked, with fallbacks; no WebGPU | PARTIAL | Baseline researched (D-44: floor on 13.6.9 is 16.6). `npm run build` fails unless the JS parses as ES2022 and the CSS passes a caniuse check against Safari 16.4; APIs only by a deny list ("a heuristic, not a proof", `check-dist-syntax.mjs`). Never run in Safari or WKWebView 16.x |
| Under about 1 GB for all processes while playing with bubbles | **PARTIAL: measured on Windows only** | **18 Nov, Windows/WebView2** (playback.md 12): Jukeoke.exe plus its 7 WebView2 processes held 551 MB resident (working set) as a median and 605 MB at most; private bytes 488 MB median in steady state, but 1.12 GB for about 4 s at start-up and 0.97 GB in the last sample, both in the WebView2 GPU process; Jukeoke.exe itself at most 70 MB. The Studio's idle probe process (775 MB until its 10-minute idle shutdown) is not counted: the Mac has no Studio. Nothing measured on a Mac (WKWebView and unified memory differ) |
| Layout for 1440x900, checked at 1280x800 | PARTIAL | WebView2 captures at both sizes exist; none in WKWebView |
| 20 minutes of key change + ticker + bubbles without dropouts on a throttling machine | PARTIAL | **18 Nov:** done in the real app on Windows (0 underruns, 0 device starved, render lead never below 140 ms, ticker 60.000 fps with 0 frames missing, 0 display frames over 25 ms, playback.md 12). The throttling fanless Mac is untested (MacBook checklist item 7) |
| First-launch steps with the exact wording | BUILT-NOT-VERIFIED | START-HERE.txt section 4 and HANDOFF section 5 quote the expected dialog; the wording comes from research and has not been seen on Ventura |
| Use the oldest Apple Silicon runner | DONE (configured) / never run | `runs-on: macos-15`, since macos-14 was retired on 2026-11-02 (D-43, MAC.md 1) |
| Deployment target 13.0 for everything compiled | BUILT-NOT-VERIFIED | As above; the Rubber Band C++ has never been compiled for macOS |
| Inspect every binary and bundled library for minos 13.0 or lower | BUILT-NOT-VERIFIED | `scripts/check-macos-minos.sh`, exercised only against mocked `vtool`/`otool` output (MAC.md table) |
| Every API newer than 13.0 behind a runtime availability check | **PARTIAL** | The recorder's own code checks `available!(macos = 13.0)`. Nothing audits the imported symbols or Objective-C selectors of Tauri, wry, cpal or objc2. D-37 says "The final Mac binary's imported symbols still need checking in CI"; no such check exists (section 0) |
| Self-check: test tone, render the bundled song, key shift, recorder permission + 3 s capture, macOS/WebKit version, memory, output format; plain-text report in Settings; actionable failure messages | DONE+VERIFIED (Windows) / BUILT-NOT-VERIFIED (Mac) | Windows dist proof: PASSED, 8 of 8 items (section 0). The macOS paths (`webkit_version` from NSBundle, the Screen Recording permission branch at `selfcheck.rs:364-387`) are type-checked only |

## 12. Section 5: audio quality rules

| Rule | Status | Evidence |
|---|---|---|
| No lossy encoding anywhere (stems, caches, recordings, exports) | DONE+VERIFIED | Stems 24-bit FLAC; recordings float WAV; exports Wav32f/Flac24; MP3 sources decoded once (D-143). Covers that are not JPEG/PNG are re-encoded to JPEG (D-144): artwork, not audio |
| 32-bit float throughout | DONE+VERIFIED | D-143; the jk-dsp graph runs in f32 |
| Resample at most once, top quality, only where needed, and record it | **PARTIAL (documented deviation)** | 48 kHz sources: the vocal, lead and drum estimates are converted 48000 to 44100 into the models and 44100 to 48000 back. Provenance lists "48000->44100 once, 44100->48000 three times" (D-150, end-to-end.md 5), so those stems have been resampled **twice**. A device whose rate differs from the song adds a third conversion (rubato, THD+N -133.8 dB, dsp.md 7). Rubber Band's internal resampler is "FastestTolerable" (about 90 dB stop band) whenever the key is not 0 (D-35). Every place is recorded (dsp.md 7) |
| No hidden processing (normalisation, limiting, EQ) | DONE+VERIFIED, one visible exception | Master 1.0 by default (D-91); no limiter, clip is reported (D-66). Flac24 export lowers the gain automatically when the mix would clip and reports `applied_gain_db` in the UI (D-67); Wav32f is never scaled |
| Stems can exceed full scale: headroom, stored gain | DONE+VERIFIED | 26 of 32 stems with a stored gain sum back within 1.3e-7 (separation.md summary) |
| Output at the device rate; exclusive mode option on Windows | DONE+VERIFIED | Shared mode at the mix format; exclusive mode at 44,100 Hz with no resampling (playback.md 7); toggle `Mixer.tsx:57` |
| Full precision for separation unless proven identical | DONE+VERIFIED | fp32 with TF32 off; fp16 differs by -65 dB, TF32 by -70 dB, so neither is used (D-116) |

## 13. Section 7: the 14 known traps

| # | Trap | Status | Evidence |
|---|---|---|---|
| 1 | RTX 5070 Ti needs a recent PyTorch | DONE+VERIFIED | torch 2.14.1+cu130, matmul and STFT on sm_120 (D-12); dist proof: CUDA matmul relative error 7.8e-7 |
| 2 | Unsigned app on macOS: ad-hoc signature, translocation, first-launch docs, permissions asked again | BUILT-NOT-VERIFIED | `build-mac.sh:228-233` codesign `-`; translocation handling D-87; MAC.md 6-8; HANDOFF section 7 item 6 warns about re-asked permissions |
| 3 | exFAT: no symlinks, `._` files, CI launch from an exFAT image | PARTIAL | `._*` skipped (tests `format.rs:706`, `core_api.rs:53`). `scripts/ci-exfat-launch.sh` was never run, and it **skips with exit 0 when no app bundle exists** (lines 45-47) |
| 4 | Relocatable Python | DONE+VERIFIED | dist proof from `Q:` with the original renamed away; runtime check 26/0; 0 files written to the sentinel profile |
| 5 | Loopback is silent during silence | DONE+VERIFIED | 10 s started and stopped in silence: 0 packets, file still 10.146 s (-0.1 frames); 20 s with 8 s of silence: -18.8 / -74.8 frames (recorder.md) |
| 6 | Pitch-shifter latency vs unshifted drums, impulse test | DONE+VERIFIED (with a physical limit) | Drum path offset 0 on every impulse; shifted band vs drums mean at most 13.6 samples (0.31 ms); single impulses jitter up to ±2 ms at -12 (dsp.md 2) |
| 7 | Lyrics drift if driven by a timer | DONE+VERIFIED | Device-frame clock (D-59): -20.92 ppm vs device -20.82 ppm |
| 8 | Aligners lost in sung music | DONE+VERIFIED | Isolated vocal (full mix: mean error 3.5 s instead of 0.21 s); LRC anchors used as windows; held-note extension (D-51 to D-53) |
| 9 | Ligatures off, real glyph positions | DONE+VERIFIED | D-94 (`liga`/`calt` off, per-grapheme measurement); 0 wrong letters |
| 10 | Model licences recorded | DONE | MODELS.md; D-114, D-149, D-50 |
| 11 | No audio in the web view, no allocation or locks in the real-time path, dropout test with bubbles and CPU load | DONE+VERIFIED (Windows) | No audio in the web view (ARCHITECTURE 6). **18 Nov:** `tests/rt_alloc.rs` counts Rust and C/C++ allocations per thread: 0 on the render worker and the device thread (it found and led to a fix of two Rubber Band allocations, D-202; playback.md 11). Locks: none by design, not measurable by the test. Real app, 20 minutes, bubbles + ticker + key changes + 14 busy threads: 0 underruns, 0 device starved, render lead never below 140 ms, ticker 60.000 fps with 0 frames missing, 0 display frames over 25 ms (playback.md 12) |
| 12 | 8 GB Mac: do not hold stems in memory | DONE (Windows) / BUILT-NOT-VERIFIED (Mac) | Streaming: 49.8 MB peak for a 4-minute song vs 339 MB if decoded (playback.md 3) |
| 13 | Atomic writes of songs and settings | DONE+VERIFIED | D-81 (`fs_atomic.rs`); Studio writes to a temp file and renames (D-147); settings slots A/B (D-21) |
| 14 | macOS 13 vs CI's newer macOS | **PARTIAL** | Minimum-OS check script (never run on a real bundle). No imported-API audit (§11 row "API newer than 13.0"). CI runs `--smoke-test`, not the self-check (§16) |

## 14. Section 9: evidence rows

| Area | Status | Evidence / what is missing |
|---|---|---|
| Separation quality (SDR vs known stems, candidates compared, residual lead measured) | DONE+VERIFIED | 18 candidates on MUSDB18-HQ test (a licensed evaluation set) and 18 CC ccMixter mixes; karaoke mix 18.10 dB, residue -15.9 dB (separation.md) |
| Nothing lost (fullest preset sums back within quantisation) | DONE+VERIFIED | Bit-exact for 24-bit sources; 5.96e-8 to 1.3e-7 otherwise; the pipeline refuses a song that misses the source by more than 2^-22 (D-146) |
| Key change accuracy (each semitone; within cents; duration; no clipping) | DONE+VERIFIED (Windows) | dsp.md 1; also on real songs at 0/+2/-5 (end-to-end.md 5.1) |
| Drum alignment (impulse through both paths, same sample) | PARTIAL (physical limit, documented) | Unshifted path exact. The shifted band lands on the same sample only on average (within 0.31 ms); a phase vocoder cannot be exact (D-32). Flam at ±5/±12 is on the listening checklist |
| Same audio on both systems (offline render Windows vs macOS runner, float tolerance) | **NOT DONE (blocked: no macOS runner)** | Only Windows vs Windows: 18/18 bit-identical. Also, `ci-compare-renders.sh` lines 19-23 **exit 0 with a notice when the macOS renders are missing**, so CI could report this as passed without comparing anything (§16) |
| Lyric alignment (JamendoLyrics word onsets, median and worst) | DONE+VERIFIED | Median 0.036 s; worst word 54.4 s with text only, 25.5 s with line anchors (alignment.md: the mean and worst are set by a few songs, section 6 there); real-app runs: worst 0.20 s |
| Ticker (letter under the marker over whole songs at several sizes; 13-inch and 4K screenshots) | DONE+VERIFIED (WebView2) | 4 sizes in vitest; real-app 0 wrong letters; captures at 1440x900 and 3840x2160 |
| Recorder (rate, sample count across silence, no dropouts, bit-transparency) | DONE+VERIFIED (Windows) / BUILT-NOT-VERIFIED (Mac) | recorder.md summary table |
| Portability (other drive letter; Mac from exFAT in CI; nothing written outside; size vs budget) | PARTIAL | Drive letter, the audit and size are done on Windows. The Mac exFAT launch has never run |
| The owner's Mac (minos of every binary, no unguarded newer API, memory vs 1 GB, screenshots at 1440x900 and 1280x800, self-check on the oldest runner) | **NOT DONE (blocked)** | Nothing ran. Additionally, CI's exFAT step, labelled "(self-check)", runs `--smoke-test` (`ci-exfat-launch.sh:38`, default of `JK_SELFCHECK_ARGS`). MAC.md section 9 still says to switch it "once a headless self-check exists", and `--selfcheck` now exists. So even a green CI run would not run the self-check |
| Weak computers (GPU hidden, Studio removed) | DONE+VERIFIED | end-to-end.md 7 |
| Smooth playback (long run, bubbles on, CPU load, 0 underruns) | DONE+VERIFIED (Windows) | **18 Nov:** the real app, 20 minutes, skin + bubbles + ticker, key change every 30 s, 14 busy threads, default device at digital silence: 0 underruns, 0 device starved, render lead never below 140 ms, ticker 60.000 fps with 0 frames missing, 0 display frames over 25 ms (playback.md 12) |
| End to end (every file in test-songs/ on Windows; songs load and render on the macOS runner) | PARTIAL | `test-songs/` is empty, so CC songs stood in: 3 songs plus a corrupt file through the real app (end-to-end.md). The macOS half has not run |
| Listening checklist (ghost vocals, dull cymbals, key ±2 and ±5, drums natural vs shifted, presets side by side) | DONE | HANDOFF section 6, items 1-8 cover each. Stereo image is not listed |
| MacBook checklist (first launch, self-check report, play from stick, key change, Bluetooth offset, Screen Recording prompt, 20-minute battery and heat) | DONE | HANDOFF section 7, items 1-8 |

## 15. Sections 10, 11 and 13

| Requirement | Status | Evidence |
|---|---|---|
| §10 GPL-3.0-or-later | DONE | `LICENSE` (GPL v3 text); `license = "GPL-3.0-or-later"` in Cargo.toml, pyproject.toml, package.json |
| §10 `THIRD_PARTY_LICENSES.md` lists every dependency | DONE | 289 lines; regenerated for the dist runtime and identical apart from the build time (TEST_RESULTS section 2) |
| §10 `MODELS.md`: source, licence, commercial use; permissive alternative | DONE | MODELS.md; the default preset uses only commercial-use licences; `permissive` preset (MIT + Apache-2.0, no backing split; D-149) |
| §10 Weights downloaded on first use, never committed or bundled | DONE (repo, release) / deviation (dist) | `.gitignore` excludes Models; `release.yml` packages no models. **`dist/Models` bundles 3.8 GB of weights** (the default set) for the owner's own stick, a recorded exception (D-212); `release.yml` re-checked on 18 Nov: it stages no weights and no Studio. The Clean and Permissive weights are not bundled and are downloaded only through the Studio form (D-209) |
| §10 No copyrighted music, lyrics or photos in the repo or its history | DONE+VERIFIED | Section 0 scan; screenshots attributed (CC BY and public domain) |
| §10 No brand names or logos | DONE+VERIFIED | Section 0 grep |
| §10 No telemetry or accounts; network only on request | DONE | Lookup is a button; downloads only through `models.download` (D-151); `HF_HUB_OFFLINE=1` (D-152); an HTTP proxy trap saw 0 connections during a GPU run (PROGRESS, verifier). **18 Nov:** the app's own download path was exercised against the real endpoints for the Clean and Permissive weights, only after the button in the Studio form (TEST_RESULTS section 8) |
| §10 README (what, run from a stick, build) and a contributor guide | DONE | README.md, CONTRIBUTING.md |
| §10 Release workflow for both platforms, ready, not triggered | PARTIAL | `release.yml`: `workflow_dispatch` only, read-only token. Never run. It ships the Windows player and the Mac app **without the Studio runtime** |
| §10 "Making it public is a single switch" | **PARTIAL** | Not yet: the Studio runtime puts NVIDIA's CUDA libraries in the same process as GPL code (our Studio, mutagen, a GPL FFmpeg build). Distributing that to others is unresolved (HANDOFF section 8 item 2; THIRD_PARTY_LICENSES section 1). Git history contains the build PC's user name (HANDOFF section 8 item 4) |
| §10 Repository private | DONE | Local git only (D-09) |
| §11 #1 Name in one place | PARTIAL | `branding.json` is the source, but its `also_edit` list names **18 files** that spell the name as a literal (D-175) |
| §11 #2-#9 defaults | DONE | D-02 to D-09; guide vocal 0.0 by default; lookup per song only; marker fixed (D-167 refused to move it) |
| §13 HANDOFF.md (state, verified numbers, built-not-verified incl. every Mac item, not done, USB steps, both checklists, owner items) | **PARTIAL** | All sections exist, but: **unfilled placeholders `[[DIST_SIZE]]` (line 51) and `[[TESTS]]` (line 54)**. Line 37 gives "vocals SDR 15.1 / 11.2 dB on MUSDB18-HQ test excerpts", which is a 2-excerpt determinism check (separation.md line 655); the benchmark mean is **13.92 dB (25 tracks) / 13.28 dB (50)** |
| §13 DECISIONS.md and PROGRESS.md up to date | **PARTIAL** | PROGRESS "In flight" still lists the Stage 7 workflow. "Next" still lists done work (e.g. "THIRD_PARTY_LICENSES.md must be created"). DECISIONS now records the bundled weights (D-212) and the 32 KB clusters (D-213); path remapping and `build-mac.sh` are in D-199 and TEST_RESULTS 5.1, not as a packaging decision. Superseded "open" notes remain in D-83 (caches, fixed by D-153) and D-109 (THIRD_PARTY_LICENSES missing) |
| §13 Ready-to-copy `dist/` laid out as 4.8 | PARTIAL | Present and proven from a copy; no Mac app |
| §13 All tests passing in CI, or a clear list of what fails | PARTIAL | CI never ran (no GitHub). The local equivalent passes (section 0), with 13 clippy warnings. TEST_RESULTS section 6 lists clearly what only macOS CI would run |

Section 3 (boundaries), briefly: project in a subfolder, `reference-images/` and
`test-songs/` outside git (DONE); no system settings changed (recorder.md: "no Windows
setting was changed"); nothing published (DONE); test material is CC, generated or a
licensed evaluation set (MUSDB18-HQ, "educational purposes only", used for §9 as allowed);
no `gh` login, so local git plus workflows plus `build-mac.sh` (DONE, D-09).

## 16. Where the repository contradicts or weakens the work order

1. **CI could pass without proving §9 Mac items:**
   - `scripts/ci-compare-renders.sh` exits 0 when the macOS renders are missing. The job
     is named "Same audio on Windows and macOS".
   - `scripts/ci-exfat-launch.sh` exits 0 when there is no app bundle.
   - The exFAT step labelled "(self-check)" runs `--smoke-test`, not `--selfcheck`.
   - `scripts/ci-sck-probe.sh` passes on "environment limit" codes 2, 3 and 5.

   These skips date from scaffolding, when parts of the project did not exist yet. Now
   that everything exists, a green CI run would hide those gaps.
2. **4.5, "render ahead and cache"** if real time would glitch: not implemented, and not
   listed as not done in HANDOFF section 4.
3. **4.4 lookahead "two to three seconds":** met at 2.0 s only by shrinking text to 0.45x,
   and D-167 admits it fails for denser songs. HANDOFF says "at least 2.0 s ahead on every
   test song", which is true but leaves the limit out.
4. **§5, "resample at most once":** 48 kHz sources are resampled twice per model stem
   (D-150). Rubber Band's internal resampler is not "top-quality" (D-35). Both are
   documented.
5. **§9, drum alignment "lands on the same sample":** true for the unshifted path only;
   the band is within 0.31 ms on average (documented physical limit).
6. **4.7, "pause animation when the window is hidden":** the ticker draws while minimised
   on WebView2 (D-97 open).
7. **§10 "never bundled":** `dist/Models` bundles the default weights for the owner's
   stick; recorded as a deliberate exception in D-212 (18 Nov). Releases ship none.
8. **4.6, honesty about the capture:** the Windows "Audio enhancements" effect on
   loopback is measured but not shown in the app, although recorder.md says it should be.
9. **4.1, "let the user correct ... cover art":** only title, artist and album can be
   corrected.
10. **Stale documents that contradict the current code:**
    - end-to-end.md sections 6 and 7 still say the lookahead is "not met ... left as it
      is", and that the "Songs are created..." sentence appears twice. D-167 and D-174
      fixed both.
    - DECISIONS D-83 ("caches not yet redirected") and D-109 ("THIRD_PARTY_LICENSES.md
      does not exist yet") are out of date.
    - PROGRESS "Next" and "Open" are out of date.
11. **HANDOFF.md** has two unfilled placeholders and one misleading SDR figure (section 15).

## 17. The most important gaps, in the order of section 2

1. **Priority 2 (Mac), blocked:** the Mac app has never been compiled, linked or run. This
   leaves unproven:
   - same audio as Windows;
   - the minimum macOS of each binary;
   - the self-check on the oldest runner;
   - the exFAT launch;
   - memory against 1 GB;
   - WKWebView rendering;
   - 20 minutes of sustained playback;
   - the ScreenCaptureKit recorder.

   Needs GitHub (`gh auth login`) or `scripts/build-mac.sh` on the MacBook.
2. **Priority 2, CI design:** green CI would not prove the Mac §9 rows (section 16 item
   1). There is also no check of imported APIs newer than 13.0 (trap 14, D-37).
3. **Priority 3 / smooth playback:**
   - No render-and-cache fallback.
   - The M1's Rubber Band cost is unmeasured.
   - No run combines bubbles, CPU load and a long duration.
   - The whole app's memory (core plus web view) has never been measured on any platform.
4. **Priority 1 (needs ears):**
   - The listening checklist is unrun.
   - The lead/backing split is weak on some songs (-6 dB ghost).
   - The stereo image has not been measured.
5. **Priority 6:** not tested on a real exFAT stick (including the 3.8 GB model re-hash),
   a slow stick or a clean Windows 11 PC.
6. **§10/§13:**
   - The Studio's licence position for distribution is unresolved.
   - HANDOFF.md has placeholders and a wrong SDR figure.
   - PROGRESS and DECISIONS are out of date for the packaging pass.

# Jukeoke: handoff

Written 18 November 2026 at the end of the first build (about 22 hours of unattended work). Read this first. Details
are in `PROGRESS.md`, `DECISIONS.md`, `docs/WORK_ORDER_STATUS.md` (every requirement
of the work order with its status and evidence), `docs/TEST_RESULTS.md` and
`docs/benchmarks/`.

## 1. The state of things, in plain language

**On Windows, Jukeoke works end to end.** You drop in a song (audio or video); on
this PC it removes the lead singer, keeps the backing vocals, times the lyrics to
the letter and writes one song file in about 3 minutes for a 4-minute song. The
player plays those files with the jukebox look, the sliding lyrics, the key change
and the recorder, from a USB-stick layout, without installing anything.

**The Mac version is written but has never been built or run.** There is no Mac
here, and GitHub (which would have built and tested it on a Mac in the cloud) was
left until after the project, as you asked. All the Mac code passes compile checks
on Windows, and scripts are ready to build it on your MacBook
(`scripts/build-mac.sh`) or on GitHub later. Until one of those runs, every Mac item
below is "built but not verified".

**Nobody has listened to anything.** Every audio claim below is a measurement. The
listening checklist (section 6) is what only your ears can settle.

**Everything that can be built and tested on this PC is done.** What is left needs you:
a GitHub login (to build and test the Mac app), the MacBook, your ears, and a few
licence decisions (section 8).

**Nothing was published.** The repository is local only (`jukeoke/.git`), separate
from inquire and everything else.

## 2. What works, and how it was verified

All measured on this PC (Ryzen 7 9700X, RTX 5070 Ti, Windows 11) unless stated. Each
area was built by one agent and re-checked by an independent one that re-ran the
tests and fixed what it found.

| Area | Result | How it was verified |
|---|---|---|
| Separation quality | Default "full" preset: vocals SDR 13.9 dB (mean, 25 MUSDB18-HQ test tracks; 13.3 dB over 50); karaoke-mix SDR 18.10 dB and lead residue in the karaoke mix -15.9 dB (means over 18 CC ccMixter mixes with known lead and backing stems) | 18 candidate models benchmarked on MUSDB18-HQ test and a CC ccMixter set with separate lead/backing vocals (`docs/benchmarks/separation.md`) |
| Nothing lost | The four stems at unity sum back to the original: bit-exact through a real `.jksong` (exact residual); worst case with stored gains 1.3e-7 (half a 24-bit step) | `separation_bench.py lossless`, conformance tests |
| Loudness and stereo image | Karaoke mix vs the true instrumental: MUSDB18-HQ every track within 0.3 LU and correlation within 0.013; ccMixter median -0.05 LU (worst -2.99 LU on the known hard track) | `studio/bench/karaoke_image.py`, `separation.md` 6.9 |
| Speed | 4-minute song: about 175 s (lyrics given) to 230 s (lyrics transcribed); separation is 85-92 % of it; 4.1 GB VRAM for separation | Real app driving the real Studio (`docs/benchmarks/end-to-end.md`) |
| Lyric alignment | JamendoLyrics, 20 English CC songs: word-onset median 0.036 s, 94.2 % within 0.3 s from text only; with line times 94.7 %; end-to-end songs: median 0.033-0.045 s, 100 % within 0.3 s, worst 0.20 s | `docs/benchmarks/alignment.md`, `end-to-end.md` |
| Key change accuracy | Pitch error at most 0.0015 cents for every step -12..+12 (sines and harmonic tones); length exact; no clipping of the test tones | `cargo test -p jk-dsp`, `docs/benchmarks/dsp.md` |
| Key change smoothness | No click on key changes (largest step 1.31x the signal's own); 0 semitones is bit-identical to the plain mix | same |
| Key change on slow computers | Setting "Key change: auto / real time / prepare ahead". Auto measures Rubber Band's speed on each computer (8.6-9.8x real time here, so real time is used) and below 3x renders keys ahead into a capped cache on the stick, showing "Preparing key +2... 40%"; bit-identical to the real-time path; a computer forced to 0.7x played with 0 underruns where the real-time path had 98 | `cargo test -p jk-engine --test keycache`, real-app run in prerender mode |
| Drum alignment | Unshifted drum path lands on exactly the right sample (offset 0); shifted band vs drums: mean offset at most 0.31 ms after correction (a phase vocoder cannot be exact; single impulses jitter up to 2 ms at -12) | impulse tests, `dsp.md` |
| Smooth playback | Real app, 20 minutes, skin + bubbles + ticker on, key change every 30 s, 14 busy CPU threads: 0 underruns, 0 device starvation, worker never below 140 ms ahead, ticker 60.000 fps with 0 frames missing. Engine alone, 20 minutes with seeks too: same result | `app/ui/scripts/real-app-soak.mjs`, `jukeoke-cli play --load-threads 14`, `docs/benchmarks/playback.md` |
| Real-time safety | 0 memory allocations and 0 frees on the audio callback and render worker across play, all 25 keys, seeks, mixer changes, song change and prerender mode (a counting allocator that also counts Rubber Band's C++). The test found Rubber Band allocating on every key change; our vendored copy is patched (output byte-identical, patch noted in `crates/jk-rubberband/VENDORED.md`) | `cargo test -p jk-engine --test rt_alloc` |
| Memory (Windows) | Whole app (core + 7 web-view processes) while playing with bubbles: 0.55-0.61 GB resident, 0.49-0.50 GB private; the web view's GPU process briefly commits about 1.1 GB at start-up | soak memory sampler, `playback.md` section 12 |
| Playback clock | Follows the audio hardware (-20.9 ppm vs the device's own -20.8 ppm), never a timer | same |
| Seeking | Sample-exact; about 40-50 ms from click to sound | `seek-test`, 1336 random seeks bit-exact on a 4-minute song |
| Export backing track | 32-bit float WAV bit-identical to playback; 24-bit FLAC within half a 24-bit step; if the mix would clip in FLAC it is lowered and the app says by how much | engine tests, UI test |
| Lyrics ticker | 0 wrong letters under the marker across whole songs at 1280, 1440, 2560 and 3840 px; upcoming text at least 2.0 s ahead on every test song at 1440x900 and 1280x800 (dense lines drawn smaller, down to 0.45x) | `npm test` ticker suite + real-app runs on the three end-to-end songs |
| Recorder (Windows) | Captures at the device's own rate (48 kHz here); recordings started/stopped in silence are full length (within 64 frames over 20 s); no dropouts. Not bit-identical on this PC because its Realtek driver applies EQ/speaker effects to everything played (residual about -125 dBFS after a linear fit); Windows settings were not changed | `docs/benchmarks/recorder.md` |
| Portability (Windows) | Runs from a different drive letter (subst), writes nothing to per-user folders (app and Studio; all caches go to the stick); exe needs only Windows' own DLLs | portable smoke test, user-folder audits, `docs/STUDIO_RUNTIME.md` |
| Size budget | Apps + Studio runtime + default models: 8.12 GB of files (dist/ proven from a copy under another drive letter: smoke test 8/8, self-check 8/8, a real Studio job, 0 per-user writes); 8.63 GB on a 32 GB exFAT stick formatted with 32 KB clusters (10.5 GB with 128 KB clusters, so START-HERE says to choose 32 KB); budget 12 GB, target 9 GB. Mac app not included (cannot be built here) | `scripts/make-dist.sh` size report |
| Weak computers | GPU hidden, Studio folder missing, a Studio that hangs or crashes: the app opens in under a second, plays, and says plainly why songs cannot be made here; never hangs | end-to-end run, screenshots |
| End to end | Three CC songs (48 kHz FLAC, float WAV, MP4 video) plus a corrupt file in one queue through the real app: three valid song files, the corrupt file failed in 0.04 s with a plain message and the queue went on | `docs/benchmarks/end-to-end.md` |
| Tests | UI 170 passed; Rust (release) 194 passed; Mac type-check 7 of 7 crates; Studio 229 CPU + 24 GPU passed; song-format conformance pass (92 checks); portable smoke test OK; 18 of 18 offline renders byte-identical to the first day's; CI script tests 74 passed; ShellCheck/actionlint clean; clippy 13 style warnings | `docs/TEST_RESULTS.md` |

## 3. Built but not verified, and why

- **Everything on the Mac** (no Mac, no GitHub): the app build itself; Core Audio
  playback (cpal 0.16, pinned because newer versions need macOS 14.2); the
  ScreenCaptureKit recorder; the self-check; launching from an exFAT stick; the
  look in Ventura's WebKit (Safari 16.4-18 range); bubbles and CPU on the fanless
  M1; memory on 8 GB; the "same audio on both systems" comparison (golden renders
  and the CI comparison job exist; only Windows-vs-Windows has run); every binary's
  minimum-macOS check (the script exists). All of it passes `cargo check
  --target aarch64-apple-darwin` on Windows, which proves the code type-checks, not
  that it runs.
- **CI**: `.github/workflows/ci.yml` and `release.yml` have never run (no GitHub).
  `docs/TEST_RESULTS.md` lists the local equivalent that did run. The Mac evidence steps
  now fail when their evidence is missing, and a new check
  (`scripts/check-macos-availability.sh`) fails the build if the Mac app hard-links
  anything newer than macOS 13.0. It has never seen a real Apple SDK. Objective-C
  classes and methods are looked up by name at run time, so those show as REVIEW
  lines rather than failures: on the first CI run someone has to read them.
- **The key-change "prepare ahead" mode on the Mac**: written in platform-neutral code
  and tested on Windows; the M1's Rubber Band speed will be measured by the self-check.
- **A real exFAT USB stick and a clean Windows 11 PC**: not available here. The
  drive-letter test used `subst` on NTFS.
- **Bluetooth latency** and other output devices: only this PC's Realtek speakers
  (and a silent null device) were used.
- **Online lyrics lookup and model downloads from the internet through the app**:
  tested against local and mocked sources; the real endpoints were not exercised end
  to end.

## 4. Not done

- The Mac app has not been built (see above).
- The optional extras in WORK_ORDER section 12 were not started: the work order
  allows them only once everything in section 4 passes section 9, and the Mac half
  cannot be checked yet.
- No Intel Mac build (not needed now).
- Your own songs: `test-songs/` was empty, so Creative Commons songs stood in.
- Seven small Rust libraries ship no licence file in their package (alloc-stdlib,
  block2, objc2, objc2-encode, objc2-foundation, webview2-com, webview2-com-sys).
  Their copyright notices must be copied from their upstream repositories before a
  public release (listed in `third_party_licenses/MISSING.txt`).

## 5. Putting it on a USB stick

You need a 32 GB stick formatted **exFAT** with a USB-C plug (or an adapter) for the
MacBook Air.

**Windows**
1. Format the stick as exFAT (File Explorer > right-click the stick > Format >
   exFAT). This erases it.
2. Copy everything inside `jukeoke\dist\` onto the stick: `Windows`, `Mac`, `Library`,
   `Recordings`, `Models`, `Settings` and `START-HERE.txt`.
3. On the stick, open `Windows` and double-click `Jukeoke.exe`. Nothing is installed.
   Song making works on a PC with an NVIDIA graphics card; any Windows 11 PC can play.

**Mac** (MacBook Air M1, macOS 13.6.9)
1. Get the Mac app: on the MacBook, open Terminal in the `jukeoke` folder and run
   `bash scripts/build-mac.sh` (it asks for Apple's Command Line Tools if missing and
   installs Rust and Node for your user only; no password). It puts `Jukeoke.app` in
   `dist/Mac/`. Copy it into `Mac/` on the stick. (Later, once GitHub is set up, CI
   builds it for you.)
2. First launch, once only: hold **Control**, click `Jukeoke.app`, choose **Open**,
   then **Open** again in the warning ("macOS cannot verify the developer..."). The
   app is not signed with a paid Apple account.
3. If the app says it cannot find its library, macOS has moved it to a read-only
   place ("App Translocation"). Choose the stick's folder when asked, or run once:
   `xattr -dr com.apple.quarantine /Volumes/<stick name>/Mac/Jukeoke.app`.

## 6. Listening checklist

Use headphones or good speakers. Write down what you hear; a later session can act on it.

1. **Ghost vocals.** Play each song's karaoke mix next to the original. Listen for
   the lead singer left behind, especially on songs with lots of backing vocals. The
   weakest step is splitting lead from backing: on one test song the lead leaked
   strongly into the backing vocals (residue -6 dB with backing on, -21 dB with it
   off). Try **Backing vocals off** when you hear it.
2. **Full vs Clean preset.** Make one song with each (Studio > preset). "Full"
   removes the singer better by measurement; "Clean" keeps slightly more cymbal
   shimmer (0.3-0.6 dB at 4-16 kHz). Listen for dull cymbals or a hollow midrange.
3. **Key change at +2, -2, +5, -5** (and +12/-12 once): listen for a phasey or
   watery sound, smeared drums or cymbals, and anything odd in the 40 ms crossfade
   when you change key while playing. The change is heard about 0.28 s after you
   click; say if that feels slow.
4. **Drums natural vs shifted.** Default: drums are not pitch-shifted. At +5 and -5,
   listen for drums flamming against the band. Then turn on "Shift drums too" and
   compare.
5. **Guide vocal** at 30-50 %: does the singer sound natural when the key is changed
   (formants move with the pitch)?
6. **Exported backing track** (Mixer > Export): compare FLAC and 32-bit WAV. If the
   app says the FLAC was lowered to avoid clipping, the WAV keeps the exact level.
7. **Recorder.** Record something playing and compare it by ear with the source. On
   this PC Windows adds the Realtek EQ/speaker effects to everything; for an exact
   copy, turn off "Audio enhancements" for the output device in Windows' sound
   settings (the build did not change any settings).
8. **Lyrics.** On a fast song the ticker draws dense lines smaller (down to 0.45x) so
   you can always read 2 s ahead: say if it is too small. Transcribed lyrics (no text
   given) are about 73 % word-accurate and marked "unchecked": correct them in the
   Sync editor.

## 7. MacBook checklist (MacBook Air M1, 8 GB, macOS Ventura 13.6.9)

1. Build or fetch the Mac app and copy it to the stick (section 5).
2. First launch with Control-click > Open > Open. Note the exact words of each dialog.
3. **Run the self-check** (Settings > Self-check). It plays a quiet test tone, opens
   the bundled demo song, renders it, shifts its key, checks the recorder permission
   and makes a 3-second capture, and notes the macOS and WebKit versions, memory and
   the output format. When macOS asks for **Screen Recording**: that is the only way
   macOS 13 lets an app capture the computer's sound; the app records audio only.
   Allow it in System Settings > Privacy & Security > Screen Recording, then "Quit &
   Reopen". The report is saved in `Settings/selfcheck-<date>.txt` on the stick.
   **Bring the stick back to the Windows PC**; the next session reads the report.
4. Play the demo song from the stick: smooth lyrics, key change at +2 and -5 while
   playing, seek, pause. Any dropout or stutter?
5. Bluetooth headphones: in the Sync editor set the device offset until the lyrics
   match the voice (AirPods typically need about 150-250 ms).
6. Recorder: record 30 seconds of something playing. For no conversion, set the Mac's
   output to 48,000 Hz in Audio MIDI Setup (the app shows the capture format).
   macOS may ask for the permission again after each new build of the app (it is
   ad-hoc signed).
7. A 20-minute session with bubbles on and a few key changes: note dropouts, how warm
   the Mac gets and the battery drop. Activity Monitor: Jukeoke's memory (target:
   under 1 GB).
8. Look at the window at the normal display setting (1440x900) and at "Larger Text"
   (1280x800): anything cut off or overlapping?

## 8. Needs you

1. **GitHub.** When you are ready: `gh auth login` (the GitHub CLI is installed). A
   later session then creates the private repository, pushes, and CI builds and tests
   the Mac app on GitHub's Macs. Making it public stays your decision.
2. **Licences: the Studio and NVIDIA.** The player and recorder are cleanly
   GPL-3.0-or-later. The Windows Studio runtime ships NVIDIA's CUDA libraries (about
   2.7 GB; NVIDIA's licence lets you redistribute them, but they are not open source)
   in the same process as GPL code (our Studio code, the `mutagen` tag reader, and
   the x264/x265 encoders inside the bundled FFmpeg). Whether that combination may be
   *distributed* to others under the GPL is not settled. For your own use it does not
   matter. Before giving the Studio to others, choose one: (a) add a GPL "additional
   permission" for the NVIDIA libraries to our own code (draft text in
   `THIRD_PARTY_LICENSES.md` section 1) and replace `mutagen` and the GPL FFmpeg build
   with LGPL/permissive equivalents; or (b) publish the player only and let people
   build the Studio themselves. See `THIRD_PARTY_LICENSES.md`.
3. **The "Clean" preset's model has no published licence** (commercial use unclear).
   It is downloaded only if you choose that preset. Keep it, or drop the preset.
   The default "Full" preset uses only models whose licences allow commercial use.
4. **Git history.** The first 17 commits contain this PC's user folder name in a
   path in `PROGRESS.md`, and every commit carries your name as author (from your git
   settings). If you do not want those public, the history should be rewritten before
   publishing (a later session can do it on request).
5. **The name.** "Jukeoke" is a placeholder (in `branding.json`); check for conflicts
   before a public release.
6. **Your songs.** Put 2 or 3 songs you want to sing in `test-songs/`; a later session
   runs them through the whole pipeline.
7. **First GitHub CI run.** Read the macOS availability report's REVIEW lines once
   (Objective-C names that cannot be checked statically) and record the reviewed ones,
   so later runs fail only on new ones.
8. **A leftover process.** An idle Node.js process from the build (port 5191) may
   still be running; restarting the PC clears it.

## 9. Where things are

| Path | What |
|---|---|
| `dist/` | The ready-to-copy stick layout (not in git) |
| `README.md`, `CONTRIBUTING.md` | What it is, how to run and build, how to contribute |
| `docs/SONG_FORMAT.md` | The `.jksong` specification |
| `docs/STUDIO_PROTOCOL.md`, `docs/APP_API.md` | The two internal interfaces |
| `docs/ARCHITECTURE.md`, `docs/DESIGN.md` | How it is built; the jukebox look |
| `docs/MAC.md` | Everything about the Mac version |
| `docs/benchmarks/` | Every measurement |
| `docs/WORK_ORDER_STATUS.md` | Each work-order requirement: status and evidence |
| `MODELS.md`, `THIRD_PARTY_LICENSES.md`, `LICENSE` | Licences |
| `scripts/` | `devenv.sh`, `make-dist.sh`, `build-mac.sh`, runtime build and checks |
| `../jukeoke-build/` | Heavy local files: toolchains, venvs, model cache, evaluation data (about 40 GB; safe to delete `eval/cache`) |

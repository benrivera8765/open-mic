# Practice mode in the app (Work Order 2, Stage 3b and the real-app half of Stage 5)

The app core's practice session (`app/src-tauri/src/practice/`) wired to the real engine
and the real UI, and proved in the **release app** from its own portable folder, driven over
WebView2's DevTools. Measured 2026-11-20 on the build PC (Ryzen 7 9700X, Windows 11). Two
rounds: round 1 (03:27-03:51 UTC, previous build) and round 2 (05:54-07:50 UTC, the build of
this page's code). **Round 2 ran while other workstreams kept the processor at 93-100 %
load** (their builds and benchmarks); the numbers below say where that mattered.

**No microphone was used.** This PC has none (Windows lists 0 active inputs), and every
singer is generated sound fed through the real capture-side path by `practice_test_input`
(APP_API 12.1; the app runs with the internal `--practice-test` flag). Nothing was played
aloud: the engine's `null` output device (no audio hardware) with master volume 0 in the
real app; the cargo tests use the null device at master 1.0 (still no audio hardware) so the
echo path has music to work on. Screenshots show only the generated, public-domain demo song
("Twinkle, Twinkle, Little Star", 1806); the real songs (a copy of the owner's song and a
Creative Commons song, guides added with `jukeoke-cli guide add`) were sung by generated
singers with no screenshot, and no lyrics were printed anywhere.

Evidence (outside the repository): round 2 in `../jukeoke-build/stage3b-r2/`
(`real-app-final/`, `real-app-key-fixed-1/`, `real-app-key-fixed-2/`, `soak/`,
`soak-control/`, `selfcheck-stdout.txt`, `userdirs-diff.json`, `practice-audit.json`,
`cargo-tests.log`), round 1 in `../jukeoke-build/stage3b-real-app/` and `stage3b-soak/`;
singers in `../jukeoke-build/stage3b-work/singers/` (`real-songs.json` lists the real songs).

## 1. What runs where

| Piece | File | Thread |
|---|---|---|
| Session life, commands, slot and settings writes | `practice/mod.rs` | command pool |
| Input list, permission (Windows switches read only), meters, sources | `practice/mic.rs` | meter thread for the picker |
| Generated test sources: singer sung along to the song, mixer return | `practice/testsource.rs` | one source thread |
| Output tap lined up with the input (the echo reference) | `practice/align.rs` | analysis thread |
| Analysis: input -> `jk_pitch::practice::Chain` -> events, take, result | `practice/worker.rs` | `jk-practice` |
| Results as stored / as shown | `practice/result.rs` | analysis thread |
| Rig check (audible; by hand only) | `practice/rigcheck.rs` | command pool |
| Self-check items 9-14 | `practice/checks.rs`, `practice/synth.rs` | self-check thread |
| The key heard at the position an event reports | `playback.rs` `heard_at` | clock thread |

## 2. Cargo tests

`cargo test -p jukeoke --release -- --test-threads=1` on the final code
(`stage3b-r2/cargo-tests.log`): **132 passed** in 11 test binaries (lib 63, build_ui_dist 1,
core_api 12, deleted_songs 6, guide_queue 9, practice_session 7, real_engine 4,
record_sessions 9, selfcheck_core 1 with the six practice items, studio_supervisor 17,
watch_and_record 3). New in this round: playback `the_key_heard_belongs_to_the_reported_position`
and practice_session `choices_exclusions_and_offsets_are_kept_where_they_belong` (section 6).
The dependencies' results are in the same log: see section 7.

`practice_session` on the real engine (null device), `--nocapture`:

| Test | Measured |
|---|---|
| Perfect generated singer, first 15 s, real input path | 100.0 A+, graded 21 of 23 (the first two notes fall in the echo stage's first seconds of "not judged yet"), 1,170 live batches in 15 s, marker on the note in 883 of 883 values inside notes |
| Silent singer | 0.0 F, 0 marker values |
| Key -3 asked at 8.015 s | heard from 8.298 s (283 ms later); `practice_target_at` key 0 at 8.248 s, -3 at 8.348 s; 9 notes finished in key 0 kept their credit, 3 after the change graded in -3 |
| Missing things | no input: watch only "No microphone or audio input is connected..."; permission refused (simulated): watch only with the Windows steps; unplugged after 4 s: input state `unplugged`, take flagged; no lead vocal: `not_ready` / `guide_no_lead`; no guide: built on the spot with no Studio |
| Mixer return 23 ms, -7 dB, guide vocal on, nobody singing | found after 1.8 s, delay 22.9 ms, stable after 4.8 s (latency source `returned_music`, 22.9 ms applied), removal judged `unusable` at 17.3 dB: **0 marker values**, no grade (`untrusted`) |
| Same with a perfect generated singer on top | 99.8 A+, graded 10 of 23, flagged `music_in_input`, `removal_marginal` |
| Choices, exclusions, offsets (new) | octave and strictness in the slot (strict forces shift 0); an excluded line's notes stop counting in `practice_guide`, undo empties the list; the slot passes `validate_practice`; input choice and manual offset in `settings.json` under this computer's hash (37.4 -> 37 ms, 900 -> 500, 0 removes); `practice_test_input` refused (`bad_request`) outside the test modes |
| Rig check, generated return 17 ms at -10 dB | detected on channel 1, 17.09 ms, -10.0 dB, spread 0.03 ms; report written, second run appended; refused without `audible_confirmed` |
| Playback with practice on vs off (capturing null device, whole song, 40 notes graded during it) | 1,824,138 frames each, **0 samples differ** |

## 3. The UI and the release build

`npm run build` (tsc, Vite, Safari 16.4 syntax and CSS checks) clean; vitest **529 passed**
(25 files). Under the 100 % load three older ticker fuzz tests (`page.test.ts` 400 random
songs at two sizes, `pageneedle.test.ts` 200 rap songs) went past vitest's 5 s per-test limit
(7.0 s); they pass with `--testTimeout=60000`, which the round-2 builds used (a copy of
`app/scripts/build-windows.sh` with only that flag changed). Release exe 16.72 MiB, 22
imported DLLs all part of Windows, no build-machine paths.

## 4. The real app (`app/ui/scripts/real-app-practice.mjs`)

Round 2's full run (`real-app-final`): 16 checks, 14 passed; the 2 failures were the
key-change flicker fixed in section 6, and the two key-change checks then passed twice on
the fixed build (`real-app-key-fixed-1`, `-2`). No page errors in any run.

| Check | Result (round 2) |
|---|---|
| (a) perfect singer, whole song | 100 % A+, graded on 42 of 42 notes; marker on the target row in 304 of 304 sampled frames at least 120 ms into a note (371 of 408 counting the note changes, where the marker still shows the previous note for the tracker's 36-62 ms); bars lit in 512 frames |
| (a) silent | 0 % F, marker never shown |
| (a) +70 cents | 64.1 % D, tendency +69 cents |
| (a) an octave low, auto (default) | 100 % A+, octave found -1; the lane moved down an octave at 5.4 s and stayed there (469 of 469 frames after 10 s) |
| (a) an octave low, strict | 0 % F |
| (b) key -3 during the song, real time | heard 308 ms after the presses (fixed build); the UI's key timeline switched at the heard moment to the millisecond (9.331 s vs 9.332 s), a sampled frame showed -3 20 ms later; target C4 -> A3 (WO2 4.6's example); 13 notes before kept key 0, 3 after graded in -3. When the three presses arrived apart (busy PC) they were heard as -1, -2, -3: each step switched at its own heard moment (within 1 ms) |
| (b) the same, forced prerender | heard 399-880 ms after the click on the fixed build (1,152 ms and once 29.5 s at 100 % load, when the low-priority render thread was starved, round 2 earlier builds); the UI switched within 1 ms of the heard moment every time; target G4 -> E4 |
| (c) every key -12..+12 (demo song, 0-18.5 s, generated singer in that key) | 100 at every key; the key in the result equals the key heard; the target at 3.1 s named G3 .. G5 with the key's spelling (Ab, Bb, Eb in flat keys) |
| Real songs (copies with guides added) | the owner's song, 20-45 s: perfect singer 100 % A+ (graded 36 of 54), marker on the bars in 123 of 123 settled frames; silent 0 % F, no marker. A Creative Commons rap song, 20-45 s: 99.9 % A+ (graded 20 of 89, `uncertain_guide`), 44 of 44 settled frames; silent 0 % F |
| (d) no input device (this PC) | watch only, "No microphone found" with the plain reason on screen |
| (d) permission refused (simulated) | watch only with the exact Windows steps and "Open the privacy settings"; practice started with the P shortcut |
| (d) input unplugged after 8 s | plain message, chips "Watch only" + "Input unplugged", the take graded (23.2) and flagged `input_lost`, phase `result` |
| (d) song without a guide, no Studio | "preparing" with progress, then listening after 1.2 s (round 1: 12.5 s on a quieter, less warmed-up disk cache) |
| Open only while on screen | window hidden: input `suspended`; back: `running`; leaving the singing screen: practice off |
| (g) results in the song's slot survive a restart | 20 recent + best 100 before and after a restart; the Library shows the best |

The real-app key sweep uses generated singers in each key (the app path: the player at key k,
the singer at k). The WO2 section 9 proof with the original lead vocal rendered through the
engine at every key, and against wrongly shifted guides, is the harness's (`jukeoke-cli
practice sweep`, docs/benchmarks/practice-score.md).

**Self-check in the release app** (`Jukeoke.exe --selfcheck --silent`, round 2): 14 of 14
pass. Microphone permission: granted, Windows switches allow / allow / allow (read only).
Microphone capture: "No input" (not a failure), the 11 inputs Windows knows but cannot use
listed by name and state. Pitch tracker on generated tones C2..C6 through the real input
path: worst 0.0 cents, 0 octave errors, first value 31 ms after the onset, 15.2 ms per second
of audio. Practice guide: read from the bundled demo song (42 notes, jk-pitch pyin+notes
0.2.0), built again in 0.32 s, same notes. Practice score: perfect 100.0 A+, a semitone flat
9.2 F, silent 0.0. Practice lane (measured by the page in the hidden window): 1.74 ms per
frame on average, 95th percentile 2.00 ms, simple lane 1.44 ms (600 frames at 1088x288,
pixel ratio 1.5; budget 4 ms).

## 5. Twenty minutes, privacy, network

**20-minute run** (practice on, perfect singer looping on the demo song, bubbles on, 14 busy
threads, `--soak 20`): lane 59.9 frames a second, 22 takes graded, history kept at 20,
whole-app memory (app + its 6 web view processes) 576 / 638 / 635 MB (first / max / last, 66
readings), 0 dropped input blocks, **8 underruns**. The **control** run right after, practice
OFF, everything else the same (`--soak 20 --soak-control`): **10 underruns**, 519 / 531 / 527
MB. Both runs' underruns came from the processor load of the other workstreams (93-100 %
before the 14 busy threads were added): the null device's own thread, which runs at normal
priority (a real WASAPI device's callback runs at a raised priority), was descheduled for up
to 59.5 ms (`max_callback_us`) while the render worker's longest block stayed at 0.59 ms.
Practice added no underruns (8 with it, 10 without) and about 60-100 MB of memory. Round 1
(lighter load, previous build): 0 underruns over 20 minutes with practice on, lane 59.9 fps,
575-624 MB (its memory readings stopped after 13 minutes: the harness's process lookup timed
out under load, fixed in round 2).

**Files** (05:54-07:40 UTC: the self-check, every real-app run, both 20-minute runs):
`scripts/user-dirs-audit.py` snapshots before and after: nothing named after the app or the
stick, no registry entry, no audio file anywhere in the per-user folders; the 31
"app_or_studio" entries are Windows' own shell web view
(`Packages\MicrosoftWindows.Client.CBS_*\LocalState\EBWebView`) and the NVIDIA App's logs,
the rest other programs (Firefox, Spotify, OneDrive, Docker, the measuring tools).
`app/ui/scripts/practice-audit.mjs`: 0 audio files created or changed in the portable folder,
LocalAppData, AppData, Temp, Documents and Music. Code: the only WAV writers near practice
are two `#[cfg(test)]` helpers writing generated tones to a test folder; the rig check writes
a text report; nothing in `practice/` or `jk-recorder::input` opens a socket.

**Network.** The app's own code opens no connection; the page's CSP allows only `'self'` and
IPC. But the **WebView2 runtime itself** opens TLS connections to Microsoft addresses
(52.96.x.x:443, 150.171.x.x:443, once 199.232.x.x:80) within 20-60 s of every start: seen in
the plain app started without DevTools and without practice, and with practice on; Tauri's
default `--disable-features=...msSmartScreenProtection` is in force (the harness now passes it
with the DevTools port) and adding `--disable-background-networking` did not stop them. Not
caused by practice mode; left for a decision (section 8).

## 6. Fixed in this round

- **Key-change flicker** (found by the real-app run). `playback://clock` and `player_state`
  report a position moved on to the moment they are sent (up to 0.1 s past the engine's
  snapshot) but sent the snapshot's key heard: an event just after a change paired the new
  position with the old key, and the lane, which trusts the pair, drew the bars in the new
  key, back in the old one, then in the new one again (60-95 ms of wrong bars, measured). The
  core now reports the key heard at the position it sends (`playback.rs` `heard_at`, unit
  test); the UI's key timeline also ignores such a pair for 0.15 s after a change it learned
  from its pending (`practice/pitch.ts`, unit test), for older cores.
- **The Practice plate looked switched off and unreadable while the mouse rested on it**
  after a click (the stage's button hover rule won over the lit plate). It stays gold.
- **The input picker's meter stayed open while the window was hidden** when practice itself
  had no input (watch only). It now closes with the window and reopens when it is back.
- "Getting practice ready ... It takes a second or two" -> "a few seconds" (measured up to
  12.5 s for a 4-minute song).
- Harness: whole-app memory survives a busy PC; marker statistics away from note changes;
  key changes checked against the engine's own heard history (every step, and the UI's own
  switch times) instead of a press time; the octave lane checked to move; real songs; a
  practice-off control run; the web view's own arguments kept for the network listing.

## 7. Open points

- Underruns under heavy outside load (section 5): the null device thread's priority is the
  engine's (`jk-engine` `output/null_out.rs`); a 20-minute run on a quiet PC is the clean
  measurement (round 1: 0).
- WebView2's own connections (section 5).
- With the app's playback returned at -7 dB and the guide vocal on, removal reached 17.3 dB
  and was judged `unusable`, so nobody singing gives **no grade** (`untrusted`) rather than
  the lowest grade; the marker stays off. With a singer the take is graded and flagged
  `removal_marginal`. For the echo and score owners.
- The first seconds of music in each take are "not judged yet" by the echo stage whenever
  there is an output tap with music in it; with master 0 (the real-app runs) there is none.

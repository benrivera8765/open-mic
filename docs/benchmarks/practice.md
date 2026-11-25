# Practice mode: the numbers (Work Order 2, section 9)

One page per WO2 section 9 row: the key numbers and where the detail is. Everything was
measured on the build PC (Ryzen 7 9700X, Windows 11), mostly while other workstreams were
loading it. **Nobody has sung into practice mode.** Every singer is generated sound, the
original isolated lead vocal rendered by the engine, or real Creative Commons / research
vocals, fed through the real capture-side path. This PC has no microphone. Nothing was
played aloud, and no microphone sound exists on disk.

Detail files (the evidence paths are in each one):

| File | What |
|---|---|
| [practice-guide.md](practice-guide.md) | The melody guide: builder, accuracy, real songs, the guide in the file (Stages 0 and 2) |
| [practice-tracker.md](practice-tracker.md) | The live pitch tracker: generated and real voices, effects, delay (Stage 0, hardening, quality round) |
| [practice-key.md](practice-key.md) | The key heard: what the engine publishes, switch timing, the original vocal at every key (Stage 0) |
| [practice-mic.md](practice-mic.md) | Microphone capture, the Mac permission, the AME2 manual (Stage 0) |
| [practice-echo.md](practice-echo.md) | The app's own music in the input: detection, delay, removal, the verdict (Stages 0, 3, quality round) |
| [practice-score.md](practice-score.md) | The grade, the analysis chain and the offline harness (Stage 3a, quality round) |
| [practice-screen.md](practice-screen.md) | The lane, the piano keys, the result screen, drawing cost (Stage 4) |
| [practice-app.md](practice-app.md) | The app core and the release app: real-app runs, 20 minutes, privacy (Stage 3b) |

## Section 9, row by row

| Row | Key numbers | Detail |
|---|---|---|
| Guide accuracy | Plain Rust builder (pYIN + note segmentation, no model). Synthetic singer alone: note F **0.975** (onset 50 ms, pitch 50 cents; best neural tracker SwiftF0 0.961), pitch error median 2.0 cents, onset error median 12 ms, 1 octave error (flagged by the independent spectral check). Through the real Studio separation: F 0.873 (0.96-0.97 without the alto mix, where separation swapped the singers). Real solo singing (vocadito): F 0.578 against a second human annotator's 0.728; 83.5 % of note time under a guide note within 50 cents. 1.17 s per 4-minute vocal, 0 bytes added to the stick | guide 4, 6 |
| Guide on real songs | Pictures for the three CC songs and the owner's song in `practice-guide/pictures/`. The owner's song in `dist/Library` (this pass): 435 notes (397 pitched, 38 unpitched, 65 low confidence, 13 octave flags, 332 graded), tuning -2.1 cents | guide 5, S2.4 |
| Every song | Both songs in `dist/Library` carry a guide (2026-11-20, `jukeoke-cli guide add`): every other chunk and both settings slots byte-identical by SHA-256, `verify --strict --deep` PASS (`../jukeoke-build/finish-dist-proof/`). Conformance: Python and Rust each add a guide the other verifies, same SHA-256 fingerprint; 8 interruption points each leave the old file intact. The previous release (ef62188) opens, plays and renders the new files byte-identically and keeps the guide on its slot save | guide S2.2, TEST_RESULTS 10 |
| Key change | The original lead vocal rendered by the engine at every key -12..+12 and graded against the guide at that key (scorer 0.2.0, final tracker): demo 100.0 at every key; owner's song lowest 98.8 (largest difference from key 0: 0.4); Crowdpleaser 97.6 (1.6); held-out singers ccm-30483 / ccm-39381 / musdb_12 lowest 96.5-98.4. **Low male songs lose points at the lowest keys:** Feel 62.4 at -12 (2.7 from key 0 down to -6), Homeless Man 85.2, musdb_06 75.5 (melody below the tracker's floor). Against a guide shifted by a different amount: at most 8.0-55.7, the lowest band (F) on every song | score 0.1, 4.3, 5 |
| Key change during a song | Engine: published switch vs pitch change measured in the produced audio, worst 6.6 ms on tones, every measurable song switch within 50 ms, in both key modes. Release app, final build (2026-11-20, `finish-real-app-key/`): real time, heard 466 ms after the presses (two steps -2, -3), the UI's key switched 0-1 ms from the heard moment, lane frames 16-37 ms later, target Bb3 -> A3; render ahead, heard 1,263 ms after the press, UI 0 ms, target A4 -> F#4; notes before kept key 0, notes after graded in -3. Names checked for every key (G3..G5 in each key's spelling) | key 2, app 4 |
| Tracker | Generated C2..C6, 16 / 44.1 / 48 / 96 kHz: max 2.26 cents (<= 0.10 above 16 kHz), 0 octave jumps on held notes; onset to first correct value median 35 ms, max 62 ms; 0 pitched events on silence, breath, speech-like noise, thumps and pops (one 20 ms whisper run in 30 fresh minutes). Real voices: 84-91 % of sung frames shown, right note when shown 98.7-99.1 %; **6.0-6.2 % of real onsets shown after 100 ms** (accepted). Self-check in the release app: worst 0.0 cents, first value 31 ms, 15.2 ms per second of audio | tracker results table, 22, 23 |
| Scoring | Scripted singers on 5 songs (standard strictness): perfect, steady, 30 cents sharp, an octave high, 100 ms early or late 98.5-100 (A+); 70 cents sharp 64.0-76.5 (D-C+); a semitone flat 9.2-26.8 (F); an octave low 100 by default where the octave is in range (strict 0.0-0.4); 400 ms late 29.0-73.3; one note 8.8-31.2; silent and noise 0.0 with no marker. Tendency within 3 cents of the scripted offset; the same input gives the same hash every time | score 4.1 |
| The owner's rig | Voice on one channel of two, 44.1 / 48 / 96 kHz, compression: at most 0.5 points from the clean take; reverb up to 6.2 points at RT60 0.6 s (tracker: **-13 points of raw pitch accuracy at RT60 0.8 s held-out**, accepted). Music returned: found 118 of 118 (median 0.84 s), USB / cable delay measured to about 0.004 ms median (12.0 / 12.0 ms), removal 29-65 dB with nobody singing; nobody singing gives 0.0 with no marker at every USB and cable level. Scripted singers at -3 / 0 dB: 80 of 80 graded within 2.6 points of clean (fresh set V); **the original singer over a -3 / 0 dB return: 6 of 16 graded** (accepted, "mostly ungraded"). Bluetooth: no grade, pitch in the rests up to 4.96 % of frames before confirmation (accepted; the app says practice needs a cable or USB) | score 4.5, 4.6; echo 13.8, 13.10 |
| The screen | Whole-song check (`laneverify.ts`) at 1280x800, 1440x900 and 4K, keys 0 / -5 / +3 and -3 with octave -1: 0 errors, worst bar offset 0.00 px, every lit key and marker right. Drawing: lane + lyrics 0.32 ms per frame at 1440x900 (budget 4 ms); release-app self-check 1.74 ms, simple lane 1.44 ms. Screenshots at 1440x900, 1280x800, 4K, reduce-motion, simple lane, result screen | screen 1-4 |
| Playback untouched | Practice on vs off over the whole demo song on a capturing device: 1,824,138 frames, **0 samples differ**. No-allocation tests pass (`jk-engine` `rt_alloc`, `jk-pitch` `echo_alloc` / `practice_alloc`). 20 minutes with practice on, bubbles, a looping singer: 0 underruns on a quiet PC (round 1); under 93-100 % outside load 8 with practice against 10 without (round 2); lane 59.9 fps; 576-638 MB for the app and its web view | app 2, 5 |
| Privacy and portability | User-folder snapshots before and after every real-app run: nothing written outside the app's folder, no audio file anywhere; nothing in `practice/` or `jk-recorder::input` opens a socket. WebView2 itself contacts Microsoft at start-up with or without practice (open point) | app 5 |
| The Mac | Type-checks for `aarch64-apple-darwin` (`app/scripts/check-mac.sh`, 2026-11-20, 0 errors). The bundle's microphone text is checked by `scripts/check-macos-infoplist.sh`, run by `build-mac.sh`. The six self-check items ran on Windows (14 of 14 pass). **Everything else on the Mac is built but not verified**; the availability check runs only on a Mac build | mic "The Mac", MAC.md 12 |
| When things are missing | Release app: no input device, permission refused, input unplugged mid-song, and a song with no guide and no Studio all explain themselves (watch only, plain messages, the take graded and flagged); nothing crashed | app 4 |

## Accepted limitations

Documented, not fixed (owner's usage limit; DECISIONS D-250):

- **Heavy mixer reverb lowers tracker accuracy**: -13 points of raw pitch accuracy at
  RT60 0.8 s on held-out voices (tracker 22, 23). Turn the mixer's reverb off while
  practising.
- **About 6 % of real note onsets are shown more than 100 ms late** (dev 6.0 %,
  held-out 6.2 %; generated voices are within 62 ms). Timing never costs pitch points.
- **Bluetooth music under a singer can show as pitch in the rests** (its delay wanders);
  the app says practice needs a cable or USB.
- **A real voice over a loud (-3 / 0 dB) return is mostly ungraded** (the result says
  why). Recommended wiring: music over USB with the mixer's loopback off (MAC.md 12.2).
- **Low male songs lose points at the lowest keys**: the melody falls below the tracker's
  floor (about C2), so fewer frames are shown there.

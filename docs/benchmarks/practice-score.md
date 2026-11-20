# Practice mode: the grade, the analysis chain and the offline harness (Work Order 2, Stage 3)

Work Order 2 sections 4.4 ("Timing"), 4.6 ("Octaves", "Notes already sung"), 4.7 ("The
grade") and the section 9 rows "Scoring", "Key change", "Key change during a song" and
"The owner's rig" (the grading parts). Measured 2026-11-19 on the build PC (Ryzen 7
9700X, 16 threads, Windows 11), GNU toolchain, release build of `jukeoke-cli` made at
20:01 (SHA-256 `a2b20a8a729bb58a...`), with the tracker and echo code as they stood on
disk at that moment (both were still being hardened by their own workstreams; the
numbers below move with them and are re-run by the commands in section 3). Stage 7
merges this file into `docs/benchmarks/practice.md`.

Nobody sang. Every performance is generated: the tracker's synthetic voice
(`jk_pitch::tracker::testvoice`) singing the song's own guide, generated noise, and the
**original isolated lead vocal** of the generated demo song, of the three Creative
Commons songs (Cortez "Feel (Stripped)", coruscate "The Homeless Man", Jason Miller
"Crowdpleaser") and of a copy of the owner's song, rendered by the engine's own graph.
No lyrics were read, printed or written (rows of the line breakdown are indices and
times). Nothing was played: renders are offline, captures used the engine's null
device.

Evidence (outside the repository): `../jukeoke-build/practice-score/` - song copies with
guides `songs/`, scripts `run_*.sh` and `table_*.py`, results `singers-v2/`, `sweep-v2/`,
`keychange-v2/`, `rig-v2/`, `feedback-v2/`, `extra-v2/`, tables `*-v2-table.md`,
per-note diagnostics `diag/`, the engine captures `captures/`. `old-attempt1/` holds the
results of an earlier, interrupted attempt with an older scorer; nothing below uses it.

## 1. What was built

| Piece | Where | What |
|---|---|---|
| Scorer | `crates/jk-pitch/src/score.rs` (+ `score/tests.rs`, 18 tests) | Per-note credit in cents against the guide's unrounded pitch with the key heard at each moment; bands; timing tolerance with the singer's usual timing; vibrato and scoops; the excusal rule (section 2.3); octave folding, strict mode and Auto octave decision; what is not graded; line breakdown; tendency; sung and voiced share; stopping early; trust flags; grade letters; the key timeline (`KeyTimeline`, D-239 semantics); note names; live hits and finished notes; `target_at`. Deterministic: no hashing, no threads, `libm` for logarithms and exponentials. |
| Analysis chain | `crates/jk-pitch/src/practice.rs` (+ `practice/tests.rs`, 5 tests; `tests/practice_alloc.rs`) | One pure object per take: input block + host-clock stamp -> chosen channel (never a sum) -> echo canceller with the reference -> tracker -> gate (Music/Unknown stretches give no pitch) -> song time through the playback-clock anchors, the declared input latency and the applied latency (returned music > stored rig check > manual > none) -> scorer. Outputs: `practice://live` batches (pitch points, hits, finished notes), music-in-the-input status, applied latency, running take state, the result with its trust flags. No threads, no I/O, no clock reads, no allocation per block. |
| Harness | `crates/jk-cli/src/practice.rs` | `jukeoke-cli practice run` and `practice sweep` (section 3). |

## 2. The grading rule, as built

### 2.1 Bands (APP_API 12.6), confirmed by the scripted singers

| strictness | full credit within | nothing from | timing tolerance | measured (owner's song, synthetic singer) |
|---|---|---|---|---|
| relaxed | 75 cents | 150 cents | 200 ms | +45 c: 99.9; +70 c: 99.7 |
| standard (default) | 50 cents | 100 cents | 150 ms | +30 c: 99.8; +45 c: 99.6; +70 c: 66.5; -100 c: 10.3 |
| strict | 30 cents | 60 cents | 100 ms | +45 c: 57.8; +70 c: 8.5 |

Grades: A+ from 95, A 90, B+ 85, B 80, C+ 75, C 70, D 60, F below 60 (unchanged). The
constants of APP_API 12.6 stand as written; nothing measured here asked for a change.

### 2.2 Per note

- A graded note is judged at one moment per 10 ms after its guide glide. The singer's
  pitch at a moment is the median of their values within 80 ms either side (a vibrato is
  judged by its centre). Error in cents against `note.midi + key_at(moment) + 12 x
  octave`, folded into -600..600 unless strict.
- **Timing.** The whole note may be matched with the singer's pitch from up to the
  tolerance earlier or later. New in this stage: once 0.5 s of on-pitch note time
  supports one offset (the singer's usual timing, remembered with a 20 s time constant),
  each note is matched within 80 ms of it (a third of the note's length for notes
  shorter than 240 ms). A steady delay of up to the tolerance - an unmeasured device delay,
  a singer always a little late - still costs nothing (unit test: 140 ms late, 100.0),
  but one note can no longer move the whole tolerance to borrow a neighbour's pitch.
- Tracker dropouts of up to 40 ms between voiced values are bridged (was 100 ms in the
  interrupted attempt; longer gaps are left to 2.3, which needs the singer to make sound).
- The note's key is the key heard at its midpoint; each moment uses the key heard at that
  moment, so a note sung across a change keeps the part sung in the old key.

### 2.3 What the tracker could not hear (new)

A moment where the tracker named no pitch while the singer was making sound (input level
at least -50 dBFS and not returned music) is **excused, up to a quarter of the note's
moments**: it neither earns nor costs. Why: the live tracker shows about 79 % of the
frames real singers sing (D-244), and on the original vocals the missing frames sit in
low notes, onsets and consonants (`diag/`, original vocal at key 0: notes at MIDI 45-52
voiced 0.74 of their length, 52-57 0.90, above 57 0.93-0.96). Without the rule the
original singer of "Feel" scored 75.6 at key 0 and of "The Homeless Man" 72.0. Limits,
each a unit test: silence is never excused (a singer who enters each note 45 % late
scores below 75), a note spoken or whispered from start to end scores 0, a note half
sung and half spoken earns at most two thirds (70.0 measured, one 10 ms moment over),
and returned music is never the singer's sound. The raw `voiced_share` in the result
still says how much the tracker heard.

### 2.4 Not graded, trust, octave

Unchanged from the contract: unpitched, confidence < 0.5, octave-flagged and excluded
notes are not graded, nor notes judged "unknown" by the echo stage for half their
moments or more ("graded on X of Y notes"; the owner's song: 332 of 435). Trust flags
`low_level` (quiet moments > 10 % of the sung ones), `uncertain_guide` (< 60 % of the
covered pitched notes gradable, or an outdated guide), `stopped_early`, plus from the
chain `music_in_input` with `removal_active | removal_marginal | untrusted` and
`latency_unmeasured`. Two chain changes in this stage, after a false detection was seen
(section 4.6): the music flags are set only when the music was present for at least 5 s
(or 5 % of a shorter take), and only a delay the echo stage calls `stable` becomes the
singer's timing correction, which is then kept for the session. Auto octave: decided
after 3 s of voiced singing near the graded notes' pitch classes; every octave-low and
octave-high synthetic singer on all five songs was detected as -1 / +1.

## 3. How to run it

```
jukeoke-cli practice run SONG.jksong (PERF.wav | --singer SPEC | --vocal)
    [--key K] [--key-changes t:k,...] [--key-timeline keys.json [--requested-keys]]
    [--target-offset N] [--octave auto|strict|N] [--strictness relaxed|standard|strict]
    [--feedback LEVEL_DB,DELAY_MS,usb|usb_eq|cable|bluetooth] [--guide-vocal G]
    [--channels C --channel N --other silent|noise|music] [--rate R]
    [--reverb RT60[,WET_DB]] [--compress moderate|heavy] [--level-db L]
    [--start S] [--seconds N] [--block F] [--jitter-us U] [--notes] [--trace] [--out F]
jukeoke-cli practice sweep SONG.jksong --keys -12:12 --wrong 1,-1,2,-2,5,-7 [--with-reference]
```

`SPEC`: `perfect` (the guide with a 5.5 Hz +-30 cent vibrato and a 60 ms scoop from 80
cents below), `steady`, `sharp:C`, `flat:C`, `cents:C`, `octave:N`, `late:MS`,
`early:MS`, `one-note` (the median graded pitch whenever the guide sings), `silent`,
`noise` (room noise at -35 dBFS with hum plus speech-shaped noise at -30 dBFS). The
reference (what the app plays) is rendered offline by jk-dsp at the same key(s) with the
song's saved mix and handed to the chain sample-aligned, as the app core will; `--feedback`
puts it back into the input through `jk_pitch::echo::testpath` (USB: fixed delay; cable:
fixed delay with 60 ppm clock drift; Bluetooth: wandering delay). The result JSON holds
the result as stored in the song plus the chain's statistics, the music status, the
applied latency, a hash of the result (equal hashes = identical results) and the timing.
Scripts: `run_singers.sh`, `run_sweep.sh`, `run_keychange.sh` (after
`run_captures.sh`), `run_rig.sh`, `run_feedback.sh`; tables `table_singers.py`,
`table_sweep2.py`, `table_keychange2.py`, `table_rig2.py`, `table_feedback.py`.

## 4. Results

### 4.1 Scripted singers through the whole chain (WO2 section 9 "Scoring")

48 kHz mono input, the echo stage on (reference rendered, nothing returned), standard
strictness, Auto octave. Grade, letter and tendency (cents, + sharp):

| singer | demo | Feel | Crowdpleaser | Homeless Man | owner's song | a person would expect |
|---|---|---|---|---|---|---|
| perfect | 100.0 A+, +0 | 99.8 A+, +0 | 99.6 A+, +0 | 97.8 A+, +0 | 99.6 A+, +0 | top |
| steady | 100.0 A+, +0 | 99.8 A+, +0 | 99.3 A+, +0 | 98.4 A+, +0 | 99.8 A+, +0 | top |
| 30 cents sharp | 100.0 A+, +30 | 99.8 A+, +30 | 99.4 A+, +30 | 99.3 A+, +30 | 99.8 A+, +30 | top (inside half a semitone) |
| 70 cents sharp | 61.6 D, +70 | 62.4 D, +70 | 67.2 D, +70 | 70.3 C, +70 | 66.5 D, +70 | middle (0.6 credit) |
| a semitone flat | 7.4 F, -100 | 7.0 F, -100 | 21.6 F, -100 | 24.3 F, -100 | 10.3 F, -100 | lowest |
| an octave low | 100.0 A+ | 65.7 D | 96.7 A+ | 89.2 B+ | 99.8 A+ | top (octaves forgiven) |
| an octave high | 100.0 A+ | 99.7 A+ | 99.5 A+ | 98.3 A+ | 99.5 A+ | top |
| an octave low, strict | 0.0 F | 0.1 F | 0.0 F | 0.0 F | 0.6 F | lowest |
| 100 ms late | 99.9 A+ | 99.8 A+ | 99.6 A+ | 97.8 A+ | 99.6 A+ | top |
| 100 ms early | 100.0 A+ | 99.8 A+ | 99.6 A+ | 97.7 A+ | 99.6 A+ | top |
| 400 ms late | 73.3 C | 66.0 D | 38.4 F | 23.9 F | 64.7 D | clearly lower; less on songs of long held notes (demo) |
| one note throughout | 9.1 F | 29.9 F | 8.8 F | 22.0 F | 21.2 F | lowest |
| silent | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | nothing |
| noise | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | 0.0 F, marker 0 | nothing |

- The tendency is the scripted offset exactly (+30, +70, -100) on every song.
- The same input gives the same result: `perfect` run twice gives identical hashes on
  all five songs; the owner's song with 10 ms blocks, 64-frame blocks with +-500 us of
  stamp jitter, and 2048-frame blocks gives one hash (`68b7983c2ff532d4`).
- Graded on: demo 40 of 42 (2 notes in the first second, before the echo stage has
  decided, are "unknown"), Feel 245 of 503, Crowdpleaser 82 of 593, Homeless Man 94 of
  679 (both rap-heavy: `uncertain_guide` flagged), owner's song 332 of 435.
- The one miss: "Feel" an octave low (65.7) puts its melody at MIDI 33-44 (55-104 Hz),
  mostly below the tracker's required range (E2, 82 Hz), where the tracker shows few
  frames. An octave high scores 99.7. Not a scorer fault; see section 5.
- Cost: the whole chain takes 1.5 ms per second of input without the echo stage, 4.5 ms
  with the echo stage idle, 9.8 ms with removal running (USB return at -10 dB): 650, 224
  and 102 times real time on one core (owner's song, `extra-v2/timing-*`).

### 4.2 Stopping early, quiet singing, keys

`extra-v2/` (owner's song, synthetic singer):

| run | grade | graded / total | sung share | flags |
|---|---|---|---|---|
| perfect, stops at 120 s | 99.6 A+ | 128 / 173 | 0.41 | stopped_early |
| perfect, 60 s to 120 s only | 99.9 A+ | 62 / 92 | 0.23 | stopped_early |
| perfect at -40 / -50 dBFS | 99.6 / 99.5 | 332 / 435 | 1.00 | - |
| perfect at -56 dBFS | 26.7 F | 332 / 435 | 1.00 | low_level |
| perfect at -62 dBFS | 0.0 F | 332 / 435 | 1.00 | low_level |
| perfect at key -3 | 99.7 A+ (key -3) | 332 / 435 | 1.00 | - |
| perfect, keys changed at 30 s (-2), 90 s (-5), 150 s (+3) | 99.7 A+ (3 changes) | 332 / 435 | 1.00 | - |
| the same against keys moved by +2 | 5.8 F, tendency -48 | 332 / 435 | 1.00 | - |

(`latency_unmeasured` is on every run without returned music, as it should be.)

### 4.3 The original singer at every key (WO2 section 9 "Key change")

The original isolated lead vocal rendered by the engine's graph at each key, fed to the
chain as the microphone (48 kHz or the song's rate, no reference), graded against the
guide at that key, and the same tracked singing graded against guides shifted by a
wrong amount (+-1, +-2, +5, -7). `sweep-v2/`, 14.6 minutes for all five songs.
Grade (voiced share) / best grade against a wrong guide:

| key | demo | Feel | Crowdpleaser | Homeless Man | owner's song |
|---:|---|---|---|---|---|
| -12 | 100.0 (0.98) / 7.4 | 57.3 (0.48) / 10.6 | 82.0 (0.67) / 22.9 | 68.6 (0.60) / 33.4 | 97.0 (0.90) / 17.2 |
| -9 | 100.0 (0.98) / 7.9 | 85.4 (0.72) / 10.3 | 85.5 (0.71) / 24.3 | 74.3 (0.67) / 36.0 | 97.3 (0.91) / 18.0 |
| -6 | 100.0 (0.99) / 7.8 | 86.5 (0.73) / 10.3 | 89.0 (0.76) / 25.0 | 79.6 (0.71) / 35.9 | 97.7 (0.92) / 18.2 |
| -3 | 100.0 (0.99) / 7.8 | 92.1 (0.79) / 10.8 | 90.2 (0.76) / 26.0 | 83.1 (0.76) / 38.6 | 97.9 (0.93) / 18.7 |
| -1 | 100.0 (1.00) / 7.8 | 92.1 (0.79) / 10.9 | 93.1 (0.81) / 27.2 | 84.7 (0.78) / 38.5 | 97.9 (0.94) / 18.8 |
| 0 | 100.0 (1.00) / 7.7 | 88.4 (0.75) / 10.0 | 91.8 (0.78) / 26.9 | 82.7 (0.75) / 38.1 | 97.8 (0.93) / 18.6 |
| +1 | 100.0 (1.00) / 7.9 | 93.1 (0.80) / 10.6 | 94.6 (0.84) / 27.3 | 87.4 (0.81) / 39.7 | 98.0 (0.94) / 18.8 |
| +3 | 100.0 (1.00) / 8.0 | 94.8 (0.83) / 10.9 | 95.2 (0.86) / 28.1 | 89.0 (0.85) / 41.0 | 97.9 (0.95) / 19.1 |
| +6 | 100.0 (1.00) / 7.8 | 96.9 (0.87) / 11.0 | 95.8 (0.89) / 29.1 | 90.8 (0.88) / 43.0 | 98.0 (0.95) / 19.4 |
| +9 | 100.0 (1.00) / 7.8 | 97.8 (0.89) / 10.9 | 95.6 (0.91) / 29.5 | 91.1 (0.89) / 43.0 | 98.1 (0.96) / 19.9 |
| +12 | 100.0 (1.00) / 7.8 | 98.3 (0.91) / 11.1 | 95.5 (0.92) / 30.1 | 91.8 (0.90) / 43.3 | 97.9 (0.97) / 20.6 |

All 25 keys are in `sweep-v2-table.md`. Summary:

| song | melody (5-95 % of graded notes) | key 0 | largest difference from key 0: all keys / keys keeping the melody in E2-C6 | best against a wrong guide |
|---|---|---|---|---|
| demo | MIDI 60-69 | 100.0 | 0.0 / 0.0 (-12..+12) | 8.0 F |
| owner's song | MIDI 61-67 | 97.8 | 0.8 / 0.8 (-12..+12) | 20.6 F |
| Crowdpleaser | MIDI 49-55 | 91.8 | 10.3 / 7.0 (-8..+12) | 30.1 F |
| Feel | MIDI 45-56 | 88.4 | 31.1 / 9.9 (-4..+12) | 11.2 F |
| Homeless Man | MIDI 46-55 | 82.7 | 14.1 / 9.1 (-5..+12) | 43.8 F |

- **Against a guide shifted by a wrong amount every song scores in the lowest band** at
  every key (F; at most 43.8, Homeless Man against -1, whose short rap-like notes
  wander by tens of cents, so a semitone-shifted target still meets parts of them).
- **The key handling is exact; the differences are the tracker.** The grade follows the
  voiced share (in brackets) at every key. Graded without the excusal rule of 2.3 (same
  tracker, `diag/`), the grade equals the voiced share: credit / voiced share 1.015,
  1.023 and 1.005 for Feel at -12, 0 and +12, and 0.98-1.02 on the other songs at key 0,
  i.e. every moment the tracker heard was on the target in the key heard. The original vocals of the demo and of the owner's song (melodies
  above MIDI 60) are within 0.8 points of key 0 at every key. The three CC songs are
  low male voices: as the key goes down the tracker hears less of them (Feel at -12 sits
  at MIDI 33-44, below the tracker's E2 floor), and as it goes up it hears more, so their
  grades rise from key 0 by up to 10 points at +12. WO2 asks for "within a few points of
  key 0": met by the two higher voices, **not met by the three low male voices** (7-10
  points inside the tracker's range, up to 31 at -12 for Feel). The fix belongs to the
  tracker's low range (D-238 / D-244: below 160 Hz a note must start "confidently"),
  not to the scorer; reported to the tracker workstream through the orchestrator.
- Key 0 of Feel (88.4) sits below its neighbours (92.1 at -1, 93.1 at +1): key 0 is the
  engine's bit-exact path, every other key goes through Rubber Band, whose output the
  tracker voices slightly more (0.75 vs 0.79-0.80).
- With the echo stage on (`--with-reference`, the karaoke mix as reference) the grades are
  equal (demo) or up to 2.2 points higher (Feel) than in the table at every key that is graded, but at key 0 of Feel and at
  six keys of the demo the take is `untrusted`: the original lead vocal is sample for
  sample present in the reference (Feel: the lead bleeds into the separated backing
  stem; the demo: its generated harmony stem), so the echo stage rightly sees "the app's
  own playback" in the input. A real singer is never sample-identical with the
  playback; see section 5 for the related synthetic-singer case.

### 4.4 Key change during a song, both key modes (WO2 section 9)

The lead vocal alone played **in real time by the engine** (null device) with key
changes requested during playback, captured with the key timeline the engine
published (`key-switch-capture`, `captures/`: real time: six changes, heard about
0.28-0.29 s after the request; render ahead: four changes, heard 1.3-2.7 s after the
request, when the engine crosses over onto the cached key). Each capture graded through
the chain against the keys **as heard** (the engine's timeline, D-239) and against the
keys **as asked** (trap 1's wrong timeline), and the same stretch of the song at key 0
throughout for comparison (`keychange-v2/`):

| song | mode | changes | graded notes | keys as heard | keys as asked | same stretch, key 0 | notes whose key differs between the two timelines: credit heard / asked |
|---|---|---:|---:|---:|---:|---:|---|
| Feel | real time | 6 | 37 | 72.8 | 67.1 | 88.7 | 2 notes (0.5 s): 97 / 9 |
| Feel | render ahead | 4 | 51 | 82.9 | 63.7 | 87.2 | 14 notes (3.6 s): 82 / 2 |
| owner's song | real time | 6 | 45 | 97.9 | 90.6 | 98.1 | 4 notes (0.8 s): 99 / 28 |
| owner's song | render ahead | 4 | 65 | 97.7 | 82.1 | 98.0 | 12 notes (4.1 s): 96 / 14 |
| Homeless Man | real time | 6 | 28 | 90.7 | 85.8 | 88.4 | 2 notes (0.4 s): 97 / 43 |
| Homeless Man | render ahead | 4 | 28 | 89.9 | 64.3 | 88.4 | 7 notes (1.4 s): 89 / 9 |
| Crowdpleaser | real time | 6 | 23 | 81.2 | 78.9 | 83.3 | 3 notes (0.3 s): 33 / 12 |
| Crowdpleaser | render ahead | 4 | 23 | 83.7 | 55.8 | 83.3 | 7 notes (1.4 s): 78 / 5 |

- The notes sung between the click and the moment the change was heard are graded in
  the key that was sounding: 78-99 % credit with the heard timeline against 2-43 % with
  the asked one (Crowdpleaser real time: 3 notes of 0.1 s, too few to say much).
- With the heard timeline the grade across six or four key changes is within 2.5 points
  of the same stretch at key 0 for the owner's song, Homeless Man and Crowdpleaser.
  Feel loses 4-16 points because its changes go to -5 and -7, where the tracker hears
  less of this low voice (voiced share 0.67 against 0.80 at key 0; accuracy of what was
  heard unchanged, `diag/` decomposition in the evidence notes).
- That the published switch matches the pitch change in the produced audio to within
  0.02 ms is D-239's measurement (`docs/benchmarks/practice-key.md`); this stage shows
  the scorer uses it: `KeyTimeline::key_at` per moment, per segment.

### 4.5 The owner's rig: channels, rates, reverb, compression (WO2 section 9, first half)

Each synthetic singer (perfect, +70 c, -100 c, an octave low, silent) on the demo, Feel
and the owner's song, against the clean 48 kHz mono result of 4.1 (`rig-v2/`; full table
`rig-v2-table.md`). Largest change of the grade over the 15 cells, and without Feel's
octave-low singer (out of the tracker's range, 4.1):

| input | largest change | without Feel an octave low |
|---|---:|---:|
| 44.1 kHz, voice on channel 2 of 2, noise on channel 1 | 2.0 | 0.1 |
| 48 kHz, voice on channel 1, the song itself on channel 2 | 0.0 | 0.0 |
| 96 kHz, voice on channel 2, channel 1 silent | 0.3 | 0.3 |
| compressor, moderate | 0.8 | 0.4 |
| compressor, heavy | 2.4 | 1.1, but the demo's perfect singer `untrusted` (section 5) |
| reverb RT60 0.6 s, -6 dB wet | 19.1 | 4.3 |
| reverb RT60 1.2 s, -3 dB wet | 49.2 | 11.1 (Feel perfect 88.7) |
| 96 kHz, channel 2, noise, reverb 0.8 s + heavy compressor | 29.1 | 8.8 (Feel perfect 91.0) |

Silent stays 0 with no marker in every variant. The channel is never summed: the song on
the other channel changes nothing. Channel layout, sample rate and compression move a
grade by at most about 2 points; a long, wet reverb costs up to 11 points (more on the
low voice), which is the tracker's open reverb item (D-244).

### 4.6 The song fed back into the input (WO2 section 9, second half)

The app's playback returned into the singer's channel as a mixer's loopback would, at
-20, -10, -3 and 0 dB (USB: 12 ms fixed), -6 dB with EQ (8 ms), cable at -10 and -3 dB
(25 ms with 60 ppm drift) and Bluetooth at -6 dB (wandering around 180 ms), with the
guide vocal off and on (`feedback-v2/`, full table `feedback-v2-table.md`):

- **Nobody singing:** grade 0.0 F (or no grade: untrusted, for Bluetooth) in all 48
  cases; the marker never showed except with Bluetooth and the guide vocal on (demo: 73
  values = 0.73 s in 40 s; Feel: 0.09 s, with the guide vocal off too). The return was
  found 0.8-1.3 s after the music started (owner's song 3.8-4.4 s: its intro is quiet).
- **With singers, USB and cable at -20 and -10 dB, and USB with EQ:** perfect within
  0.2 points of clean in every case but one (the demo, USB with EQ, guide vocal on:
  `untrusted`, no grade); +70 c within 0.5 points except on the demo with the guide vocal
  on (+2.3 to +4.5); the octave-low singer within 0.2 points (Feel 1.0-3.8 points, out of
  range). Flagged `music_in_input` with `removal_active` or `removal_marginal`.
- **Loud returns (-3 and 0 dB) with someone singing:** `untrusted`, no grade, on Feel and
  the owner's song (and on the demo with the guide vocal on), as APP_API 12.5 asks when
  the removal is not good enough; the demo without guide vocal is graded (marginal) and
  within 1.1 points.
- **Bluetooth:** never graded (`untrusted`), never used for the timing (`latency.source`
  `none`).
- **Measured delay against the true one:** USB 12.0 / 12.0 ms, USB with EQ 8.0 / 8.0;
  cable 27.3 / 27.4 ms at the end of the 40 s demo and 39.1-39.4 / 39.4 ms at the end of
  the 4-minute songs (the true delay drifts from 25 ms by 60 ppm; the chain follows it);
  used as `latency.source` `returned_music`. Bluetooth: measured 204-205 ms against 205
  on the demo, not applied.

## 5. What does not pass, and why

1. **The original low male voices are not "within a few points" across keys** (4.3) and
   lose up to 11 points under a long wet reverb (4.5): the live tracker voices too little
   of low notes (below about 160 Hz) and of reverberant input. The grade of what it does
   hear is exact (credit / voiced share 0.98-1.02 at every key). The demo and the owner's
   song pass. For the tracker workstream.
2. **False "music in the input" with a singer in unison or harmony with the song's own
   vocal parts.** With no music returned at all, the echo stage declared the return
   present for 2.3 s (Feel, perfect synthetic singer) and 14.5 s (Feel, +70 c), and on the
   demo (whose generated harmony stem uses the same kind of synthesis as the synthetic
   singer) under heavy compression for 23.6 s, making that take `untrusted`. The chain now
   ignores detections shorter than 5 s for the flags and never takes a timing correction
   from a delay that is not `stable` (2.4), which removed the Feel cases; the demo case
   remains. A real voice is not sample-locked to a stem, so this may not happen in use,
   but it is unmeasured with real singers. For the echo workstream.
3. **Loud returns with a singer are not graded** (-3 and 0 dB, 4.6), although the same
   return with nobody singing is removed well enough to keep the marker off. Honest (the
   result says why), but a mixer at full loopback level gets no grade. For the echo
   workstream.
4. **Bluetooth with the guide vocal on shows the marker for 0.73 s in 40 s with nobody
   singing** (no grade). Practice over Bluetooth is not promised (APP_API 12.5).
5. **Not run here:** the app core (Stage 3b) does not exist yet, so nothing in this file
   went through a real microphone, the real capture thread or the real clocks; the chain
   was fed with exact host stamps (plus jitter in one run) and a playback clock at the
   input's rate.

## 6. Unit tests

`cargo test --release -p jk-pitch --lib -- score:: practice::`: 23 tests (18 scorer, 5
chain), all pass; `cargo test --release -p jk-pitch --test practice_alloc`: the chain
allocates nothing per block over 20 s of a singer with a USB return (passes). New in this
stage: the excusal rule, the usual timing, note names at every key -12..12, the target at
a key; fixed: expectations that ignored the note sung across a key change and a
neighbour's borrowable end.

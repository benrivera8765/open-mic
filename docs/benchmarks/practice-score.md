# Practice mode: the grade, the analysis chain and the offline harness (Work Order 2, Stage 3)

Work Order 2 sections 4.4 ("Timing"), 4.6 ("Octaves", "Notes already sung"), 4.7 ("The
grade") and the section 9 rows "Scoring", "Key change", "Key change during a song" and
"The owner's rig" (the grading parts). Stage 7 merges this file into
`docs/benchmarks/practice.md`.

**Scorer 0.2.0 (quality round, 2026-11-19 evening to 2026-11-20).** This file was first
written for scorer 0.1.0 (Stage 3a, commit 8abc0b4). Its independent verifier found four
faults in the grade: per-note timing cost pitch points, the "excusal" rule had been tuned
on the proof songs and excused any sound, the original singer of the low male songs was
not in the top band at key 0, and takes with nothing fed back were flagged "music in the
input". Section 0 says what changed.

**Final measurement (Fri Nov 20 05:36:17 EST 2026).** The scorer has not changed since the quality
round's first measurement (snapshot of Fri Nov 20 00:35:55 EST 2026, `q/s2a-*`); the
tracker and echo workstreams have changed their code since. As this workstream's
instructions ask (measure with the tracker stream's latest code at the end, re-measure with
the echo stream's latest code), every table in section 4 was re-run on **one
snapshot of the tree taken at Fri Nov 20 05:36:17 EST 2026** (the tracker and echo code exactly as their
own workstreams had them then; `../jukeoke-build/q-score-s3/SNAPSHOT_TIME.txt` holds the
time and the SHA-256 of every tracker, echo, scorer and harness source), with two builds
that differ only in the scorer: **before** = scorer 0.1.0 (the sources of commit 8abc0b4:
`score.rs`, `practice.rs`, their tests and the harness) and **after** = scorer 0.2.0.
Section 0.1 sets the final numbers beside the first measurement's.
Measured on the build PC (Ryzen 7 9700X, 16 threads, Windows 11), GNU toolchain, release
builds, while other workstreams were building and testing on the same machine (the
speed figures are therefore pessimistic).

Nobody sang. Every performance is generated: the tracker's synthetic voice
(`jk_pitch::tracker::testvoice`) singing the song's own guide, generated noise, and the
**original isolated lead vocal** rendered by the engine's own graph. No lyrics were read,
printed or written (rows of the line breakdown are indices and times; the evaluation
songs were packed without lyrics). Nothing was played: renders are offline, captures used
the engine's null device. No microphone sound exists anywhere in this work.

**Which material is which** (the Stage 3a verifier found tuning on proof material):

| role | material | used for |
|---|---|---|
| development (tuning) | the five proof songs: the generated demo song, Cortez "Feel (Stripped)", Jason Miller "Crowdpleaser", coruscate "The Homeless Man" (Creative Commons) and a copy of the owner's song; synthetic singers with random seeds 1 and 2 | every choice in section 2: the timing rule and its constants (`min_rendition`, `overlap_s`), the excusal clarity threshold |
| held-out (validation only; run after the rules were frozen, nothing changed after) | 16 real lead vocals none of the development singers sang, all from the tracker stream's held-out split: ccMixter lead stems ccm-30483, ccm-34740, ccm-39381, ccm-55541 (CC BY 3.0), MUSDB18-HQ test-set vocals musdb_01, 06, 10, 12 (research use, kept outside the repository), vocadito solo recordings 5, 11, 13, 19, 23, 26, 30, 37 (CC BY 4.0); each packed as a `.jksong` copy (`q/make_heldout.py`) and given its guide by `jukeoke-cli guide add`, exactly as the app does; synthetic singers with new seeds 3 and 4 | sections 4.3, 4.7, 4.8 |
| fresh (final measurement; run once, nothing changed after) | four MUSDB18-HQ test songs packed by the echo workstream's verifier for its own check (set V: Bobby Nobody, Carlos Gonzalez, The Mountaineering Club, Secretariat "Over The Top"), never run by the scoring stream before; synthetic singers with seeds 5, 6, 7 and 8, never run before | sections 4.7 (rubato at every strictness), 4.10 |

Evidence (outside the repository): `../jukeoke-build/practice-score/` - development song
copies `songs/`, Stage 3a results `*-v2/`, engine captures `captures/`; the quality round
in `q/`: held-out songs `heldout/`, scripts (`snapshot.sh`, `build_pair.sh`,
`run_final2.sh`, `run_proof.sh`, `run_timing.sh`, `run_indep.sh`, `table_final.py`,
`fill_doc2.py`, `diag_notes.py`, `check_order.py`, `adversary_ceiling.py`,
`clar_hist.py`, `make_heldout.py`), the results of this file `s3-after/`,
`s3-before/`, `s3-timing-dev/`, `s3-timing-ho/`, `s3-indep/`,
`s3-diag/`, the binaries `bin/cli-s3-after.exe` and `bin/cli-s3-before.exe`;
the first session's development runs (`timing-dev*`, `exp*`, `mono/`, `mr-dev/`,
`sweep-new4/`, `clar/`) and its proof on the 22:40 tree (`final-*`), the sources of the
"before" scorer `before-src/`; the first measurement of scorer 0.2.0 (`s2a-*`); the final
measurement's own scripts `fill_doc3.py`, `compare_runs.py`, `lowkey_diag.sh`,
`lowkey_table.py` and its low-key runs `s3-lowkey/`.

## 0. The quality round: what changed and why

| finding (Stage 3a verifier) | change (scorer 0.2.0) | result (section 4) |
|---|---|---|
| The "usual timing" rule (proposed D-246: each note within 80 ms of the singer's usual timing) made per-note rubato cost points (+-60 ms: 95.5, +-120 ms: 81), against WO2 4.7 | Each note's **rendition** may start and end up to the timing tolerance (150 ms by default) away from the guide's note, the two ends independently, and be as short as half the note; the renditions of consecutive pitched notes keep their order and may not overlap (one stretch of singing is credited to one note only); each note's timing is decided together with the next note's (section 2.2). This is a monotone alignment of the singing to the notes within the tolerance window | Rubato with random per-note offsets of +-60 / +-120 / +-150 ms on the held-out songs: 99.0-99.9 / 98.5-99.9 / 97.8-99.7 (before: 97.7-99.3 / 93.9-97.1 / 90.4-95.7; the perfect singer 99.6-100.0). Notes of 0.2 s or more lose at most 0.7 points even at +-150 ms; what is lost is on notes the rubato singer squeezes below 0.1 s (4.7). No two renditions overlapped by more than 10 ms in 79439 pairs of consecutive notes (all 247 timing takes; largest overlap 10.0 ms, 0 pairs over 10 ms). Adversaries that sing each note at the previous note's pitch: 30.3-59.8 (F) when sung in the next note's place, and no more than each note could earn on its own within the tolerance (+2.2 points at most) when moved 150 ms early (4.7) |
| The excusal rule (proposed D-245) was tuned on the proof songs and excused any sound of -50 dBFS or more | Excuses only frames the tracker found **periodic** (clarity at least 0.7) but named no pitch for, never unvoiced sound, at most a quarter of a note (section 2.3); threshold chosen on development material, validated on held-out singers | Held-out original singers: +0.0 to +1.3 points from the rule. Breath and speech-shaped noise in the middle of every note never score more than silence there, on development and held-out songs (before: up to +12.2 points above it) (4.8) |
| The original singer below the top band at key 0 on the low male songs (Feel 88-92, Homeless Man 83-85, Crowdpleaser 92-94) | Diagnosed per note (4.9): the scorer's share was the timing rule (the tracker's onset delay, scoops, slides and note ends charged as misses) and the any-sound excusal; the rest is the tracker's voicing of low notes and the guide's note pitches | At key 0: demo 100.0, Feel 97.1, Crowdpleaser 99.2, Homeless Man 96.1, owner's song 99.2 (before, same tracker: demo 100.0, Feel 96.3, Crowdpleaser 94.4, Homeless Man 87.6, owner's song 97.6); held-out original singers 93.3-100.0 (before 85.8-99.8). Across keys see 4.3 and section 5 |
| False "music in the input" flags on takes with nothing fed back | The chain flags music, makes a take untrusted or leaves moments ungraded only on the echo stage's own verdicts (unchanged in behaviour, now under a unit test); the false detections themselves belong to the echo stream | With the echo code of the snapshot: takes with nothing fed back that carry a music flag: 2 of 75 singers takes, 5 of 120 rig takes (the before build, same echo code: 2 of 75 singers takes, 5 of 120 rig takes; the Stage 3a doc, older echo code: 12 of 75 and 32 of 120) (4.6) |

Public API of `jk-pitch` (used by the app core, Stage 3b): **additive only.**
`SingerSample` gained `clarity` (`#[serde(default)]`; built only inside `jk-pitch`);
`ScoreConfig` gained `excuse_clarity`, `min_rendition`, `overlap_s` (the app builds it with
`..ScoreConfig::default()`); `NoteOutcome` gained `start_shift_s`, `end_shift_s`;
`ScoreConfig::{jitter_s, lag_memory_s, lag_evidence_s}` are kept but ignored;
`SCORER_VERSION` is "0.2.0", so results of 0.1.0 and 0.2.0 are never compared as equal.
One behaviour the app sees: the finished-note entry (`practice://live` `done`) of a pitched
note whose rendition could meet the next note's (the next note starts less than about
0.31 s after it ends) comes out when the next note is judged, not at once; the lit parts
of the bars (`hits`) are unchanged. `Chain::drain_live` gives the caller's vectors their
room on its first call. The app crate type-checks against this `jk-pitch`
(`cargo check -p jukeoke` on the final snapshot, with the app core as the Stage 3b
workstream had it at that moment: passes).

### 0.1 The final measurement beside the first one

Same scorer (0.2.0), same songs, singers and seeds; only the tracker and echo code differ
(`python -I q/compare_runs.py s2a s3`):

| item | quality round (snapshot `s2a`) | final (snapshot `s3`) |
|---|---|---|
| original singer at key 0, demo (dev) | 100.0 | 100.0 |
| original singer at key 0, Feel (dev) | 97.0 | 97.1 |
| original singer at key 0, Crowdpleaser (dev) | 99.4 | 99.2 |
| original singer at key 0, Homeless Man (dev) | 96.5 | 96.1 |
| original singer at key 0, owner's song (dev) | 99.0 | 99.2 |
| original singer at key 0, 16 held-out singers | 93.0-100.0 | 93.3-100.0 |
| every key, demo (dev) | lowest 58.0 (key -11); largest difference from key 0 42.0 all keys / 19.4 from -6; wrong guide at most 8.4 | lowest 100.0 (key -12); largest difference from key 0 0.0 all keys / 0.0 from -6; wrong guide at most 8.0 |
| every key, Feel (dev) | lowest 62.8 (key -12); largest difference from key 0 34.2 all keys / 2.8 from -6; wrong guide at most 16.7 | lowest 62.4 (key -12); largest difference from key 0 34.7 all keys / 2.7 from -6; wrong guide at most 16.9 |
| every key, Crowdpleaser (dev) | lowest 97.1 (key -12); largest difference from key 0 2.3 all keys / 0.3 from -6; wrong guide at most 39.9 | lowest 97.6 (key -8); largest difference from key 0 1.6 all keys / 0.4 from -6; wrong guide at most 39.7 |
| every key, Homeless Man (dev) | lowest 85.4 (key -12); largest difference from key 0 11.1 all keys / 1.4 from -6; wrong guide at most 54.9 | lowest 85.2 (key -12); largest difference from key 0 10.9 all keys / 2.3 from -6; wrong guide at most 55.7 |
| every key, owner's song (dev) | lowest 98.7 (key -12); largest difference from key 0 0.3 all keys / 0.2 from -6; wrong guide at most 27.2 | lowest 98.8 (key -10); largest difference from key 0 0.4 all keys / 0.2 from -6; wrong guide at most 28.0 |
| every key, ccm-30483 (held-out) | lowest 96.9 (key -12); largest difference from key 0 0.6 all keys / 0.4 from -6; wrong guide at most 30.4 | lowest 96.5 (key -12); largest difference from key 0 0.7 all keys / 0.3 from -6; wrong guide at most 30.0 |
| every key, ccm-39381 (held-out) | lowest 96.8 (key -12); largest difference from key 0 3.0 all keys / 0.7 from -6; wrong guide at most 33.1 | lowest 96.7 (key -12); largest difference from key 0 3.2 all keys / 0.8 from -6; wrong guide at most 33.5 |
| every key, musdb_06 (held-out) | lowest 76.9 (key -12); largest difference from key 0 18.5 all keys / 2.9 from -6; wrong guide at most 21.7 | lowest 75.5 (key -12); largest difference from key 0 20.1 all keys / 3.4 from -6; wrong guide at most 22.2 |
| every key, musdb_12 (held-out) | lowest 98.3 (key -12); largest difference from key 0 1.0 all keys / 0.1 from -6; wrong guide at most 27.6 | lowest 98.4 (key -12); largest difference from key 0 1.1 all keys / 0.2 from -6; wrong guide at most 28.4 |
| takes with nothing fed back that carry a music flag (singers + rig) | 7 of 195 | 7 of 195 |
| notes left ungraded by the echo stage's 'cannot judge' on those takes | 139 (graded 36546) | 593 (graded 36092) |
| rubato +-60 / +-120 / +-150 ms, held-out songs (seeds 3, 4) | 99.0-99.9 / 98.5-99.9 / 97.8-99.7 | 99.0-99.9 / 98.5-99.9 / 97.8-99.7 |
| rubato +-60 / +-120 / +-150 ms, development songs (seeds 1, 2) | 95.7-100.0 / 95.4-100.0 / 94.2-100.0 | 95.7-100.0 / 95.3-100.0 / 94.2-100.0 |
| perfect singer (timing set), held-out / development | 99.6-100.0 / 98.3-100.0 | 99.6-100.0 / 98.3-100.0 |
| adversary behind, all 13 songs | 30.3-59.8 | 30.3-59.8 |
| adversary behind:150, all 13 songs | 41.8-75.2 | 41.8-75.1 |
| adversary ahead:150, all 13 songs | 39.2-80.5 | 39.2-80.5 |
| adversary swap, all 13 songs | 22.7-54.9 | 22.7-54.9 |

- **The scorer's results moved only where the tracker or the echo code changed.** Every
  take without a reference (the original vocal, the key sweeps, key changes, the timing
  singers and adversaries) is graded by the same scorer on the same input, so a change
  there is the tracker's: the demo song's vocal now scores 100.0 at every key (the
  tracker's comb exemption, practice-tracker.md 22.1 item 7; the first measurement had
  58-96.5 at keys below -1); the low male songs at their lowest keys are where they were
  (within 1.4 points). The original singers at key 0 moved by at most 0.4 points on the
  development songs and by at most 0.3 on the held-out ones apart from three short
  vocadito recordings (23: 0.8 lower, 40 graded notes; 26: 0.6 lower, 18; 30: 1.6 higher,
  19).
- **Takes with a reference (the echo stage on)** moved with the echo code: loud returns (-3 and 0 dB) with the guide vocal off are now graded within about a point of
the clean take (the first measurement: mostly not graded); the takes with nothing fed back
that carry a music flag are the same 7 of 195, but the echo stage leaves more of their
notes unjudged (593 against 139 notes, 4.6); with the karaoke mix as the reference the demo
loses notes to the echo stage at three keys (4.3).
- Nothing in the scorer was changed by this measurement; `SCORER_VERSION` stays "0.2.0".
- **These are the workstreams' latest code.** The tracker and echo sources were compared with
the snapshot again at Fri Nov 20 06:52 EST 2026, after the tracker workstream's independent
verification (practice-tracker.md section 23) and the echo workstream's quality-round
write-up (practice-echo.md section 13): the only differences are two doc comments in
`tracker.rs`, a diagnostics accessor in `echo/filter.rs` and a diagnostics string in
`echo.rs` (`diff -r` against `../jukeoke-build/q-score-s3/crates/jk-pitch/src`), none of
which changes what the code computes.

## 1. What was built

| Piece | Where | What |
|---|---|---|
| Scorer | `crates/jk-pitch/src/score.rs` (+ `score/tests.rs`, 22 tests) | Per-note credit in cents against the guide's unrounded pitch with the key heard at each moment; bands; the rendition timing rule (2.2); vibrato and scoops; the excusal rule (2.3); octave folding, strict mode and Auto octave decision; what is not graded; line breakdown; tendency; sung and voiced share; stopping early; trust flags; grade letters; the key timeline (`KeyTimeline`, D-239 semantics); note names; live hits and finished notes; `target_at`. Deterministic: no hashing, no threads, `libm` for logarithms and exponentials. |
| Analysis chain | `crates/jk-pitch/src/practice.rs` (+ `practice/tests.rs`, 6 tests; `tests/practice_alloc.rs`) | One pure object per take: input block + host-clock stamp -> chosen channel (never a sum) -> echo canceller with the reference -> tracker -> gate (Music/Unknown stretches give no pitch) -> song time through the playback-clock anchors, the declared input latency and the applied latency (returned music > stored rig check > manual > none) -> scorer (with the tracker's clarity). No threads, no I/O, no clock reads, no allocation per block. |
| Harness | `crates/jk-cli/src/practice.rs` | `jukeoke-cli practice run` and `practice sweep` (section 3). |

## 2. The grading rule (scorer 0.2.0)

### 2.1 Bands (APP_API 12.6)

| strictness | full credit within | nothing from | timing tolerance | measured (owner's song, synthetic singer, after) |
|---|---|---|---|---|
| relaxed | 75 cents | 150 cents | 200 ms | +70 c: 99.9 |
| standard (default) | 50 cents | 100 cents | 150 ms | +30 c: 99.9; +45 c: 99.9; +70 c: 70.5; -100 c: 15.5 |
| strict | 30 cents | 60 cents | 100 ms | +45 c: 60.8 |

Grades: A+ from 95, A 90, B+ 85, B 80, C+ 75, C 70, D 60, F below 60 (unchanged). The
constants of APP_API 12.6 stand as written.

### 2.2 Per note, and timing

- A graded note is judged at one moment per 10 ms after its guide glide. The singer's
  pitch at a moment is the median of their voiced values within 80 ms either side (a
  vibrato is judged by its centre). Error in cents against `note.midi + key_at(moment) +
  12 x octave`, folded into -600..600 unless strict. Tracker dropouts of up to 40 ms
  between voiced values are bridged.
- **Timing (new).** The singer's *rendition* of a note runs from `alpha` to `beta` away
  from the note's judged start and end, each anywhere within the strictness's timing
  tolerance (150 ms by default), and is at least half as long as the note
  (`min_rendition` 0.5); moment `k` of the note is compared with the singer at the same
  relative place in the rendition. So a singer who is early or late by up to the
  tolerance, steadily or differently on every note, who scoops into a note or leaves it
  a little early, and the tracker's own onset delay, cost no pitch points; the middle of
  a note that was not sung is never forgiven.
- **No borrowing.** The renditions of consecutive pitched notes (graded or not) keep their
  order and may overlap by at most `overlap_s` (10 ms): one stretch of singing is credited
  to one note only, so a note cannot take its neighbour's singing. Each pitched note's
  timing is decided together with the next pitched note's (when the two renditions could
  meet), taking what gives the two notes together the most credit; ties go to the least
  stretch, then to the timing closest to the previous note's. Unit tests: a steady 140 ms
  delay is matched as a plain 140 ms shift on every note; rubato of +-150 ms scores 100;
  two notes sung in each other's place cannot both be credited; a note sung 180 ms late
  loses only the 30 ms beyond the tolerance; 100 ms of a 400 ms note earns 40 %.
- What this rule cannot tell apart, by construction: a note sung for only part of its
  length is indistinguishable from a note sung late or left early within the tolerance.
  On notes shorter than about twice the tolerance (0.3 s) a singer who sounds only the
  first half of every note can therefore keep most of the credit (`staccato:0.5` in 4.7);
  on longer notes the part beyond the tolerance is lost. Making that cost points would make
  legato rubato cost points too (a note started 150 ms late and left 150 ms early because
  the next note came in early is 300 ms shorter), which WO2 4.7 forbids.
- Choices made on development material only: `min_rendition` 0.5 (0.67 and 0.75 cost the
  original singer of Homeless Man 2.1 and 5.6 points and +-150 ms rubato 2-4 points, while
  the borrowing adversary moved by at most 2.6, `q/mr-dev/`, `q/sweep-mr/`); `overlap_s`
  one hop.
- The note's key is the key heard at its midpoint; each moment uses the key heard at that
  moment, so a note sung across a change keeps the part sung in the old key.
- Cost: the credit of every timing is computed at once from prefix sums when the whole
  note is judged against one target (an exact rearrangement of the plain sums, unit
  test `the_fast_credit_table_equals_the_plain_sums`): one push that finishes a 20 s note
  takes 2.1 ms at relaxed strictness (53 ms without the prefix sums), a 3 s note 0.26 ms.
  The whole chain: 62-149 times real time on one core with the echo stage idle (perfect singer, development songs), 25 times with removal running (owner's song, USB return at -10 dB).

### 2.3 What the tracker could not resolve (the excusal rule, new)

A moment where the tracker named no pitch while the input was **periodic** sound -
tracker clarity at least 0.7 (`excuse_clarity`), level at least -50 dBFS, not returned
music - is excused, up to a quarter of the note's moments: it neither earns nor costs.
The tracker reports for every frame the clarity of its most periodic candidate even when
it names no pitch (`PitchEvent::clarity`), so such a frame is a voice it heard but did
not commit to (a low or rough voice, an onset). Why 0.7, chosen on development material
in the first session (`q/clar_hist.py`, 22:40 tracker): of the frames where the guide's
own curve is voiced but the tracker named no pitch, 45 % (Feel), 57 % (Homeless Man),
58 % (Crowdpleaser) and 72 % (owner's song) had clarity 0.7 or more; of the frames of
generated breath and speech-shaped noise in the middle of notes (which include the
singer's next onset), 8-22 %. With 0.6 instead, breath and speech-shaped noise in the
middle of notes earned up to 3 points more than silence there on the development songs;
with 0.7 never more than silence (`q/exp3/`). Limits, each a unit test: silence, breath,
whispers and periodic sound quieter than -50 dBFS score exactly what silence scores; a
note with no pitch named at all scores 0; a note whose middle half was not resolved earns
at most about two thirds; returned music is never the singer's sound. The raw
`voiced_share` in the result still says how much the tracker heard. Validated on the
held-out singers, with the snapshot's tracker, in 4.8.

### 2.4 Not graded, trust, octave

Unchanged from Stage 3a: unpitched, confidence < 0.5, octave-flagged and excluded notes
are not graded, nor notes judged "unknown" by the echo stage for half their moments or
more ("graded on X of Y notes"). Trust flags `low_level`, `uncertain_guide`,
`stopped_early`, and from the chain `music_in_input` with `removal_active |
removal_marginal | untrusted` and `latency_unmeasured`. The chain sets the music flags
only from the echo stage's verdicts: `music_in_input` when the echo stage said "present"
for at least 5 s (or 5 % of a shorter take), the removal flag from its removal verdicts,
`untrusted` when it called the removal unusable for half the take; a moment is left
ungraded only when the echo stage says it cannot judge it (its start-up, an unusable
removal) or that it held only music; and only a delay it calls `stable` corrects the
singer's timing. Unit test `music_flags_and_ungraded_moments_come_only_from_the_echo_stage`:
music playing but nothing fed back gives no music flag, no timing taken from it and every
note graded after the echo stage's own start-up. Auto octave: decided after 3 s of voiced
singing near the graded notes' pitch classes.

## 3. How to run it

```
jukeoke-cli practice run SONG.jksong (PERF.wav | --singer SPEC | --vocal)
    [--key K] [--key-changes t:k,...] [--key-timeline keys.json [--requested-keys]]
    [--target-offset N] [--octave auto|strict|N] [--strictness relaxed|standard|strict]
    [--feedback LEVEL_DB,DELAY_MS,usb|usb_eq|cable|bluetooth] [--guide-vocal G]
    [--channels C --channel N --other silent|noise|music] [--rate R]
    [--reverb RT60[,WET_DB]] [--compress moderate|heavy] [--level-db L]
    [--start S] [--seconds N] [--block F] [--jitter-us U] [--notes] [--trace] [--out F]
    [--excuse SHARE[,CLARITY]] [--min-rendition X] [--overlap-ms MS] [--dump-samples F]
    [--write-input F] [--no-reference]
jukeoke-cli practice sweep SONG.jksong --keys -12:12 --wrong 1,-1,2,-2,5,-7 [--with-reference]
    [--excuse ...] [--min-rendition X] [--overlap-ms MS]
```

`SPEC`: `perfect` (the guide with a 5.5 Hz +-30 cent vibrato and a 60 ms scoop from 80
cents below), `steady`, `sharp:C`, `flat:C`, `cents:C`, `octave:N`, `late:MS`,
`early:MS`, `one-note`, `silent`, `noise`, and new: `rubato:MS[:SEED]` (each note's onset
moved by its own uniform random offset in +-MS, the note held until the next starts and
kept for at least half its length), `behind[:EARLY_MS]` (each note's time sung at the
previous pitched note's pitch, moved EARLY_MS earlier), `ahead[:LATE_MS]` (at the next
note's pitch, moved later), `swap` (adjacent notes sung in each other's place),
`staccato:SHARE` (only the first SHARE of every note), `gap:SHARE:KIND` (the middle SHARE
of every note replaced by silence, breath, whisper-like fricatives or speech-shaped
noise at the singer's level). `--overlap-ms 100000` switches the order rule off (every
note its own timing, for the comparison in 4.7). `--dump-samples` writes the tracker
values the scorer got (song time, MIDI, level, clarity, flags) and the guide's curve:
pitch numbers only. With `--notes` the result also lists the guide's notes (times, MIDI,
graded, pitched) and every note's outcome. The reference (what the app plays) is
rendered offline by jk-dsp at the same key(s) and handed to the chain sample-aligned, as
the app core does; `--feedback` returns it into the input through
`jk_pitch::echo::testpath`. The whole proof: `q/snapshot.sh WS`, `q/build_pair.sh WS TAG`,
`q/run_final2.sh TAG` (both scorers, about 70 minutes; on a later snapshot whose tracker,
scorer and harness sources are byte-identical, `q/run_echo_parts.sh OLD TAG` re-runs only
the takes with a reference, the ones the echo code touches, and copies the rest), plus
`q/lowkey_diag.sh` and `q/run_strictness.sh`, then `python -I q/fill_doc3.py TAG WS OUT.md`
(this file).

## 4. Results (snapshot of Fri Nov 20 05:36:17 EST 2026; before = scorer 0.1.0, after = scorer 0.2.0)

### 4.1 Scripted singers through the whole chain (WO2 section 9 "Scoring")

48 kHz mono input, the echo stage on (reference rendered, nothing returned), standard
strictness, Auto octave. Grade before -> after, letter and tendency (after):

**singers: before -> after (grade, tendency after)**

| singer | demo | Feel | Crowdpleaser | Homeless Man | owner's song |
|---|---|---|---|---|---|
| perfect | 100.0 -> 100.0 A+, +0 c | 99.8 -> 100.0 A+, +0 c | 99.6 -> 99.9 A+, +0 c | 97.8 -> 98.5 A+, +0 c | 99.5 -> 99.7 A+, +0 c |
| steady | 100.0 -> 100.0 A+, +0 c | 99.9 -> 100.0 A+, +0 c | 99.4 -> 99.6 A+, +0 c | 98.7 -> 99.2 A+, +0 c | 99.8 -> 99.9 A+, +0 c |
| sharp_30 | 100.0 -> 100.0 A+, +30 c | 99.8 -> 99.9 A+, +30 c | 99.6 -> 99.9 A+, +30 c | 99.1 -> 99.6 A+, +30 c | 99.8 -> 99.9 A+, +30 c |
| sharp_70 | 61.6 -> 64.0 D, +69 c | 62.8 -> 64.7 D, +70 c | 68.4 -> 72.4 C, +68 c | 71.8 -> 76.5 C+, +65 c | 66.8 -> 70.5 C, +68 c |
| flat_100 | 7.8 -> 9.2 F, -97 c | 6.9 -> 9.7 F, -99 c | 21.3 -> 22.0 F, -100 c | 24.4 -> 26.8 F, -100 c | 10.4 -> 15.5 F, -99 c |
| octave_-1 | 100.0 -> 100.0 A+, +0 c | 65.8 -> 67.4 D, +0 c | 99.0 -> 99.8 A+, +0 c | 91.8 -> 93.4 A, +0 c | 99.7 -> 99.9 A+, +0 c |
| octave_1 | 100.0 -> 100.0 A+, +0 c | 99.7 -> 100.0 A+, +0 c | 99.4 -> 99.7 A+, +0 c | 98.0 -> 98.5 A+, +0 c | 99.5 -> 99.7 A+, +0 c |
| octave_-1-strict | 0.0 -> 0.0 F | 0.1 -> 0.1 F | 0.0 -> 0.0 F | 0.0 -> 0.0 F | 0.5 -> 0.4 F |
| late_100 | 99.9 -> 100.0 A+, +0 c | 99.7 -> 99.9 A+, +0 c | 99.6 -> 99.9 A+, +0 c | 97.8 -> 98.5 A+, +0 c | 99.5 -> 99.7 A+, +0 c |
| early_100 | 100.0 -> 100.0 A+, +0 c | 99.7 -> 100.0 A+, +0 c | 99.6 -> 99.9 A+, +0 c | 97.8 -> 98.8 A+, +0 c | 99.5 -> 99.7 A+, +0 c |
| late_400 | 73.3 -> 73.3 C, +0 c | 65.6 -> 68.1 D, +0 c | 38.8 -> 47.5 F, +0 c | 24.6 -> 29.0 F, +0 c | 64.9 -> 68.5 D, +0 c |
| one-note | 8.8 -> 8.8 F, +100 c | 31.2 -> 31.2 F, -2 c | 8.8 -> 8.8 F, -73 c | 21.2 -> 21.2 F, -34 c | 21.2 -> 21.2 F, -16 c |
| silent | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 |
| noise | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 | 0.0 -> 0.0 F, marker 0 |

- demo: hashes 523b693fce953472 / 523b693fce953472 (same), graded 40 of 42, unknown 2, trust ['latency_unmeasured'], octave-low singer's octave -1, chain x real time 90
- Feel: hashes 660d7aa385b8ca8a / 660d7aa385b8ca8a (same), graded 244 of 503, unknown 1, trust ['latency_unmeasured', 'uncertain_guide'], octave-low singer's octave -1, chain x real time 126
- Crowdpleaser: hashes a7e7acde91493397 / a7e7acde91493397 (same), graded 82 of 593, unknown 0, trust ['latency_unmeasured', 'uncertain_guide'], octave-low singer's octave -1, chain x real time 62
- Homeless Man: hashes f68a399b44b93cfa / f68a399b44b93cfa (same), graded 94 of 679, unknown 0, trust ['latency_unmeasured', 'uncertain_guide'], octave-low singer's octave -1, chain x real time 149
- owner's song: hashes b513b3221f75720d / b513b3221f75720d (same), graded 332 of 435, unknown 0, trust ['latency_unmeasured'], octave-low singer's octave -1, chain x real time 143

- Every scripted singer lands where a person would expect: perfect, steady, 30 cents sharp,
  an octave high and 100 ms early or late at the top; 70 cents sharp in the middle (0.6
  credit by the bands, with vibrato and scoops around it); a semitone flat, one note
  throughout, an octave low in strict mode, silence and noise at the bottom; 400 ms late
  clearly lower (less so on the demo's long held notes). The tendency is the scripted
  offset within 3 cents on every song.
- Before -> after: the singers who sing every note in time move by at most 1.0 point;
  70 cents sharp, a semitone flat and 400 ms late gain up to 9 points, because the singer's
  timing is now chosen per note within the tolerance (scoops and note ends no longer cost);
  each of them stays in its band.
- An octave low on Feel (about 67) puts the melody at MIDI 33-44 (55-104 Hz), mostly below
  the tracker's required range (E2, 82 Hz) and partly below its aim (C2, 65 Hz), where it
  shows few frames (the low-key table in 4.3: notes below C2 voiced 13 %); Homeless Man an
  octave low (93.4) is partly there too. The octave was detected as -1 on every song.
- The same input gives the same result: identical hashes for a repeated take on every song,
  and for 10 ms, 64-frame (with +-500 us stamp jitter) and 2048-frame blocks (4.2).
- Graded on (perfect singer): demo 40 of 42 (2 notes in the first second, before the echo
stage has decided, are "unknown"), Feel 244 of 503, Crowdpleaser 82 of 593, Homeless Man
94 of 679 (rap-heavy: `uncertain_guide`), owner's song 332 of 435. With this snapshot's
echo code some other singers lose notes to the echo stage's "cannot judge" verdict with
nothing fed back: one note throughout on Feel (181 graded) and on the demo (19), steady
and a semitone flat on the demo (31), 30 cents sharp on the demo (33), 100 ms late on
Feel (233); their grades are those of the notes graded (4.6).

### 4.2 The owner's song: stopping early, quiet singing, keys, block sizes

**the owner's song, more runs: before -> after**

| run | grade | graded / total | sung share | flags (after) | hash (after) |
|---|---|---|---|---|---|
| stop120 | 99.5 -> 99.7 | 128 / 173 | 0.41 | stopped_early | fd800c1e53fb302f |
| mid60 | 99.9 -> 100.0 | 62 / 92 | 0.23 | stopped_early | afc5612dd09b707e |
| lv40 | 99.5 -> 99.7 | 332 / 435 | 1.0 |  | b513b3221f75720d |
| lv50 | 99.5 -> 99.7 | 332 / 435 | 1.0 |  | b513b3221f75720d |
| lv56 | 27.4 -> 27.6 | 332 / 435 | 1.0 | low_level | 4fd7d8985d144d77 |
| lv62 | 0.0 -> 0.0 | 332 / 435 | 1.0 | low_level | fbbc62106e87851e |
| key-3 | 99.6 -> 99.7 | 332 / 435 | 1.0 |  | 691cb8d9fc00e715 |
| kc | 99.7 -> 99.8 | 332 / 435 | 1.0 |  | db8378c47ce0fa70 |
| kc-wrong2 | 5.6 -> 5.5 | 332 / 435 | 1.0 |  | 63a0780673ad6d96 |
| blk10 | 99.6 -> 99.7 | 332 / 435 | 1.0 |  | 02a96a8411ad2291 |
| blk64j | 99.6 -> 99.7 | 332 / 435 | 1.0 |  | 02a96a8411ad2291 |
| blk2048 | 99.6 -> 99.7 | 332 / 435 | 1.0 |  | 02a96a8411ad2291 |
| std-sharp45 | 99.6 -> 99.9 | 332 / 435 | 1.0 |  | 269e0ce77a87a90e |
| strict-sharp45 | 58.0 -> 60.8 | 332 / 435 | 1.0 |  | 542fdc30212c2f03 |
| relaxed-sharp70 | 99.8 -> 99.9 | 332 / 435 | 1.0 |  | 75f9b764bd8fd912 |

### 4.3 The original singer at key 0 and at every key (WO2 section 9 "Key change")

The original isolated lead vocal rendered by the engine at the key, fed to the chain as
the microphone (no reference), graded against the guide at that key; and the same
tracked singing against guides shifted by a wrong amount (+-1, +-2, +5, -7).

**original singer at key 0: before / after without the excusal rule / after**

| song | set | graded / total | voiced share | before (0.1.0) | after, excusal off | after (0.2.0) |
|---|---|---|---|---|---|---|
| demo | dev | 42 / 42 | 0.99 | 100.0 | 100.0 | 100.0 |
| Feel | dev | 245 / 503 | 0.85 | 96.3 | 94.2 | 97.1 |
| Crowdpleaser | dev | 82 / 593 | 0.86 | 94.4 | 98.7 | 99.2 |
| Homeless Man | dev | 94 / 679 | 0.81 | 87.6 | 94.3 | 96.1 |
| owner's song | dev | 332 / 435 | 0.92 | 97.6 | 98.4 | 99.2 |
| ccm-30483 | held-out | 294 / 608 | 0.86 | 95.0 | 96.3 | 97.2 |
| ccm-34740 | held-out | 318 / 402 | 0.93 | 94.7 | 97.8 | 98.0 |
| ccm-39381 | held-out | 303 / 406 | 0.84 | 96.1 | 99.4 | 99.9 |
| ccm-55541 | held-out | 212 / 223 | 0.96 | 98.4 | 99.9 | 99.9 |
| musdb_01 | held-out | 138 / 323 | 0.93 | 96.4 | 97.3 | 97.7 |
| musdb_06 | held-out | 182 / 351 | 0.86 | 92.9 | 94.3 | 95.6 |
| musdb_10 | held-out | 334 / 516 | 0.93 | 95.5 | 96.9 | 97.6 |
| musdb_12 | held-out | 308 / 335 | 0.97 | 98.5 | 99.5 | 99.5 |
| vocadito_5 | held-out | 70 / 82 | 0.92 | 98.1 | 100.0 | 100.0 |
| vocadito_11 | held-out | 33 / 36 | 0.96 | 98.5 | 100.0 | 100.0 |
| vocadito_13 | held-out | 63 / 77 | 0.94 | 97.4 | 99.2 | 99.2 |
| vocadito_19 | held-out | 73 / 88 | 0.94 | 98.6 | 99.8 | 99.8 |
| vocadito_23 | held-out | 40 / 54 | 0.83 | 91.8 | 94.2 | 95.1 |
| vocadito_26 | held-out | 18 / 57 | 0.98 | 89.6 | 93.3 | 93.3 |
| vocadito_30 | held-out | 19 / 45 | 0.85 | 85.8 | 93.6 | 94.6 |
| vocadito_37 | held-out | 49 / 51 | 0.98 | 99.8 | 100.0 | 100.0 |

**the original singer at every key (after): grade (voiced share) / best against a wrong guide**

| key | demo | Feel | Crowdpleaser | Homeless Man | owner's song | ccm-30483 | ccm-39381 | musdb_06 | musdb_12 |
|---:|---|---|---|---|---|---|---|---|---|
| -12 | 100.0 (0.95) / 7.3 | 62.4 (0.51) / 15.3 | 97.9 (0.75) / 28.1 | 85.2 (0.63) / 41.9 | 99.0 (0.9) / 24.4 | 96.5 (0.84) / 25.5 | 96.7 (0.77) / 28.8 | 75.5 (0.63) / 14.6 | 98.4 (0.94) / 27.5 |
| -9 | 100.0 (0.96) / 7.6 | 91.0 (0.77) / 15.7 | 98.5 (0.79) / 30.8 | 93.5 (0.73) / 45.3 | 99.0 (0.91) / 25.1 | 96.8 (0.85) / 26.3 | 98.3 (0.8) / 30.0 | 85.0 (0.74) / 16.6 | 99.2 (0.95) / 27.6 |
| -6 | 100.0 (0.98) / 7.8 | 94.4 (0.81) / 15.6 | 99.1 (0.83) / 32.5 | 95.8 (0.77) / 45.6 | 99.0 (0.92) / 25.5 | 97.0 (0.85) / 26.7 | 99.1 (0.82) / 30.3 | 92.2 (0.82) / 17.7 | 99.3 (0.96) / 28.0 |
| -3 | 100.0 (0.98) / 7.9 | 97.6 (0.86) / 15.7 | 99.3 (0.85) / 34.8 | 96.6 (0.81) / 50.4 | 99.1 (0.92) / 25.9 | 97.0 (0.86) / 27.0 | 99.7 (0.83) / 31.2 | 93.4 (0.84) / 18.0 | 99.4 (0.97) / 27.9 |
| -1 | 100.0 (0.99) / 8.0 | 98.0 (0.87) / 16.1 | 99.3 (0.88) / 36.4 | 97.1 (0.84) / 47.4 | 99.3 (0.93) / 25.7 | 97.1 (0.86) / 27.1 | 99.9 (0.85) / 31.4 | 94.1 (0.86) / 18.5 | 99.5 (0.97) / 27.8 |
| +0 | 100.0 (0.99) / 7.8 | 97.1 (0.85) / 16.0 | 99.2 (0.86) / 35.0 | 96.1 (0.81) / 50.6 | 99.2 (0.92) / 25.6 | 97.2 (0.86) / 26.9 | 99.9 (0.84) / 30.8 | 95.6 (0.86) / 18.6 | 99.5 (0.97) / 27.9 |
| +1 | 100.0 (1.0) / 8.0 | 98.0 (0.88) / 15.8 | 99.3 (0.89) / 37.4 | 96.6 (0.85) / 51.3 | 99.3 (0.93) / 25.7 | 97.2 (0.86) / 27.6 | 99.9 (0.85) / 31.4 | 95.6 (0.88) / 19.7 | 99.5 (0.97) / 28.0 |
| +3 | 100.0 (1.0) / 8.0 | 97.9 (0.87) / 16.4 | 99.3 (0.91) / 39.1 | 97.2 (0.87) / 51.0 | 99.1 (0.93) / 26.2 | 97.3 (0.86) / 27.5 | 99.9 (0.85) / 32.0 | 95.5 (0.89) / 20.2 | 99.5 (0.98) / 27.9 |
| +6 | 100.0 (1.0) / 7.8 | 98.3 (0.89) / 16.5 | 99.6 (0.93) / 39.5 | 97.4 (0.89) / 54.2 | 99.1 (0.93) / 26.4 | 97.4 (0.87) / 28.8 | 99.9 (0.85) / 32.3 | 95.6 (0.91) / 21.1 | 99.5 (0.98) / 27.9 |
| +9 | 100.0 (1.0) / 7.8 | 98.7 (0.9) / 16.6 | 99.4 (0.94) / 39.2 | 97.0 (0.92) / 55.2 | 99.1 (0.94) / 27.3 | 97.4 (0.88) / 29.4 | 100.0 (0.86) / 32.8 | 96.3 (0.93) / 21.5 | 99.5 (0.98) / 28.3 |
| +12 | 100.0 (1.0) / 7.8 | 98.7 (0.91) / 16.9 | 99.4 (0.95) / 39.0 | 97.4 (0.92) / 55.7 | 99.1 (0.94) / 28.0 | 97.4 (0.88) / 29.9 | 99.8 (0.86) / 33.4 | 96.4 (0.94) / 22.2 | 99.6 (0.98) / 28.4 |

| song | set | key 0 before -> after | min, max (after, -12..12) | largest difference from key 0: all keys / -6..+12 (before -> after) | best against a wrong guide (before -> after) |
|---|---|---|---|---|---|
| demo | dev | 100.0 -> 100.0 | 100.0, 100.0 | 0.0 / 0.0 -> 0.0 / 0.0 | 8.0 -> 8.0 |
| Feel | dev | 96.3 -> 97.1 | 62.4, 98.7 | 36.8 / 3.1 -> 34.7 / 2.7 | 11.8 -> 16.9 |
| Crowdpleaser | dev | 94.4 -> 99.2 | 97.6, 99.6 | 4.1 / 1.3 -> 1.6 / 0.4 | 31.7 -> 39.7 |
| Homeless Man | dev | 87.6 -> 96.1 | 85.2, 98.4 | 15.4 / 5.0 -> 10.9 / 2.3 | 45.6 -> 55.7 |
| owner's song | dev | 97.6 -> 99.2 | 98.8, 99.3 | 0.4 / 0.4 -> 0.4 / 0.2 | 19.8 -> 28.0 |
| ccm-30483 | held-out | 95.0 -> 97.2 | 96.5, 97.5 | 1.1 / 0.6 -> 0.7 / 0.3 | 19.8 -> 30.0 |
| ccm-39381 | held-out | 96.1 -> 99.9 | 96.7, 100.0 | 5.5 / 1.8 -> 3.2 / 0.8 | 24.6 -> 33.5 |
| musdb_06 | held-out | 92.9 -> 95.6 | 75.5, 96.7 | 22.5 / 4.0 -> 20.1 / 3.4 | 14.3 -> 22.2 |
| musdb_12 | held-out | 98.5 -> 99.5 | 98.4, 99.6 | 1.3 / 0.3 -> 1.1 / 0.2 | 18.2 -> 28.4 |
- demo with the echo stage on (karaoke mix as reference): key 0 100.0, keys untrusted []
- Feel with the echo stage on (karaoke mix as reference): key 0 97.1, keys untrusted []

- **At key 0 the original singer is in the top band (A+) on every development song**:
  demo 100.0, Feel 97.1, Crowdpleaser 99.2, Homeless Man 96.1, owner's song 99.2 (Stage 3a doc: 100.0, 88.4, 91.8, 82.7, 97.8; the 0.1.0 scorer on this
  snapshot: demo 100.0, Feel 96.3, Crowdpleaser 94.4, Homeless Man 87.6, owner's song 97.6). Held-out: 14 of 16 in the top band; the
  others are vocadito 26 and 30 (18 and 19 graded notes; where they lose is in 4.9).
- **Across keys, from -6 to +12** every development song stays within 3 points of key 0
  (the demo now included: 100.0 at every key), and the held-out songs too, except
  musdb_06 at -6 (3.4 below). Below -6 the low male voices drop as the melody leaves the
  tracker's range: Feel from -7 (93.3) to -12 (62.4), Homeless Man at -11 and -12 (88.1,
  85.2), held-out musdb_06 from -8 (87.2) to -12 (75.5). The table below shows where.
- **Against a guide shifted by a wrong amount every song scores in the lowest band** at
  every key: at most 55.7 (Homeless Man against a guide moved by -1 at key +12) (before: 45.6 (Homeless Man against a guide moved by -1 at key +10)); held-out at most
  33.5 (ccm-39381 against a guide moved by -1 at key +11). The wrong-guide grades rose with 0.2.0 on songs of short notes that move
  by semitone steps: there the timing freedom lets a note meet its neighbour's pitch, which
  is a semitone away (the "behind" and "ahead" cases of 4.7, sung by the original singer
  against a guide moved by one semitone).
- With the echo stage on (the karaoke mix as the reference, nothing fed back): no key of the demo or Feel is untrusted, but the echo stage declares the music present at
14 of 25 keys of the demo (its karaoke mix carries some of its own lead vocal) and then
leaves notes unjudged or removes part of the singer: the demo's grade differs from the
plain sweep by more than a point at keys -6 (65.0), -1 (61.0) and +2 (89.0, 28 of 42 notes
not judged); Feel only at -4 (96.1 against 97.7) and by up to 1.4 at -12 and -10. The
echo workstream's (section 5).

**Where the original singer loses credit at the lowest keys** (`q/lowkey_diag.sh`,
`python -I q/lowkey_table.py q/s3-lowkey`): the same renders graded with per-note
detail, the graded notes grouped by their target pitch at the key heard (WO2 4.4: the
tracker must cover E2 to C6 and aim for C2). A cell is the share of graded note time, the
grade of those notes, and the share of their judged moments where the tracker named a
pitch:

| song | key | overall | voiced share | below C2 (< 65 Hz): time share, grade, voiced | C2-E2 (65-82 Hz): time share, grade, voiced | E2 and up: time share, grade, voiced | grade on notes from C2 | grade on notes from E2 |
|---|---:|---:|---:|---|---|---|---:|---:|
| ccm-30483 | -12 | 96.5 | 0.84 | - | - | 100 %, 96.5, 0.84 | 96.5 | 96.5 |
| ccm-30483 | -11 | 96.7 | 0.84 | - | - | 100 %, 96.7, 0.84 | 96.7 | 96.7 |
| ccm-30483 | -9 | 96.8 | 0.85 | - | - | 100 %, 96.8, 0.85 | 96.8 | 96.8 |
| ccm-30483 | -6 | 97.0 | 0.85 | - | - | 100 %, 97.0, 0.85 | 97.0 | 97.0 |
| ccm-30483 | +0 | 97.2 | 0.86 | - | - | 100 %, 97.2, 0.86 | 97.2 | 97.2 |
| crowd | -12 | 97.9 | 0.75 | 1 %, 100.0, 0.46 | 52 %, 96.9, 0.77 | 47 %, 99.1, 0.73 | 97.9 | 99.1 |
| crowd | -11 | 98.0 | 0.76 | - | 43 %, 97.6, 0.79 | 57 %, 98.3, 0.75 | 98.0 | 98.3 |
| crowd | -9 | 98.5 | 0.79 | - | 10 %, 96.1, 0.72 | 90 %, 98.8, 0.80 | 98.5 | 98.8 |
| crowd | -6 | 99.1 | 0.83 | - | - | 100 %, 99.1, 0.83 | 99.1 | 99.1 |
| crowd | +0 | 99.2 | 0.86 | - | - | 100 %, 99.2, 0.86 | 99.2 | 99.2 |
| feel | -12 | 62.4 | 0.51 | 45 %, 21.3, 0.13 | 39 %, 93.9, 0.78 | 16 %, 98.6, 0.88 | 95.3 | 98.6 |
| feel | -11 | 80.1 | 0.68 | 35 %, 50.1, 0.42 | 44 %, 95.5, 0.81 | 21 %, 98.1, 0.87 | 96.4 | 98.1 |
| feel | -9 | 91.0 | 0.77 | 11 %, 53.6, 0.41 | 52 %, 94.9, 0.80 | 37 %, 96.7, 0.83 | 95.7 | 96.7 |
| feel | -6 | 94.4 | 0.81 | 4 %, 34.2, 0.24 | 13 %, 94.3, 0.78 | 83 %, 97.6, 0.84 | 97.1 | 97.6 |
| feel | +0 | 97.1 | 0.85 | - | 0 %, 100.0, 0.50 | 100 %, 97.1, 0.85 | 97.1 | 97.1 |
| homeless | -12 | 85.2 | 0.63 | 10 %, 40.9, 0.25 | 51 %, 91.6, 0.68 | 39 %, 87.8, 0.67 | 90.0 | 87.8 |
| homeless | -11 | 88.1 | 0.69 | 6 %, 16.6, 0.11 | 42 %, 93.4, 0.70 | 52 %, 91.4, 0.74 | 92.3 | 91.4 |
| homeless | -9 | 93.5 | 0.73 | 2 %, 0.0, 0.06 | 20 %, 98.4, 0.71 | 78 %, 94.9, 0.76 | 95.6 | 94.9 |
| homeless | -6 | 95.8 | 0.77 | - | 4 %, 100.0, 0.73 | 96 %, 95.6, 0.78 | 95.8 | 95.6 |
| homeless | +0 | 96.1 | 0.81 | - | - | 100 %, 96.1, 0.81 | 96.1 | 96.1 |
| musdb_06 | -12 | 75.5 | 0.63 | 26 %, 32.6, 0.27 | 17 %, 98.0, 0.79 | 57 %, 88.2, 0.75 | 90.5 | 88.2 |
| musdb_06 | -11 | 81.1 | 0.69 | 15 %, 19.6, 0.23 | 24 %, 97.7, 0.80 | 61 %, 89.4, 0.76 | 91.7 | 89.4 |
| musdb_06 | -9 | 85.0 | 0.74 | 7 %, 20.0, 0.15 | 20 %, 86.4, 0.82 | 73 %, 91.1, 0.78 | 90.1 | 91.1 |
| musdb_06 | -6 | 92.2 | 0.82 | 4 %, 0.0, 0.19 | 6 %, 95.4, 0.79 | 91 %, 95.9, 0.85 | 95.9 | 95.9 |
| musdb_06 | +0 | 95.6 | 0.86 | - | 3 %, 71.0, 0.64 | 97 %, 96.5, 0.87 | 95.6 | 96.5 |

- **The loss at the lowest keys is in notes the tracker cannot hear.** Feel at -12 has
  45 % of its graded note time below C2 (MIDI 33-35, 55-65 Hz), where the tracker named a
  pitch on 13 % of the moments; those notes score 21, the rest 95.3 (from C2) and 98.6
  (from E2) against 97.1 at key 0. At -11 and -9 the same: the notes from C2 score 96.4
  and 95.7. WO2 4.4 requires the tracker from E2 and aims for C2, so these notes lie
  outside what it was asked to hear. A person singing Feel 12 semitones down sings it an
  octave up, in a range people sing in: that is the key-0 melody, where Auto moves the lane
  and the original singer scores 97.1.
- **Homeless Man and musdb_06 at -12 also lose above E2** (87.8 and 88.2 against 96.1 and
  96.5 at key 0; the tracker names a pitch on 67 % and 75 % of those moments against 81 %
  and 87 % at key 0): these two voices, shifted an octave down by the engine, are voiced
  less by the tracker even where their pitch is in range. That is the tracker's voicing of
  low voices (practice-tracker.md 22.4 "not met at the lowest keys").
- Crowdpleaser (52 % of its time between C2 and E2 at -12) and ccm-30483 stay within 1.3
  points of key 0 at every key: the tracker hears C2-E2 well on those voices.
- Considered and not done: a scorer rule that leaves notes below C2 (in the octave the lane
  shows) ungraded, as moments the system cannot judge. It would give Feel at -12 about 95,
  but the lane's octave is the singer's own choice in Fixed mode, so a singer could move
  the lane down to drop the notes they cannot sing from the grade, and the result would need
  a new trust flag (SONG_FORMAT 11, the app's messages) to say why fewer notes were graded.
  The grade keeps charging those notes; section 5 lists it as the tracker's open item.

### 4.4 Key change during a song, both key modes (WO2 section 9)

The lead vocal played in real time by the engine (null device) with key changes
requested during playback, captured with the key timeline the engine published
(`captures/`, Stage 3a: real time, six changes heard about 0.28-0.29 s after the request;
render ahead, four changes heard 1.3-2.7 s after it), graded against the keys as heard
(D-239) and as asked (trap 1's wrong timeline), and the same stretch at key 0 throughout:

**key change during a song: before -> after**

| song | mode | changes | graded | keys as heard | keys as asked | same stretch, key 0 | notes whose key differs: credit heard / asked (after) |
|---|---|---:|---:|---:|---:|---:|---|
| Feel | realtime | 6 | 37 | 93.7 -> 98.5 | 88.0 -> 93.4 | 96.5 -> 100.0 | 2 notes (0.5 s): 93 / 19 |
| Feel | prerender | 4 | 51 | 93.4 -> 98.9 | 70.6 -> 75.1 | 95.9 -> 99.5 | 14 notes (3.6 s): 99 / 2 |
| owner's song | realtime | 6 | 45 | 97.3 -> 98.5 | 89.9 -> 92.9 | 98.0 -> 99.3 | 4 notes (0.8 s): 100 / 39 |
| owner's song | prerender | 4 | 65 | 97.4 -> 98.5 | 82.1 -> 82.7 | 97.8 -> 99.2 | 12 notes (4.1 s): 97 / 14 |
| Homeless Man | realtime | 6 | 28 | 92.4 -> 98.3 | 87.0 -> 94.2 | 93.5 -> 100.0 | 2 notes (0.4 s): 97 / 50 |
| Homeless Man | prerender | 4 | 28 | 93.0 -> 100.0 | 67.4 -> 72.0 | 93.5 -> 100.0 | 7 notes (1.4 s): 100 / 12 |
| Crowdpleaser | realtime | 6 | 23 | 87.9 -> 96.2 | 82.6 -> 90.8 | 87.3 -> 97.3 | 3 notes (0.3 s): 67 / 14 |
| Crowdpleaser | prerender | 4 | 23 | 87.7 -> 96.4 | 57.3 -> 61.0 | 87.3 -> 97.3 | 7 notes (1.4 s): 95 / 5 |

- The notes sung between the click and the moment the change was heard are graded in the
  key that was sounding: 93-100 % credit with the heard timeline against 2-50 % with the
  asked one (Crowdpleaser real time: 3 notes of 0.1 s, 67 % against 14 %).
- With the heard timeline the grade across six or four key changes is within
  1.7 points of the same stretch at key 0 on every song and in both modes (Stage 3a
  doc: Feel lost 4-16 points; both the tracker and the scorer have changed since).
- That the published switch matches the pitch change in the produced audio to within
  0.02 ms is D-239's measurement (`docs/benchmarks/practice-key.md`); this section shows
  the scorer uses it: `KeyTimeline::key_at` per moment, per segment.

### 4.5 The owner's rig: channels, rates, reverb, compression

Each synthetic singer (perfect, +70 c, -100 c, an octave low, silent) on the demo, Feel
and the owner's song, against the clean 48 kHz mono take of 4.1:

**the owner's rig (after; change from the clean 48 kHz mono take in brackets)**

| input | largest change, after (before) | without Feel an octave low, after (before) | perfect: demo / Feel / owner (after) |
|---|---:|---:|---|
| 44.1 kHz, voice on ch 2 of 2, noise on ch 1 | 0.4 (0.4) | 0.4 (0.4) | 100.0 / 100.0 / 99.7 |
| 48 kHz, voice on ch 1, the song on ch 2 | 0.0 (0.0) | 0.0 (0.0) | 100.0 / 100.0 / 99.7 |
| 96 kHz, voice on ch 2, ch 1 silent | 0.2 (0.1) | 0.1 (0.1) | 100.0 / 100.0 / 99.7 |
| compressor, moderate | 0.5 (0.5) | 0.5 (0.5) | 100.0 / 100.0 / 99.7 |
| compressor, heavy | 0.5 (0.9) | 0.5 (0.9) | 100.0 / 100.0 / 99.7 |
| reverb RT60 0.6 s, -6 dB wet | 6.2 (5.5) | 6.2 (5.5) | 100.0 / 99.5 / 99.5 |
| reverb RT60 1.2 s, -3 dB wet | 11.1 (12.2) | 6.7 (6.6) | 100.0 / 97.5 / 98.8 |
| 96 kHz, ch 2, noise, reverb 0.8 s + heavy compressor | 12.2 (12.9) | 8.5 (8.1) | 100.0 / 97.7 / 99.6 |

- silent: grade [0.0], marker values [0] over 24 variants

Silent stays 0 with no marker in every variant. The channel is never summed: the song on
the other channel changes nothing. Channel layout, sample rate and compression move a grade
by at most 0.5 points; reverb costs up to 6.2 points at RT60 0.6 s and up to 11.1 at 1.2 s
(Feel's octave-low singer, out of the tracker's range; 6.7 without it), which is the
tracker's open reverb item (D-244). The scorer moves these differences by about a point
at most (before against after).

### 4.6 The song fed back into the input, and music flags with nothing fed back

The app's playback returned into the singer's channel at -20, -10, -3 and 0 dB (USB:
12 ms), -6 dB with EQ (8 ms), cable at -10 and -3 dB (25 ms, 60 ppm drift) and Bluetooth
at -6 dB (wandering around 180 ms), guide vocal off and on. Grade change from the clean
take (after; n/g = not graded, the result says why):

**the song fed back into the input (after)**

| return | guide vocal | silent: grade, marker values (demo / Feel / owner) | perfect vs clean (demo / Feel / owner) | +70 c vs clean | octave vs clean | delay measured / true (owner) |
|---|---|---|---|---|---|---|
| -20,12,usb | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.0 / -0.2 / -0.1 | +0.0 / -0.7 / +0.0 | 12.0 (returned_music) / 12.0 |
| -20,12,usb | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.5 / -0.2 / -0.2 | +0.0 / -0.7 / +0.0 | 12.0 (returned_music) / 12.0 |
| -10,12,usb | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.1 / -0.2 / -0.2 | +0.0 / -0.7 / +0.0 | 12.0 (returned_music) / 12.0 |
| -10,12,usb | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +5.0 / -0.2 / -0.3 | +0.0 / -0.7 / +0.0 | 12.0 (returned_music) / 12.0 |
| -3,12,usb | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.5 / -0.3 / -0.9 | +0.0 / -0.7 / +0.0 | 12.0 (returned_music) / 12.0 |
| -3,12,usb | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | n/g / +0.0 / n/g | n/g / -0.4 / n/g | -0.1 / -1.7 / -0.2 | 12.0 (returned_music) / 12.0 |
| 0,12,usb | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.1 | +0.7 / -0.3 / -1.1 | +0.0 / -0.7 / -0.1 | 12.0 (returned_music) / 12.0 |
| 0,12,usb | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | n/g / +0.0 / n/g | n/g / -0.7 / n/g | n/g / -3.4 / n/g | 12.0 (returned_music) / 12.0 |
| -6,8,usb_eq | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.9 / +0.0 / -0.4 | +0.0 / +0.4 / +0.0 | 8.0 (returned_music) / 8.0 |
| -6,8,usb_eq | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | -0.4 / +0.0 / n/g | +8.9 / +0.0 / -0.5 | +0.0 / +0.5 / -0.1 | 8.0 (returned_music) / 8.0 |
| -10,25,cable | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +0.8 / +0.1 / -0.2 | +0.0 / +0.1 / +0.0 | 39.2 (returned_music) / 32.2 |
| -10,25,cable | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +3.2 / +0.0 / -0.3 | +0.0 / +0.1 / -0.1 | 39.2 (returned_music) / 32.2 |
| -3,25,cable | 0 | 0.0, 0 / 0.0, 0 / 0.0, 0 | +0.0 / +0.0 / +0.0 | +1.1 / +0.0 / -0.9 | +0.0 / +0.3 / -0.1 | 39.2 (returned_music) / 32.2 |
| -3,25,cable | 1 | 0.0, 0 / 0.0, 0 / 0.0, 0 | n/g / +0.0 / n/g | n/g / -0.1 / n/g | n/g / -1.4 / -0.2 | 39.2 (returned_music) / 32.2 |
| -6,180,bluetooth | 0 | n/g, 0 / n/g, 0 / n/g, 5 | n/g / n/g / n/g | n/g / n/g / n/g | n/g / n/g / n/g | None (none) / 121.3 |
| -6,180,bluetooth | 1 | n/g, 68 / n/g, 0 / n/g, 12 | n/g / n/g / n/g | n/g / n/g / n/g | n/g / n/g / n/g | None (none) / 121.3 |

**music flags and unjudged notes on takes with nothing fed back (after)**

- singers: 75 takes, 2 with a music flag [('demo-one-note.json', ['music_in_input', 'removal_active', 'latency_unmeasured'], 2.4), ('demo-steady.json', ['music_in_input', 'removal_marginal', 'latency_unmeasured'], 14.9)]; notes left unjudged by the echo stage 176 of 11925
- rig: 120 takes, 5 with a music flag [('demo-perfect-compm.json', ['music_in_input', 'removal_active', 'latency_unmeasured'], 17.5), ('feel-octave_-1-comph.json', ['music_in_input', 'removal_active', 'latency_unmeasured', 'uncertain_guide'], 12.4), ('feel-octave_-1-rev_comp_96_c1.json', ['music_in_input', 'removal_active', 'latency_unmeasured', 'uncertain_guide'], 12.8), ('feel-sharp_70-comph.json', ['music_in_input', 'removal_active', 'latency_unmeasured', 'uncertain_guide'], 12.4), ('feel-sharp_70-rev_comp_96_c1.json', ['music_in_input', 'removal_active', 'latency_unmeasured', 'uncertain_guide'], 12.7)]; notes left unjudged by the echo stage 417 of 24760

- **Nobody singing:** the grade is 0.0 with no marker for every USB and cable return, at
  every level, with the guide vocal off or on. Bluetooth: no grade (`untrusted`), and the
  marker showed 0-68 values in a take (practice over Bluetooth is not promised, APP_API
  12.5).
- **With singers, USB and cable at -20 and -10 dB, and USB with EQ:** within 1 point of the
  clean take wherever graded, except +70 cents on the demo with the guide vocal on (+3 to
  +9: the guide vocal is the same synthetic voice the singer uses, 70 cents away). Flagged
  `music_in_input` with `removal_active` or `removal_marginal`.
- **Loud returns (-3 and 0 dB):** with the guide vocal off every take is graded, within
  1.1 points of the clean take (the first measurement: not graded for most singers on Feel
  and the owner's song); with the guide vocal on, the echo stage calls the removal unusable
  (`untrusted`, no grade, the result says why) on the demo and the owner's song (16 takes,
  with the owner's song at -6 dB with EQ), and Feel is graded (within 3.4).
- Whether a take with returned music is graded is the echo stage's verdict; the scorer only
  relays it (2.4).
- **Measured delay against the true one:** USB 12.0 / 12.0 ms, USB with EQ 8.0 / 8.0;
  cable 39.2 ms at the end of the owner's song against 39.4 ms true there (the cable's
  delay drifts from 25 ms by 60 ppm; the table's "true" column is its value at mid-take,
  32.2 ms); used as `latency.source` `returned_music`. Bluetooth: never used for the timing
  (`latency.source` `none`).
- **Takes with nothing fed back:** the same 7 of 195 carry a music flag as in the first
  measurement (the echo stage declares the return present for 2-18 s where the synthetic
  singer is in unison with the song's own vocal parts or under heavy compression). More of
  their notes are now left unjudged by the echo stage (593 of about 36,700 against 139): it
  answers "cannot judge" while a detection is on probation. Those notes are not graded,
  and nothing but "graded on X of Y notes" tells the singer so. The echo workstream's.

### 4.7 Timing fairness: rubato and borrowing (new)

Generated singers, no reference; the same generated input graded by both scorers.

**timing singers and adversaries, development songs (seeds 1, 2): before / after**

| singer | demo | Feel | Crowdpleaser | Homeless Man | owner's song |
|---|---|---|---|---|---|
| perfect | 100.0 / 100.0 | 99.9 / 100.0 | 99.5 / 100.0 | 97.7 / 98.3 | 99.6 / 99.7 |
| rubato:60:1 | 99.9 / 100.0 | 98.6 / 99.6 | 97.3 / 99.6 | 95.2 / 95.7 | 98.7 / 99.7 |
| rubato:60:2 | 99.8 / 100.0 | 98.9 / 99.6 | 96.6 / 98.8 | 94.3 / 96.5 | 99.0 / 99.8 |
| rubato:120:1 | 98.5 / 100.0 | 96.1 / 99.2 | 90.9 / 97.0 | 87.3 / 95.3 | 95.9 / 99.5 |
| rubato:120:2 | 97.5 / 100.0 | 96.2 / 99.1 | 90.2 / 97.0 | 87.2 / 95.5 | 96.1 / 99.5 |
| rubato:150:1 | 96.6 / 100.0 | 93.8 / 98.5 | 86.4 / 96.3 | 82.2 / 94.9 | 93.6 / 99.3 |
| rubato:150:2 | 95.2 / 100.0 | 94.0 / 98.5 | 83.9 / 95.9 | 83.6 / 94.2 | 93.8 / 99.5 |
| behind | 49.4 / 49.3 | 45.5 / 47.5 | 51.7 / 55.3 | 56.2 / 59.8 | 55.3 / 56.6 |
| behind:150 | 64.4 / 64.3 | 50.5 / 57.0 | 56.0 / 65.8 | 59.5 / 69.1 | 62.2 / 68.1 |
| ahead:150 | 65.9 / 68.1 | 62.8 / 70.4 | 72.3 / 80.5 | 55.7 / 64.7 | 61.4 / 68.4 |
| swap | 39.9 / 45.3 | 47.1 / 50.2 | 52.0 / 52.2 | 41.5 / 45.1 | 46.7 / 50.8 |
| staccato:0.5 | 58.0 / 78.4 | 59.0 / 73.2 | 72.7 / 91.0 | 74.6 / 95.4 | 67.0 / 81.9 |
| gap:0.25:silence | 83.2 / 84.3 | 82.1 / 81.8 | 92.3 / 93.2 | 93.8 / 95.5 | 86.8 / 88.5 |
| gap:0.25:breath | 92.6 / 81.4 | 89.1 / 79.5 | 90.5 / 90.7 | 88.4 / 90.6 | 91.2 / 85.0 |
| gap:0.25:speech | 95.4 / 83.4 | 92.0 / 80.3 | 91.7 / 90.9 | 87.9 / 92.1 | 94.9 / 86.0 |
| late:150 | 100.0 / 100.0 | 99.8 / 99.8 | 99.3 / 99.5 | 97.3 / 97.5 | 99.4 / 99.4 |
| late:400 | 74.6 / 74.5 | 67.2 / 68.9 | 39.1 / 48.1 | 24.4 / 28.5 | 65.0 / 68.2 |
| flat:100 | 7.0 / 9.2 | 6.7 / 9.4 | 21.3 / 21.9 | 23.3 / 26.7 | 10.4 / 15.3 |
| one-note | 16.7 / 16.7 | 34.3 / 34.3 | 8.8 / 8.8 | 21.9 / 21.9 | 21.2 / 21.2 |

**timing singers and adversaries, held-out songs (seeds 3, 4): before / after**

| singer | ccm-30483 | ccm-34740 | ccm-39381 | ccm-55541 | musdb_01 | musdb_06 | musdb_10 | musdb_12 |
|---|---|---|---|---|---|---|---|---|
| perfect | 99.8 / 100.0 | 99.6 / 99.9 | 99.7 / 99.9 | 99.7 / 100.0 | 99.3 / 99.6 | 99.7 / 99.9 | 99.2 / 99.9 | 99.6 / 99.9 |
| rubato:60:3 | 98.9 / 99.9 | 98.6 / 99.5 | 98.6 / 99.7 | 98.5 / 99.9 | 98.9 / 99.6 | 97.9 / 99.0 | 98.0 / 99.6 | 98.0 / 99.6 |
| rubato:60:4 | 99.3 / 99.9 | 99.0 / 99.9 | 98.4 / 99.5 | 98.7 / 99.9 | 99.0 / 99.6 | 98.1 / 99.5 | 98.4 / 99.6 | 97.7 / 99.3 |
| rubato:120:3 | 95.4 / 99.6 | 96.0 / 99.4 | 94.4 / 99.0 | 95.6 / 99.6 | 97.0 / 99.6 | 94.8 / 98.6 | 94.5 / 99.2 | 94.2 / 99.1 |
| rubato:120:4 | 96.2 / 99.7 | 96.0 / 99.6 | 93.9 / 98.5 | 96.3 / 99.9 | 97.1 / 99.5 | 94.1 / 99.0 | 95.1 / 99.4 | 94.7 / 98.6 |
| rubato:150:3 | 92.6 / 99.1 | 94.0 / 99.3 | 90.9 / 98.3 | 93.3 / 99.5 | 94.9 / 99.0 | 92.1 / 97.8 | 91.8 / 98.8 | 91.2 / 98.7 |
| rubato:150:4 | 93.9 / 99.5 | 93.9 / 99.4 | 90.4 / 98.1 | 93.9 / 99.7 | 95.7 / 99.1 | 91.1 / 98.9 | 92.1 / 99.0 | 91.8 / 98.2 |
| behind | 56.3 / 57.6 | 49.5 / 51.4 | 52.6 / 56.7 | 31.4 / 32.5 | 29.4 / 30.3 | 36.0 / 37.4 | 44.6 / 45.7 | 32.7 / 33.6 |
| behind:150 | 69.5 / 75.1 | 57.7 / 64.7 | 61.5 / 71.5 | 46.4 / 50.9 | 37.9 / 41.8 | 50.1 / 56.3 | 56.5 / 63.5 | 43.8 / 49.3 |
| ahead:150 | 65.7 / 73.2 | 55.3 / 61.1 | 60.0 / 68.0 | 48.2 / 53.8 | 34.9 / 39.2 | 50.1 / 59.3 | 59.0 / 68.4 | 49.2 / 57.2 |
| swap | 50.0 / 54.9 | 37.1 / 43.0 | 36.7 / 44.2 | 23.9 / 27.2 | 18.9 / 22.7 | 27.0 / 31.4 | 33.5 / 37.7 | 24.2 / 30.0 |
| staccato:0.5 | 69.0 / 84.9 | 68.1 / 81.5 | 72.8 / 90.6 | 64.3 / 84.1 | 60.5 / 72.1 | 62.5 / 82.4 | 68.5 / 83.5 | 65.5 / 80.6 |
| gap:0.25:silence | 88.6 / 90.3 | 86.7 / 88.5 | 91.0 / 92.0 | 87.8 / 88.7 | 82.4 / 82.5 | 84.2 / 84.9 | 87.5 / 88.9 | 86.1 / 86.7 |
| gap:0.25:breath | 92.2 / 86.6 | 91.7 / 84.9 | 92.0 / 88.9 | 91.5 / 84.1 | 92.0 / 80.3 | 88.1 / 81.4 | 91.8 / 84.7 | 90.8 / 83.4 |
| gap:0.25:speech | 95.0 / 87.3 | 94.3 / 86.3 | 94.2 / 90.0 | 95.2 / 84.8 | 93.4 / 81.0 | 90.5 / 82.2 | 94.4 / 86.1 | 92.9 / 84.4 |
| late:150 | 99.5 / 99.6 | 99.4 / 99.5 | 99.6 / 99.6 | 99.3 / 99.4 | 99.1 / 99.1 | 99.5 / 99.4 | 98.9 / 99.1 | 99.4 / 99.5 |
| late:400 | 59.2 / 60.5 | 60.7 / 67.0 | 46.5 / 50.5 | 48.5 / 49.5 | 67.8 / 68.7 | 48.2 / 50.6 | 53.9 / 57.4 | 52.1 / 55.2 |
| flat:100 | 10.3 / 15.4 | 10.2 / 14.8 | 14.5 / 19.3 | 14.6 / 18.2 | 6.4 / 7.8 | 9.1 / 12.0 | 13.1 / 17.4 | 9.0 / 15.3 |
| one-note | 8.3 / 8.3 | 21.4 / 21.4 | 14.7 / 14.7 | 19.8 / 19.8 | 34.4 / 34.4 | 11.6 / 11.6 | 29.0 / 29.1 | 14.2 / 14.2 |

**rubato and staccato: points lost against the perfect singer, notes shorter than 0.2 s / the rest (after)**

| song | share of graded time in notes < 0.2 s | rubato:60:1 | rubato:120:1 | rubato:150:1 | rubato:150:2 | staccato:0.5 |
|---|---:|---|---|---|---|---|
| demo | 0 % | - / 0.0 | - / 0.0 | - / 0.0 | - / 0.0 | - / 21.6 |
| Feel | 12 % | 3.1 / 0.0 | 6.2 / 0.1 | 10.4 / 0.3 | 11.4 / 0.1 | 0.9 / 30.5 |
| Crowdpleaser | 37 % | 1.0 / 0.0 | 8.1 / 0.0 | 10.0 / 0.0 | 9.9 / 0.7 | 2.8 / 12.7 |
| Homeless Man | 62 % | 4.2 / 0.0 | 4.8 / 0.0 | 5.6 / 0.0 | 6.4 / 0.4 | 2.2 / 4.2 |
| owner's song | 11 % | 0.3 / 0.0 | 2.0 / 0.0 | 2.9 / 0.1 | 1.8 / 0.1 | -1.2 / 20.2 |

| song | share of graded time in notes < 0.2 s | rubato:60:3 | rubato:120:3 | rubato:150:3 | rubato:150:4 | staccato:0.5 |
|---|---:|---|---|---|---|---|
| ccm-30483 | 13 % | 0.9 / 0.0 | 3.1 / 0.0 | 6.4 / 0.0 | 3.9 / 0.0 | 0.5 / 17.2 |
| ccm-34740 | 14 % | 2.6 / 0.0 | 3.6 / 0.0 | 4.5 / 0.0 | 3.5 / 0.0 | -0.3 / 21.3 |
| ccm-39381 | 29 % | 0.6 / 0.0 | 3.1 / 0.0 | 5.4 / 0.0 | 5.8 / 0.1 | 0.9 / 12.9 |
| ccm-55541 | 8 % | 1.3 / 0.0 | 3.8 / 0.1 | 3.1 / 0.2 | 2.3 / 0.1 | 0.2 / 17.2 |
| musdb_01 | 7 % | 1.0 / -0.0 | 0.3 / -0.0 | 8.6 / 0.0 | 7.4 / -0.0 | -0.3 / 29.6 |
| musdb_06 | 13 % | 6.9 / 0.0 | 9.9 / 0.1 | 13.1 / 0.4 | 7.6 / 0.1 | 6.9 / 19.1 |
| musdb_10 | 19 % | 1.5 / 0.0 | 3.7 / 0.0 | 5.1 / 0.2 | 4.6 / 0.0 | 0.5 / 20.0 |
| musdb_12 | 18 % | 2.1 / 0.0 | 4.2 / 0.1 | 5.9 / 0.3 | 9.7 / 0.0 | 2.2 / 23.0 |

**the order rule: the same singers with it / without it (each note its own timing within 150 ms)**

| singer | demo | Feel | Crowdpleaser | Homeless Man | owner's song | ccm-30483 | ccm-34740 | ccm-39381 | ccm-55541 | musdb_01 | musdb_06 | musdb_10 | musdb_12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| perfect | 100.0 / 100.0 | 100.0 / 100.0 | 100.0 / 100.0 | 98.3 / 98.5 | 99.7 / 99.8 | 100.0 / 100.0 | 99.9 / 99.9 | 99.9 / 99.9 | 100.0 / 100.0 | 99.6 / 99.6 | 99.9 / 99.9 | 99.9 / 99.9 | 99.9 / 99.9 |
| rubato:150 | 100.0 / 100.0 | 98.5 / 98.9 | 96.3 / 96.6 | 94.9 / 95.6 | 99.3 / 99.4 | 99.1 / 99.3 | 99.3 / 99.4 | 98.3 / 98.8 | 99.5 / 99.6 | 99.0 / 99.1 | 97.8 / 98.4 | 98.8 / 99.0 | 98.7 / 98.9 |
| behind | 49.3 / 49.3 | 47.5 / 49.2 | 55.3 / 58.9 | 59.8 / 62.5 | 56.6 / 58.4 | 57.6 / 58.1 | 51.4 / 54.6 | 56.7 / 59.0 | 32.5 / 35.1 | 30.3 / 32.5 | 37.4 / 39.0 | 45.7 / 48.1 | 33.6 / 36.1 |
| behind:150 | 64.3 / 64.3 | 57.0 / 57.3 | 65.8 / 67.7 | 69.1 / 69.6 | 68.1 / 68.5 | 75.1 / 75.3 | 64.7 / 65.0 | 71.5 / 71.9 | 50.9 / 51.5 | 41.8 / 42.1 | 56.3 / 56.5 | 63.5 / 64.0 | 49.3 / 49.5 |
| ahead:150 | 68.1 / 68.1 | 70.4 / 70.8 | 80.5 / 81.1 | 64.7 / 66.0 | 68.4 / 68.8 | 73.2 / 73.3 | 61.1 / 61.6 | 68.0 / 68.8 | 53.8 / 54.2 | 39.2 / 39.4 | 59.3 / 59.5 | 68.4 / 68.7 | 57.2 / 57.4 |
| swap | 45.3 / 51.6 | 50.2 / 55.3 | 52.2 / 63.8 | 45.1 / 55.1 | 50.8 / 56.2 | 54.9 / 60.0 | 43.0 / 48.6 | 44.2 / 52.6 | 27.2 / 34.6 | 22.7 / 26.9 | 31.4 / 39.0 | 37.7 / 46.5 | 30.0 / 39.1 |

What each borrowing adversary could get, computed from the guide alone (`q/adversary_ceiling.py`): for each graded note the best share of a rendition within the tolerance (both ends free, at least half the note) that the adversary fills with a pitch near the note (the default bands), each note on its own, with no order rule - an upper bound for any rule that forgives 150 ms; and the same with no timing freedom at all. Scorer (after) / no timing freedom / ceiling within 150 ms:

| song | behind | behind:150 | ahead:150 | swap |
|---|---|---|---|---|
| demo | 49.3 / 37.6 / 47.7 | 64.3 / 44.4 / 62.7 | 68.1 / 44.4 / 65.9 | 45.3 / 37.6 / 48.2 |
| Feel | 47.5 / 34.5 / 48.6 | 57.0 / 41.2 / 56.7 | 70.4 / 54.3 / 71.0 | 50.2 / 40.4 / 55.3 |
| Crowdpleaser | 55.3 / 25.4 / 55.8 | 65.8 / 41.2 / 66.3 | 80.5 / 58.8 / 81.1 | 52.2 / 29.4 / 63.9 |
| Homeless Man | 59.8 / 20.8 / 61.8 | 69.1 / 45.6 / 71.6 | 64.7 / 42.1 / 66.6 | 45.1 / 20.0 / 55.8 |
| owner's song | 56.6 / 39.0 / 57.0 | 68.1 / 49.8 / 66.7 | 68.4 / 49.0 / 68.7 | 50.8 / 37.5 / 55.9 |
| ccm-30483 | 57.6 / 33.4 / 55.5 | 75.1 / 51.3 / 72.9 | 73.2 / 54.8 / 73.3 | 54.9 / 40.0 / 60.0 |
| ccm-34740 | 51.4 / 30.2 / 52.5 | 64.7 / 43.7 / 62.5 | 61.1 / 42.6 / 62.1 | 43.0 / 27.2 / 48.5 |
| ccm-39381 | 56.7 / 30.9 / 55.9 | 71.5 / 43.8 / 70.1 | 68.0 / 43.9 / 68.7 | 44.2 / 25.3 / 50.8 |
| ccm-55541 | 32.5 / 12.1 / 34.8 | 50.9 / 29.0 / 50.4 | 53.8 / 31.5 / 52.7 | 27.2 / 11.7 / 34.2 |
| musdb_01 | 30.3 / 17.3 / 31.8 | 41.8 / 26.5 / 40.9 | 39.2 / 24.0 / 40.0 | 22.7 / 12.7 / 27.5 |
| musdb_06 | 37.4 / 16.2 / 38.6 | 56.3 / 32.3 / 54.8 | 59.3 / 35.1 / 59.4 | 31.4 / 16.4 / 39.4 |
| musdb_10 | 45.7 / 18.0 / 46.3 | 63.5 / 40.3 / 62.1 | 68.4 / 47.5 / 68.9 | 37.7 / 18.3 / 46.2 |
| musdb_12 | 33.6 / 12.0 / 34.7 | 49.3 / 29.7 / 48.0 | 57.2 / 37.1 / 58.0 | 30.0 / 13.8 / 39.4 |

247 takes, 79439 pairs of consecutive pitched notes, largest overlap of renditions 10.0 ms, pairs over 10 ms: 0

**Rubato at every strictness's own tolerance, new seeds** (final measurement; synthetic
seeds 5 and 6 were never run before, held-out songs; `q/run_strictness.sh`,
`python -I q/strictness_table.py q/s3-strictness`): relaxed forgives 200 ms, standard
150 ms, strict 100 ms (APP_API 12.6); the perfect singer and the rubato singer graded at
the same strictness:

| song | relaxed (+-200 ms): perfect / rubato seed 5 / seed 6 (lost on notes >= 0.2 s) | standard (+-150 ms): perfect / rubato seed 5 / seed 6 (lost on notes >= 0.2 s) | strict (+-100 ms): perfect / rubato seed 5 / seed 6 (lost on notes >= 0.2 s) |
|---|---|---|---|
| ccm-30483 | 100.0 / 98.7 / 99.2 (0.7) | 100.0 / 98.9 / 99.3 (0.4) | 99.8 / 98.8 / 99.1 (0.2) |
| ccm-34740 | 100.0 / 99.5 / 99.2 (0.4) | 99.9 / 99.3 / 99.2 (0.3) | 99.6 / 98.8 / 98.9 (0.1) |
| ccm-39381 | 100.0 / 98.7 / 98.2 (0.6) | 99.9 / 98.2 / 98.5 (0.1) | 99.3 / 97.4 / 97.3 (0.3) |
| ccm-55541 | 100.0 / 99.6 / 99.6 (0.4) | 100.0 / 99.4 / 99.9 (0.4) | 99.9 / 99.3 / 99.6 (0.4) |
| musdb_01 | 99.7 / 98.5 / 98.7 (1.0) | 99.6 / 99.1 / 99.4 (0.3) | 99.6 / 99.2 / 99.2 (0.1) |
| musdb_06 | 100.0 / 98.5 / 98.1 (1.1) | 99.9 / 98.6 / 99.0 (0.3) | 99.6 / 98.6 / 99.1 (0.1) |
| musdb_10 | 99.9 / 97.5 / 97.7 (0.9) | 99.9 / 98.1 / 98.9 (0.2) | 99.2 / 97.8 / 98.0 (0.2) |
| musdb_12 | 100.0 / 98.5 / 98.4 (0.4) | 99.9 / 98.5 / 98.4 (0.1) | 99.5 / 98.4 / 98.0 (0.1) |

Largest drop from the perfect singer, all notes / notes of 0.2 s or more: relaxed 2.4 / 1.1; standard 1.8 / 0.4; strict 2.0 / 0.4

- **Rubato costs nothing on notes of 0.2 s or more** (at most 0.7 points,
  every song and seed, +-60 to +-150 ms; with the new seeds 5 and 6 at every strictness's
  own tolerance at most 1.1). What rubato still costs is on short notes: the
  rubato singer keeps each note at least half its length, so with offsets as large as
  the notes themselves it squeezes notes of 0.1-0.15 s to 40-70 ms. The tracker does not
  resolve a 40-70 ms note (its window is 43 ms, plus a 3-value median), and the singer's
  60 ms scoop fills most of it (`q/s2a/rub/`: on Homeless Man at +-60 ms every note under
  0.6 credit was sung for 40-70 ms, with the tracker still showing the previous note).
  Crowdpleaser and Homeless Man (37 % and 62 % of their graded time in notes under 0.2 s)
  therefore lose 3-4 points at +-150 ms, the other songs 0-2; on the held-out songs no
  rubato singer is more than 2.4 points under the perfect singer at any strictness.
- **One stretch of singing is credited to one note only**, checked on every timing take of
  the real chain (the line under the ceiling table): the renditions of consecutive pitched
  notes never overlapped by more than one hop. Without that rule (`--overlap-ms 100000`,
  each note its own timing) the swap adversary would gain 4-12 points, all of it from
  singing credited twice.
- **The borrowing adversaries.** `behind` sings each note's time at the previous note's
  pitch, so the right pitch for a note arrives one note late: it scores in the lowest band
  on every song (30.3-59.8). Moved 150 ms early (`behind:150`) or late
  (`ahead:150`), the right pitch for a note arrives one note-spacing less 150 ms late or
  early, which on songs of short notes is within the tolerance; then the adversary *is* a
  singer who sings the right notes a little late or early, and WO2 4.7 says that must not
  cost. The ceiling table shows the scorer gives these adversaries no more than each note
  could earn on its own within the tolerance (within 2.2 points: part of that
  is the scorer's bridging of short tracker dropouts, `--bridge-ms 0` gave 56.7 and
  74.6 instead of 57.6 and 75.2 on ccm-30483 in the first measurement, the rest the 80 ms
  vibrato median at note changes, neither of which the guide-only computation has), and
  far above what a rule with no timing freedom gives: the difference is exactly the
  tolerance doing its job. Swapped pairs score below the ceiling (only one note of a pair
  can have the stretch).
- `staccato:0.5` (only the first half of every note sung) keeps most of the credit on
  notes shorter than about 0.3 s and loses on longer notes (4-30 points on the notes of
  0.2 s or more in the table above); see 2.2 for why this cannot be told apart from legato
  rubato.
- Unvoiced sound in the middle of every note (gap singers) never scores more than silence
  there, before it did by up to 12.2 points (4.8).

### 4.8 The excusal rule on held-out singers (new)

**the excusal rule: gain on the original singers, and unvoiced sound in the middle of notes (after; before in brackets)**

| song | set | gain from the rule (points) | moments excused / judged |
|---|---|---:|---|
| demo | dev | +0.0 | 20 / 2202 (0.9 %) |
| Feel | dev | +2.9 | 698 / 10605 (6.6 %) |
| Crowdpleaser | dev | +0.5 | 128 / 1693 (7.6 %) |
| Homeless Man | dev | +1.8 | 163 / 1502 (10.9 %) |
| owner's song | dev | +0.8 | 547 / 11652 (4.7 %) |
| ccm-30483 | held-out | +0.9 | 638 / 9572 (6.7 %) |
| ccm-34740 | held-out | +0.2 | 265 / 10685 (2.5 %) |
| ccm-39381 | held-out | +0.5 | 480 / 7245 (6.6 %) |
| ccm-55541 | held-out | +0.0 | 130 / 7439 (1.7 %) |
| musdb_01 | held-out | +0.4 | 193 / 6700 (2.9 %) |
| musdb_06 | held-out | +1.3 | 384 / 5938 (6.5 %) |
| musdb_10 | held-out | +0.7 | 380 / 10378 (3.7 %) |
| musdb_12 | held-out | +0.0 | 189 / 9867 (1.9 %) |
| vocadito_5 | held-out | +0.0 | 76 / 1625 (4.7 %) |
| vocadito_11 | held-out | +0.0 | 16 / 979 (1.6 %) |
| vocadito_13 | held-out | +0.0 | 40 / 1231 (3.2 %) |
| vocadito_19 | held-out | +0.0 | 46 / 1735 (2.7 %) |
| vocadito_23 | held-out | +0.9 | 73 / 950 (7.7 %) |
| vocadito_26 | held-out | +0.0 | 12 / 315 (3.8 %) |
| vocadito_30 | held-out | +1.0 | 27 / 381 (7.1 %) |
| vocadito_37 | held-out | +0.0 | 22 / 2976 (0.7 %) |

| development song | middle quarter silent | breath | speech-shaped noise | breath or speech above silence? |
|---|---|---|---|---|
| demo | 84.3 (83.2) | 81.4 (92.6) | 83.4 (95.4) | -0.9 |
| Feel | 81.8 (82.1) | 79.5 (89.1) | 80.3 (92.0) | -1.5 |
| Crowdpleaser | 93.2 (92.3) | 90.7 (90.5) | 90.9 (91.7) | -2.3 |
| Homeless Man | 95.5 (93.8) | 90.6 (88.4) | 92.1 (87.9) | -3.4 |
| owner's song | 88.5 (86.8) | 85.0 (91.2) | 86.0 (94.9) | -2.5 |

| held-out song | middle quarter silent | breath | speech-shaped noise | breath or speech above silence? |
|---|---|---|---|---|
| ccm-30483 | 90.3 (88.6) | 86.6 (92.2) | 87.3 (95.0) | -3.0 |
| ccm-34740 | 88.5 (86.7) | 84.9 (91.7) | 86.3 (94.3) | -2.2 |
| ccm-39381 | 92.0 (91.0) | 88.9 (92.0) | 90.0 (94.2) | -2.0 |
| ccm-55541 | 88.7 (87.8) | 84.1 (91.5) | 84.8 (95.2) | -3.9 |
| musdb_01 | 82.5 (82.4) | 80.3 (92.0) | 81.0 (93.4) | -1.5 |
| musdb_06 | 84.9 (84.2) | 81.4 (88.1) | 82.2 (90.5) | -2.7 |
| musdb_10 | 88.9 (87.5) | 84.7 (91.8) | 86.1 (94.4) | -2.8 |
| musdb_12 | 86.7 (86.1) | 83.4 (90.8) | 84.4 (92.9) | -2.3 |

The threshold again with the snapshot's tracker (`q/run_clar.sh`): the middle quarter of every note replaced by unvoiced sound, graded with the rule off / clarity 0.6 / clarity 0.7 (the default). A cell is the grade; the last columns say how far the best unvoiced kind lands above silence at 0.6 and at 0.7:

| song | set | silence | breath | whisper | speech | above silence at 0.6 | at 0.7 |
|---|---|---|---|---|---|---:|---:|
| demo | dev | 83.0 / 84.8 / 84.3 | 80.0 / 83.5 / 81.4 | 80.2 / 82.6 / 82.5 | 80.3 / 86.5 / 83.4 | +1.7 | -0.9 |
| Feel | dev | 78.9 / 82.8 / 81.8 | 77.0 / 81.2 / 79.5 | 77.8 / 81.6 / 80.6 | 76.9 / 83.5 / 80.3 | +0.7 | -1.2 |
| Crowdpleaser | dev | 91.5 / 94.1 / 93.2 | 88.4 / 92.2 / 90.7 | 88.2 / 91.4 / 90.7 | 87.7 / 93.3 / 90.9 | -0.8 | -2.3 |
| Homeless Man | dev | 93.4 / 95.8 / 95.5 | 88.2 / 91.7 / 90.6 | 90.9 / 93.6 / 93.0 | 89.6 / 93.5 / 92.1 | -2.2 | -2.5 |
| owner's song | dev | 87.6 / 88.8 / 88.5 | 83.9 / 86.2 / 85.0 | 85.1 / 86.8 / 86.3 | 84.2 / 88.8 / 86.0 | +0.0 | -2.2 |
| ccm-30483 | held-out | 89.7 / - / 90.3 | 85.5 / - / 86.6 | 86.8 / - / 87.9 | 85.1 / - / 87.3 | - | -2.4 |
| ccm-34740 | held-out | 87.7 / - / 88.5 | 84.0 / - / 84.9 | 85.4 / - / 86.6 | 84.3 / - / 86.3 | - | -1.9 |
| ccm-39381 | held-out | 91.0 / - / 92.0 | 87.6 / - / 88.9 | 88.7 / - / 90.2 | 87.8 / - / 90.0 | - | -1.8 |
| ccm-55541 | held-out | 87.8 / - / 88.7 | 82.7 / - / 84.1 | 84.7 / - / 86.0 | 83.0 / - / 84.8 | - | -2.7 |
| musdb_01 | held-out | 81.2 / - / 82.5 | 79.1 / - / 80.3 | 80.0 / - / 81.4 | 78.8 / - / 81.0 | - | -1.1 |
| musdb_06 | held-out | 82.2 / - / 84.9 | 79.0 / - / 81.4 | 80.1 / - / 82.9 | 78.9 / - / 82.2 | - | -2.0 |
| musdb_10 | held-out | 88.0 / - / 88.9 | 83.7 / - / 84.7 | 85.2 / - / 86.5 | 84.1 / - / 86.1 | - | -2.4 |
| musdb_12 | held-out | 85.2 / - / 86.7 | 81.9 / - / 83.4 | 83.2 / - / 84.9 | 81.9 / - / 84.4 | - | -1.8 |

| frames with sound but no pitch, inside sung notes | frames | clarity >= 0.6 | >= 0.7 (excusable) |
|---|---:|---:|---:|
| original vocal, crowd | 4017 | 76 % | 54 % |
| original vocal, feel | 5116 | 68 % | 44 % |
| original vocal, homeless | 6259 | 77 % | 54 % |
| original vocal, musdb_06 | 3515 | 79 % | 51 % |
| original vocal, owner | 1831 | 83 % | 69 % |
| original vocal, vocadito_26 | 130 | 82 % | 65 % |
| original vocal, vocadito_30 | 199 | 78 % | 66 % |
| generated singer, middle quarter breath, crowd | 925 | 45 % | 27 % |
| generated singer, middle quarter speech, crowd | 1002 | 48 % | 29 % |
| generated singer, middle quarter whisper, crowd | 908 | 45 % | 32 % |
| generated singer, middle quarter breath, demo | 368 | 27 % | 12 % |
| generated singer, middle quarter speech, demo | 407 | 39 % | 19 % |
| generated singer, middle quarter whisper, demo | 293 | 25 % | 24 % |
| generated singer, middle quarter breath, feel | 4391 | 41 % | 28 % |
| generated singer, middle quarter speech, feel | 4861 | 46 % | 29 % |
| generated singer, middle quarter whisper, feel | 4393 | 39 % | 30 % |
| generated singer, middle quarter breath, homeless | 1312 | 46 % | 30 % |
| generated singer, middle quarter speech, homeless | 1357 | 51 % | 34 % |
| generated singer, middle quarter whisper, homeless | 1243 | 50 % | 35 % |
| generated singer, middle quarter breath, owner | 2483 | 28 % | 13 % |
| generated singer, middle quarter speech, owner | 2821 | 39 % | 18 % |
| generated singer, middle quarter whisper, owner | 2423 | 25 % | 16 % |

- **Held-out original singers gain +0.0 to +1.3 points from the rule** (development
  +0.0 to +2.9), with 0.7-10.9 % of the judged moments excused; two held-out grades change band with
  the rule (musdb_06 94.3 -> 95.6, vocadito 23 94.2 -> 95.1, A to A+).
- **Unvoiced sound is never excused:** with the middle quarter of every note replaced by
  breath, whisper-like fricatives or speech-shaped noise at the singer's level, every
  development and held-out song scores below the same singer with silence there (the gap
  sound also delays the tracker's next onset). With the any-sound rule of 0.1.0 the same
  takes scored up to 12.2 points above silence.
- The threshold still separates the two with this snapshot's tracker (the table of frames
  with sound but no pitch): most of the original vocals' unresolved frames inside notes
  have clarity 0.7 or more (44-69 %), against 12-35 % of the generated unvoiced sound; at
  0.6 instead, unvoiced sound on two development songs scored up to 1.7 points above
  silence, at 0.7 never (last columns).

### 4.9 Where the original singer loses credit (new; `q/diag_notes.py`)

For every graded note at key 0, each 10 ms moment of the note's own time span is put in
one category before the scorer's rules: `ok` (the tracker's value within 50 cents of the
note), `unv_curvev_hiclar` / `_loclar` (the tracker named no pitch where the guide's own
pitch curve is voiced, clarity at least / below 0.7), `unv_curveu` (both unvoiced),
`off_curveok` (the tracker's value off the note but within 50 cents of the guide's own
curve: a slide, scoop, ornament or the note's ends, which the note's single pitch does not
describe), `off_curveoff` (off both: a tracker error or a guide curve error). Then the
scorer's credit by target pitch band, and the notes that lose most.

**where the original singer loses credit at key 0 (after)**

```
== Feel: overall 97.1 graded 245 voiced_share 0.85
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    75.3 %
     unv_curvev_hiclar      8.9 %
     unv_curvev_loclar      9.2 %
     unv_curveu             0.3 %
     off_curveok            6.2 %
     off_curveoff           0.1 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     <48    n=106 time  44.7 %  credit  97.1  ok  72.3  unvoiced  20.5
     48-52  n= 79 time  38.8 %  credit  96.4  ok  76.7  unvoiced  17.9
     52-57  n= 58 time  16.2 %  credit  98.7  ok  79.9  unvoiced  11.8
     57-62  n=  2 time   0.3 %  credit 100.0  ok  89.3  unvoiced   7.1
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (469, 217.85, 1.92, 45.0, 0.72, {'ok': 109, 'unv_curvev_hiclar': 26, 'unv_curvev_loclar': 52, 'unv_curveu': 5})
      (197, 93.61, 2.08, 49.0, 0.81, {'ok': 133, 'unv_curvev_hiclar': 24, 'unv_curvev_loclar': 34, 'unv_curveu': 2, 'off_curveok': 15})
      (389, 179.47, 2.15, 45.2, 0.83, {'ok': 112, 'unv_curvev_hiclar': 51, 'unv_curvev_loclar': 40, 'off_curveok': 12})
      (382, 173.78, 1.24, 49.0, 0.85, {'ok': 76, 'unv_curvev_hiclar': 23, 'unv_curvev_loclar': 22, 'off_curveok': 3})
      (398, 189.84, 1.01, 49.0, 0.82, {'ok': 60, 'unv_curvev_hiclar': 23, 'unv_curvev_loclar': 18})
```
```
== Crowdpleaser: overall 99.2 graded 82 voiced_share 0.86
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    67.2 %
     unv_curvev_hiclar      8.9 %
     unv_curvev_loclar      8.5 %
     unv_curveu             0.4 %
     off_curveok           14.8 %
     off_curveoff           0.2 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     <48    n=  1 time   0.7 %  credit 100.0  ok  53.8  unvoiced  46.2
     48-52  n= 40 time  52.0 %  credit  98.7  ok  62.8  unvoiced  14.4
     52-57  n= 41 time  47.3 %  credit  99.7  ok  72.2  unvoiced  20.3
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (528, 157.12, 0.43, 49.5, 0.88, {'ok': 10, 'unv_curvev_hiclar': 5, 'unv_curvev_loclar': 1, 'off_curveok': 27})
      (497, 148.88, 0.07, 49.0, 0.38, {'ok': 5, 'off_curveok': 2})
      (466, 141.0, 0.25, 48.1, 0.87, {'ok': 9, 'unv_curvev_hiclar': 3, 'unv_curvev_loclar': 3, 'off_curveok': 10})
      (549, 161.66, 0.32, 54.5, 0.92, {'ok': 9, 'unv_curvev_hiclar': 10, 'unv_curvev_loclar': 4, 'off_curveok': 9})
      (2, 20.45, 0.26, 51.3, 1.0, {'ok': 19, 'unv_curvev_hiclar': 3, 'unv_curvev_loclar': 3, 'off_curveoff': 1})
```
```
== Homeless Man: overall 96.1 graded 94 voiced_share 0.81
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    57.5 %
     unv_curvev_hiclar     17.8 %
     unv_curvev_loclar      7.3 %
     unv_curveu             0.4 %
     off_curveok           17.0 %
     off_curveoff           0.1 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     <48    n= 12 time  11.3 %  credit  99.5  ok  47.7  unvoiced  34.5
     48-52  n= 48 time  48.8 %  credit  97.4  ok  61.9  unvoiced  24.5
     52-57  n= 31 time  36.5 %  credit  93.4  ok  54.3  unvoiced  23.6
     57-62  n=  3 time   3.4 %  credit 100.0  ok  61.0  unvoiced  16.9
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (24, 16.43, 0.27, 54.1, 0.36, {'ok': 7, 'unv_curvev_hiclar': 2, 'unv_curvev_loclar': 1, 'off_curveok': 17})
      (657, 201.53, 0.47, 52.7, 0.71, {'ok': 16, 'unv_curvev_hiclar': 9, 'unv_curvev_loclar': 11, 'off_curveok': 11})
      (323, 105.89, 0.19, 50.2, 0.44, {'ok': 5, 'unv_curvev_hiclar': 7, 'unv_curvev_loclar': 1, 'unv_curveu': 1, 'off_curveok': 5})
      (104, 37.84, 0.22, 52.5, 0.69, {'ok': 7, 'unv_curvev_hiclar': 3, 'unv_curvev_loclar': 1, 'off_curveok': 11})
      (82, 31.59, 0.1, 49.4, 0.32, {'ok': 4, 'unv_curvev_hiclar': 4, 'unv_curvev_loclar': 1, 'off_curveok': 1})
```
```
== owner's song: overall 99.2 graded 332 voiced_share 0.92
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    79.5 %
     unv_curvev_hiclar      8.0 %
     unv_curvev_loclar      2.9 %
     unv_curveu             0.2 %
     off_curveok            8.8 %
     off_curveoff           0.4 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     48-52  n=  1 time   0.2 %  credit 100.0  ok  94.7  unvoiced   5.3
     52-57  n=  3 time   0.9 %  credit 100.0  ok  71.2  unvoiced  26.1
     57-62  n=108 time  36.4 %  credit  99.7  ok  79.6  unvoiced  10.6
     >=62   n=220 time  62.6 %  credit  99.0  ok  79.6  unvoiced  11.0
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (46, 39.24, 0.7, 64.1, 0.65, {'ok': 34, 'unv_curvev_loclar': 1, 'off_curveok': 35})
      (29, 30.61, 0.58, 62.1, 0.73, {'ok': 25, 'unv_curvev_hiclar': 22, 'unv_curvev_loclar': 8, 'off_curveok': 3})
      (335, 186.88, 0.86, 61.9, 0.87, {'ok': 53, 'unv_curvev_hiclar': 7, 'unv_curvev_loclar': 2, 'off_curveok': 23, 'off_curveoff': 1})
      (135, 98.03, 0.85, 66.1, 0.88, {'ok': 54, 'unv_curvev_hiclar': 5, 'unv_curvev_loclar': 12, 'unv_curveu': 1, 'off_curveok': 13})
      (288, 164.46, 0.26, 63.0, 0.67, {'ok': 7, 'unv_curvev_hiclar': 2, 'unv_curvev_loclar': 3, 'off_curveok': 14})
```
```
== musdb_06: overall 95.6 graded 182 voiced_share 0.86
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    73.7 %
     unv_curvev_hiclar     10.1 %
     unv_curvev_loclar      7.2 %
     unv_curveu             0.1 %
     off_curveok            6.7 %
     off_curveoff           2.2 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     <48    n= 52 time  25.7 %  credit  86.9  ok  65.9  unvoiced  22.2
     48-52  n= 39 time  17.4 %  credit  99.9  ok  76.1  unvoiced  16.0
     52-57  n= 63 time  40.4 %  credit  98.4  ok  74.1  unvoiced  17.3
     57-62  n= 24 time  13.7 %  credit  99.3  ok  81.2  unvoiced  12.7
     >=62   n=  4 time   2.8 %  credit 100.0  ok  87.4  unvoiced   4.0
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (295, 170.01, 1.45, 47.0, 0.2, {'ok': 17, 'unv_curvev_hiclar': 66, 'unv_curvev_loclar': 24, 'off_curveoff': 38})
      (312, 182.24, 0.55, 36.0, 0.31, {'ok': 8, 'unv_curvev_hiclar': 14, 'unv_curvev_loclar': 11, 'off_curveoff': 22})
      (319, 186.19, 0.3, 47.1, 0.0, {'unv_curvev_hiclar': 4, 'off_curveoff': 26})
      (334, 190.84, 0.4, 36.0, 0.7, {'ok': 8, 'unv_curvev_hiclar': 19, 'unv_curvev_loclar': 8, 'off_curveoff': 5})
      (166, 112.42, 0.1, 38.0, 0.0, {'unv_curvev_hiclar': 6, 'unv_curvev_loclar': 4})
```
```
== vocadito_26: overall 93.3 graded 18 voiced_share 0.98
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    65.4 %
     unv_curvev_hiclar      5.1 %
     unv_curvev_loclar      1.9 %
     unv_curveu             0.0 %
     off_curveok           22.2 %
     off_curveoff           5.4 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     48-52  n= 13 time  63.8 %  credit  95.3  ok  72.9  unvoiced   3.0
     52-57  n=  5 time  36.2 %  credit  89.1  ok  52.2  unvoiced  14.2
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (44, 10.62, 0.4, 54.3, 0.64, {'ok': 6, 'off_curveok': 16, 'off_curveoff': 18})
      (42, 10.41, 0.08, 48.3, 0.0, {'ok': 7, 'off_curveok': 1})
      (40, 10.24, 0.11, 51.6, 0.72, {'ok': 6, 'off_curveok': 5})
      (32, 8.62, 0.39, 53.1, 1.0, {'ok': 26, 'unv_curvev_hiclar': 5, 'unv_curvev_loclar': 1, 'off_curveok': 7})
      (33, 9.02, 0.18, 48.6, 1.0, {'ok': 16, 'off_curveok': 2})
```
```
== vocadito_30: overall 94.6 graded 19 voiced_share 0.85
   share of graded note time by moment category (raw, before the scorer's rules):
     ok                    59.4 %
     unv_curvev_hiclar     12.7 %
     unv_curvev_loclar      5.5 %
     unv_curveu             0.0 %
     off_curveok           21.0 %
     off_curveoff           1.5 %
     off_curveu             0.0 %
   by target pitch (MIDI): notes, time share, scorer credit, raw on-pitch, raw tracker-unvoiced-where-guide-voiced
     <48    n=  1 time  21.6 %  credit 100.0  ok  79.8  unvoiced   3.0
     52-57  n=  9 time  40.4 %  credit  89.0  ok  50.3  unvoiced  25.9
     57-62  n=  9 time  38.0 %  credit  96.3  ok  57.5  unvoiced  18.4
   biggest losses (note, start s, length s, target, credit, categories in 10 ms moments):
      (21, 7.79, 0.19, 56.9, 0.03, {'ok': 3, 'unv_curvev_hiclar': 7, 'unv_curvev_loclar': 3, 'off_curveok': 6})
      (39, 13.22, 0.14, 57.8, 0.55, {'ok': 3, 'unv_curvev_hiclar': 11})
      (32, 10.77, 0.16, 56.0, 0.87, {'ok': 6, 'unv_curvev_hiclar': 1, 'off_curveok': 8, 'off_curveoff': 1})
      (1, 1.58, 0.17, 53.5, 1.0, {'ok': 8, 'unv_curvev_hiclar': 2, 'unv_curvev_loclar': 7})
      (2, 1.79, 0.16, 53.0, 1.0, {'ok': 11, 'unv_curvev_hiclar': 5})
```

- **What the scorer had to fix (done in 0.2.0):** the `off_curveok` moments - the singer
  on the guide's own pitch curve but off the note's single pitch at a slide, scoop or the
  note's ends: 6-17 % of graded time on the CC songs - and the tracker's late onsets were
  charged as misses by 0.1.0's per-note shift; the rendition timing absorbs them. Its
  any-sound excusal also hid how much the tracker missed.
- **What is the tracker's:** `unv_curvev` - the tracker named no pitch where the guide's
  curve is voiced - is the largest remaining category on the low male songs (Homeless Man
  about a quarter of graded time), against about a tenth on the owner's song; at clarity
  0.7 or more a quarter of a note at most is excused. musdb_06's losses are at its lowest
  notes (MIDI 36-38, 65-73 Hz, below E2), where the tracker names other pitches
  (`off_curveoff`).
- **What is the guide's:** `off_curveoff` (neither the note nor the curve) is under 0.5 %
  on the development songs; on vocadito 26 one 0.4 s note carries most of it. The notes
  that lose most are short (0.07-0.3 s), held notes the tracker half-voiced, or notes whose
  single pitch does not describe a slide (the owner's song at 39.2 s: half its moments on
  the guide's curve but off the note).

### 4.10 Fresh held-out songs never used by the scoring stream (final measurement)

Four MUSDB18-HQ test songs that the echo workstream's verifier packed for its own check
(set V, practice-echo.md 13.10; `../jukeoke-build/practice-echo/qv/songs/`, guides by
`jukeoke-cli guide add`, research use, kept outside the repository). The scoring stream had
never run them; they were run once with the final build (`q/run_setv.sh`,
`python -I q/setv_table.py q/s3-setv`), nothing was changed after. Whole songs, no
reference; the original vocal through the engine at key 0 and in the sweep at every key;
synthetic singers with new seeds 7 and 8:

| song | original singer, key 0 (graded / total, voiced share) | without the excusal rule | every key -12..+12: lowest (key), largest difference from key 0 all keys / from -6 | best against a wrong guide |
|---|---|---|---|---|
| MUSDB Bobby Nobody | 98.6 (247 / 564, 0.89) | 98.0 | 95.8 (-12), 2.8 / 0.4 | 31.8 |
| MUSDB Carlos Gonzalez | 90.4 (244 / 463, 0.81) | 88.6 | 75.8 (-12), 14.6 / 11.8 | 21.2 |
| MUSDB The Mountaineering Club | 95.5 (186 / 321, 0.8) | 91.1 | 95.2 (-12), 0.8 / 0.8 | 33.3 |
| MUSDB Secretariat | 97.9 (144 / 314, 0.91) | 95.4 | 96.6 (-12), 1.3 / 0.5 | 31.3 |

| singer | MUSDB Bobby Nobody | MUSDB Carlos Gonzalez | MUSDB The Mountaineering Club | MUSDB Secretariat |
|---|---|---|---|---|
| perfect | 99.0 | 99.9 | 99.5 | 99.8 |
| rubato:60:7 | 98.8 | 99.8 | 99.4 | 99.6 |
| rubato:120:7 | 97.8 | 99.7 | 99.1 | 99.4 |
| rubato:150:7 | 97.0 | 99.5 | 99.0 | 99.0 |
| rubato:150:8 | 96.9 | 99.0 | 98.5 | 98.9 |
| behind | 45.4 | 43.1 | 29.9 | 40.0 |
| behind:150 | 63.7 | 57.5 | 44.4 | 49.3 |
| ahead:150 | 63.2 | 61.4 | 42.3 | 55.1 |
| swap | 34.1 | 37.8 | 25.6 | 37.3 |
| gap:0.25:silence | 90.6 | 84.6 | 88.5 | 84.9 |
| gap:0.25:breath | 86.4 | 82.1 | 86.4 | 82.1 |
| gap:0.25:speech | 87.6 | 82.5 | 86.9 | 82.7 |
| silent | 0.0, marker 0 | 0.0, marker 0 | 0.0, marker 0 | 0.0, marker 0 |
| noise | 0.0, marker 0 | 0.0, marker 0 | 0.0, marker 0 | 0.0, marker 0 |

- **On material it was never near, the scorer behaves as on the held-out set:** rubato of
  +-60 to +-150 ms costs at most 2.1 points against the perfect singer (Bobby Nobody, the
  song of shortest notes); the borrowing adversaries stay low (`behind` 29.9-45.4,
  `swap` 25.6-37.8; `behind:150` / `ahead:150` 42.3-63.7, the "a little late or early"
  case of 4.7); breath or speech-shaped noise in the middle of every note scores below
  silence there on every song (1.6 to 4.2 points below); silence and noise 0 with no
  marker.
- **The original singer at key 0:** three of the four in the top band (95.5-98.6) and
  within 2.8 points at every key; **Carlos Gonzalez 90.4 (A), falling to 78.6 at -6 and
  75.8 at -12.**
  Per note (`q/diag_notes.py` on `q/s3-setv-diag/`, `q/lowkey_table.py` on
  `q/s3-setv-lowkey/`): its notes from MIDI 48 up score 94.9-99.6; the loss is in 23 %
  of the graded time where the guide's notes lie at MIDI 36-41 (C2-F2). There the stem
  carries a second voice: the tracker reads MIDI 67-69 (about 400 Hz) where the guide's
  own pitch curve says 39 (78 Hz) - two octaves and a third apart, so not an octave error
  the scorer could fold (a MUSDB "vocals" stem holds every voice of the song). The guide
  followed one voice and the live tracker the other; at lower keys those guide notes fall
  below C2 and the grade follows (from -6, the notes from C2 score 97.3-97.7). The guide's
  and the tracker's, not the scorer's: the scorer grades what the tracker reports against
  the notes the guide gives. Noted for the guide workstream (a lead stem with a second
  voice above the guide's line).

## 5. What does not pass, and why

1. **The original singer is not within a few points of key 0 at the lowest keys on the low
   male songs.** At key 0 it is in the top band on every test song, and from -6 to +12
   within 3 points on every development song (the demo now included). Below that the
   melody leaves the tracker's range: Feel from -7 (93.3) to -12 (62.4), Homeless Man at
   -11/-12 (88.1, 85.2), held-out musdb_06 from -8 (87.2) to -12 (75.5). The low-key table
   (4.3) shows the loss is in notes below C2, which the tracker is not required to hear
   (Feel's notes from C2 score 95.3-97.1 at keys -12, -11, -9, -6 and 0), and for Homeless
   Man and musdb_06
   at -12 also in the tracker's voicing of the shifted voice above E2. The tracker's; the
   grade of what the tracker hears is exact. A scorer rule to stop grading notes below C2
   was considered and not done (the low-key notes in 4.3).
2. **One fresh held-out original singer is below the top band at key 0** (MUSDB Carlos
   Gonzalez 90.4, 4.10): a stem with two voices, where the guide follows a low line (C2-F2)
   and the live tracker a voice two octaves above it on 23 % of the graded time. The
   guide's and the tracker's; its notes from MIDI 48 up score 94.9-99.6.
3. **Rubato on very short notes** (4.7): notes squeezed below about 0.1 s are not resolved
   by the tracker, so rubato of +-120 to +-150 ms costs the two development songs of short
   notes 3-4 points (nothing on notes of 0.2 s or more; held-out songs at most 2.4).
4. **Singing half of every short note keeps most of its credit** (`staccato:0.5`, 2.2):
   inherent in forgiving timing at both ends of a note.
5. **Wrong guides on songs of short stepwise notes score higher than before** (up to
   about 56, still the lowest band): the tolerance lets a note meet its neighbour's pitch.
6. **Music flags and unjudged notes with nothing fed back** (4.6): 7 of 195 takes with
   nothing fed back still carry a music flag (the echo stage declares the return present
   for 2-18 s when the synthetic singer is in unison with the song's own vocal parts or
   under heavy compression), and its "cannot judge" verdict leaves 593 notes of such takes
   ungraded (139 in the first measurement) with no trust flag saying why. With the karaoke
   mix as the reference the demo's original vocal loses up to 39 points at three keys
   (4.3). The chain only relays the echo stage's verdicts. For the echo workstream.
7. **Loud returns with the guide vocal on are not graded** (-3 and 0 dB on the demo and the
   owner's song, 4.6): the echo stage's removal verdict (`untrusted`), as APP_API 12.5 asks
   when the removal is not good enough. For the echo workstream.
8. **Reverb** costs up to about 6 points at RT60 0.6 s and up to 12 at 1.2 s (4.5): the
   tracker's reverb item.
9. **Not run here:** nothing in this file went through a real microphone, the real capture
   thread or the real clocks (the app core of Stage 3b wires the same chain); the chain was
   fed with exact host stamps (plus jitter in one run) and a playback clock at the input's
   rate. Nobody has sung into any of it.

## 6. Unit tests

`cargo test --release -p jk-pitch --lib -- score:: practice::`: 28 tests (22 scorer, 6
chain), all pass; `cargo test --release -p jk-pitch --test practice_alloc`: the chain
allocates nothing per block over 20 s of a singer with a USB return (passes);
`cargo test --release -p jk-cli`: passes; the whole `cargo test --release -p jk-pitch`
(81 library tests, `echo_alloc` 4, `practice_alloc` 1, `tracker_alloc` 3): all pass;
`cargo clippy --release -p jk-pitch -p jk-cli --all-targets`: no warning in either crate
(run on the final snapshot). New in
the quality round: `only_periodic_sound_the_tracker_could_not_resolve_is_excused`,
`being_early_or_late_up_to_the_tolerance_costs_nothing`,
`one_stretch_of_singing_is_credited_to_one_note_only`, `the_borrowing_adversary_scores_low`,
`the_fast_credit_table_equals_the_plain_sums`,
`music_flags_and_ungraded_moments_come_only_from_the_echo_stage`; removed: the two tests
of scorer 0.1.0's usual timing and any-sound excusal.

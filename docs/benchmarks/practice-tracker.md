# Live pitch tracker measurements (Work Order 2: Stage 0, then hardening on real voices)

Stage 0 measured on 2026-11-19 on the build PC (Windows 11, Ryzen 7 9700X, 32 GB), Rust 1.100
`x86_64-pc-windows-gnu`, release profile. The tracker was then **hardened on real voices**
the same day (sections 13 to 17): it is now evaluated on about 108 minutes of real singing
with a singer-disjoint dev / held-out split, and its octave decision, start rules and
analysis band changed. **Section 1 and sections 13 to 17 describe the current tracker;
sections 2 to 12 are the Stage 0 record** (the "before" of the hardening), unchanged.

Evidence: Stage 0 in `../jukeoke-build/practice-tracker/bench-final.txt`; hardening in
`../jukeoke-build/practice-tracker/runs/final/` (bench, real-voice tables for both trackers,
generated-voice probes, ablations) and the evaluation set in
`../jukeoke-build/practice-tracker/eval/` (nothing of it in the repository). No microphone
was opened and nobody sang for this report: real voices are recordings.

Code under test: `jk_pitch::tracker::Tracker` (`crates/jk-pitch/src/tracker.rs`), default
`TrackerConfig`:

| Setting | Value (Stage 0 value if different) | Why |
|---|---|---|
| Method | McLeod Pitch Method (McLeod and Wyvill, ICMC 2005) on `realfft` 3.5; octave decided over time (below) | WO2 section 6; the `pitch-detection` crate plans an FFT per call and has an octave bug |
| Window | 42.67 ms, sized in time: 683 / 1882 / 2048 / 4096 samples at 16 / 44.1 / 48 / 96 kHz | a C2 period (15.3 ms) fits 1.8 times in the NSDF overlap (trap 5); same answer at every rate (trap 14) |
| FFT | real FFT of the zero-padded window, length the next power of two >= 2 x window | linear autocorrelation, no wrap-around |
| Hop | 10 ms | a new estimate every 10 ms |
| Pitch range | 62 Hz to 1400 Hz | C2 with margin, above 60 Hz hum; C6 plus vibrato |
| Filters | 4th-order Butterworth high-pass at 55 Hz; **4th-order Butterworth low-pass at 4 kHz** (none) | thumps, pops, DC, hum out; a bright EQ, sibilance and air no longer lower a vowel's clarity (section 13.4) |
| Candidates | **every NSDF key maximum >= 0.5 x the highest, at most 6** (the first >= 0.9 x the highest) | the octave is chosen among them |
| Octave scoring | **periodicity of the flattened spectrum (autocorrelation of \|X\|^0.5, 0-5 kHz) at each candidate, less 0.06 per octave below MPM's choice** (none) | tells weak odd harmonics from a second voice or a reverb tail (section 13.4) |
| Path | **Viterbi over the last frames, decided 2 frames (20 ms) behind the newest; 0.2 per octave down, 0.1 per octave up; pinned to what was shown; a note starts on the newest window's octave** (three-point median only) | no octave flips inside a note, no onset flashes, leaps still followed |
| Singing / not singing | start: clarity >= 0.93 then a continuing frame >= 0.75, **or five frames (20 ms before the decided one to the newest) >= 0.8 within 40 cents of each other, at least 160 Hz**; keep: >= 0.75 while the pitch continues (<= 150 cents per 10 ms); level >= -55 dBFS to start, >= -60 to keep | real voices with clarity 0.8-0.93 are shown; breath, whispers and thumps are not |
| Smoothing | three-point median of the previous reported frame, the decided frame and the next one on the path | removes single-frame slips |
| Input | one mono channel chosen by the caller; channels are never summed | trap 14, WO2 4.4 |
| Output | `PitchEvent { time, ready_at, hz: Option<f32>, clarity, level_dbfs }` every hop | `time` = centre of the decided window; `ready_at - time` = half a window + 2 hops = 41.3 ms (was half a window + 1 hop) |

## 1. Summary (current tracker)

| WO2 requirement (4.4 table, section 9 row "Tracker", 4.10) | Result (Stage 0 in brackets) | Criterion |
|---|---|---|
| Range | C2 to C6 tracked, every semitone, three vowels, four rates, 100 % of held events pitched (same) | E2..C6, aim C2 |
| Accuracy on steady tones | max 2.36 cents at 16 kHz, <= 0.10 cents at 44.1 / 48 / 96 kHz (2.38 / 0.10) | within 10 cents |
| Octave jumps on a steady note, generated | 0 in every held part; whole notes including onsets: 3 / 6 / 6 / 9 octave-off events at 16 / 44.1 / 48 / 96 kHz in 147 notes (14 / 14 / 12 / 10); ordinary jitter 0 (2-6); mild breath held part 0 except 16 kHz (8-13) | none |
| Octave errors, real voices | 0.47 % of frames dev, **0.42 % held-out** (0.90 / 0.74 %); octave jumps on a held pitch 63 dev, 37 held-out in about 50 minutes each (121 / 51) | none |
| Real sung frames shown (voicing recall) | 89.6 % dev, **79.1 % held-out** (81.6 / 69.7 %); right note when shown 99.0 / 98.8 % | (report) |
| Not singing: silence, breath, speech-like noise, consonants, room noise, thumps, pops | 0 pitched events, every rate, 30 s per kind (0); a 10-minute whispered-speech probe per rate gave 10 / 7 short events (50-70 ms) at 44.1 / 96 kHz, 0 with steady starts off (section 19) | no pitch, no wandering |
| Delay, onset to first correct event, generated | median 46 ms, max 72 ms (36 / 62); marker off after singing stops median 67, max 80 ms (47 / 60) | aim 50 ms, never > 100 ms |
| Delay on real voices (from the reference's first voiced 10 ms frame) | median 70 / 80 ms dev / held-out, p90 120 ms (p90 180 / 190); 15-18 % of real onsets take more than 100 ms (vocadito 11-13 %), section 19 | aim 50 ms, never > 100 ms: **not met on real voices** |
| Same answer across sample rates | generated melody p95 <= 0.68 cents, max 28.8 cents (16 kHz); real voices resampled to 16 / 48 / 96 kHz: RPA within 0.3 points of the original | same answer |
| The owner's rig (real voices through generated effects) | compression, warm EQ, proximity, pops and thumps: within 1 point (or better); **bright EQ -5 to -7 points, reverb RT60 0.3-1.5 s at -6 dB -5 to -19 points** of real sung frames shown (Stage 0 -8 to -9 and -7 to -23); right note when shown >= 93.8 % | within a few points |
| Processor cost | 1.7 ms per second of audio at 48 kHz, 18 us per analysis, 0.17 % of one core (1.0 ms) | (report) |
| MacBook Air M1 estimate | 3.5-5 ms per second of audio at 48 kHz (0.35-0.5 % of one core) | (estimate) |
| No allocation per block | 0 allocations (`tests/tracker_alloc.rs`) | none |
| Same code on the Mac | `cargo check --all-targets --target aarch64-apple-darwin` of the tracker module passes (type-checked only) | WO2 5 |

> **Sections 2 to 12 below are the Stage 0 tracker's measurements**, kept as the record the
> hardening is compared with. The same bench re-run on the current tracker is
> `../jukeoke-build/practice-tracker/runs/final/bench-wo2.txt`; its headline numbers are in
> section 1 and section 14.

## 2. Steady notes C2..C6, every semitone, three vowels

Each note: 0.2 s silence, 0.6 s held at -20 dBFS RMS, 0.2 s silence; vowels ah, ee, oo (ah's
first formant at 730 Hz makes the harmonics of a low note louder than its fundamental, the
classic trap for octave errors). Errors of the events whose described time lies from 0.1 s
after the onset to 0.05 s before the release.

| rate | register | median abs err (cents) | p95 | max | octave errors | gross (>100 c) | voiced | worst note voiced | events |
|---|---|---|---|---|---|---|---|---|---|
| 16000 | C2-B2 | 0.01 | 0.08 | 0.11 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 16000 | C3-B3 | 0.03 | 0.19 | 0.24 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 16000 | C4-B4 | 0.06 | 0.69 | 0.81 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 16000 | C5-C6 | 0.18 | 1.92 | 2.38 | 0 | 0 | 100.0 % | 100.0 % | 1755 |
| 44100 | C2-B2 | 0.00 | 0.00 | 0.01 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 44100 | C3-B3 | 0.00 | 0.01 | 0.02 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 44100 | C4-B4 | 0.01 | 0.03 | 0.04 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 44100 | C5-C6 | 0.01 | 0.07 | 0.10 | 0 | 0 | 100.0 % | 100.0 % | 1755 |
| 48000 | C2-B2 | 0.00 | 0.00 | 0.01 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 48000 | C3-B3 | 0.00 | 0.01 | 0.02 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 48000 | C4-B4 | 0.01 | 0.02 | 0.03 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 48000 | C5-C6 | 0.01 | 0.05 | 0.06 | 0 | 0 | 100.0 % | 100.0 % | 1755 |
| 96000 | C2-B2 | 0.00 | 0.00 | 0.00 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 96000 | C3-B3 | 0.00 | 0.00 | 0.01 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 96000 | C4-B4 | 0.00 | 0.01 | 0.01 | 0 | 0 | 100.0 % | 100.0 % | 1620 |
| 96000 | C5-C6 | 0.00 | 0.01 | 0.01 | 0 | 0 | 100.0 % | 100.0 % | 1755 |

The lowest clarity in any held note was 0.987 (16 kHz, C5-C6), against a start threshold of
0.93. The error grows at 16 kHz on high notes because a C6 period is only 15 samples there
and parabolic interpolation of the NSDF peak is coarser; it stays under 2.4 cents.

## 3. Vibrato, slides, breathy voices

**Vibrato** (+-50 cents; 1.5 s notes every third semitone, vowels ah and oo). Error against the
true instantaneous pitch at each event's described time, 0.15 s after onset to 0.05 s before
release. Every row: 0 octave errors, 0 gross errors, 100.0 % voiced (worst note 100.0 %).

| rate | vibrato | C2-B2 median / p95 / max | C3-B3 | C4-B4 | C5-C6 |
|---|---|---|---|---|---|
| 16000 | 5 Hz | 2.78 / 6.45 / 9.59 | 2.82 / 5.58 / 8.37 | 2.77 / 6.80 / 8.85 | 2.77 / 6.15 / 8.21 |
| 16000 | 5.5 Hz | 3.21 / 7.69 / 10.31 | 3.25 / 6.56 / 9.47 | 3.25 / 7.58 / 9.99 | 3.29 / 7.07 / 9.22 |
| 16000 | 6 Hz | 3.76 / 8.32 / 11.69 | 3.82 / 7.40 / 10.26 | 3.89 / 8.60 / 11.10 | 3.93 / 8.18 / 10.44 |
| 48000 | 5 Hz | 2.87 / 6.60 / 9.51 | 3.00 / 5.72 / 8.56 | 2.82 / 7.01 / 9.13 | 2.76 / 5.99 / 8.85 |
| 48000 | 5.5 Hz | 3.33 / 7.82 / 10.45 | 3.45 / 6.79 / 9.69 | 3.40 / 7.82 / 10.24 | 3.25 / 6.86 / 10.13 |
| 48000 | 6 Hz | 3.86 / 8.44 / 12.02 | 3.96 / 7.67 / 10.78 | 4.03 / 8.84 / 11.53 | 3.93 / 8.11 / 11.23 |

44.1 and 96 kHz are within 0.2 cents of the 48 kHz figures (full table in
`bench-final.txt`). The error is the 42.7 ms window and the three-frame median averaging the
pitch curve, which flattens a +-50 cent, 6 Hz vibrato by a few cents at its peaks. The
scorer will compare with a guide that itself follows the vibrato or sits in its middle; a
few cents is far inside the 50-cent full-credit band.

**Slides** (pitch moving linearly in cents between two notes held 0.3 s each; error over the
whole sung part):

| slide | speed (cents/s) | 16 kHz median / p95 / max | 48 kHz median / p95 / max |
|---|---|---|---|
| C3 -> C4 in 0.5 s | 2400 | 0.90 / 8.34 / 13.57 | 0.61 / 8.90 / 13.83 |
| C4 -> C3 in 0.5 s | 2400 | 0.92 / 8.79 / 11.46 | 1.02 / 9.51 / 11.37 |
| A3 -> A4 in 0.25 s | 4800 | 0.08 / 26.60 / 36.63 | 0.00 / 28.00 / 36.66 |
| G2 -> G3 in 1 s | 1200 | 1.32 / 4.02 / 5.51 | 1.66 / 4.03 / 5.72 |
| C5 -> D5 in 0.1 s | 2000 | 0.16 / 9.32 / 12.26 | 0.01 / 9.68 / 12.22 |
| A4 -> A5 in 0.3 s | 4000 | 0.20 / 22.13 / 28.77 | 0.01 / 22.48 / 28.83 |

No gaps and no gross errors in any slide at any rate. The largest errors are at the corners
where a fast slide starts or stops (the window straddles the corner).

**Breathy voices** (aspiration noise through the vowel, every fourth semitone):

| rate | breath noise vs voice | median / p95 / max (cents) | octave errors | voiced | worst note voiced |
|---|---|---|---|---|---|
| 16000 | -20 dB | 0.17 / 0.61 / 0.89 | 0 | 100.0 % | 100.0 % |
| 16000 | -12 dB | 0.54 / 2.29 / 3.59 | 0 | 100.0 % | 100.0 % |
| 16000 | -6 dB | 1.30 / 7.74 / 11.15 | 0 | 93.7 % | 64.4 % |
| 48000 | -20 dB | 0.11 / 0.31 / 0.75 | 0 | 100.0 % | 100.0 % |
| 48000 | -12 dB | 0.33 / 1.07 / 2.00 | 0 | 100.0 % | 100.0 % |
| 48000 | -6 dB | 0.82 / 4.02 / 6.25 | 0 | 100.0 % | 100.0 % |

At -6 dB (a whisper-like voice) and 16 kHz the tracker gives up on parts of the highest notes
rather than guessing. With the start threshold at 0.90 instead of 0.93 the same case produced
one octave-error event (exploration run, `JK_CLARITY_ON=0.9`), one reason for 0.93.

## 4. Not singing: no pitch, no wandering

Pitched events reported (must be 0); the longest run of analysis frames that passed the
start rule before the minimum-duration rule (`min_voiced_frames = 1` re-run); the highest raw
clarity seen. Same at all four rates; the worst rate is shown.

| signal | seconds per rate | pitched events | longest raw run | highest clarity (worst rate) |
|---|---|---|---|---|
| digital silence | 3 | 0 | 0 | 0.000 |
| breath, -25 dBFS | 30 | 0 | 0 | 0.913 (96 kHz) |
| breath, -12 dBFS (very close) | 30 | 0 | 0 | 0.902 (96 kHz) |
| speech-like noise (whispered vowels in syllables), -20 dBFS | 30 | 0 | 0 | 0.897 (16 / 48 kHz) |
| fricatives s / sh, -20 dBFS | 20 | 0 | 0 | 0.623 (16 kHz) |
| room noise -50 dBFS + 50 Hz hum -62 dBFS | 5 | 0 | 0 | 0.736 (48 kHz) |
| room noise -50 dBFS + 60 Hz hum -62 dBFS | 5 | 0 | 0 | 0.762 (44.1 kHz) |
| room noise -35 dBFS (loud) | 5 | 0 | 0 | 0.676 (16 kHz) |
| room noise + 50 Hz hum at -40 dBFS (above the level gate) | 5 | 0 | 0 | 0.439 (48 kHz) |
| room noise + 60 Hz hum at -40 dBFS (above the level gate) | 5 | 0 | 0 | 0.876 (16 kHz) |
| handling thumps 40..200 Hz, peak -1 and -12 dBFS (14) | 7 | 0 | 0 | 0.840 |
| popped plosives, peak -1 / -6 / -12 dBFS (9) | 4.5 | 0 | 0 | 0.150 |

Defence in depth, measured by lowering the start threshold on purpose (exploration runs,
same signals): at 0.85 only the speech-like noise produced pitched events (2 to 5 in 30 s,
per rate); at 0.80 it produced 9 to 18. Breath, thumps, pops, fricatives and room noise gave
none even at 0.80: the continuity and minimum-duration rules stop them. The false pitches of
the whispered vowels sat at 260..310 Hz, the first formant of "ee" and "oo" (noise through a
narrow resonance is nearly periodic over a window). The synthetic whisper uses the narrow
formant bandwidths of voiced speech (60 Hz); real whispers have wider ones, so this is a
pessimistic case.

**Minimum duration** (loud -6 dBFS tone of a given held length plus 10 ms ramps each side,
alone in silence; the shortest held length, in 5 ms steps, that gives any pitched event, worst
of 4 onset positions):

| pitch | 65.4 Hz | 82.4 Hz | 110 Hz | 150 Hz | 220 Hz | 440 Hz | 880 Hz |
|---|---|---|---|---|---|---|---|
| 16 kHz: shortest burst shown (ms) | 35 | 35 | 40 | 35 | 20 | 10 | 10 |
| 48 kHz | 35 | 35 | 35 | 35 | 20 | 10 | 10 |

In the range where handling noise lives (below 160 Hz) a periodic sound must last about
55 ms with its ramps before it shows; higher, a window fills with periods sooner, so a
30 ms sung blip at 440 Hz does show. Thumps and pops are not periodic enough to show at all
(table above).

## 5. Delay

Onset = first sample of a 10 ms ramp after 0.3 s of silence; 10 onset positions spread over
one hop; every semitone C2..C6, vowel ah. "Correct" = within 50 cents (the right note) or
within 10 cents. "Raw" = the same tracker with `min_voiced_frames = 1`. "Release" = from the
start of the 10 ms release ramp to the first "no pitch" event. Delays are measured to the
event's `ready_at`, i.e. the moment the event exists; device buffering and drawing come on
top and belong to Stage 3.

| rate | register | to 50 c: median / max (ms) | to 10 c: median / max | raw to 50 c: median / max | release: median / max |
|---|---|---|---|---|---|
| 16000 | C2-B2 | 57.0 / 62.0 | 57.0 / 62.0 | 47.0 / 52.0 | 30.0 / 43.0 |
| 16000 | C3-B3 | 45.0 / 61.0 | 45.0 / 61.0 | 35.0 / 51.0 | 43.0 / 51.0 |
| 16000 | C4-B4 | 31.0 / 44.0 | 31.0 / 44.0 | 21.0 / 34.0 | 50.0 / 56.0 |
| 16000 | C5-C6 | 25.0 / 34.0 | 25.0 / 34.0 | 15.0 / 24.0 | 54.0 / 60.0 |
| 16000 | all | 35.0 / 62.0 | 35.0 / 62.0 | 25.0 / 52.0 | 47.0 / 60.0 |
| 44100 | all | 36.0 / 62.0 | 36.0 / 62.0 | 26.0 / 52.0 | 47.0 / 60.0 |
| 48000 | C2-B2 | 57.0 / 62.0 | 57.0 / 62.0 | 47.0 / 52.0 | 30.0 / 43.0 |
| 48000 | C3-B3 | 45.0 / 60.0 | 45.0 / 60.0 | 35.0 / 50.0 | 43.0 / 51.0 |
| 48000 | C4-B4 | 32.0 / 49.0 | 32.0 / 49.0 | 22.0 / 39.0 | 50.0 / 56.0 |
| 48000 | C5-C6 | 25.0 / 34.0 | 25.0 / 34.0 | 15.0 / 24.0 | 54.0 / 60.0 |
| 48000 | all | 36.0 / 62.0 | 36.0 / 62.0 | 26.0 / 52.0 | 47.0 / 60.0 |
| 96000 | all | 36.0 / 62.0 | 36.0 / 62.0 | 26.0 / 52.0 | 47.0 / 60.0 |

(44.1 and 96 kHz per register are identical to 48 kHz within 5 ms; full table in
`bench-final.txt`.) The first correct event is already within 10 cents: the tracker does not
creep onto the note.

**Where the delay comes from.** The same tracker re-run with an analysis after every input
sample (hop = 1 sample, `min_voiced_frames = 1`) gives the window's share alone: the time
from the onset until the window holds enough of the note for clarity 0.93.

| rate | C2-B2 | C3-B3 | C4-B4 | C5-C6 | all |
|---|---|---|---|---|---|
| window fill, median / max (ms), 16 kHz | 43.8 / 44.8 | 29.6 / 43.1 | 14.1 / 19.4 | 9.6 / 10.6 | 19.4 / 44.8 |
| window fill, median / max (ms), 48 kHz | 44.0 / 44.8 | 29.7 / 43.1 | 14.0 / 19.4 | 9.3 / 10.6 | 19.4 / 44.8 |

So: total = window fill (9 ms for high notes, the whole 42.7 ms window plus the 2 ms the onset
ramp needs for the lowest) + the wait for the next hop (0..10 ms, 5 ms on average) + one hop
(10 ms) for the minimum run and the median. Worst case 44.8 + 10 + 10 = 65 ms; measured
max 62 ms. A shorter window would cut the low-register delay but not resolve C2 (trap 5); a
5 ms hop would save about 7 ms on average at twice the processor cost. Neither is needed for
the 100 ms limit.

## 6. How the voicing thresholds were chosen

Raw clarity of held voices with vibrato (+-50 cents, 5.5 Hz), every semitone, three vowels:

| rate | voice | p1 | p5 | median |
|---|---|---|---|---|
| 16000 | clean | 0.883 | 0.951 | 0.995 |
| 16000 | breathy -12 dB | 0.595 | 0.724 | 0.976 |
| 16000 | breathy -6 dB | 0.409 | 0.546 | 0.933 |
| 48000 | clean | 0.885 | 0.952 | 0.997 |
| 48000 | breathy -12 dB | 0.676 | 0.795 | 0.985 |
| 48000 | breathy -6 dB | 0.484 | 0.631 | 0.966 |

The highest clarity on any not-singing signal was 0.913 (section 4). Starting at 0.93 sits
above that and below what a held voice reaches within about 25 ms; keeping at 0.75, but only
while the pitch continues the line, keeps vibrato and breathy notes whole (100 % voiced at
-12 dB breath) without letting a breath after a note drag the marker around. Measured cost of
0.93 instead of 0.90: +3 ms median onset delay (33 -> 36 ms), max 61 -> 62 ms. At 0.95 the
median would be 41 ms and the -6 dB breathy voice would lose whole notes at 16 kHz.

## 7. Same answer at every sample rate

A 40 s generated melody (notes E2..G#5, steady, vibrato and slides, 0.1 s gaps) at each rate;
event k describes the same moment at every rate (within half a 16 kHz sample). Compared with
48 kHz:

| rate | events | voicing differs | both voiced | median abs diff (cents) | p95 | max |
|---|---|---|---|---|---|---|
| 16000 | 4000 | 3 | 3521 | 0.08 | 0.63 | 6.14 |
| 44100 | 4000 | 0 | 3522 | 0.00 | 0.03 | 0.18 |
| 48000 | 4000 | 0 | 3522 | 0.00 | 0.00 | 0.00 |
| 96000 | 4000 | 1 | 3522 | 0.02 | 0.15 | 1.15 |

The differences are at note edges (a voicing decision one frame apart) and on the fastest
slides; 16 kHz differs most because its signal has no content above 7.2 kHz.

## 8. Processor cost

60 s of the generated melody fed in device-sized blocks, one thread, best and median of 5
runs. Block times: p50 / p99 / max of single `process()` calls over all runs (a 10 ms block
holds exactly one analysis; the max includes the OS pre-empting the thread). Final run
(`bench-final.txt`; CPU load 60 % from other work right after it):

| rate | block | window / FFT | best ms per s of audio | median | share of one core | block p50 / p99 / max (us) |
|---|---|---|---|---|---|---|
| 16000 | 160 | 683 / 2048 | 0.419 | 0.423 | 0.042 % | 4.1 / 6.0 / 130 |
| 16000 | 512 | 683 / 2048 | 0.415 | 0.416 | 0.041 % | 12.6 / 18.4 / 93 |
| 44100 | 441 | 1882 / 4096 | 0.955 | 0.972 | 0.096 % | 9.5 / 13.7 / 129 |
| 44100 | 512 | 1882 / 4096 | 0.949 | 0.961 | 0.095 % | 9.9 / 20.5 / 78 |
| 48000 | 480 | 2048 / 4096 | 0.999 | 1.013 | 0.100 % | 9.9 / 15.8 / 101 |
| 48000 | 512 | 2048 / 4096 | 0.991 | 1.019 | 0.099 % | 10.1 / 18.0 / 131 |
| 96000 | 960 | 4096 / 8192 | 2.199 | 2.356 | 0.220 % | 23.5 / 31.9 / 141 |
| 96000 | 512 | 4096 / 8192 | 2.293 | 2.439 | 0.229 % | 17.4 / 28.6 / 148 |

Two earlier runs while the PC was at 100 % load from the other Stage 0 experiments
(`cpu-runs.txt`): 1.56 to 1.82 ms per second at 48 kHz, 3.3 to 3.9 ms at 96 kHz, single
blocks up to 36 ms when the thread was pre-empted. That is why WO2 4.8 puts tracking on its
own thread, off the audio callback: the callback only copies samples into a ring.

**MacBook Air M1 (estimate, not measured).** The cost is one forward and one inverse real
FFT plus a few linear passes per 10 ms. Assume x2 to x3 this PC's time: about x1.4 for the
M1's lower single-thread speed (public Geekbench 6 single-core scores, roughly 2,350 against
3,350) and up to x2 for RustFFT's 128-bit NEON kernels against 256-bit AVX here. That gives
about **2 to 3 ms per second of audio at 48 kHz (0.2 to 0.3 % of one core), 20 to 30 us per
analysis**. The self-check addition in WO2 4.8 ("the pitch tracker run on a generated tone
... with its speed on this machine") will measure it.

## 9. What this does not show

- **No human voice and no microphone.** Every signal is synthetic (`jk_pitch::tracker::testvoice`:
  a band-limited pulse train through a spectral tilt and four formant resonators). Real voices
  have jitter, shimmer, irregular onsets and breathiness that vary within a note; the owner's
  first session (WO2 section 13 checklist) is the real test.
- **The mixer's effects and music in the input are not measured here**: reverb,
  compression, the voice on one of two channels at the mixer's rates, and the app's own
  playback coming back through loopback belong to the owner's-rig tests (WO2 section 9, row
  "The owner's rig"). The interleaved entry point is tested (section 10), not the mixer.
- **A steady buzz above the level gate is a pitch.** Mains hum with natural harmonics gave no
  pitch even at -40 dBFS (50 Hz is filtered and below the range; 60 Hz is below the range),
  but a buzz dominated by 100 or 120 Hz and louder than -55 dBFS would be reported as that
  note: the tracker cannot tell it from a voice. The rig check (WO2 4.8) measures the noise
  floor.
- **Short high blips show.** The minimum duration is in analysis frames, so a periodic sound
  above about 200 Hz shows after 20 to 30 ms (section 4). Only low-frequency handling noise is
  required not to show, and it does not.
- **Delay is the tracker's own.** Device buffering, the input's reported latency and drawing
  add to it; Stage 3 stamps blocks against the playback clock (WO2 4.4).
- **M1 cost is an estimate** (section 8), not a measurement.

## 10. Unit tests (Stage 0; the tests the hardening added are in section 16)

`cargo test -p jk-pitch` runs, for the tracker (debug build, about 6 s plus 15 s for the
allocation test):

| Test | Checks |
|---|---|
| `tracker::tests::geometry_is_sized_in_time` | window and hop in samples at 16 / 44.1 / 48 / 96 kHz; `describe_delay` |
| `tracker::tests::one_event_per_hop_with_consistent_times` | one event per hop; `ready_at`, `time` |
| `tracker::tests::steady_notes_every_semitone_c2_to_c6` | 48 kHz, every semitone, vowels cycling: no gaps, no octave errors, max error <= 5 cents |
| `tracker::tests::steady_notes_at_other_rates` | 16, 44.1, 96 kHz, every other semitone, the same bounds |
| `tracker::tests::vibrato_is_followed` | 5 / 5.5 / 6 Hz vibrato at E2, A3, A4, A5, 16 and 48 kHz: no gaps, median <= 6, max <= 15 cents |
| `tracker::tests::slides_are_followed` | octave glides up and down: no gaps, every event within 50 cents, p95 <= 15 |
| `tracker::tests::not_singing_gives_no_pitch` | silence, breath (two levels), speech-like noise, fricatives, room noise with 50 / 60 Hz hum, loud room noise, thumps, plosives at 16 and 48 kHz: 0 pitched events |
| `tracker::tests::short_low_bursts_are_not_notes` | loud periodic bursts at 65..155 Hz held 25 ms (plus 10 ms ramps) give nothing; held 80 ms they are found |
| `tracker::tests::onset_delay_within_budget` | every third semitone x 5 onset positions: max <= 100 ms, median <= 50 ms, release max <= 100 ms, no pitch before the onset |
| `tracker::tests::same_answer_at_every_rate` | 8 s melody: same event times, voicing differs on <= 1 %, p95 <= 1 cent, max <= 15 cents vs 48 kHz |
| `tracker::tests::one_channel_of_an_interleaved_stream` | the voice on channel 0 of 2, 1 of 2 and 2 of 4 gives bit-identical events to the voice alone; the noise channel gives no pitch |
| `tracker::tests::block_size_and_reset_do_not_change_the_answer` | sample-by-sample, whole-buffer and random block sizes give identical events; `reset()` equals a new tracker |
| `tracker::tests::non_finite_input_is_silence` | NaN / infinity from a broken driver count as silence and do not poison the filter |
| `tracker::tests::level_gate` | a voice at -70 dBFS gives nothing, at -45 dBFS it is tracked |
| `tests/tracker_alloc.rs` (3 tests) | 0 allocations or frees on the processing thread over 20 s at every rate (with a `reset()` in between) and over an interleaved stream; the counter itself is checked |

## 11. How to reproduce (Stage 0; the current commands are in section 18)

```sh
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-tracker"
cargo test -p jk-pitch                                         # unit + allocation tests
cargo run --release -p jk-pitch --example tracker_bench        # every table above (about 3 min)
cargo run --release -p jk-pitch --example tracker_bench cpu    # one section
JK_CLARITY_ON=0.85 cargo run --release -p jk-pitch --example tracker_bench noise   # exploration
cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin   # Mac type-check
```

## 12. Independent verification (Stage 0 verifier, 2026-11-19)

A second agent re-ran everything above and added probes that the bench does not cover.
Probe source and raw outputs: `../jukeoke-build/practice-tracker-verify/` (`probe/` is a
small crate with a path dependency on `jk-pitch`; `realvoice.py` runs the real-voice
comparison). The tracker code was not changed.

**Reproduced exactly.** `cargo test -p jk-pitch` (23 + 3 passed), clippy (no warnings except
the shared `libm` manifest one), the Mac type-check, and every bench table in sections 2 to 8
(`bench-verify.txt`: same numbers; 48 kHz cost 0.98 ms per second of audio).

**Octave errors outside the held part (synthetic voice, default settings).** Every
semitone C2..C6, three vowels, the whole note including onset and release; events more than
50 cents off:

| voice | 16 kHz | 44.1 kHz | 48 kHz | 96 kHz | where |
|---|---|---|---|---|---|
| clean | 14 | 14 | 12 | 10 | all at onsets: high "oo" notes start an octave (or an octave and a fifth) low for 10 to 20 ms |
| breath -20 dB (a mildly breathy voice) | 22 (13 in held part) | 21 (11) | 18 (8) | 12 (0) | held "ee" / "oo" notes in C5..C6 drop an octave or more |
| breath -12 dB | 165 (138) | 126 (104) | 133 (72) | 47 (37) | mostly octave-low |

Section 3's breathy table shows 0 because it uses only the vowel "ah" and every fourth
semitone. With cycle-to-cycle jitter 0.5 % and shimmer 3 % (an ordinary healthy voice) plus
slow drift, held notes still give 2 octave-error events per rate (16 and 48 kHz).

**Real voices.** 60 s excerpts of four ccMixter lead stems (eval set, Creative Commons; IDs
22364, 31416, 55540, 68969; one channel, never summed), compared with librosa pYIN:

| stem | pYIN-voiced frames the tracker voices (all / inside notes) | tracker-voiced frames pYIN calls voiced | within 50 cents | octave-off events |
|---|---|---|---|---|
| 22364 | 78 % / 84 % | 100 % | 99.9 % | 0 |
| 31416 | 86 % / 94 % | 99.9 % | 96.9 % | 58 (3 segments, tracker an octave high) |
| 55540 | 75 % / 87 % | 100 % | 98.8 % | 0 (1 event more than 100 cents off) |
| 68969 | 42 % / 63 % | 99.7 % | 96.3 % | 13 (2 segments, tracker an octave high) |

A spectral check, which looks for the odd harmonics of the lower pitch, sides with pYIN on all
71 octave-off frames. On real voices the error is octave **high**. The 68969 stem's median
clarity on sung frames is 0.85, below the 0.93 start threshold, so the marker would be missing
for about a third of clearly sung frames.

**Peak threshold k = 0.8 instead of 0.9 does not fix this.** It removes the synthetic
octave-low errors: clean 0 to 1 per rate, jittered 0, and it keeps every bench table within
its criteria. But it doubles the octave-high segments on stem 31416 (58 to 114 events).
Lowering `clarity_on` to 0.85 raises recall on 68969 to 56 %, but it also raises the
octave-off events there (21 to 53). A better octave decision than the single relative
threshold is needed before Stage 3. Examples are a harmonic or odd-harmonic check of the
chosen period, or picking the octave by continuity over a few frames.

**Other probes:**
- Reverb: a Schroeder reverb on an 8 s melody. RT60 0.8 s at -12 dB keeps 99.6 % of note cores
  voiced with 0 wrong events. RT60 1.2 s at -6 dB gives 93 % voiced and 4 to 6 wrong.
- Long noise (120 s at 16, 48 and 96 kHz): 0 pitched events. The highest clarity was 0.931,
  from one 48 kHz breath frame, above the 0.93 start threshold, so the margin is thin.
- Tones above 1400 Hz are reported an octave low (1450 Hz as 725, 2000 Hz as 1000) instead of
  as no pitch. This is outside the required range, but it is a wrong answer, not a missing one.
- Other sample rates (8, 11.025, 22.05, 32, 88.2 and 192 kHz) track steady notes within
  3 cents. At 11.025 and 22.05 kHz the hop is 110 or 221 samples, so events are not exactly
  10 ms apart. `time` is in samples, so consumers must convert with the rate.

## 13. Hardening on real voices (WO2 tracker hardening, 2026-11-19)

The Stage 0 verifier (section 12, DECISIONS D-238) found the tracker not ready for real
singing: octave-low flashes at the onsets of high "oo"/"ee" notes, octave errors inside
held notes with mild breath or ordinary jitter, octave-high stretches on real lead stems,
and only 63-94 % of real sung frames voiced. This section is the real-voice evaluation
built for the hardening, the method changes, and the numbers before and after. Everything
was tuned on the **dev** split only; the **held-out** split was run once at the end.

### 13.1 The evaluation set

Prepared by `crates/jk-pitch/examples/tracker_eval_prep.py` (run once with the guide
benchmark's Python venv; output under `../jukeoke-build/practice-tracker/eval/`, nothing in
the repository). One channel of each source (channel 0, never a sum), at its own rate
(44.1 kHz; one song at 48 kHz), the 60 s with the most singing for long tracks.

| source | licence | clips dev / held-out | minutes dev / held-out | reference |
|---|---|---|---|---|
| vocadito (40 solo a-cappella recordings, 29 singers) | CC BY 4.0 | 23 / 17 | 7.9 / 5.7 | the set's own f0 annotations (Annotations/F0), 10 ms grid |
| ccMixter lead stems (eval set) | Creative Commons | 22 / 19 | 22.0 / 19.0 | agreed offline reference (below) |
| MUSDB18-HQ test vocal stems | research licence, local only | 23 / 27 | 23.0 / 27.0 | agreed offline reference |
| the three CC test songs' isolated lead (Studio separation) | Creative Commons | 0 / 3 | 0 / 3.0 | agreed offline reference |

*Agreed offline reference* (every source but vocadito): librosa pYIN and torchcrepe "full"
(Viterbi), both on 10 ms frames. A frame is **voiced** when pYIN voices it and CREPE is
within 50 cents of pYIN (f0 = pYIN's), **unvoiced** when pYIN does not voice it and CREPE's
periodicity is under 0.21, **undecided** otherwise (left out of every metric): 11-26 % of the
frames per set. MUSDB "vocals" stems contain backing vocals, doubles and effects, so they
are the hardest and least like the owner's single microphone.

*Split*: by singer or artist (a stable hash, about 45 % dev); no singer or artist is in
both (checked: 0 shared groups). The ccMixter artist of one of the three test songs and the
songs themselves are always held out, which puts 14 of that artist's 19 ccMixter stems in
the held-out ccMixter set; those stems are heavily processed (the Stage 0 verifier's stem
with median clarity 0.85 is one of them) and pull the held-out ccMixter numbers down.

*Metrics* (`crates/jk-pitch/examples/tracker_eval.rs`; each event matched to the reference
frame at its described `time`):

- **VR** voicing recall: reference-voiced frames the tracker voices (frames whose reference
  flag is the same 20 ms either side, so a 10 ms disagreement about a note edge is not
  counted); **VFA** voicing false alarm: such reference-unvoiced frames it voices;
- **RPA** raw pitch accuracy: reference-voiced frames voiced within 50 cents (what the marker
  shows); **Acc**: of the frames both voice, within 50 cents;
- **Oct**: of the frames both voice, an octave or more off (in brackets: the tracker above);
- **flips**: the marker jumping 600 cents or more between consecutive events while the
  reference moves less than 300 cents (an octave jump on a held pitch);
- **onsets**: reference onsets (100 ms unvoiced, then 150 ms voiced); **delay**: onset to the
  `ready_at` of the first event within 50 cents of the note; **found**: delay <= 100 ms;
  **onset flashes**: events an octave or more off in a note's first 150 ms (in brackets in
  the raw tables: onsets with any).

### 13.2 Results: Stage 0 against the hardened tracker

Both trackers on the same clips (Stage 0 = the tracker of git HEAD a211234, built from a
private copy, `../jukeoke-build/practice-tracker/devcrate-stage0/`). Raw tables:
`runs/final/{new,stage0}-{dev,heldout}.{txt,tsv}`; these tables by `runs/final/report.py`.

**Dev split (tuned on), clean input**

| set | clips | min | VR % | VFA % | RPA % | Acc % | Oct % (high) | flips | onsets | delay p50 / p90 ms | found % | onset flashes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ccmixter | 22 | 22.0 | 90.9 -> **95.7** | 0.07 -> 0.08 | 85.2 -> **90.9** | 99.29 -> 99.33 | 0.45 (0.28) -> **0.25 (0.04)** | 37 -> **28** | 117 | 80 / 360 -> 80 / 110 | 75.2 -> 84.6 | 0 -> 0 |
| musdb | 23 | 23.0 | 65.6 -> **79.9** | 0.18 -> 0.43 | 56.7 -> **71.0** | 97.99 -> 98.61 | 1.69 (1.18) -> **0.91 (0.71)** | 61 -> **31** | 50 | 100 / 500 -> 90 / 300 | 50.0 -> 64.0 | 0 -> 0 |
| vocadito | 23 | 7.9 | 93.3 -> **95.3** | 3.58 -> 5.02 | 88.0 -> **91.0** | 98.62 -> 99.09 | 0.80 (0.62) -> **0.21 (0.17)** | 23 -> **4** | 386 | 60 / 110 -> 70 / 110 | 89.4 -> 86.8 | 11 -> 4 |
| **all** | 68 | 52.9 | 81.6 -> **89.6** | 0.94 -> 1.36 | 74.2 -> **82.9** | 98.76 -> 99.04 | 0.90 (0.63) -> **0.47 (0.29)** | 121 -> **63** | 553 | 70 / 180 -> 70 / 120 | 82.8 -> 84.3 | 11 -> 4 |

**Held-out split (never tuned on), clean input**

| set | clips | min | VR % | VFA % | RPA % | Acc % | Oct % (high) | flips | onsets | delay p50 / p90 ms | found % | onset flashes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ccmixter | 19 | 19.0 | 65.0 -> **74.6** | 0.08 -> 0.16 | 50.1 -> **59.4** | 98.84 -> 98.34 | 0.33 (0.26) -> **0.29 (0.23)** | 12 -> **7** | 61 | 80 / 500 -> 80 / 120 | 67.2 -> 72.1 | 0 -> 0 |
| musdb | 27 | 27.0 | 67.0 -> **78.4** | 0.06 -> 0.15 | 58.9 -> **70.5** | 98.55 -> 99.06 | 1.26 (0.36) -> **0.61 (0.21)** | 32 -> **22** | 85 | 80 / 500 -> 80 / 230 | 57.6 -> 70.6 | 0 -> 0 |
| songs (3 CC test songs, separated lead) | 3 | 3.0 | 50.3 -> **62.8** | 0.00 -> 0.13 | 38.1 -> **48.9** | 98.84 -> 98.07 | 0.20 (0.20) -> **0.08 (0.08)** | 2 -> **1** | 8 | 120 / 500 -> 110 / 500 | 25.0 -> 50.0 | 0 -> 0 |
| vocadito | 17 | 5.7 | 95.3 -> **96.4** | 6.57 -> 10.05 | 91.3 -> **92.7** | 99.19 -> 99.09 | 0.18 (0.11) -> **0.17 (0.10)** | 5 -> 7 | 288 | 60 / 100 -> 70 / 100 | 91.3 -> 89.2 | 10 -> 12 |
| **all** | 66 | 54.7 | 69.7 -> **79.1** | 1.05 -> 1.65 | 58.8 -> **68.3** | 98.77 -> 98.81 | 0.74 (0.28) -> **0.42 (0.19)** | 51 -> **37** | 442 | 70 / 190 -> 80 / 120 | 80.3 -> 82.6 | 10 -> 12 |

What this says:

- **Voicing**: +8 points of reference-voiced frames shown on dev, +9.4 on held-out; the
  marker shows a real sung note within 100 ms of its onset more often (84 / 83 %) and the
  slow tail is gone (p90 delay 180 -> 120 ms dev, 190 -> 120 ms held-out). The cost is more
  false alarms, mostly on vocadito, whose annotation stops at note ends while a held vowel
  still rings in the 43 ms window (VFA 5.0 % dev, 10.1 % held-out; 0.08-0.43 % on the stems).
- **Octaves**: octave errors halved (0.90 -> 0.47 % dev, 0.74 -> 0.42 % held-out), the
  octave-high stretches the verifier found are mostly gone (0.63 -> 0.29 %, 0.28 -> 0.19 %),
  and the marker jumps an octave on a held pitch half as often (121 -> 63, 51 -> 37 in about
  50 minutes each). Onset flashes on real voices stay rare (4 dev, 12 held-out, in 1 and 3
  of 995 onsets).
- **Held-out is harder than dev** for both trackers (58.8 against 74.2 % RPA for Stage 0),
  mostly the one heavily processed ccMixter artist (above) and the separated leads of the
  test songs; the improvement carries over (+9.5 points RPA against +8.7 on dev).
- **What is left**: on MUSDB the stems hold other voices (harmonies, doubles), and two notes
  a fifth apart share a period an octave below: most remaining octave errors there are
  octave-low on such frames. On the separated leads of the test songs, separation residue
  keeps clarity low: 63 % of the reference-voiced frames are shown.

### 13.3 What each change contributed (dev, clean, all sets)

Each row turns one part off in the final tracker (`--set` overrides of `tracker_eval`;
`runs/final/ablations-dev.txt`):

| tracker | VR % | VFA % | RPA % | Oct % (high) | flips | onset flashes | delay p50 / p90 |
|---|---|---|---|---|---|---|---|
| hardened (all of it) | 89.6 | 1.36 | 82.9 | 0.47 (0.29) | 63 | 4 | 70 / 120 |
| candidates scored by NSDF instead of the flattened spectrum (`flat_power=0`) | 89.6 | 1.38 | 82.5 | 0.92 (0.39) | 130 | 22 | 70 / 120 |
| no path costs (`jump_cost=0`, `jump_up_cost=0`) | 89.3 | 1.35 | 82.5 | 0.61 (0.38) | 247 | 6 | 70 / 120 |
| no steady start (`clarity_steady=2`) | 85.8 | 1.25 | 79.3 | 0.41 (0.21) | 54 | 4 | 80 / 140 |
| no 4 kHz low-pass (`lowpass_hz=0`) | 87.9 | 1.24 | 80.9 | 0.46 (0.28) | 58 | 4 | 80 / 120 |
| Stage 0 tracker | 81.6 | 0.94 | 74.2 | 0.90 (0.63) | 121 | 11 | 70 / 180 |

### 13.4 The method, and why

The analysis window, hop, high-pass, NSDF and pitch range are unchanged from Stage 0. New:

1. **Analysis low-pass at 4 kHz** (4th-order Butterworth): above it a sung vowel is mostly
   breath, sibilance and the mixer's "air"; without it a bright channel EQ cost 8 points.
2. **Candidates instead of one choice**: every NSDF key maximum reaching half the highest
   (at most six, shortest periods first, the highest always kept).
3. **Octave decision on a flattened spectrum.** On the dev frames where Stage 0 was an octave
   off, the NSDF could not tell the cases apart: real voices with weak odd harmonics (true
   pitch the lower octave; NSDF 0.93 at half the period against 0.99 at the period) and
   voices with a second voice, a reverb tail or a rough stretch (true pitch the upper
   octave; NSDF 0.82 against 0.94) look alike to any per-frame NSDF rule (best per-frame rule
   over 2,500 error frames: no better than Stage 0's). Each candidate is now scored by the
   autocorrelation of `|X|^0.5` (0-5 kHz, the same band at every rate), normalised for the
   window overlap: compression lets odd harmonics 15 dB down count, and noise in a narrow
   band no longer looks periodic at a multiple of the period. Less `octave_cost` (0.06) per
   octave below MPM's own choice.
4. **Viterbi path search with a fixed lag of two frames** (20 ms): stepping down an octave
   between frames costs 0.2, up 0.1 (asymmetric: after a leap up the old lower octave stays
   a valid period of the new note, after a leap down the old upper one usually does not).
   While a note is shown the path is **pinned** to what was shown (fixed-lag decoding alone
   switched octave between consecutive decisions). A note **starts on the octave of the
   newest window** (`anchored`): the first windows of a note hold silence and formant ringing,
   which made the octave below look more periodic for 10-20 ms (the onset flashes).
5. **Voicing**: Stage 0's confident start (clarity 0.93, then 0.75) and keep rules, plus a
   **steady start**: five frames (20 ms before the decided frame to the newest) each with
   clarity >= 0.8, within 40 cents of each other, at least 160 Hz. Real voices whose clarity
   stays at 0.8-0.93 (rough, breathy, processed) hold a steady pitch; breath, whispered
   vowels and noise wander, and thumps live below 160 Hz.
6. Delay: the event describes the frame two hops before the newest (`describe_delay` = half a
   window + 2 hops = 41 ms). Measured onset delay on generated notes: median 46 ms, max 72 ms
   (section 14); on real voices median 70-80 ms from the reference onset (which marks the
   first voiced 10 ms frame, before the clarity of a real onset builds up).

**Tried and dropped** (all measured on dev, `--set` overrides; numbers in the session log):

- *Octave cost and a continuity bonus on the NSDF* (the interrupted first attempt): moved
  errors from octave-high to octave-low (raw octave errors 2.94 % -> 2.61 % at best).
- *Onset segmentation* (analysing only the sound after a quiet stretch in the window): no
  measurable effect on real voices, and it broke low notes (the energy between glottal
  pulses of a C2 counted as "before the note"). Removed.
- *Adaptive noise-floor gate* (minimum statistics, 10-15 dB margin): cost 4-6 points of VR
  on clean input and up to 11 under heavy compression for 0.1-0.4 points less VFA. Removed.
- *Second-period check for steady starts*, *extra octave cost at note starts*, *resume
  bonus after a dip*, *look-ahead of one frame*: each lost on either the real-voice or the
  generated-noise numbers. A look-ahead of one frame was 10 ms faster and slightly better on
  dev, but let a generated whispered vowel through (5 pitched events in 30 s).
- *Large downward jump cost* (2.0): best on dev octave numbers, but a legato leap down from
  D4 to D3 on "oo" was never followed (the D4 period stays a fair candidate of a D3 "oo").
  0.2 follows every generated leap within 80 ms.
- *A small neural tracker*: **not tried.** The plain method reached the numbers above at
  17 us per analysis with no model; much of what is left is a second voice or separation
  residue in the stems (frames where pYIN and CREPE disagree are left out of the metrics
  altogether; on those the tracker is octave-off against CREPE on about 4 % of the frames it
  voices, section 19), and a neural model would add a runtime and a model file to the Mac
  build for an unmeasured gain.

## 14. Generated voices: Stage 0 against the hardened tracker

Same generators as sections 2-8 (`tracker_bench`) and the Stage 0 verifier's probes, now in
`crates/jk-pitch/examples/tracker_synth.rs`. Full outputs:
`runs/final/bench-wo2.txt` (hardened), `bench-final.txt` (Stage 0, unchanged), and
`runs/final/synth-{stage0,new}.txt`.

**Whole notes, onsets and releases included** (every semitone C2..C6, ah / ee / oo, 0.6 s;
events more than 50 cents off; "octave" = of those, octave errors):

| breath | rate | Stage 0: > 50 c / octave / in held part | hardened: > 50 c / octave / in held part |
|---|---|---|---|
| none | 16000 | 14 / 13 / 0 | **3 / 3 / 0** |
| none | 44100 | 14 / 14 / 0 | **6 / 6 / 0** |
| none | 48000 | 12 / 12 / 0 | **6 / 6 / 0** |
| none | 96000 | 10 / 10 / 0 | **9 / 9 / 0** |
| -20 dB (mild) | 16000 | 22 / 8 / 13 | 26 / **3** / 11 |
| -20 dB | 44100 | 21 / 5 / 11 | **9 / 3 / 0** |
| -20 dB | 48000 | 18 / 11 / 8 | **15 / 14 / 0** |
| -20 dB | 96000 | 12 / 12 / 0 | **9 / 9 / 0** |
| -12 dB (breathy) | 16000 | 165 / 156 / 138 | 447 / 403 / 316 |
| -12 dB | 44100 | 126 / 125 / 104 | 256 / 159 / 187 |
| -12 dB | 48000 | 133 / 118 / 72 | 283 / 193 / 183 |
| -12 dB | 96000 | 47 / 40 / 37 | 244 / 117 / 174 |

**Rough voices** (cycle-to-cycle jitter and shimmer, slow drift; held part; events more
than 600 cents off; voiced share):

| rate | jitter / shimmer | breath | Stage 0: voiced / > 600 c | hardened: voiced / > 600 c |
|---|---|---|---|---|
| 16000 | 0.5 % / 3 % | none | 99.8 % / 2 | **100.0 % / 0** |
| 16000 | 1.0 % / 6 % | none | 98.7 % / 2 | **99.3 % / 0** |
| 16000 | 1.0 % / 6 % | -20 dB | 92.1 % / 7 | 97.6 % / 87 |
| 16000 | 2.0 % / 10 % | none | 85.5 % / 4 | 91.7 % / 18 |
| 16000 | 2.0 % / 10 % | -15 dB | 75.6 % / 37 | 84.5 % / 111 |
| 48000 | 0.5 % / 3 % | none | 99.9 % / 2 | **100.0 % / 0** |
| 48000 | 1.0 % / 6 % | none | 99.0 % / 6 | **99.4 % / 0** |
| 48000 | 1.0 % / 6 % | -20 dB | 94.9 % / 4 | **99.0 % / 0** |
| 48000 | 2.0 % / 10 % | none | 90.3 % / 3 | **93.2 % / 0** |
| 48000 | 2.0 % / 10 % | -15 dB | 79.9 % / 15 | 86.6 % / 46 |

**Legato** (A then B with no gap, intervals 1..19 semitones from five base notes, targets
below C2 skipped): events off both notes 1 / 10 / 10 (ah / ee / oo, 16 kHz; Stage 0 1 / 9 /
9); change to the first event on B median 60-70 ms, max 80 ms (Stage 0 50-60 / 70); octave
leaps up and down reach the new note within 80 ms at 16 and 48 kHz (Stage 0 60 ms).

What this says: the verifier's findings on generated voices are fixed where they were
reported (onset flashes 10-14 per rate -> 3-9; ordinary jitter 2-6 -> 0; mild breath with
jitter at 48 kHz 4 -> 0; held-part octave errors with mild breath 8-13 -> 0 except 16 kHz).
**Not fixed, and worse than Stage 0: strongly breathy high "ee" / "oo" notes (C5 and up)
and mild breath at 16 kHz**, which come out an octave or a twelfth low for whole notes. The
generator's breath noise goes through the vowel's formant filters (60 Hz wide), so it rings
at the first formant (270-300 Hz), right on the subharmonic of those notes; at 16 kHz the
same "-20 dB" puts about 5 dB more noise in the analysed band than at 48 kHz. The
hardened tracker also voices far more of these notes (8,359 against 7,758 events at
48 kHz, -12 dB), so it has more chances to be wrong. Real breathy voices have wider formants;
on the real-voice sets octave errors went down (section 13).

**The Stage 0 bench re-run with the hardened tracker** (`runs/final/bench-wo2.txt`):

| measure | Stage 0 | hardened | criterion |
|---|---|---|---|
| steady notes C2..C6, every semitone, 3 vowels, held part: max error / octave errors / voiced | 2.38 c / 0 / 100 % | **2.36 c / 0 / 100 %** | 10 cents, none |
| vibrato +-50 c at 5-6 Hz: median / max error | 2.8-4.1 / 12.2 c | 2.8-4.1 / 12.2 c, 0 octave errors | (report) |
| delay onset -> first correct event, every semitone x 10 onset positions: median / max | 36 / 62 ms | **46 / 72 ms** | aim 50, never > 100 |
| marker off after singing stops: median / max | 47 / 60 ms | 67 / 80 ms | (report) |
| not singing (silence, breath, whispered vowels, fricatives, room noise with hum, thumps, pops; 30 s each kind per rate) | 0 pitched events | **0 pitched events** | none |
| shortest periodic burst shown, 65-150 Hz | 35 ms | 35-45 ms | thumps never notes |
| same answer vs 48 kHz (40 s melody): voicing differs / p95 / max | 3 of 4000 / 0.63 / 6.1 c | 3 of 4000 / 0.68 / 28.8 c (16 kHz; 96 kHz 11.4 c) | same answer |
| breathy voice (ah, every 4th semitone), -6 dB breath: voiced / octave errors | 93.7 % (worst note 64.4 %) / 0 at 16 kHz | **100 % / 0** at 16 and 48 kHz | (report) |
| processor, 48 kHz, best of 5 | 1.0 ms per s (10 us per analysis) | **1.7-1.8 ms per s (18 us per analysis, 0.17 % of a core)** | (report) |
| no allocation per block | 0 | **0** (`tests/tracker_alloc.rs`) | none |

Processor: the same machine, same session, both trackers back to back
(`tracker_bench cpu`): 16 kHz 0.43 -> 0.89 ms per second of audio, 44.1 kHz 0.97 -> 1.66,
48 kHz 1.00 -> 1.72, 96 kHz 2.07 -> 3.32. The extra is the second inverse FFT (the flattened
spectrum) and the candidate bookkeeping. MacBook Air M1 estimate (x2-3, section 8): 3.5-5 ms
per second of audio, 0.35-0.5 % of one core. A first version computed `powf` per spectrum bin
and cost 11 ms per second: the MinGW runtime's `powf` is slow; square roots replaced it.

## 15. The owner's rig: effects on the voice (WO2 4.10)

Generated effects (`crates/jk-pitch/src/tracker/rig.rs`, test fixtures only) applied to every
real-voice clip before tracking (`tracker_eval --cond`): generated room impulse responses
(RT60 0.3 / 0.8 / 1.5 s, reverb at -6 dB, and 1.5 s at 0 dB), a voice compressor (-30 dBFS
4:1) and a "broadcast" one (-45 dBFS 10:1, +24 dB, limiter) over a -65 dBFS booth noise
floor, a bright channel EQ (80 Hz low cut, +5 dB at 3 kHz, +6 dB air) and a warm one, the
proximity bass of a handheld dynamic microphone (+12 dB below 200 Hz), the same with popped
plosives and handling thumps (30 and 10 per minute), everything at once ("rig": proximity,
pops, thumps, noise floor, bright EQ, heavy compression, RT60 1.0 s reverb at -8 dB), and the
clip resampled to 16 / 48 / 96 kHz (windowed sinc). RPA change against clean input:

| condition | dev: Stage 0 | dev: hardened | held-out: Stage 0 | held-out: hardened |
|---|---|---|---|---|
| clean (RPA %) | 74.2 | **82.9** | 58.8 | **68.3** |
| resampled to 16 / 48 / 96 kHz | +0.5 / 0.0 / 0.0 | -0.2 / 0.0 / 0.0 | +0.7 / 0.0 / 0.0 | -0.3 / 0.0 / 0.0 |
| compressor / heavy compressor | -0.4 / -0.5 | -0.4 / -0.6 | -0.5 / -0.7 | -0.6 / -0.9 |
| warm EQ / proximity / proximity + pops + thumps | +2.8 / +2.7 / +0.7 | +1.3 / +1.2 / -0.4 | +5.1 / +6.4 / +5.0 | +3.6 / +4.5 / +3.4 |
| bright EQ | -8.4 | **-5.3** | -8.9 | **-7.4** |
| reverb RT60 0.3 s / 0.8 s / 1.5 s (at -6 dB) | -7.0 / -15.0 / -22.3 | **-5.4 / -11.5 / -16.9** | -9.7 / -16.9 / -22.7 | **-8.7 / -14.5 / -19.2** |
| reverb RT60 1.5 s at 0 dB | -33.9 | -28.9 | -31.6 | -30.1 |
| the whole rig at once | -22.1 | **-14.7** | -20.9 | **-16.4** |

Under every condition the hardened tracker is ahead of Stage 0 in absolute terms (rig: 68.2
against 52.1 % RPA on dev). Of the frames shown, the share on the right note stays 97.0-99.1 %
except under reverb (95.0 % at 0.8 s, 93.8 % at 1.5 s: the previous note's tail), and octave
errors stay at 0.44-0.77 % (`runs/final/new-*.txt`). Pops and thumps added no pitched event
on their own (unit test `not_singing_gives_no_pitch`; the generated-melody test below).

**Within a few points: met** for sample rate, compression, warm EQ, proximity, pops and
thumps. **Not met** for a bright EQ (-5 to -7 points) and for reverb (-5 to -9 at RT60 0.3 s,
-12 to -19 at 0.8-1.5 s at -6 dB): the marker stays right where it shows, but shows less of
each note, mostly at note changes where the last note's tail is still as loud as the new
note. These reverbs are strong (the wet part at a quarter of the voice's energy); the
podcast mixer's reverb should be off, or low, on the feed to the computer, which is the
recommendation for the setup checklist (decision for the owner, below). The generated-melody
test (`mixer_effects_keep_the_note`, 44.1 kHz) measures note cores: RT60 0.3 s at -6 dB
100 % pitched, 1.0 s at -8 dB 95.6 %, 1.5 s at -6 dB 77.5 % (5 of 426 events off), all
others >= 90 % with none off.

## 16. Unit tests added by the hardening

`cargo test -p jk-pitch` (all tracker tests also pass in a debug build, 17 s):

| Test | Checks |
|---|---|
| `tracker::tests::whole_notes_onsets_included` | every semitone C2..C6 x 3 vowels at 16 and 48 kHz, the whole note: no octave error in any held part, at most 12 octave-off onset events in all (measured 3 and 6), no more octave jumps than wrong events |
| `tracker::tests::rough_voices_keep_their_octave` | jitter / shimmer 0.5 % / 3 % and 1 % / 6 %, every fourth semitone, 3 vowels, 16 and 48 kHz: no held event more than 600 cents off, no octave jump |
| `tracker::tests::octave_leaps_are_followed` | legato octave up and down from C3..A4, 3 vowels, 16 and 48 kHz: the new note within 100 ms |
| `tracker::tests::mixer_effects_keep_the_note` | a melody through three reverbs, heavy compression with a noise floor, two channel EQs, proximity with pops and thumps: note cores >= 90 % pitched and on the note (>= 75 %, <= 2 % off under RT60 1.5 s at -6 dB) |
| `tracker::tests::geometry_is_sized_in_time` (changed) | `describe_delay` = half a window + two hops |

Every Stage 0 test still passes unchanged (steady notes, vibrato, slides, not singing,
short low bursts, onset delay, same answer at every rate, one channel of an interleaved
stream, block size and reset, non-finite input, level gate, and the three allocation tests).

## 17. Decisions, and what is still open

Decisions made here (to be recorded in DECISIONS.md by the coordinator):

- Live tracker = MPM candidates + flattened-spectrum octave scoring + fixed-lag (2 frames)
  Viterbi with asymmetric octave costs and a pinned path + confident or steady start + 4 kHz
  analysis low-pass; describe delay 41 ms; 1.7 ms per second of audio at 48 kHz.
- Real-voice evaluation (vocadito annotations, ccMixter, MUSDB18-HQ, test songs; pYIN+CREPE
  agreed reference; singer-disjoint dev / held-out) is the tracker's acceptance test from now
  on (`tracker_eval`), next to the generated bench (`tracker_bench`, `tracker_synth`).
- No neural tracker.

Open:

- **Reverb and bright EQ on the mixer feed lower the share of each note shown** (section 15).
  Proposed for the owner's checklist: mixer reverb and "presence/air" EQ off on the USB feed
  while practising (the owner still hears them in the EarPods only if the mixer allows a
  separate monitor mix; this depends on the AME2 manual, WO2 4.10).
- **Strongly breathy high "ee"/"oo" voices** can be shown an octave or a twelfth low for whole
  notes (generated voices, section 14); not seen in the real-voice sets.
- **vocadito false alarms** (5-10 % of its annotated unvoiced frames, the end of held
  vowels): the scorer should not penalise a marker that lingers 20-40 ms after a note.
- MUSDB stems with harmonies keep an octave-low error rate of 0.6-0.9 %; the owner's input is
  one voice, so this matters only with music in the input (the echo workstream's removal).

## 18. How to reproduce the hardening numbers

```sh
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-tracker"
cargo test -p jk-pitch                                     # unit + allocation tests
# once: the real-voice set (needs the guide benchmark's venv with torch, torchcrepe, librosa,
# soxr; reads $JK_BUILD/eval, $JK_BUILD/practice-guide; writes $JK_BUILD/practice-tracker/eval)
"$JK_BUILD/practice-guide/venv/Scripts/python.exe" -I crates/jk-pitch/examples/tracker_eval_prep.py
cargo run --release -p jk-pitch --example tracker_eval -- --split dev               # tune here
cargo run --release -p jk-pitch --example tracker_eval -- --split heldout \
    --cond clean,rev03,rev08,rev15,rev15wet0,comp,comp_heavy,eq_bright,eq_warm,prox,pops,rig,r16k,r48k,r96k
cargo run --release -p jk-pitch --example tracker_eval -- --set jump_cost=0 --clips   # an ablation, per clip
cargo run --release -p jk-pitch --example tracker_synth                                # whole notes, rough voices, legato
cargo run --release -p jk-pitch --example tracker_bench                                # sections 2-8 tables
JK_SET="flat_power=0" cargo run --release -p jk-pitch --example tracker_bench noise    # any setting, exploration
```

`../jukeoke-build/practice-tracker/runs/final/run-eval.sh` runs both trackers on both splits
and every condition (17 minutes on this PC); `report.py` there writes the tables of sections
13 and 15. The Stage 0 tracker for the comparison builds from
`../jukeoke-build/practice-tracker/devcrate-stage0/` (git HEAD a211234's `tracker.rs` with the
same evaluation examples).

## 19. Independent verification of the hardening (2026-11-19)

Re-run by an independent verifier from the repository code (`CARGO_TARGET_DIR=
$JK_BUILD/target-wo2-trackerverify`). Evidence, probe sources and analysis scripts:
`../jukeoke-build/practice-tracker/verify/` (`probe-src/`; tracker events only, no audio).

**Reproduced exactly**: `cargo test --release -p jk-pitch` (43 lib, 3 `echo_alloc`, 3
`tracker_alloc`, all pass); clippy reports nothing in the tracker files; the
`aarch64-apple-darwin` check passes; `tracker_eval --split dev` and `--split heldout`
(clean, r16k, r96k) give the section 13 and 15 numbers to the last digit; `tracker_synth`
output is identical; `tracker_bench` is identical except the timings, and `tracker_bench cpu`
on an idle machine gives 1.69 ms per second of audio at 48 kHz. No singer or artist group is
in both splits; the only held-out result files are those of the final run; the Stage 0
baseline is git HEAD's `tracker.rs` plus `pub mod rig;`.

**Found** (each with its file in `verify/`):

1. *Delay on real onsets* (`analysis-onsets.txt`): 15.7 % (dev) and 17.4 % (held-out) of real
   onsets are not shown on the note within 100 ms; on vocadito, whose onsets are hand
   annotated, 13.2 % and 10.8 % (the first pitched event of any pitch is later than 100 ms
   for 9.8 % and 5.2 %). The generated-voice maximum is 72 ms (Stage 0 62 ms). The marker
   delay budget in `docs/APP_API.md` (tracker 36 / 62 ms + up to 16 ms batch wait + up to
   17 ms frame + the trip to the web view) was written for Stage 0; with 46 / 72 ms its worst
   case is about 105-110 ms, over WO2's 100 ms ceiling. A look-ahead of one frame gives back
   10 ms (its cost is in section 13.4).
2. *Whispered speech* (`probe-noise.txt`, `probe-whisper.txt`): 40 x 15 s per rate at -30 to
   -6 dBFS gave 10 pitched events at 44.1 kHz and 7 at 96 kHz (0 at 16 and 48 kHz), each a
   50-70 ms run at 260-310 Hz with clarity 0.78-0.91, all from steady starts (0 with
   `clarity_steady=2`). Breath, fricatives and room noise: 0 at every rate. `clarity_steady`
   0.85 removes them; on dev it costs 1.8 points (VR 89.6 -> 87.8 %, RPA 82.9 -> 81.1 %,
   `dev-clarity_steady-0.85.txt`). Not changed here.
3. *Low voices* (`analysis-bands.txt`, `analysis-low.txt`): below 160 Hz, where there are no
   steady starts, 44-48 % of held-out sung frames on the stems are shown (ccMixter 44.5,
   MUSDB 44.3, songs 47.7 %), against 70-78 % above 160 Hz; vocadito 85-88 %. All held-out
   sets together below 160 Hz: 49-50 % (Stage 0 43-44 %). Of the missed low frames 29-55 % have clarity 0.8-0.93.
4. *Music only, nobody singing* (`music-only.txt`; MUSDB18-HQ accompaniment, bass + drums +
   other, the same 50 one-minute excerpts): a pitch is shown on 6.9 % of the frames (Stage 0
   4.9 %). The tracker is never told the expected note; keeping the marker off with music in
   the input is the echo stage's job, and the steady start gives it more to remove.
5. *Frames the reference leaves out* (`analysis-undec.txt`): where pYIN and CREPE disagree
   (11-26 % of the stem frames) the tracker voices 14-15 % (Stage 0 9-10 %); where CREPE is
   confident (periodicity > 0.5), 3.7 % (dev) and 4.3 % (held-out) of those are an octave
   off CREPE and not on pYIN (Stage 0 5.5 and 6.7 %). Counting them would add an estimated
   quarter (dev) to half (held-out) to the hardened tracker's octave-error frames (Stage 0:
   about +15 and +38 %); the before / after comparison still holds.
6. *The 16 kHz breathy rows of section 14 are the generator* (`probe-breath-rate.txt`): the
   same breathy rough voice generated at 48 kHz and resampled to 16 kHz gives 0 -> 3
   (1 % / 6 % / -20 dB) and 46 -> 48 (2 % / 10 % / -15 dB) held events more than 600 cents
   off, against 87 and 111 for the voice generated at 16 kHz: the tracker gives the same
   answer, the generator puts more breath in band. The breathy regression against Stage 0 is
   real at 48 kHz (1 % / 6 % / -15 dB: 152 of about 7,900 held events).

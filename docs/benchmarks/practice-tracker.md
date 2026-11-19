# Live pitch tracker measurements (Work Order 2, Stage 0, "live tracker")

Measured on 2026-11-19 on the build PC (Windows 11, Ryzen 7 9700X, 32 GB), Rust 1.100
`x86_64-pc-windows-gnu`, release profile of the workspace. Every number below comes from
`crates/jk-pitch/examples/tracker_bench.rs` or from the unit tests named in "How to
reproduce"; the full output is kept in `../jukeoke-build/practice-tracker/bench-final.txt`.
All input is **generated** (no microphone was opened, nobody sang): see "What this does not
show".

Code under test: `jk_pitch::tracker::Tracker` (`crates/jk-pitch/src/tracker.rs`), default
`TrackerConfig`:

| Setting | Value | Why |
|---|---|---|
| Method | McLeod Pitch Method (McLeod and Wyvill, ICMC 2005) written on `realfft` 3.5 | WO2 section 6; the `pitch-detection` crate plans an FFT per call and has an octave bug |
| Window | 42.67 ms, sized in time: 683 / 1882 / 2048 / 4096 samples at 16 / 44.1 / 48 / 96 kHz | a C2 period (15.3 ms) fits 1.8 times in the NSDF overlap (trap 5); same answer at every rate (trap 14) |
| FFT | real FFT of the zero-padded window, length the next power of two >= 2 x window (2048 / 4096 / 4096 / 8192) | linear autocorrelation, no wrap-around |
| Hop | 10 ms (160 / 441 / 480 / 960 samples) | a new estimate every 10 ms |
| Pitch range | 62 Hz to 1400 Hz | C2 (65.4 Hz) with margin, above 60 Hz mains hum; C6 (1046.5 Hz) plus vibrato |
| Peak picking | key maxima of the NSDF's positive regions; first one >= 0.9 x the highest; parabolic interpolation | the paper's relative peak picking |
| High-pass | 4th-order Butterworth at 55 Hz (pre-warped bilinear, f64) | takes handling thumps, pops, DC and 50 Hz hum away before analysis (WO2 4.10) |
| Singing / not singing | clarity >= 0.93 to start; >= 0.75 to keep going **only while the pitch continues the line** (<= 150 cents per 10 ms); window RMS >= -55 dBFS to start, >= -60 dBFS to keep | hysteresis; thresholds chosen from the measurements in section 6 |
| Minimum duration | 2 voiced analysis frames in a row, each continuing the previous one's pitch | a thump, pop or noise burst cannot become a note |
| Smoothing | three-point median of the voiced estimates (newest of two at the start of a run) | removes single-frame slips |
| Input | one mono channel chosen by the caller (`process`, or `process_interleaved` with a channel index); channels are never summed | trap 14, WO2 4.4 "choosing the channel" |
| Output | `PitchEvent { time, ready_at, hz: Option<f32>, clarity, level_dbfs }` every hop | `time` = centre of the analysed audio (what was sung when, for the scorer); `ready_at` = when the event became available (for the display); `ready_at - time` = half a window + one hop, constant |

## 1. Summary

| WO2 requirement (4.4 table, section 9 row "Tracker") | Result | Criterion |
|---|---|---|
| Range | **C2 to C6 tracked, every semitone**, three vowels, four rates: 100.0 % of the events in every held note pitched | E2..C6, aim C2 |
| Accuracy on steady tones | **max 2.38 cents** (16 kHz, C5-C6); max 0.10 cents at 44.1 / 48 / 96 kHz; median <= 0.18 cents everywhere | within 10 cents |
| Octave jumps on a steady note | **0** of 26,460 events (49 notes x 3 vowels x 4 rates) in the **held part** (0.1 s after the onset to 0.05 s before the release). **Not met over the whole note**: the independent check (section 12) found 10 to 14 octave-low events per rate at onsets of high "oo" notes, octave-low events inside held notes once mild breath noise or realistic jitter is added, and octave-high segments on 2 of 4 real lead vocals | none |
| Vibrato +-50 cents at 5 / 5.5 / 6 Hz | median 2.8 to 4.1 cents, p95 <= 8.9, max 12.2 cents from the true instantaneous pitch; 0 octave errors; no gaps | (report) |
| Slides 1200 to 4800 cents/s | median <= 1.8 cents; p95 4.0 (1200 c/s) to 28.2 cents (4800 c/s); 0 gross errors; no gaps | (report) |
| Silence, breath, speech-like noise, consonants, room noise | **0 pitched events** at every rate (30 s of breath and of speech-like noise per rate); no single analysis frame passed the start threshold in these runs (in 120 s runs one breath frame reached 0.931 and the two-frame rule held it back: section 12) | no pitch, no wandering |
| Handling thumps and popped plosives (handheld dynamic mic) | **0 pitched events** (14 thumps 40..200 Hz up to -1 dBFS peak; 9 pops up to -1 dBFS peak), every rate | never a note |
| Delay, onset to first correct event | **median 36 ms, max 62 ms** over every semitone C2..C6 x 10 onset positions, every rate; C4 and up: max 49 ms | aim 50 ms, never > 100 ms |
| ... of which window / hop / median | window fill median 19 ms (C2-B2: 44 ms, the whole window; C5-C6: 9 ms); hop wait 0..10 ms; minimum run / median 10 ms | (report) |
| Marker off after singing stops | median 47 ms, max 60 ms | marker disappears |
| Same answer across sample rates | vs 48 kHz on a 40 s melody: p95 <= 0.63 cents, max 6.1 cents (16 kHz, on transitions); voicing differs on <= 3 of 4000 events | same answer |
| Processor cost | **1.0 ms per second of audio at 48 kHz** (0.10 % of one core, 10 us per analysis); 16 kHz 0.42 ms, 44.1 kHz 0.95 ms, 96 kHz 2.2 ms; 1.6 to 1.8 ms at 48 kHz while the PC was at 100 % load from other work | (report) |
| MacBook Air M1 estimate | 2 to 3 ms per second of audio at 48 kHz (0.2 to 0.3 % of one core), 20 to 30 us per analysis | (estimate, not measured) |
| No allocation per block | **0 allocations** while processing 20 s at each rate and an interleaved stream (counting global allocator) | none after construction |
| Same code on the Mac | `cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin` passes (type-checked only; not run on a Mac) | WO2 5 "same on both systems" |

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

## 10. Unit tests

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

## 11. How to reproduce

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

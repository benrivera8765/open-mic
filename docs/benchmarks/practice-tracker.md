# Live pitch tracker measurements (Work Order 2: Stage 0, hardening on real voices, robustness, quality round)

Stage 0 measured on 2026-11-19 on the build PC (Windows 11, Ryzen 7 9700X, 32 GB), Rust 1.100
`x86_64-pc-windows-gnu`, release profile. The tracker was then **hardened on real voices**
the same day (sections 13 to 19, "Stage 1"): it is evaluated on about 108 minutes of real
singing with a singer-disjoint dev / held-out split. The **robustness work** that followed
(section 20) made it cope better with reverb, a bright EQ, slow real onsets, whispers and
breathy voices. The **WO2 quality round** (section 22, 2026-11-20) then worked on the
verifier findings of section 21: low voices, reverb, onset delay, whispers with fresh seeds
and the tail rule on dry fades. **Section 1 and section 22 describe the current tracker;
sections 20-21 are the robustness record, 13 to 19 the Stage 1 record and 2 to 12 the
Stage 0 record**, unchanged.

Evidence: Stage 0 in `../jukeoke-build/practice-tracker/bench-final.txt`; Stage 1 in
`../jukeoke-build/practice-tracker/runs/final/`; the robustness work in
`../jukeoke-build/practice-tracker/s4/` (`runs/`: real-voice tables, onset-delay
distributions, generated-voice probes, bench, music-only run; `log*.txt`: settings tried on
dev); the quality round in `../jukeoke-build/practice-tracker/q/` (`final/`: every table of
section 22); the evaluation set in `../jukeoke-build/practice-tracker/eval/` (nothing of it in the
repository). No microphone was opened and nobody sang for this report: real voices are
recordings.

Code under test: `jk_pitch::tracker::Tracker` (`crates/jk-pitch/src/tracker.rs`), default
`TrackerConfig`:

| Setting | Value (Stage 1 value if different) | Why |
|---|---|---|
| Method | McLeod Pitch Method (McLeod and Wyvill, ICMC 2005) on `realfft` 3.5; octave decided over time (below) | WO2 section 6; the `pitch-detection` crate plans an FFT per call and has an octave bug |
| Window | 42.67 ms, sized in time: 683 / 1882 / 2048 / 4096 samples at 16 / 44.1 / 48 / 96 kHz | a C2 period (15.3 ms) fits 1.8 times in the NSDF overlap (trap 5); same answer at every rate (trap 14) |
| FFT | real FFT of the zero-padded window, length the next power of two >= 2 x window | linear autocorrelation, no wrap-around |
| Hop | 10 ms | a new estimate every 10 ms |
| Pitch range | 62 Hz to 1400 Hz | C2 with margin, above 60 Hz hum; C6 plus vibrato |
| Filters | 4th-order Butterworth high-pass at 55 Hz; 4th-order Butterworth low-pass at **3.5 kHz** (4 kHz) | thumps, pops, DC, hum out; a bright EQ, sibilance and air do not lower a vowel's clarity (sections 13.4, 20.2) |
| Candidates | every NSDF key maximum >= 0.5 x the highest, at most 6 | the octave is chosen among them |
| Octave scoring | periodicity of the flattened spectrum (autocorrelation of \|X\|^0.5, 0-5 kHz) at each candidate, less 0.06 per octave below MPM's choice; **0.3 less for a candidate with another at 2 or 3 times its pitch whose own harmonics are not peaks (power at them under 2 x the power halfway between)** (no harmonic check) | weak odd harmonics, a second voice, a reverb tail (13.4); breath through a narrow formant (20.2) |
| Late-reverberation suppression | **each candidate's clarity is the higher of the plain NSDF and the NSDF of the spectrum less a late-reverberation estimate (the same bin 50 ms earlier, decayed as RT60 1.5 s, x 2, floor 0.1); above 130 Hz and for candidates with flattened periodicity >= 0.13 only** (none) | a new note is seen through the tail of the one before (20.2) |
| Path | Viterbi over the last frames, decided **1 frame (10 ms)** behind the newest (2 before the quality round); 0.2 per octave down, 0.1 per octave up; pinned to what was shown; a note starts on the newest window's octave; **a subharmonic whose harmonic comb is 6 dB weaker than that of the candidate at 2 or 3 times its pitch loses 0.2** | no octave flips inside a note, no onset flashes, leaps still followed; real onsets 10 ms sooner (section 22) |
| Low-band clarity (quality round) | **each candidate up to 300 Hz with a harmonic series may take the NSDF of the spectrum below 1 kHz as its clarity** | a low voice whose high harmonics lose their alignment (jitter, vibrato, a room) is still seen as periodic |
| Singing / not singing | start: clarity >= 0.93 then a continuing frame >= 0.75; or five frames >= 0.8 within 40 cents, at least 160 Hz, **each with a harmonic series (flattened periodicity >= 0.09)**; **or early: two adjacent newer frames at 0.93 / 0.75 and the decided frame >= 0.75, at least 160 Hz, the newest level risen <= 1 dB, no candidate an octave up at NSDF >= 0.4**; keep: >= 0.75 while the pitch continues (<= 150 cents per 10 ms); level >= -55 dBFS to start, >= -60 to keep; **a decaying tail (15 dB under the recent peak, falling >= 0.3 dB in each of the last 4 frames) ends a note and starts none** (robustness work; the quality round changed this rule, see "Starts and keeps added by the quality round" below) | real voices with clarity 0.8-0.93 are shown; breath, whispers and thumps are not; clear real onsets show up to 20 ms sooner; a room tail stops being a note (20.2) |
| Smoothing | three-point median of the previous reported frame, the decided frame and the next one on the path | removes single-frame slips |
| Input | one mono channel chosen by the caller; channels are never summed | trap 14, WO2 4.4 |
| Starts and keeps added by the quality round | **evidence start**: a pitch line followed frame to frame whose summed clarity above 0.72 reaches 0.6 (0.35 and 3 frames for a line below 200 Hz); every start needs a **harmonic comb** (peaks over valleys of the first harmonics up to 3 kHz, 6 dB scaled down below 200 Hz) unless its plain clarity is >= 0.97; below 160 Hz a non-confident start needs a 60 ms line; keep at 0.7 while the input sounds reverberant; the **tail rule** (10 dB under the peak, falling over 80 ms) applies only while the input sounds reverberant, judged from the average fall of the level at the recent phrase endings | low, processed and reverberant voices are shown; whispers, breath, thumps are not; a dry fade after phrases that end abruptly is not cut (section 22), but after phrases that end in fades the input is judged reverberant and fades are cut more than before (section 23) |
| Output | `PitchEvent { time, ready_at, hz: Option<f32>, clarity, level_dbfs }` every hop | `time` = centre of the decided window; `ready_at - time` = half a window + 1 hop = **31.3 ms** (41.3 ms before the quality round) |

## 1. Summary (current tracker)

Numbers of the WO2 quality round (section 22); in brackets the Stage 3a tracker (git HEAD
before the round) on the same material. "Dev" = development set (tuned on), "held-out" =
never tuned on (run once at the end); both have two 60 s windows per long stem (section 22.2).

| WO2 requirement (4.4 table, section 9 row "Tracker", 4.10) | Result | Criterion |
|---|---|---|
| Range | C2 to C6 tracked on generated voices, every semitone, three vowels, four rates, 100 % of held events pitched (same). Real voices shifted 7-12 semitones down by the engine's own Rubber Band settings, frames shown in C2-D2 / D2-E2 / E2-A2: dev 69 / 75 / 76 % (65 / 71 / 71), held-out 50 / 54 / 62 % (43 / 47 / 55); on single-voice material (ccMixter, vocadito; dev) 75-88 / 69-90 / 86-91 %. Real sung frames C2-160 Hz shown: dev 78.1 % (74.1), held-out 60.8 % (53.6) | E2..C6, aim C2: **E2 met on single clean voices; low voices on processed stems still shown less than higher ones** |
| Accuracy on steady tones | max 2.26 cents at 16 kHz, <= 0.10 cents at 44.1 / 48 / 96 kHz (same); vibrato max 12.2 cents | within 10 cents |
| Octave jumps on a steady note, generated | 0 in every held part; whole notes including onsets: 9 / 10 / 8 / 8 events more than 50 cents off at 16 / 44.1 / 48 / 96 kHz in 147 notes, all at onsets (6 / 6 / 6 / 12); breathy voice (-12 dB breath) 306 / 118 / 86 / 28 (379 / 205 / 173 / 138) | none |
| Octave errors, real voices | 0.38 % of frames dev, 0.41 % held-out (0.44 / 0.47 %); octave jumps on a held pitch 75 dev, 56 held-out (116 / 66) | none |
| Real sung frames shown (voicing recall) / right note when shown | dev 90.7 % / 99.14 % (89.9 / 99.10), held-out 84.3 % / 98.73 % (82.4 / 98.74); false alarms 0.80 / 0.96 % (0.61 / 0.62 %) | (report) |
| Not singing: silence, breath, speech-like noise, consonants, room noise, thumps, pops | 0 pitched events in the 30 s probes and in the 10-minute probes per kind and rate (tuning seeds); **fresh seeds** (held-out, 30 min per kind and rate): 0 everywhere except one 20 ms run (2 events, 743 Hz) of whispered speech at 48 kHz (Stage 3a: 5 events at 16 kHz). The independent verifier's own seeds and noise kinds (section 23): whispered speech 2 runs (40 ms at 16 kHz, 20 ms at 48 kHz) in 2 hours (Stage 3a 5 runs); a narrow-band low rumble (70-230 Hz) pitched on 1.6-3.9 % of frames (Stage 3a 3.7-7.7 %) | no pitch, no wandering: **not quite 0 on fresh whisper seeds; not met for tonal low rumble** |
| Delay, onset to first correct event, generated | median 35 ms, max 62 ms (46 / 72); marker off after singing stops median 57, max 70 ms (66 / 76); the pipeline's own delay 31.3 ms (41.3) | aim 50 ms, never > 100 ms |
| Delay on real voices (onset = the reference's first voiced 10 ms frame) | onsets shown on the note later than 100 ms: dev 6.0 % (14.2), held-out 6.2 % (15.4); under reverb RT60 0.8 s 14.2 / 15.2 % (25.1 / 24.7); low voices shifted down 14.3 / 15.7 % (23.9 / 26.2); p50 / p90 60 / 90 ms | aim 50 ms, never > 100 ms: **halved, still not met for every real onset** |
| Same answer across sample rates | generated melody p95 <= 0.67 cents; max 29.8 cents at 16 kHz (5.8), 9.7 at 96 kHz; real voices resampled to 16 / 48 / 96 kHz: RPA within 0.3 points of the original | same answer |
| The owner's rig (RPA change against clean input, dev / held-out) | compression, warm EQ, proximity, pops and thumps: -0.1 to +2.8 points; bright EQ -3.1 / -4.3 (-4.0 / -5.8); **reverb RT60 0.3 / 0.8 / 1.5 s at -6 dB: -6.0 / -9.9 / -13.9 dev, -8.7 / -13.0 / -16.9 held-out** (-4.8 / -9.7 / -14.7 and -8.2 / -13.1 / -17.6); absolute RPA under those reverbs up 0 to 2.8 points, false alarms 0.98 / 2.93 / 7.24 % dev (1.28 / 5.28 / 10.53), right note when shown unchanged within 0.3 points | within a few points: **met for compression, EQ, proximity, pops, sample rate; not met for reverb** |
| Music alone (MUSDB accompaniment, nobody singing) | a pitch on 12.3 % of frames (7.0): the tracker is never told the expected note; keeping the marker off with music in the input is the echo stage's gate | (report) |
| Processor cost | 2.70 ms per second of audio at 48 kHz, 0.27 % of one core (2.13); 7.4 ms at 96 kHz | (report) |
| MacBook Air M1 estimate | 5.4-8.1 ms per second of audio at 48 kHz (0.5-0.8 % of one core) | (estimate) |
| No allocation per block | 0 allocations (`tests/tracker_alloc.rs`) | none |
| Same code on the Mac | `cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin` passes (type-checked only) | WO2 5 |

> **Sections 2 to 12 below are the Stage 0 tracker's measurements and sections 13 to 19 the
> Stage 1 tracker's**, kept as the record the later work is compared with. The bench re-run
> on the current tracker is `../jukeoke-build/practice-tracker/s4/runs/bench-final.txt`; its
> headline numbers are in section 1 and section 20.

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

Both trackers on the same clips (Stage 0 = the tracker of git HEAD cb5d359, built from a
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

> The open items below are the Stage 1 state; section 20 is the work done on them and
> what is still open now.

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
`../jukeoke-build/practice-tracker/devcrate-stage0/` (git HEAD cb5d359's `tracker.rs` with the
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

## 20. Robustness: reverb, EQ, real onsets, whispers, breath (WO2 4.4 delay, 4.10)

The Stage 1 verifier (section 19, DECISIONS D-244) left these open: 15-17 % of real onsets
shown after more than 100 ms; reverb (RT60 0.3 / 0.8 / 1.5 s) costing 5 / 12 / 17 points of
raw pitch accuracy (RPA) with false alarms up to 17 %, a bright mixer EQ 5-7 points;
whispered speech briefly passing the steady start at 44.1 / 96 kHz on long runs; low voices
on stems shown about half the time; music alone voiced more often than by Stage 0; breathy
generated voices worse than Stage 0. This section is the work on them, measured 2026-11-19
on the same PC, with the same evaluation set and rig effects (sections 13.1, 15).

**Protocol.** Every setting was chosen on the **dev** split (the logs `s4/log*.txt` hold every
setting tried) and on the generated voices; nothing was tuned on held-out. The held-out split
was run twice, and the second run is the one reported: the first run
(`s4/runs/v1-final-heldout.txt`) was of the tracker without the harmonic check of change 3
below. That check was then added because of the generated breathy voices, chosen on dev and
on generated voices only, and the final tracker was run on held-out once more. Over every
condition the two held-out runs differ by at most 0.15 points of RPA, 0.05 of octave errors
and 7 octave jumps (clean: RPA 72.5 both, octave errors 0.41 -> 0.38 %). The "Stage 1"
columns are the current code with the new switches off (`s4/stage1-set.txt`:
`lowpass_hz=4000 early_start=0 tail_drop_db=0 tail_frames=2 derev_rt60_s=0
steady_harmonic_min=0 odd_cost=0`), which reproduces the Stage 1 tables of section 13 to the
digit on dev and within 0.01 points on held-out.

### 20.1 What changed

1. **Late-reverberation suppression for clarity ("dereverberation lite").** Each frame's power
   spectrum below 5 kHz loses an estimate of its late reverberation: the power the same bin
   had 50 ms earlier, decayed as a room with RT60 1.5 s would decay it, times 2, keeping at
   least 10 % (spectral subtraction after Lebart et al. 2001). A second NSDF is computed from
   the result (one more inverse FFT), and each candidate's clarity becomes the higher of the
   plain and the suppressed NSDF at its period. The candidates, their pitch and their octave
   scores still come from the plain spectrum: feeding the suppressed spectrum to everything
   gave far fewer false alarms under reverb (RT60 1.5 s: 18.0 -> 12.6 %) but cost 6 points of
   RPA on clean input, with 1.7 times the octave jumps (musical noise in the pitch). Below
   130 Hz (bass lines in music) and for candidates whose flattened-spectrum periodicity is
   under 0.13 (noise, whispers) the plain clarity is kept. The parameters matter little (RT60
   0.5-1.5 s, over-subtraction 1.5-3, floor 0.03-0.1, delay 30-50 ms: RPA 85.5-86.6 % clean
   and 74.7-76.1 % under RT60 0.8 s on dev, `log-derev.txt`).
2. **Analysis low-pass at 3.5 kHz** instead of 4 kHz: bright EQ on dev 77.6 -> 78.5 % RPA
   alone (Stage 1 settings otherwise), clean 82.9 -> 83.3 %.
3. **Harmonic evidence for subharmonics.** A candidate with another candidate at two or three
   times its pitch must show peaks at its own harmonics that the higher one does not have (its
   odd harmonics for an octave): the power at them, up to 3 kHz, at least twice the power
   halfway between them, or its score loses 0.3. Breath through a narrow first formant
   (270-300 Hz) fills the band around a high "ee" / "oo" note's subharmonic with noise but puts
   no peaks at its odd harmonics; a real voice whose odd harmonics are 15 dB down still has
   them as peaks over the valleys. Generated breathy voice (-12 dB breath, whole notes) at
   48 kHz: 289 -> 173 events off; real voices: octave errors 0.45 -> 0.44 % dev. A stricter
   ratio (3 or 4) helped the breathy voice more (122-131 events) but raised octave jumps
   under reverb (RT60 0.8 s: 33 -> 47-49), the reverb tail filling the valleys of a real low
   note.
4. **Steady starts need a harmonic series**: every frame of a steady start must have a
   flattened-spectrum periodicity of at least 0.09. A whispered vowel is nearly periodic over
   one window in the NSDF (noise through a narrow formant) but has no harmonic series; real
   sung frames are under 0.09 in 5 % of cases inside runs and 24 % at the first frame of a run
   (`s3/perdiag.py`), which the other start rules cover.
5. **Early start** on clear onsets: two adjacent newer frames of the look-ahead at 0.93 and
   0.75 on one line and the decided frame (and any frame between) at 0.75 start the note, up
   to two hops before the plain confident start would. Three guards keep its octave right and
   thumps out: not below 160 Hz (the generated 25 ms burst at 155 Hz became a note without
   it, `short_low_bursts_are_not_notes`); the newest window's level must have risen by at most
   1 dB over the previous one (the attack is over); and no frame used may hold a candidate an
   octave above its own at NSDF >= 0.4. Without the last two, generated whole notes had 42 /
   32 / 32 / 38 events more than 50 cents off at the onsets (Stage 1: 3 / 6 / 6 / 9): the first
   30 ms of a generated high "ee" / "oo" note look an octave low (the first formant ringing),
   and the early start took that octave. With them: 6 / 6 / 6 / 12.
6. **Reverb tail rule**: a frame whose level is 15 dB or more under the recent peak (released
   by 0.1 dB per frame) after the level fell by at least 0.3 dB between each of the last four
   frames is a decaying tail: it ends a shown note and starts none. After a tail a note starts
   again on a new onset (3 dB above the lowest level of the tail), when the level stops
   falling over 40 ms, or on a clear pitch away from the last note (without the last two, a
   legato step down to a quieter note was never shown, `legato` in `tracker_synth`). On dev it
   is a trade, not a free gain (`log-tail.txt`, `log-allcond.txt`): false alarms under RT60
   0.8 / 1.5 s 11.5 -> 8.2 % and 18.0 -> 15.1 %, on clean input 1.58 -> 1.08 % (mostly
   vocadito, whose annotation ends with the note while the vowel still rings), for 1.2
   points of RPA on clean input. The frames it loses on clean input lie in the last 100 ms
   of the reference's voiced runs on the processed stems (`s4/lostdiag.py`: 56 % of them in
   the last 100 ms, 6 % on vocadito), where the stems carry their own reverb and the
   reference calls its tail voiced. Gentler settings (20-25 dB) removed fewer false alarms
   for less; stricter ones (6-10 dB) cost 2-6 points of voicing recall.

### 20.2 Results on real voices, every rig condition

**dev, all sets** (Stage 1 -> final; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets shown <= 100 ms % |
|---|---|---|---|---|---|---|---|---|
| clean | 82.9 -> **85.0** |  | 89.6 -> 91.2 | 1.36 -> 1.08 | 99.04 -> 99.08 | 0.47 -> 0.44 | 63 -> 70 | 84.3 -> 87.2 |
| reverb RT60 0.3 s, -6 dB | 77.5 -> **80.6** | -5.4 -> **-4.4** | 85.5 -> 88.5 | 4.24 -> 2.35 | 97.00 -> 97.16 | 0.65 -> 0.62 | 50 -> 58 | 72.0 -> 77.0 |
| reverb RT60 0.8 s, -6 dB | 71.4 -> **75.3** | -11.5 -> **-9.7** | 80.2 -> 84.1 | 11.11 -> 8.21 | 95.00 -> 95.29 | 0.62 -> 0.53 | 39 -> 33 | 70.2 -> 76.0 |
| reverb RT60 1.5 s, -6 dB | 65.9 -> **70.1** | -16.9 -> **-14.8** | 75.2 -> 79.4 | 17.38 -> 15.06 | 93.80 -> 94.22 | 0.62 -> 0.49 | 25 -> 29 | 64.6 -> 73.1 |
| reverb RT60 1.5 s, 0 dB | 54.0 -> **56.7** | -28.9 -> **-28.3** | 66.3 -> 69.1 | 21.22 -> 18.98 | 86.25 -> 86.89 | 0.77 -> 0.62 | 28 -> 35 | 44.9 -> 52.3 |
| compressor | 82.5 -> **85.4** | -0.4 -> **+0.4** | 89.3 -> 91.6 | 1.29 -> 1.14 | 99.03 -> 99.04 | 0.46 -> 0.43 | 66 -> 68 | 83.4 -> 87.3 |
| heavy compressor | 82.3 -> **85.6** | -0.6 -> **+0.6** | 89.1 -> 91.8 | 1.30 -> 1.26 | 99.02 -> 99.00 | 0.44 -> 0.42 | 68 -> 66 | 82.6 -> 87.0 |
| bright EQ | 77.6 -> **81.2** | -5.3 -> **-3.8** | 84.8 -> 88.1 | 1.10 -> 0.89 | 99.08 -> 99.05 | 0.45 -> 0.48 | 49 -> 52 | 76.8 -> 83.4 |
| warm EQ | 84.2 -> **85.9** | +1.3 -> **+1.0** | 90.7 -> 92.0 | 1.72 -> 1.48 | 98.77 -> 98.82 | 0.60 -> 0.54 | 67 -> 76 | 86.1 -> 89.0 |
| proximity bass | 84.0 -> **86.1** | +1.2 -> **+1.1** | 90.5 -> 92.0 | 1.95 -> 1.72 | 98.61 -> 98.62 | 0.47 -> 0.45 | 49 -> 51 | 86.1 -> 89.3 |
| proximity + pops + thumps | 82.5 -> **84.6** | -0.4 -> **-0.4** | 88.9 -> 90.5 | 1.83 -> 1.59 | 98.60 -> 98.60 | 0.47 -> 0.48 | 50 -> 52 | 83.4 -> 87.5 |
| the whole rig | 68.2 -> **74.0** | -14.7 -> **-11.0** | 75.9 -> 81.7 | 5.40 -> 5.32 | 96.90 -> 97.06 | 0.50 -> 0.44 | 24 -> 25 | 57.1 -> 67.8 |
| resampled 16 kHz | 82.8 -> **84.7** | -0.1 -> **-0.3** | 89.5 -> 91.0 | 1.35 -> 1.08 | 98.99 -> 99.04 | 0.52 -> 0.48 | 76 -> 72 | 84.3 -> 87.2 |
| resampled 48 kHz | 82.9 -> **85.0** | -0.0 -> **+0.0** | 89.6 -> 91.2 | 1.36 -> 1.08 | 99.03 -> 99.10 | 0.48 -> 0.43 | 66 -> 72 | 84.3 -> 87.2 |
| resampled 96 kHz | 82.9 -> **85.0** | -0.0 -> **+0.0** | 89.6 -> 91.2 | 1.36 -> 1.08 | 99.03 -> 99.09 | 0.47 -> 0.43 | 66 -> 72 | 84.3 -> 87.2 |

**heldout, all sets** (Stage 1 -> final; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets shown <= 100 ms % |
|---|---|---|---|---|---|---|---|---|
| clean | 68.3 -> **72.5** |  | 79.2 -> 82.9 | 1.65 -> 0.94 | 98.82 -> 98.87 | 0.42 -> 0.38 | 37 -> 36 | 82.6 -> 86.2 |
| reverb RT60 0.3 s, -6 dB | 59.6 -> **64.5** | -8.7 -> **-8.0** | 71.5 -> 76.7 | 3.68 -> 1.90 | 95.94 -> 96.31 | 0.99 -> 0.84 | 49 -> 56 | 69.9 -> 75.3 |
| reverb RT60 0.8 s, -6 dB | 53.8 -> **59.5** | -14.5 -> **-13.0** | 65.9 -> 72.0 | 7.89 -> 6.17 | 94.40 -> 95.06 | 0.74 -> 0.53 | 28 -> 22 | 70.8 -> 76.7 |
| reverb RT60 1.5 s, -6 dB | 49.2 -> **54.9** | -19.2 -> **-17.5** | 61.2 -> 67.4 | 9.75 -> 8.90 | 93.49 -> 94.09 | 0.57 -> 0.46 | 15 -> 18 | 68.3 -> 75.6 |
| reverb RT60 1.5 s, 0 dB | 38.3 -> **41.6** | -30.1 -> **-30.9** | 51.6 -> 55.4 | 11.63 -> 10.79 | 85.44 -> 86.50 | 0.85 -> 0.56 | 13 -> 13 | 50.5 -> 58.1 |
| compressor | 67.8 -> **72.9** | -0.6 -> **+0.4** | 78.7 -> 83.3 | 1.59 -> 1.13 | 98.76 -> 98.81 | 0.44 -> 0.38 | 38 -> 36 | 82.1 -> 86.4 |
| heavy compressor | 67.4 -> **73.0** | -0.9 -> **+0.5** | 78.4 -> 83.5 | 1.57 -> 1.27 | 98.72 -> 98.75 | 0.43 -> 0.35 | 40 -> 35 | 80.3 -> 86.9 |
| bright EQ | 61.0 -> **67.0** | -7.3 -> **-5.4** | 72.0 -> 78.1 | 1.31 -> 0.76 | 98.92 -> 98.97 | 0.39 -> 0.34 | 28 -> 24 | 74.0 -> 81.0 |
| warm EQ | 71.9 -> **75.4** | +3.6 -> **+2.9** | 82.3 -> 85.3 | 1.89 -> 1.14 | 98.40 -> 98.48 | 0.41 -> 0.34 | 30 -> 38 | 85.5 -> 88.9 |
| proximity bass | 72.9 -> **76.6** | +4.5 -> **+4.1** | 83.0 -> 86.3 | 2.17 -> 1.62 | 97.70 -> 97.79 | 0.35 -> 0.29 | 21 -> 21 | 86.2 -> 89.6 |
| proximity + pops + thumps | 71.7 -> **75.5** | +3.4 -> **+3.0** | 81.8 -> 85.1 | 2.06 -> 1.49 | 97.71 -> 97.81 | 0.35 -> 0.29 | 21 -> 21 | 85.3 -> 89.6 |
| the whole rig | 51.9 -> **59.6** | -16.4 -> **-12.9** | 62.8 -> 71.2 | 3.74 -> 3.97 | 96.03 -> 96.36 | 0.36 -> 0.31 | 8 -> 11 | 59.5 -> 74.9 |
| resampled 16 kHz | 68.2 -> **72.2** | -0.1 -> **-0.3** | 79.0 -> 82.6 | 1.72 -> 0.92 | 98.76 -> 98.88 | 0.46 -> 0.36 | 44 -> 39 | 81.9 -> 85.5 |
| resampled 48 kHz | 68.3 -> **72.5** | +0.0 -> **+0.1** | 79.2 -> 83.0 | 1.65 -> 0.93 | 98.83 -> 98.89 | 0.39 -> 0.35 | 32 -> 33 | 82.6 -> 85.8 |
| resampled 96 kHz | 68.3 -> **72.6** | +0.0 -> **+0.1** | 79.2 -> 83.0 | 1.65 -> 0.93 | 98.83 -> 98.91 | 0.39 -> 0.34 | 33 -> 33 | 82.6 -> 85.8 |

**dev, clean, per set** (Stage 1 -> final)

| set | VR % | VFA % | RPA % | Acc % | Oct % (high) | flips | delay p50 / p90 ms | shown <= 100 ms % | onset flashes |
|---|---|---|---|---|---|---|---|---|---|
| ccmixter | 95.7 -> **95.9** | 0.08 -> 0.15 | 90.9 -> **91.2** | 99.33 -> 99.39 | 0.25 (0.04) -> 0.22 (0.04) | 28 -> 27 | 80 / 110 -> 80 / 100 | 84.6 -> **88.0** | 0 -> 1 |
| musdb | 79.9 -> **83.6** | 0.43 -> 0.68 | 71.1 -> **75.4** | 98.61 -> 98.70 | 0.91 (0.71) -> 0.83 (0.65) | 31 -> 38 | 90 / 300 -> 80 / 170 | 64.0 -> **72.0** | 0 -> 0 |
| vocadito | 95.3 -> **96.1** | 5.02 -> 3.40 | 91.0 -> **92.2** | 99.09 -> 99.07 | 0.21 (0.17) -> 0.23 (0.18) | 4 -> 5 | 70 / 110 -> 60 / 110 | 86.8 -> **88.9** | 4 -> 0 |
| all | 89.6 -> **91.2** | 1.36 -> 1.08 | 82.9 -> **85.0** | 99.04 -> 99.08 | 0.47 (0.29) -> 0.44 (0.28) | 63 -> 70 | 70 / 120 -> 70 / 110 | 84.3 -> **87.2** | 4 -> 1 |

**heldout, clean, per set** (Stage 1 -> final)

| set | VR % | VFA % | RPA % | Acc % | Oct % (high) | flips | delay p50 / p90 ms | shown <= 100 ms % | onset flashes |
|---|---|---|---|---|---|---|---|---|---|
| ccmixter | 74.6 -> **80.1** | 0.16 -> 0.22 | 59.4 -> **64.9** | 98.34 -> 98.44 | 0.29 (0.23) -> 0.20 (0.15) | 7 -> 10 | 80 / 120 -> 80 / 110 | 72.1 -> **77.0** | 0 -> 0 |
| musdb | 78.4 -> **81.6** | 0.15 -> 0.30 | 70.5 -> **74.1** | 99.06 -> 99.12 | 0.61 (0.21) -> 0.58 (0.25) | 22 -> 19 | 80 / 230 -> 80 / 150 | 70.6 -> **81.2** | 0 -> 0 |
| songs | 62.8 -> **70.1** | 0.13 -> 0.00 | 48.9 -> **55.4** | 98.07 -> 98.12 | 0.08 (0.08) -> 0.22 (0.22) | 1 -> 1 | 110 / 500 -> 110 / 210 | 50.0 -> **50.0** | 0 -> 0 |
| vocadito | 96.4 -> **97.4** | 10.05 -> 4.75 | 92.7 -> **94.0** | 99.09 -> 99.10 | 0.17 (0.10) -> 0.17 (0.11) | 7 -> 6 | 70 / 100 -> 60 / 100 | 89.2 -> **90.6** | 12 -> 11 |
| all | 79.2 -> **82.9** | 1.65 -> 0.94 | 68.3 -> **72.5** | 98.82 -> 98.87 | 0.42 (0.19) -> 0.38 (0.19) | 37 -> 36 | 80 / 120 -> 70 / 110 | 82.6 -> **86.2** | 12 -> 11 |

**Onset delay, dev, all sets** (share of the reference onsets by the delay to the first event on the note, %)

| tracker | condition | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | > 100 ms | p50 / p75 / p90 ms |
|---|---|---|---|---|---|---|---|---|---|---|
| Stage 1 | clean | 553 | 9.0 | 42.3 | 32.9 | 9.9 | 0.7 | 5.1 | 15.7 | 70 / 90 / 120 |
| Stage 1 | reverb RT60 0.8 s, -6 dB | 553 | 10.3 | 26.6 | 33.3 | 14.1 | 3.8 | 11.9 | 29.8 | 80 / 110 / 470 |
| Stage 1 | bright EQ | 553 | 6.9 | 37.8 | 32.2 | 13.6 | 2.2 | 7.4 | 23.2 | 80 / 100 / 140 |
| **final** | clean | 553 | 15.2 | 42.1 | 29.8 | 7.6 | 0.9 | 4.3 | 12.8 | 70 / 90 / 110 |
| **final** | reverb RT60 0.8 s, -6 dB | 553 | 15.6 | 36.0 | 24.4 | 12.1 | 3.1 | 8.9 | 24.1 | 70 / 100 / 190 |
| **final** | bright EQ | 553 | 13.0 | 40.0 | 30.4 | 9.4 | 1.6 | 5.6 | 16.6 | 70 / 90 / 120 |

**Onset delay, heldout, all sets** (share of the reference onsets by the delay to the first event on the note, %)

| tracker | condition | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | > 100 ms | p50 / p75 / p90 ms |
|---|---|---|---|---|---|---|---|---|---|---|
| Stage 1 | clean | 442 | 13.6 | 35.5 | 33.5 | 9.7 | 2.5 | 5.2 | 17.4 | 80 / 90 / 120 |
| Stage 1 | reverb RT60 0.8 s, -6 dB | 442 | 15.2 | 28.1 | 27.6 | 12.2 | 3.6 | 13.3 | 29.1 | 80 / 110 / 500 |
| Stage 1 | bright EQ | 442 | 10.4 | 31.7 | 31.9 | 14.5 | 2.5 | 9.0 | 26.0 | 80 / 100 / 210 |
| **final** | clean | 442 | 19.9 | 40.0 | 26.2 | 8.8 | 1.6 | 3.4 | 13.8 | 70 / 90 / 110 |
| **final** | reverb RT60 0.8 s, -6 dB | 442 | 18.8 | 35.7 | 22.2 | 11.1 | 3.2 | 9.0 | 23.3 | 70 / 100 / 200 |
| **final** | bright EQ | 442 | 17.0 | 36.2 | 27.8 | 10.9 | 1.6 | 6.6 | 19.1 | 70 / 90 / 120 |

What this says:

- **Every condition is ahead of Stage 1 in absolute RPA**, by 1.7 to 5.8 points on dev and 3.3
  to 7.7 on held-out: the gains carry over to held-out (clean +4.2 points, the whole rig +7.7).
- **Bright EQ**: the cost falls from 5.3 to 3.8 points on dev and from 7.3 to 5.4 on held-out.
- **Reverb**: the cost relative to clean input falls by 1-2 points (RT60 0.8 s: 11.5 -> 9.7
  dev, 14.5 -> 13.0 held-out), and false alarms by an eighth to a half (RT60 0.3 / 0.8 / 1.5 s:
  4.2 -> 2.4, 11.1 -> 8.2, 17.4 -> 15.1 % dev). **This is not "within a few points"**: at RT60 0.8 s with
  the wet part a quarter of the voice's energy, a new note is shown more than 100 ms after its
  onset for 23-24 % of onsets (12.8-13.8 % without reverb), mostly where the last note's
  tail is still as loud as the new note, and the marker lingers on the last note's pitch for
  part of the tail (the right note when shown stays at 94-97 %). The setup checklist should
  keep the mixer's reverb off (or low) on the feed to the computer (decision for the owner,
  section 17).
- **Octaves**: octave errors 0.47 -> 0.44 % dev and 0.42 -> 0.38 % held-out; octave jumps on a
  held pitch 63 -> 70 dev (more frames shown), 37 -> 36 held-out.

### 20.3 Onset delay against the 100 ms ceiling

The tables above give the distribution: the share of onsets shown later than 100 ms falls
from 15.7 to 12.8 % (dev) and from 17.4 to 13.8 % (held-out); with reverb RT60 0.8 s from
29.8 to 24.1 % and 29.1 to 23.3 %. Where the rest comes from:

- **The pipeline itself is 41.3 ms** (half the 42.7 ms window plus the two-hop look-ahead):
  the event describing a moment exists 41.3 ms after it. On generated notes (a clean 10 ms
  ramp onset) the first correct event comes 46 ms after the onset in the median and at most
  72 ms (section 5 method; the same as Stage 1: the early start never fires there, because a
  generated note's level keeps rising until the window is full). The rest of the time on real
  voices is evidence: the window must hold enough of the note to be clear.
- **The reference marks the first voiced 10 ms frame.** On the stems (pYIN + CREPE
  reference) almost no onset (0-2.4 %) is shown within 50 ms (the reference voices the breathy start of
  a note before it is periodic enough for a 43 ms window); on vocadito (hand-annotated) 21-30
  % are. Vocadito onsets later than 100 ms: 11.1 % dev, 9.3 % held-out (13.2 / 10.7 %).
- **Onsets shown on the note only after 250 ms, or never** (wrong note, or not voiced within
  500 ms): 4.3 % dev, 3.4 % held-out (5.1 / 5.2 %).
- A **one-frame look-ahead** for every event (pipeline 31.3 ms) was measured: onsets within
  100 ms 89.3 % on dev, but generated onset errors 14-16 per rate (limit in the unit test:
  12) and a mild-breath held part with 30 errors at 16 kHz; not taken. The guarded early start
  keeps generated onset errors close to Stage 1 (6 / 6 / 6 / 12 against 3 / 6 / 6 / 9).

**For the marker budget** (`docs/APP_API.md`): the tracker part is unchanged at 46 / 72 ms
(median / max, generated), so the Stage 1 verifier's note still stands: with the batch wait,
the frame and the trip to the web view, the worst case is about 105-110 ms. Real onsets add
evidence time on top, as above.

### 20.4 Generated voices, not singing, processor

Full outputs: `s4/runs/synth-final.txt` and `s4/runs/synth-stage1.txt` (same generators, the
Stage 1 settings), `s4/runs/bench-final.txt` (the Stage 0 bench on the final tracker).

| measure | Stage 1 | final |
|---|---|---|
| whole notes, every semitone C2..C6 x 3 vowels: events > 50 c off at 16 / 44.1 / 48 / 96 kHz | 3 / 6 / 6 / 9 | 6 / 6 / 6 / 12 (all at onsets, none held) |
| the same, mild breath (-20 dB) | 27 / 9 / 15 / 9 | 34 / 6 / 14 / 9 |
| the same, breathy (-12 dB) | 448 / 256 / 283 / 244 | **379 / 205 / 173 / 138** (Stage 0: 165 / 126 / 133 / 47) |
| rough voices, held events > 600 c off: 16 kHz 1 % / 6 % / -20 dB; 2 % / 10 %; 2 % / 10 % / -15 dB | 87; 18; 111 | **29; 6; 108** |
| rough voices, 48 kHz 2 % / 10 % / -15 dB | 46 | **29** (all other rough rows 0 in both) |
| legato steps (5 base notes x 10 intervals), change -> first event on the new note, max | 80 ms | 80 ms; events off both notes 1 / 9-10 / 10 (same) |
| 10-minute not-singing probes per kind and rate (whispered speech, breath, fricatives, room noise; 40 x 15 s, -60..-6 dBFS) | whispered speech 10 events at 44.1 kHz, 7 at 96 kHz | **0 at every rate** |
| 30 s probes per kind (silence, breath, speech-like noise, fricatives, room noise with hum, thumps, pops) | 0 | 0 |
| shortest periodic burst shown, 65-150 Hz | 35-45 ms | 35-45 ms |
| steady notes held part, max error / octave errors / voiced | 2.36 c / 0 / 100 % | 2.26 c / 0 / 100 % |
| same answer vs 48 kHz (40 s melody), voicing differs / p95 / max | 3 of 4000 / 0.68 c / 28.8 c | 2 of 4000 / 0.66 c / 5.8 c (16 kHz) |
| delay onset -> first correct event, median / max; marker off after the note, median / max | 46 / 72 ms; 67 / 80 ms | 46 / 72 ms; 66 / 76 ms |
| music only (MUSDB accompaniment, nobody singing, 50 min): frames with a pitch | 6.89 % (Stage 0 4.9 %) | 6.98 % |
| held-out sung frames below 160 Hz shown, stems (ccMixter / MUSDB / test songs) | 44.5 / 44.3 / 47.7 % (section 19) | 50.3 / 49.8 / 51.7 % (`s4/runs/bands-heldout-final.txt`) |

The breathy rows are still worse than Stage 0 at 16-96 kHz: the generator's breath goes
through formant filters 60 Hz wide and rings on the subharmonic of the highest "ee" / "oo"
notes (C#5-B5); the harmonic check removes the cases where the higher candidate exists, not
the frames where breath alone makes the octave below look periodic. The 16 kHz rows carry
more breath in band than the others (section 19, item 6).

**Processor** (`tracker_bench cpu`, best of 5, this PC idle): 1.14 / 2.07 / 2.13 / 4.06 ms per
second of audio at 16 / 44.1 / 48 / 96 kHz (Stage 1: 0.89 / 1.66 / 1.72 / 3.32), 22 us per
analysis at 48 kHz, 0.21 % of one core. The extra is the third inverse FFT (the suppressed
NSDF). MacBook Air M1 estimate (x2-3, section 8): 4.3-6.5 ms per second of audio. No
allocation per block (`tests/tracker_alloc.rs`), same answer at every sample rate (above),
the delay of the pipeline itself 41.3 ms.

### 20.5 Tried and dropped (dev)

Each measured on dev with the other changes in place (`s4/log*.txt`; RPA / voicing recall /
false alarms on clean input unless stated):

- **Freshness tail** (a frame is a tail when less than a share of its power is more than a room
  could carry over from the same bin 50 ms earlier): rev15 false alarms 18.0 -> 16.4-13.9 % for
  0.1-2.6 points of clean voicing recall, a worse trade than the level rule; the generated
  room's per-bin power fluctuates too much frame to frame (as real rooms do).
- **Keep only while the suppressed NSDF stays periodic** (0.3-0.6): false alarms rev15 18.0 ->
  15.1 % at 0.6 for 1.3 points of clean voicing recall; little gain.
- **The suppressed spectrum for pitch and candidates too** (20.1, change 1): -6 points of RPA.
- **Lower start thresholds**: confident start at 0.90 (+1.2 points clean, +2.4 under RT60 0.8 s,
  but octave jumps 70 -> 99 and onset flashes doubled); steady start at 0.75 (+0.5 points, but
  generated onset errors 6 -> 12 per rate); keep at 0.70 (+1.1 points, false alarms up).
- **Steady starts below 160 Hz** (`steady_min_hz` 120 / 90): +0.6 / +0.9 points of dev frames
  below 160 Hz, music alone voiced 7.9 -> 8.0 / 8.7 %; and a formant-band (1-5 kHz)
  periodicity to allow low steady starts or block bass-like ones: no gain (low frames 82.1 ->
  81.3 % dev).
- **Early start from the older pair only** (decided frame and the next, before the guards
  existed): onsets within 100 ms 87.0 % instead of 88.8 %, and still 20-22 generated onset
  errors per rate.

### 20.6 Unit tests

No test was loosened. `cargo test --release -p jk-pitch` (the repository workspace, with the
other workstreams' code): 67 library tests, 4 `echo_alloc`, 1 `practice_alloc`, 3
`tracker_alloc`, all pass; the 18 tracker tests and 3 allocation tests also pass in the
private tracker-only build. The early start's 160 Hz floor exists because
`short_low_bursts_are_not_notes` failed without it (a 25 ms burst at 155 Hz became a note at
16 kHz). `cargo clippy --all-targets` reports nothing in the tracker files;
`cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin` passes.

### 20.7 Decisions, and what is still open

Decisions (for DECISIONS.md, by the coordinator):

- Live tracker = Stage 1 + late-reverberation suppression for clarity only + 3.5 kHz analysis
  low-pass + harmonic evidence for subharmonic candidates + harmonic steady starts + guarded
  early start + level-based reverb-tail rule; pipeline delay unchanged (41.3 ms); 2.1 ms per
  second of audio at 48 kHz.
- `tracker_eval --delays` reports the onset-delay distribution; the 100 ms share is part of
  the acceptance numbers from now on.

Open:

- **Reverb is not "within a few points"** (RPA cost 4-18 points by room and split, onsets later
  than 100 ms for about a quarter of notes at RT60 0.8 s). Recommendation for the owner's
  checklist: mixer reverb off or low on the USB feed while practising. A stronger fix would
  need a longer look at the past (a proper late-reverberation estimate per bin over several
  hundred ms, or a learned model) and was not attempted here.
- **Real onsets later than 100 ms: 12.8-13.8 %** (vocadito 9-11 %). Part of it is the reference
  marking breathy starts voiced; the tracker's own share is the 41.3 ms pipeline plus the
  window fill.
- **Low voices on processed stems** are shown about half the time (50-52 %), vocadito 86 %.
- **Breathy generated "ee" / "oo" above C5** still drop an octave or a twelfth more often than
  with Stage 0 (above).
- **Music alone** is voiced on 7.0 % of frames (Stage 0 4.9 %); keeping the marker off with
  music in the input stays the echo stage's job.

### 20.8 How to reproduce

```sh
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-tracker"
cargo test --release -p jk-pitch
E="cargo run --release -p jk-pitch --example"
ALL=clean,rev03,rev08,rev15,rev15wet0,comp,comp_heavy,eq_bright,eq_warm,prox,pops,rig,r16k,r48k,r96k
$E tracker_eval -- --split dev --cond $ALL --delays --tsv final-dev.tsv
$E tracker_eval -- --split dev --cond $ALL --delays $(cat $JK_BUILD/practice-tracker/s4/stage1-set.txt) --tsv stage1-dev.tsv
$E tracker_eval -- --split heldout --cond $ALL --delays --tsv final-heldout.tsv
$E tracker_eval -- --manifest $JK_BUILD/practice-tracker/s3/music/manifest.tsv --split all   # music only
$E tracker_synth                     # whole notes, rough voices, legato, 10-minute noise probes
$E tracker_bench                     # sections 2-8 tables on the current tracker
```

`../jukeoke-build/practice-tracker/s4/report.py` writes the tables of 20.2 from the TSV files
in `s4/runs/`; `s4/try.sh NAME --set k=v ...` is the dev loop used for every setting (dev
clean, three reverbs, bright EQ, music only, generated whole notes); `s3/music.py` made the
music-only set (MUSDB18-HQ accompaniment, local only).

## 21. Independent verification of the robustness work (2026-11-19)

A verifier re-ran the robustness work from the same tree, with its own target directory
(`$JK_BUILD/target-wo2-trackerverify`). Evidence is in `../jukeoke-build/practice-tracker/verify-s4/`:
`v-*.txt` (the re-runs), `probe/` (a small crate that probes level changes and whispers
with fresh seeds), `levels-probe.txt` and `whisper-fresh-seeds.txt`.

**Reproduced** (identical to the digit unless noted):

- `cargo test --release -p jk-pitch`: 67 library tests, 4 `echo_alloc`, 1 `practice_alloc`
  and 3 `tracker_alloc`, all pass. In a debug build, `cargo test -p jk-pitch --lib tracker`
  passes (19 tests). `cargo test --release -p jk-engine --test rt_alloc` passes, and
  `rustfmt --check` on the tracker files is clean.
- `tracker_eval --split dev --cond clean,rev08,eq_bright --delays`, run once with the final
  defaults and once with `s4/stage1-set.txt`, then `--split heldout --cond clean,rev08 --delays`
  with the final defaults: every **all** row and every onset-delay row in 20.2 and 20.3 matches.
  The Stage 1 settings reproduce section 13 (dev, clean: RPA 82.9, VR 89.6, VFA 1.36,
  63 flips, 84.3 % of onsets within 100 ms).
- `tracker_synth` with the final defaults matches `s4/runs/synth-final.txt`, including 0
  pitched events in every 10-minute probe. `tracker_bench` gives a delay of 46 / 72 ms, a
  release of 66 / 76 ms, a same-answer maximum of 5.75 cents and 2.14 ms per second of audio
  at 48 kHz. Music only: 6.98 % (6.89 % with the Stage 1 settings). Held-out low band (`s4/bands.py`
  on both trackers' events): 44.5 / 44.3 / 47.7 % with Stage 1, 50.3 / 49.8 / 51.7 % final.
- Ablation (each change switched off on its own; RPA %, with dev / held-out in each cell). The
  held-out runs are for verification only, and nothing was tuned on them. Every change moves
  held-out in the same direction as dev:

| switched off | clean | RT60 0.8 s | bright EQ | held-out false alarms, clean / RT60 0.8 s |
|---|---|---|---|---|
| nothing (final) | 85.0 / 72.5 | 75.3 / 59.5 | 81.2 / 67.0 | 0.94 / 6.17 % |
| late-reverberation suppression | 83.0 / 68.9 | 71.4 / 54.0 | 78.3 / 62.3 | 0.83 / 6.01 % |
| early start | 83.9 / 71.0 | 74.1 / 58.1 | 79.9 / 65.3 | 0.91 / 6.11 % |
| reverb-tail rule | 85.9 / 73.3 | 75.8 / 59.9 | 82.0 / 67.8 | 1.83 / 8.17 % |
| harmonic check | 85.0 / 72.5 | 75.3 / 59.6 | 81.2 / 67.1 | 0.95 / 6.20 % |
| harmonic steady start | 85.3 / 72.6 | 75.6 / 59.8 | 81.5 / 67.1 | 0.99 / 6.42 % |
| 3.5 kHz low-pass (back to 4 kHz) | 84.6 / 72.0 | 74.8 / 58.9 | 80.5 / 66.2 | 0.90 / 5.91 % |

**Corrected:** `s4/runs/synth-stage1.txt` had been run with the harmonic check on
(`odd_cost: 0.3` in its settings line; breathy 367 / 202 / 173 / 165), not with the Stage 1
settings the text gives. It has been replaced with a re-run using `s4/stage1-set.txt`: breathy
448 / 256 / 283 / 244, rough 87 / 18 / 111 and 46, whispered speech 10 / 7. These are the
numbers given in 20.4, which were right. The old file is kept as
`synth-stage1-oddcost-on.txt`.

**Found:**

1. **Whispers with fresh seeds.** The probe ran 120 x 15 s of `speech_noise` per rate, with
   seeds and levels (-36..-3 dBFS) that `tracker_synth` does not use. The final tracker gave
   0 / 3 / 6 / 4 pitched events at 16 / 44.1 / 48 / 96 kHz, in runs of 3-4 events at
   294-308 Hz. The Stage 1 settings gave 12 / 17 / 0 / 6. That is better overall, but not 0,
   and worse at 48 kHz. With `derev_rt60_s=0` every rate gives 0: the clarity measured on
   the suppressed spectrum lifts a few whispered frames over the confident start, even with
   `derev_per_min`. Raising `derev_per_min` to 0.16 is not a free fix: it costs 0.7 points of
   dev RPA on clean input and 1.7 under RT60 0.8 s. Breath with fresh seeds gives 0 at every
   rate.
2. **The tail rule cuts dry fades.** A dry held note fading at 45 / 60 / 100 dB/s is shown
   for only 83 / 77 / 71 % of the time until it is 24 dB down. It is still above -36 dBFS
   there, far above the level gate, and the gaps last up to 100 ms. With `tail_drop_db=0`, or
   with the Stage 1 settings, it is shown 100 % of the time. These cases are unaffected
   (at least 99.5 % shown, gaps of 20 ms at most): fades at 15-30 dB/s, a sudden 12-26 dB
   drop on the same pitch, and legato steps of 1, 2 or 5 semitones up or down to a note
   20 dB quieter. So the clean-input loss in 20.1 item 6 comes from more than the stems'
   own reverb: on any fast release, the marker loses the last ~100 ms. The scorer excuses
   moments without a pitch while the singer makes sound (at least -50 dBFS, up to a quarter
   of the note), so the cost to the grade should be small. The visible effect is that the
   marker ends early.
3. **The tail rule lowers RPA in every condition, reverb included** (table above). What it
   buys is fewer false alarms. About 0.4 of the points that the reverb "cost" improves by in
   20.2 come from the clean figure falling, not from reverb getting better, so read the
   absolute RPA rows there.
4. 20.1 item 3: a ratio of 3 gives 131 breathy events at 48 kHz and 47 octave jumps under
   RT60 0.8 s; a ratio of 4 gives 112 and 58 (the text says 122-131 and 47-49). The ratio
   tries are not in `s4/log*.txt`. The choice of 2 stands.
5. No unit test exercises the tail rule, the early start, the harmonic check or the
   suppression directly. They run inside the existing tests only because they are on by
   default.

## 22. WO2 quality round: low voices, reverb, onset delay, whispers, dry fades (2026-11-20)

The verifier of section 21 and the scoring stream left five tracker findings: (1) low
voices (roughly E2-C3) on real stems shown about half the time, which cost the original
low male singers their top band at low keys (7-31 points, practice-score.md 4.3); (2) a
room reverb (RT60 0.3 / 0.8 / 1.5 s) costing 4-17 points of raw pitch accuracy (RPA) and
23-24 % of onsets later than 100 ms at RT60 0.8 s; (3) 12.8-13.8 % of real onsets shown
more than 100 ms late; (4) the whisper fix holding only for the probe's own seeds; (5) the
reverb-tail rule cutting dry fast fades. This section is the work on them. "Stage 3a" below
is the tracker of git HEAD before the round (`e10d5d2`), built from a private copy and run
on exactly the same material with the same evaluation binary.

The round ran in two passes on 19-20 November (the first was cut off by the usage limit;
its uncommitted changes were kept, measured again and finished in the second). A second
copy of the tracker builder, started by mistake by the message system, added one change
(`comb_exempt_clarity`, item 7 below) for the scoring stream's report that the demo song's
vocal lost its note starts at low keys; the coordinator sanctioned it and it is kept.

### 22.1 What changed

1. **Look-ahead 1 frame instead of 2** (`lookahead_frames`): the pipeline delay falls from
   41.3 to 31.3 ms. It was not safe before (generated onsets of high "ee" / "oo" took the
   wrong octave); item 2 made it safe.
2. **Harmonic comb** (`Tracker::comb`): the mean peak-over-valley ratio (dB, clipped 0..20)
   of a candidate's first 16 harmonics up to 3 kHz, from the power spectrum already there.
   A sung vowel has a comb of 9-13 dB (dev, clear frames), a whispered vowel about 4, breath
   5, a handling thump 1. Used twice: as an **octave cue** (a subharmonic whose comb is
   6 dB weaker than that of the candidate at 2 or 3 times its pitch loses 0.2 of score,
   `comb_octave_cost`; in the first windows of a high "ee" / "oo" half of the octave
   below's "harmonics" are valleys) and as a **start guard** (every start rule needs a mean
   comb of 6 dB, scaled down in proportion below 200 Hz, `start_comb_db`).
3. **Low-band clarity** (`lowband_hz` 1 kHz, `lowband_max_hz` 300 Hz): a fourth inverse FFT,
   of the power spectrum below 1 kHz; a candidate up to 300 Hz with a harmonic series may
   take that NSDF as its clarity. A low voice keeps most of its energy at high harmonic
   numbers, which lose their alignment over the window first (jitter, vibrato, a reverberant
   copy at another pitch). The first pass had 400 Hz; at 359 Hz breath through a narrow
   formant reached a low-band clarity of 0.96 and gave a 30 ms false note at 44.1 kHz, so
   the ceiling is 300 Hz (0.2 points of dev RPA).
4. **Evidence start** (`evidence_need` 0.6, `evidence_base` 0.72, `evidence_min_frames` 5):
   a pitch line followed from frame to frame (the candidate nearest the line within 40
   cents, one frame of gap allowed) collects each frame's clarity above 0.72; a note starts
   when the line holds 0.6, the decided frame is on it, the frames on it have the comb a
   start needs and no octave guard objects. A voice whose clarity hovers at 0.8-0.9 (low,
   processed, reverberant) never shows the run of confident or steady frames the older
   rules ask for but does hold one pitch line; breath and whispers wander and have no comb.
5. **Low lines need less evidence** (second pass, `evidence_low_hz` 200 Hz,
   `evidence_need_low` 0.35, `evidence_min_frames_low` 3): low notes were shown late
   (dev shifted down: 14.6 % of onsets later than 100 ms); the noise that passes a relaxed
   evidence start lies above 200 Hz (whispered vowels at 260-310 Hz, breath at 360-760 Hz).
   **Low starts** (`low_start_hz` 160 Hz, `low_min_frames` 6): below 160 Hz a non-confident
   start needs a pitch line of 60 ms, so a 25 ms periodic burst is not a note (unit test
   `short_low_bursts_are_not_notes`; 3, 4 and 5 frames all failed it). Steady starts below
   160 Hz are now allowed through this rule (before: none).
6. **Reverb-aware tail rule**: the tail rule (now 10 dB under the recent peak, the fall judged
   by the least-squares trend of the level over 80 ms, `tail_fit_frames`) applies only while
   the input sounds reverberant; while it does, a note keeps going at clarity 0.7
   (`reverb_clarity_off`; 0.6 in the first pass showed more of the previous note inside
   the next one, so the right-note share under reverb fell below Stage 3a's). **The
   reverberant judge** (second pass): every time the level falls 20 dB under its recent peak
   (an ending), the fall over the last 30 ms is noted; the input counts as reverberant while
   the average of the recent falls (each new one half the weight) is under 8 dB per 30 ms.
   Measured on dev (event levels, `q/endings.py`): median fall 11.6 dB on clean real voices
   (30 % of single endings under 8 dB: singers fade and stems carry their own reverb),
   5.4 / 3.4 / 3.0 dB under RT60 0.3 / 0.8 / 1.5 s (83-97 % under 8 dB). The first pass's
   judge (25 dB endings, 12 dB per 30 ms, a share of reverberant endings) called clean dev
   input reverberant on 36-69 % of the frames per set; the second on 16-54 % (vocadito 16 %,
   ccMixter 23-24 %, MUSDB 44-54 %, whose stems carry reverb), and RT60 0.8 s input on
   66-87 %.
7. **Comb exemption** (the second copy, `comb_exempt_clarity` 0.97): a frame whose plain
   clarity is 0.97 or more needs no comb. A clean sung tone with few harmonics (the demo
   song's vocal, three strong harmonics, shifted down by the engine) had most of its
   harmonic positions under the window's leakage (comb 5.1 dB against 6 needed at key -9)
   at clarity 0.99-1.00; whispered vowels never exceed 0.913.

Nothing in `PitchEvent`, `Tracker` or its methods changed. `TrackerConfig` gained fields
(serde default, additive) and changed default values; the only user outside the tracker,
`practice::ChainConfig`, takes `TrackerConfig::default()`. No allocation per block
(`tests/tracker_alloc.rs`).

### 22.2 Material and protocol

- **Real voices** (section 13.1), now with **two 60 s windows** per long stem
  (`tracker_eval_prep.py --second`: the best window not overlapping the first): dev 112
  clips (vocadito 23, ccMixter 22 + 21, MUSDB18-HQ 23 + 22), held-out 108 clips (vocadito
  17, ccMixter 19 + 16, MUSDB 27 + 23, the three CC test songs' separated leads 3 + 3).
  Same singer-disjoint split. The first held-out windows were run once by earlier stages;
  the second held-out windows had never been tracked before this round's final run.
- **Low voices**: every clip shifted -7 and -12 semitones with the app's own Rubber Band
  settings (`jk-rubberband`, R3 real-time, high-quality pitch, formants shifted; start pad
  fed, start delay dropped; reference f0 scaled), dev 224 and held-out 216 clips
  (`q/shift`). This puts real male voices at C2-A2, where WO2 4.4 aims the range.
- **Tuning** used dev only (both windows, both shifts), the generated voices and the
  not-singing probes with the seeds of `tracker_synth` and of the section 21 verifier.
- **Held-out, run once at the end** with the final settings, for both trackers: the real
  held-out set under every rig condition, the shifted held-out set (clean, RT60 0.8 s),
  not-singing probes with **fresh seeds** (120 x 15 s per kind and rate, seeds 7,000,000 +
  977 k, levels -40..-2 dBFS, never used before), the scorer's key sweep on the test songs
  (`jukeoke-cli practice sweep`, the same scorer built with each tracker), the section 21
  level-change probe, music alone. Nothing was changed after these runs.
- The demo song's vocal at every key (`q/demo/`, the second copy's regression set, and the
  real `practice sweep`) is a regression check, not tuning material.

### 22.3 Results on real voices, every rig condition

**dev, all sets** (Stage 3a -> quality round; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|---|---|
| clean | 83.5 -> **84.4** |  | 89.9 -> 90.7 | 0.61 -> 0.80 | 99.10 -> 99.14 | 0.44 -> 0.38 | 116 -> 75 | 14.2 -> **6.0** | 74.1 -> **78.1** |
| compressor | 84.0 -> **84.9** | +0.5 -> **+0.4** | 90.4 -> 91.2 | 0.65 -> 0.79 | 99.04 -> 99.09 | 0.44 -> 0.39 | 116 -> 81 | 14.3 -> **6.3** | 74.9 -> **78.3** |
| heavy compressor | 84.2 -> **85.1** | +0.7 -> **+0.6** | 90.6 -> 91.5 | 0.70 -> 0.80 | 98.99 -> 99.06 | 0.43 -> 0.39 | 111 -> 78 | 14.3 -> **6.0** | 75.0 -> **78.2** |
| bright EQ | 79.4 -> **81.4** | -4.0 -> **-3.1** | 86.4 -> 88.2 | 0.49 -> 0.64 | 99.11 -> 99.11 | 0.45 -> 0.42 | 88 -> 62 | 18.3 -> **8.7** | 69.6 -> **75.0** |
| warm EQ | 84.8 -> **85.5** | +1.3 -> **+1.0** | 91.0 -> 91.6 | 0.83 -> 1.05 | 98.85 -> 98.95 | 0.52 -> 0.42 | 123 -> 79 | 12.6 -> **5.7** | 79.5 -> **82.5** |
| proximity + pops + thumps | 83.7 -> **84.4** | +0.3 -> **-0.1** | 89.9 -> 90.5 | 0.92 -> 1.11 | 98.62 -> 98.61 | 0.45 -> 0.48 | 88 -> 63 | 13.6 -> **6.6** | 82.6 -> **84.4** |
| proximity bass | 85.1 -> **85.5** | +1.6 -> **+1.1** | 91.2 -> 91.7 | 0.99 -> 1.17 | 98.62 -> 98.60 | 0.43 -> 0.48 | 88 -> 63 | 11.9 -> **5.7** | 84.3 -> **85.9** |
| resampled 16 kHz | 83.2 -> **84.3** | -0.3 -> **-0.2** | 89.7 -> 90.6 | 0.61 -> 0.80 | 99.07 -> 99.11 | 0.47 -> 0.40 | 113 -> 79 | 14.3 -> **6.0** | 74.0 -> **78.1** |
| resampled 48 kHz | 83.5 -> **84.5** | +0.0 -> **+0.1** | 89.9 -> 90.7 | 0.61 -> 0.80 | 99.11 -> 99.15 | 0.43 -> 0.37 | 115 -> 78 | 14.2 -> **5.3** | 74.1 -> **78.2** |
| resampled 96 kHz | 83.5 -> **84.5** | +0.0 -> **+0.0** | 89.9 -> 90.7 | 0.61 -> 0.81 | 99.11 -> 99.15 | 0.44 -> 0.37 | 115 -> 78 | 14.2 -> **5.3** | 74.1 -> **78.1** |
| reverb RT60 0.3 s, -6 dB | 78.7 -> **78.5** | -4.8 -> **-6.0** | 86.7 -> 86.5 | 1.28 -> 0.98 | 97.18 -> 97.29 | 0.70 -> 0.66 | 109 -> 99 | 23.6 -> **13.0** | 66.1 -> **70.7** |
| reverb RT60 0.8 s, -6 dB | 73.8 -> **74.5** | -9.7 -> **-9.9** | 82.7 -> 83.6 | 5.28 -> 2.93 | 95.37 -> 95.34 | 0.60 -> 0.66 | 66 -> 51 | 25.1 -> **14.2** | 58.2 -> **66.4** |
| reverb RT60 1.5 s, -6 dB | 68.8 -> **70.6** | -14.7 -> **-13.9** | 78.1 -> 80.2 | 10.53 -> 7.24 | 94.33 -> 93.99 | 0.52 -> 0.59 | 42 -> 34 | 28.4 -> **16.6** | 49.8 -> **60.0** |
| reverb RT60 1.5 s, 0 dB | 55.3 -> **57.8** | -28.2 -> **-26.7** | 67.8 -> 71.3 | 13.64 -> 10.86 | 86.88 -> 85.91 | 0.70 -> 0.81 | 52 -> 39 | 49.3 -> **38.5** | 39.3 -> **51.0** |
| the whole rig | 73.1 -> **75.0** | -10.4 -> **-9.4** | 80.9 -> 83.1 | 3.26 -> 2.96 | 97.27 -> 97.07 | 0.40 -> 0.52 | 31 -> 22 | 33.5 -> **23.1** | 66.1 -> **72.6** |

**held-out, all sets** (Stage 3a -> quality round; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|---|---|
| clean | 71.8 -> **73.9** |  | 82.4 -> 84.3 | 0.62 -> 0.96 | 98.74 -> 98.73 | 0.47 -> 0.41 | 66 -> 56 | 15.4 -> **6.2** | 53.6 -> **60.8** |
| compressor | 72.2 -> **74.2** | +0.4 -> **+0.3** | 82.9 -> 84.7 | 0.72 -> 0.81 | 98.67 -> 98.67 | 0.46 -> 0.42 | 67 -> 51 | 15.2 -> **6.9** | 53.8 -> **60.9** |
| heavy compressor | 72.3 -> **74.2** | +0.5 -> **+0.3** | 83.0 -> 84.8 | 0.81 -> 0.88 | 98.60 -> 98.59 | 0.45 -> 0.42 | 68 -> 55 | 15.1 -> **7.1** | 53.8 -> **60.9** |
| bright EQ | 66.0 -> **69.6** | -5.8 -> **-4.3** | 77.3 -> 80.7 | 0.48 -> 0.75 | 98.84 -> 98.74 | 0.46 -> 0.48 | 48 -> 36 | 21.4 -> **9.6** | 44.3 -> **53.8** |
| warm EQ | 74.8 -> **76.1** | +3.0 -> **+2.2** | 84.8 -> 85.9 | 0.79 -> 1.07 | 98.36 -> 98.38 | 0.45 -> 0.37 | 71 -> 58 | 12.7 -> **5.1** | 62.5 -> **68.0** |
| proximity + pops + thumps | 74.8 -> **75.7** | +3.0 -> **+1.8** | 84.6 -> 85.4 | 1.03 -> 1.34 | 97.60 -> 97.63 | 0.39 -> 0.35 | 42 -> 40 | 12.0 -> **4.7** | 69.1 -> **73.5** |
| proximity bass | 76.0 -> **76.7** | +4.2 -> **+2.8** | 85.8 -> 86.4 | 1.10 -> 1.38 | 97.59 -> 97.62 | 0.39 -> 0.35 | 46 -> 39 | 12.0 -> **4.5** | 70.0 -> **74.2** |
| resampled 16 kHz | 71.5 -> **73.6** | -0.3 -> **-0.3** | 82.1 -> 84.0 | 0.61 -> 0.96 | 98.74 -> 98.73 | 0.46 -> 0.41 | 72 -> 60 | 16.0 -> **6.7** | 53.3 -> **60.6** |
| resampled 48 kHz | 71.9 -> **73.9** | +0.1 -> **+0.0** | 82.4 -> 84.3 | 0.62 -> 0.97 | 98.76 -> 98.73 | 0.45 -> 0.41 | 68 -> 58 | 15.8 -> **6.5** | 53.7 -> **60.8** |
| resampled 96 kHz | 71.9 -> **73.9** | +0.1 -> **+0.0** | 82.4 -> 84.3 | 0.62 -> 0.97 | 98.77 -> 98.73 | 0.43 -> 0.41 | 66 -> 56 | 15.8 -> **6.4** | 53.7 -> **60.8** |
| reverb RT60 0.3 s, -6 dB | 63.6 -> **65.2** | -8.2 -> **-8.7** | 75.9 -> 77.4 | 1.19 -> 0.94 | 96.50 -> 96.44 | 0.83 -> 0.73 | 85 -> 58 | 26.1 -> **14.7** | 37.3 -> **46.0** |
| reverb RT60 0.8 s, -6 dB | 58.7 -> **60.9** | -13.1 -> **-13.0** | 71.2 -> 73.6 | 4.06 -> 2.44 | 95.23 -> 94.98 | 0.57 -> 0.58 | 48 -> 34 | 24.7 -> **15.2** | 27.6 -> **36.5** |
| reverb RT60 1.5 s, -6 dB | 54.2 -> **57.0** | -17.6 -> **-16.9** | 66.6 -> 69.9 | 6.27 -> 4.96 | 94.42 -> 94.13 | 0.50 -> 0.51 | 32 -> 26 | 26.3 -> **18.7** | 21.7 -> **30.2** |
| reverb RT60 1.5 s, 0 dB | 41.1 -> **44.2** | -30.7 -> **-29.7** | 54.5 -> 58.6 | 7.89 -> 7.54 | 87.09 -> 86.23 | 0.58 -> 0.62 | 21 -> 21 | 45.7 -> **36.8** | 13.3 -> **20.9** |
| the whole rig | 58.5 -> **61.5** | -13.3 -> **-12.4** | 70.1 -> 73.5 | 2.62 -> 2.32 | 96.47 -> 96.25 | 0.33 -> 0.37 | 15 -> 14 | 27.4 -> **18.9** | 37.5 -> **46.5** |

Per set, clean:

**dev, clean, per set** (Stage 3a -> quality round)

| set | VR % | VFA % | RPA % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|
| ccmixter | 95.9 -> 96.0 | 0.15 -> 0.17 | 91.2 -> **91.4** | 0.22 -> 0.22 | 27 -> 22 | 12.0 -> **4.3** | 84.3 -> **85.1** |
| ccmixter-b | 95.1 -> 95.2 | 0.14 -> 0.17 | 90.3 -> **90.4** | 0.27 -> 0.27 | 16 -> 18 | 10.3 -> **5.7** | 82.0 -> **84.0** |
| musdb | 83.6 -> 85.1 | 0.68 -> 0.74 | 75.4 -> **77.2** | 0.83 -> 0.71 | 38 -> 23 | 28.0 -> **16.0** | 60.4 -> **66.4** |
| musdb-b | 80.7 -> 82.7 | 0.24 -> 0.38 | 72.8 -> **74.9** | 0.66 -> 0.54 | 30 -> 11 | 32.8 -> **17.2** | 58.8 -> **65.8** |
| vocadito | 96.1 -> 96.5 | 3.40 -> 4.60 | 92.2 -> **92.5** | 0.23 -> 0.09 | 5 -> 1 | 11.1 -> **3.6** | 88.0 -> **90.5** |
| all | 89.9 -> 90.7 | 0.61 -> 0.80 | 83.5 -> **84.4** | 0.44 -> 0.38 | 116 -> 75 | 14.2 -> **6.0** | 74.1 -> **78.1** |

**held-out, clean, per set** (Stage 3a -> quality round)

| set | VR % | VFA % | RPA % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|
| ccmixter | 80.1 -> 83.8 | 0.22 -> 0.32 | 64.9 -> **68.6** | 0.20 -> 0.13 | 10 -> 4 | 23.0 -> **8.2** | 50.4 -> **58.1** |
| ccmixter-b | 81.3 -> 83.8 | 0.34 -> 0.42 | 66.3 -> **69.0** | 0.48 -> 0.47 | 8 -> 9 | 13.5 -> **7.7** | 50.0 -> **56.8** |
| musdb | 81.6 -> 82.3 | 0.30 -> 0.36 | 74.1 -> **74.9** | 0.58 -> 0.54 | 19 -> 16 | 18.8 -> **5.9** | 50.9 -> **59.6** |
| musdb-b | 83.1 -> 84.2 | 0.15 -> 0.24 | 75.9 -> **77.1** | 0.70 -> 0.60 | 21 -> 22 | 24.0 -> **14.0** | 59.6 -> **67.7** |
| songs | 70.1 -> 77.8 | 0.00 -> 0.00 | 55.4 -> **62.1** | 0.22 -> 0.06 | 1 -> 0 | 50.0 -> **25.0** | 51.7 -> **58.4** |
| songs-b | 71.8 -> 78.2 | 0.00 -> 0.00 | 58.5 -> **64.6** | 0.41 -> 0.22 | 1 -> 1 | 71.4 -> **0.0** | 50.7 -> **58.2** |
| vocadito | 97.4 -> 97.6 | 4.75 -> 7.99 | 94.0 -> **94.1** | 0.17 -> 0.18 | 6 -> 4 | 9.4 -> **3.8** | 85.7 -> **88.0** |
| all | 82.4 -> 84.3 | 0.62 -> 0.96 | 71.8 -> **73.9** | 0.47 -> 0.41 | 66 -> 56 | 15.4 -> **6.2** | 53.6 -> **60.8** |

### 22.4 Low voices (finding 1)

By pitch band (shown / within 50 cents, frames), clean and under RT60 0.8 s:

**dev, clean, reference-voiced frames shown (and within 50 cents) by pitch band, all sets**

| tracker | < 63.5 Hz (below C2) | 63.5-73 Hz (C2-D2) | 73-82 Hz (D2-E2) | 82-110 Hz (E2-A2) | 110-130 Hz (A2-C3) | 130-160 Hz (C3-D#3) | 160-250 Hz | >= 250 Hz | C2-160 Hz |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a, clean | 0.0 / 0.0 (3) | 0.0 / 0.0 (17) | 8.3 / 8.3 (145) | 70.0 / 69.0 (2632) | 76.4 / 75.2 (7061) | 74.4 / 73.5 (24720) | 82.3 / 81.6 (128215) | 87.6 / 86.8 (175981) | 74.1 / 73.2 (34575) |
| Stage 3a, reverb RT60 0.8 s, -6 dB | 0.0 / 0.0 (3) | 0.0 / 0.0 (17) | 5.5 / 4.8 (145) | 37.9 / 34.0 (2632) | 53.3 / 48.7 (7061) | 62.1 / 57.8 (24720) | 74.7 / 71.0 (128215) | 83.1 / 79.7 (175981) | 58.2 / 53.8 (34575) |
| this round, clean | 0.0 / 0.0 (3) | 0.0 / 0.0 (17) | 11.7 / 11.7 (145) | 76.3 / 75.3 (2632) | 80.2 / 78.9 (7061) | 78.2 / 77.3 (24723) | 83.9 / 83.2 (128246) | 87.5 / 86.8 (176004) | 78.1 / 77.2 (34578) |
| this round, reverb RT60 0.8 s, -6 dB | 0.0 / 0.0 (3) | 0.0 / 0.0 (17) | 7.6 / 6.9 (145) | 55.0 / 49.8 (2632) | 66.1 / 59.9 (7061) | 68.1 / 63.4 (24723) | 76.0 / 72.3 (128246) | 82.1 / 78.8 (176004) | 66.4 / 61.4 (34578) |

**held-out, clean, reference-voiced frames shown (and within 50 cents) by pitch band, all sets**

| tracker | < 63.5 Hz (below C2) | 63.5-73 Hz (C2-D2) | 73-82 Hz (D2-E2) | 82-110 Hz (E2-A2) | 110-130 Hz (A2-C3) | 130-160 Hz (C3-D#3) | 160-250 Hz | >= 250 Hz | C2-160 Hz |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a, clean | 0.0 / 0.0 (453) | 20.9 / 20.9 (67) | 67.8 / 67.6 (423) | 44.7 / 43.1 (7019) | 48.8 / 47.5 (15940) | 57.1 / 55.9 (39076) | 74.9 / 74.0 (117612) | 80.3 / 79.5 (127511) | 53.6 / 52.4 (62525) |
| Stage 3a, reverb RT60 0.8 s, -6 dB | 0.2 / 0.0 (453) | 9.0 / 3.0 (67) | 30.0 / 25.3 (423) | 13.4 / 11.9 (7019) | 21.2 / 20.1 (15940) | 32.8 / 30.7 (39076) | 63.3 / 59.9 (117612) | 77.0 / 73.9 (127511) | 27.6 / 25.8 (62525) |
| this round, clean | 0.0 / 0.0 (454) | 13.2 / 11.8 (68) | 77.1 / 76.8 (423) | 56.4 / 54.1 (7019) | 59.1 / 57.5 (15942) | 62.1 / 60.8 (39083) | 76.6 / 75.6 (117623) | 80.4 / 79.7 (127529) | 60.8 / 59.3 (62535) |
| this round, reverb RT60 0.8 s, -6 dB | 0.7 / 0.4 (454) | 13.2 / 5.9 (68) | 48.9 / 43.3 (423) | 23.8 / 21.7 (7019) | 32.6 / 30.6 (15942) | 40.3 / 37.3 (39083) | 65.8 / 62.0 (117623) | 76.2 / 73.3 (127529) | 36.5 / 33.9 (62535) |

The same clips shifted down 7 and 12 semitones by the engine's Rubber Band settings:

**dev shifted -7 / -12, all sets** (Stage 3a -> quality round; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|---|---|
| clean | 77.9 -> **80.8** |  | 84.8 -> 87.5 | 0.52 -> 0.70 | 98.98 -> 99.03 | 0.50 -> 0.41 | 204 -> 145 | 23.9 -> **14.3** | 73.7 -> **78.4** |
| reverb RT60 0.8 s, -6 dB | 65.2 -> **69.2** | -12.7 -> **-11.5** | 74.1 -> 78.4 | 4.18 -> 2.72 | 94.97 -> 95.16 | 0.86 -> 0.68 | 214 -> 154 | 38.4 -> **25.1** | 59.9 -> **67.4** |

**dev shifted -7 / -12, clean, reference-voiced frames shown (and within 50 cents) by pitch band, all sets**

| tracker | < 63.5 Hz (below C2) | 63.5-73 Hz (C2-D2) | 73-82 Hz (D2-E2) | 82-110 Hz (E2-A2) | 110-130 Hz (A2-C3) | 130-160 Hz (C3-D#3) | 160-250 Hz | >= 250 Hz | C2-160 Hz |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a, clean | 6.7 / 6.1 (8838) | 64.6 / 63.6 (17353) | 70.8 / 69.0 (19696) | 70.6 / 69.7 (106434) | 73.4 / 72.7 (98874) | 77.9 / 77.1 (134058) | 86.3 / 85.6 (240207) | 92.3 / 91.5 (52088) | 73.7 / 72.8 (376415) |
| Stage 3a, reverb RT60 0.8 s, -6 dB | 4.8 / 3.1 (8838) | 46.4 / 42.7 (17353) | 53.7 / 48.3 (19696) | 54.4 / 50.4 (106434) | 58.9 / 55.4 (98874) | 67.7 / 63.9 (134058) | 80.3 / 77.2 (240207) | 89.1 / 86.8 (52088) | 59.9 / 56.0 (376415) |
| this round, clean | 7.2 / 6.6 (8838) | 69.4 / 68.3 (17355) | 75.0 / 73.2 (19700) | 76.2 / 75.3 (106459) | 79.5 / 78.8 (98894) | 81.1 / 80.2 (134084) | 86.7 / 86.1 (240237) | 92.6 / 91.9 (52095) | 78.4 / 77.5 (376492) |
| this round, reverb RT60 0.8 s, -6 dB | 6.2 / 4.3 (8838) | 55.8 / 51.5 (17355) | 61.0 / 55.4 (19700) | 63.8 / 59.5 (106459) | 69.0 / 65.3 (98894) | 71.6 / 67.7 (134084) | 80.3 / 77.2 (240237) | 88.2 / 86.1 (52095) | 67.4 / 63.4 (376492) |

**held-out shifted -7 / -12, all sets** (Stage 3a -> quality round; cost = RPA change against clean input of the same tracker)

| condition | RPA % | cost (points) | VR % | VFA % | right note when shown (Acc) % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|---|---|
| clean | 63.6 -> **67.3** |  | 74.5 -> 78.2 | 0.50 -> 0.72 | 98.68 -> 98.62 | 0.49 -> 0.47 | 114 -> 83 | 26.2 -> **15.7** | 60.9 -> **66.3** |
| reverb RT60 0.8 s, -6 dB | 48.0 -> **53.7** | -15.7 -> **-13.6** | 59.4 -> 66.0 | 3.07 -> 2.04 | 94.70 -> 94.61 | 0.79 -> 0.66 | 110 -> 79 | 40.1 -> **26.9** | 42.0 -> **51.0** |

**held-out shifted -7 / -12, clean, per set** (Stage 3a -> quality round)

| set | VR % | VFA % | RPA % | Oct % | flips | onsets later than 100 ms % | C2-160 Hz shown % |
|---|---|---|---|---|---|---|---|
| ccmixter-b-k-12 | 66.2 -> 70.1 | 0.16 -> 0.28 | 51.6 -> **55.4** | 0.67 -> 0.38 | 10 -> 5 | 40.4 -> **21.2** | 53.9 -> **58.6** |
| ccmixter-b-k-7 | 73.4 -> 78.2 | 0.28 -> 0.39 | 58.7 -> **63.3** | 0.63 -> 0.52 | 7 -> 8 | 28.8 -> **13.5** | 52.7 -> **59.1** |
| ccmixter-k-12 | 64.1 -> 68.7 | 0.08 -> 0.15 | 49.5 -> **53.9** | 0.58 -> 0.17 | 11 -> 2 | 39.3 -> **23.0** | 52.1 -> **57.5** |
| ccmixter-k-7 | 73.0 -> 78.5 | 0.13 -> 0.24 | 58.0 -> **63.2** | 0.42 -> 0.21 | 14 -> 8 | 36.1 -> **19.7** | 51.6 -> **58.1** |
| musdb-b-k-12 | 75.8 -> 80.1 | 0.10 -> 0.11 | 68.0 -> **72.2** | 0.64 -> 0.64 | 15 -> 12 | 52.0 -> **36.0** | 69.1 -> **73.9** |
| musdb-b-k-7 | 80.7 -> 83.0 | 0.12 -> 0.13 | 73.3 -> **75.7** | 0.49 -> 0.55 | 17 -> 13 | 40.0 -> **22.0** | 71.2 -> **75.7** |
| musdb-k-12 | 74.2 -> 77.7 | 0.16 -> 0.20 | 66.5 -> **69.7** | 0.51 -> 0.72 | 14 -> 17 | 35.3 -> **22.4** | 65.6 -> **71.0** |
| musdb-k-7 | 78.5 -> 80.5 | 0.18 -> 0.22 | 71.0 -> **72.9** | 0.34 -> 0.60 | 20 -> 16 | 30.6 -> **14.1** | 64.8 -> **69.9** |
| songs-b-k-12 | 45.8 -> 53.5 | 0.00 -> 0.00 | 35.9 -> **42.8** | 0.61 -> 0.21 | 1 -> 0 | 85.7 -> **85.7** | 44.0 -> **52.2** |
| songs-b-k-7 | 60.7 -> 71.7 | 0.00 -> 0.00 | 48.2 -> **57.8** | 0.52 -> 0.05 | 1 -> 0 | 57.1 -> **28.6** | 49.1 -> **58.9** |
| songs-k-12 | 41.4 -> 50.5 | 0.00 -> 0.00 | 32.2 -> **39.6** | 0.67 -> 0.00 | 0 -> 0 | 75.0 -> **37.5** | 39.3 -> **47.6** |
| songs-k-7 | 59.2 -> 70.7 | 0.00 -> 0.13 | 45.9 -> **55.3** | 0.37 -> 0.00 | 0 -> 0 | 62.5 -> **25.0** | 46.8 -> **56.1** |
| vocadito-k-12 | 89.1 -> 90.0 | 4.14 -> 5.86 | 84.8 -> **85.4** | 0.25 -> 0.62 | 1 -> 0 | 18.1 -> **12.5** | 91.6 -> **92.1** |
| vocadito-k-7 | 95.3 -> 95.9 | 4.70 -> 7.04 | 91.2 -> **92.1** | 0.21 -> 0.11 | 3 -> 2 | 11.1 -> **6.9** | 89.2 -> **90.3** |
| all | 74.5 -> 78.2 | 0.50 -> 0.72 | 63.6 -> **67.3** | 0.49 -> 0.47 | 114 -> 83 | 26.2 -> **15.7** | 60.9 -> **66.3** |

**held-out shifted -7 / -12, clean, reference-voiced frames shown (and within 50 cents) by pitch band, all sets**

| tracker | < 63.5 Hz (below C2) | 63.5-73 Hz (C2-D2) | 73-82 Hz (D2-E2) | 82-110 Hz (E2-A2) | 110-130 Hz (A2-C3) | 130-160 Hz (C3-D#3) | 160-250 Hz | >= 250 Hz | C2-160 Hz |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a, clean | 5.6 / 5.1 (24506) | 42.8 / 41.8 (27707) | 47.0 / 45.8 (34664) | 55.3 / 54.1 (131625) | 63.3 / 62.5 (86257) | 74.6 / 73.7 (109839) | 79.2 / 78.7 (168506) | 75.5 / 74.8 (33098) | 60.9 / 59.9 (390092) |
| Stage 3a, reverb RT60 0.8 s, -6 dB | 3.3 / 2.2 (24506) | 18.4 / 16.8 (27707) | 20.5 / 18.6 (34664) | 32.3 / 29.6 (131625) | 44.1 / 41.2 (86257) | 64.8 / 61.5 (109839) | 73.4 / 70.6 (168506) | 71.5 / 69.0 (33098) | 42.0 / 39.3 (390092) |
| this round, clean | 8.4 / 6.2 (24512) | 49.7 / 48.6 (27711) | 53.8 / 52.4 (34670) | 61.6 / 60.5 (131640) | 69.6 / 68.8 (86264) | 77.4 / 76.4 (109854) | 80.2 / 79.7 (168525) | 74.2 / 73.5 (33106) | 66.3 / 65.2 (390139) |
| this round, reverb RT60 0.8 s, -6 dB | 5.5 / 3.3 (24512) | 27.6 / 25.1 (27711) | 30.4 / 27.5 (34670) | 43.4 / 40.1 (131640) | 55.7 / 52.2 (86264) | 68.9 / 65.5 (109854) | 74.7 / 71.9 (168525) | 71.5 / 69.0 (33106) | 51.0 / 47.7 (390139) |

**The scorer's key sweep** (the original lead vocal rendered by the engine at every key and
scored against the guide at that key, `jukeoke-cli practice sweep`, the scoring stream's
current scorer built once with each tracker; grade, voiced share in brackets). The demo
and the owner's song were the scoring stream's development songs; the three CC songs are
held-out material for the tracker:

| song | tracker | key 0 | -12 | -9 | -7 | -5 | -3 | +3 | +7 | +12 | largest difference from key 0: all keys / keys -5..+12 | best against a wrong guide |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| demo | Stage 3a | 100.0 (1.00) | 100.0 (0.98) | 100.0 (0.98) | 100.0 (0.99) | 100.0 (0.99) | 100.0 (0.99) | 100.0 (1.00) | 100.0 (1.00) | 100.0 (1.00) | 0.0 / 0.0 | 8.0 |
| demo | this round | 100.0 (0.99) | 100.0 (0.95) | 100.0 (0.96) | 100.0 (0.97) | 100.0 (0.98) | 100.0 (0.98) | 100.0 (1.00) | 100.0 (1.00) | 100.0 (1.00) | 0.0 / 0.0 | 4.2 |
| owner | Stage 3a | 99.1 (0.94) | 98.9 (0.90) | 99.0 (0.92) | 99.2 (0.93) | 99.2 (0.93) | 99.1 (0.94) | 99.3 (0.95) | 99.2 (0.95) | 99.2 (0.96) | 0.2 / 0.2 | 28.6 |
| owner | this round | 99.2 (0.92) | 99.0 (0.90) | 99.0 (0.91) | 99.0 (0.92) | 99.0 (0.93) | 99.1 (0.92) | 99.1 (0.93) | 99.1 (0.94) | 99.1 (0.94) | 0.4 / 0.2 | 28.0 |
| crowd | Stage 3a | 99.2 (0.83) | 93.5 (0.67) | 96.2 (0.72) | 95.7 (0.73) | 98.6 (0.78) | 99.2 (0.80) | 99.4 (0.88) | 99.4 (0.90) | 99.4 (0.93) | 5.7 / 0.6 | 39.3 |
| crowd | this round | 99.2 (0.86) | 97.9 (0.75) | 98.5 (0.79) | 99.2 (0.82) | 99.1 (0.83) | 99.3 (0.85) | 99.3 (0.91) | 99.4 (0.93) | 99.4 (0.95) | 1.6 / 0.4 | 39.7 |
| feel | Stage 3a | 94.4 (0.79) | 61.1 (0.49) | 89.1 (0.72) | 90.4 (0.74) | 93.3 (0.76) | 95.4 (0.80) | 97.9 (0.88) | 98.7 (0.92) | 99.0 (0.94) | 33.3 / 4.6 | 16.8 |
| feel | this round | 97.1 (0.85) | 62.4 (0.51) | 91.0 (0.77) | 93.3 (0.80) | 95.8 (0.82) | 97.6 (0.86) | 97.9 (0.87) | 98.3 (0.89) | 98.7 (0.91) | 34.7 / 1.6 | 16.9 |
| homeless | Stage 3a | 94.6 (0.77) | 78.7 (0.58) | 84.8 (0.66) | 88.6 (0.69) | 90.7 (0.72) | 94.8 (0.77) | 95.5 (0.85) | 98.0 (0.87) | 97.6 (0.90) | 15.9 / 3.9 | 52.5 |
| homeless | this round | 96.1 (0.81) | 85.2 (0.63) | 93.5 (0.73) | 94.2 (0.76) | 96.2 (0.79) | 96.6 (0.81) | 97.2 (0.87) | 98.4 (0.90) | 97.4 (0.92) | 10.9 / 2.3 | 55.7 |

- The low male CC songs keep their top band much further down: Crowdpleaser's largest
  difference from key 0 falls from 5.7 to 1.6 points, Feel's within -5..+12 from 4.6 to 1.6
  (at -7 / -9: 90.4 / 89.1 -> 93.3 / 91.0), Homeless Man's from 15.9 to 10.9 (at -9: 84.8 ->
  93.5).
- **Not met at the lowest keys**: Feel at -12 (62.4; its melody at MIDI 33-44, 55-104 Hz, is
  partly below C2 = 65.4 Hz and the tracker's 62 Hz floor) and -11 (80.1); Homeless Man at
  -12 / -11 (85.2 / 88.1). Below E2 (82 Hz) the shifted real voices are shown 50-75 % of the
  time (single clean voices, ccMixter and vocadito: 69-90 %), against 76-93 % above E2
  (dev).
- Against a guide shifted by a wrong amount every song stays in the lowest band (at most
  55.7, Homeless Man, F).
- The demo song's vocal scores 100 at every key -12..+12 (the regression the second copy
  fixed: 58-97 at keys -2..-12 with the first pass's comb rule); the owner's song stays
  within 0.4 points of key 0.

### 22.5 Reverb (finding 2) and onset delay (finding 3)

**dev, onset delay, clean, all sets** (share of reference onsets, %)

| tracker | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | p50 / p75 / p90 ms | > 100 ms: on the note / on the reference curve / any pitch |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a | 698 | 12.3 | 41.4 | 32.1 | 8.3 | 1.4 | 4.4 | 70 / 90 / 110 | 14.2 / 10.6 / 10.5 |
| this round | 698 | 26.2 | 45.7 | 22.1 | 3.3 | 0.7 | 2.0 | 60 / 80 / 90 | 6.0 / 4.4 / 4.2 |

**dev, onset delay, reverb RT60 0.8 s, -6 dB, all sets** (share of reference onsets, %)

| tracker | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | p50 / p75 / p90 ms | > 100 ms: on the note / on the reference curve / any pitch |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a | 698 | 12.8 | 34.2 | 27.9 | 13.0 | 2.9 | 9.2 | 80 / 100 / 200 | 25.1 / 21.9 / 19.5 |
| this round | 698 | 21.8 | 40.0 | 24.1 | 7.7 | 2.0 | 4.4 | 70 / 90 / 120 | 14.2 / 12.6 / 12.0 |

**held-out, onset delay, clean, all sets** (share of reference onsets, %)

| tracker | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | p50 / p75 / p90 ms | > 100 ms: on the note / on the reference curve / any pitch |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a | 551 | 16.3 | 37.2 | 31.0 | 9.4 | 2.2 | 3.8 | 70 / 90 / 110 | 15.4 / 11.3 / 10.7 |
| this round | 551 | 29.0 | 41.6 | 23.2 | 3.1 | 1.3 | 1.8 | 60 / 80 / 90 | 6.2 / 4.5 / 4.0 |

**held-out, onset delay, reverb RT60 0.8 s, -6 dB, all sets** (share of reference onsets, %)

| tracker | onsets | <= 50 ms | 50-75 | 75-100 | 100-150 | 150-250 | later or never | p50 / p75 / p90 ms | > 100 ms: on the note / on the reference curve / any pitch |
|---|---|---|---|---|---|---|---|---|---|
| Stage 3a | 551 | 15.4 | 33.2 | 26.7 | 11.3 | 4.0 | 9.4 | 80 / 100 / 210 | 24.7 / 20.1 / 18.7 |
| this round | 551 | 25.2 | 37.0 | 22.5 | 8.5 | 2.0 | 4.7 | 70 / 90 / 110 | 15.2 / 12.3 / 12.0 |

- **Onset delay**: real onsets shown on the note more than 100 ms after the reference's
  first voiced frame fall from 14.2 to 6.0 % (dev) and from 15.4 to 6.2 % (held-out); under
  RT60 0.8 s from 25.1 / 24.7 to 14.2 / 15.2 %; for voices shifted down from 23.9 / 26.2 to
  14.3 / 15.7 %. Median 60 ms, p90 90 ms on clean input. The pipeline's own delay is 31.3 ms;
  on generated notes the first correct event comes 35 ms after the onset in the median and
  62 ms at most (46 / 72). **WO2's "never more than 100 ms" is still not met for every real
  onset**: what is left is evidence (the window must hold enough of a breathy or quiet start
  to be clear) and, below 160 Hz, the 60 ms line a start needs so that thumps are not notes.
- **Reverb**: the absolute RPA under the rooms changes by -0.2 to +3.1 points (dev RT60
  0.3 s -0.2, every other room and split +0.7 to +3.1), false alarms fall by a quarter to a
  half (dev RT60 0.8 s 5.28 -> 2.93 %, held-out 4.06 -> 2.44 %), late onsets fall by about
  10 points, and the right-note share of what is shown stays within 0.4 points of Stage
  3a's in the -6 dB rooms (0.9-1.0 points lower at RT60 1.5 s, 0 dB). **The cost relative to clean input
  does not shrink** (dev -9.7 -> -9.9, held-out -13.1 -> -13.0 at RT60 0.8 s), because clean
  input gained as much. WO2 4.10 ("reverb must not upset the tracker") is **not met** for a
  strong room (a wet part at a quarter of the voice's energy): what is lost is mostly the
  first 50-150 ms of each new note, where the last note's tail is as loud as the new one
  (dev, RT60 0.8 s: 57 % of the reference-voiced frames in the first 50 ms after a note
  change are not shown and 7 % show the previous note; clean 42 % and 0.5 %), and fast
  glides, whose pitch the tail drags back. The knobs that remain (keep threshold, tail
  depth, suppression strength and delay) only trade false alarms and right-note share
  against RPA. The setup checklist should keep the mixer's reverb off or low on the feed to
  the computer.

### 22.6 Generated voices, not singing, dry fades, processor

| measure | Stage 3a | quality round |
|---|---|---|
| steady notes C2..C6 held part: max error / octave errors / voiced | 2.26 c / 0 / 100 % | 2.26 c / 0 / 100 % (vibrato max 12.2 c, slides max 37 c, same) |
| whole notes C2..C6 x 3 vowels, events > 50 c off at 16 / 44.1 / 48 / 96 kHz | 6 / 6 / 6 / 12 | 9 / 10 / 8 / 8 (all at onsets, none held) |
| the same, mild breath (-20 dB) | 34 / 6 / 14 / 9 | 11 / 8 / 13 / 9 |
| the same, breathy (-12 dB) | 379 / 205 / 173 / 138 | **306 / 118 / 86 / 28** |
| rough voices, held events > 600 c off (16 kHz 1 % / 6 % / -20 dB; 2 % / 10 %; 2 % / 10 % / -15 dB; 48 kHz 2 % / 10 % / -15 dB) | 29; 6; 108; 29 | 18; 3; 68; 19 |
| legato steps, change -> first event on the new note, median / max | 60 / 80 ms | 50 / 70 ms, except one 16 kHz "oo" case at 110 ms |
| delay onset -> first correct event, median / max; marker off after the note | 46 / 72 ms; 66 / 76 ms | **35 / 62 ms; 57 / 70 ms** |
| same answer vs 48 kHz (40 s melody): voicing differs / p95 / max | 2 of 4000 / 0.66 c / 5.8 c (16 kHz) | 6 of 4000 / 0.67 c / 29.8 c (16 kHz) |
| shortest periodic burst shown, 65-150 Hz | 35-45 ms | 35-40 ms |
| not singing, tuning seeds (`tracker_synth` 10-minute probes + the section 21 verifier's 30-minute whisper and breath probes, every rate) | 3 events (whispered speech, 48 kHz) | **0** |
| not singing, **fresh seeds** (held-out: 30 min per kind and rate of whispered speech, breath, fricatives, room noise) | 5 events (one 50 ms run at 280 Hz, whispered speech, 16 kHz) | 2 events (one 20 ms run at 743 Hz, whispered speech, 48 kHz); 0 for every other kind and rate |
| dry diminuendo 45 / 60 / 100 dB/s on a single note from a cold start, shown until 24 dB down (section 21 probe, 16 and 48 kHz) | 83 / 77 / 71 % | **100 / 97.5 / 88 %** |
| dry fade 45 / 100 dB/s after dry phrases that end abruptly (unit test) | cut | **shown whole** (after phrases that end in fades: cut more than Stage 3a, section 23) |
| music alone (MUSDB accompaniment, 50 min, nobody singing): frames with a pitch | 7.0 % | 12.3 % (8.0 % without the evidence start) |
| processor, ms per second of audio at 16 / 44.1 / 48 / 96 kHz (best of 5) | 1.14 / 2.07 / 2.13 / 4.06 | 1.43 / 2.55 / 2.70 / 7.36 (0.27 % of one core at 48 kHz; the fourth inverse FFT and the comb) |

- **Whispers (finding 4)**: on fresh seeds, one 20 ms blip in two hours of whispered
  speech across four rates (the section 21 verifier found 13 events in 30-40 ms runs at
  44.1 / 48 / 96 kHz with its seeds; those seeds give 0 now). **Not 0**: the remaining blip
  was found on held-out material and was not tuned away.
- **Dry fades (finding 5)**: after dry phrases that end abruptly a dry fade is shown whole
  (100 % at 45 and 100 dB/s, unit test). After phrases that themselves end in a fade, the
  judge calls the input reverberant and the 10 dB tail rule cuts fades more than Stage 3a's
  did (independent verification, section 23). From a cold start the first ending decides: a dry fade at 100 dB/s
  is itself slow when its level is 20 dB down (3 dB per 30 ms), so it turns the judge to
  "reverberant" and its last 4 dB are cut (88 % shown, gaps up to 30 ms).
- **Music alone** is voiced more often (12.3 against 7.0 % of frames). The tracker is never
  told the expected note (WO2 trap 6); with music in the input the echo stage's gate keeps
  the marker off (Music / Unknown = no pitch). The evidence start, which shows low and
  processed voices, is what lets more of a bass line or a lead instrument through.

### 22.7 Tried and dropped (dev)

- Fewer frames of line below 160 Hz (`low_min_frames` 3 / 4 / 5): +0.9 to +1.8 points on the
  shifted set and 1-3 points fewer late onsets, but a 25 ms periodic burst at 65-155 Hz
  becomes a note (unit test); kept at 6.
- A relaxed evidence start at every pitch (`evidence_need` 0.35-0.45, 3-4 frames): +0.1 to
  +1.1 points dev clean, but whispered vowels at 260-310 Hz and breath at 750 Hz passed
  (2-20 events per rate); hence the relaxation only below 200 Hz.
- Disabling the octave veto of the evidence start: +0.1 points.
- Reverb knobs (keep threshold 0.55-0.75, tail depth 8-13 dB, suppression over-subtraction
  3, delay 40 ms, RT60 0.8 s): each trades false alarms or right-note share against RPA
  within about 1.5 points; none lowers the reverb cost.
- A freshness test on the suppressed NSDF to end a note inside the last note's tail: the
  suppression floors a steady note and a decaying tail alike (both lose more than the floor),
  so the suppressed NSDF does not tell them apart. Not built.
- Unit tests that toggle the early start and the late-reverberation suppression on
  generated voices: no measurable effect there (identical mean onset delay; 100 against 99
  events), so they would test nothing; their effect is measured on real voices (section 21
  ablation table).

### 22.8 Unit tests

`cargo test --release -p jk-pitch --lib tracker::`: 25 tests, all pass (also in the private
tracker-only build); `tests/tracker_alloc.rs` 3, `practice_alloc` 1, `echo_alloc` 4 pass.
New this round: `breath_through_a_formant_gives_no_pitch` (every rate),
`reverb_judge_tells_a_room_from_dry_singing`, `low_lines_start_on_less_evidence`, and from
the first pass `dry_fades_are_shown_and_room_tails_end_the_note`,
`whispers_with_other_seeds_give_no_pitch`, `low_breathy_voices_in_a_room_are_shown`,
`harmonic_comb_tells_voices_from_noise`. No test was loosened. In the workspace,
`cargo test --release -p jk-pitch` reports 3 failures outside the tracker
(`echo::tests::absent_when_nothing_comes_back`, `echo::tests::loud_return_with_a_singer_is_gradable`,
`practice::tests::music_flags_and_ungraded_moments_come_only_from_the_echo_stage`): they fail
the same way with the Stage 3a tracker and the echo stream's current files (`q/ws-orig`),
so they belong to the echo stream's work in progress. `cargo clippy -p jk-pitch
--all-targets` reports nothing in the tracker files; `rustfmt --check` is clean on them;
`cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin` passes.

### 22.9 Decisions, and what is still open

Decisions (for DECISIONS.md, by the coordinator): look-ahead 1 frame (pipeline 31.3 ms);
harmonic comb as start guard and octave cue, with the 0.97 clarity exemption; low-band
clarity up to 300 Hz; evidence start, relaxed below 200 Hz; a 60 ms line below 160 Hz;
reverb-aware tail rule with an averaged-fall judge and a 0.7 keep threshold while
reverberant; the shifted real-voice set and the demo key sweep join the acceptance checks.

Open:

- **Reverb** costs 6-10 points of RPA at RT60 0.3-0.8 s (dev; 9-13 held-out) and 14-17 at
  1.5 s: WO2 4.10 not met for a strong room. A proper late-reverberation estimate over
  several hundred ms per bin, or a learned model, was not attempted.
- **Onsets later than 100 ms**: 6 % of real onsets (14-16 % under strong reverb or for
  voices shifted down to C2-A2).
- **Low voices**: below E2 the shifted real voices are shown 50-75 % of the time, and the
  lowest keys of the low male CC songs still lose points (Feel -12 / -11, Homeless Man -12 /
  -11); below C2 (Feel at -12) is outside WO2's range.
- **Whispered speech** on fresh seeds: one 20 ms blip in two hours.
- **Music alone** voiced on 12.3 % of frames (the echo gate's job).
- Generated: one legato "oo" step at 16 kHz reaches the new note after 110 ms; the same
  answer at 16 kHz has a 29.8-cent maximum (p95 0.67 cents).

### 22.10 How to reproduce

```sh
cd jukeoke && source scripts/devenv.sh
Q=$JK_BUILD/practice-tracker/q
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-q-tracker"
cargo test --release -p jk-pitch --lib tracker:: && cargo test --release -p jk-pitch --test tracker_alloc
"$JK_BUILD/practice-guide/venv/Scripts/python.exe" -I crates/jk-pitch/examples/tracker_eval_prep.py --second   # once
sh $Q/build.sh                        # private tracker-only build (sync + dev examples)
$JK_BUILD/target-wo2-q-tracker/release/qshift.exe $Q/manifest-dev.tsv dev -12,-7 $Q/low-dev   # once (q/shift)
sh $Q/run.sh [--set k=v ...]          # dev loop: clean + RT60 0.8 s, bands, delays
sh $Q/final/run.sh dev; sh $Q/final/run.sh heldout     # both trackers, every condition
python -I $Q/report.py ORIG.tsv NEW.tsv ORIG.txt NEW.txt TITLE                       # the tables above
$JK_BUILD/target-wo2-q-tracker/release/examples/qnoise.exe dev|fresh                # not-singing probes
cargo run --release -p jk-pitch --example tracker_synth; cargo run --release -p jk-pitch --example tracker_bench
jukeoke-cli practice sweep songs/<song>.jksong --keys -12:12 --wrong 1,-1,2,-2,5,-7 --threads 16 --out OUT.json
```

Evidence: `$JK_BUILD/practice-tracker/q/final/` (`orig-*` / `new-*` `.txt` / `.tsv` for
dev, held-out and the shifted sets, `report-*.md`, `sweep/`, `sweep-orig/`,
`sweep-table.md`, `noise-{dev,fresh}-*.txt`, `levels-*.txt`, `synth-*.txt`,
`bench-new.txt`, `music-*.txt`). The Stage 3a baselines build from `q/devcrate-orig`
(tracker only) and `q/ws-orig` (the workspace with git HEAD's tracker, for `jukeoke-cli`).

## 23. Independent verification of the quality round (2026-11-20)

A verifier re-ran the quality round from the working tree with its own target directories
(`$JK_BUILD/target-wo2-q-trackerverify2*`) and added material the builder never ran.
Evidence is in `../jukeoke-build/practice-tracker/verify-q2/` (`repro-heldout.*`,
`third-*`, `lowho-*`, `lowthird-*`, `myrev-*`, `noise-*`, `fades-*`, `revtail-*`, `low-*`,
`lowroom-*`, `rumble-*`, `singers/`, `singers-orig/`, `sweep/`, `bench.txt`; the probes in
`probe-new/` and `probe-orig/`, the Stage 3a workspace copy in `ws-orig/`). "Stage 3a" is
git HEAD's tracker (`q/devcrate-orig`, checked identical to HEAD).

**Reproduced** (identical to the digit):

- `cargo test --release -p jk-pitch`: 81 library tests (the 3 echo / practice failures of
  22.8 are gone in the current tree), `tracker_alloc` 3, `practice_alloc` 1, `echo_alloc` 4.
  `cargo test -p jk-pitch --lib tracker::` in a debug build (overflow checks): 25 pass.
  `rustfmt --check` on the tracker files, `cargo clippy -p jk-pitch --all-targets` and
  `cargo check -p jk-pitch --all-targets --target aarch64-apple-darwin`: clean.
- `tracker_eval --split heldout --cond clean,rev08 --delays --bands` on `q/manifest-heldout.tsv`:
  clean RPA 73.9, VR 84.3, VFA 0.96, onsets later than 100 ms 6.2 %, C2-160 Hz shown 60.8 %;
  RT60 0.8 s RPA 60.9, VFA 2.44, late onsets 15.2 %.
- `describe_delay()` 31.31-31.34 ms at 16 / 44.1 / 48 / 96 kHz (Stage 3a 41.31-41.34).
  `tracker_bench`: onset to first correct event 35 / 62 ms, release 57 / 70 ms, same answer
  max 29.84 cents at 16 kHz. Processor 2.74 ms per second of audio at 48 kHz on a quiet run
  (other runs on the shared machine were noisier; not used).
- `jukeoke-cli practice sweep` (Feel, Homeless Man, Crowdpleaser, keys -12..+12): every key
  equal to `q/final/sweep/`.
- No tuning on held-out: `q/` holds held-out outputs only from the final runs (02:36-02:39);
  none of the event dumps used while tuning (`q/ev-*`) holds a held-out clip; the only
  `tracker.rs` change after the final runs is two doc comments.

**Fresh material the builder never ran** (nothing tuned on it):

- *Third windows*: a third 60 s window of every held-out long stem, overlapping neither of
  the builder's windows (21 clips: ccMixter 8, MUSDB 12, one test song; same reference
  rule, `verify-q2/prep_third.py`). Stage 3a -> now, clean: RPA 70.8 -> 71.2, VFA 0.10 ->
  0.22 %, C2-160 Hz shown 52.2 -> 59.0 %, onsets later than 100 ms 19.0 -> 14.3 % (21
  onsets only); RT60 0.8 s: RPA 59.4 -> 60.9 (cost -11.4 -> -10.3); bright EQ 64.5 -> 66.6;
  the whole rig 58.5 -> 61.2. The clean gain is smaller here than on the builder's held-out
  set (+0.4 against +2.1 points); the low-band gain holds.
- *Other shifts*: every held-out clip shifted -5 and -10 semitones by the same Rubber Band
  settings (216 clips; the builder used -7 / -12). Clean RPA 66.2 -> 69.7; onsets later
  than 100 ms 23.3 -> 12.2 %; shown C2-D2 / D2-E2 / E2-A2 / A2-C3 44.8 / 44.8 / 52.4 / 61.8
  -> 51.7 / 52.5 / 59.6 / 68.2 %; RT60 0.8 s RPA 50.9 -> 56.1, late onsets 35.6 -> 21.2 %.
  Third windows shifted -5 / -10 (42 clips): clean RPA 65.8 -> 68.4, C2-160 Hz 57.5 -> 63.0 %.
- *Another room model*: the held-out set plus the third windows (129 clips) through the
  verifier's own impulse response (pre-delay, early reflections, a double-slope velvet-noise
  tail that darkens; not `rig::reverb`). RT60 0.5 s, -8 dB: RPA 63.4 -> 64.9 (cost -8.2 ->
  -8.5), VFA 1.36 -> 0.79 %, late onsets 25.3 -> 14.5 %. RT60 1.0 s, -6 dB: RPA 54.6 ->
  57.2 (cost -17.0 -> -16.2), VFA 4.39 -> 3.15 %, late onsets 27.1 -> 17.1 %, right note
  when shown 93.97 -> 93.47 %. The reverb result generalises: better in absolute terms,
  the cost against clean input unchanged; WO2 4.10 not met.
- *Fresh synthetic seeds* (seeds and levels unused by the builder; 30 minutes per kind and
  rate at 16 / 44.1 / 48 / 96 kHz). Pitched events, Stage 3a -> now: whispered speech
  3 / 6 / 3 / 8 -> 4 / 0 / 2 / 0 (runs of 40 ms at 292 Hz, 16 kHz, and 20 ms at 296 Hz,
  48 kHz); breath, fricatives, room noise, mains hum, babble at 16-48 kHz, breath in a
  room up to 48 kHz, handling thumps and pops: 0 -> 0; chest breath 0 / 0 / 0 / 3 -> 0 / 0 / 3 / 0
  (30 ms at 707 Hz); babble at 96 kHz 3 -> 2; breath in a room at 96 kHz 0 -> 4 (40 ms at
  756 Hz). A **narrow-band low rumble** (white noise through two resonators at 70-230 Hz,
  bandwidth 35-90 Hz, -50..-20 dBFS) is pitched on 3.7-7.7 % of frames by Stage 3a and
  1.6-3.9 % now, at every level, in runs up to 0.68 s: better, but WO2's "room noise gives
  no pitch" is not met for a tonal rumble.
- *Low generated notes* (C2..C4, four rates, breath, 1 % jitter, a room): held parts on the
  note the same or better, onset to first correct event 70 -> 60 ms median. In the
  verifier's room (RT60 0.5 s, -10 dB) with breath and 0.7 % jitter, 2-5 of 270 notes
  A2-C4 per rate are held a whole octave off ("ee" / "oo" around C3-D#3; 0.8-1.1 % of held
  events, Stage 3a 1.1-1.6 %); dry: 0. Pre-existing, slightly better.

**Found:**

1. **Fades after fading phrases are cut more than before** (finding 5 only partly fixed). The
   judge reads a smooth phrase-end fade as a room: after three phrases that each end in a
   30, 60 or 100 dB/s fade, a held note fading at 30 / 45 / 60 / 100 / 150 dB/s is shown
   until 24 dB down for 71.9 / 54.4 / 50.1 / 46.1 / 44.4 % of the time (Stage 3a 98.6 /
   83.3 / 76.7 / 70.8 / 68.8 %), because the tail rule is now 10 dB deep. After phrases
   that end abruptly the same fades are shown 100 %; from a cold start 100 / 100 / 97.2 /
   87.5 / 81.9 %. On real dry singing the cost does not show: on the held-out vocadito
   clips the last 300 ms of every voiced run is shown 98.0 % with both trackers.
   Sections 1 and 22.6 now say "after phrases that end abruptly".
2. **False alarms on vocadito held-out 4.75 -> 8.00 %**: 656 of the 756 false-alarm frames
   are note tails shown 40 ms (median) to 80 ms (p90) past the annotated end, and the 15
   isolated runs (Stage 3a 7) are the previous note's pitch shown again at -50 to -57 dBFS
   within 150 ms of it, not wandering.
3. Through the whole chain (`jukeoke-cli practice run`, current scorer and echo stage, each
   tracker): silent and noise singers 0 / F with the marker never shown on all five songs,
   also with the music returned by cable or USB at 0 / -6 / -20 / -30 dB and with a
   returned song in a room; perfect A+ (98.5-100); 150 ms rubato 95.5-100; "behind"
   (borrowing the previous note) 47-59 F; a loud return (0 / -6 dB) with a perfect singer
   97.9-100. A perfect singer through RT60 0.8 s moves at most 1.4 points (Stage 3a 4.4);
   through RT60 1.5 s at 0 dB wet up to 16.6 points (Homeless Man 81.9; Stage 3a 28.6).
   With the music returned over Bluetooth and nobody singing, every song is "not graded"
   with both trackers; the marker still shows 17 values on Crowdpleaser (both trackers)
   and 8 on the owner's song (now; Stage 3a 0). For the echo stage, not this tracker:
   Homeless Man's original vocal alone (`--vocal`, nothing returned) is reported as music
   in the input (level -119 dB, removal "unusable") and not graded, with either tracker.
4. Code: `Cand::nd` is computed for every candidate and never read (its comment suggests a
   use); the module documentation said the low-band clarity applies up to 400 Hz (it is
   300 Hz) - corrected.

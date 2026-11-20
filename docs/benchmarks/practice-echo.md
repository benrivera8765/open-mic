# The app's own music in the input: detection and removal (Work Order 2, "echo")

Measured on 2026-11-19 on the build PC (Windows 11, Ryzen 7 9700X, 32 GB), Rust 1.100
`x86_64-pc-windows-gnu`, release profile of the workspace. Every number below comes from
`crates/jk-pitch/examples/echo_eval.rs` (summarised by `examples/echo_eval_report.py`) or from
the tests named in "How to reproduce". The raw results are kept in
`../jukeoke-build/practice-echo/` (`results.jsonl`, one line per scenario; `report.md`, every
table; `cpu.jsonl`). Two runs of the final code gave identical results (126 of 126 rows).

All input is **generated** or read from Creative Commons stems: no microphone was opened,
nobody sang, and the owner's mixer (MAONO AME2) was not available. The "returns" are models
of what a mixer does (section 2). See section 9, "What this does not show".

Code under test: `jk_pitch::echo::EchoCanceller` (`crates/jk-pitch/src/echo.rs` and
`src/echo/`): pure DSP, no threads, no I/O, no allocation per block.

**Stage 3 update (same day, evening).** The verifier's three gaps were worked on and the
engine got its output tap: sections 10 (what changed), 11 (results, with the benchmark and
the held-out songs re-run) and 12 (the output tap). Rows marked "Stage 3" in the summary
below come from there; sections 2-9 and the appendix are the Stage 2 run.

## 1. Summary

WO2 4.4 "Music in the input", 4.10 "two ways to give the app a clean voice" and "The delay
comes for free", trap 4 (two clocks), trap 6 (the mixer can sing), section 9 row "The
owner's rig".

| Requirement | Result (126 scenarios, 3 songs, 45 s each) | Verdict |
|---|---|---|
| Notice its own playback in the input, quickly and reliably, at every level that could disturb the marker | Found in **118 of 118** runs with music returned, -40 to 0 dB, delays 2-300 ms, USB / cable / Bluetooth shapes, with and without a singer and the guide vocal. Median **0.84 s** after the music starts, slowest **4.60 s** (a return 40 dB under the music with a singer 28 dB louder than it, which cannot move the marker). Without a singer: at most 0.98 s at every level. | Pass |
| No false alarm | Nothing returned, singer singing, guide vocal on or off: "absent" after 3.21 s of music in **8 of 8** runs, never "present". | Pass |
| Measure the level | Median error **0.10 dB**; worst 0.40 dB for USB and cable, 5.5 dB for Bluetooth (a wandering delay biases it low). | Pass |
| Measure the delay ("the delay comes for free") | USB and cable: median error **0.004 ms**, worst **0.19 ms** (94 runs, 2-300 ms, with singer). Bluetooth: the current delay of a delay that wanders, within 10 ms of its mean. | Pass |
| Delay stable or wandering | USB and cable: "stable" in **94 of 94** (one filter restart in all 94 runs). Bluetooth: "wandering" in **24 of 24**. Clock drift measured to a median 0.3 ppm, worst 1.6 ppm (30 cable runs at +-100 ppm). | Pass |
| Remove it where it can be done well | Without a singer: USB loopback **30-64 dB** (median 46 dB, 32 runs), cable (EQ, noise, +-100 ppm drift) **29-42 dB** (median 33 dB, 15 runs). With a singer singing over it: USB 12-35 dB (median 26), cable 9-29 dB (median 24): the filter learns only in the singer's pauses. | Pass, with the double-talk limit in section 4 |
| Bluetooth: say plainly it is not good enough | Removal 1-2 dB; quality "unusable" in **24 of 24** Bluetooth runs; the gate then assumes nothing was removed. | Pass |
| With nobody singing and the music fed back, the marker stays off (trap 6, guide vocal on) | Pitch frames the app would show (tracker on the cleaned input, trusted by the gate): **0.00 %** in every USB and cable run (worst 1 frame of 4,500), at most **0.27 %** with Bluetooth. Without removal and gate the tracker reports a pitch in up to **21 %** of the frames (guide vocal on). | Pass |
| With a singer, the tracker after removal vs a clean input | Right pitch (within 50 cents) on steady notes: clean voice 99.9 %; after removal **97-100 %** for returns up to 10 dB under the music (raw input: 66-100 %); 96-97 % for returns as loud as the music (raw: 30 %). With the gate's verdicts applied, from 15 s on: **96-100 %** up to -10 dB; 64-80 % at 0 dB, where quality is "unusable" and the app warns instead of grading. | Pass for returns at or under -10 dB; at 0 dB the app warns |
| CPU per second of audio | 48 kHz: **8.5-20 ms per second of audio (0.9-2.0 % of one core)**, 44.1 kHz 10-17 ms, 96 kHz 19-31 ms, 16 kHz 5-6 ms (best of 3, two sessions while other jobs ran on the PC). Stage 3 code: 7.0 ms at 48 kHz, 6.5 / 13.3 / 3.4 ms at 44.1 / 96 / 16 kHz (section 11.6). | Pass |
| No allocation per block | 0 allocations over 20 s at 16, 44.1, 48 and 96 kHz, with queries and a reset (`tests/echo_alloc.rs`). | Pass |
| Same answer at every rate | USB return on generated music at 16 / 44.1 / 96 kHz: delay 15.313 / 15.305 / 15.296 ms (truth 15.3), level -6.7 / -6.8 / -6.7 dB (truth -6.0), suppression 40.8 / 36.0 / 33.4 dB (`echo::tests::other_rates`); 48 kHz in the other unit tests, 44.1 kHz in the benchmark. | Pass |
| The grade of each scripted singer after removal vs clean (section 9) | **Not measured**: the scorer does not exist yet (Stage 3). The tracker frame accuracy above stands in for it. | Open |
| Stage 3: the loopback switched off (what the warning asks for) | "present" cleared **0.5 s** after with nobody singing, **2.6-3.3 s** with a singer (5 songs; Stage 2: 11-25 s, or never); nothing shown with nobody singing; a loud return no longer hides the singer meanwhile (86-95 % of the singer's frames shown at 0 dB, vs 48-56 % before) | Pass |
| Stage 3: a high voice on "ee" / "oo" | Frames the gate takes away while quality says good or marginal: at most 6.6 points on the 3 developing songs, 2.1 on held-out ccm-59840, but **16.8 on held-out ccm-39381** (Stage 2: 30-41 points, verifier) | Pass on 4 of 5 songs; open on one |
| Stage 3: continuous singing from the first second | A verdict after **2.0-7.6 s** in every case (Stage 2 code: never for returns at -20 dB and under, 2.5-37 s otherwise); gate loss at most 0.9 points where quality is good or marginal; verdicts cautious ("unusable" for a -20 dB return on 2 of 3 songs) | Pass (cautious) |
| Stage 3: the echo path changes (OS / mixer volume -20 to +6 dB) or the app's master volume | No pitch shown with nobody singing in any case (5 songs); "present" kept, no restart; master volume -12 dB costs nothing (41-50 dB still removed), because the output tap is after the master volume | Pass |
| Stage 3: benchmark and held-out songs re-run | Detection 118 of 118 and 166 of 166, no false "present", delay, drift, stability and removal without a singer unchanged; worst gated accuracy with a singer from 15 s: USB 62 -> 97 % (held out 37 -> 68 %), cable 45 -> 82 % (37 -> 76 %); Bluetooth with nobody singing 0.27 % / held out 1.80 % pitch frames (Stage 2: 0.27 / 2.02 %) | Pass; Bluetooth marker open |
| Stage 3: engine output tap | Bit-identical copy of the device output (180,480 frames), DAC stamps within 0.1 ms of the playback clock on a real device, 0 allocations on the callback and the worker (`rt_alloc`), 0.65 us per callback block, nothing copied without a reader; end to end with the recorder's capture of the same device: delay steady to 0.2 ms over 3 runs | Pass |

## 2. The returns, the songs and the singer

**Returns** (`jk_pitch::echo::testpath`, public so later stages can reuse them):

| Shape | What it models | Model |
|---|---|---|
| `usb` | the mixer's USB loopback | fixed delay and level, same clock as the output, mono sum of the stereo playback |
| `usb_eq` | the same through the mixer's EQ | + 40 Hz high-pass, +4 dB at 3 kHz, -3 dB shelf at 10 kHz |
| `cable` | 3.5 mm cable into AUX-IN | fixed delay and level, 25 Hz coupling high-pass, +3 dB low shelf at 120 Hz, -2 dB at 900 Hz, -2.5 dB shelf at 6 kHz, 18 kHz low-pass, -90 dBFS noise, **+-100 ppm clock drift** (the delay grows or shrinks 0.1 ms every second) |
| `bluetooth` | music sent to the mixer over Bluetooth | 120-300 ms delay with a 2 ms RMS random walk (3 s time constant) and a jump of up to +-25 ms every 12 s on average, +40 ppm drift, 15 kHz codec band limit, coding noise 30 dB under the music following its loudness, -90 dBFS noise |

The fractional delay of the models is a 64-tap Kaiser sinc at 4096 phases, deliberately not
the canceller's own 32-tap interpolator. A microphone noise floor of -78 dBFS is added to
every input.

**Songs**: three ccMixter songs with separate stems (Creative Commons, evaluation only, never
committed): ccm-22364 "Persephone" by Madam Snowflake (CC BY 2.5), every scenario;
ccm-31416 "There's Always a Better Day" by Admiral Bob (CC BY 3.0) and ccm-55540 "Altar" by
Zutsuri (CC BY 3.0), a subset of 10 each. A 45 s excerpt from 30 s into each song. The
**reference** (what the app plays) is the instrumental + backing vocal stems at -14 dBFS RMS;
with the **guide vocal on**, the lead vocal stem is added at its level in the song. So
"0 dB" means the music comes back as loud as the app plays it (-14 dBFS RMS).

**Singer** (`tracker::testvoice::sing`, the Stage 0 synthetic voice): a male range melody
A2-E4, phrases of four 0.75 s notes with 0.15 s breaths and a 1.2 s rest, vibrato +-25
cents at 5.5 Hz, breath noise 30 dB under the voice, -18 dBFS RMS: 4 dB under the music
returned at 0 dB, 6 dB above it at -10 dB.

**Scoring**: the live tracker (`jk_pitch::tracker::Tracker`, as it stands in the working
tree) runs on the clean voice, on the raw input and on the canceller's output; a pitch
frame is "right" within 50 cents of the melody on steady parts of notes, "shown" when the
canceller's `trust_window` for that frame says `Clean` or `Singer` (`Music` and `Unknown`
count as no pitch). The true suppression is the returned music's power over what is left of
it in the output (output minus the voice and noise, aligned by the canceller's latency),
from 15 s to 45 s.

## 3. Detection

Method (`src/echo/detect.rs`): both signals low-passed and decimated to about 8 kHz; every
128 ms the last 256 ms of input (Hann window) is cross-correlated with the reference over
every lag from 0 to 600 ms; the cross-spectrum is averaged over 0.3 s, 1 s and 4 s and
weighted by a regularised coherence normaliser (GCC-SCOT). "Present" needs a correlation
peak at least 12 times (9 for the 4 s average) the RMS of the rest, staying within 3 ms over
about a second (at least 5 of 8 hops).

Three findings shaped it (each measured on the way):

- Without a window on the input frame, steady tones in both signals put a false peak at the
  offset between the two analysis windows: z = 18.7 with only a singer in the input, which
  was declared "present". With the Hann window, tapered reference ends and the
  regularisation: z 5.8, "absent".
- Requiring every one of the 8 hops to be confident delayed the Bluetooth warning by 25 s on
  "Altar" (sparse music, z hovering at 6-14); requiring 5 of 8 found it after 1.1 s.
- The 0.3 s average is there for wandering delays, which smear the longer averages.

| Return | Level | Singer | Runs | Found after (worst) | Delay error (worst) | Level error (worst) |
|---|---|---|---|---|---|---|
| USB | -40 dB | no | 5 | 0.98 s | 0.004 ms | 0.37 dB |
| USB | -30 dB | no | 5 | 0.98 s | 0.006 ms | 0.38 dB |
| USB | -20 dB | no | 6 | 0.98 s | 0.006 ms | 0.38 dB |
| USB | -10 dB | no | 5 | 0.98 s | 0.005 ms | 0.37 dB |
| USB | 0 dB | no | 6 | 0.98 s | 0.005 ms | 0.37 dB |
| USB | -40 dB | yes | 5 | 4.60 s | 0.009 ms | 0.38 dB |
| USB | -30 dB | yes | 5 | 4.32 s | 0.009 ms | 0.38 dB |
| USB | -20 dB | yes | 6 | 1.68 s | 0.008 ms | 0.35 dB |
| USB | -10 dB | yes | 5 | 0.98 s | 0.006 ms | 0.31 dB |
| USB | 0 dB | yes | 6 | 0.98 s | 0.007 ms | 0.40 dB |
| cable | -30 dB | no | 2 | 0.84 s | 0.022 ms | 0.10 dB |
| cable | -20 dB | no | 2 | 0.84 s | 0.024 ms | 0.09 dB |
| cable | -10 dB | no | 2 | 0.84 s | 0.019 ms | 0.09 dB |
| cable | 0 dB | no | 2 | 0.84 s | 0.021 ms | 0.09 dB |
| cable | -30 dB | yes | 2 | 4.32 s | 0.188 ms | 0.12 dB |
| cable | -20 dB | yes | 2 | 1.68 s | 0.193 ms | 0.12 dB |
| cable | -10 dB | yes | 2 | 1.12 s | 0.182 ms | 0.12 dB |
| cable | 0 dB | yes | 2 | 0.84 s | 0.024 ms | 0.15 dB |
| Bluetooth | -20 dB | no | 3 | 0.98 s | 9.0 ms (wandering) | 2.8 dB |
| Bluetooth | -10 dB | no | 3 | 0.98 s | 9.0 ms | 2.7 dB |
| Bluetooth | 0 dB | no | 3 | 0.98 s | 9.0 ms | 2.7 dB |
| Bluetooth | -20 dB | yes | 3 | 1.40 s | 9.6 ms | 3.2 dB |
| Bluetooth | -10 dB | yes | 3 | 0.98 s | 9.6 ms | 5.2 dB |
| Bluetooth | 0 dB | yes | 3 | 0.98 s | 9.0 ms | 5.5 dB |

(USB rows include the delays 2, 10, 50, 150 and 300 ms; "USB" includes the `usb_eq` runs. The
delay error of cable runs is against the delay at the end of the run, after 45 s of drift;
of Bluetooth runs against the mean of the wandering delay.)

Which levels "could disturb the marker": without a singer, any return above the tracker's
-55 dBFS level gate, i.e. above about -41 dB at this playback level; all were found within
0.98 s. With a singer, the slow cases (4.3-4.6 s) are returns 30-40 dB under the music,
16-28 dB under the singer: there the raw input already gives 99.9 % right pitch.

Guide vocal on, nothing returned, music breaks and the other songs: section 6.

**Drift (trap 4)**: the 30 cable runs at +-100 ppm: measured drift median error 0.3 ppm,
worst 1.6 ppm; reported delay at the end of the run within 0.024 ms without a singer.

## 4. Removal

Method (`src/echo/filter.rs`, `src/echo/refline.rs`, control in `src/echo.rs`): a
partitioned-block frequency-domain NLMS filter (overlap-save, constrained gradient, block
2.7 ms, 24 ms long, starting 2 ms before the measured delay; one set of partitions per
reference channel, so a mixer that returns only one side of the stereo mix is modelled), run
as a background filter that always learns and a foreground filter that makes the output
(the two-path design of the Speex canceller; independent implementation). The learning rate
per frequency bin follows Valin's double-talk rule (IEEE TASLP 15(3), 2007) with a -23 dB
leakage floor; during start-up the rule runs on the echo the detector expects. The reference
is read at a fractional position that slides at the measured drift; the drift comes from the
foreground filter's impulse-response peak, measured only right after an update (a filter
held during singing has not followed the drift), else from the correlation's delay line.

Each of these was a measured fix:

| Problem seen | Measured before | Fix | After |
|---|---|---|---|
| Per-hop fine delay (phase slope) too noisy: the filter restarted over and over (inherited code) | generated cable return: 17 restarts in 30 s, drift 59 ppm for 100, delay 10.9 ms for 10.1 | delay from the filter's own impulse response; restart only on 3 hops of disagreement > 2 ms | one restart in 94 USB and cable runs |
| Leakage floor -50 dB starved the step size | generated USB return at -6 dB: 25 dB from 6 to 14 s | floor -23 dB, step x2 (with the later changes) | 41 dB |
| The filter learnt the singer (start-up step 0.25 whatever the singer; 2.7 ms correlation bound) | USB -20 dB with singer: -4.6 dB (output worse than input) | start-up steps from the detector's expected echo; correlation bound over 40 ms | 26 dB |
| Foreground took the background's weights 60 times a second while the singer sang | cable 0 dB with singer: 6-24 dB swinging | foreground held while the input holds more than the expected echo | 20-25 dB |
| Drift line fed by a held filter (self-confirming) | drift sank from 98 to 25 ppm, 8 restarts | measure only freshly updated filters; correlation line as fallback | 99.4 ppm, 1 restart |
| Spikes where the filter's blocks join, and at its first tap (long low-frequency tail), taken for the echo | 2 restarts in most cable runs; "wandering" | peak search on 5-tap smoothed energy of the channels' summed response, edges left out, plausibility checks | 0 |

True suppression (from 15 s to 45 s), "Persephone":

| Return | Level | Singer | Suppression, true: min / median | Left in the output (worst) | Reported (median) | Quality |
|---|---|---|---|---|---|---|
| USB | -40 dB | no | 29.9 / 31.7 dB | -83.4 dBFS | 16.4 dB | good |
| USB | -30 dB | no | 37.3 / 39.0 dB | -80.8 dBFS | 19.8 dB | good |
| USB | -20 dB | no | 43.3 / 45.2 dB | -77.0 dBFS | 29.4 dB | good |
| USB | -10 dB | no | 49.5 / 50.2 dB | -72.9 dBFS | 39.2 dB | good |
| USB | 0 dB | no | 50.4 / 54.2 dB | -64.2 dBFS | 48.3 dB | good |
| USB | -40 dB | yes | 20.8 / 25.7 dB | -74.3 dBFS | 15.5 dB | good |
| USB | -30 dB | yes | 25.2 / 31.2 dB | -68.7 dBFS | 17.4 dB | good |
| USB | -20 dB | yes | 19.9 / 25.0 dB | -53.4 dBFS | 10.7 dB | good |
| USB | -10 dB | yes | 12.9 / 17.5 dB | -36.4 dBFS | 12.3 dB | marginal |
| USB | 0 dB | yes | 24.1 / 25.1 dB | -37.9 dBFS | 12.5 dB | unusable |
| cable | -30 dB | no | 31.5 / 32.2 dB | -73.6 dBFS | 19.0 dB | good |
| cable | -20 dB | no | 32.8 / 33.4 dB | -64.9 dBFS | 27.4 dB | marginal |
| cable | -10 dB | no | 33.2 / 33.8 dB | -55.4 dBFS | 32.1 dB | marginal |
| cable | 0 dB | no | 33.2 / 33.7 dB | -45.3 dBFS | 33.5 dB | unusable (before anybody sings: left-overs above the tracker's gate) |
| cable | -30 dB | yes | 23.0 / 23.6 dB | -65.1 dBFS | 16.3 dB | good |
| cable | -20 dB | yes | 9.0 / 12.8 dB | -41.1 dBFS | 13.0 dB | good, marginal |
| cable | -10 dB | yes | 16.4 / 20.8 dB | -38.5 dBFS | 12.8 dB | marginal |
| cable | 0 dB | yes | 24.3 / 25.1 dB | -36.4 dBFS | 13.0 dB | unusable |
| Bluetooth | -20, -10, 0 dB | no | 1.5-1.7 dB | -15.2 dBFS (0 dB) | 0.2 dB | unusable (24 of 24) |
| Bluetooth | -20, -10, 0 dB | yes | 0.8-1.5 dB | -14.9 dBFS (0 dB) | - | unusable |

What limits it:

- **Cable, about 33 dB**: the 24 ms filter cannot hold the long low-frequency tail of the
  coupling high-pass and bass shelf, and the drift compensation is good to about 1 ppm.
- **Singing over the return**: the foreground filter only learns in the singer's pauses, so
  while somebody sings the suppression is 13-25 dB (9 dB in the worst run, where the song
  changed during a phrase). For returns at or under -10 dB the left-overs are then 24 dB or
  more under the singer: the tracker gets 97-100 % right (section 6).
- **Bluetooth**: a delay that walks a few ms a second and jumps cannot be followed by a
  filter that needs a delay steady to a few microseconds at 4 kHz. It is measured, flagged
  and not removed.

**The reported suppression and the quality verdict** use the mean share of the echo left in
the output, measured on blocks whose input is all returned music. Against the truth (47 runs
each): without a singer the report is a median 10.9 dB under the true suppression (worst
10.1 dB over, one run); with a singer 11.4 dB under (worst 1.4 dB over). It errs on the side
of "less removed than really", so a "good" verdict can be believed. An earlier estimator (the
floor of that share) overstated the suppression by up to 20 dB in singer runs and was
dropped for the report (it is still what the per-block gate uses, with a 3 dB margin, where
it must not mistake a singer for left-overs).

Quality (`RemovalQuality`): `NotNeeded` (nothing returned), `Checking` (starting),
`Good` / `Marginal` / `Unusable` from the reported left-over level: once somebody has sung,
at least 25 dB / 15 dB under the singer's level; before that, at or under -60 / -48 dBFS
(the tracker's level gate is -55 dBFS); `Unusable` whenever the delay wanders.

## 5. The gate for the tracker (trap 6)

Every 2.7 ms block gets a verdict (`Trust`): `Clean` (no return), `Singer` (the output
holds more than the left-overs), `Music` (only left-over music: any pitch there is not the
singer's), `Unknown` (the first quarter second of music, the filter's start-up). A block
counts as the singer only when (a) the output is 6 dB above the expected left-overs (the
echo estimate times the measured share left, floor-tracked, plus 3 dB; the whole expected
echo plus 3 dB when the delay wanders) and the noise floor, and (b) the input holds more
than the echo expected (filter estimate or detector level) plus 1 dB. Rule (b) was added
after seeing brief bursts of left-over music shown as pitch when an instrument entered in a
band the filter had not learnt yet (up to 1.6 % of frames in a USB or cable run before,
0.00 % after).

Without a singer, music returned (pitch frames the app would show):

| Return | Without removal and gate (worst) | After removal (worst) | After removal and gate (worst) |
|---|---|---|---|
| USB, -40 to 0 dB, 2-300 ms, 3 songs (30 runs) | 18.4 % | 0.36 % | **0.00 %** |
| cable, -30 to 0 dB, +-100 ppm, 3 songs (10 runs) | 18.1 % | 1.6 % | **0.00 %** |
| guide vocal on, USB and cable, 3 songs (7 runs) | 21.1 % | 4.3 % | **0.02 %** (1 frame in 4,500) |
| Bluetooth, 3 songs (12 runs) | 17.8 % | 16.3 % | **0.27 %** |

With a singer, pitch shown in the 1.2 s rests between phrases: 0 % in 49 of 51 USB and
cable runs; 3.2 % and 4.8 % of rest frames in two cable runs at -20 dB (about 0.5 s, right
after the singer's last note, when the song changed while the filter was held).

## 6. The live tracker with a singer, guide vocal, breaks, nothing returned, other songs

Right pitch on steady notes (within 50 cents), "Persephone" (worst run of each group):

| Return | Level | Raw input | After removal | After removal + gate, from 15 s |
|---|---|---|---|---|
| none | - | 99.9 % | 99.9 % | 100.0 % |
| USB | -40 dB | 99.9 % | 99.9 % | 100.0 % |
| USB | -30 dB | 99.9 % | 99.9 % | 100.0 % |
| USB | -20 dB | 96.9 % | 99.4 % | 100.0 % |
| USB | -10 dB | 70.2 % | 97.1 % | 96.3 % |
| USB | 0 dB | 30.2 % | 97.4 % | 75.2 % (quality "unusable": warn) |
| cable | -30 dB | 99.9 % | 99.9 % | 100.0 % |
| cable | -20 dB | 93.8 % | 99.5 % | 100.0 % |
| cable | -10 dB | 66.5 % | 97.6 % | 97.0 % |
| cable | 0 dB | 30.1 % | 96.0 % | 79.6 % (warn) |
| Bluetooth | -20 dB | 97.1 % | 98.5 % | 0.0 % (warn; gate assumes no removal) |
| Bluetooth | 0 dB | 30.2 % | 43.1 % | 1.2 % (warn) |

(Over the whole 45 s instead of from 15 s, the gated figures are 2-20 points lower, median
11: the first seconds, before detection and while the filter starts, are `Unknown`, and
the output filter learns only in the singer's pauses.)

Guide vocal in the playback (trap 6), "Persephone":

| Scenario | Found after | Suppression true / reported | Quality | No singer: pitch shown raw / gated | Singer: right raw / removed / gated from 15 s |
|---|---|---|---|---|---|
| USB 0 dB, 10 ms | 0.84 s | 64.5 / 51.7 dB | good | 21.1 % / 0.00 % | - |
| USB -10 dB | 0.84 s | 56.6 / 43.1 dB | good | 21.1 % / 0.00 % | - |
| cable 0 dB, +100 ppm | 0.84 s | 32.4 / 29.3 dB | unusable | 18.8 % / 0.00 % | - |
| cable -20 dB, +100 ppm | 0.84 s | 31.8 / 27.0 dB | marginal | 18.7 % / 0.02 % | - |
| Bluetooth 0 dB, 200 ms | 1.26 s | 0.7 / 0.4 dB | unusable | 17.8 % / 0.27 % | - |
| USB 0 dB + singer | 0.84 s | 25.4 / 10.9 dB | unusable | - | 23.5 / 96.3 / 62.1 % |
| USB -10 dB + singer | 0.84 s | 31.6 / 16.5 dB | marginal | - | 60.0 / 97.6 / 99.0 % |
| cable 0 dB + singer | 0.84 s | 23.6 / 11.2 dB | unusable | - | 20.8 / 90.6 / 67.4 % |
| cable -20 dB + singer | 0.84 s | 28.6 / 15.4 dB | good | - | 93.6 / 99.9 / 100.0 % |
| Bluetooth 0 dB + singer | 1.26 s | 0.6 / - dB | unusable | - | 24.8 / 38.7 / 0.0 % |

A break in the music (the reference silent from 15 to 23 s): the canceller stays "present"
through it, no restart in the 4 runs, the delay stays put (USB -10 dB: 10.000 ms; cable
+100 ppm with guide: 11.4975 ms vs 11.5 ms truth after 45 s), and nothing is shown with
nobody singing (0.00 %). Unit test `silence_in_the_music_changes_nothing` checks the same on generated
music.

Nothing returned (singer only, guide on or off, 3 songs, 8 runs): "absent" after 3.21 s of
music, never "present"; with a singer, right pitch 99.9 % raw and after the canceller
(it passes the input through untouched: bit-exact, unit test
`absent_when_nothing_comes_back`), 100 % gated from 15 s (90.2 % over the whole run: the
first 3.2 s are `Unknown`).

Other songs ("There's Always a Better Day" and "Altar", 10 runs each): all returns found
within 0.84-3.77 s; USB -20 dB without singer 43.4 / 48.4 dB; cable 0 dB without singer
29.0 / 34.6 dB, drift 100.7 / 100.2 ppm; with a singer, cable -10 dB plus guide: 96.3 % /
99.7 % right after removal, 99.7 % / 100 % gated; cable 0 dB with a singer: 82.8 % / 97.1 %
after removal, "unusable" (warn). One Bluetooth run on "Altar" with a singer was declared
"absent" at 3.21 s and "present" at 3.77 s (0.56 s of `Clean` verdicts in between).
Every row: appendix.

## 7. Cost

| Rate | Session 1 | Session 2 (final code) |
|---|---|---|
| 16 kHz | 6.42 ms per s of audio (0.64 % of one core) | 5.49 ms (0.55 %) |
| 44.1 kHz | 17.37 ms (1.74 %) | 10.02 ms (1.00 %) |
| 48 kHz | 19.98 ms (2.00 %) | 8.53 ms (0.85 %) |
| 96 kHz | 30.63 ms (3.06 %) | 18.57 ms (1.86 %) |

60 s of generated music returned over a cable with a singer, 10 ms blocks, one thread,
best of 3 (`echo_eval --cpu`). Other workstreams were running on the PC in both sessions,
which explains the spread; in the 8-thread benchmark the per-scenario figure was a median
11.8 ms per s at 44.1 kHz. The tracker's own cost comes on top (see practice-tracker.md).
The canceller delays its output by `latency()`: 240 samples at 48 kHz (5.0 ms), to add to
the tracker's delay budget.

Allocations: `tests/echo_alloc.rs` counts 0 allocations and frees over 20 s at 48 kHz
(480-sample blocks), 44.1 kHz (441), 16 kHz (160) and 96 kHz (1024), including `report`,
`trust_window`, `measured_delay_s`, `measured_suppression_db` after every block, a music
break, a found return with drift, and `reset`. Everything is allocated in `new`.

## 7b. Unit tests (generated music, 48 kHz unless stated)

`cargo test -p jk-pitch --lib echo::` (9 tests, all passing; output kept in
`../jukeoke-build/practice-echo/unit-tests.txt`):

| Test | Checks | Measured |
|---|---|---|
| `absent_when_nothing_comes_back` | singer only: "absent" within 4 s, never "present", output bit-identical to the input delayed by `latency()`, verdict `Clean` | absent, z 5.8 |
| `usb_return_found_measured_and_removed` | USB -6 dB, 12.3 ms: present within 2 s, delay within 0.05 ms, level within 1 dB, stable, suppression > 30 dB, reported suppression not above the truth + 3 dB | 40.8 dB true, 28.5 dB reported |
| `cable_return_with_drift_is_tracked` | cable -3 dB, +100 ppm, 30 s: delay within 0.1 ms of the drifted truth, drift within 15 ppm, stable, suppression > 25 dB | 30.4 dB |
| `bluetooth_return_is_flagged_unusable` | present within 3 s, wandering, unusable | as checked |
| `singer_over_returned_music_is_trusted_and_kept` | cable -10 dB, -60 ppm, singer 6 dB above the return: stable, suppression > 15 dB, steady singing `Singer` in at least 90 % of 50 ms slots, rests `Music` in all | 21.7 dB; singer 25 of 26; rests 7 of 7 |
| `no_singer_with_guide_vocal_in_the_return_is_music` | trap 6: guide vocal 8 dB above the backing in the playback, USB 0 dB, nobody sings: the last 2 s are `Music`; the live tracker on the output reports no trusted pitch; the tracker on the raw input finds the guide's notes and the gate hides every one | output 0 pitch frames; raw input 66 pitch frames, 0 trusted (independent check: with the guide at the backing's level the tracker found no pitch even in the raw input, so the test could not fail) |
| `silence_in_the_music_changes_nothing` | 6 s silent break: stays present, delay within 0.05 ms, no restart | as checked |
| `other_rates` | 16 / 44.1 / 96 kHz: present, delay within 0.05 ms, level within 1 dB, suppression > 25 dB | 15.313 / 15.305 / 15.296 ms; 40.8 / 36.0 / 33.4 dB |
| `reset_forgets` | after `reset`: searching, not removing | as checked |

## 8. What the app does with it (for Stage 3)

- Feed the canceller the output mix the engine sends to the device (stereo, resampled to the
  input's rate) and the voice channel, sample-aligned to the best of the time stamps,
  continuously (zeros for lost input samples); call `reset` on a device change. Run the
  tracker on the canceller's output.
- `report().presence == Present`: show the warning (the mixer is sending the music back, what
  it does to the marker and the grade, how to stop it: loopback off, or the USB wiring of
  WO2 4.10). Arrives within 0.84-0.98 s at every level without a singer, at most 4.6 s with
  one.
- `report().quality`: `Good` grade normally and mark the result "music came back, removed";
  `Marginal` grade and mark it less certain; `Unusable` warn instead of grading (always for
  Bluetooth: "practice needs the cable or USB"; for a return louder than the singer: "turn
  the music down in the mixer's USB feed").
- Per pitch event: `trust_window(t - latency - hop/2, t - latency + hop/2)`; show the marker
  and score only for `Clean` or `Singer`; treat `Music` and `Unknown` as no pitch (the
  measurements above do exactly this).
- Device delay for the scorer ("the delay comes for free"): `measured_delay_s()` while
  present (USB and cable: within 0.19 ms; Bluetooth: within about 10 ms of a wandering delay,
  and `stability == Wandering`), added to the part the time stamps already account for.

## 9. What this does not show

- No real mixer, cable, Bluetooth link or microphone: the returns are models. The AME2's
  real USB feed, loopback switch and effects were not checked (the manual reading belongs to
  docs/MAC.md, Stage 6). A real analog path may add distortion, which a linear filter cannot
  remove; the cable figures assume a linear path.
- One synthetic male singer, not a person; the grade (scorer) does not exist yet, so the
  section 9 row "how far each scripted singer's grade is from its clean grade" is not
  measured.
- Three songs, 45 s each; the Bluetooth model is a guess at how such links wander.
- The tracker under the canceller is the working-tree tracker of the tracker-hardening
  workstream as of this run; its numbers may move when that work lands.
- The reported suppression is conservative by about 11 dB in the median (section 4); the
  per-block gate's floor estimate can be optimistic after the song changes during a phrase
  (two runs showed about 0.5 s of left-over pitch in a rest, section 5).

### Independent check (verifier, same day)

Re-run of the benchmark on the final code: every echo field (detection, delay, level,
drift, stability, suppression, quality) identical in 126 of 126 rows; tracker-based fields
moved by up to 3.5 points because the tracker-hardening workstream changed the tracker in the
meantime. The same benchmark on **9 songs not used to develop the canceller** (ccm-39381 in
full, ccm-29611, -29878, -30378, -32665, -44399, -59840, -67424, -68969 with the 10-scenario
subset; 186 scenarios; results in `../jukeoke-build/practice-echo/verify/holdout/`):

- Every return found (166 of 166), no false alarm (20 of 20), USB and cable all "stable",
  Bluetooth all "wandering" and "unusable"; with no singer, trusted pitch 0.00 % (USB) and at
  most 0.09 % (cable). Found within 0.84-4.6 s of the music starting (one excerpt starts with
  10 s of silence).
- Worse than the figures above: delay error worst **1.0 ms** (two USB runs with a singer;
  0.19 ms above); Bluetooth with nobody singing up to **2.0 %** trusted pitch frames (0.27 %
  above); returns as loud as the music with a singer, gated accuracy median **53 %**, worst 37 %
  (quality "unusable"); one cable run at -10 dB with guide and singer showed pitch in **6.6 %**
  of rest frames while quality said "marginal". USB with a singer: reported suppression median
  11.7 dB for a true 30.2 dB.

Cases the benchmark does not cover (generated singer over ccm-39381 and ccm-59840, script kept
in `../jukeoke-build/practice-echo/verify/echo_verify.rs.txt`):

| Case | Measured |
|---|---|
| Loopback switched **off** at 20 s (what the warning tells the owner to do), USB 0 dB, nobody sings | still "present" at 45 s on both songs (25 s later); output clean and every frame `Music`, so the marker stays off, but the warning stays up. The detector's averages are normalised, so a vanished echo is forgotten only as fast as new input replaces it |
| The same with a singer: USB -20 dB / USB 0 dB with guide | "absent" 12-15 s / 19-22 s after; at 0 dB, for 4 s the output carries -26 to -36 dBFS of re-injected music and only 3-33 % of the singer's frames are shown |
| Loopback switched **on** at 20 s | found 0.6-0.8 s later |
| A high voice (C4-G5) singing "ee" or "oo", USB or cable -10 dB | gated accuracy **59-70 %** (tracker after removal 100 %), quality "marginal", so the app would grade. The same notes on "ah", or "ee" an octave and more lower: 99-100 % |
| Continuous singing (0.1 s breaths, no rests) from the first second, USB -10 dB | `Unknown` for all 45 s, quality "checking": no marker and no grade for the whole song. With an 8 s instrumental intro: 100 % |
| Singer 6 dB louder than the music (-8 dBFS) over a 0 dB return | 100 % gated, "marginal": the 0 dB "unusable" verdicts above come from the singer's level |
| Return 6 dB above the playback; only the left channel returned | 64 dB and 41 dB removed, no pitch shown with nobody singing |
| Breathy voice (aspiration 10 dB under the voice), USB -10 dB | 100 % gated |
| Singer along with the guide (12 or 3 cents sharp), nothing returned | "absent", 100 % gated. (A generated singer on exactly the guide's pitch track is a filtered copy of the guide, since the synthetic voice has no jitter, and is found as a return: never use one pitch track for both in tests.) |
| Onsets in the good cases | gated marker within 5 ms of the clean tracker's |

The first, fourth and fifth rows are what Stage 3 worked on: section 11.1 (loopback off), 11.2
(high voices) and 11.3 (continuous singing).

## 10. Stage 3: what changed in the canceller (2026-11-19, evening)

The independent verifier's three gaps (section "Independent check" below: "present" lingering
after the loopback is switched off, high "ee"/"oo" losing frames to the gate, continuous
singing never getting a verdict) and the changes of the echo path a session meets. Code:
`src/echo.rs`, `src/echo/spectral.rs` (new), `src/echo/detect.rs`, `src/echo/filter.rs`,
`src/echo/refline.rs`. New measurement tool: `examples/echo_cases.rs` (grown from the
verifier's script; cases on generated voices over the Creative Commons songs) with
`echo_cases_report.py`, and `echo_eval_compare.py` for before/after tables of the scenario
benchmark. Everything stays pure DSP with all memory allocated in `new`.

| Change | Measured before | What it does |
|---|---|---|
| **Echo path gain tracker** | "present" lingered 12-25 s after the loopback went off (the detector's GCC-SCOT averages are scale-invariant: a vanished echo is forgotten only as fast as new input outweighs it) | In 21 ms chunks, input x the filter's echo estimate against the estimate squared; over the last 0.5 s their ratio is the path's gain relative to what the filter learnt (1 while unchanged, 0 once gone; a singer adds noise, not bias; the chunks' spread gives the standard error). Gain under 0.25 with 3 standard errors to spare: removal is suspended and a fresh detector must confirm the echo within 2 s, else "absent". Input 25 dB under the expected echo, or 10 dB under it and down at the noise floor, 4 s or more after the filter started: "absent" at once. Gain under 0.5: the input passes untouched (subtracting would add music) |
| **Detector forgetting** (`forget_echo`) | as above | Clears the correlation averages, keeping the signal history, so an echo that is still there (or comes back) is found as fast as at the start; "present" kept on probation while the canceller decides; the long average then needs 3 s before it may re-declare |
| **Spectral look** ("tier 2" of the gate, `spectral.rs`) | a high "ee"/"oo" lost about a third of its frames to the gate while quality said "marginal" (verifier: 59-70 % gated, 100 % after removal): the per-block level comparison cannot see a voice quieter than the music it sits in | Every ~10 ms over 43 ms (60 Hz-4 kHz): output energy standing more than 6 dB above the most music there can be in its frequency bin (the filter's estimate, or the detector's level x the reference) cannot be music; if it is half the output energy or more, and above the noise floor, the frame is the singer and the blocks around its centre that the level comparison called Music or Unknown become Singer. Uses the singer's harmonic structure and the echo-path estimate, not level ratios, and works from the moment the delay is known. With nobody singing the output never exceeds the bound (a misadjusted filter can at most double the amplitude); the guide vocal is part of the reference, so it is bounded too (trap 6). Not used while the delay wanders |
| **Left-over share from a regression** (`spectral.rs`) | continuous singing from the first second: "checking" for all 45 s, every frame Unknown (the left-overs were only measured on stretches of music without singing) | The regression of the output's per-bin power fluctuations on the reference's (after Valin's leakage estimate; bands the filter has not learnt count whole) gives the share of the echo left during singing, with a 6 dB margin; used by the gate and the report until a stretch without singing exists |
| **Background filter held while the spectral look sees the singer** | continuous singing: the output filter took weights 2.3 times the echo (a voice holding a note over the same note in the music was slowly learnt as if the music predicted it) | As said |
| **A path that got louder** (OS volume, mixer return level) | turned up 6 dB with nobody singing: 2.6 % of the frames shown as pitch in the 5 s after | The expected echo follows g^2 (half-second gain 2 standard errors over 1, or the last 84 ms 3 standard errors over 1.4); the left-over (g-1)^2; for 0.5 s after the last such sign the level comparison may not call a block the singer (the spectral look, whose bound holds up to about 9 dB louder, still can) |
| **Wandering delay** (Bluetooth) | the gate compared with the echo at the filter's delay only; with the spectral regression the Stage 3 code showed 1.31 % pitch frames with nobody singing (Stage 2: 0.27 %) | The echo bound is the detector's level x the largest reference block power within +-40 ms of the filter's position, 6 dB over it (was 3 dB): 0.27 % again on the benchmark, 1.80 % on the held-out songs (Stage 2: 2.02 %) |

## 11. Stage 3 results

Final code; results in `../jukeoke-build/practice-echo/stage3/v14/` (`bench/`, `holdout/`,
`cases.jsonl`, `cases-verifier-songs/`, `cpu.jsonl`, `unit-tests.txt`, `echo_alloc.txt`);
the intermediate versions v1-v13 beside it. "Stage 2 code" figures: the scenario benchmark
re-run with the committed Stage 2 canceller (`stage3/bench-base`, `stage3/holdout-base`) and
the cases on ccm-22364 (`stage3/baseline`). The cases use the developing songs ccm-22364,
ccm-31416, ccm-55540 and, held out, the verifier's two songs ccm-39381 and ccm-59840.
Tracker-based figures use the live tracker as it stood in the working tree at each run (the
tracker workstream was changing it the same evening), so they can move by a point or two
between runs with the canceller unchanged.

### 11.1 The loopback switched off at 20 s (what the warning tells the owner to do)

| Case (USB return at 10 ms unless "cable") | "present" cleared after, s (3 songs) | the 2 held-out songs | Singer's frames right, 5 s after the switch: after removal / shown (worst song) | Stage 2 code (ccm-22364) |
|---|---|---|---|---|
| 0 dB, nobody sings | 0.5 / 0.5 / 0.5 | 0.5 / 0.5 | - (0.00 % pitch shown) | never (still present at 45 s) |
| 0 dB + guide vocal, nobody sings | 0.5 / 0.5 / 0.5 | 0.5 / 0.5 | - (0.00 %) | never |
| -20 dB, nobody sings | 0.5 / 0.5 / 0.5 | 0.5 / 0.5 | - (0.00 %) | never |
| -40 dB, nobody sings | 0.5 / 0.5 / 0.5 | 0.5 / 0.5 | - (0.00 %) | never |
| cable 0 dB, +100 ppm, nobody sings | 0.5 / 0.5 / 0.5 | 0.5 / 0.5 | - (0.00 %) | never |
| -20 dB, singer | 3.1 / 3.2 / 3.2 | 3.2 / 3.1 | 95.1 / 95.1 % (held out 100 / 100 %) | 10.8 s; 95.1 % |
| -10 dB, singer | 2.6 / 3.1 / 3.1 | 3.2 / 3.1 | 97.9 / 97.9 % (100 / 100 %) | 21.9 s; 91.5 % |
| 0 dB + guide, singer along | 3.2 / 2.6 / 3.3 | 2.6 / 2.6 | 98.9 / 95.0 % (89.6 / 86.0 %) | 22.5 s; 48.4 % |
| cable 0 dB, singer | 2.6 / 3.1 / 2.6 | 2.6 / 2.6 | 93.6 / 86.2 % (88.0 / 86.9 %) | 22.5 s; 56.2 % |

With nobody singing the input falls to the noise floor and "absent" comes after 0.5 s; with
a singer the gain test suspends removal, the fresh detector finds nothing within 2 s and
"absent" follows 2.6-3.3 s after the switch. Meanwhile the loud return no longer hides the
singer: shown frames within 0-8 points of what the tracker gets after removal (Stage 2: 48
and 56 % shown at 0 dB). Switched back **on** at 20 s: found again after 0.6-1.2 s (5 songs).

### 11.2 A high voice (C4-G5) singing "ee" and "oo"

Right pitch on steady notes from 15 s (gated = what the app would show and grade); quality
letters G / M / U per song (good / marginal / unusable: the app warns instead of grading).

| Case | After removal (worst song) | Gated: worst / median of 3 songs | Quality | Held out (2 songs): gated worst / median, quality | Stage 2 code (ccm-22364): gated, quality |
|---|---|---|---|---|---|
| "ee", USB -30 dB | 100.0 % | 99.0 / 100.0 % | G G G | 100 / 100 %, G G | 99.3 %, good |
| "ee", USB -20 dB | 100.0 % | 99.5 / 100.0 % | G G G | 99.5 / 100 %, G G | 97.9 %, good |
| "ee", cable -20 dB | 97.2 % | 96.1 / 97.4 % | G M G | 99.8 / 100 %, M G | 97.6 %, good |
| "ee", USB -10 dB | 98.7 % | 89.3 / 98.3 % | M U M | 83.4 / 97.4 %, M U | 91.3 %, marginal |
| "ee", cable -10 dB | 92.6 % | 92.3 / 93.0 % | M M M | 85.0 / 96.4 %, M U | 90.3 %, marginal |
| "oo", USB -20 dB | 100.0 % | 98.7 / 100.0 % | M G G | 99.6 / 100 %, G G | 98.4 %, good |
| "oo", cable -20 dB | 95.0 % | 95.0 / 98.2 % | G G G | 100 / 100 %, G G | 98.0 %, good |
| "oo", USB -10 dB | 98.2 % | 90.8 / 99.7 % | M U M | 83.2 / 98.0 %, M U | 94.1 %, marginal |
| "oo", cable -10 dB | 90.3 % | 84.6 / 99.7 % | M U M | 85.0 / 98.3 %, M U | 90.6 %, marginal |
| "ee" / "oo", USB or cable 0 dB | 68-96 % | 27-64 / 76-93 % | U everywhere | 52-56 / 73-75 %, U | 44-59 %, unusable |
| "ah" USB -10 dB; male "ee" USB -10 dB | 100 % | 98.3 / 100 %; 96.8 / 99.8 % | M M M; M U M | 100 / 100 %; 97.9 / 99.8 % | 96.2 %; 96.5 % |

Frames the gate takes away while quality says good or marginal (after removal minus gated):
at most **6.6 points** on the developing songs (one run: "ee", cable -10 dB, ccm-31416) and at
most **16.8 points** on the held-out ccm-39381 ("oo" / "ee" at -10 dB, quality marginal; the
other held-out song loses at most 2.1 points), against about a third (30-41 points) in the
verifier's Stage 2 runs. So the gap is closed on four of five songs and narrowed on the fifth:
on ccm-39381 a high note under a -10 dB return is still judged music in about one frame in
six while quality says marginal (open item, section 11.7).

### 11.3 Continuous singing from the first second (0.1 s breaths, no rests)

| Case | First verdict, s (3 songs) | Held out (2 songs) | After removal (worst) | Gated from 15 s: worst / median | Gated, whole song (worst) | Quality | Stage 2 code (ccm-22364): first verdict / gated / quality |
|---|---|---|---|---|---|---|---|
| USB -30 dB | 5.9 / 3.9 / 7.6 | 2.4 / 5.1 | 100.0 % | 100.0 / 100.0 % | 90.8 % | M M M | never / 0 % / checking |
| USB -20 dB | 3.4 / 2.9 / 2.4 | 2.4 / 2.4 | 97.3 % | 97.3 / 97.8 % | 92.3 % | M U U | never / 0 % / checking |
| USB -10 dB | 2.5 / 2.7 / 2.4 | 2.4 / 2.4 | 94.9 % | 93.0 / 94.3 % | 89.4 % | U U M | 25.6 / 58 % / unusable |
| USB 0 dB | 2.4 / 2.4 / 2.4 | 2.4 / 2.4 | 92.4 % | 86.5 / 88.0 % | 84.9 % | U U U | 3.7 / 60 % / unusable |
| cable -10 dB | 2.7 / 2.7 / 2.4 | 2.4 / 2.4 | 92.0 % | 90.0 / 90.5 % | 86.8 % | U U M | 8.5 / 74 % / unusable |
| cable 0 dB | 2.4 / 2.4 / 2.0 | 2.4 / 2.4 | 89.5 % | 84.1 / 88.4 % | 80.8 % | U U U | 2.5 / 58 % / unusable |
| "ee", USB -10 dB | 2.5 / 2.5 / 2.4 | 2.4 / 2.4 | 86.9 % | 76.5 / 87.9 % | 71.0 % | U U M | 37.4 / 23 % / unusable |
| control: 8 s intro first, USB -10 dB | 1.4 / 1.4 / 1.4 | 1.4 / 1.4 | 100.0 % | 99.6 / 99.8 % | 99.7 % | M M G | 1.4 / 98 % / marginal |

Every case now gets a verdict within 2.0-7.6 s and a working gate: where quality says good
or marginal the gate takes at most 0.9 points of the singer's frames; where it says unusable
up to 10 points (27 on held-out ccm-39381 at 0 dB, where the app warns). The verdicts are
cautious: with nobody ever silent the filter learns only in the breaths (true suppression
-3 to 25 dB on the developing songs, -9 to 26 dB held out), and the regression's left-over
share carries a 6 dB margin, so a -20 dB return with continuous singing is judged "unusable"
on two songs although 97 % of the frames are right (the app would warn instead of grading;
the first rest the singer takes settles it). At -30 dB the filter, learning beside a singer
26 dB louder, can make the music left in the output louder than the return itself (by up to
9 dB, held out): still 17 dB or more under the singer, 99.8-100 % of the frames right.

### 11.4 Changes of the echo path, and the master volume (at 20 s, 10 ms ramp)

| Case | "present" at the end; filter lost / restarts | Pitch shown with nobody singing, 5 s after | Singer shown, 5 s after (worst) | True suppression from 15 s (worst) | Quality |
|---|---|---|---|---|---|
| OS / mixer return turned down 6 dB (USB -10 dB) | present; 0 / 0 | 0.00 % | 83.4 % (held out 100 %) | 24.2 dB (no singer) | G G G |
| turned up 6 dB | present; 0 / 0 | **0.00 %** (Stage 3 before this fix: 2.6 %) | 49.8 % cable (held out 75.6 %) | 23.4 dB | G M G |
| turned down 20 dB (USB 0 dB) | present; **0 / 0** | **0.00 %** (with the plain -10 dB "gone" rule: taken for "off", 3.8 %) | - | 16.9 dB | G G G |
| turned down 12 dB | present; 0 / 0 | 0.00 % | 94.7 % | 18.2 dB | G G G |
| **app master volume -12 dB** (reference and return change together: the output tap is after the master volume) | present; 0 / 0 | 0.00 % | **100.0 %** | **41.2 dB** (held out 49.7 dB) | G G G |

Turned up, the singer is hidden for up to about a second while the filter re-learns the path
(the level comparison waits 0.5 s after the last sign of the change; the spectral look keeps
most notes): 50 % of the singer's frames in the 5 s after on the worst song (76 % held out),
91-99 % from 15 s on.

### 11.5 The benchmark and the held-out songs again (126 + 186 scenarios)

Stage 2 code -> final code, same scenarios (`echo_eval_compare.py`):

| Group | Figure | Benchmark (3 songs) | Held out (9 songs) |
|---|---|---|---|
| all returned (118 / 166) | found; median / slowest | 118 of 118 -> 118 of 118; 0.84 / 4.60 s, unchanged | 166 of 166 -> 166 of 166; unchanged |
| nothing returned (8 / 20) | false "present" | 0 -> 0 | 0 -> 0 |
| USB, cable | delay error, worst | 0.193 -> 0.192 ms (cable, singer); USB 0.009 -> 0.012 ms | 1.007 -> 0.924 ms (USB, singer) |
| USB, cable | stability | all stable, 1 restart in 94 runs, unchanged | all stable; restarts 1 -> 0 |
| USB, no singer | true suppression min / median | 29.9 / 45.9 dB, unchanged | 33.9 / 49.2 dB, unchanged |
| cable, no singer | true suppression min / median | 29.0 / 33.2 dB, unchanged | 28.8 / 33.0 dB, unchanged |
| USB, singer | true suppression min / median | 11.7 / 25.5 -> 11.4 / 26.5 dB | 12.6 / 30.2 -> 14.7 / 32.9 dB |
| cable, singer | true suppression min / median | 9.0 / 24.3 -> 12.8 / 25.2 dB | 9.8 / 23.5 -> 9.4 / 26.2 dB |
| USB / cable, no singer | pitch shown, worst run | 0.00 / 0.02 % -> 0.00 / 0.02 % | 0.00 / 0.09 % -> 0.00 / 0.02 % |
| USB, singer | gated from 15 s, worst / median | 62.1 / 100 -> **96.5** / 100 % | 36.9 / 100 -> **68.4** / 100 % |
| USB, singer | gated whole run, worst / median | 56.7 / 88.3 -> 90.2 / 93.9 % | 35.4 / 89.4 -> 71.0 / 97.3 % |
| cable, singer | gated from 15 s, worst / median | 45.4 / 97.6 -> **81.9** / 99.7 % | 36.7 / 94.7 -> **75.7** / 99.0 % |
| cable, singer | pitch shown in the rests, worst run | 4.76 -> 2.52 % | 6.63 -> **0.00** % |
| Bluetooth | quality | unusable 24 of 24, unchanged | unusable 36 of 36, unchanged |
| Bluetooth, no singer | pitch shown, worst run | 0.27 -> 0.27 % | 2.02 -> 1.80 % |
| Bluetooth, singer | gated from 15 s, median | 0.0 -> 25.5 % | 0.7 -> 0.1 % |
| quality verdicts that changed | | 2 of 126 (one cable -20 dB singer run marginal -> good, one USB -20 dB 300 ms singer run good -> marginal) | 2 of 186 (one unusable -> marginal, one marginal -> good) |

The worst gated figures are the returns as loud as the music with a singer, where quality is
"unusable" and the app warns.

### 11.6 Cost and allocation

| Rate | ms per second of audio (best of 3, 60 s, cable return with a singer) |
|---|---|
| 16 kHz | 3.37 (0.34 % of one core) |
| 44.1 kHz | 6.47 (0.65 %) |
| 48 kHz | 6.98 (0.70 %) |
| 96 kHz | 13.33 (1.33 %) |

The spectral look adds three 2,048-point real FFTs (48 kHz) every four blocks; the run
was faster than Stage 2's figures (8.5 ms at 48 kHz) because Stage 2 measured while other
workstreams loaded the PC (they were running this time as well; treat both as "under 1 %
of a core at 48 kHz"). `tests/echo_alloc.rs`: 0 allocations at 16 / 44.1 / 48 / 96 kHz
as before, plus a new run in which a 0 dB USB return is switched off at 15 s under a singer
(gain test, suspension, fresh detector, "absent") and a wandering Bluetooth return (the
wider bound): 4 of 4 tests pass. Unit tests: 9 of 9 (`echo::`).

### 11.7 What Stage 3 does not fix

- **Bluetooth**: up to 1.80 % of the frames show a pitch with nobody singing on the held-out
  songs (quality is always "unusable"). Proposal for the app: show no marker while
  `stability == Wandering` (APP_API 12.5 now says warning + marker + no grade).
- **High notes under a -10 dB return on one held-out song** (ccm-39381): 15-17 points of the
  singer's frames judged music while quality says marginal (section 11.2).
- **Continuous singing**: verdicts are cautious ("unusable" for a -20 dB return on two of
  three songs) because the filter can only learn in the breaths, and a weak return (-30 dB)
  can end up up to 9 dB louder in the output than in the input, still far under the singer
  (section 11.3).
- A generated voice that holds exactly a tone of the generated music is a filtered copy of
  it: after the loopback went off it was found again as a "return" (z 130) in a unit-test
  setup; the tests now sing with vibrato like a person. Real voices were not available.
- Everything is still generated or Creative Commons audio through modelled returns: no
  real mixer, microphone or person (section 9).

### 11.8 Independent check of Stage 3 (verifier, same evening)

Re-runs on the final code reproduce the figures above: `echo::` 9/9, `echo_alloc` 4/4,
`tap::` 5/5, `output_tap` 2/2 (with `JK_TAP_REAL_DEVICE=1` too: Realtek, median 0.099 ms,
worst 0.342 ms), `rt_alloc` 0 on worker and device in every phase (96,480 tap frames read,
0 lost); the benchmark and the 9 held-out songs give every echo field identical to
`stage3/v14` (126 + 186 rows; tracker fields within 2 points); the cases on ccm-31416 and
ccm-39381 identical but for tracker decimals; 6.97 ms per second at 48 kHz. Results in
`../jukeoke-build/practice-echo/verify2/`.

**Five songs nobody had used for echo work** (ccm-30483, -33628, -34740, -60767, -68853:
`echo_cases`, 245 cases, and `echo_eval --main ccm-34740 --others` the other four, 146
scenarios), each also run with the committed Stage 2 canceller (a `git archive HEAD` copy
with the same `echo_cases.rs`: `verify2/fresh-s2`, `verify2/fresh-bench-s2`):

| Case | Stage 2 code | Stage 3 code |
|---|---|---|
| Loopback off, nobody sings: "present" cleared | never in 23 of 25; 20-24 s in 2 | 0.44-2.3 s in 25 of 25; 0.00 % pitch shown |
| Loopback off, singer: cleared | 12.3-23.1 s (one never) | 2.6-3.3 s on 4 songs; on ccm-34740 "absent" after about 2.5 s, then a false "present" from 33.0 to 37.8 s with only the singer there (the same in Stage 2; that song's nothing-returned benchmark scenario with a singer is declared present at 32.88 s by both) |
| Loopback switched on, nobody sings: pitch shown in the 5 s after (before it is found) | 6.6 % on ccm-34740, 0 elsewhere | the same (and 2.6 % on ccm-22364 in `stage3/v14`) |
| High "ee"/"oo" and male "ee": largest gate loss while quality says good or marginal, per song | 12-37 points | 6.2 / 8.3 / 14.5 / 18.1 / 20.3 points (the last: male "ee", ccm-33628; its high notes 17.0) |
| Continuous singing: first verdict | never in 16 of 35 cases | 2.4-8.4 s in 35 of 35 |
| Continuous singing: largest gate loss while good or marginal | - | 17.6 points (ccm-33628, USB -10 dB, marginal); at most 5.8 on the other four |
| Path changes, nobody sings: pitch shown | 0.00 % | 0.00 % (25 of 25) |
| Path turned down 6 / 12 dB, singer: true suppression from 15 s | 10.6-22.0 dB | 1.2-4.4 dB lower on 4 of 5 songs (the background is held while the singer is seen); shown frames the same or better |
| Benchmark: found / false "present" | 134 of 134 / 1 of 12 | the same (the ccm-34740 run above) |
| Benchmark: gated from 15 s with a singer, worst USB / cable | 78.3 / 56.4 % | 82.0 / 81.8 % |
| Benchmark: Bluetooth, nobody sings, pitch shown (worst) | 1.07 % | 0.40 % |
| Benchmark: fixed-delay USB returns judged "wandering" (quality unusable) | 5 (ccm-34740, -30/-40 dB, singer) | 6 |

So the three gaps are much narrower on new songs too, but not closed as far as sections
11.2 and 11.3 suggest: on 3 of the 5 new songs a high note under a -10 dB return still
loses 14-20 points (about one frame in six) while quality says marginal, and on ccm-33628 a
male voice over a USB -10 dB return loses 16-17 points while quality says good (Stage 2:
13 points). Music-flagged frames are neither credited nor charged by the scorer
(`practice.rs`), so this costs the marker, not the grade.

- **Held-out tuning.** `WANDER_BIAS` (3 -> 6 dB) was chosen after the held-out figure was
  seen: from v12 to v13 the benchmark's Bluetooth no-singer figure stayed 0.27 % while the
  held-out one fell from 2.56 to 1.73 %. The held-out Bluetooth figure (1.80 %) is not an
  independent measurement; on the five new songs it is 0.40 %.
- **The "no marker while wandering" proposal** (11.7) would hide the marker for whole songs
  in the six ccm-34740 runs above, where a fixed USB return at -30/-40 dB under a singer is
  judged "wandering" (82-94 % of the singer's frames shown today). Fix that verdict first,
  or apply the rule only to returns loud enough to move the marker.
- **The gate's window.** The app's chain asks `trust_window` for the whole tracker window
  (`practice.rs`, +-window/2), the harnesses for +-hop/2. With `JK_CASES_TRUST=window`
  (opt-in in `echo_cases.rs`) on ccm-31416, -39381, -30483, -60767 (144 cases with a
  singer): 0.2 points fewer frames on average, at most 2.2. The harness figures stand for
  the app.
- A held-out Bluetooth 0 dB run with nobody singing (ccm-29611) now drops to "absent" from
  3.91 to 5.44 s (Stage 2: never), so the warning flickers once there.

## 12. The engine's output tap (Stage 3)

What the canceller needs as its reference is exactly what the app sent to the device, with
the host time each frame left the DAC. Code: `crates/jk-engine/src/tap.rs` (new), one block
in `Filler::fill` (`src/output/mod.rs`), `Engine::output_tap()` (`src/api.rs`).

**Where the copy is taken.** In the device filler, at the very end of each device callback:
after the graph, the key shift, the output resampler, the seek/pause fades **and the master
volume**, i.e. the `f32` frames handed to the device (first two channels; a mono device is
duplicated). After the tap come only the conversion to the device's sample format (i16 /
i24 / i32, clamped to full scale), the operating system's endpoint volume, the device and the
mixer: for the canceller those are part of the echo path.

**Master volume is before the tap.** Turning the app's volume down scales the reference and
the returned music alike, so the echo path the canceller learnt does not change. Measured
(section 11.4, "master-12dB" vs "path-12dB"): with the tap after the master volume a 12 dB
master change costs nothing (no restart, true suppression after it 41-49 dB with nobody
singing, the same as without the change; the singer's frames 100 % shown); a tap before
the master volume would have made the same change look like the path turning 12 dB quieter
(true suppression 18-23 dB while the filter re-learns). The OS volume or the mixer's
USB-return level does change the path; the canceller follows it (section 11.4).

**Time stamps.** Each callback block is recorded with an anchor: its first frame's index in
the tap stream, its length, the device rate, the song generation and position, and `dac_ns`
= the callback's start time (`now_ns`, the engine's QPC / `mach_absolute_time` clock) plus
the frames the device still holds ahead of the block (the latency the playback clock already
uses). `TapReader::read` returns each piece with the stamp of its first frame and the frames
lost before it (a reader more than the ring behind).

**Real-time safety, cost.** A sequence-checked "overwrite the oldest" ring of 2^18 stereo
frames (5.5 s at 48 kHz, 2 MiB) plus 2,048 anchors, allocated on the first `output_tap()`
call (control thread) and kept for the engine's life; the callback writes it with plain
atomic stores (no allocation, no lock, never waits, never fails); a reader checks after
copying that the writer has not lapped it. With no reader attached the callback does two
atomic loads (the `OnceLock` holding the tap and the reader count; one before the first
attach). The render worker is untouched; one
writer at a time (the engine joins a device's thread before it opens the next).

| Check | Measured | Where |
|---|---|---|
| Tap = what the device played | Null device with capture, a reader on its own thread over 3.7 s with a master change (0.5 -> 0.2), a seek with its fades and a pause: **180,480 frames identical bit for bit**, none lost | `tests/output_tap.rs` `tap_is_exactly_what_the_device_played` |
| Playback unchanged | Device output with and without a reader: **identical over 288,000 frames** (fast capture) | `no_reader_no_copy_and_playback_unchanged` |
| No reader, no copy | Tap frames written: 9,600 while a reader was attached, **0** in the 30 ms after it left | same |
| Stamps agree with the playback clock | Song position the tap gives at a DAC time vs the clock's at the same time: null device (same callback) median 0.003 ms, worst 0.037 ms; real device (Realtek speakers, WASAPI shared, 48 kHz, 22 ms latency, master 0.001 = -60 dB) median **0.10 ms**, worst 0.45 ms (842 comparisons; the tap stamps the callback's start, the clock its end); stamp steps minus the frame duration: median -0.003 ms, 99th percentile 0.23 ms | `JK_TAP_REAL_DEVICE=1 ... output_tap` |
| No allocation on the callback or the worker | `rt_alloc`: new phases "output tap read (2 s, seek, 2 master)" and "output tap detached": worker **0/0/0/0**, device **0/0/0/0** (Rust alloc / free, C alloc / free); 96,960 frames read on the analysis thread, 0 lost; every earlier phase still 0 | `tests/rt_alloc.rs` |
| Cost on the callback | **0.65 us** per 10 ms stereo block (480 frames at 48 kHz, release build), i.e. under 0.01 % of the callback period (verifier's re-run under load from other work: 2.02 us); with no reader attached two atomic loads | `tap::tests::cost_of_one_callback_block` |
| Reader correctness | 4 unit tests: new readers start at the newest frame, pieces stop at block ends with exact stamps, a reader far behind is told what it lost, and a concurrent writer and reader (20,000 blocks) never tear a frame | `cargo test -p jk-engine --lib tap::` |
| End to end on real hardware | `examples/tap_loopback.rs`: the engine plays at -60 dB, the tap is read with its stamps, the recorder's capture code (the microphone path, `jk_recorder::input`) records the same output's WASAPI loopback with its QPC stamps; lined up once by the stamps (reference 50 ms early), then fed continuously to the canceller: found after **0.77 s**, delay **21.041 / 21.208 / 21.166 ms** in three runs (brute-force cross-correlation: the same to 0.001 ms), drift -0.0 to -0.1 ppm, "stable", 22-31 dB removed. So the two clocks meet (offset read once) and the residual the stamps leave is steady to 0.2 ms: here -28.8 to -29.0 ms, because Windows takes the loopback copy at its mixer, before the 22 ms of output latency the engine counts to the DAC (plus about 7 ms). Through a real mixer the return comes after the DAC, so the residual will be positive: that is the device delay "for free" (WO2 4.10) | `$JK_BUILD/practice-echo/stage3/tap/loopback*.txt` |

**For the app core (practice session, next stage).** Read the tap on the analysis thread
next to the input rings. Line the two streams up once by their stamps (the reference frame
whose `dac_ns` + the clock offset equals the capture stamp of the first input frame, minus a
50 ms lead), then feed both continuously, resampled from the device rate to the input rate at
the nominal ratio: the canceller measures the residual delay and the drift itself and must
not be re-anchored per block. On `lost_before > 0`, a rate change, a device change or an input
discontinuity: `reset()` the canceller and line up again. The ARCHITECTURE section 12.4 text
("one extra lock-free push ... on the render worker; the device callback is unchanged")
describes the plan; the tap is in the device callback instead, because only there are the
fades and the master volume applied and the DAC time known, and frames the worker renders
ahead may be discarded by a seek.

## How to reproduce

```text
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-wo2-echo"
cargo test --release -p jk-pitch --lib echo::          # 9 unit tests (generated signals)
cargo test --release -p jk-pitch --test echo_alloc     # no allocation per block, 4 rates
cargo run --release -p jk-pitch --example echo_eval -- \
    --tracks "$JK_BUILD/eval/ccmixter_voices/tracks" --out "$JK_BUILD/practice-echo" \
    --others ccm-31416,ccm-55540                        # 126 scenarios, about 25 s on 8 threads
cargo run --release -p jk-pitch --example echo_eval -- --cpu --out "$JK_BUILD/practice-echo"
python -I crates/jk-pitch/examples/echo_eval_report.py \
    "$JK_BUILD/practice-echo/results.jsonl" "$JK_BUILD/practice-echo/cpu.jsonl"
```

`JK_ECHO_TRACE=<scenario name>` prints per-second diagnostics of one scenario (true
suppression, report, filter updates, drift).

Stage 3 (sections 10-12):

```text
T="$JK_BUILD/eval/ccmixter_voices/tracks"; O="$JK_BUILD/practice-echo/stage3/v14"
cargo test --release -p jk-pitch --lib echo::                 # 9 unit tests
cargo test --release -p jk-pitch --test echo_alloc            # 4 tests, incl. loopback off and Bluetooth
cargo run --release -p jk-pitch --example echo_eval -- --tracks "$T" --out "$O/bench" \
    --others ccm-31416,ccm-55540                               # the benchmark, 126 scenarios
cargo run --release -p jk-pitch --example echo_eval -- --tracks "$T" --out "$O/holdout" \
    --main ccm-39381 --others ccm-29611,ccm-29878,ccm-30378,ccm-32665,ccm-44399,ccm-59840,ccm-67424,ccm-68969
cargo run --release -p jk-pitch --example echo_cases -- --tracks "$T" \
    --songs ccm-22364,ccm-31416,ccm-55540 --out "$O"           # 147 cases (off/on, high, continuous, path)
cargo run --release -p jk-pitch --example echo_cases -- --tracks "$T" \
    --songs ccm-39381,ccm-59840 --out "$O/cases-verifier-songs" # the verifier's songs, held out
python -I crates/jk-pitch/examples/echo_eval_compare.py "$JK_BUILD/practice-echo/stage3/bench-base/results.jsonl" "$O/bench/results.jsonl"
python -I crates/jk-pitch/examples/echo_cases_report.py "$O/cases.jsonl" [before.jsonl]
cargo run --release -p jk-pitch --example echo_eval -- --cpu --out "$O"
cargo test --release -p jk-engine --lib tap::                 # 5 tests incl. the callback cost
JK_TAP_REAL_DEVICE=1 cargo test --release -p jk-engine --test output_tap -- --nocapture --test-threads=1
cargo test --release -p jk-engine --test rt_alloc -- --nocapture
cargo run --release -p jk-engine --example tap_loopback -- --secs 20   # real output device, -60 dB
```

`JK_ECHO_TRACE=<case name>` works for `echo_cases` too (every 0.1 s, with the internal
state line `EchoCanceller::debug_line`).

## Appendix: every scenario

Generated by `echo_eval_report.py` from `results.jsonl`.

### Main track ccm-22364: 106 scenarios; all tracks: 126

#### USB loopback, no singer

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| usb_-40dB_2ms_nosinger | 0.84 | -40.0 (-40.0) | -0.004 | -0.0 | stable | 16.4 / 32.0 | -85.5 | good | 0 |
| usb_-40dB_10ms_nosinger | 0.84 | -40.0 (-40.0) | -0.003 | -0.1 | stable | 16.8 / 31.2 | -84.7 | good | 0 |
| usb_-40dB_50ms_nosinger | 0.84 | -40.0 (-40.0) | -0.000 | -0.0 | stable | 16.1 / 31.7 | -85.1 | good | 0 |
| usb_-40dB_150ms_nosinger | 0.84 | -40.0 (-40.0) | 0.002 | 0.0 | stable | 16.4 / 31.8 | -85.3 | good | 0 |
| usb_-40dB_300ms_nosinger | 0.98 | -40.4 (-40.1) | 0.000 | 0.0 | stable | 16.0 / 29.9 | -83.4 | good | 0 |
| usb_-30dB_2ms_nosinger | 0.84 | -30.0 (-30.0) | -0.006 | -0.0 | stable | 19.8 / 39.0 | -82.5 | good | 0 |
| usb_-30dB_10ms_nosinger | 0.84 | -30.0 (-30.0) | 0.000 | 0.0 | stable | 19.8 / 37.3 | -80.8 | good | 0 |
| usb_-30dB_50ms_nosinger | 0.84 | -30.0 (-30.0) | -0.001 | 0.0 | stable | 19.9 / 40.2 | -83.6 | good | 0 |
| usb_-30dB_150ms_nosinger | 0.84 | -30.0 (-30.0) | 0.000 | 0.0 | stable | 19.7 / 39.6 | -83.1 | good | 0 |
| usb_-30dB_300ms_nosinger | 0.98 | -30.4 (-30.1) | 0.000 | -0.0 | stable | 19.4 / 38.0 | -81.6 | good | 0 |
| usb_-20dB_2ms_nosinger | 0.84 | -20.0 (-20.0) | -0.006 | -0.0 | stable | 29.5 / 45.7 | -79.2 | good | 0 |
| usb_-20dB_10ms_nosinger | 0.84 | -20.0 (-20.0) | 0.000 | 0.0 | stable | 29.4 / 43.5 | -77.0 | good | 0 |
| usb_-20dB_50ms_nosinger | 0.84 | -20.0 (-20.0) | -0.000 | -0.0 | stable | 29.4 / 44.7 | -78.2 | good | 0 |
| usb_-20dB_150ms_nosinger | 0.84 | -20.0 (-20.0) | 0.000 | 0.0 | stable | 29.4 / 46.2 | -79.7 | good | 0 |
| usb_-20dB_300ms_nosinger | 0.98 | -20.4 (-20.1) | 0.000 | -0.0 | stable | 29.2 / 45.6 | -79.1 | good | 0 |
| usb_-10dB_2ms_nosinger | 0.84 | -10.0 (-10.0) | -0.005 | -0.0 | stable | 39.2 / 50.4 | -73.8 | good | 0 |
| usb_-10dB_10ms_nosinger | 0.84 | -10.0 (-10.0) | 0.000 | 0.0 | stable | 39.2 / 49.5 | -72.9 | good | 0 |
| usb_-10dB_50ms_nosinger | 0.84 | -10.0 (-10.0) | 0.000 | 0.0 | stable | 39.2 / 50.2 | -73.6 | good | 0 |
| usb_-10dB_150ms_nosinger | 0.84 | -10.0 (-10.0) | 0.000 | 0.0 | stable | 39.2 / 51.1 | -74.5 | good | 0 |
| usb_-10dB_300ms_nosinger | 0.98 | -10.4 (-10.1) | 0.000 | -0.0 | stable | 39.2 / 50.1 | -73.6 | good | 0 |
| usb_+0dB_2ms_nosinger | 0.84 | -0.0 (-0.0) | -0.005 | -0.0 | stable | 48.3 / 54.9 | -68.3 | good | 0 |
| usb_+0dB_10ms_nosinger | 0.84 | -0.0 (-0.0) | 0.000 | 0.0 | stable | 48.2 / 53.9 | -67.3 | good | 0 |
| usb_+0dB_50ms_nosinger | 0.84 | 0.0 (-0.0) | 0.000 | 0.0 | stable | 48.3 / 54.1 | -67.6 | good | 0 |
| usb_+0dB_150ms_nosinger | 0.84 | -0.0 (-0.0) | 0.000 | 0.0 | stable | 49.0 / 57.4 | -70.8 | good | 0 |
| usb_+0dB_300ms_nosinger | 0.98 | -0.4 (-0.1) | 0.000 | -0.0 | stable | 48.9 / 54.2 | -67.8 | good | 0 |
| usb_eq_-20dB_12ms_nosinger | 0.84 | -20.4 (-20.4) | -0.002 | 0.0 | stable | 29.5 / 43.3 | -77.1 | good | 0 |
| usb_eq_+0dB_12ms_nosinger | 0.84 | -0.4 (-0.4) | -0.002 | -0.0 | stable | 47.0 / 50.4 | -64.2 | good | 0 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| usb_-40dB_2ms_nosinger | 6.6 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-40dB_10ms_nosinger | 6.3 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-40dB_50ms_nosinger | 6.2 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-40dB_150ms_nosinger | 6.1 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-40dB_300ms_nosinger | 6.0 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-30dB_2ms_nosinger | 12.0 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-30dB_10ms_nosinger | 11.9 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-30dB_50ms_nosinger | 11.8 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-30dB_150ms_nosinger | 11.7 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-30dB_300ms_nosinger | 11.5 % | 0.1 % | 0.0 % | 0.0 % |
| usb_-20dB_2ms_nosinger | 12.3 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-20dB_10ms_nosinger | 12.1 % | 0.2 % | 0.0 % | 0.0 % |
| usb_-20dB_50ms_nosinger | 12.1 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-20dB_150ms_nosinger | 11.9 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-20dB_300ms_nosinger | 11.7 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-10dB_2ms_nosinger | 13.1 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-10dB_10ms_nosinger | 13.0 % | 0.2 % | 0.0 % | 0.0 % |
| usb_-10dB_50ms_nosinger | 12.9 % | 0.0 % | 0.0 % | 0.0 % |
| usb_-10dB_150ms_nosinger | 12.8 % | 0.1 % | 0.0 % | 0.0 % |
| usb_-10dB_300ms_nosinger | 12.7 % | 0.0 % | 0.0 % | 0.0 % |
| usb_+0dB_2ms_nosinger | 13.2 % | 0.0 % | 0.0 % | 0.0 % |
| usb_+0dB_10ms_nosinger | 13.0 % | 0.1 % | 0.0 % | 0.0 % |
| usb_+0dB_50ms_nosinger | 12.9 % | 0.4 % | 0.0 % | 0.0 % |
| usb_+0dB_150ms_nosinger | 12.8 % | 0.0 % | 0.0 % | 0.0 % |
| usb_+0dB_300ms_nosinger | 12.7 % | 0.0 % | 0.0 % | 0.0 % |
| usb_eq_-20dB_12ms_nosinger | 11.4 % | 0.0 % | 0.0 % | 0.0 % |
| usb_eq_+0dB_12ms_nosinger | 12.1 % | 0.3 % | 0.0 % | 0.0 % |

#### USB loopback, with singer

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| usb_-40dB_2ms_singer | 4.32 | -40.0 (-40.0) | -0.004 | -0.3 | stable | 15.5 / 25.7 | -79.2 | good | 0 |
| usb_-40dB_10ms_singer | 4.32 | -40.0 (-40.0) | 0.002 | 0.6 | stable | 15.0 / 26.4 | -79.8 | good | 0 |
| usb_-40dB_50ms_singer | 4.32 | -39.9 (-40.0) | 0.004 | 0.7 | stable | 15.7 / 26.9 | -80.4 | good | 0 |
| usb_-40dB_150ms_singer | 4.46 | -40.0 (-40.0) | -0.007 | -0.8 | stable | 15.5 / 25.5 | -79.0 | good | 0 |
| usb_-40dB_300ms_singer | 4.60 | -40.5 (-40.1) | -0.009 | -0.1 | stable | 15.4 / 20.8 | -74.3 | good | 0 |
| usb_-30dB_2ms_singer | 1.68 | -30.0 (-30.0) | -0.005 | 0.1 | stable | 14.0 / 31.8 | -75.2 | good | 0 |
| usb_-30dB_10ms_singer | 4.18 | -30.1 (-30.0) | -0.004 | -0.8 | stable | 17.8 / 25.2 | -68.7 | good | 0 |
| usb_-30dB_50ms_singer | 3.77 | -29.9 (-30.0) | -0.005 | 0.6 | stable | 17.4 / 25.5 | -69.0 | good | 0 |
| usb_-30dB_150ms_singer | 4.32 | -30.0 (-30.0) | 0.009 | 1.8 | stable | 21.2 / 31.4 | -74.9 | good | 0 |
| usb_-30dB_300ms_singer | 2.09 | -30.4 (-30.1) | -0.005 | 0.0 | stable | 15.0 / 31.2 | -74.7 | good | 0 |
| usb_-20dB_2ms_singer | 1.54 | -20.0 (-20.0) | -0.008 | 0.4 | stable | 15.6 / 19.9 | -53.4 | good | 0 |
| usb_-20dB_10ms_singer | 1.68 | -20.0 (-20.0) | -0.006 | -0.7 | stable | 16.5 / 27.8 | -61.3 | good | 0 |
| usb_-20dB_50ms_singer | 1.26 | -20.0 (-20.0) | -0.006 | -1.2 | stable | 10.8 / 21.1 | -54.6 | good | 0 |
| usb_-20dB_150ms_singer | 1.26 | -20.0 (-20.0) | -0.004 | -0.2 | stable | 9.9 / 35.1 | -68.6 | good | 0 |
| usb_-20dB_300ms_singer | 1.26 | -20.4 (-20.1) | -0.006 | 0.0 | stable | 9.3 / 34.7 | -68.3 | good | 0 |
| usb_-10dB_2ms_singer | 0.98 | -10.1 (-10.0) | 0.006 | 0.0 | stable | 10.5 / 12.9 | -36.4 | marginal | 0 |
| usb_-10dB_10ms_singer | 0.98 | -10.1 (-10.0) | -0.004 | 0.1 | stable | 14.3 / 17.5 | -40.9 | marginal | 0 |
| usb_-10dB_50ms_singer | 0.84 | -9.9 (-10.0) | -0.006 | -0.5 | stable | 12.3 / 17.2 | -40.6 | marginal | 0 |
| usb_-10dB_150ms_singer | 0.84 | -10.0 (-10.0) | 0.005 | 0.5 | stable | 13.8 / 31.4 | -54.9 | marginal | 0 |
| usb_-10dB_300ms_singer | 0.98 | -10.4 (-10.1) | -0.006 | -0.1 | stable | 10.3 / 34.1 | -57.7 | marginal | 0 |
| usb_+0dB_2ms_singer | 0.84 | -0.1 (-0.0) | -0.004 | -0.0 | stable | 12.6 / 26.3 | -39.7 | unusable | 0 |
| usb_+0dB_10ms_singer | 0.84 | -0.0 (-0.0) | 0.000 | 0.7 | stable | 12.6 / 25.1 | -38.5 | unusable | 0 |
| usb_+0dB_50ms_singer | 0.84 | 0.0 (-0.0) | -0.007 | -0.0 | stable | 12.6 / 25.1 | -38.5 | unusable | 0 |
| usb_+0dB_150ms_singer | 0.84 | 0.0 (-0.0) | 0.007 | 0.0 | stable | 12.4 / 27.0 | -40.5 | unusable | 0 |
| usb_+0dB_300ms_singer | 0.98 | -0.5 (-0.1) | -0.002 | -0.1 | stable | 11.7 / 24.6 | -38.1 | unusable | 0 |
| usb_eq_-20dB_12ms_singer | 1.26 | -20.4 (-20.4) | 0.005 | 1.0 | stable | 10.6 / 22.1 | -55.9 | good | 0 |
| usb_eq_+0dB_12ms_singer | 0.84 | -0.4 (-0.4) | -0.002 | -0.0 | stable | 12.5 / 24.1 | -37.9 | unusable | 0 |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| usb_-40dB_2ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-40dB_10ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-40dB_50ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-40dB_150ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-40dB_300ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-30dB_2ms_singer | 99.9 % | 99.9 % | 99.9 % | 89.2 % | 100.0 % | 0.0 % |
| usb_-30dB_10ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |
| usb_-30dB_50ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |
| usb_-30dB_150ms_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| usb_-30dB_300ms_singer | 99.9 % | 99.9 % | 99.9 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-20dB_2ms_singer | 99.9 % | 98.4 % | 99.9 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-20dB_10ms_singer | 99.9 % | 98.2 % | 99.8 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-20dB_50ms_singer | 99.9 % | 98.2 % | 99.4 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-20dB_150ms_singer | 99.9 % | 98.7 % | 99.6 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-20dB_300ms_singer | 99.9 % | 96.9 % | 99.5 % | 89.4 % | 100.0 % | 0.0 % |
| usb_-10dB_2ms_singer | 99.9 % | 75.5 % | 97.1 % | 86.2 % | 96.3 % | 0.0 % |
| usb_-10dB_10ms_singer | 99.9 % | 75.4 % | 99.6 % | 88.5 % | 99.2 % | 0.0 % |
| usb_-10dB_50ms_singer | 99.9 % | 74.7 % | 99.7 % | 88.5 % | 99.0 % | 0.0 % |
| usb_-10dB_150ms_singer | 99.9 % | 75.1 % | 99.7 % | 88.8 % | 99.4 % | 0.0 % |
| usb_-10dB_300ms_singer | 99.9 % | 70.2 % | 99.4 % | 87.4 % | 98.1 % | 0.0 % |
| usb_+0dB_2ms_singer | 99.9 % | 34.3 % | 98.2 % | 73.8 % | 79.2 % | 0.0 % |
| usb_+0dB_10ms_singer | 99.9 % | 34.2 % | 98.2 % | 77.6 % | 80.0 % | 0.0 % |
| usb_+0dB_50ms_singer | 99.9 % | 35.1 % | 97.6 % | 77.8 % | 79.7 % | 0.0 % |
| usb_+0dB_150ms_singer | 99.9 % | 34.5 % | 98.2 % | 78.1 % | 80.5 % | 0.0 % |
| usb_+0dB_300ms_singer | 99.9 % | 30.2 % | 97.8 % | 73.0 % | 75.2 % | 0.0 % |
| usb_eq_-20dB_12ms_singer | 99.9 % | 99.6 % | 99.8 % | 89.4 % | 100.0 % | 0.0 % |
| usb_eq_+0dB_12ms_singer | 99.9 % | 35.4 % | 97.4 % | 72.4 % | 77.6 % | 0.0 % |

#### Cable, no singer

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| cable_-30dB_7ms_-100ppm_nosinger | 0.84 | -28.6 (-28.5) | 0.022 | -99.9 | stable | 19.0 / 32.9 | -75.1 | good | 0 |
| cable_-30dB_7ms_+100ppm_nosinger | 0.84 | -28.6 (-28.5) | -0.002 | 99.7 | stable | 19.1 / 31.5 | -73.6 | good | 0 |
| cable_-20dB_7ms_-100ppm_nosinger | 0.84 | -18.6 (-18.5) | 0.024 | -99.6 | stable | 27.4 / 34.0 | -66.1 | marginal | 0 |
| cable_-20dB_7ms_+100ppm_nosinger | 0.84 | -18.6 (-18.5) | -0.003 | 99.7 | stable | 27.4 / 32.8 | -64.9 | marginal | 0 |
| cable_-10dB_7ms_-100ppm_nosinger | 0.84 | -8.6 (-8.5) | 0.019 | -100.4 | stable | 32.3 / 34.3 | -56.4 | marginal | 0 |
| cable_-10dB_7ms_+100ppm_nosinger | 0.84 | -8.6 (-8.5) | 0.001 | 100.4 | stable | 31.9 / 33.2 | -55.4 | marginal | 0 |
| cable_+0dB_7ms_-100ppm_nosinger | 0.84 | 1.4 (1.5) | 0.021 | -100.1 | stable | 33.7 / 34.2 | -46.3 | unusable | 0 |
| cable_+0dB_7ms_+100ppm_nosinger | 0.84 | 1.4 (1.5) | 0.001 | 100.4 | stable | 33.3 / 33.2 | -45.3 | unusable | 0 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| cable_-30dB_7ms_-100ppm_nosinger | 10.5 % | 0.0 % | 0.0 % | 0.0 % |
| cable_-30dB_7ms_+100ppm_nosinger | 10.4 % | 0.0 % | 0.0 % | 0.0 % |
| cable_-20dB_7ms_-100ppm_nosinger | 10.7 % | 0.0 % | 0.0 % | 0.0 % |
| cable_-20dB_7ms_+100ppm_nosinger | 10.8 % | 0.5 % | 0.0 % | 0.0 % |
| cable_-10dB_7ms_-100ppm_nosinger | 11.6 % | 0.2 % | 0.0 % | 0.0 % |
| cable_-10dB_7ms_+100ppm_nosinger | 11.7 % | 1.0 % | 0.0 % | 0.0 % |
| cable_+0dB_7ms_-100ppm_nosinger | 11.7 % | 1.1 % | 0.0 % | 0.0 % |
| cable_+0dB_7ms_+100ppm_nosinger | 11.7 % | 1.6 % | 0.0 % | 0.0 % |

#### Cable, with singer

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| cable_-30dB_7ms_-100ppm_singer | 4.32 | -28.6 (-28.5) | 0.188 | -99.8 | stable | 15.7 / 23.0 | -65.1 | good | 0 |
| cable_-30dB_7ms_+100ppm_singer | 3.77 | -28.6 (-28.5) | -0.172 | 99.4 | stable | 16.9 / 24.3 | -66.5 | good | 0 |
| cable_-20dB_7ms_-100ppm_singer | 1.68 | -18.6 (-18.5) | 0.193 | -98.4 | stable | 10.4 / 9.0 | -41.1 | marginal | 0 |
| cable_-20dB_7ms_+100ppm_singer | 1.68 | -18.6 (-18.5) | -0.137 | 100.8 | stable | 15.6 / 16.6 | -48.7 | good | 0 |
| cable_-10dB_7ms_-100ppm_singer | 1.12 | -8.6 (-8.5) | 0.182 | -98.5 | stable | 11.9 / 16.4 | -38.5 | marginal | 0 |
| cable_-10dB_7ms_+100ppm_singer | 1.12 | -8.6 (-8.5) | -0.072 | 99.2 | stable | 13.8 / 25.3 | -47.4 | marginal | 0 |
| cable_+0dB_7ms_-100ppm_singer | 0.84 | 1.3 (1.5) | 0.024 | -99.2 | stable | 13.1 / 24.3 | -36.4 | unusable | 0 |
| cable_+0dB_7ms_+100ppm_singer | 0.84 | 1.4 (1.5) | 0.005 | 100.2 | stable | 12.9 / 26.0 | -38.1 | unusable | 0 |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| cable_-30dB_7ms_-100ppm_singer | 99.9 % | 99.9 % | 99.9 % | 79.7 % | 100.0 % | 0.0 % |
| cable_-30dB_7ms_+100ppm_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |
| cable_-20dB_7ms_-100ppm_singer | 99.9 % | 94.1 % | 99.5 % | 89.4 % | 100.0 % | 3.2 % |
| cable_-20dB_7ms_+100ppm_singer | 99.9 % | 93.8 % | 99.5 % | 89.4 % | 100.0 % | 4.8 % |
| cable_-10dB_7ms_-100ppm_singer | 99.9 % | 67.4 % | 97.6 % | 86.8 % | 97.0 % | 0.0 % |
| cable_-10dB_7ms_+100ppm_singer | 99.9 % | 66.5 % | 98.6 % | 87.1 % | 97.6 % | 0.0 % |
| cable_+0dB_7ms_-100ppm_singer | 99.9 % | 30.1 % | 96.5 % | 78.4 % | 82.6 % | 0.0 % |
| cable_+0dB_7ms_+100ppm_singer | 99.9 % | 30.2 % | 96.0 % | 76.4 % | 79.6 % | 0.0 % |

#### Bluetooth

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| bluetooth_-20dB_120ms_nosinger | 0.84 | -22.6 (-20.1) | 7.803 | 207.5 | wandering | 0.2 / 1.7 | -35.2 | unusable | 8 |
| bluetooth_-20dB_200ms_nosinger | 0.84 | -22.2 (-20.1) | 9.036 | 147.8 | wandering | 10.2 / 1.7 | -35.3 | unusable | 10 |
| bluetooth_-20dB_300ms_nosinger | 0.98 | -22.9 (-20.2) | 3.377 | 220.2 | wandering | 0.2 / 1.6 | -35.3 | unusable | 12 |
| bluetooth_-10dB_120ms_nosinger | 0.84 | -12.6 (-10.1) | 7.803 | 207.5 | wandering | 0.2 / 1.6 | -25.2 | unusable | 8 |
| bluetooth_-10dB_200ms_nosinger | 0.84 | -12.1 (-10.1) | 9.036 | 147.8 | wandering | 10.0 / 2.1 | -25.7 | unusable | 10 |
| bluetooth_-10dB_300ms_nosinger | 0.98 | -12.9 (-10.2) | 3.377 | 220.2 | wandering | 0.2 / 1.6 | -25.2 | unusable | 12 |
| bluetooth_+0dB_120ms_nosinger | 0.84 | -2.6 (-0.1) | 7.803 | 207.5 | wandering | 0.2 / 1.7 | -15.2 | unusable | 8 |
| bluetooth_+0dB_200ms_nosinger | 0.84 | -2.1 (-0.1) | 9.036 | 147.8 | wandering | 10.0 / 2.1 | -15.7 | unusable | 10 |
| bluetooth_+0dB_300ms_nosinger | 0.98 | -2.9 (-0.2) | 3.377 | 220.2 | wandering | 0.2 / 1.5 | -15.2 | unusable | 12 |
| bluetooth_-20dB_120ms_singer | 1.26 | -23.4 (-20.1) | 9.558 | 194.2 | wandering | - / 1.6 | -35.2 | unusable | 11 |
| bluetooth_-20dB_200ms_singer | 1.26 | -22.7 (-20.1) | - | 195.8 | wandering | - / 1.2 | -34.8 | unusable | 12 |
| bluetooth_-20dB_300ms_singer | 1.40 | -23.3 (-20.2) | 3.372 | 216.2 | wandering | - / 0.8 | -34.5 | unusable | 10 |
| bluetooth_-10dB_120ms_singer | 0.84 | -13.2 (-10.1) | 9.558 | 215.0 | wandering | - / 1.7 | -25.2 | unusable | 8 |
| bluetooth_-10dB_200ms_singer | 0.84 | -15.3 (-10.1) | 9.035 | 140.0 | wandering | - / 1.3 | -24.9 | unusable | 10 |
| bluetooth_-10dB_300ms_singer | 0.98 | -13.3 (-10.2) | 3.377 | 204.9 | wandering | - / 1.1 | -24.7 | unusable | 11 |
| bluetooth_+0dB_120ms_singer | 0.84 | -4.3 (-0.1) | 7.803 | 202.8 | wandering | 4.5 / 1.8 | -15.3 | unusable | 10 |
| bluetooth_+0dB_200ms_singer | 0.84 | -5.6 (-0.1) | 9.036 | 139.1 | wandering | - / 1.5 | -15.1 | unusable | 10 |
| bluetooth_+0dB_300ms_singer | 0.98 | -4.8 (-0.2) | 3.378 | 207.1 | wandering | - / 1.2 | -14.9 | unusable | 15 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| bluetooth_-20dB_120ms_nosinger | 8.4 % | 6.9 % | 0.0 % | 0.0 % |
| bluetooth_-20dB_200ms_nosinger | 7.9 % | 6.4 % | 0.0 % | 0.0 % |
| bluetooth_-20dB_300ms_nosinger | 8.1 % | 7.8 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_120ms_nosinger | 9.0 % | 7.4 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_200ms_nosinger | 8.4 % | 6.4 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_300ms_nosinger | 8.4 % | 8.7 % | 0.0 % | 0.0 % |
| bluetooth_+0dB_120ms_nosinger | 9.0 % | 7.3 % | 0.0 % | 0.0 % |
| bluetooth_+0dB_200ms_nosinger | 8.4 % | 6.5 % | 0.0 % | 0.0 % |
| bluetooth_+0dB_300ms_nosinger | 8.4 % | 8.8 % | 0.0 % | 0.0 % |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| bluetooth_-20dB_120ms_singer | 99.9 % | 99.2 % | 99.6 % | 0.0 % | 0.0 % | 0.0 % |
| bluetooth_-20dB_200ms_singer | 99.9 % | 98.2 % | 99.5 % | 0.0 % | 0.0 % | 0.0 % |
| bluetooth_-20dB_300ms_singer | 99.9 % | 97.1 % | 98.5 % | 0.0 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_120ms_singer | 99.9 % | 74.1 % | 85.7 % | 0.0 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_200ms_singer | 99.9 % | 73.3 % | 83.4 % | 2.3 % | 0.0 % | 0.0 % |
| bluetooth_-10dB_300ms_singer | 99.9 % | 70.2 % | 78.3 % | 0.0 % | 0.0 % | 0.0 % |
| bluetooth_+0dB_120ms_singer | 99.9 % | 34.9 % | 56.3 % | 15.7 % | 11.8 % | 0.0 % |
| bluetooth_+0dB_200ms_singer | 99.9 % | 33.3 % | 53.6 % | 8.0 % | 6.1 % | 0.0 % |
| bluetooth_+0dB_300ms_singer | 99.9 % | 30.2 % | 43.1 % | 0.8 % | 1.2 % | 0.0 % |

#### Guide vocal in the playback

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| usb+guide_+0dB_10ms_nosinger | 0.84 | 0.1 (-0.0) | 0.000 | 0.0 | stable | 51.7 / 64.5 | -76.2 | good | 0 |
| usb+guide_-10dB_10ms_nosinger | 0.84 | -9.9 (-10.0) | 0.000 | 0.0 | stable | 43.1 / 56.6 | -78.3 | good | 0 |
| cable+guide_+0dB_7ms_+100ppm_nosinger | 0.84 | 0.6 (0.8) | -0.002 | 100.3 | stable | 29.3 / 32.4 | -43.4 | unusable | 0 |
| cable+guide_-20dB_7ms_+100ppm_nosinger | 0.84 | -19.4 (-19.2) | -0.001 | 100.3 | stable | 27.0 / 31.8 | -62.8 | marginal | 0 |
| bluetooth+guide_+0dB_200ms_nosinger | 1.26 | -5.0 (-0.2) | 9.103 | 116.5 | wandering | 0.4 / 0.7 | -12.6 | unusable | 10 |
| cable+guide+break_+0dB_7ms_+100ppm_nosinger | 0.84 | 0.6 (0.8) | -0.002 | 100.2 | stable | 28.9 / 30.4 | -42.9 | unusable | 0 |
| usb+guide_+0dB_10ms_singer | 0.84 | 0.1 (-0.0) | -0.004 | -0.1 | stable | 10.9 / 25.4 | -37.1 | unusable | 0 |
| usb+guide_-10dB_10ms_singer | 0.84 | -9.9 (-10.0) | -0.005 | -0.6 | stable | 16.5 / 31.6 | -53.3 | marginal | 0 |
| cable+guide_+0dB_7ms_+100ppm_singer | 0.84 | 0.6 (0.8) | -0.001 | 99.9 | stable | 11.2 / 23.6 | -34.6 | unusable | 0 |
| cable+guide_-20dB_7ms_+100ppm_singer | 0.84 | -19.5 (-19.2) | -0.154 | 100.1 | stable | 15.4 / 28.6 | -59.6 | good | 0 |
| bluetooth+guide_+0dB_200ms_singer | 1.26 | -4.0 (-0.2) | 9.103 | 113.7 | wandering | - / 0.6 | -12.4 | unusable | 15 |
| cable+guide+break_+0dB_7ms_+100ppm_singer | 0.84 | 0.5 (0.8) | 0.000 | 100.7 | stable | 11.0 / 21.4 | -33.9 | unusable | 0 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| usb+guide_+0dB_10ms_nosinger | 21.1 % | 3.3 % | 0.0 % | 0.0 % |
| usb+guide_-10dB_10ms_nosinger | 21.1 % | 1.4 % | 0.0 % | 0.0 % |
| cable+guide_+0dB_7ms_+100ppm_nosinger | 18.8 % | 4.3 % | 0.0 % | 0.0 % |
| cable+guide_-20dB_7ms_+100ppm_nosinger | 18.7 % | 1.9 % | 0.0 % | 0.0 % |
| bluetooth+guide_+0dB_200ms_nosinger | 17.8 % | 16.3 % | 0.3 % | 0.0 % |
| cable+guide+break_+0dB_7ms_+100ppm_nosinger | 16.0 % | 4.0 % | 0.0 % | 0.0 % |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| usb+guide_+0dB_10ms_singer | 99.9 % | 23.5 % | 96.3 % | 56.7 % | 62.1 % | 0.0 % |
| usb+guide_-10dB_10ms_singer | 99.9 % | 60.0 % | 97.6 % | 88.3 % | 99.0 % | 0.0 % |
| cable+guide_+0dB_7ms_+100ppm_singer | 99.9 % | 20.8 % | 90.6 % | 61.7 % | 67.4 % | 0.0 % |
| cable+guide_-20dB_7ms_+100ppm_singer | 99.9 % | 93.6 % | 99.9 % | 89.4 % | 100.0 % | 0.0 % |
| bluetooth+guide_+0dB_200ms_singer | 99.9 % | 24.8 % | 38.7 % | 1.5 % | 0.0 % | 0.0 % |
| cable+guide+break_+0dB_7ms_+100ppm_singer | 99.9 % | 37.5 % | 89.9 % | 67.8 % | 76.7 % | 0.0 % |

#### Nothing returned

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| none_+0dB_0ms_nosinger | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| none+guide_+0dB_0ms_nosinger | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| none_+0dB_0ms_singer | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| none+guide_+0dB_0ms_singer | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| none_+0dB_0ms_nosinger | 0.0 % | 0.0 % | 0.0 % | 0.0 % |
| none+guide_+0dB_0ms_nosinger | 0.0 % | 0.0 % | 0.0 % | 0.0 % |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| none_+0dB_0ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |
| none+guide_+0dB_0ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |

#### Break in the music

| scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|
| usb+break_-10dB_10ms_nosinger | 0.84 | -10.0 (-10.0) | 0.000 | 0.0 | stable | 38.2 / 46.1 | -71.4 | good | 0 |
| cable+guide+break_+0dB_7ms_+100ppm_nosinger | 0.84 | 0.6 (0.8) | -0.002 | 100.2 | stable | 28.9 / 30.4 | -42.9 | unusable | 0 |
| usb+break_-10dB_10ms_singer | 0.98 | -10.1 (-10.0) | 0.008 | 0.3 | stable | 12.5 / 11.7 | -37.0 | marginal | 0 |
| cable+guide+break_+0dB_7ms_+100ppm_singer | 0.84 | 0.5 (0.8) | 0.000 | 100.7 | stable | 11.0 / 21.4 | -33.9 | unusable | 0 |

| scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|
| usb+break_-10dB_10ms_nosinger | 8.5 % | 0.2 % | 0.0 % | 0.0 % |
| cable+guide+break_+0dB_7ms_+100ppm_nosinger | 16.0 % | 4.0 % | 0.0 % | 0.0 % |

| scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|
| usb+break_-10dB_10ms_singer | 99.9 % | 84.2 % | 99.3 % | 88.4 % | 99.1 % | 0.0 % |
| cable+guide+break_+0dB_7ms_+100ppm_singer | 99.9 % | 37.5 % | 89.9 % | 67.8 % | 76.7 % | 0.0 % |

#### Other tracks

| track | scenario | found after (s) | level dB (truth) | delay error ms | drift ppm | stability | suppression dB est / true | residual dBFS | quality | relocks |
|---|---|---|---|---|---|---|---|---|---|---|
| ccm-31416 | usb_-20dB_10ms_nosinger | 0.84 | -19.9 (-20.0) | -0.002 | 0.0 | stable | 41.0 / 43.4 | -77.1 | good | 0 |
| ccm-31416 | cable_+0dB_7ms_+100ppm_nosinger | 0.84 | -0.9 (-0.4) | -0.096 | 100.7 | stable | 39.1 / 29.0 | -43.3 | marginal | 0 |
| ccm-31416 | cable+guide_-10dB_7ms_-100ppm_nosinger | 0.84 | -11.0 (-10.6) | 0.016 | -100.2 | stable | 42.7 / 42.2 | -64.1 | good | 0 |
| ccm-31416 | bluetooth_+0dB_200ms_nosinger | 1.26 | -4.1 (-0.1) | - | 217.3 | wandering | - / -0.1 | -13.8 | unusable | 9 |
| ccm-31416 | none+guide_+0dB_0ms_nosinger | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| ccm-31416 | usb_-20dB_10ms_singer | 1.26 | -19.9 (-20.0) | -0.001 | 0.2 | stable | 12.7 / 23.7 | -57.5 | marginal | 0 |
| ccm-31416 | cable_+0dB_7ms_+100ppm_singer | 0.84 | -0.9 (-0.4) | -0.003 | 100.0 | stable | 10.4 / 14.3 | -28.7 | unusable | 1 |
| ccm-31416 | cable+guide_-10dB_7ms_-100ppm_singer | 0.84 | -11.1 (-10.6) | 0.086 | -99.8 | stable | 13.7 / 28.4 | -50.3 | unusable | 0 |
| ccm-31416 | bluetooth_+0dB_200ms_singer | 2.37 | -3.9 (-0.1) | - | 226.3 | wandering | - / -0.4 | -13.5 | unusable | 8 |
| ccm-31416 | none+guide_+0dB_0ms_singer | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| ccm-55540 | usb_-20dB_10ms_nosinger | 0.84 | -20.0 (-20.0) | -0.000 | -0.0 | stable | 41.5 / 48.4 | -82.4 | good | 0 |
| ccm-55540 | cable_+0dB_7ms_+100ppm_nosinger | 0.84 | 1.3 (1.3) | -0.001 | 100.2 | stable | 34.0 / 34.6 | -47.2 | unusable | 0 |
| ccm-55540 | cable+guide_-10dB_7ms_-100ppm_nosinger | 0.84 | -9.1 (-8.9) | 0.021 | -100.2 | stable | 34.3 / 35.2 | -57.3 | marginal | 0 |
| ccm-55540 | bluetooth_+0dB_200ms_nosinger | 1.12 | -2.3 (-0.2) | 9.147 | 234.5 | wandering | - / 2.4 | -16.6 | unusable | 10 |
| ccm-55540 | none+guide_+0dB_0ms_nosinger | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |
| ccm-55540 | usb_-20dB_10ms_singer | 0.84 | -20.0 (-20.0) | -0.005 | -0.1 | stable | 14.9 / 34.0 | -68.1 | good | 0 |
| ccm-55540 | cable_+0dB_7ms_+100ppm_singer | 0.84 | 1.3 (1.3) | -0.074 | 100.4 | stable | 11.2 / 25.3 | -37.9 | unusable | 0 |
| ccm-55540 | cable+guide_-10dB_7ms_-100ppm_singer | 0.84 | -9.1 (-8.9) | 0.094 | -99.9 | stable | 14.1 / 29.0 | -51.2 | marginal | 0 |
| ccm-55540 | bluetooth_+0dB_200ms_singer | 3.77 | -2.5 (-0.2) | 9.156 | 257.8 | wandering | - / 1.3 | -15.5 | unusable | 7 |
| ccm-55540 | none+guide_+0dB_0ms_singer | - | - (-300.0) | - | - | unknown | - / - | - | notneeded | 0 |

| track | scenario | pitch frames, no removal | after removal | after removal and trusted | same, from 15 s |
|---|---|---|---|---|---|
| ccm-31416 | usb_-20dB_10ms_nosinger | 5.8 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-31416 | cable_+0dB_7ms_+100ppm_nosinger | 6.1 % | 0.2 % | 0.0 % | 0.0 % |
| ccm-31416 | cable+guide_-10dB_7ms_-100ppm_nosinger | 5.3 % | 0.2 % | 0.0 % | 0.0 % |
| ccm-31416 | bluetooth_+0dB_200ms_nosinger | 3.2 % | 3.8 % | 0.2 % | 0.3 % |
| ccm-31416 | none+guide_+0dB_0ms_nosinger | 0.0 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-55540 | usb_-20dB_10ms_nosinger | 18.4 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-55540 | cable_+0dB_7ms_+100ppm_nosinger | 18.1 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-55540 | cable+guide_-10dB_7ms_-100ppm_nosinger | 15.7 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-55540 | bluetooth_+0dB_200ms_nosinger | 13.1 % | 12.3 % | 0.1 % | 0.0 % |
| ccm-55540 | none+guide_+0dB_0ms_nosinger | 0.0 % | 0.0 % | 0.0 % | 0.0 % |

| track | scenario | right, clean voice | no removal | after removal | removal + trusted | same, from 15 s | rests shown as pitch |
|---|---|---|---|---|---|---|---|
| ccm-31416 | usb_-20dB_10ms_singer | 99.9 % | 97.9 % | 99.9 % | 89.4 % | 100.0 % | 0.0 % |
| ccm-31416 | cable_+0dB_7ms_+100ppm_singer | 99.9 % | 10.1 % | 82.8 % | 59.5 % | 63.8 % | 0.0 % |
| ccm-31416 | cable+guide_-10dB_7ms_-100ppm_singer | 99.9 % | 47.9 % | 96.3 % | 88.6 % | 99.7 % | 0.0 % |
| ccm-31416 | bluetooth_+0dB_200ms_singer | 99.9 % | 11.6 % | 11.7 % | 2.0 % | 3.0 % | 0.3 % |
| ccm-31416 | none+guide_+0dB_0ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |
| ccm-55540 | usb_-20dB_10ms_singer | 99.9 % | 99.9 % | 99.9 % | 89.4 % | 100.0 % | 0.0 % |
| ccm-55540 | cable_+0dB_7ms_+100ppm_singer | 99.9 % | 0.2 % | 97.1 % | 37.5 % | 45.4 % | 0.0 % |
| ccm-55540 | cable+guide_-10dB_7ms_-100ppm_singer | 99.9 % | 18.0 % | 99.7 % | 89.4 % | 100.0 % | 0.0 % |
| ccm-55540 | bluetooth_+0dB_200ms_singer | 99.9 % | 1.1 % | 5.2 % | 0.0 % | 0.0 % | 0.0 % |
| ccm-55540 | none+guide_+0dB_0ms_singer | 99.9 % | 99.9 % | 99.9 % | 90.2 % | 100.0 % | 0.0 % |

#### Aggregates

- returns found: 118 of 118; slowest 4.60 s; median 0.84 s; not found: []
- delay error (USB and cable, 94 runs): median 0.0040 ms, worst 0.1933 ms
- level error: median 0.10 dB, worst 5.51 dB
- no singer, music returned: pitch frames after removal and trusted: worst 0.3 % (bluetooth+guide_+0dB_200ms_nosinger @ ccm-22364); without removal up to 21.1 %
- same without Bluetooth: worst 0.0 % (cable+guide_-20dB_7ms_+100ppm_nosinger @ ccm-22364)
- Bluetooth runs judged unusable: 24 of 24; wandering: 24
- USB/cable runs judged wandering: []

#### CPU

- 16000 Hz: 5.49 ms per second of audio (0.55 % of one core)
- 44100 Hz: 10.02 ms per second of audio (1.00 % of one core)
- 48000 Hz: 8.53 ms per second of audio (0.85 % of one core)
- 96000 Hz: 18.57 ms per second of audio (1.86 % of one core)

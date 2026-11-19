# Practice mode: the key in effect (Work Order 2, Stage 0)

Work Order 2 section 4.6 ("Changing key during a song"), trap 1 and the section 9 rows
"Key change" and "Key change during a song". Stage 0 question: can the engine tell
practice mode which key is really *heard*, and does the original lead vocal, shifted by
the engine, land exactly on the shifted guide? Measured 2026-11-19 on the build PC
(Ryzen 7 9700X, Windows 11), GNU toolchain, Rubber Band R3 as shipped. Stage 7 merges
this file into `docs/benchmarks/practice.md`.

Evidence (outside the repository): `../jukeoke-build/practice-key/` (scripts
`key_proof.py`, `run_b.py`, `key_switch_measure.py`, `remeasure.py`; per-render pitch
arrays `a/*.npz`; reports `a/report*.txt|json`; captures and published timelines
`b/*/capture.wav|keys.json|measure.json`, final measurements in `b-measure.log`
(`b-run.log` is the capture run, measured there with a first, envelope-based alignment
that `remeasure.py` replaced by the exact one); test logs `engine-dsp-tests.log`,
`release-tests.log`, `key_in_effect-release.txt`). Only the three Creative Commons
songs, the generated demo song and generated tones were used. No lyrics were read or
written.

## 1. What the engine now publishes

`Snapshot::key` (`KeyStatus`, already sent with every snapshot) has four new fields;
everything else in the snapshot is unchanged, so the app compiles and behaves as before.

| field | meaning |
|---|---|
| `heard` | semitones sounding at the DAC now, i.e. at `Snapshot::position_s` |
| `requested` | the key last asked for (`load` / `set_params`, clamped to -12..12) |
| `pending` | `None`, or the next key to be heard with `at_s`: the song time from which it is heard (`None` while the change has no position yet) |
| `segment` | playback segment that `position_s` and `heard` belong to (a new one at every load, seek, device switch and paused re-render) |

`Engine::key_at(pos_s, segment)` answers "which key was sounding at song time t" for
the recent past (64 segments; within a segment every change ever scheduled), so each
note can be graded in the key that was heard when it was sung. Without a segment id the
newest segment that played that position answers. `Engine::key_timeline()` returns the
history itself (tests, evidence).

**Definition of the switch.** A key change crossfades the old and new path over 40 ms
(equal power). The switch is the crossfade's midpoint, where both keys have the same
weight; `jk_dsp::PitchGraph::audible_semitones` already switched there. In render-ahead
mode it is the midpoint of the crossfade onto the cached key, i.e. the moment the engine
crosses over, not the moment the key was asked for or finished rendering.

**How.** `jk_dsp::PitchGraph::scheduled_switch()` reports the running crossfade's
midpoint and target (one graph latency, about 0.1 s, before any of it is output; again,
with the same position and key 0, if a cached key goes missing during its crossfade). The
render worker turns it into a song frame and pushes a small mark (segment, frame, key)
into a 256-slot lock-free ring, plus one mark with the key each segment starts in. The
house thread (every 5 ms) and every snapshot drain the ring into a bounded history and
read the key at the DAC position. The device callback's only change: it publishes the
DAC position and its segment as one consistent pair (a sequence lock of plain atomic
stores) instead of two separate stores, so a snapshot can never pair a position of one
segment with another segment's key. No allocation, lock or wait was added to the
callback or the render worker: `tests/rt_alloc.rs` passes unchanged (real-time and
prerender phases, key changes and seeks included).

## 2. Key change during playback (WO2 section 9 "Key change during a song")

Bar: the published switch matches the moment the pitch changes in the produced audio
within 50 ms, in both key modes.

### 2.1 Graph level (jk-dsp, offline, exact)

`cargo test --release -p jk-dsp --test key_switch -- --nocapture`. A 660 Hz sine through
the graph, eight key changes (0 > +5 > -7 > +12 > -12 > 0 > +2 > -3 > 0) at 44.1 and
48 kHz. The pitch change in the output is located with
`jk_dsp::analysis::tone_switch_index` (the point where the old and the new tone's
windowed energies, each relative to its steady level, cross; for a symmetric crossfade
and window that is exactly the crossfade midpoint).

| | result |
|---|---|
| reported switch vs measured pitch change | worst 0.01 ms (16 changes) |
| reported before it was output | 112.9 ms (44.1 kHz), 105.3 ms (48 kHz), every change |
| request to switch | 20.8-30.1 ms of song (next 512-frame block + half the crossfade) |
| cached key lost during its crossfade | reported again at the same frame with key 0 (test `a_cached_key_that_goes_missing_is_reported_again_as_the_bypass`) |

### 2.2 Engine end to end, generated tone (CI test)

`cargo test -p jk-engine --test key_in_effect -- --nocapture` (debug; release in 2.4).
The real engine on the null device in real time with a capture of every played buffer;
a generated song whose lead stem is a steady 440 Hz sine
(`jk_engine::demo::make_tone_song`), guide vocal up. Capture index to song frame: the
first non-zero frame is the segment's first frame (the sine is never zero there). The
switch positions are the ones the engine published (`key_timeline`).

| mode | changes | published switch vs measured pitch change | published ahead of the DAC (wall clock) | request to switch (song time) |
|---|---|---|---|---|
| real time | 0>+3>-4>+7>-9>+12>-12>0 | -0.00 .. +0.02 ms | 269-289 ms | 279-296 ms |
| render ahead (forced) | 0>+3>-4>+7>0 | -0.00 .. +0.01 ms | 278-290 ms | 1093-1274 ms (new key rendered first); 294 ms back to 0 (no rendering) |

In render-ahead mode the new key was reported as pending without a position for
0.68-0.85 s per change while it was rendered, with the old key heard throughout. The
same test checks every snapshot taken (one every ~2 ms): `heard` equals the history's key
at that snapshot's position, a pending switch always lies ahead of the DAC and is one
that really happened, and right after each request the old key is still `heard` with the
new one `pending`. Zero underruns, zero lost marks.

The other tests in that file:

- `key_in_effect_on_a_resampling_device_in_both_modes` (added by the Stage 0
  verifier): the same check with the 44.1 kHz tone song on a 48 kHz null device, so the
  output resampler sits between the graph and the DAC (the usual shared-mode and M1
  case; the tests above run device rate = song rate). Real time 0>+5>-7>+12>0 and
  forced render ahead 0>-5>0: worst |published - measured| 0.01 ms (release and debug),
  published 276-291 ms ahead of the DAC.

- `seeks_crossfades_and_paused_changes_report_the_key_heard` (real time): a seek while a
  change is scheduled but not yet heard starts the new segment directly in the new key;
  the old segment keeps its history (`key_at` with and without a segment id); sampled
  every 1 ms around a 40 ms crossfade, `heard` flips exactly at the published position
  (21 snapshots before, 19 after, within 30 ms of it); a key change while paused
  re-renders from the paused position and is heard at once (no pending), and stays after
  play.
- `prerender_seek_before_the_new_key_is_rendered_keeps_the_old_key`: renderer slowed to
  0.5x real time (`debug_set_slowdown`), key +4 asked: `heard` 0, `pending` +4 without
  position, still so after 1 s and after a seek (the new segment starts on the old key);
  at full speed again the crossover is published 277 ms of song ahead of the DAC, `heard`
  flips at it, `key_at` gives 0 just before and +4 at it.

### 2.3 Real lead vocals (supplementary)

`jukeoke-cli key-switch-capture` (new) plays a song's lead vocal alone on the null
device in real time, changes key at given times, and writes the played audio and the
timeline the engine published; `run_b.py` drove it for a 440 Hz tone, the three CC songs
and the demo song, 19 s each from the most voiced part, in both modes (real time: six
changes 3 s apart; render ahead: four, 4 s apart). Zero underruns in all 10 captures.
`key_switch_measure.py`: the capture is placed in the song by an exact sample match with
the key-0 offline render (wherever key 0 is heard the capture is bit-identical to it);
pyin + yin run on the capture and on that render with the same 46 ms frames;
cents(capture) - cents(key-0 render) at the same song time is 100 x the key heard; voiced
frames within 60 cents of the old or new key are classified, and the change is placed
half-way between the last old and the first new frame (half that gap is the
uncertainty). This is much coarser than 2.1/2.2 (frames that straddle the switch hold both
keys; consonants and breaths leave gaps) but it is real singing through the whole path.

| set | switches | measured | within 50 ms | outside 50 ms + uncertainty | uncertainty <= 30 ms: median / max offset |
|---|---|---|---|---|---|
| tone, real time | 6 | 6 | 6 | 0 | 2.1 / 6.6 ms |
| tone, render ahead | 4 | 4 | 4 | 0 | 3.7 / 5.8 ms |
| songs, real time | 24 | 22 | 16 | 0 | 4.0 / 15.6 ms (12 switches) |
| songs, render ahead | 16 | 16 | 13 | 0 | 3.2 / 31.6 ms (12 switches) |

Per switch (offset ms +- uncertainty; "unvoiced": too few voiced frames within 0.6 s):

| song | mode | changes (published switch in song s: offset) |
|---|---|---|
| 440 Hz tone | realtime | 0>-3 @3.78: +2+-17; -3>+4 @6.77: -7+-23; +4>-5 @9.77: -2+-29; -5>+2 @12.77: +2+-23; +2>-7 @15.77: +6+-29; -7>0 @18.77: -2+-23 |
| 440 Hz tone | prerender | 0>-3 @4.74: +6+-17; -3>+4 @8.11: -4+-23; +4>-5 @12.07: +4+-29; -5>0 @15.27: -4+-17 |
| Feel (Stripped) | realtime | 0>-3 @219.65: -22+-41; -3>+4 @222.65: -18+-35; +4>-5 @225.65: +16+-23; -5>+2 @228.65: -50+-52; +2>-7 @231.65: +6+-29; -7>0 @234.65: -13+-23 |
| Feel (Stripped) | prerender | 0>-3 @220.11: +1+-12; -3>+4 @224.18: +7+-29; +4>-5 @228.31: +59+-168; -5>0 @231.16: -3+-17 |
| Crowdpleaser | realtime | 0>-3 @116.81: -20+-46; -3>+4 @119.81: -3+-17; +4>-5 @122.81: -90+-151; -5>+2 @125.81: +9+-41; +2>-7 @128.81: +67+-75; -7>0 @131.81: +3+-23 |
| Crowdpleaser | prerender | 0>-3 @117.20: +9+-17; -3>+4 @121.32: -8+-23; +4>-5 @125.35: -165+-168; -5>0 @128.32: -32+-23 |
| The Homeless Man | realtime | 0>-3 @62.29: +3+-23; -3>+4 @65.29: +96+-70; +4>-5 @68.29: +3+-23; -5>+2 @71.29: unvoiced; +2>-7 @74.29: +195+-203; -7>0 @77.29: -78+-151 |
| The Homeless Man | prerender | 0>-3 @62.76: +15+-12; -3>+4 @66.90: +44+-52; +4>-5 @70.75: +172+-342; -5>0 @73.80: -3+-17 |
| demo song | realtime | 0>-3 @1.78: unvoiced; -3>+4 @4.78: -7+-23; +4>-5 @7.78: -1+-29; -5>+2 @10.77: -7+-23; +2>-7 @13.77: -1+-29; -7>0 @16.77: +5+-23 |
| demo song | prerender | 0>-3 @2.25: +3+-12; -3>+4 @6.16: +3+-23; +4>-5 @10.55: -3+-29; -5>0 @13.29: -3+-17 |

Every offset larger than 50 ms has an uncertainty that covers it (the singer was between
notes or breathing at the switch); where the measurement is tight the offsets are a few
milliseconds, as on the tone. The timing itself does not depend on the material: the
switch is placed by the graph's sample counting (2.1).

### 2.4 Release build

`cargo test --release -p jk-engine -p jk-dsp -p jk-cli` (what `scripts/local-ci.sh`
runs), while other Stage 0 workstreams were building and benchmarking on the same PC:
64 tests passed, including `rt_alloc` and all of the above; one failed:
`keycache.rs::cached_output_is_bit_identical_to_the_real_time_path_and_seeks_work`
timed out (59.8 s) in one of its waits for the background renderer, the CPU-starvation
case D-208 describes for that file's "fast" null-device tests. Run again alone, twice:
3/3 passed both times (35.2 s, 38.1 s). The debug run of the same suite before had passed
in full (53 tests).

`key_in_effect` in release, same load: real time, 7 changes, |published - measured| <=
0.01 ms, published 260-288 ms before the DAC got there, request to switch 286-296 ms of
song; render ahead, 4 changes, <= 0.01 ms, published 279-289 ms ahead, the new key
pending without a position for 1.28-1.56 s while it rendered (slower than in 2.2: the
CPU was shared), request to switch 1731-1968 ms; the prerender seek test published the
crossover 275 ms of song ahead.

## 3. The original vocal, shifted by the engine (WO2 section 9 "Key change")

For every key -12..+12, the original isolated lead vocal of each song (guide 1, backing
off, other 0, drums 0) rendered by `jukeoke-cli render` (the export path: the playback
graph, single-threaded), 32-bit float, then mono, 22.05 kHz (soxr HQ), librosa 0.11
`pyin` (C1..C7, 2048-sample frames, hop 256: voicing and octave-robust f0) refined by
`yin` on the same frames where they agree within 50 cents (pyin's f0 is quantised to
10-cent bins). Compared frame by frame with the key-0 render: difference in cents minus
100 x key, over frames pyin calls voiced in both renders. Estimator octave errors
(|diff| within 100 cents of 1200) and gross errors (|diff| >= 600) are counted
separately and left out of the median and IQR. Timing: cross-correlation of the two
renders' 1 ms RMS envelopes (whole song, and the worst of the 10 s windows with signal).
100 renders, 12 in parallel, 528 s (`key_proof.py render`, then `report`).

Summary per song over the 24 shifted keys:

| song | voiced frames compared per key | median of (diff - 100k) | widest IQR | within 50 cents | octave errors (estimator) | envelope lag, whole song | worst 10 s window |
|---|---|---|---|---|---|---|---|
| Feel (Stripped), 48 kHz | 14,774-15,355 | -0.33 .. +0.37 c | -3.4 .. +4.2 c | 93.1-94.8 % | <= 1.9 % | -1.9 .. +2.9 ms | 13.9 ms |
| Crowdpleaser, 44.1 kHz | 10,492-10,848 | +0.57 .. +0.74 c | -2.9 .. +5.9 c | 90.3-95.3 % | <= 0.4 % | -0.8 .. +4.4 ms | 5.4 ms |
| The Homeless Man, 44.1 kHz | 12,688-13,174 | +0.39 .. +1.70 c | -5.1 .. +9.9 c | 85.0-89.2 % | <= 5.2 % | -1.2 .. +2.3 ms | 5.2 ms |
| demo song (synthetic), 44.1 kHz | 2,047-2,051 | -0.48 .. +1.30 c | -1.1 .. +2.5 c | 99.7-100 % | 0 | -1.1 .. +2.0 ms | 10.1 ms |

Per key: median of (diff - 100k) in cents [IQR] / envelope lag of the shifted render:

| key | Feel (Stripped) | Crowdpleaser | The Homeless Man | demo song |
|---:|---|---|---|---|
| -12 | +0.37 c [-3.3, +4.1] / -1.9 ms | +0.57 c [-2.8, +4.4] / -0.1 ms | +1.70 c [-5.1, +8.9] / -0.4 ms | +1.30 c [+0.4, +2.5] / -1.0 ms |
| -11 | +0.18 c [-3.4, +3.7] / -0.9 ms | +0.65 c [-2.4, +4.1] / -0.0 ms | +1.57 c [-4.7, +8.5] / -0.0 ms | +1.07 c [+0.3, +2.2] / +0.8 ms |
| -10 | +0.17 c [-3.2, +3.4] / -1.0 ms | +0.60 c [-2.1, +3.9] / +0.7 ms | +1.45 c [-4.4, +7.7] / +0.3 ms | +0.96 c [+0.2, +1.9] / +1.2 ms |
| -9 | +0.24 c [-2.6, +3.1] / -0.9 ms | +0.70 c [-1.7, +3.7] / -0.1 ms | +1.44 c [-3.9, +7.5] / +0.4 ms | +0.80 c [+0.1, +1.9] / +0.6 ms |
| -8 | +0.11 c [-2.7, +2.8] / -1.2 ms | +0.59 c [-1.8, +3.7] / -0.8 ms | +1.14 c [-4.0, +7.0] / -0.3 ms | +0.73 c [+0.0, +1.6] / +0.7 ms |
| -7 | +0.10 c [-2.4, +2.5] / -1.2 ms | +0.66 c [-1.5, +3.5] / -0.6 ms | +1.12 c [-3.7, +6.6] / -0.3 ms | +0.64 c [+0.0, +1.4] / +0.1 ms |
| -6 | +0.12 c [-2.1, +2.7] / -0.8 ms | +0.58 c [-1.3, +3.4] / -0.4 ms | +1.09 c [-3.2, +6.4] / +0.0 ms | +0.25 c [-0.4, +1.0] / +2.0 ms |
| -5 | +0.14 c [-2.0, +2.7] / -0.8 ms | +0.67 c [-1.1, +3.5] / -0.2 ms | +1.12 c [-2.8, +6.3] / +0.2 ms | +0.34 c [-0.2, +1.0] / +0.5 ms |
| -4 | +0.09 c [-1.9, +2.4] / -0.6 ms | +0.72 c [-0.9, +3.5] / -0.1 ms | +1.40 c [-2.4, +6.8] / +0.4 ms | +0.10 c [-0.4, +0.8] / +0.4 ms |
| -3 | +0.06 c [-1.7, +2.2] / -0.4 ms | +0.68 c [-0.7, +3.4] / +0.5 ms | +1.34 c [-2.2, +6.9] / +0.7 ms | +0.09 c [-0.4, +0.7] / +0.5 ms |
| -2 | +0.02 c [-1.6, +1.9] / -0.4 ms | +0.63 c [-0.7, +3.4] / +0.4 ms | +1.38 c [-2.1, +7.1] / +0.9 ms | +0.08 c [-0.4, +0.6] / +1.6 ms |
| -1 | +0.02 c [-1.5, +1.9] / -0.1 ms | +0.65 c [-0.6, +3.5] / +0.7 ms | +1.38 c [-1.8, +7.6] / +0.6 ms | -0.04 c [-0.4, +0.5] / +1.1 ms |
| +1 | -0.20 c [-2.2, +1.0] / -0.3 ms | +0.58 c [-0.7, +3.2] / -0.6 ms | +0.39 c [-3.6, +4.8] / -1.2 ms | -0.18 c [-0.6, +0.3] / -0.8 ms |
| +2 | -0.25 c [-2.4, +1.1] / -0.1 ms | +0.57 c [-0.7, +3.4] / -0.1 ms | +0.45 c [-3.5, +4.9] / -0.3 ms | -0.20 c [-0.7, +0.2] / +1.1 ms |
| +3 | -0.30 c [-2.5, +1.1] / +0.2 ms | +0.66 c [-0.8, +3.5] / +0.6 ms | +0.59 c [-3.0, +5.2] / +0.2 ms | -0.21 c [-0.7, +0.3] / +0.4 ms |
| +4 | -0.30 c [-2.7, +1.1] / +0.3 ms | +0.67 c [-0.9, +3.8] / +0.9 ms | +0.69 c [-2.9, +5.5] / +0.8 ms | -0.31 c [-0.9, +0.2] / +0.9 ms |
| +5 | -0.33 c [-2.7, +1.2] / +0.8 ms | +0.67 c [-1.0, +4.0] / +1.7 ms | +0.65 c [-3.0, +5.7] / +1.1 ms | -0.29 c [-0.8, +0.2] / +1.0 ms |
| +6 | -0.32 c [-2.6, +1.3] / +1.0 ms | +0.70 c [-1.2, +4.3] / +1.7 ms | +0.80 c [-2.6, +6.3] / +1.7 ms | -0.40 c [-1.0, +0.1] / -1.1 ms |
| +7 | -0.30 c [-2.6, +1.4] / +1.5 ms | +0.67 c [-1.4, +4.6] / +2.5 ms | +0.94 c [-2.7, +6.6] / +1.6 ms | -0.47 c [-1.0, +0.1] / +1.5 ms |
| +8 | -0.29 c [-2.6, +1.4] / +1.1 ms | +0.62 c [-1.5, +4.6] / +3.0 ms | +0.86 c [-2.8, +6.8] / +1.3 ms | -0.42 c [-1.0, +0.1] / +1.3 ms |
| +9 | -0.31 c [-2.7, +1.6] / +1.3 ms | +0.63 c [-1.7, +5.0] / +2.1 ms | +0.94 c [-3.0, +7.2] / +1.8 ms | -0.47 c [-1.1, +0.1] / +1.6 ms |
| +10 | -0.33 c [-2.8, +1.6] / +1.8 ms | +0.70 c [-1.9, +5.3] / +3.5 ms | +1.27 c [-2.8, +8.9] / +2.0 ms | -0.48 c [-1.1, +0.1] / +1.2 ms |
| +11 | -0.29 c [-2.9, +1.7] / +2.0 ms | +0.74 c [-2.0, +5.6] / +3.6 ms | +1.35 c [-3.0, +9.9] / +2.1 ms | -0.45 c [-1.1, +0.1] / +2.0 ms |
| +12 | -0.31 c [-2.9, +1.8] / +2.9 ms | +0.71 c [-2.2, +5.9] / +4.4 ms | +1.21 c [-3.3, +9.7] / +2.3 ms | -0.31 c [-1.1, +0.3] / +0.1 ms |

Reading:

- **Pitch.** At every key and on every song the shifted vocal sits on 100 x key cents
  above the original to within 1.7 cents (median), and half of all frames within 10
  cents. The engine's own error is far smaller than that (docs/benchmarks/dsp.md:
  <= 0.0015 cents on tones); what is left is the estimator measuring two different
  waveforms (Rubber Band changes the timbre, so pyin/yin's errors on breathy or noisy
  frames differ between the renders). That spread is the tracker's problem, not the
  key's, and it is small next to the grading tolerance (full credit within 50 cents).
- **Octave errors** appear only in the estimator, mostly on The Homeless Man at -12 to
  -9 and +6/+7 (<= 5.2 % of frames): a pitch shifter cannot jump an octave. A practice
  tracker must still be built to resist them (trap 8 and the tracker workstream).
- **Timing.** The shifted vocal stays aligned with the original: whole-song envelope lags
  -1.9 .. +4.4 ms, the worst 10 s window 13.9 ms. A small key-dependent lean (higher keys
  slightly later, at most +4.4 ms) is Rubber Band's envelope smearing at large shifts;
  dsp.md's impulse tests bound the graph's own alignment at 0.31 ms. Both are far below
  what can matter for grading notes that last hundreds of milliseconds.
- **Voicing.** With pyin's voicing probability above 0.5 required in both renders (the
  most confident frames only: 225-2,244 per key on the CC songs), the medians are
  -0.47 .. +1.60 cents and the widest IQR -1.8 .. +5.7 cents (`a/report-p0.5.txt`);
  The Homeless Man's estimator octave errors rise to 15.8 % at -12 there, which is why
  the main table uses pyin's own voicing decision.

The section 9 row "Key change" asks for the next step too: render at every key, feed the
app's own tracker and scorer as if from the microphone, grade against the shifted guide
(within a few points of key 0) and against a wrongly shifted guide (lowest band). That
needs the jk-pitch tracker, guide and scorer (Stages 2-3); the renders and their pitch
curves here show the input side of that test is sound.

## 4. Reproduce

```bash
cd jukeoke && source scripts/devenv.sh
cargo test --release -p jk-dsp --test key_switch -- --nocapture
cargo test -p jk-engine --test key_in_effect -- --nocapture
cargo test -p jk-engine --lib keyheard
cargo test --release -p jk-engine --test rt_alloc
cargo build --release -p jk-cli
cd "$JK_BUILD/practice-key"   # scripts live here, with the evidence
WORKERS=12 ../studio-venv/Scripts/python.exe -I key_proof.py render <jukeoke-cli.exe> a <song.jksong>...
../studio-venv/Scripts/python.exe -I key_proof.py report a          # pyin voicing
../studio-venv/Scripts/python.exe -I key_proof.py report a 0.5      # confident frames only
../studio-venv/Scripts/python.exe -I run_b.py <jukeoke-cli.exe> "$JK_BUILD" <song.jksong>...
../studio-venv/Scripts/python.exe -I remeasure.py <jukeoke-cli.exe> <song.jksong>...
```

New command-line tools: `jukeoke-cli key-switch-capture SONG --out DIR --mode
realtime|prerender --start S --changes 1.5:-3,4.5:4 --seconds 19` (plays nothing aloud:
null device) and `jukeoke-cli make-test-song OUT --tone 440` (the tone song).

## 5. Not covered here

- The owner's own song was not used: the key path does not depend on the material
  (2.1), and the guide workstream measures that song.
- No real device: the null device paces in real time like a sound card but reports a
  fixed 10 ms latency. A real device's reported latency is already subtracted by the
  playback clock (docs/benchmarks/playback.md); devices that hide part of their latency
  (WO2 4.8, the M1's built-in microphone side) are the microphone workstream's timing
  problem, not the key's.
- The Mac: the engine changes type-check for `aarch64-apple-darwin`; not run on a Mac.
- The UI does not use the new fields yet (Stage 1 settles the app API; Stage 5 wires key
  and octave end to end).

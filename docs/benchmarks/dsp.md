# DSP and audio engine measurements (Stage 0, experiment 2)

Measured on 2026-11-17 on the build PC (Windows 11, Ryzen 7 9700X, 32 GB, Realtek onboard
audio), Rust 1.100 `x86_64-pc-windows-gnu`, WinLibs GCC 16.2. Every number below was
produced by a test or a command listed in "How to reproduce"; nothing here is a listening
judgement.

Engine under test:

- **Rubber Band Library 4.0.0** (tag `v4.0.0`, commit `1d95888b`, GPL), vendored in
  `crates/jk-rubberband/vendor/`, single-file build, **built-in FFT and built-in resampler on
  every platform** (no vDSP on macOS), `-O3 -ffp-contract=off`, no fast-math.
  Real-time stretcher, `OptionEngineFiner` (R3) | `OptionProcessRealTime` |
  `OptionChannelsTogether` | `OptionThreadingNever` | `OptionPitchHighQuality`, formants
  shifted (default), standard window.
- jk-dsp graph: pitch bus = lead x guide + backing + other; drum bus delayed by the graph
  latency `L` = 4608 frames at 44.1/48 kHz (104.5 ms at 44.1 kHz), 9216 at 88.2/96 kHz;
  bit-transparent bypass at 0 semitones; 40 ms equal-power crossfade on key change; 20 ms
  gain ramps.

## Summary

| Check | Result | Criterion |
|---|---|---|
| Pitch accuracy, every step -12..+12, sines 110/440/1760 Hz + 6-partial 220 Hz tone | **max 0.0015 cents** | < 5 cents |
| Output length | **exactly the input length** in every case (100 renders) | equal |
| Clipping | max output peak **-3.98 dBFS** for -6.02 dBFS input (worst: 1760 Hz at +12) | < 0 dBFS |
| Tone-burst onset position (2 ms attack) | 44.1 kHz: within +-26 samples (0.6 ms) at +-2 st; worst 212 samples (4.8 ms), harmonic tone at -12 | < 10 ms |
| Drum path impulse offset | **0 samples, every impulse** (44.1, 48, 96 kHz) | 0 |
| Band (shifted) vs drum impulse offset, mean of 64 impulses | 44.1 kHz: 17 of 24 shifts within +-3 samples, worst +13.6 samples (0.31 ms, at -4); 48 kHz worst 7.1 (0.15 ms); 96 kHz worst 14.1 (0.15 ms) | mean within 0.5 ms |
| Band impulse jitter (single impulse, depends on hop-grid position) | +-7 samples at +-1 st, +-13 at +-2, up to +-89 at -12 (44.1 kHz) | reported |
| Transparency at 0 semitones | max \|output - plain mix\| = **0** (bit-exact) | 0 |
| Key change 0 -> +2 -> -3 -> 0 during playback | largest step 1.31x, largest curvature (click detector) 1.26x the signal's own | < 1.6x / < 2x |
| Block-size independence (64, 333, 512, 1024, 4096 frames; 44.1/48/96 kHz) | **bit-identical** | identical |
| Determinism (same render twice) | **bit-identical**; golden SHA-256 recorded | identical |
| Cross-platform tolerance (defined) | max \|diff\| 1e-5 and RMS diff -110 dB; measured rounding sensitivity 1.4e-7 / -137.7 dB | - |
| R3 CPU, stereo, one core of the 9700X | RTF 0.109-0.163 (44.1 kHz), 0.116-0.170 (48 kHz) | reported |
| Output resampler THD+N (100 Hz-18 kHz) | **-133.8 dB** worst; gain at 20 kHz -0.006 dB | < -100 dB |
| FLAC 24-bit decode | bit-exact; 49 seeks per file sample-accurate | exact |
| 60 s device playback, 14 busy threads, key change every 10 s | **0 underruns**, longest callback 10.8 us, worker lead never below 137.6 ms | 0 |
| Clock vs wall time (60 s) | trend -20.4 ppm (= device clock -21.0 ppm vs QPC), jitter +-0.2 ms | reported |
| Static runtime | exe imports only Windows DLLs | no MinGW DLLs |

## 1. Key change accuracy

`cargo test -p jk-dsp --test key_change` (all 25 steps x 4 tones, 3 s each, tone in the
"other" stem so it goes through the pitch bus; 44.1 kHz stereo; input peak -6.02 dBFS).
Pitch = frequency of the strongest component near the target, measured over 1.0-2.0 s by
maximising a Blackman-Harris-windowed DTFT (golden-section search to 0.0001 cent).
Onsets = 50 % point of the 1 ms RMS envelope of three 0.4 s bursts with a 2 ms attack,
output minus input.

| Tone | max \|pitch error\| (cents) | onset error range (samples) | output peak range (dBFS) |
|---|---|---|---|
| sine 110 Hz | 0.0002 | -67 .. +105 | -6.02 .. -5.86 |
| sine 440 Hz | 0.0000 | -66 .. +116 | -6.02 .. -5.75 |
| sine 1760 Hz | 0.0015 | -30 .. +70 | -6.02 .. -3.98 |
| harmonic 220 Hz | 0.0001 | -122 .. +212 | -8.74 .. -6.37 |

Onset error grows with the size of the shift (at +-2: at most 26 samples = 0.6 ms; at +-12:
up to 212 samples = 4.8 ms for the harmonic tone). Shifting up makes a phase vocoder place
onsets early, shifting down late; the transient trim (section 2) is calibrated on impulses
and removes most, not all, of that for soft tonal attacks. The pure-tone peak rises by up to
2 dB at large upward shifts (1760 Hz at +12: -3.98 dBFS): keep stem headroom.

Every output has exactly the input's sample count (the renderer removes the latency).

## 2. Drum alignment (WORK_ORDER trap 6)

`cargo test -p jk-dsp --test graph_props drum_alignment_impulse`: 64 isolated impulses
(amplitude 0.5, 8192 samples apart, sub-positions stepping 29 samples mod 1024 so every
position of Rubber Band's hop grid is hit) rendered once through the backing stem (pitch
path) and once through the drum stem (unshifted path).

- **Drum path: every impulse lands on its input sample (offset 0) at 44.1, 48 and 96 kHz.**
  The drum delay equals the graph latency `L` exactly.
- **Shifted band path:** a phase vocoder cannot put an isolated transient on one exact
  sample: where the impulse lands depends on its position relative to the analysis hop grid.
  Single-impulse offsets, 44.1 kHz: +-7 at +-1 st, -11..+13 at +2, -38..+41 at +12, -89..+88 at
  -12. Uncorrected, R3 also has a pitch-dependent mean bias (shifting up = early): -134
  samples at +12, -19 at +2, +6 at -1, +29 at -12 (44.1 kHz); at 96 kHz +8..+12 come 664 to
  902 samples (7-9 ms) early. jk-dsp removes the measured mean bias per key
  (`TRIM_FFT_4096`, `TRIM_FFT_8192` in `crates/jk-dsp/src/shifter.rs`, calibrated with
  `examples/calibrate_trim.rs` on two independent sets of 256 impulses, strides 17 and 23).
- Validation on a third set (stride 29), mean band-drum offset after the trim:

| rate | shifts tested | worst \|mean\| | where | most shifts |
|---|---|---|---|---|
| 44.1 kHz | all 24 | 13.56 samples (0.31 ms) | -4 st | 17 of 24 within +-3 samples |
| 48 kHz | -12 -7 -5 -2 +2 +5 +7 +12 | 7.05 samples (0.15 ms) | -7 st | +2: +0.03 |
| 96 kHz | same | 14.14 samples (0.15 ms) | -12 st | +12: -0.30 |

The residual mean differs between position sets for a few shifts (-4, -7, -9 at 44.1 kHz):
R3's transient handling is bimodal there. 0.3 ms is far below the ~5 ms where a flam is
usually heard, but this is a listening-checklist item (drums vs band at +-5 and +-12).

Why the graph uses a fixed-pitch Rubber Band instance per key (not `setPitchScale` on a
running instance): R3's start delay depends on the pitch scale (HighQuality, 44.1/48 kHz:
`delay = 2048` frames for downward shifts and `2048 / scale` for upward ones, e.g. 1024 at
+12; HighConsistency: `2048 / scale` always, 4096 at -12), so changing the pitch of a running
stretcher moves the band against the unshifted drums. The measured output lag in streaming
use (frames of input fed beyond the newest aligned output) is at most 4041 (HighQuality) and
6213 (HighConsistency, at -12) at 44.1/48 kHz; `L` = 4608 covers HighQuality plus the trim
plus the 256-frame process quantum. No shortfall occurred in any test.

## 3. Transparency at 0 semitones

`transparent_at_zero_semitones` (6 s synthetic song, three gain settings, with and
without "shift drums"): max |output - (lead*g + backing*g + other*g) + drums*g| = **0**.
Summing the same stems in a different order differs by up to 5.96e-8 (one float ulp), which
is why the reference uses the graph's summation order.

## 4. Key change during playback

`key_change_during_playback_is_click_free`: 440 Hz sine at -6 dBFS, key 0 -> +2 (1 s) ->
-3 (2 s) -> 0 (3 s). d1 = largest |x[n]-x[n-1]|, d2 = largest |x[n]-2x[n-1]+x[n-2]| (a step
or kink shows up as a d2 many times the sine's own), within +-10 ms around each 40 ms
crossfade, versus the steady signal at the higher of the two keys.

| change | d1 ratio | d2 ratio | 10 ms RMS during the crossfade |
|---|---|---|---|
| 0 -> +2 | 1.314 | 1.256 | -3.90 .. +1.95 dB |
| +2 -> -3 | 1.234 | 1.135 | -0.69 .. +0.91 dB |
| -3 -> 0 | 1.257 | 1.193 | -1.93 .. +1.42 dB |

No click (a click gives d2 ratios of 10x and more). The ratios above 1 and the RMS ripple
are the two crossfaded tones beating (440 vs 494 Hz) during the 40 ms equal-power fade.
Each change uses a fresh Rubber Band instance primed with the 4096 frames of real audio
before the change; the crossfade starts at the requested frame of the output (heard after
`L`, like a gain change). The 69 ms worker block seen with an earlier design that primed
`L + 8192` frames to crossfade at once dropped to 16 ms (section 8).

## 5. CPU cost (real-time factor)

`jukeoke-cli bench --seconds 30` (release build), 30 s of synthetic stereo music, one core,
256-frame `process()` calls. RTF = processing time / audio duration.

| | -12 | -5 | -2 | +2 | +5 | +12 |
|---|---|---|---|---|---|---|
| R3 alone, 44.1 kHz | 0.1505 | 0.1257 | 0.1124 | 0.1086 | 0.1282 | 0.1627 |
| R3 alone, 48 kHz | 0.1620 | 0.1282 | 0.1177 | 0.1160 | 0.1375 | 0.1696 |

Full graph (offline render): 0 st RTF 0.0005 (Rubber Band idle); +3 st RTF 0.119 (44.1 kHz),
0.125 (48 kHz); +3 st with the 44.1 -> 48 kHz resampler 0.132. During live playback the
render worker used 18.5 % of one core with 14 busy threads competing (section 8).
The C++ is built for baseline x86-64 (SSE2, no AVX), the same code path class as ARM64
NEON; on the M1 expect a similar order of magnitude (not measured).

## 6. Determinism and the cross-platform golden files

`cargo test -p jk-dsp --test golden`:

- Rendering the same input twice gives byte-identical output (both cases).
- Golden renders (Windows x86-64 GNU reference), little-endian f32 interleaved stereo in
  `crates/jk-dsp/tests/golden/`:
  - `render_a.f32` (44 100 frames, 44.1 kHz, keys 0 -> +3 -> -2 with shift drums -> 0 and a
    gain ramp): SHA-256 `75e62952208a30caece35b46f55ca5167324578e949c52df9b1e6adacbda9000`
  - `render_b.f32` (24 000 frames, 44.1 -> 48 kHz through the resampler, +2):
    SHA-256 `1820ed99e80874004ca96decb6ec2df071c4b35cc2a2949a7966087085146248`
- Inputs are generated with integer PRNGs and the pure-Rust `libm` crate, so both platforms
  start from bit-identical stems; the crossfade table also uses `libm`.
- The pitch scale handed to Rubber Band comes from a table of correctly rounded `2^(k/12)`
  values (`jk_rubberband::semitones_to_scale`), not from the platform `pow`: measured on
  Windows, `2f64.powf(k/12)` is one ulp off at +8 and +11 (and `libm::pow` at other keys),
  so a `pow` call could feed different bits to Rubber Band on Windows and macOS. The golden
  files did not change (they use +3, -2 and +2, where the old values were already exact).
- Limit of the tolerance basis: the x87 perturbation changes arithmetic rounding only; it
  does not exercise differences between the platforms' `sin`/`cos`/`atan2`/`exp` (Rubber
  Band calls the system maths library) or a flipped threshold decision inside R3. Only the
  macOS CI run can show those.
- **Tolerance (Windows vs macOS): every sample within 1e-5 absolute (-100 dBFS) and RMS
  of the difference at most -110 dB relative to the signal.** On the reference platform
  the hash must match exactly. Basis: rebuilding Rubber Band with x87 80-bit maths
  (`JK_RUBBERBAND_PERTURB=1`, a far larger rounding change than the last-bit libm
  differences expected between the platforms) moved the output by at most 1.42e-7, RMS
  -137.7 dB (render_a; render_b 1.5e-8, -185 dB). The limit leaves about 70x margin and would
  still catch a real divergence (another FFT, a different algorithm path).
  (An FMA build was tried first; `-mfma` crashes MinGW builds, see section 10.)
- `jukeoke-cli ci-render OUT` writes 18 cases (13 keys, shift drums, a key-change sequence,
  resampled, a 48 kHz song) as 32-bit float WAV for `scripts/ci-compare-renders.sh`; two runs
  on Windows: 18 files, max difference 0.

## 7. Output resampler (only when device rate != song rate)

rubato 5.1 asynchronous sinc, 256 taps, Blackman-Harris^2, 256x oversampling, cubic
interpolation, automatic cutoff; fixed ratio. `cargo test -p jk-dsp --test resampler`
(sines at -6.02 dBFS, least-squares fit at the known frequency over 1 s):

| conversion | gain 100 Hz-18 kHz | gain at 20 kHz | THD+N 100 Hz-18 kHz |
|---|---|---|---|
| 44.1 -> 48 kHz | -0.0001 dB | -0.0017 dB | -134.5 .. -135.7 dB |
| 48 -> 44.1 kHz | +0.0000 dB | -0.0055 dB | -133.8 .. -134.8 dB |
| 44.1 -> 96 kHz | -0.0001 dB | -0.0017 dB | -134.8 .. -135.6 dB |
| 48 -> 96 kHz | -0.0001 dB | -0.0001 dB | -137.6 .. -143.4 dB |

Output length is exactly round(len x ratio); an impulse at 44.1 kHz frame 20 000 peaks at
48 kHz frame 21 768 (exact position 21 768.71) after removing the 139-frame delay.

### Every place a resample happens

1. **Inside Rubber Band, as part of the pitch shift itself** (R3 stretches by the pitch
   ratio and resamples with its built-in BQResampler, "FastestTolerable" quality as R3
   chooses: 90 dB Kaiser stop band, 0.975 cutoff). Only while the key is not 0. At 0 the
   bypass path has no resampling at all.
2. **jk-dsp `OutputResampler` (rubato)**, after the mix, only when the device rate differs
   from the song rate (e.g. 44.1 kHz song on a 48 kHz shared-mode device). Not used in
   WASAPI exclusive mode when the device accepts the song rate (measured: Realtek accepted
   44.1 kHz, no resampling).
3. **Not in WASAPI shared mode**: we open the stream in the device mix format and rate
   (48 kHz f32 here), so the Windows audio engine does not convert.
4. **macOS (not yet run)**: the stream is opened at the device's nominal rate, so Core
   Audio should not convert; to be confirmed on the Mac.

## 8. Device playback (WASAPI)

> Stage 0 prototype (`jk_engine::Player`, a stem folder). The prototype player was
> replaced in Stage 1 by the engine API (`jk_engine::Engine`, `.jksong` files, in-place
> seek, pre-built Rubber Band instances); its long-run, memory and seek measurements are
> in `docs/benchmarks/playback.md`. The numbers below are kept as the Stage 0 record.

`jukeoke-cli play --stems <synth-44100> --seconds 60 --master-db -60 --semitones 3
--load-threads 14 --key-cycle 10` (90 s generated 24-bit FLAC stems at 44.1 kHz; master
-60 dB, so the peak output was about -62 dBFS; key changes every 10 s: +3, -2, +5, 0, -7):

- Device: Realtek "Speakers", WASAPI shared, 48 000 Hz, 2 ch, f32, endpoint buffer 1056
  frames (22.0 ms), reported stream latency 0 frames; resampling 44.1 -> 48 kHz.
- **Underruns: 0** (6006 device callbacks), longest callback 10.8 us.
- Render worker (MMCSS "Pro Audio"): 18.5 % of one core, longest block 16.0 ms, output
  lead never below 137.6 ms (target 150 ms), stem reader never starved, Rubber Band
  shortfall 0. Busy threads completed 9.57 M work units (the CPU was loaded).
- Playback clock (frames delivered - reported latency) vs wall clock: trend -24.3 ppm
  with +-5.2 ms steps (the raw clock advances one 10 ms device period at a time);
  interpolated between callbacks as the UI does: end offset -1.24 ms, trend -20.4 ppm,
  jitter -0.21 .. +0.16 ms. The device's own clock (IAudioClock position vs the OS
  performance counter) ran -20.97 ppm: the playback clock follows the DAC, as it must.
- Same test with the earlier "crossfade immediately" design: also 0 underruns, but the
  longest worker block was 68.7 ms and the lead dropped to 84.6 ms, which is why the
  transition was redesigned (section 4).

WASAPI exclusive mode, 5 s, digital silence, key -2: 44 100 Hz, 2 ch, i32 with 24 valid
bits, 432-frame buffer (9.80 ms), reported latency 9.80 ms, no resampling, 0 underruns,
device clock -21.62 ppm.

Independent re-run (verifier, same day, master -70 dB, otherwise the same command): 0
underruns in 6002 callbacks, longest callback 5.4 us, worker 18.65 % of one core, longest
block 19.3 ms, lead never below 137.6 ms, interpolated clock -20.8 ppm, device clock
-20.99 ppm. The `underruns` counter only sees an empty output ring; a device thread that
wakes too late is now counted separately (`Clock::device_starved`: shared-mode events that
found the endpoint buffer empty). That re-run: **0**. (Not measurable in exclusive event
mode, where WASAPI always reports the whole buffer free.)

## 9. FLAC stems: decode and seek

`cargo test -p jk-engine --test decode_seek`: 24-bit FLAC (stereo 300 000 frames, mono
123 457 frames) decodes bit-exactly (every sample == integer / 2^23); 49 seeks per file
(block starts, block interiors, the last frames, 40 random positions), each followed by a
5000-frame comparison against the full decode: all identical. Encoder edge cases (digital
silence, a 100-frame file, full-scale alternating samples, 16-bit) round-trip exactly.

The FLAC writer is our own (`crates/jk-engine/src/flac_write.rs`): the pure-Rust `flacenc`
0.5.1 caps the Rice parameter at 14 (no RICE2), so loud 24-bit material produced ~100
bits/sample frames (a 90 s stereo mix became 24.4 GB) that symphonia then could not read.
Since Stage 1 it streams (constant memory), writes the STREAMINFO MD5 (jk-format's
verifier requires it), a SEEKTABLE with a point every second, and codes "wasted bits"
(samples with trailing zero bits, e.g. 16-bit material on the 24-bit grid).

**symphonia FLAC seek bug (found in Stage 1, worked around).** symphonia-bundle-flac 0.6.1
keeps its packet parser's state across a seek when the seek lands exactly on a frame
start (its `resync` soft-resets the parser only if the read position moved; a SEEKTABLE
point or the first frame makes it land exactly). The first packet after such a seek is
right; the following ones are assembled from stale fragments and decode to wrong samples
without any error. Measured on the demo song's `other` stem: after reading 40 000 frames,
`seek(0)` gave 45 900 wrong samples in the next 50 000 (first wrong sample at frame 4096,
the second packet). Work-around: `FlacStem` rebuilds its demuxer and decoder from a fresh
source on every seek (`FlacStem::from_factory`; about 1 KB of metadata re-read).
Regression test: `decode_seek::long_spans_after_seeks_on_compressible_audio` (60 000
frames compared after each of 9 forward and backward seeks, seek table present). Checks
that compare only the first packet after a seek (jk-format's `verify` compares 64 frames)
cannot see this bug.

## 10. Build checks

- **Static runtime** (`objdump -p jukeoke-cli.exe | grep "DLL Name"`): KERNEL32, msvcrt,
  ntdll, bcryptprimitives, api-ms-win-core-synch-l1-2-0, avrt, combase, ole32, oleaut32,
  propsys. No libstdc++-6, libgcc_s_seh-1 or libwinpthread-1. `+crt-static` plus a static
  `libstdc++.a` from `crates/jk-rubberband/build.rs`.
- Do not build the C++ with `-mavx`/`-mfma`/`-march=native` under MinGW: the FMA
  perturbation build crashed with an access violation (MinGW GCC cannot align the stack for
  256-bit spills, GCC bug 54412).
- **`cargo check --target aarch64-apple-darwin -p jk-dsp -p jk-rubberband -p jk-engine -p
  jk-cli` passes** (also with `--tests --examples`, and clippy is clean for both targets).
  What it proves: all Rust code, including the cfg(macos) Core Audio backend (cpal 0.16 /
  objc2 bindings; pinned below 0.17 because cpal 0.17+ links the macOS 14.2 process-tap
  API and its README gives macOS 14.2 as the backend's minimum) and the macOS QoS call, type-checks against the aarch64-apple-darwin
  standard library and dependency APIs. What it does **not** prove: the C++ was not compiled
  for macOS (build.rs skips it on a Windows host with a warning), nothing was linked, no
  Apple SDK headers were checked, no deployment-target (13.0) check of a binary, and nothing
  ran. The Mac numbers (golden comparison, CPU, playback) must come from the macOS CI job or
  the MacBook.

## 11. Rubber Band behaviour found and worked around

| Finding | Evidence | Work-around |
|---|---|---|
| `reset()` leaves R3 state (output phase, classification, guidance): a used-then-reset stretcher differs from a fresh one by up to 0.25 | `cargo run --release -p jk-rubberband --example probe_reset` | new stretcher per key change (2.4 ms to create + drop at 44.1 kHz, 3.6 ms at 96 kHz; `probe_create`) |
| Constructing an instance allocates (about 2.4 ms of work) | `probe_create`, `jukeoke-cli bench` | Stage 1: built on the engine's house-keeping thread and handed to the render worker through an SPSC ring (`PitchGraph::offer_stretcher`); replaced instances travel back the same way to be freed; the worker never constructs or frees one (counter `inline_constructions` = 0 in every engine test and run). A key change waits for a spare (`deferred_blocks`) rather than build one inline |
| Real-time R3 output depends on `process()` chunking for non-power-of-two ratios (max diff 0.34-0.52 between 64- and 512-frame calls at -7 and +3; identical at 0 and +12) | `probe_chunking` | always 256-frame `process()` calls; output then bit-identical for any caller block size |
| Start delay depends on pitch | `probe` | one fixed-pitch instance per key, prime + crossfade |
| Transient bias per pitch (up to 7-9 ms at 96 kHz +8..+12) | `probe_jitter`, `calibrate_trim` | per-key trim tables (44.1/48 and 88.2/96 kHz measured; other rates scaled, unverified) |
| `getStartDelay`/`getPreferredStartPad` differ between HighQuality and HighConsistency | `probe` | HighQuality (fixed pitch per instance; smaller latency for downward shifts) |

## How to reproduce

```bash
source scripts/devenv.sh
cargo test -p jk-rubberband -p jk-dsp -p jk-engine            # all tests (debug, ~1 min)
cargo test --release -p jk-dsp                                 # same numbers, faster
# numbers are written to target/tmp/jk-dsp-results/*.txt
cargo run --release -p jk-cli -- gen-test-stems --seconds 90   # -> $JK_BUILD/fixtures/synth-44100
cargo run --release -p jk-cli -- bench --seconds 30
cargo run --release -p jk-cli -- make-demo-song "$JK_BUILD/demo.jksong"
cargo run --release -p jk-cli -- play "$JK_BUILD/demo.jksong" \
    --seconds 60 --silence --loop --key-every 10 --load-threads 14   # see playback.md
cargo run --release -p jk-cli -- ci-render out/render-windows
cargo run --release -p jk-dsp --example calibrate_trim 44100 17 256   # trim table
JK_RUBBERBAND_PERTURB=1 JK_GOLDEN_STRICT=0 CARGO_TARGET_DIR=target-perturb \
    cargo test --release -p jk-dsp --test golden                       # tolerance basis
objdump -p target/release/jukeoke-cli.exe | grep "DLL Name"
cargo check --target aarch64-apple-darwin -p jk-dsp -p jk-rubberband -p jk-engine -p jk-cli
```

# Playback engine benchmarks (Stage 1 + 4)

Measured on 2026-11-17 on the build machine: AMD Ryzen 7 9700X (8 cores, 16 threads),
32 GB, Windows 11 Home 26200, Realtek "Speakers" (WASAPI shared mode, 48 000 Hz, 2 ch,
f32, endpoint buffer 1056 frames = 22.0 ms, reported stream latency 0 frames). Release
build (`x86_64-pc-windows-gnu`). Every device test plays **digital silence** (master
volume 0: the whole pipeline runs, nothing is audible). Songs are generated, no
copyrighted material:

- the **demo song** (`tests/fixtures/generated/demo-song.jksong`: 40 s, 44.1 kHz, mono
  stems; on this 48 kHz device every run also goes through the 44.1 -> 48 kHz output
  resampler);
- a **4-minute song** (`jukeoke-cli make-test-song song4min.jksong --seconds 240`: 4
  stereo 24-bit stems at 44.1 kHz, 103 MB file).

Sections 11 (real-time allocation test) and 12 (20 minutes in the real app, whole-app
memory) were added on 2026-11-18. **Nothing here has run on macOS.** The Core Audio backend type-checks for
`aarch64-apple-darwin` (section 8). Every Mac number has to come from the macOS CI job or
the owner's MacBook.

## 1. What the engine is (to read the numbers)

- **Threads**: stem reader (decodes FLAC from the song file into a ~4 s ring), render
  worker (MMCSS "Pro Audio": graph + resampler, keeps the output ring 150 ms ahead,
  measures peak and clipping), device callback (copies, fades, master volume, clock),
  house-keeping thread (builds Rubber Band instances, frees replaced objects, turns flags
  into events). The worker never allocates or frees; the callback never allocates, locks
  or logs.
- **Segments**: load, seek and device switch start a new *generation*. The worker writes
  a `Start` marker before a segment's first frame and an `End` marker after the song's
  last frame; the callback fades the old audio out (10 ms), drops the rest of the old
  generation, waits until 50 ms of the new one is buffered and fades it in (10 ms). The
  device stream never stops for a seek, a pause or a song change.
- **Clock** (`Snapshot::position_s`): the callback logs what it wrote (song audio from
  position p, or silence while paused at p) and walks back by the latency the device
  reports (frames queued + stream latency): the song time of the sample leaving the DAC.

## 2. 20-minute run under load

Command (`$JK_BUILD/engine-work/run20.log`, `run20.json`):

```
jukeoke-cli play demo-song.jksong --seconds 1200 --silence --loop \
    --key-every 30 --seek-every 120 --load-threads 14 --report run20.json
```

1200 s of playback, the demo song looped (28 times), a key change every 30 s (40 changes
through +3, -2, +5, 0, -7, +12, -12, +2), a seek to a pseudo-random position every 120 s
(10 seeks), 14 busy-loop threads on the 16-thread CPU (209.8 M work units done), WASAPI
shared mode 48 kHz with 44.1 -> 48 kHz resampling.

| Measure | Result |
|---|---|
| Underruns (callback found the output ring empty) | **0** (120 001 callbacks, 0 frames of silence inserted) |
| Device starved (WASAPI endpoint buffer found empty: device thread late) | **0** |
| Render worker lead (output ring ahead of the device, target 150 ms) | **min 131.6 ms** |
| Render worker | 20.6 % of one core, longest block 26.9 ms (preempted by the load threads; the lead absorbs it) |
| Stem reader starved / Rubber Band shortfall | 0 / 0 frames |
| Rubber Band instances built on the render thread | **0** (key changes that waited for a pre-built one: 0 blocks) |
| Longest device callback | 109.3 us |
| Playback clock (song time at the DAC) vs the OS clock | **-20.92 ppm** (weighted mean over 38 continuous segments, 1176 s; max \|drift\| 24.5 ppm per segment); jitter around the trend -0.96 .. +0.19 ms |
| Device clock (IAudioClock position vs QueryPerformanceCounter) | -20.82 ppm |
| Peak working set of the process | 49.4 MB (private bytes at the end 53.2 MB) |

Reading the clock line: the playback clock follows the sound card's crystal, which runs
20.8 ppm slow against the OS counter (25 ms over 20 minutes). The lyrics follow the
playback clock, i.e. the audio itself, so this does not shift lyrics against the song;
it only means song time and wall time differ by that much. The two independent
measurements agree to 0.1 ppm.

(The run was made with the final engine code except two later edits that do not touch
the audio path: the house-keeping thread's status check now takes its lock once instead
of three times, and the Flac24 export renders once into a temporary file. All tests were
re-run on the final code.)

## 3. Memory: a 4-minute, 4-stem, 44.1 kHz stereo song

`jukeoke-cli play song4min.jksong --seconds 120 --silence --key-every 15 --seek-every 25`
(8 key changes, 4 seeks): **peak working set 49.8 MB**, private bytes 54.6 MB at the end;
0 underruns, min lead 140.0 ms, worker 10.3 % of one core. For comparison, decoding the
four stems into memory as float would take 339 MB; the engine streams them (a ~4 s stem
ring per segment, 5.6 MB for this song). The demo song peaks at 49.4 MB (section 2), so
the figure is dominated by fixed costs (runtime, Rubber Band instances, rings), not by
song length. Target was "well under 300 MB" for the engine process; measured on Windows
only.

## 4. Seek: accuracy and latency

**Accuracy** (`jukeoke-cli seek-test`, null device at the song rate, every played buffer
captured, compared with an offline render of the whole song):

| Case | Demo song (20 seeks) | 4-minute song (12 seeks) |
|---|---|---|
| Key 0, full mix | **20/20 bit-exact** for 20 000 frames after the 10 ms fade-in | **12/12 bit-exact** |
| Key +3, drums only (the unshifted drum path) | **20/20 bit-exact** | **12/12 bit-exact** |
| Key +3, full mix | 1/20 bit-exact, the others not; residual at the best alignment -2.2 dB (median), +3.1 dB (worst) relative to the signal | 0/12 bit-exact |

So a seek lands on the exact sample, and the 10 ms fade-in is exactly `(k+1)/441` per
frame (checked in `tests/engine.rs`). With a key shift, the shifted band after a seek is
not sample-identical to an uninterrupted render: the restarted Rubber Band instance
starts its phase accumulation afresh (primed with 4096 frames of the real audio before
the seek point, the same as at every key change), so each partial's phase differs and
the waveforms do not line up sample by sample. Its timing goes through the same graph
latency as the drums (impulse-tested in `docs/benchmarks/dsp.md`). Whether a seek at
a shifted key sounds identical is on the listening checklist.

`cargo test -p jk-engine --test engine seek_is_in_place_and_sample_accurate` repeats the
two exact cases with 10 seeks **while playing** and also checks that the device kept
calling back throughout (the stream is not restarted).

**Latency** (same command, real device, 20 seeks while playing, digital silence):

| | Demo song | 4-minute song |
|---|---|---|
| `seek()` call -> the callback adopts the new segment | median 39.8 ms (36.1 .. 40.8) | median 40.0 ms (39.0 .. 40.6) |
| `seek()` call -> the new position is at the DAC (what the listener hears) | median 49.2 ms (36.1 .. 50.5) | median 40.2 ms (39.0 .. 50.5) |
| Underruns during the test | 0 | 0 |

The ~40 ms is: 10 ms fade-out of the old audio, the decoder seek and graph prime (Rubber
Band primed with 4096 frames of real audio), 50 ms of buffered audio of the new segment
(rendered faster than real time), and up to one device period.

## 5. Real-time output = offline render; key changes

`cargo test --release -p jk-engine --test engine` (null device, "fast" mode, every buffer
captured):

- **Real-time output equals the offline render**: the whole demo song played through the
  engine (reader thread, rings, worker, filler) is **bit-identical** to `jk_dsp::render`
  on the fully decoded stems, at key 0 and at key +4: 1 763 559 frames each after the
  10 ms fade-in.
- **Key changes**: 8 changes while playing; 0 Rubber Band instances constructed on the
  render thread, 0 shortfall frames. The house-keeping thread builds each instance (2.80
  ms per construct + drop at 44.1 kHz, `jukeoke-cli bench`) and hands it over through an
  SPSC ring; replaced instances go back the same way to be freed.
- **Peak and clip**: the default demo mix peaks below full scale with no clip warning;
  levels at 4x drive the output above full scale, which is reported (`clipped`, peak
  > 1.0) and **not limited**; lowering the master clears the warning; no meter while
  paused (`output_peak_and_clip_warning`).
- **Forward compatibility**: a song with only newer roles plays `bass` (pitched, in the
  mix) at unity with its stored gain, exactly; `shaker` (not pitched) at +5 unshifted
  (max difference < 1e-6); `click` (not in the karaoke mix) not at all.
- **Device loss** (simulated: the device thread fails through the same path as an
  invalidated WASAPI device): the engine pauses, keeps the position, emits `DeviceLost`;
  `play()` reopens the device and continues.

## 6. Export ("Export backing track")

`cargo test --release -p jk-engine --test engine export` on the demo song:

- **Wav32f** is **bit-identical** to `jk_dsp::render` on the fully decoded stems at keys
  0, +3 and -5 (1 764 000 frames each). Never scaled.
- **Flac24** decodes to the render x the applied gain within **half an LSB of 24 bits**
  (max error 5.96e-8 = 0.5 / 2^23): normal mix peak 0.441, applied gain 0.000 dB; a "hot"
  mix (levels 4x) peaks at 1.961, so the whole file is scaled by **-5.849 dB** and the
  report says so. Every Flac24 export is decoded back and its audio MD5 checked before
  the file is renamed into place.
- Cancelling removes the temporary files and returns `cancelled`.

Speed on the 4-minute song: Flac24 at key 0 in 2.13 s (113x real time); key +3 in 28.8 s
(8.3x real time: Rubber Band R3 costs ~0.12 of real time per core). The 103 MB song gives
a 38.7 MB Flac24 file (key 0) and an 84.7 MB Wav32f file. The Flac24 export renders once
into a temporary 32-bit float file next to the output (85 MB for 4 minutes), then encodes
it; rendering twice would double the time at a key other than 0.

## 7. Devices (real hardware)

`cargo run --release -p jk-engine --example device_switch -- tests/fixtures/generated/demo-song.jksong`
(digital silence, playing at +2):

| Step | Result |
|---|---|
| Default device, shared | Speakers (Realtek(R) Audio), 48 000 Hz, latency 22.0 ms |
| `set_device(None, exclusive = true)` | OK in 71 ms; **44 100 Hz** (the song rate: no resampling), latency 19.6 ms; position continued (5.948 -> 6.908 s one second later) |
| back to shared | OK in 140 ms; 48 000 Hz |
| `set_device("null")` | OK in 40 ms; the null device |
| back to the default | OK in 140 ms |
| simulated device loss | paused 11 ms later at the position it had, `DeviceLost` emitted, device closed |
| `play()` | reopened the default device and continued |
| Underruns over the whole sequence | 0 |

`jukeoke-cli devices` lists the active WASAPI endpoints with friendly names, endpoint ids,
mix format and whether exclusive mode is accepted (here: one Realtek output, exclusive
yes).

## 8. Not verified

- **macOS**: not compiled with the Apple SDK, not run. `cargo check` and `cargo clippy
  --target aarch64-apple-darwin -p jk-dsp -p jk-rubberband -p jk-engine -p jk-cli --tests
  --examples` pass (type-check only; jk-rubberband's C++ is skipped on a Windows host).
  The Core Audio backend (cpal 0.16), its device list (ids are device names), its
  device-loss path (`StreamError::DeviceNotAvailable`) and every number above are
  unmeasured on a Mac.
- **A real unplug** was not tested (no one to unplug a device); device loss was
  simulated through the device thread's own failure path. A Windows default-device
  change while playing on "the default" is not followed (playback stays on the device it
  opened until the next `set_device`/reopen).
- **Bluetooth outputs**: the clock subtracts whatever latency WASAPI / Core Audio report;
  Bluetooth adds more than that, which is what the app's per-device offset is for.
- **Seek at a shifted key** is timing-correct by construction but not sample-identical to
  an uninterrupted render (section 4); listening checklist.
- The 20-minute run and the memory figure are from Windows; the MacBook Air (8 GB,
  fanless) is the real acceptance target.

## 9. Independent re-run (verifier, 2026-11-17, final code)

Same machine and device, release build with the verifier's three engine fixes (stale mix
parameters no longer carried into the next song; clip warning no longer re-latched by
the old mix still in the graph; a refused exclusive mode falls back to shared mode on
the same device in `Engine::new` / `play()` / `load()` too). Digital silence throughout.

| Run | Result |
|---|---|
| 20-minute run (command of section 2; a `cargo build -j 4` of the workspace ran at the same time for ~9 minutes) | underruns **0** (120 016 callbacks), device starved **0**, min lead **132.1 ms**, worker 20.6 % of one core, longest block 24.7 ms, longest callback 166.5 us, reader starved 0, shortfall 0, built on the render thread 0; 40 key changes, 10 seeks, 28 loops; clock -21.57 ppm weighted mean (IAudioClock -20.83 ppm); peak working set 63.9 MB, private bytes 59.1 MB at the end (stable at ~51 MiB while sampled at 11 and 13 min: no growth). One clock residual of -57.5 ms around the linear trend (max \|drift\| 36.5 ppm per segment); not explained by an underrun or a late device thread (both 0). A follow-up 125 s loop run (3 song ends, 14 busy threads plus a second process with 16 more, no build) did not reproduce it: jitter -0.27 .. +0.18 ms, 0 underruns. Cause unknown (an OS audio-engine stall during the concurrent build is one candidate) |
| Memory, 4-minute song (section 3 command) | peak working set **49.8 MB**, private 54.8 MB, 0 underruns, min lead 140.0 ms |
| `seek-test` demo, 20 seeks | key 0 and key +3 drums-only 20/20 bit-exact; key +3 full mix 1/20; seek latency to the DAC median 49.3 ms (37.2 .. 50.8); 0 underruns |
| `device_switch` example | every switch OK (214, 640, 73, 638 ms; slower than section 7 with other work on the machine), exclusive at 44 100 Hz, simulated loss paused in 80 ms with `DeviceLost`, `play()` reopened, 0 underruns |
| Export, 4-minute song | Flac24 key 0: 2.15 s (111.5x); key +3: 31.96 s (7.5x). Flac24 at +3 decoded by libsndfile vs a separate Wav32f render at +3: max difference 5.96e-8 = half a 24-bit LSB |
| `cargo test --release -p jk-engine` | 14/14 engine tests (12 + 2 new regression tests `params_sent_without_a_song_do_not_leak_into_the_next_load`, `clip_warning_clears_when_the_mix_is_lowered`, both failing before the fixes: 3 350 290 samples played with stale settings; warning stuck on) |

## 10. How to reproduce

```bash
source scripts/devenv.sh
cargo test --release -p jk-engine                 # engine, export, seek, devices (null), demo song
cargo run --release -p jk-cli -- make-demo-song "$JK_BUILD/demo.jksong"
cargo run --release -p jk-cli -- make-test-song "$JK_BUILD/song4min.jksong" --seconds 240
cargo run --release -p jk-cli -- play "$JK_BUILD/demo.jksong" --seconds 1200 --silence --loop \
    --key-every 30 --seek-every 120 --load-threads 14 --report run20.json
cargo run --release -p jk-cli -- play "$JK_BUILD/song4min.jksong" --seconds 120 --silence \
    --key-every 15 --seek-every 25
cargo run --release -p jk-cli -- seek-test "$JK_BUILD/demo.jksong" --seeks 20
cargo run --release -p jk-engine --example device_switch -- "$JK_BUILD/demo.jksong"
cargo run --release -p jk-cli -- export "$JK_BUILD/song4min.jksong" --format flac24 --out x.flac
```

## 11. Real-time path: zero allocations (test, 2026-11-18)

`cargo test --release -p jk-engine --test rt_alloc -- --nocapture`
(`crates/jk-engine/tests/rt_alloc.rs`, part of the workspace tests and of local CI).

**Instrument.** A counting global allocator sees every Rust allocation and free. On
Windows with the GNU toolchain the test also redirects the executable's import slots of
the C runtime's `malloc`, `calloc`, `realloc`, `free`, `_aligned_malloc` and
`_aligned_free` through counters, so Rubber Band's C++ (which allocates with
`_aligned_malloc` and libstdc++'s `operator new`) is counted too. Each call is
attributed to its thread by name: `jk-render` (render worker), `jk-null-out` /
`jk-wasapi-out` (the device thread: the device callback `Filler::fill` plus the device
API calls), `jk-engine-house` (house-keeping, expected to allocate). Each thread's own
start-up (its buffers, the thread-priority string) is before the counted window.

**Positive controls** (the test fails if they see nothing): the capture engine's Vec
growth on the device thread is counted (3 Rust allocations within 0.05 s); during the
key-change phases the house thread's Rust and C/C++ allocations are counted (256 Rust,
101,711 C/C++ allocations: it builds the Rubber Band instances).

**Result** (null device at 48 kHz with the 44.1 to 48 kHz resampler, a generated 30 s
stereo song; counts are Rust alloc / Rust free / C alloc / C free):

| Phase (counted from the steady state) | Render worker | Device thread | House thread |
|---|---|---|---|
| play at key 0, 2 s | 0/0/0/0 | 0/0/0/0 | 0/0/0/0 |
| 25 key changes, every key from -12 to +12 | 0/0/0/0 | 0/0/0/0 | 0/0/72182/72182 |
| 4 seeks at key +3 | 0/0/0/0 | 0/0/0/0 | 0/16/13124/13124 |
| 8 mixer changes (guide, backing off/on and level, drums, other, shift drums on/off, reset) and 3 master changes | 0/0/0/0 | 0/0/0/0 | 0/0/6562/6562 |
| pause and resume | 0/0/0/0 | 0/0/0/0 | 0/0/0/0 |
| song change while playing (key -2) | 0/0/0/0 | 0/0/0/0 | 0/335/16405/22879 |
| prerender mode (null device 44.1 kHz, 20 s song): key +4 prepared then played from the cache | 0/0/0/0 | 0/0/0/0 | 232/210/0/0 |
| prerender: 3 seeks on the cached key | 0/0/0/0 | 0/0/0/0 | 24/27/0/0 |
| prerender: mix change (re-render) and live drum level | 0/0/0/0 | 0/0/0/0 | 247/227/0/0 |
| prerender: back to key 0 | 0/0/0/0 | 0/0/0/0 | 0/1/0/0 |

0 underruns, 0 Rubber Band instances built on the render thread, 0 cache misses;
longest device callback 7.3-7.9 us, longest worker block 0.06-1.8 ms
(`$JK_BUILD/final/rt_alloc_final.log`, the final code). With
`JK_RT_ALLOC_REAL_DEVICE=1` the real-time phases also ran on the default device
(Realtek, WASAPI shared 48 kHz, master 0 = digital silence): all zero on the worker and
on `jk-wasapi-out` (so the WASAPI calls in the device loop do not allocate either),
0 underruns, 0 device starved, longest callback 8.5 us
(`$JK_BUILD/final/rt_alloc_realdevice.log`).

**What it found, and the fix.** The first run failed: on the render worker, the first
`process()` of every new Rubber Band instance allocated 568 bytes and freed the buffer
reserved by the constructor (Rubber Band's `BQResampler::state_for_ratio` replaced its
state buffer with a newly constructed vector), and one key at 44.1 kHz grew the
resampler's phase table by 1,524,032 bytes there (the constructor reserves 2 x the
sample rate entries, the ratio's numerator can reach `rational_max` = 96000). Both
happened on every key change, every seek at a shifted key and every "shift drums"
toggle. The vendored `BQResampler.cpp` now fills the buffer with `assign()` (reusing the
reserved capacity) and reserves the phase table for `rational_max + 16` entries in the
real-time (RatioOftenChanging) mode; the change is marked in the file and in
`crates/jk-rubberband/vendor/VENDORED.md`. The output is unchanged: the 18 golden
renders of `scripts/ci-render.sh` are byte-identical to the earlier Windows renders
(`ci-compare-renders.sh`: 18 files, 0 failures, max difference 0), and
`cargo test --release -p jk-rubberband -p jk-dsp -p jk-engine` passes (49 tests, among
them the real-time-equals-offline and prerender bit-identity tests).

**Not covered.** Lock waits are not measurable with this instrument; the worker and the
callback use only atomics and SPSC rings (no `Mutex`, `RwLock`, `Condvar` or channel
receive in `Filler::fill` or `Worker::run`, checked by reading the code). On macOS the
C runtime hooks are not installed (Rust allocations only) and the Core Audio callback
thread is not named by us, so there the test covers the render worker and the null
device. The macOS run of this test has never happened.

## 12. 20 minutes in the real app: skin, bubbles, ticker, key changes, CPU load (2026-11-18)

WORK_ORDER 9 "Smooth playback", trap 11 and 4.10 "Sustained performance" / "Memory", on
Windows. Earlier long runs were the CLI engine without UI (sections 2 and 9); the real app
with bubbles had run for 5 s without load.

```
# a copy of dist/ (Models left out: playback does not use them) plus the 4-minute song
node app/ui/scripts/real-app-soak.mjs <copy>\Windows\Jukeoke.exe <copy> <out> "Feel (Stripped)" 20 14 30
```

**Set-up.** The release exe from `$JK_BUILD/final/soak-copy` (built by
`scripts/make-dist.sh` at 02:24-02:28 with the Rubber Band fix of section 11), driven over
WebView2's DevTools port. Song: "Feel (Stripped)" by Cortez (CC BY, JamendoLyrics),
made by the Studio in the end-to-end run: 4:00.6, 48 kHz, 4 stereo stems. Settings: skin,
bubbles and full motion on; Stage (lyrics ticker) visible; window 1440x900; default
output device (Realtek, WASAPI shared, 48 kHz, 2 ch f32, 1056-frame buffer) at **master 0,
so the device played digital zeros** (the reported output peak stayed at the -120 dB
floor in all 1,187 one-second samples; the script stops if it rises above -100 dBFS);
`audio.key_mode` = `realtime` (what `auto` picks on this PC; the heavier path). A key
change every 30 s through `player_set` (the command the key controls call): +3, -2, +5,
0, -7, +12, -12, +2, repeated, 39 changes. The song looped 5 times by a seek to 0 three
seconds before its end. **14 busy threads** on the 16-thread CPU in a separate process
for the whole run (47.7 M work units). The display runs at about 178 Hz.

| Measure | Result |
|---|---|
| Duration | 1200.7 s of playback, state `playing` in every sample |
| Underruns (callback found the output ring empty) | **0** (120,371 callbacks, 0 frames of silence) |
| Device starved (WASAPI endpoint buffer empty) | **0** |
| Render worker lead, minimum while playing (target 150 ms) | **140.0 ms** (no sample of the 119 below it) |
| Longest device callback / longest worker block | 149.6 us / 14.5 ms (the lead absorbs it) |
| Render worker CPU | 14.2 % of one core (170.9 s busy in 1200.7 s), MMCSS "Pro Audio" |
| Stem reader starved, Rubber Band shortfall, instances built on the render thread | 0, 0, 0 |
| Device clock vs the OS counter | -20.82 ppm (as in section 2) |
| Ticker | 72,041 frames = **60.000 fps** mean (cap 60), 0 frames missing against 60 per second; every 10 s window 59.9-60.1 fps; draw time mean 0.025 ms, max 1.5 ms |
| Bubbles | 29.3 fps mean (cap 30; 28.6-29.7 per window), 49 bubbles alive, draw time mean 0.067 ms, max 0.8 ms |
| Display frames (requestAnimationFrame) | 213,269 at 177.6 per second; interval median 5.5 ms, 99th and 99.9th percentile 6.5 ms, max 12.5 ms; **0 intervals over 25 ms**; 3 frames over 1.5x the median (one missed refresh each) in 20 minutes |
| Page visibility | `visible` in all 119 samples |

### Whole-app memory (WORK_ORDER 4.10: "under about 1 GB" for all processes)

`app/ui/scripts/soak-memory.ps1` sampled every process under Jukeoke.exe every 2 s
(`Win32_Process`: working set, private bytes, and the OS's per-process peak counters):
Jukeoke.exe and its seven WebView2 processes (browser, renderer, GPU, network and storage
utilities, crashpad handler). 581 samples over the run.

| | Working set (resident) | Private bytes (commit) |
|---|---|---|
| Steady state, 60 s to the end of playback: median / maximum of the summed tree | **551 MB / 557 MB** | **488 MB / 504 MB** |
| Maximum of the summed tree over the whole run | 605 MB (at 1,203.7 s, after playback ended, see below) | **1,122 MB** (first sample, see below) |
| Sum of each process's own OS peak (upper bound; the peaks are not simultaneous) | 653 MB | 1,330 MB |
| Growth: mean of the first 3 minutes vs the last 3 minutes | 541 -> 557 MB | 516 -> 503 MB |

Per process (OS peak working set / peak private bytes): GPU process 209 / 1,094 MB,
WebView2 browser 147 / 56 MB, renderer 144 / 88 MB, **Jukeoke.exe 70 / 69 MB** (the core
and the audio engine), network utility 43 / 12 MB, crashpad 21 / 4 MB, storage utility
20 / 8 MB.

- **Against the 1 GB budget, on Windows:** the resident memory of the whole app stays
  between 0.51 and 0.61 GB for the whole run; its committed private memory settles
  at 0.48-0.50 GB from the first minute on (0.46-0.72 GB during the first minute). Two transient private-bytes peaks go above that, both
  in the WebView2 GPU process: **1.12 GB in the first 4 s after the window appeared**
  (GPU process 1.0 GB, back to 0.53 GB 4 s later and 0.31 GB after a minute) and 0.97 GB
  in the second-to-last sample (1,203.7 s, 3 s after playback ended; the last sample, 2 s
  later, was 0.76 GB), taken while the test driver read the final statistics and
  disconnected DevTools (cause not determined). Neither shows up as resident memory.
- **The Studio is not part of the player budget.** On this PC the start-up probe launches
  the Studio's Python, which stays idle until its 10-minute idle shutdown: 775 MB working
  set, 1.95-2.10 GB private bytes, gone at 601 s, as designed. The Mac has no Studio.
- **This is Windows with WebView2 and a discrete GPU, not the Mac.** WKWebView on
  Ventura has a different process model (WebContent, GPU, Networking) and the M1 has
  unified memory, so the Mac's number can only come from the owner's MacBook (Activity
  Monitor, "Memory" column, for Jukeoke and its "Web Content" processes) or the
  self-check. The engine's share is small on both: Jukeoke.exe peaked at 70 MB here.

Evidence: `$JK_BUILD/final/soak20/soak.json` (every sample, the diagnostics every 10 s,
the frame histogram), `memory.csv`, `soak20.log`; process types in
`$JK_BUILD/final/soak20-webview-types.txt`. A 30 s trial with 2 load threads before it
(`$JK_BUILD/final/soak-trial/`) gave the same picture.

**Not shown by this run:** a throttling fanless machine (the M1), a Bluetooth or USB
device, exclusive mode, the prerender key mode on a real song, a minimised window.

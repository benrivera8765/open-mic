# Recorder measurements: Windows WASAPI loopback (Stage 5)

Measured on 2026-11-17 on the build PC (Windows 11 build 26200.9974 25H2, Ryzen 7 9700X,
32 GB), output device **"Speakers (Realtek(R) Audio)"**, the only active output device:
shared-mode mix format 48000 Hz, 2 ch, 32-bit float (channel mask 0x3); the engine's
hardware format 48000 Hz, 2 ch, 24 valid bits; endpoint volume 100 % (read, not changed),
not muted. Rust 1.100 `x86_64-pc-windows-gnu`, release build. Every number below comes from
`loopback_probe` (an example in `crates/jk-recorder`) or the crate's tests; the commands are
under "How to reproduce". Nothing here is a listening judgement, and no Windows setting was
changed to obtain any of it.

Implementation under test: `crates/jk-recorder/src/platform/windows.rs` on the shared
pipeline `crates/jk-recorder/src/pipeline.rs` (WORK_ORDER 4.6, trap 5).

## Summary

| Check (WORK_ORDER section 9) | Result | Criterion |
|---|---|---|
| Captured sample rate = device mix rate | **48000 Hz = 48000 Hz**, every run | equal |
| Sample count vs elapsed time, 20 s with 8 s of total silence, keep-alive on (default) | **-18.8 frames** (-0.39 ms) vs the OS-clock time from `start()` to `stop()`; ratio 0.999980 | within one packet (480 frames) |
| Same, keep-alive off (silence filled only from QPC timestamps) | **-74.8 frames** (-1.56 ms); ratio 0.999923; Windows sent no packet for 7.73 s, filled exactly | within one packet |
| Start during silence, stop during silence, 10 s, keep-alive on | **-10.9 frames** (-0.23 ms) | within one packet |
| Same, keep-alive off | **-0.1 frames**; Windows delivered **0 packets in 10.1 s**, the file is still 10.146 s | within one packet |
| Recording length vs the caller's wall clock (`start()` call to `stop()` call) | 20.024 s vs 20.024 s; 10.233 s vs 10.233 s; 10.146 s vs 10.146 s | equal |
| Dropouts | **none**: 0 device-position jumps, 0 discontinuity flags after the first packet, 0 dropped frames, each episode's last pseudo-random segment exactly where the first predicts it (offset 0 frames), 0 player underruns | none |
| Bit-transparency of the whole chain (played float -> loopback -> file) | **NOT bit-identical on this PC**: max abs diff 7.87e-4 to 7.98e-4 against a 9.77e-4 signal; cause: Windows endpoint effects (below) | report |
| Bit-transparency of the recorder's own path (packet -> file) | bit-exact (unit tests `conversions_are_exact`, `stereo_float_round_trip_is_bit_exact`, `missing_block_is_filled_with_exact_silence`) | exact |
| Device lost mid-recording (another stream takes the device exclusively) | state **Failed** with "...(AUDCLNT_E_DEVICE_INVALIDATED, 0x88890004). This usually means another app took exclusive control of the device...". File already a **valid closed WAV (2.253 s)** before `stop()` | visible error, file finalized |
| Heap allocations, whole process, during 5 s of steady recording | **0** (5 runs) | 0 |

## The test signal and schedule

`loopback_probe` records the default output device with `jk_recorder::Recorder` and, in
the same process, plays a test signal on that device through its own shared-mode WASAPI
stream, written in the device's float mix format, so it reaches the mix without any
conversion:

- amplitude 2^-10 = **-60.2 dBFS peak** in the runs reported below. **Correction (verifier):**
  loopback taps the mix after Windows' endpoint effects, i.e. what the DAC receives, and those
  runs captured a peak of **-58.0 dBFS**: the enhancements raised the signal above the -60 dBFS
  limit. The probe now plays 2^-11 = -66.2 dBFS (captured peak -64.0 dBFS) and stops before
  episode B (`result=too_loud`) if the captured output exceeds -60 dBFS. See "Verifier re-run";
- episode A: 1.0 s pseudo-random sequence (xorshift32, +-2^-10, a different sequence per
  channel), 2.5 s of 997 Hz sine, 1.0 s of a second pseudo-random sequence;
- episode B: 1.0 s + 2.0 s + 1.0 s, new sequences;
- schedule (20 s): start recording in silence, 1 s lead-in, episode A, **8 s of total
  silence** (the probe's player stream is stopped and released; only the recorder's own
  keep-alive zeros play), episode B, about 2.4 s of silence, stop recording in silence.

From the file alone the probe then checks the rate, the length against the Start..Stop
time, aligns each episode on its first pseudo-random segment (correlation within +-64
frames of the first sample at half the signal level), checks the last segment sits at
exactly the predicted offset (any dropout or sample slip inside an episode moves it),
compares every sample of both episodes with what was played, measures the sine, and
measures everything outside the episodes. It polls the device's audio sessions every
250 ms and reports `disturbed` (exit 7) if another app played during the test.
Firefox was playing a video on this PC at about -12 dBFS during the first attempts.
Those runs are not used: `--wait-quiet` waits until no other app has an active audio
session.

## Results in detail (final runs, final binary)

| Run | Packets | Largest gap between packets | Head fill | Gap fill | Tail fill | Length - (Stop - Start) |
|---|---|---|---|---|---|---|
| 20 s, keep-alive on | 2001 (0 flagged silent) | 0.0 ms | 1220 fr (25 ms: set-up before the stream ran) | 0 | 0 (565 fr read after Stop cut off) | -18.8 fr |
| 20 s, keep-alive off | 929 | **7727.6 ms** | 59231 fr (1.23 s) | 370924 fr in 1 gap (7.73 s) | 89633 fr (1.87 s) | -74.8 fr |
| 10 s silence, keep-alive on | 1001 | 0.0 ms | 11274 fr (235 ms set-up) | 0 | 0 | -10.9 fr |
| 10 s silence, keep-alive off | **0** | n/a | 0 | 0 | 486994 fr (all of it) | -0.1 fr |

Earlier runs of the same schedule while the code was still being finished gave -20.9, -8.8,
-10.1 and +0.1 frames for the same four cases (consistent).

### Trap 5 measured

With the keep-alive off, Windows delivered **no loopback packets at all** while nothing
played: nothing for the 1.23 s before episode A, nothing for 7.73 s between the episodes,
nothing after episode B, and in the all-silent run not a single packet in 10.1 s. A
recorder that writes only what arrives would have produced 9.3 s instead of 20.1 s, and an
empty file instead of 10.1 s. Both measures from the work order are in place and were each
shown to work alone:

- (a) the **silent keep-alive render stream** on the same endpoint (buffers flagged
  `AUDCLNT_BUFFERFLAGS_SILENT`, i.e. digital zeros) makes Windows deliver a packet every
  10 ms throughout: 2001 packets in 20 s, largest gap 0.0 ms, no gap fill needed;
- (b) **QPC timestamps** (`GetBuffer`'s `pu64QPCPosition`) fill everything that is missing:
  with the keep-alive off, the head, the 7.73 s hole and the tail were filled with digital
  silence of exactly the right length (-74.8 frames over 20.1 s).

### Start and Stop

The recording spans the instant `Recorder::start()` was called to the instant `stop()` was
called (the Stage 0 verifier's finding: the time from Start to the first packet and from
the last packet to Stop was lost). Both instants are converted to the QPC clock. One
measured detail matters here: loopback packet timestamps run **9.8 to 10.6 ms ahead** of
the QPC time at which the packet can be read (about one engine period). Start and Stop are
moved onto the packets' timeline by the same amount (the first packet's lead), so the
length is exactly Stop - Start whatever the error of that estimate. The packet that
straddles Stop is held back and cut at the stop instant (565 to 587 frames cut per run), so
the recording ends at Stop instead of up to one packet later. The remaining difference
(-0.1 to -75 frames) is timestamp jitter below the 10 ms gap threshold, which is measured
but never "corrected" by inserting or dropping samples. The worst case was 66 frames, in
the keep-alive-off run, when the capture thread on this busy build PC read some packets up
to 30 ms late.

### Bit-transparency: what Windows does to the mix on this PC

The capture is **not** bit-identical to what was played, and the cause is measured:
Windows reports, through `IAudioEffectsManager` on the probe's render stream, that this
endpoint has **equalizer ON, bass boost ON, speaker compensation ON** (speaker fill,
virtual surround, virtual headphones, room correction off). These are the endpoint's
"audio enhancements". Loopback captures the mix **after** them. Measured on the
pseudo-random segment of episode A (Welch, 8192-point FFT), captured / played:

| Frequency | 31.5 Hz | 63 | 125 | 250 | 500 | 1 k | 2 k | 4 k | 8 k | 16 k | 20 kHz |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Gain (dB), both channels | +2.9 | +3.0 | +2.9 | +2.6 | +1.8 | +0.2 | -1.4 | -2.5 | -2.9 | -3.0 | -3.0 |

- 997 Hz sine: -62.98 dBFS RMS captured vs -63.22 dBFS played (+0.24 dB, as the curve
  predicts); its frequency measured 997.000 Hz.
- Least-squares broadband gain on the whole episode: 0.83 (A) and 0.82 (B). It varied by
  about 0.4 % between runs, and slightly more in the keep-alive-off run, where the effects
  start from idle with each stream.
- The processing behaves almost like a fixed linear filter: episode B predicted from the
  impulse response measured on episode A (8192-tap FIR) leaves a residual of -119.7 dBFS
  RMS against a -62.8 dBFS signal (57 dB down).
- Its tail: after each episode the effects ring for a while: peak -77.7 dBFS at the end
  of the signal, -121.6 dBFS 100 ms later, about -181 dBFS peak (-191 dBFS RMS) after
  1 s. While the keep-alive keeps the endpoint running, "silence" therefore carries the
  effects' numerical floor (silence-only run: peak -120.6 dBFS) instead of exact zeros.
  Without the keep-alive the silent stretches are exact digital zero (filled by the
  recorder; 0 of 973988 samples non-zero in the all-silent run).
- The stream option `AUDCLNT_STREAMOPTIONS_RAW` changes nothing: it was tried on the
  loopback stream, on the player stream and on both. The capture was the same to within
  0.03 % in gain; with RAW on the player, Windows reported no effects on that stream, yet
  the loopback still showed the same curve. So the processing is endpoint-wide, ahead of
  the loopback tap, and no application option bypasses it. The experiment code was
  removed; RAW is not used.

**What this means for the owner.** The recorder adds no processing. Packets are copied
into the file bit for bit, as the unit tests show. But on this PC Windows itself shapes
the system mix: about +3 dB bass, about -3 dB treble. To record the mix exactly as
applications play it, switch off **Settings > System > Sound > Speakers (Realtek(R)
Audio) > Audio enhancements** before recording. That should make the capture
bit-identical. This is expected but **not verified**: changing Windows sound settings was
not allowed in this run. The app should say this next to the capture format.

### Device lost

`loopback_probe --grab-exclusive` records, then after 2 s opens the same endpoint in
**exclusive mode** for 1 s (playing digital zeros, so nothing is audible). Windows then
invalidates every shared-mode stream on the device. This produces a real device loss
without unplugging anything or changing a setting. Result:

- `GetNextPacketSize` returned `AUDCLNT_E_DEVICE_INVALIDATED`. The recorder checked the
  device (still present, same format) and set the state to
  `Failed("Windows stopped the loopback stream on \"Speakers (Realtek(R) Audio)\"
  (AUDCLNT_E_DEVICE_INVALIDATED, 0x88890004). This usually means another app took exclusive
  control of the device, or its driver restarted. Everything recorded up to that moment is
  saved.")`;
- the file was a **valid, closed WAV of 2.253 s** **before** `stop()` was called. Its
  108127 frames are a 10207-frame head (the 207 ms of set-up after the `start()` call)
  plus 97920 frames from packets, up to the moment of the loss. `stop()` then returned
  the statistics with the same error.

Not exercised on hardware (each would need unplugging a device or changing a Windows
setting): a device that is **removed**, and a **format change** in Sound settings. Both
arrive as the same `AUDCLNT_E_DEVICE_INVALIDATED`. The recorder then tells them apart by
re-reading the device: "was removed, unplugged or disabled", or "changed its format
from 48000 Hz ... to ...". The pipeline's part (state Failed, file closed before `stop()`,
nothing written after the error) is covered by the unit tests
`format_change_stops_the_recording_visibly_and_closes_the_file` and
`close_with_error_finalizes_and_reports`.

### Real-time behaviour

- The capture loop runs on the recorder's own thread under MMCSS "Pro Audio", waking on the
  WASAPI event (every 10 ms while packets flow) or every 20 ms.
- `loopback_probe --alloc-check` counts every heap allocation in the process with a counting
  global allocator. In 5 s of steady recording (capture loop, pipeline, writer thread
  and keep-alive thread together; the probe's own thread only sleeps), there were **0**
  allocations, in 5 out of 5 runs. Before the sample-buffer pool was pre-filled when the
  format is declared, the same check counted 2 allocations in 5 s (the pool grew while
  the writer sat in its 5-second `fsync`).
- The capture buffer is 200 ms, so the thread can be late by that much without losing
  audio. Packets read up to 30 ms late were seen on this busy PC, with no loss.

## How to reproduce

```bash
cd jukeoke && source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-recorder"
cargo test -p jk-recorder                      # includes a real 0.5 s loopback capture
cargo build --release -p jk-recorder --example loopback_probe
P=$CARGO_TARGET_DIR/release/examples/loopback_probe.exe
$P --list                                      # output devices (ids for --device)
$P --wait-quiet 1500 --out run.wav --report run.txt                 # 20 s, 8 s silence
$P --wait-quiet 1500 --no-keepalive                                 # trap 5 without (a)
$P --wait-quiet 1500 --silence-only --seconds 10                    # start/stop in silence
$P --wait-quiet 1500 --silence-only --seconds 10 --no-keepalive
$P --wait-quiet 1500 --grab-exclusive                               # device lost
$P --alloc-check
```

The last line of each run is machine-readable
(`LOOPBACK_PROBE result=ok rate=48000 device_rate=48000 frames=... ratio=... bit_exact=no ...`).
Exit codes: 0 ok, 1 error, 3 rate mismatch, 4 length off by more than one packet,
5 signal not found, 6 dropout or slip, 7 another app played (or kept playing), 8 a
device loss was not reported. The kept captures and reports of the runs above are in
`$JK_BUILD/recorder-bench/` (outside the repository).

## Verifier re-run (independent, same PC, same day)

Release `loopback_probe` built from the current tree, test signal at 2^-11 (-66.2 dBFS; the
captured output peaked at -64.0 dBFS). Outputs are in `$JK_BUILD/verify-recorder/`.

| Run | Packets | Head / gap / tail fill (frames) | Length - (Stop - Start) | Result |
|---|---|---|---|---|
| 20 s, keep-alive on | 2001, largest gap 0.0 ms | 5794 / 0 / 0 | **-18.3 fr** (ratio 0.999981) | ok; 0 slips, 0 dropped, 997.000 Hz |
| 20 s, keep-alive off | 928, largest gap 7722.9 ms | 60627 / 370701 (1 gap) / 90614 | **-64.0 fr** (ratio 0.999934) | ok; 0 slips, 0 dropped |
| 10 s silence, keep-alive on | 1001 | 9982 / 0 / 0 | **-10.5 fr** | length ok (another test process opened a session: "disturbed") |
| 10 s silence, keep-alive off | 108 (another process played at the end) | 434054 / 0 / 0 | **-54.0 fr** | length ok ("disturbed") |
| `--grab-exclusive` | | | | state Failed (AUDCLNT_E_DEVICE_INVALIDATED), valid closed WAV of 2.419 s before `stop()` |
| `--alloc-check` | 700 | | | **0** allocations in 5.00 s |

Bit-transparency reproduced: not bit-identical, the same broadband gains as above to 6 digits
(0.834436 / 0.822445 with the keep-alive), equalizer, bass boost and speaker compensation
reported ON; with the keep-alive the residual after a linear model fitted on each episode is
-112 to -116 dBFS peak (-124 to -128 dBFS RMS), so the change is a fixed linear filter.

**Correction:** the larger difference in the keep-alive-off runs (1.3e-3 in the run above at
2^-10, 6.3e-4 here at 2^-11) is not "the effects starting from idle". It sits in the **last
15 to 25 ms of each episode**: when the probe's player stopped its stream and no other stream
kept the endpoint running, Windows delivered a fade to about -105 dBFS instead of the end of the
signal (found by comparing every 10 ms block with what was played; the samples are real
packets, not recorder fill: no gaps, overlaps or zero runs there). With the keep-alive the
episode ends intact. This is one more reason the keep-alive is on by default.

## Limits and open points

- **One output device only** on this PC (Realtek onboard, effects on). Not measured: USB
  DACs, HDMI, Bluetooth, a device whose mix rate is not 48 kHz, multichannel mixes (the
  code reads the channel mask and writes WAVE_FORMAT_EXTENSIBLE above 2 channels; unit
  test only), integer mix formats (conversion unit-tested only).
- With the keep-alive on, silence in a recording is the effects' noise floor
  (-120 to -180 dBFS peak here), not exact zero. It is inaudible, and it is what the
  device outputs while any stream plays.
- If the user switches the **default** output device mid-recording, the recording stays on
  the device it started on. It is not invalidated, so it records what that device still
  plays, possibly silence. This is not reported to the UI yet.
- `RecorderStatus.stats.media_elapsed_secs` only advances when packets arrive. With the
  keep-alive on (default) that is continuous. If the keep-alive could not start (noted in
  the report), it pauses during silence and catches up at the next packet or at Stop. A
  UI clock should use `wall_elapsed` while recording.
- macOS: the same start/stop extension is in the shared pipeline and the ScreenCaptureKit
  backend now marks Start and Stop on the host-time clock. That is type-checked only; it
  has never run on a Mac.

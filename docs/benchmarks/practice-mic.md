# Practice mode: microphone input (WO2 Stage 0)

Work order 2 sections 4.4 ("Choosing the input", "Choosing the channel", "Levels"),
4.8 (permission text), 4.10 (the owner's rig), 5 (real-time safety) and traps 2, 3, 4
and 14. Measured on the build PC on 2026-11-19 (Windows 11 build 26200, Ryzen 7 9700X,
Realtek on-board audio). Stage 7 merges this file into `docs/benchmarks/practice.md`.

Nobody has sung into any of this. No microphone sound was written anywhere: the probe
analyses captured blocks in memory and drops them. Evidence (text and JSON reports, no
audio) is in `../jukeoke-build/practice-mic/`.

## Results at a glance

| Claim | Command | Result |
|---|---|---|
| Input module unit tests (ring, meters, channel choice, device choice, Bluetooth rule, unplug, stall watchdog, slow reader, Info.plist text, a generated WAV file through the input path) | `cargo test -p jk-recorder --lib input` | 25 passed |
| Whole jk-recorder suite still passes | `cargo test -p jk-recorder` | 86 lib + 1 + 2 + 1 + 1 passed, 0 failed |
| Capture side never allocates (generated source thread through `InputStream`; integer conversion with `push_iter`; positive control) | `cargo test -p jk-recorder --test input_rt_alloc -- --nocapture` | 150 blocks: 0 allocations, 0 frees; 200 i16 blocks: 0 / 0; control thread: 2 allocations seen |
| The real WASAPI capture thread never allocates in its steady state (independent verifier; temporary test, same counting allocator, on the loopback test path) | `cargo test -p jk-recorder --test zz_verify_wasapi_rt_alloc -- --nocapture` (file kept in `practice-mic-verify/`, not in the repo: it needs an output device) | 300 blocks of 480 frames: 0 allocations, 0 frees; positive control: 12 allocations seen on `jk-input-wasapi` during open and warm-up |
| A WAV file through the same path as a microphone | `mic_probe --wav generated-voice-ch2-96k.wav` (generated, 96 kHz, noise -60 dBFS RMS on ch 1, voice from 1 s on ch 2) | 2 ch, 96 kHz, 384000 frames, state Ended; ch 1 RMS -60.0, floor -60.1 dBFS, advice waiting; ch 2 peak -12.5, RMS -17.9 dBFS, advice good; suggested voice channel 2 |
| Clippy, Windows | `cargo clippy -p jk-recorder --all-targets` | 0 warnings |
| Mac type-check of jk-recorder (lib, tests, examples) and the app | `app/scripts/check-mac.sh -p jk-recorder --all-targets` | exit 0, 1 m 58 s; the only jk-recorder warning is the existing one in `preview.rs` |
| Clippy, Mac target | `JK_CARGO_CMD=clippy app/scripts/check-mac.sh -p jk-recorder --all-targets` | exit 0; 0 warnings in `src/input/`, `mic_probe`, `input_rt_alloc` (all warnings listed are in files this workstream did not touch) |
| Input devices on this PC | `mic_probe --list` | **0 active input devices.** 17 known but unusable (14 "not present", 2 "unplugged", 1 "disabled": Stereo Mix); automatic choice: none |
| 3-second capture of a real microphone | `mic_probe --capture 3` | not possible: refused with "No microphone or audio input is connected ..." (kind `no_device`), no crash |
| The real WASAPI capture code on real hardware (test path: loopback of the default output through the same capture loop, silent keep-alive, nothing audible) | `mic_probe --capture 3 --loopback` (run twice) | 2 ch, 48 kHz, f32; 300 blocks of 480 frames, 0 dropped, 0 discontinuities; time-stamp step error at most 0.020 / 0.021 ms; device clock vs QPC -19 / -17 ppm; state Running |
| Voice on one channel of two at 16 / 44.1 / 48 / 96 kHz through the input path | `mic_probe --signals` | 24 of 24 PASS: voice channel bit-exact, peak -12.00 dBFS, RMS within 1e-6 dB of the generated value, other channel metered independently, suggested channel = voice channel, pitch preserved (check estimator: at most 0.07 cents) |
| Info.plist text parses and merges like Tauri's | plist 1.8.0 (Tauri's crate) on `app/src-tauri/Info.plist` | parsed, merged, key present (`merged-Info.plist`) |
| Bundle check script | `scripts/check-macos-infoplist.sh` on fake bundles | merged plist: exit 0; raw fragment: exit 0; plist without the key: exit 1 (FAIL); no argument: exit 2 |
| Shell scripts lint | `shellcheck -x scripts/check-macos-infoplist.sh scripts/build-mac.sh` | 0 findings |

## What was built

New module `crates/jk-recorder/src/input/` (GPL-3.0-or-later), separate from the
system-audio recorder:

| File | What it does |
|---|---|
| `mod.rs` | `InputSource` trait (`open` -> real format, `start(producer)`, `stop`); `InputStream` (opens a source, sizes the rings for 2 s, reads blocks, meters them, stall watchdog, `state()`, `health()`); `extract_channel` (bit-exact, never sums); `DeviceSource`; `list_input_devices`, `list_inactive_inputs`; `mic_permission` / `request_mic_permission`; permission texts; `InputError` (its own type: `RecorderError::Unsupported` reads "system audio capture ..." and the app matches `RecorderError` exhaustively, so a new variant there would break the app crate) |
| `ring.rs` | Two `rtrb` SPSC rings (interleaved f32 samples, one `BlockStamp` per block: first frame, frames, host-clock ns of the first frame, flags). The producer writes samples then the stamp; a full ring drops the whole block, counts it and flags the next one as a discontinuity. Errors go through `StreamStatus::fail` (error path only, never the data callback) |
| `clock.rs` | Host clock in ns: QPC on Windows (what WASAPI stamps packets with), `mach_absolute_time` scaled by `mach_timebase_info` on macOS (cpal 0.16 builds its `StreamInstant` from `mHostTime` with the same scaling, so `stream_instant_ns` is on this clock) |
| `levels.rs` | Per-channel meters on the reading thread: peak and RMS (300 ms), 3-s peak, noise floor (10th percentile of 50 ms RMS over 10 s), active level (90th percentile), clipping (>= 0.999, last 2 s and total), plain advice (`good`, `waiting`, `too_quiet` below -30 dBFS peak, `clipping`, `silent` below -90 dBFS) and `suggest_voice_channel` |
| `device.rs` | `InputDeviceInfo` (id, name, adapter, transport, form factor, bus, channels, rate, format, default, hands-free, declared latency) and the choice rule: saved choice by id then name; else USB > Thunderbolt/FireWire/PCI > built-in > unknown > display; **never** Bluetooth, virtual, aggregate, AirPlay or Continuity automatically (they can be picked by hand, with a warning) |
| `win.rs` | WASAPI: listing from each active capture endpoint's property store (friendly name, adapter, `PKEY_AudioEndpoint_FormFactor`, `PKEY_Device_EnumeratorName`, else the device-topology adapter id); Bluetooth endpoints' format read from the property store so no audio client is ever activated on them. Capture thread (MTA, MMCSS "Pro Audio"): shared mode at the mix format, event driven, stream category "Other" (never "Communications", which ducks other sounds), raw stream requested (no Windows voice processing, trap 7) with a fallback that is reported; QPC capture time per packet; f32/i16/i24/i32 to f32 exactly, straight into the ring. `AUDCLNT_E_DEVICE_INVALIDATED` / not found -> `unplugged`, `E_ACCESSDENIED` -> `permission_denied` with the steps, exclusive use -> `device`. Privacy switches read (only) from `HKLM`/`HKCU ...\CapabilityAccessManager\ConsentStore\microphone` and `...\NonPackaged` |
| `mac.rs` | Core Audio HAL listing (input channels from the input stream configuration, UID, name, manufacturer, transport, nominal rate, declared latency = device latency + safety offset + stream latency). Capture through cpal 0.16 on a private thread (cpal's `Stream` is not `Send`), device from cpal's enumeration so cpal installs its "device is alive" listener; the device keeps its own rate and channel count; time stamp = `mHostTime` of the buffer (cpal's `callback` instant; its `capture` field subtracts a guessed buffer and is not used). Permission via `+[AVCaptureDevice authorizationStatusForMediaType:]` and `requestAccessForMediaType:completionHandler:`; opening refuses when permission is missing, because macOS hands denied apps silence rather than an error |
| `source.rs` | `SignalSource`: a generated or WAV signal delivered block by block by a thread that behaves like a capture callback (real-time pacing with host-clock stamps, or as fast as read); can end, loop, "unplug" or go quiet (for the failure tests) |
| `signal.rs` | Generated voice-like tones and "voice on channel N, something else on the others" layouts; a plain autocorrelation check of the fundamental (plumbing check only, not the practice tracker) |
| `other.rs` | Linux and others: compiles, lists nothing |

Also: `examples/mic_probe.rs` (the probe; plays nothing, writes only its JSON report;
`--wav FILE` feeds a 32-bit float WAV through the input path),
`tests/input_rt_alloc.rs`, `app/src-tauri/Info.plist`, `scripts/check-macos-infoplist.sh`,
a check in `scripts/build-mac.sh` step 6. Dependencies: `rtrb` 0.4 and, on macOS, `cpal`
0.16 were already in the workspace (jk-engine); `Cargo.lock` gains two dependency edges and
no new package, so `THIRD_PARTY_LICENSES.md` needs no new entry.

## Windows proof on this PC

### Input devices

`mic_probe --list` (`practice-mic/list.txt`, `list.json`): Windows reports **no active
input endpoint**. Known but unusable (names only):

- unplugged: "Line In (Realtek(R) Audio)", "Microphone (Realtek(R) Audio)";
- disabled: "Stereo Mix (Realtek(R) Audio)";
- not present: "Microphone (Antlion USB Microphone)", "Headset Microphone (DualSense
  Wireless Controller)", two "Microphone (High Definition Audio Device)", "Line In (High
  Definition Audio Device)" and nine Realtek jack endpoints.

The registry (read only) also lists two Bluetooth capture endpoints, an iPhone hands-free
link (enumerator `BTHHFENUM`) and a `BTHENUM` one; Windows' audio API returned neither, in
either the active or the all-states enumeration, so the phone was not connected. Had it
been, the listing would flag it `hands_free` and the automatic choice would skip it: that
rule is covered by `device::tests` and `win::tests::classification_from_enumerator_topology_and_name`
(`BTHHFENUM`, `BTHLEDEVICE`, a `bthhfenum#...` topology id and a "Hands-Free" name all give
Bluetooth + hands-free; `USB` gives USB; `HDAUDIO` built-in; `ROOT` virtual). The same
registry query shows that endpoint property stores on this PC do carry
`{a45c254e-df1c-4efd-8020-67d146a850e0},24` (`PKEY_Device_EnumeratorName`: `USB`,
`HDAUDIO`, `BTHHFENUM`, `BTHENUM`), the property the classification reads first.

### 3-second capture

No input device exists, so no microphone was opened. `mic_probe --capture 3` without a
device id refuses cleanly (`list-and-auto-capture.txt`): kind `no_device`, message "No
microphone or audio input is connected. Plug in the mixer (or a microphone) and open
practice mode again." Exit code 1, no panic.

### The capture code on real hardware (test path)

To prove the WASAPI capture loop itself, `--loopback` passes the default *output*
endpoint's id to the same `DeviceSource`. WASAPI then captures that endpoint's loopback
through exactly the code a microphone uses (open, raw/category, event wait, `GetBuffer`
QPC stamps, conversion into the ring, meters, state); a keep-alive stream submits only
buffers flagged silent (digital zeros), so nothing is audible and packets keep coming.
Practice mode never uses this path.

| Run | Format | Blocks | Dropped | Discontinuities | Largest stamp step error | Device clock vs QPC | Levels |
|---|---|---|---|---|---|---|---|
| 1 | 2 ch, 48 kHz, f32 | 300 x 480 frames (3.000 s) | 0 | 0 | 0.020 ms | -19 ppm | -160 dBFS both (digital silence) |
| 2 | same | 300 x 480 | 0 | 0 | 0.021 ms | -17 ppm | same |

The -17 to -19 ppm agree with the -21 ppm the engine measured on this device in WO1
Stage 0: the sample clock and the host clock really differ (trap 4), by about 4.6 ms over
four minutes here, which is why blocks carry host-clock stamps. Loopback packet stamps
run about one engine period *ahead* of their arrival (they mark when the sound will play,
the reader saw them 19.6 to 19.8 ms before the end of the block); for a microphone they
mark capture time instead, so this run says nothing about microphone latency.

### Generated signals: voice on one channel of two

`mic_probe --signals` (`signals.txt`, `signals.json`): a voice-like A3 (220 Hz, 8
harmonics, -12 dBFS peak, 3 s) on channel 1 or 2 of 2, the other channel carrying exact
silence, white noise at -70 dBFS RMS or a 1 kHz tone at -30 dBFS peak, at 16, 44.1, 48 and
96 kHz, fed through `SignalSource` -> rings -> `InputStream::read` -> meters ->
`extract_channel`:

| Check | All 24 combinations |
|---|---|
| Voice channel after extraction equals the generated voice | bit-exact (not halved, nothing leaked in) |
| Voice channel peak / RMS | -12.00 dBFS / -17.65 dBFS (expected -17.65; largest difference 1e-6 dB) |
| Other channel: silence / noise / tone | -160 dBFS / RMS -70.09 to -69.97 dBFS / peak -30.00, RMS -33.01 dBFS |
| Suggested voice channel | the voice channel every time |
| Pitch of the extracted channel (plumbing check) | within 0.07 cents of 220 Hz at every rate |

The same at every rate (trap 14). A real-time-paced run at 48 kHz (300 blocks of 10 ms):
each block reached the reader 1.25 ms (median) / 21.1 ms (max) after its last frame; an
earlier run under the other workstreams' load gave 49 / 138 ms. That figure measures the
generated source's thread and this probe's 1 ms polling under load, not a device; the
practice-mode reader (Stage 3) will set its own wake-up policy.

### Failure handling (unit tests, `input::tests`)

- A source that reports "unplugged" mid-stream ends `Failed { kind: unplugged }`; stopping
  afterwards keeps the failure visible.
- A source that just stops delivering (a dead driver) is marked `Failed { kind: stalled }`
  by the watchdog 1.0 to 1.6 s later (limit 1 s after the last block, 3 s for the first).
- A reader that sleeps longer than the 2 s ring loses whole blocks only; every delivered
  block is intact, the next one carries the discontinuity flag and the frame numbering
  counts the lost frames.

## The Mac

**Built but not verified.** Everything below is type-checked on Windows; nothing ran on a
Mac.

- `NSMicrophoneUsageDescription` is in `app/src-tauri/Info.plist`: "Jukeoke listens to
  your microphone only while practice mode is open, to show and grade the pitch you sing.
  The sound is analysed in memory on this Mac and is never recorded, saved or sent
  anywhere." It equals `jk_recorder::input::MACOS_MIC_USAGE_DESCRIPTION` (test
  `info_plist_carries_the_microphone_text`). The app's own explanation, shown before macOS
  asks, is `MIC_PERMISSION_EXPLANATION`; the steps after a refusal are `mic_denied_steps()`.
- **How it reaches the bundle.** Tauri 2 merges an `Info.plist` placed next to
  `tauri.conf.json` into the bundle's `Contents/Info.plist` (Tauri docs, "macOS Application
  Bundle", Native configuration, whose example is exactly the camera and microphone usage
  keys; the config schema compiled into this project's Tauri CLI 2.12.5 and
  tauri-utils 2.10.3 `MacConfig::info_plist` say the same: Tauri looks for an `Info.plist`
  beside the configuration file). `tauri.conf.json` is therefore unchanged (no `infoPlist` key: a
  relative path there would be resolved by the CLI, which adds a way to get it wrong). The
  file parses and merges with `plist` 1.8.0, the crate Tauri uses (`merged-Info.plist`).
- **build-mac.sh** step 6 now runs `scripts/check-macos-infoplist.sh` on the built app
  and stops the build if the key is missing (PlistBuddy on the Mac; an XML fallback lets
  the script be tested here). It also reads the final signature's CodeDirectory flags and,
  if they include `runtime`, requires the `com.apple.security.device.audio-input`
  entitlement. Its closing notes now say macOS may ask for the Microphone permission again
  after each new build (ad-hoc signature), as for Screen Recording.
- **Does the ad-hoc-signed app need `com.apple.security.device.audio-input`? No, as
  build-mac.sh signs it.** That entitlement is a *hardened runtime* resource-access
  exception: Apple's "Configuring the hardened runtime"
  (<https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime>)
  lists "Audio Input" in the table of resources the hardened runtime restricts and that an
  entitlement re-opens, and the entitlement's own page
  (<https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input>)
  files it under the hardened runtime's resource-access entitlements; Apple's
  developer forums trace missing microphone prompts in hardened apps to its absence. The
  hardened runtime is a flag in the code signature (`codesign --options runtime`), and
  build-mac.sh signs with `codesign --force --deep --sign -` without it. Without the
  hardened runtime there is no restriction to lift; macOS still asks the user (TCC) and
  still requires the usage text. One thing I could not confirm from a source: Tauri's
  `bundle.macOS.hardenedRuntime` defaults to true, and whether `codesign --force` without
  `--options` drops a runtime flag Tauri may have set earlier. The new check therefore
  reads the final signature instead of trusting either; if it ever shows `runtime`, the
  build fails unless the entitlement is present.
- **Availability (nothing newer than macOS 13.0).** APIs the new Mac code adds:
  `+[AVCaptureDevice authorizationStatusForMediaType:]` and
  `+requestAccessForMediaType:completionHandler:` (macOS 10.14), `AVMediaTypeAudio`
  (10.7, linked from AVFoundation), `AudioObjectGetPropertyData(Size)` with device-list,
  name, UID, manufacturer, transport, stream-configuration, nominal-rate, latency and
  safety-offset properties (all 10.x; transport codes are compile-time numbers, no symbol),
  `mach_absolute_time` / `mach_timebase_info`, and cpal 0.16 (already used by jk-engine,
  D-37). Apple's voice-processing I/O unit is not used (cpal opens the HAL output unit).
  The binary check (`scripts/check-macos-availability.sh`) runs only on a Mac build, in
  build-mac.sh step 7b: not run here.
- Not done here (Stage 6): the permission flow in the UI, the self-check additions
  (permission state, 3-second capture), the practice rig check.

## MAONO MaonoCaster AME2: what the manual says (WO2 4.10)

Sources read:

- **[M]** MAONO's user manual for model AME2A (the AME2 console sold with a microphone),
  version V1.3-20220214, as hosted for MAONO's Amazon listing:
  <https://m.media-amazon.com/images/I/C1JTQuwVjyL.pdf> (text extracted with pypdf; copy
  in `practice-mic/manual/`, outside the repository).
- **[F]** MAONO's FAQ for the E2/AME2: <https://faq.maono.com/docs/E2-AME2-FAQs>.
- **[L]** MAONO's FAQ "What is loopback": <https://faq.maono.com/docs/what-is-loopback>.
- **[G]** MAONO's FAQ for the newer E2 GEN2 (a different model, used only where marked):
  <https://faq.maono.com/docs/Maonocaster-E2-Gen2-FAQs>.
- Not read: the newer manual MAONO links from its AME2 download page,
  <https://update.maono.com/User%20Manual/AME2Pro%20User%20Manual%20V1.3_20230830.pdf>
  (larger than the fetch tool's 10 MB limit; downloading files needs the owner's
  permission). The owner's labels ("MIC1 SETTING", "LIVE OUTPUT 1") match the FAQ's
  wording rather than the 2022 manual's ("Mic 1 adjustment", "Pod 1,2 output"), so the
  owner's unit may be a later revision. **Read that manual, or check on the unit, before
  relying on anything marked unconfirmed.**

| WO2 4.10 asks | What the sources say | Status |
|---|---|---|
| What the USB feed to the computer contains | The console has two analog outputs and one USB audio output for streaming [M Description]. What goes out is the console's recorded mix: the microphones with their effects and the background music, unless DRY mode is on, which sends only microphone audio [M 8.4]. The computer's own playback is part of it only with LOOPBACK on [M 8.5, L]. That USB and the LIVE outputs carry one and the same master mix is stated only for the E2 GEN2 [G] | mic + effects + background music: confirmed [M]; "same mix on every output": **unconfirmed for the AME2** |
| What the loopback switch does | On: system audio arriving from the computer over USB-C is added to what the console records and sends. Off: that audio is still heard in the headphone monitor but is not recorded [M 8.5]; [L] says the same (off: only the microphone is recorded or transmitted) | confirmed |
| AUX-IN and Bluetooth music in the USB feed | Background music enters by the music (AUX) input or Bluetooth 4.2 [M Description, items 1, 6, 21; Bluetooth is for accompaniment playback only]; in WET mode background audio is recorded and exported to the output devices, in DRY mode it is not [M 8.4]. [F] fixes "the recording has only my voice, no music" with DRY/WET off (wet) and LOOPBACK on | yes in WET mode (default), no in DRY mode: confirmed [M 8.4]; that "output devices" includes the USB port: inferred, not stated |
| Sample rate, bit depth, channels | 48 kHz / 16-bit [M Specification]. Channel count: not stated [M]; [G] says the GEN2 sends one stereo (two-channel) mix | rate and depth confirmed; **channels unconfirmed** (the input module accepts any count and the rig check will report it) |
| Which effects reach the USB feed | Effects on the console [M 8-13]: reverb (6 presets, depth, decay), side chain (ducks the background music under the voice), Music Only (removes the vocal from the background music), Denoise, a PITCH knob that changes the voice from low male to high female, low/middle/high tone controls, 12-key Auto-Tune, 11 sound pads. Reverb reaches the outputs in WET mode [M 8.4]. For the others the manual does not say which outputs they reach | reverb confirmed; **all others unconfirmed** (assume they are in the feed: they are part of the console's mix) |
| Driver | None: the computer recognises the console by itself; compatible with Windows and macOS [M Product features 10, connection step 2] | confirmed |
| MIC1 gain | 40 / 50 / 60 dB switch; dynamic microphones 50 or 60 dB, condenser 40 dB; 48 V phantom has its own button [M items 5, 23, 25; F "Microphone gain switch setting"] | confirmed (matches the owner's "DYNAMIC 60dB, 48V off") |
| LIVE OUTPUT / Pod outputs | Two 3.5 mm outputs that send the mix with reverb to phones [M item 19; F] | confirmed |

### What follows for practice mode (to settle in Stages 3 and 6)

1. **Option 1, "music over USB, loopback off", gives the app a clean voice, but probably
   takes the song off the iPhone recording.** With the AME2 as the Mac's output and
   LOOPBACK off, the owner hears the song in the EarPods and the computer receives no
   song [M 8.5]. But loopback-off audio is "not recorded" [M] and "not transmitted" [L],
   which most likely includes LIVE OUTPUT 1, the owner's iPhone recording (unconfirmed).
   Practising while recording *with the backing track on the iPhone* then needs LOOPBACK
   on, and the app gets its own music back over USB: a digital loop with a fixed level and
   delay, the easiest case for removal (WO2 4.4). The practice rig check must measure both
   switch positions, and the checklist must not tell the owner to switch loopback off
   without saying what it does to the iPhone recording.
2. **Today's wiring (song into AUX or Bluetooth) puts the song in the USB feed** in WET
   mode [M 8.4]. AUX gives a fixed delay (removable); Bluetooth's delay wanders.
3. **DRY mode would give a voice-only feed without reverb** (whether it also bypasses
   Auto-Tune, PITCH and Denoise is not stated: unconfirmed), but the manual says DRY records
   only the microphone while WET sends reverb and background audio to the output devices,
   so DRY most likely strips the music and reverb from the LIVE outputs too [M 8.4; that
   DRY affects every output is inferred, not stated], so it is only for practice without
   recording.
4. **Must be off while practising:** Auto-Tune and the PITCH voice changer (they change
   the pitch: the grade would be meaningless and the app cannot tell). **Should be off:**
   Music Only and Side Chain (they change the returned music's content and level, which
   breaks the "fixed level and delay" that removal relies on) and Denoise (it can swallow
   quiet singing; unconfirmed). Reverb may stay on (the tracker must cope, WO2 9). Which
   PITCH knob position is neutral is not in the manual (**unconfirmed**; probably centre).
5. 48 kHz / 16-bit: the Mac will present the AME2 at 48 kHz; Core Audio hands the app
   floats, Windows its shared-mode float mix.

## Decisions (for DECISIONS.md)

- Microphone input lives in a new module `jk-recorder::input`, behind `InputSource`; tests
  and the probe feed generated or WAV signals through the same rings, meters and channel
  extraction as the microphone.
- Two `rtrb` rings (samples, stamps), whole-block drop when full, discontinuity flag; 2 s
  of capacity; meters on the reading thread. Stall watchdog: 1 s after the last block, 3 s
  for the first.
- Time stamps: QPC from `GetBuffer` on Windows; `mHostTime` (cpal's `callback` instant)
  on macOS, both as host-clock ns of the block's first frame. cpal's `capture` field is not
  used (it subtracts a guessed buffer).
- Windows: shared mode at the mix format, event driven, category Other, raw stream when
  accepted (fallback reported). Bluetooth endpoints are never activated while listing.
- Device choice: saved id, saved name, then USB > wired external > built-in > unknown >
  display; never Bluetooth, virtual, aggregate, AirPlay or Continuity automatically.
- macOS: cpal 0.16 (HAL output unit, no voice processing); permission checked before
  opening, because a denied app gets silence, not an error.
- `InputError` is the input module's own error type (the app's exhaustive match on
  `RecorderError` would break with a new variant; `Unsupported` says "system audio").
- The microphone usage text is in `app/src-tauri/Info.plist` (Tauri merges it); no
  `infoPlist` key in `tauri.conf.json`; no audio-input entitlement, because the app is
  signed without the hardened runtime, with a build check that fails if that changes.

## Not done, not verified

- No microphone exists on this PC: no real capture, format, level or noise-floor
  measurement of a microphone; no measurement of input latency. The Windows capture loop
  ran only on the loopback test path.
- No Bluetooth device was exercised (none connected); the Bluetooth rule is unit-tested.
- Nothing ran on a Mac: listing, capture, permission, Info.plist merge by the real Tauri
  CLI, the availability check and the bundle check are built but not verified.
- The AME2 facts marked unconfirmed (USB channel count, same mix on all outputs, which
  effects reach USB, what loopback does to LIVE OUTPUT, neutral PITCH position) need the
  2023 manual or the rig check.
- Not in this workstream: removal of the returned music, the delay measurement, the rig
  check, the UI, the self-check additions.
- Found by the independent check, left for Stage 3/6 (Mac only, untestable here): cpal
  0.16's input path (coreaudio-rs 0.13 `set_input_callback`) re-reads the stream format
  and may grow its buffer on the audio thread whenever a callback's frame count differs
  from the device buffer size, so "no allocation" holds for this module's callback code,
  not for that library wrapper; and a device with several input streams is listed with the
  sum of their channels but opened by cpal with the first stream's channels only (cpal reads
  `buffers[0]`). Neither matters for a single-stream device such as a USB mixer at its own
  rate; the rig check should report both counts.

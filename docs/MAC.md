# Jukeoke on macOS

The acceptance target is the owner's **MacBook Air (M1, 2020), 8 GB, macOS Ventura
13.6.9** (WORK_ORDER section 4.10). Nothing in this file has run on that machine yet.
Research was done on 2026-11-17; every fact has a source at the bottom (S1, S2, ...).

**State on 2026-11-17 (be exact about this):**

| Item | State |
|---|---|
| `crates/jk-recorder` macOS backend (ScreenCaptureKit) | On Windows: `cargo check` and `cargo clippy -D warnings` for `aarch64-apple-darwin` pass (0 errors, 0 warnings), and the library compiles to ARM64 machine code (`cargo build --lib --target aarch64-apple-darwin`; all 214 object files declare `LC_BUILD_VERSION` macOS, minos 13.0). **Never linked into a program, never run** (no Apple linker/SDK on Windows). |
| `jk-recorder` cross-platform core (WAV/RF64 writer, meter, timeline) | Tested on Windows: 22 unit tests + 1 example test + 1 doc-test pass; float WAV, 6-channel WAVE_FORMAT_EXTENSIBLE and RF64 files read back bit-exact by an independent reader (libsndfile 1.2.2). |
| `.github/workflows/ci.yml`, `release.yml` | Pass `actionlint` 1.7.12 (with shellcheck). **Never run**: no GitHub login (DECISIONS D-09). |
| `scripts/build-mac.sh`, `scripts/check-macos-minos.sh`, `scripts/ci-*.sh` | Pass `shellcheck` 0.11.0. `check-macos-minos.sh` was exercised on Windows against mocked `vtool`/`otool`/`lipo` output. **Not run on a Mac.** |

## 1. Which macOS runner CI uses (and why)

GitHub-hosted runner images in November 2026 (S1, S2):

| Image | Labels | Arch |
|---|---|---|
| macOS 26 | `macos-latest`, `macos-26`, `macos-26-xlarge` | arm64 |
| macOS 26 | `macos-latest-large`, `macos-26-intel`, `macos-26-large` | x64 |
| macOS 15 | `macos-15`, `macos-15-xlarge` | arm64 |
| macOS 15 | `macos-15-large`, `macos-15-intel` | x64 |
| macOS 14 | `macos-14` (x64) | deprecated; all macOS 14 images retired from GitHub Actions on **2026-11-02** |
| Xcode 27 | `xcode-27`, `xcode-27-xlarge` | arm64, preview |

**The oldest Apple Silicon image is `macos-15`** (image 20261104.0514.1: macOS 15.8.2
24H232, default Xcode 16.4, SDKs 15.0 to 26.2, rustup 1.29.3 preinstalled; S3). CI uses
it. It is two major versions newer than the owner's Ventura, so CI proves the build,
not the runtime behaviour on 13.6.9. To close the gap as far as possible:

- everything is compiled with `MACOSX_DEPLOYMENT_TARGET=13.0` (Rust, C, C++);
- `scripts/check-macos-minos.sh` reads the minimum-OS load command of **every** Mach-O
  file in the bundle and fails if any says newer than 13.0;
- every API newer than 13.0 is behind a runtime availability check (the recorder uses
  none; see section 3);
- the app's self-check (WORK_ORDER section 4.10) runs on the real MacBook and writes a
  report the next session reads.

Re-check this table when GitHub announces the macOS 15 retirement; `ci.yml` and
`release.yml` each have the label once (`runs-on: macos-15`; `ci.yml` also names it
in a comment and in its cache key `macos15-arm64-`).

## 2. The web view on Ventura 13.6.9 (is Safari 16.4 the right baseline?)

Tauri uses the system WebKit (`WKWebView`), which updates with the OS (S4).

- Ventura 13.0 shipped Safari/WebKit 16.1; 13.3 brought **16.4** (WebKit 615.1.26);
  13.4.1 had 16.5.1 (S4).
- 13.5 brought 16.6, and **13.6 did not update Safari**: a 13.6 machine reports Safari
  16.6 (18615.3.12.11.2) (S5). The 13.6.x security updates up to 13.6.9 are not listed
  with Safari changes either, so **the lowest WebKit a 13.6.9 Mac can have is 16.6**.
- Apple also shipped Safari 17.x (17.0 to 17.6) and 18.x for Ventura as separate
  updates; Safari 18.6 (2025-07-30) is listed for "macOS Ventura and macOS Sonoma"
  (S6, S7). So, depending on what the owner installed, the range is **16.6 to 18.6**.
- Whether a separately installed Safari update also changes the WebKit that *other*
  apps' `WKWebView` uses on Ventura is not documented in anything I found (S8 shows
  Safari's staged frameworks can be loaded by other processes, which suggests yes).
  The self-check must record the real number: `navigator.userAgent` plus
  `CFBundleVersion` of `/System/Library/Frameworks/WebKit.framework/Resources/Info.plist`
  (the command Tauri documents, S4).

**Conclusion: keep Safari 16.4 as the baseline.** It is two minor releases below the
real floor on 13.6.9 (16.6), so anything that works on 16.4 works on the MacBook, and
the difference costs almost nothing (16.5/16.6 added few web-platform features). No
WebGPU in any of these. ARCHITECTURE.md already builds the UI for `safari16.4`.

## 3. ScreenCaptureKit system audio on macOS 13

The recorder must use ScreenCaptureKit on this Mac: the Core Audio process-tap API
needs macOS 14.2 (WORK_ORDER 4.10, S9).

### What the API offers (Apple's documentation, S10 to S14)

| API | First macOS | Notes |
|---|---|---|
| `SCStreamConfiguration.capturesAudio` | 13.0 | "A stream doesn't capture audio by default." |
| `SCStreamConfiguration.sampleRate` (NSInteger) | 13.0 | "The framework supports sample rates of 8000, 16000, 24000, and 48000. If you don't specify a sample rate, or specify an unsupported value, the system uses a default sample rate of 48 kHz." |
| `SCStreamConfiguration.channelCount` | 13.0 | 1 or 2; anything else becomes stereo. |
| `SCStreamConfiguration.excludesCurrentProcessAudio` | 13.0 | Default `false`. |
| `SCStreamOutputType.audio` | enum 12.3, audio delivered from 13.0 | Header: the sample buffer wraps an AudioBufferList whose format "is based on sampleRate and channelCount". `.microphone` is macOS 15 and is not used. |
| `SCShareableContent`, `SCContentFilter`, `SCStream`, width/height/`minimumFrameInterval`/`showsCursor` | 12.3 | |
| `CGPreflightScreenCaptureAccess`, `CGRequestScreenCaptureAccess` | 10.15 | Permission check without / with the system prompt. |

Consequences for Jukeoke:

- **44.1 kHz cannot be captured as 44.1 kHz.** If the Mac's output device runs at
  44,100 Hz, macOS converts the system mix to 48 kHz before ScreenCaptureKit hands it
  over. **To avoid any conversion, set the output to 48,000 Hz**: open *Audio MIDI
  Setup* (Applications > Utilities), select the output device (e.g. "MacBook Air
  Speakers" or the headphones), and choose *Format: 2 ch 32-bit Float 48,000 Hz*
  (or any 48,000 Hz entry). The recorder requests the device's own rate when it is in
  ScreenCaptureKit's list (so a 48 kHz device gets a conversion-free 48 kHz capture),
  otherwise 48 kHz, and says so in the UI and the report.
- **The real format is read from the audio, not assumed.** The first sample buffer's
  `AudioStreamBasicDescription` decides the file format and is shown to the user.
  Developers report 2-channel 48 kHz Float32 non-interleaved (planar) buffers (S15);
  the code also accepts interleaved layouts and 16/32-bit integer PCM, and refuses
  anything else with a specific error rather than guessing.
- **Minimum video.** ScreenCaptureKit needs a display to capture even for audio, and
  a stream without a screen output is reported to log "stream output NOT found.
  Dropping frame"; developers capture the screen and drop the frames (S16). Jukeoke
  asks for a **2 x 2 pixel** picture at **at most 1 frame per second**
  (`minimumFrameInterval = 1 s`), no cursor, adds a screen output whose frames are
  counted and thrown away unseen, and falls back to 16 x 16 and 128 x 72 if the OS
  rejects a size (each attempt is written to the report). Whether 2 x 2 is accepted
  on 13.6.9 is **unverified**; the self-check report will say which size worked.
- **Keep strong references** to the output and delegate objects; developers traced
  "stream output NOT found" to SCStream holding outputs weakly (S16). The recorder
  keeps them in its session struct.
- **No processing.** Samples are stored bit-for-bit as 32-bit float WAV, promoted to
  RF64 above 4 GiB; the only transformation is planar to interleaved (and the exact
  `x / 32768` mapping if the OS ever delivers 16-bit integers).
- **Sample count = elapsed time.** Each buffer's presentation timestamp is compared
  with the end of the previous one: a jump larger than 10 ms (or half a buffer) is a
  real hole and is filled with exact digital silence; smaller differences are jitter
  or drift and are only measured (re-anchored every buffer), so clock drift can never
  turn into inserted silence. The report shows gaps, jitter, drops and the ratio of
  recording length to OS-clock span.

### The crate

`crates/jk-recorder` (Rust, objc2 0.6 family: `objc2-screen-capture-kit`,
`objc2-core-media`, `objc2-core-audio`, `objc2-core-graphics`, `block2`, `dispatch2`):

```rust
let rec = jk_recorder::Recorder::start(RecorderConfig::new("<root>/Recordings/take.wav"))?;
let status = rec.status();   // state, real format, elapsed, frames, levels, clips, gaps, drops
let done = rec.stop()?;      // closes the file; done.report() is plain text for the self-check
jk_recorder::permission_preflight();   // never prompts
jk_recorder::permission_request();     // may show the system prompt (first time only)
jk_recorder::PERMISSION_EXPLANATION;   // text the UI shows BEFORE asking
jk_recorder::MACOS_SCREEN_RECORDING_SETTINGS_URL;
```

`Recorder` is `Send + Sync`; the Objective-C objects live on a private thread. The
Windows backend (`src/platform/windows.rs`, WASAPI loopback) is implemented and tested on
Windows (DECISIONS D-70 to D-76, docs/benchmarks/recorder.md) and shares the same
`pipeline` (timeline, meter, writer). The app drives either backend through
`app/src-tauri/src/recorder.rs` (APP_API section 8) and the self-check's three-second
capture; on macOS that path is type-checked only, like the backend itself.

Self-check tool: `sck_probe` (an example in the crate; `build-mac.sh` puts it in
`dist/Mac/Tools/sck_probe`):

```
dist/Mac/Tools/sck_probe [--seconds 3] [--request] [--tone] [--keep --out FILE.wav] [--report FILE.txt]
```

It prints macOS version, output device (name, nominal rate, physical bit depth),
permission state, the real capture format, sample count against the OS clock, peak
dBFS, clips, gaps and drops, and with `--tone` plays a 997 Hz tone at -20 dBFS through
`afplay` and measures its frequency in the capture. Exit codes: 0 ok, 1 error,
2 permission not granted, 3 no audio arrived, 4 sample count does not match the OS
clock span of the buffers that arrived, 5 tone did not come back. Exit 4 cannot see
audio missing before the first buffer or after the last one (for example if macOS
sends nothing while the system is silent), so the probe also prints
`length_vs_window`: recording length divided by the wall-clock capture window, with
a warning below 0.9 (a few percent below 1.0 is start-up latency). From Terminal, macOS attributes the permission to
**Terminal**, not to the probe.

## 4. The Screen Recording permission

What the owner will see on Ventura (S17, S18, S19):

1. The first time the app asks, macOS shows: **"“Jukeoke” would like to record this
   computer's screen and audio."** with the text *"Grant access to this application in
   Privacy & Security settings, located in System Settings."* and the buttons **Open
   System Settings** and **Deny**. (The title wording is reported on Ventura 13.3.1
   (S17); the body and buttons were reported on macOS 15 (S18) and match Ventura's
   System Settings naming. Confirm on the MacBook.)
2. In **System Settings > Privacy & Security > Screen Recording**, switch Jukeoke on.
   macOS may ask for the Mac's password. (macOS 15 renamed this list "Screen & System
   Audio Recording"; on Ventura it is "Screen Recording", S19.)
3. macOS then usually offers to quit and reopen the app; the permission applies after
   a relaunch (S20). The wording I remember is *"“Jukeoke” will not be able to record
   the contents of your screen until it is quit."* with **Later** / **Quit & Reopen**;
   I found no source quoting it, so treat it as unverified.

The app must explain this before asking (WORK_ORDER 4.10): the text is
`jk_recorder::PERMISSION_EXPLANATION`. `CGRequestScreenCaptureAccess()` shows the
prompt only once; afterwards the app opens
`x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`.

**After every new build the permission may be asked for again.** An ad-hoc signature
has no team identity; macOS ties the grant to the code signature, and each build has a
new one. Toggle Jukeoke off and on in the same list if recording fails after an update.

## 5. Can CI grant the permission?

- GitHub's own image build writes allow-rows for `kTCCServiceScreenCapture` into the
  **system** and user TCC databases (for `/bin/bash`, the runner provisioner and
  `/opt/hca/hosted-compute-agent`) with `sudo sqlite3`, and pre-approves
  `hosted-compute-agent` in replayd's `ScreenCaptureApprovals.plist` to suppress
  macOS 15's recurring "...is requesting to bypass the system private window picker"
  alert (S21, S22). Writing the system TCC database is only possible with SIP off, so
  the runners evidently run with SIP disabled (the CI log prints `csrutil status`).
- TCC asks the "responsible" process; for CI jobs that is normally the runner agent,
  which is already approved. So **a command-line capture probably works as-is**.
- `scripts/ci-sck-probe.sh` runs `sck_probe` once as-is; if it reports "permission
  denied" it adds a row for the probe (`scripts/ci-grant-screen-capture.sh`, which
  refuses to run outside GitHub Actions) and runs again. The job summary says which
  case happened, or that the runner could not grant it (then the real test is the
  MacBook self-check, as WORK_ORDER section 8 allows).
- Runner VMs may have no audio output device; then the probe reports "no audio" or
  "tone missing" as a warning, not a failure.
- None of this has run yet (no GitHub login).

## 6. App Translocation (quarantined apps run from a random read-only folder)

- An app that carries the `com.apple.quarantine` extended attribute and is launched
  through Finder/LaunchServices without having been moved by Finder is run from a
  randomised read-only copy under `/private/var/folders/.../AppTranslocation/...`
  ("Gatekeeper Path Randomization"). Moving it with Terminal `mv` does not help;
  moving it with Finder or removing the quarantine attribute does (S23, S24).
  From 13.3.1 a quarantined app can keep being translocated on later launches (S24).
- **Detection:** `SecTranslocateIsTranslocatedURL()` (Security.framework, 10.12+, not
  in the public headers, so look it up with `dlsym`) or, simpler and dependency-free,
  check whether the executable path contains `/AppTranslocation/` (S23, S25). The app
  workstream's `portable.rs` has `is_translocated()`; when it is true the app asks
  for the library folder instead of looking next to itself (WORK_ORDER trap 2).
- **When does it happen to the owner?** Only if the app was downloaded on the Mac
  (Safari, AirDrop of a download). A stick filled on the Windows PC, or an app built on
  the Mac with `build-mac.sh`, has **no** quarantine attribute: no translocation and
  no Gatekeeper dialog at all (S26).
- **Fix for a downloaded copy:** in Terminal,
  `xattr -dr com.apple.quarantine "/Volumes/<STICK>/Mac/Jukeoke.app"`.

## 7. First launch of the ad-hoc-signed app on Ventura

Apple's Ventura 13 guide (S27) gives exactly these steps for an app from an
unidentified developer:

1. Find the app **in Finder** (not Launchpad: it has no shortcut menu).
2. **Control-click** the app icon, then choose **Open** from the shortcut menu.
3. Click **Open**.

Alternative from the same page: *System Settings > Privacy & Security*, scroll to
*Security*, click **Open Anyway** (available for about an hour after a blocked
attempt). Ventura allows apps that are only ad-hoc signed, after this override (S28).
Once opened this way the app opens normally from then on. (macOS 15 removed the
Control-click route; irrelevant for 13.6.9.)

The dialog wording on Ventura for a quarantined, ad-hoc-signed, unnotarized app is not
quoted on any Apple page I found. The wording widely reported for Ventura is:
on double-click, *"“Jukeoke” can't be opened because Apple cannot check it for
malicious software."* (buttons *OK* / *Show in Finder*); after Control-click > Open,
*"macOS cannot verify the developer of “Jukeoke”. Are you sure you want to open it?"*
(buttons **Open** / *Cancel*). Treat both as expected, not verified; the MacBook
checklist asks the owner to note what actually appears. Again: a copy that came over
on the stick from Windows is not quarantined and opens with a plain double-click.

## 8. Ad-hoc signing

- Apple Silicon refuses to run code that has no signature at all; the linker already
  ad-hoc signs each executable ("linker-signed"), but the **bundle** must be signed
  after it is assembled, or the Info.plist/resources are not covered.
- `scripts/build-mac.sh` and CI run:
  `codesign --force --deep --sign - Jukeoke.app` then
  `codesign --verify --deep --strict --verbose=2 Jukeoke.app`.
  (`-` is the ad-hoc identity. Tauri can do the same with
  `bundle.macOS.signingIdentity: "-"` or `APPLE_SIGNING_IDENTITY=-`, S29; we sign
  explicitly so the step is visible and the same in CI and on the MacBook.)
- No paid Apple account, no notarization. Consequences: the Gatekeeper override of
  section 7 for downloaded copies, and a new signature per build (section 4).
- CI zips the app with `ditto -c -k --keepParent` so permissions and the signature
  survive the artifact download.

## 9. exFAT

- exFAT has no symlinks and no Unix permissions; macOS stores extended attributes in
  `._name` AppleDouble files (ignore them in library scans). A Tauri bundle has no
  framework symlinks (D-20).
- CI builds an exFAT disk image (`hdiutil create -fs ExFAT -layout MBRSPUD`), lays it
  out like the stick (`Mac/Jukeoke.app`, `Library/`, `Recordings/`, `Settings/`),
  copies the app with `ditto`, re-verifies the signature and the minimum-macOS check
  **on the stick copy**, and launches it from there twice: the executable directly
  (exit code) and via `open` (like a double-click). Crash reports and the unified log
  are collected (`scripts/ci-exfat-launch.sh`).
- **Self-check contract with the app:** CI passes `JK_SELFCHECK_ARGS` (default
  `--smoke-test`, the app's current unattended test mode) and expects the report in
  `<stick>/Settings/` (default `smoke-test.json`, override `JK_SELFCHECK_REPORT`), exit
  code 0 pass / 1 check failed / 2 UI never reported. When the full WORK_ORDER 4.10
  self-check (`--self-check`?) exists, set `JK_SELFCHECK_ARGS` in `ci.yml`.

## 10. Building on the MacBook without GitHub

In Terminal, in the project folder:

```sh
bash scripts/build-mac.sh
```

It checks for Apple's Command Line Tools (opens Apple's installer and stops if they
are missing; run it again afterwards), installs Rust (rustup, into `~/.cargo`) and
Node 24 LTS (nodejs.org, checksum verified, into `~/.local/share/jukeoke-build-tools`)
for the user only if missing, builds for Apple Silicon with minimum macOS 13.0, signs
ad hoc, checks every binary's minimum macOS, and copies the result to
`dist/Mac/Jukeoke.app` and `dist/Mac/Tools/sck_probe`. It never uses `sudo` and never
edits shell profiles. Node 24 needs macOS 13.5 or newer (S30), which 13.6.9 meets.
Options: `--no-install` (never install, just explain), `--skip-app`.

## 11. Sources

- S1 GitHub changelog, "GitHub Actions: macOS 14 runner image retirement", 2026-10-01: https://github.blog/changelog/2026-10-01-github-actions-macos-14-runner-image-retirement/
- S2 actions/runner-images README (image table): https://github.com/actions/runner-images
- S3 macOS 15 arm64 image readme: https://github.com/actions/runner-images/blob/main/images/macos/macos-15-arm64-Readme.md
- S4 Tauri, "Webview Versions": https://v2.tauri.app/reference/webview-versions/
- S5 Mr. Macintosh, "macOS Ventura 13.6 Update (22G120)" (Safari not updated): https://mrmacintosh.com/macos-ventura-13-6-update-22g120-whats-new/ ; TidBITS, Safari 16.6.1 discussion (13.6 reports Safari 16.6, 18615.3.12.11.2): https://talk.tidbits.com/t/safari-16-6-1/24409
- S6 Apple, "About the security content of Safari 17" (Monterey and Ventura): https://support.apple.com/HT213941
- S7 Apple, "About the security content of Safari 18.6" (Ventura and Sonoma, 2025-07-30): https://support.apple.com/124152
- S8 WebKit bug 159958 (staged Safari frameworks loaded by other apps' XPC services): https://bugs.webkit.org/show_bug.cgi?id=159958
- S9 Apple Developer Forums, "Is it possible to get only audio from ScreenCaptureKit?": https://developer.apple.com/forums/thread/718279
- S10 `SCStreamConfiguration.sampleRate`: https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/samplerate
- S11 `SCStreamConfiguration.channelCount`: https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/channelcount
- S12 `SCStreamConfiguration.capturesAudio`: https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/capturesaudio
- S13 `SCStreamConfiguration.excludesCurrentProcessAudio`: https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/excludescurrentprocessaudio
- S14 `SCStreamOutputType` and `minimumFrameInterval`: https://developer.apple.com/documentation/screencapturekit/scstreamoutputtype , https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/minimumframeinterval ; SDK header text as shipped in objc2-screen-capture-kit 0.3.2
- S15 Apple Developer Forums thread 848245 (SCK audio arrives as 2 ch 48 kHz Float32 deinterleaved): https://developer.apple.com/forums/thread/848245
- S16 Apple Developer Forums threads 718279 and 733077 ("stream output NOT found", keep strong references): https://developer.apple.com/forums/thread/733077
- S17 Unity Discussions, prompt seen on Ventura 13.3.1: https://discussions.unity.com/t/unity-hub-would-like-to-record-this-computers-screen-and-audio/915565
- S18 barahn/chirp issue 51 (prompt body and buttons): https://github.com/barahn/chirp/issues/51
- S19 Respondus, Screen Recording permission on macOS 13+: https://respondus.knowledgeowl.com/home/macos-13-ventura-and-newer-screen-recording-permission
- S20 Managing macOS Screen Recording permissions (quit and reopen): https://eusd.knowledgeowl.com/help/managing-macos-screen-recording-permissions
- S21 actions/runner-images `configure-tccdb-macos.sh`: https://github.com/actions/runner-images/blob/main/images/macos/scripts/build/configure-tccdb-macos.sh
- S22 actions/runner-images `utils.sh` (`configure_system_tccdb` uses `sudo sqlite3`): https://github.com/actions/runner-images/blob/main/images/macos/scripts/helpers/utils.sh
- S23 Michael Tsai, "Gatekeeper Path Randomization" (2016): https://mjtsai.com/blog/2016/06/16/gatekeeper-path-randomization
- S24 Eclectic Light, "What causes App Translocation?" (2023): https://eclecticlight.co/2023/05/09/what-causes-app-translocation
- S25 Apple Security source, `SecTranslocate.h`: https://opensource.apple.com/source/Security/Security-58286.31.2/OSX/libsecurity_translocate/lib/SecTranslocate.h.auto.html
- S26 Eclectic Light, "Living with(out) notarization" (2024): https://eclecticlight.co/2024/10/01/living-without-notarization/
- S27 Apple, Mac User Guide for macOS Ventura 13, "Open a Mac app from an unidentified developer": https://support.apple.com/guide/mac-help/open-a-mac-app-from-an-unidentified-developer-mh40616/13.0/mac/13.0
- S28 Eclectic Light, "How does Ventura check the security of known apps and command tools?": https://eclecticlight.co/2023/07/05/how-does-ventura-check-the-security-of-known-apps-and-command-tools/
- S29 Tauri, "macOS Code Signing" (ad-hoc signing with "-"): https://v2.tauri.app/distribute/sign/macos/
- S30 Node.js, "Node.js v22 to v24" (pre-built macOS binaries need 13.5+): https://nodejs.org/en/blog/migrations/v22-to-v24

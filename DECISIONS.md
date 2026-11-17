# Decisions

One line of reasoning each. Newest at the bottom of each section. "WO" = WORK_ORDER.md.

## Owner decisions (WO section 11) - defaults taken

- D-01 Name: "Jukeoke" placeholder, kept in `branding.json` only. Needs a conflict check before release.
- D-02 Mac target: M1, 8 GB, macOS 13.6.9; Apple Silicon only, deployment target 13.0.
- D-03 Licence: GPL-3.0-or-later.
- D-04 Ticker: marker fixed, text slides under it.
- D-05 No microphone recording in v1.
- D-06 Online lyrics lookup: per-song button, never automatic.
- D-07 English first.
- D-08 Guide vocal off by default.
- D-09 Hosting: local git only. `gh` is installed but not logged in; on 2026-11-16 the owner chose to deal with GitHub after the project is finished. CI workflows and `scripts/build-mac.sh` are written anyway.

## Environment and toolchain

- D-10 Rust uses the `x86_64-pc-windows-gnu` host with WinLibs GCC (MSVCRT), not MSVC: installing Visual Studio Build Tools needs an administrator (UAC) prompt nobody is here to approve, and the WO allows only per-user toolchain installs. Side benefit: with static libgcc/libstdc++ the exe needs no Visual C++ runtime on a clean Windows 11 PC.
- D-11 Toolchains are installed per user (rustup with `--no-modify-path`, winget portable packages for uv and WinLibs). Nothing changes the system PATH; `scripts/devenv.sh` sets PATH per shell.
- D-12 PyTorch 2.14.1+cu130 on Python 3.12: verified with a real matmul and STFT on the RTX 5070 Ti (compute capability 12.0, sm_120 in the wheel's arch list). Python 3.12 rather than the system 3.14 for the widest wheel coverage of audio/ML packages.
- D-13 Heavy local state (Python runtimes, venv, model cache, datasets) lives in `../jukeoke-build/`, outside the repository.

## Architecture (see docs/ARCHITECTURE.md)

- D-20 App shell: Tauri 2 + Rust core + Preact/TypeScript UI, as the WO suggests. Small, one codebase, no framework symlinks in the Mac bundle (exFAT-safe).
- D-21 Song file: a small custom chunked container rather than ZIP, because it has two fixed-size settings slots that are updated in place with an A/B sequence number and checksum. Changing the key or the mix then writes a few KB instead of rewriting a 100 MB file, and a stick pulled mid-write can only damage the slot being written. A ZIP cannot do that without breaking its own CRCs.
- D-22 Stems are stored as 24-bit FLAC with a per-stem gain chosen so the peak sits below full scale (headroom kept, gain stored), as WO section 5 asks. 24-bit quantisation noise is about -144 dBFS.
- D-23 The pitch shifter runs on one stereo mix bus (lead x guide + backing + other) instead of per stem: same result for a linear mix, a quarter of the CPU on the M1. Drums bypass it through a latency-matched delay.
- D-24 At 0 semitones the pitch bus bypasses Rubber Band (bit-transparent mix), with an equal-power crossfade to the shifted path when the key changes. Fidelity first.
- D-25 Heavy DSP runs in a render-ahead worker about 150 ms ahead of the device; the device callback only copies and applies master volume. Generous headroom for a fanless, throttling M1.
- D-26 The Studio writes `.jksong` with its own small Python writer; Rust has a reader and writer. Two independent implementations of the spec, cross-checked by tests.

## Stage 0 results (2026-11-17)

- D-30 Rubber Band 4.0.0 vendored unmodified; built-in FFT and resampler forced on every platform (#error guards against vDSP/IPP/FFTW/fast-math), so the Mac runs the same code as Windows. Pitch option HighQuality, formants shift with pitch.
- D-31 DEVIATION from ARCHITECTURE 4 (now updated): every key change crossfades (40 ms, equal-power) between two fixed-pitch R3 instances; no running setPitchScale. Measured: R3's start delay depends on the pitch scale, so a running change would slide the band against the unshifted drums. Rubber Band is idle at 0 semitones; a new instance is primed with 4096 frames of real audio (Rubber Band 4.0.0's reset() is incomplete, measured diff up to 0.25).
- D-32 One graph latency L = longest R3 FFT x 9/8 (4608 frames at 44.1/48 kHz, 9216 at 88.2/96 kHz). Drum and bypass delays equal L exactly; per-key transient-bias tables correct R3's systematic transient placement. Result: drum path offset 0; band-vs-drum mean offset at most 13.6 samples (0.31 ms); single impulses jitter up to +-2 ms at -12. Exactly 0 is not possible through a phase vocoder; drums vs band at +-5/+-12 goes on the listening checklist.
- D-33 Rubber Band is always fed 256-frame chunks (its real-time output depends on chunking); output is bit-identical for any block size. Pitch scales come from a correctly rounded table, transcendental maths in jk-dsp from the pure-Rust libm crate. Cross-platform tolerance: max |diff| 1e-5, RMS -110 dB.
- D-34 A key change is heard about 280 ms after the click (150 ms render-ahead + L 104.5 ms + device buffer). No click; acceptable for a key control. Recorded for the owner.
- D-35 Output resampler: rubato 5.1 sinc (256 taps, Blackman-Harris^2, 256x oversampling, cubic), only when device rate != song rate. Measured THD+N -133.8 dB. Note: R3 itself resamples internally with its "FastestTolerable" BQResampler (about 90 dB SNR) during pitch shifting; that is inside Rubber Band's best engine and not configurable without patching it.
- D-36 WASAPI shared mode at the device mix format (no Windows resampling); exclusive mode tries the song rate first (f32, i32, i24, i16). MMCSS "Pro Audio" on device and render threads. Render worker 150 ms ahead, stem rings 4 s, 10 ms fade-in.
- D-37 macOS audio output via cpal pinned to 0.16: cpal 0.17+ links the macOS 14.2 process-tap API (found by the verifier; a Windows type-check cannot see it). The final Mac binary's imported symbols still need checking in CI.
- D-38 Own minimal FLAC encoder in jk-engine for fixtures (flacenc's Rice parameter cap produced a 24 GB file). The Studio writes FLAC via libsndfile.
- D-39 Tauri 2.12 on x86_64-pc-windows-gnu needed two fixes: an in-house WebView2 loader in C (app/src-tauri/native/webview2_loader.c, GPL) instead of Microsoft's WebView2Loader.dll, and our own application manifest (MinGW's default manifest shadowed). Exe 6.0 MB stripped, imports only Windows system DLLs. Workspace rust-version raised to 1.90 (Tauri needs it).
- D-40 Portable root: the parent of a folder named `Windows` (or `Mac` holding the .app) that contains `Library/` or `portable.txt`. Names compared case-insensitively. No unbounded upward walk (it would find ~/Library on macOS). WebView2 profile in `<root>/Settings/webview-windows`; macOS web view is non-persistent (incognito).
- D-41 WO 4.8 "write nothing to the registry" cannot hold literally: the WebView2 runtime updates its shared HKCU EdgeUpdate `lastrun` value on every launch, and Explorer records MuiCache entries for any windowed exe path. Neither names our app's data; the app itself writes nothing. Reported to the owner.
- D-42 macOS type-checking from Windows uses Zig (PyPI `ziglang`, MIT) as the C compiler only for objc2-exception-helper; no Apple SDK involved.
- D-43 CI targets macos-15 (arm64): GitHub retired macos-14 on 2026-11-02, so it is the oldest Apple Silicon image. Neither macOS 13 nor 14 can be tested in CI; the self-check on the owner's Mac is the real test.
- D-44 Web-view baseline stays Safari 16.4. The true floor on 13.6.9 is WebKit 16.6 (13.6 did not update Safari); separately installed Safari updates can reach 18.6. The self-check records the real version. Build check: every emitted JS file must parse as ES2022.
- D-45 ScreenCaptureKit: capture at the device's nominal rate if SCK supports it (8, 16, 24 or 48 kHz), else 48 kHz with a plain note; the file format comes from the first real buffer. 2x2 px video at 1 fps, discarded. Recordings are float WAV (RF64 above 4 GiB) written progressively to the final path (header patched every 1 s) so a pulled stick loses at most ~1 s.
- D-46 Song file library: serde_json `float_roundtrip` enabled (gains must survive a rewrite bit-exactly). Settings that do not fit the slot trigger a full rewrite with a larger slot. Unknown keys preserved at every level.

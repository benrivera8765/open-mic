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

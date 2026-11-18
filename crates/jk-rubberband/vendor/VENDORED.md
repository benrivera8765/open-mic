# Vendored: Rubber Band Library

| | |
|---|---|
| Project | Rubber Band Library, by Chris Cannam / Particular Programs Ltd (Breakfast Quay) |
| Upstream | https://github.com/breakfastquay/rubberband (mirror of https://hg.sr.ht/~breakfastquay/rubberband) |
| Release tag | `v4.0.0` (latest release tag on 2026-11-17) |
| Commit | `1d95888bec3ae0a17c0c4af791810d5a63f6bc35` (2024-10-25) |
| `RUBBERBAND_VERSION` | `"4.0.0"` (API 3.0) |
| Licence | GPL-2.0-or-later (see `rubberband/COPYING`); used here under GPL-3.0-or-later, the licence of this project |

## What was copied (into `vendor/rubberband/`)

Unmodified upstream files, with **one exception**: `src/common/BQResampler.cpp` carries a
small real-time-safety patch (2026-11-18, marked "Jukeoke patch" in the file, with a notice
of changes at its top as GPL-2.0 section 2a asks):

- `state_for_ratio` fills the state buffer with `assign()` instead of replacing it with a
  newly constructed vector, so the capacity reserved by the constructor is reused;
- in `RatioOftenChanging` mode (the one R3 uses in real time) the constructor reserves the
  phase table for `rational_max + 16` entries instead of twice the sample rate.

Before the patch, the first `process()` of every new stretcher on the render worker
allocated (568 bytes) and freed (the reserved buffer) through `_aligned_malloc`, and one key
at 44.1 kHz grew the phase table by 1.5 MB there. Found by `crates/jk-engine/tests/rt_alloc.rs`
(counting allocator plus CRT import hooks). The output is unchanged: the 18 golden renders
of `scripts/ci-render.sh` are byte-identical before and after, and the jk-dsp/jk-engine
bit-identity tests pass. Re-apply it when updating Rubber Band (or drop it if upstream
stops allocating there; the test will tell).

- `COPYING`, `README.md`, `CHANGELOG`
- `rubberband/` - the public headers
- `single/RubberBandSingle.cpp` - the single-file compilation unit
- `src/common`, `src/faster`, `src/finer`, `src/RubberBandStretcher.cpp`,
  `src/RubberBandLiveShifter.cpp`, `src/rubberband-c.cpp`

Left out (not compiled by the single-file build in our configuration):
`src/ext` (kissfft, speex, getopt, pommier - only used with optional defines we do not set),
`src/test`, `src/jni`, `main/` (command-line tool), `ladspa-lv2/`, `vamp/`, `com/`, `dotnet/`,
`otherbuilds/`, the meson build files.

## How it is compiled

`crates/jk-rubberband/csrc/jk_rubberband_single.cpp` defines `ALREADY_CONFIGURED` and then
sets the configuration itself before including `single/RubberBandSingle.cpp`, so that
**every platform uses the same code**:

- `USE_BUILTIN_FFT` - Rubber Band's own FFT. Upstream's single-file build switches to Apple
  vDSP (`HAVE_VDSP`) on `__APPLE__`; we suppress that so macOS runs exactly the same FFT code
  as Windows. A `#error` guard fails the build if `HAVE_VDSP` is ever defined.
- `USE_BQRESAMPLER` - Rubber Band's own resampler (used inside the R3 engine for the pitch
  shift itself).
- `NO_THREADING`, `NO_THREAD_CHECKS`, `NO_TIMING` - as upstream's single-file build.

Compiler flags (all targets): `-O3 -ffp-contract=off -fno-fast-math -std=c++14`, plus
`-mmacosx-version-min=13.0` for macOS (see `build.rs`). Windows links `libstdc++.a`
statically.

## Known upstream behaviour that Jukeoke works around (measured, see docs/benchmarks/dsp.md)

- `reset()` does not clear all R3 state (output phase, bin classification, guidance): a
  used-then-reset stretcher differs from a fresh one. jk-dsp constructs a fresh stretcher
  for every key change instead (`examples/probe_reset.rs`).
- R3 real-time output depends on how the input is split into `process()` calls when the
  pitch ratio is not a power of two. jk-dsp always calls `process()` with 256 frames
  (`examples/probe_chunking.rs`).
- The start delay depends on the pitch scale (`examples/probe.rs`), and isolated
  transients come out a pitch-dependent number of samples early or late; jk-dsp measures
  and compensates both.

## Updating

Replace the files above with the new release, update this table and the `RUBBERBAND_VERSION`
assertion in `src/lib.rs`, then run `cargo test -p jk-rubberband -p jk-dsp` and regenerate the
DSP golden files only if the change in output is understood (see `docs/benchmarks/dsp.md`).

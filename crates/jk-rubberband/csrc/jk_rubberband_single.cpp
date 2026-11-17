// SPDX-License-Identifier: GPL-3.0-or-later
//
// Single compilation unit for the vendored Rubber Band Library.
//
// Upstream's single/RubberBandSingle.cpp picks Apple's vDSP FFT on macOS and the
// built-in FFT elsewhere. Jukeoke must produce the same samples on Windows (x86-64)
// and macOS (ARM64), so we configure the build ourselves (ALREADY_CONFIGURED) and use
// Rubber Band's built-in FFT and built-in resampler on every platform.

#define ALREADY_CONFIGURED 1

#define USE_BUILTIN_FFT 1
#define USE_BQRESAMPLER 1

#define NO_TIMING 1
#define NO_THREADING 1
#define NO_THREAD_CHECKS 1

#if defined(HAVE_VDSP) || defined(HAVE_IPP) || defined(HAVE_FFTW3) || defined(HAVE_SLEEF) \
    || defined(HAVE_LIBSAMPLERATE) || defined(HAVE_SAMPLERATE) || defined(USE_SPEEX) \
    || defined(USE_KISSFFT) || defined(USE_POMMIER_MATHFUN) || defined(USE_APPROXIMATE_ATAN2)
#error "jk-rubberband: only the built-in FFT and resampler are allowed (cross-platform determinism)"
#endif

#if defined(__FAST_MATH__)
#error "jk-rubberband: never build with -ffast-math (cross-platform determinism)"
#endif

#include "../vendor/rubberband/single/RubberBandSingle.cpp"

// Re-check after the include, in case some header tried to switch backends.
#if defined(HAVE_VDSP)
#error "jk-rubberband: HAVE_VDSP was defined by the vendored sources"
#endif

// ---------------------------------------------------------------------------------
// Small C helpers for the Rust side (build provenance and self-check).

#include <cstring>

extern "C" const char *jk_rubberband_version(void)
{
    return RUBBERBAND_VERSION;
}

// Writes the compiled-in FFT implementations, comma separated, into `buf`
// (NUL-terminated, truncated to `len`). Expected result: "builtin,dft" ("dft" is the
// always-compiled naive fallback for non-power-of-two sizes; R3 uses only powers of two).
// Returns the number of implementations. Not real-time safe (allocates).
extern "C" int jk_rubberband_fft_implementations(char *buf, int len)
{
    std::set<std::string> impls = RubberBand::FFT::getImplementations();
    std::string joined;
    for (const std::string &s : impls) {
        if (!joined.empty()) joined += ",";
        joined += s;
    }
    if (buf && len > 0) {
        std::strncpy(buf, joined.c_str(), size_t(len - 1));
        buf[len - 1] = '\0';
    }
    return int(impls.size());
}

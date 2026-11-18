// SPDX-License-Identifier: GPL-3.0-or-later
//! Build script: compiles the vendored Rubber Band Library (single-file build) with the
//! `cc` crate.
//!
//! * Same code on every platform: built-in FFT + built-in resampler (no vDSP on macOS);
//!   see `csrc/jk_rubberband_single.cpp`.
//! * Same float behaviour on x86-64 and ARM64: `-ffp-contract=off`, never fast-math.
//! * Windows (GNU toolchain): libstdc++ is linked statically so the final exe does not
//!   need `libstdc++-6.dll`.
//! * Cross-checking for macOS from Windows (`cargo check --target aarch64-apple-darwin`):
//!   there is no Apple SDK or Apple linker here, so the C++ compile is skipped with a
//!   warning. `cargo check` never links, so the crate still type-checks. A real macOS build
//!   (CI / `scripts/build-mac.sh`) compiles the C++ normally.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=vendor/rubberband");
    println!("cargo:rerun-if-env-changed=JK_RUBBERBAND_PERTURB");

    // Host Windows, target Apple: no SDK, no linker. Skip the C++ build so that
    // `cargo check --target aarch64-apple-darwin` works. Linking would fail later, which
    // is the honest outcome: you cannot produce a Mac binary on this machine.
    let host_is_windows = host.contains("windows");
    let target_is_apple = target_os == "macos" || target_os == "ios";
    if host_is_windows && target_is_apple {
        println!(
            "cargo:warning=jk-rubberband: host {host} cannot compile C++ for {target}; \
             skipping the Rubber Band build (type-check only, the result cannot be linked). \
             Build on macOS or in CI for a real Mac binary."
        );
        println!("cargo:rustc-cfg=jk_rubberband_not_compiled");
        println!("cargo:rustc-check-cfg=cfg(jk_rubberband_not_compiled)");
        return;
    }
    println!("cargo:rustc-check-cfg=cfg(jk_rubberband_not_compiled)");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file(manifest_dir.join("csrc/jk_rubberband_single.cpp"))
        .include(manifest_dir.join("vendor/rubberband"))
        // Optimise the C++ even in debug builds: tests run the real engine.
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .flag_if_supported("-std=c++14")
        // Determinism across x86-64 and ARM64 (docs/ARCHITECTURE.md section 3).
        .flag("-ffp-contract=off")
        .flag_if_supported("-fno-fast-math")
        // No build-machine path in the binary: __FILE__ in Rubber Band's messages would
        // otherwise carry this folder's absolute path (the build PC's user folder).
        .flag_if_supported(format!(
            "-ffile-prefix-map={}=jk-rubberband",
            manifest_dir.display()
        ));

    // Test-only perturbation build, used once to estimate how far a different
    // compiler/architecture rounding could move the output (docs/benchmarks/dsp.md,
    // "cross-platform tolerance"): x87 maths with 80-bit intermediates instead of SSE2, a
    // much larger rounding change than the last-bit libm differences expected between
    // Windows and macOS. (FMA via -mfma is not usable: it enables AVX, and MinGW GCC cannot
    // align the stack for 256-bit spills, GCC bug 54412, so the code crashes.)
    // Never set this for a real build.
    if env::var("JK_RUBBERBAND_PERTURB").map(|v| v == "1").unwrap_or(false) {
        println!("cargo:warning=jk-rubberband: PERTURBATION BUILD (x87 80-bit maths) - test use only");
        build.flag("-mfpmath=387");
    }

    if target_is_apple {
        // The owner's Mac runs macOS 13; never let the C++ pick up a newer minimum.
        build.flag("-mmacosx-version-min=13.0");
    }

    // Windows GNU: link libstdc++ statically (no libstdc++-6.dll next to the exe).
    // `cpp_link_stdlib(None)` stops cc from emitting a dynamic `-lstdc++`; we then ask for
    // the static archive ourselves. `-bundle` keeps it out of the rlib and links it once
    // at the final link step.
    let windows_gnu = target_os == "windows" && target_env == "gnu";
    if windows_gnu {
        build.cpp_link_stdlib(None::<&str>);
        // libstdc++ needs pthread and libgcc helpers; both come from static archives too
        // (see .cargo/config.toml: +crt-static and -static-libgcc).
        build.define("_WIN32_WINNT", "0x0A00");
    }

    build.compile("jk_rubberband");

    if windows_gnu {
        println!("cargo:rustc-link-lib=static:-bundle=stdc++");
    }
}

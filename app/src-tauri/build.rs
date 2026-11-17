// SPDX-License-Identifier: GPL-3.0-or-later
// Build script of the Jukeoke app crate.
//
// 1. Checks that the literals Tauri forces into tauri.conf.json match branding.json,
//    the single source of truth for the app name (docs/ARCHITECTURE.md section 9).
// 2. Makes sure app/ui/dist exists, so `cargo build --workspace` works even before
//    the UI has been built (a stub page then explains what to do).
// 3. On the Windows GNU target, makes Jukeoke.exe depend on Windows system DLLs only:
//    - links the built-in WebView2 loader (native/webview2_loader.c) instead of
//      importing Microsoft's WebView2Loader.dll;
//    - hides MinGW's default-manifest.o, which the GNU linker cannot merge with
//      our manifest ("multiple non-default manifests").
// 4. Runs tauri-build: config codegen, capabilities, and on Windows the resource
//    file (icon, version info, our application manifest), compiled with windres.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    check_branding(&manifest_dir);
    ensure_ui_dist(&manifest_dir);

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "gnu" {
        build_webview2_loader(&manifest_dir);
        hide_mingw_default_manifest();
    }

    // Our application manifest replaces both Tauri's default one and MinGW's (the
    // file itself carries no comments: it is embedded verbatim):
    // - Common Controls v6: Tauri imports TaskDialogIndirect, which only v6 has;
    // - asInvoker: never ask for administrator rights (WORK_ORDER section 4.8);
    // - supportedOS {8e0f7a12-...}: Windows 10/11, so no compatibility shims;
    // - per-monitor DPI awareness v2: sharp on 4K and mixed-DPI setups;
    // - longPathAware: deep library paths do not hit MAX_PATH (Windows also needs
    //   the system-wide LongPathsEnabled setting for this one).
    let manifest_xml = manifest_dir.join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest_xml.display());
    let manifest = fs::read_to_string(&manifest_xml).expect("read windows-app-manifest.xml");
    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("tauri-build failed");
}

/// Compiles native/webview2_loader.c and links it as a whole archive into the
/// library, so its definitions of the loader functions (and of the `__imp_`
/// pointers the Rust bindings call through) are present before the linker reaches
/// webview2-com-sys's `-lWebView2Loader.dll` import library. Nothing is then taken
/// from that import library and the exe does not import WebView2Loader.dll.
/// Checked after every build by `objdump -p Jukeoke.exe | grep "DLL Name"`
/// (app/scripts/check-windows-exe.sh).
fn build_webview2_loader(manifest_dir: &Path) {
    let source = manifest_dir.join("native/webview2_loader.c");
    println!("cargo:rerun-if-changed={}", source.display());
    cc::Build::new()
        .file(&source)
        .define("UNICODE", None)
        .define("_UNICODE", None)
        .warnings(true)
        .extra_warnings(true)
        .flag_if_supported("-ffp-contract=off")
        // Emit our own link lines below (with the whole-archive modifier).
        .cargo_metadata(false)
        .compile("jk_webview2_loader");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    println!("cargo:rustc-link-search=native={out_dir}");
    // -bundle: passed to the final link instead of being copied into the rlib;
    // +whole-archive: every symbol is linked even though nothing in the Rust code
    // of this crate references them.
    println!("cargo:rustc-link-lib=static:-bundle,+whole-archive=jk_webview2_loader");
    // GetFileVersionInfoW (fixed-version runtimes) lives in version.dll, a system DLL.
    println!("cargo:rustc-link-lib=dylib=version");
}

/// MinGW's GCC links `default-manifest.o` into every executable (see
/// `gcc -dumpspecs`, `*endfile`). Together with our manifest the GNU linker ends up
/// with two manifests. GCC looks for that file in `-B` prefixes first, so an empty
/// object of the same name in a `-B` folder takes its place.
fn hide_mingw_default_manifest() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let dir = out_dir.join("no-default-manifest");
    fs::create_dir_all(&dir).expect("create no-default-manifest dir");
    let empty_c = dir.join("empty.c");
    fs::write(
        &empty_c,
        "/* Stands in for MinGW's default-manifest.o (see build.rs). */\n",
    )
    .expect("write empty.c");
    let objects = cc::Build::new()
        .file(&empty_c)
        .cargo_metadata(false)
        .compile_intermediates();
    let object = objects.first().expect("compiled empty.c");
    fs::copy(object, dir.join("default-manifest.o")).expect("copy default-manifest.o");
    // The trailing separator matters: -B is a prefix, not a directory.
    let mut prefix = dir.display().to_string();
    prefix.push('/');
    println!("cargo:rustc-link-arg=-B{prefix}");
}

fn read_json(path: &Path) -> serde_json::Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Fails the build when tauri.conf.json and branding.json disagree, so a rename can
/// never ship half done. The places Tauri forces a literal are listed in
/// app/README.md and in branding.json `also_edit`.
fn check_branding(manifest_dir: &Path) {
    let branding_path = manifest_dir.join("../../branding.json");
    let conf_path = manifest_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", branding_path.display());
    println!("cargo:rerun-if-changed={}", conf_path.display());

    let branding = read_json(&branding_path);
    let conf = read_json(&conf_path);
    let name = branding["name"].as_str().expect("branding.json: name");
    let bundle_id = branding["bundle_id"]
        .as_str()
        .expect("branding.json: bundle_id");

    let pairs = [
        ("productName", name),
        ("mainBinaryName", name),
        ("identifier", bundle_id),
    ];
    for (key, expected) in pairs {
        let actual = conf[key].as_str().unwrap_or("<missing>");
        if actual != expected {
            panic!(
                "app/src-tauri/tauri.conf.json `{key}` is {actual:?} but branding.json says {expected:?}. \
                 branding.json is the source of truth: update tauri.conf.json (see app/README.md)."
            );
        }
    }
}

/// tauri::generate_context!() embeds app/ui/dist and refuses to compile when it is
/// missing. Write a one-page stub in that case and say so loudly; a real build runs
/// `npm run build` in app/ui first (app/README.md).
fn ensure_ui_dist(manifest_dir: &Path) {
    let dist = manifest_dir.join("../ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    if dist.join("index.html").is_file() {
        return;
    }
    println!(
        "cargo:warning=app/ui/dist is missing: embedding a stub page. Run `npm install && npm run build` in app/ui, then rebuild."
    );
    fs::create_dir_all(&dist).expect("create app/ui/dist");
    fs::write(
        dist.join("index.html"),
        "<!doctype html><meta charset=\"utf-8\"><title>UI not built</title>\
         <p>The web UI was not built when this binary was compiled. \
         Run <code>npm install</code> and <code>npm run build</code> in <code>app/ui</code>, then rebuild.</p>\n",
    )
    .expect("write stub app/ui/dist/index.html");
}

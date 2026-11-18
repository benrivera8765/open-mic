#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# build-mac.sh - build the Mac version of Jukeoke on a Mac, without GitHub.
#
# Written for the owner's MacBook Air (M1, macOS Ventura 13.6.9), and also run
# by CI on every push, so it is tested even when nobody has a Mac at hand.
#
# What it does, in order (each step prints what it is doing and why):
#   1. Checks this is an Apple Silicon Mac running natively (not Rosetta).
#   2. Checks for Apple's Command Line Tools (compiler, codesign, otool).
#      If they are missing it opens Apple's installer and stops; run it again
#      after the installer finishes.
#   3. Finds Rust, or installs it for your user account only (rustup, from
#      https://sh.rustup.rs, into ~/.cargo). No sudo, no system changes.
#   4. Finds Node.js 20 or newer, or downloads Node 24 LTS from nodejs.org
#      into ~/.local/share/jukeoke-build-tools (checksum verified). No sudo.
#   5. Builds the app for Apple Silicon with macOS 13.0 as the minimum system.
#   6. Signs it "ad hoc" (no Apple account needed; Apple Silicon refuses to
#      run code without at least this).
#   7. Checks every program and library inside the app:
#      a. declares macOS 13.0 or older as its minimum (scripts/check-macos-minos.sh);
#      b. links no API newer than macOS 13.0 without a weak reference
#         (scripts/check-macos-availability.sh: imported symbols, Objective-C
#         classes and selectors against the SDK headers' availability annotations);
#      c. contains no path of this Mac (your home folder, this checkout): the
#         build remaps them with --remap-path-prefix, as the Windows build does,
#         so your account name does not travel with every copy of the app
#         (scripts/check-macos-paths.sh).
#   8. Copies the result to dist/Mac/Jukeoke.app (and the recorder self-check
#      tool to dist/Mac/Tools/sck_probe).
#
# Usage:   bash scripts/build-mac.sh [--no-install] [--skip-app]
#   --no-install  never install anything; stop with instructions instead
#   --skip-app    build only the recorder self-check tool, not the app
#
# Environment (optional): JK_NONINTERACTIVE=1 (CI: never open installers),
#   JK_TOOLS_DIR (where Node is unpacked), JK_SKIP_TESTS=1,
#   JK_ALLOW_NEWER_API=1 (step 7b warns instead of stopping),
#   JK_ALLOW_BUILD_PATHS=1 (step 7c warns instead of stopping),
#   JK_AVAILABILITY_IN_CI_STEP=1 (CI runs step 7b as its own step instead).
#
# Takes 5 to 20 minutes the first time (downloads and compiles), then less.
# Needs about 3 GB of free disk space. Never uses sudo.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-apple-darwin"
MIN_MACOS="13.0"
NODE_MAJOR="24"            # Node 24 LTS needs macOS 13.5+; the MacBook runs 13.6.9
NODE_MIN_MAJOR="20"
RUST_MIN="1.85"            # workspace rust-version (edition 2024)
TOOLS_DIR="${JK_TOOLS_DIR:-$HOME/.local/share/jukeoke-build-tools}"
NO_INSTALL=0
SKIP_APP=0

for arg in "$@"; do
    case "$arg" in
        --no-install) NO_INSTALL=1 ;;
        --skip-app) SKIP_APP=1 ;;
        -h|--help) sed -n '4,49p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
    esac
done

step() { printf '\n==> %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
die() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

# The app name lives in branding.json only (docs/ARCHITECTURE.md section 9).
APP_NAME="$(sed -n 's/^[[:space:]]*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$ROOT/branding.json" | head -n 1)"
[ -n "$APP_NAME" ] || die "could not read the app name from branding.json"

# These make every compiler target macOS 13.0: rustc writes it into each
# binary's LC_BUILD_VERSION, and the C/C++ builds (Rubber Band) read it too.
export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"

# ---------------------------------------------------------------------------
step "1/8  Checking the Mac"
[ "$(uname -s)" = "Darwin" ] || die "this script must run on macOS (this is $(uname -s))."
machine="$(uname -m)"
if [ "$machine" != "arm64" ]; then
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
        die "Terminal is running under Rosetta (Intel emulation). Quit Terminal, select it in Finder > Applications > Utilities, choose File > Get Info, untick \"Open using Rosetta\", and try again."
    fi
    die "this is an Intel Mac ($machine). Jukeoke's Mac build targets Apple Silicon only (WORK_ORDER section 4.10)."
fi
info "$(sw_vers -productName) $(sw_vers -productVersion) (build $(sw_vers -buildVersion)), $machine"

# ---------------------------------------------------------------------------
step "2/8  Checking Apple's Command Line Tools (compiler, codesign, otool)"
if ! xcode-select -p >/dev/null 2>&1 || ! xcrun --find clang >/dev/null 2>&1; then
    if [ "$NO_INSTALL" = 1 ] || [ "${JK_NONINTERACTIVE:-0}" = 1 ]; then
        die "the Command Line Tools are missing. Run:  xcode-select --install  then run this script again."
    fi
    info "They are missing. A macOS window will open: click \"Install\" and accept Apple's licence."
    info "When it has finished (5 to 15 minutes), run this script again."
    xcode-select --install || true
    exit 1
fi
info "developer dir: $(xcode-select -p)"
info "$(xcrun clang --version | head -n 1)"
info "macOS SDK: $(xcrun --show-sdk-version 2>/dev/null || echo unknown)"
for tool in codesign ditto otool lipo; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found; reinstall the Command Line Tools (xcode-select --install)."
done

# ---------------------------------------------------------------------------
step "3/8  Checking Rust"
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
    if [ "$NO_INSTALL" = 1 ]; then
        die "Rust (rustup) is not installed. Install it from https://rustup.rs and run again."
    fi
    info "Rust is not installed. Installing it for your user only (into ~/.cargo) from https://sh.rustup.rs ..."
    info "(--no-modify-path: your shell profile is not changed.)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable
fi
version_lt() { # version_lt A B: true if A < B (numeric dotted versions)
    awk -v a="$1" -v b="$2" 'BEGIN { na = split(a, x, "."); nb = split(b, y, ".");
        for (i = 1; i <= 3; i++) { xi = (i <= na) ? x[i] + 0 : 0; yi = (i <= nb) ? y[i] + 0 : 0;
            if (xi < yi) exit 0; if (xi > yi) exit 1 } exit 1 }'
}
rustc_version="$(rustc --version 2>/dev/null | awk '{print $2}' || true)"
if [ -z "$rustc_version" ] || version_lt "$rustc_version" "$RUST_MIN"; then
    if [ "$NO_INSTALL" = 1 ]; then die "Rust $RUST_MIN or newer is needed (found ${rustc_version:-none}). Run: rustup update stable"; fi
    info "Updating Rust (found ${rustc_version:-none}, need $RUST_MIN or newer) ..."
    rustup update stable
    rustup default stable
fi
rustup target add "$TARGET" >/dev/null 2>&1 || true
info "$(rustc --version)"

# ---------------------------------------------------------------------------
step "4/8  Checking Node.js (only needed to build the app's user interface)"
node_ok() {
    command -v node >/dev/null 2>&1 || return 1
    local major
    major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
    [ "$major" -ge "$NODE_MIN_MAJOR" ] 2>/dev/null
}
if [ -x "$TOOLS_DIR/node/bin/node" ]; then
    export PATH="$TOOLS_DIR/node/bin:$PATH"
fi
if [ "$SKIP_APP" = 0 ] && [ -f "$ROOT/app/src-tauri/tauri.conf.json" ] && ! node_ok; then
    if [ "$NO_INSTALL" = 1 ]; then die "Node.js $NODE_MIN_MAJOR or newer is needed. Install it from https://nodejs.org and run again."; fi
    info "Downloading Node.js $NODE_MAJOR LTS for this user only into $TOOLS_DIR ..."
    base="https://nodejs.org/dist/latest-v$NODE_MAJOR.x"
    tmp="$(mktemp -d)"
    curl -fsSL "$base/SHASUMS256.txt" -o "$tmp/SHASUMS256.txt"
    tarball="$(awk '/-darwin-arm64\.tar\.gz$/ {print $2; exit}' "$tmp/SHASUMS256.txt")"
    [ -n "$tarball" ] || die "could not find the macOS arm64 Node.js download in $base/SHASUMS256.txt"
    expected="$(awk -v f="$tarball" '$2 == f {print $1; exit}' "$tmp/SHASUMS256.txt")"
    curl -fSL --progress-bar "$base/$tarball" -o "$tmp/$tarball"
    actual="$(shasum -a 256 "$tmp/$tarball" | awk '{print $1}')"
    [ "$actual" = "$expected" ] || die "Node.js download checksum mismatch (expected $expected, got $actual); try again later."
    info "checksum ok ($actual)"
    mkdir -p "$TOOLS_DIR"
    rm -rf "$TOOLS_DIR/node"
    tar -xzf "$tmp/$tarball" -C "$tmp"
    mv "$tmp/${tarball%.tar.gz}" "$TOOLS_DIR/node"
    rm -rf "$tmp"
    export PATH="$TOOLS_DIR/node/bin:$PATH"
    node_ok || die "Node.js was unpacked but does not run (macOS $(sw_vers -productVersion)); Node $NODE_MAJOR needs macOS 13.5 or newer."
fi
if command -v node >/dev/null 2>&1; then info "node $(node --version), npm $(npm --version)"; else info "node: not needed for this build"; fi

# ---------------------------------------------------------------------------
step "5/8  Building for Apple Silicon, minimum macOS $MIN_MACOS"
cd "$ROOT"
mkdir -p "$ROOT/dist/Mac/Tools"

# Use the exact dependency versions in Cargo.lock when the checkout has one.
LOCKED=""
if [ -f "$ROOT/Cargo.lock" ]; then LOCKED="--locked"; fi

# No path of this Mac in the binaries (the Mac twin of app/scripts/build-windows.sh).
# Without this a Rust release binary holds hundreds of copies of
# /Users/<you>/.cargo/registry/... (panic locations of dependencies), the checkout's
# path (env!("CARGO_MANIFEST_DIR") in developer fallbacks) and the object-file paths the
# linker records for debuggers. Fixes, all for this build only:
#   * --remap-path-prefix for the home folder, the checkout, the target folder and
#     CARGO_HOME (rustc applies the LAST matching rule, so the broad home folder comes
#     first). They go in CARGO_ENCODED_RUSTFLAGS (separated by 0x1f), which every cargo
#     run inherits, including the one the Tauri CLI starts, and which survives spaces
#     in paths; .cargo/config.toml sets no rustflags for this target, so none are lost;
#   * JK_DIST_BUILD=1: portable::SOURCE_DIR becomes None (no CARGO_MANIFEST_DIR);
#   * CARGO_PROFILE_RELEASE_STRIP=debuginfo: no debug info and no linker stabs naming
#     target/.../*.o (the Windows build strips its exe too);
#   * Rubber Band's __FILE__: -ffile-prefix-map in crates/jk-rubberband/build.rs.
# Step 7c fails the build if any such path is left.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [ -n "${RUSTFLAGS:-}" ] || [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
    info "note: RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS from your shell are replaced for this build"
fi
sep="$(printf '\037')"
CARGO_ENCODED_RUSTFLAGS=""
for pair in "$HOME=/home" "$ROOT=/jukeoke" "$TARGET_DIR=/target" "${CARGO_HOME:-$HOME/.cargo}=/cargo"; do
    case "$pair" in =*) continue ;; esac
    CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS:+$CARGO_ENCODED_RUSTFLAGS$sep}--remap-path-prefix=$pair"
done
export CARGO_ENCODED_RUSTFLAGS
export JK_DIST_BUILD=1
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
unset RUSTFLAGS
info "path remapping: home -> /home, checkout -> /jukeoke, target -> /target, cargo -> /cargo"
info "Building the recorder self-check tool (sck_probe) ..."
cargo build --release $LOCKED -p jk-recorder --example sck_probe --target "$TARGET"
PROBE="$TARGET_DIR/$TARGET/release/examples/sck_probe"
[ -x "$PROBE" ] || die "the sck_probe build did not produce $PROBE"

APP=""
if [ "$SKIP_APP" = 1 ]; then
    info "--skip-app given: not building the app."
elif [ ! -f "$ROOT/app/src-tauri/tauri.conf.json" ]; then
    info "SKIPPED: the app (app/src-tauri/tauri.conf.json) is not in this checkout yet."
    info "Only the recorder self-check tool was built."
else
    # Install the UI's npm packages exactly as locked.
    for dir in "$ROOT/app" "$ROOT/app/ui"; do
        if [ -f "$dir/package.json" ]; then
            info "Installing npm packages in ${dir#"$ROOT"/} ..."
            if [ -f "$dir/package-lock.json" ]; then (cd "$dir" && npm ci --no-audit --no-fund); else (cd "$dir" && npm install --no-audit --no-fund); fi
        fi
    done
    # The Tauri command-line tool: the project's own copy if it has one.
    # (An array, so a checkout path with spaces still works.)
    # (bash 3.2 with `set -u` cannot expand an empty array, hence the flag.)
    TAURI=(none)
    have_tauri=0
    for c in "$ROOT/app/ui/node_modules/.bin/tauri" "$ROOT/app/node_modules/.bin/tauri"; do
        if [ -x "$c" ]; then TAURI=("$c"); have_tauri=1; break; fi
    done
    if [ "$have_tauri" = 0 ]; then
        if ! cargo tauri --version >/dev/null 2>&1; then
            if [ "$NO_INSTALL" = 1 ]; then die "the Tauri CLI is missing (npm package @tauri-apps/cli or cargo install tauri-cli)."; fi
            info "Installing the Tauri CLI for this user (cargo install tauri-cli, about 5 minutes) ..."
            cargo install tauri-cli --version "^2" --locked
        fi
        TAURI=(cargo tauri)
    fi
    info "Tauri CLI: $("${TAURI[@]}" --version 2>/dev/null || echo "${TAURI[*]}")"
    info "Building $APP_NAME.app (this compiles the audio engine and the UI) ..."
    # --bundles app: a plain .app, no .dmg (the app runs from a USB stick).
    # TAURI_APP_PATH / TAURI_FRONTEND_PATH tell the CLI where the Rust app and
    # the UI are, instead of letting it search from the current folder.
    (cd "$ROOT/app" && TAURI_APP_PATH="$ROOT/app/src-tauri" TAURI_FRONTEND_PATH="$ROOT/app/ui" \
        "${TAURI[@]}" build --target "$TARGET" --bundles app)
    for d in "$TARGET_DIR/$TARGET/release/bundle/macos" "$ROOT/app/src-tauri/target/$TARGET/release/bundle/macos"; do
        if [ -d "$d/$APP_NAME.app" ]; then
            APP="$d/$APP_NAME.app"
            break
        fi
        if [ -z "$APP" ] && [ -d "$d" ]; then
            first="$(find "$d" -maxdepth 1 -name '*.app' -type d | head -n 1)"
            if [ -n "$first" ]; then APP="$first"; fi
        fi
    done
    [ -n "$APP" ] || die "the Tauri build finished but no .app was found under target/$TARGET/release/bundle/macos"
    info "built: $APP"
fi

# ---------------------------------------------------------------------------
step "6/8  Signing ad hoc (no Apple account; required to run on Apple Silicon)"
# The linker already ad-hoc signs each executable, but the bundle as a whole
# (Info.plist, resources) must be signed after it is assembled.
codesign --force --sign - "$PROBE"
codesign --verify --strict --verbose=1 "$PROBE"
if [ -n "$APP" ]; then
    codesign --force --deep --sign - "$APP"
    codesign --verify --deep --strict --verbose=2 "$APP"
    codesign -dv "$APP" 2>&1 | grep -E '^(Identifier|Format|CodeDirectory|Signature|TeamIdentifier)' | sed 's/^/    /' || true
    plist="$APP/Contents/Info.plist"
    lsmin="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist" 2>/dev/null || echo "not set")"
    info "Info.plist LSMinimumSystemVersion: $lsmin (should be $MIN_MACOS; set bundle.macOS.minimumSystemVersion in tauri.conf.json)"
fi

# ---------------------------------------------------------------------------
step "7/8  Checking every binary: minimum macOS $MIN_MACOS, no newer API, no path of this Mac"
info "7a. declared minimum macOS (scripts/check-macos-minos.sh)"
bash "$ROOT/scripts/check-macos-minos.sh" --max "$MIN_MACOS" --require-arm64 "$PROBE" ${APP:+"$APP"}

if [ "${JK_AVAILABILITY_IN_CI_STEP:-0}" = 1 ]; then
    info "7b. API availability: skipped here, CI runs it as its own step on dist/Mac"
else
    info "7b. no API newer than macOS $MIN_MACOS without a weak reference (scripts/check-macos-availability.sh)"
    set +e
    bash "$ROOT/scripts/check-macos-availability.sh" --max "$MIN_MACOS" \
        --out "$TARGET_DIR/$TARGET/macos-availability" "$PROBE" ${APP:+"$APP"}
    avail=$?
    set -e
    case "$avail" in
        0) info "API availability: ok (evidence in $TARGET_DIR/$TARGET/macos-availability)" ;;
        1) if [ "${JK_ALLOW_NEWER_API:-0}" = 1 ]; then
               info "WARNING: a binary links an API newer than macOS $MIN_MACOS (FAIL lines above); continuing because JK_ALLOW_NEWER_API=1"
           else
               die "a binary links an API newer than macOS $MIN_MACOS without a weak reference (FAIL lines above): it would not open on macOS $MIN_MACOS. If you are sure this is a false alarm, run again with JK_ALLOW_NEWER_API=1 and report it."
           fi ;;
        *) # CI (release.yml runs this step here) must not pass on a check that did not run;
           # on the MacBook a tool problem only warns.
           if [ "${GITHUB_ACTIONS:-}" = true ]; then
               die "the API availability check could not run (exit $avail, see above); CI does not pass an unchecked build"
           fi
           info "WARNING: the API availability check could not run (exit $avail, see above); the app was not checked for APIs newer than macOS $MIN_MACOS" ;;
    esac
fi

info "7c. no path of this Mac in the build (scripts/check-macos-paths.sh)"
forbid=(--forbid "$HOME")
case "$ROOT/" in "$HOME"/*) ;; *) forbid+=(--forbid "$ROOT") ;; esac
if ! bash "$ROOT/scripts/check-macos-paths.sh" "${forbid[@]}" "$PROBE" ${APP:+"$APP"}; then
    if [ "${JK_ALLOW_BUILD_PATHS:-0}" = 1 ]; then
        info "WARNING: the build contains paths of this Mac (above); continuing because JK_ALLOW_BUILD_PATHS=1. Do not give this build to anyone."
    else
        die "the build contains paths of this Mac (your home folder or this checkout; see FOUND lines above), which would publish your account name with the app. Run again with JK_ALLOW_BUILD_PATHS=1 to keep such a build for yourself only, and report it."
    fi
fi

# ---------------------------------------------------------------------------
step "8/8  Copying to dist/Mac"
ditto "$PROBE" "$ROOT/dist/Mac/Tools/sck_probe"
info "dist/Mac/Tools/sck_probe"
if [ -n "$APP" ]; then
    rm -rf "$ROOT/dist/Mac/$APP_NAME.app"
    ditto "$APP" "$ROOT/dist/Mac/$APP_NAME.app"
    codesign --verify --deep --strict "$ROOT/dist/Mac/$APP_NAME.app"
    info "dist/Mac/$APP_NAME.app"
fi

cat <<EOF

Done.

Next steps (docs/MAC.md has the details and the exact wording macOS shows):
  * Copy the whole dist/ folder's contents to the root of the USB stick (Finder
    is fine), so the stick has Mac/$APP_NAME.app next to Library/.
  * An app built on this Mac is not quarantined, so it opens with a normal
    double-click. A copy downloaded from the internet needs Control-click >
    Open > Open the first time.
  * Check the recorder without the app:  dist/Mac/Tools/sck_probe --request --tone
    (from Terminal; macOS will then ask for Screen Recording for "Terminal"; --tone
    plays a 3-second test tone at -20 dBFS, so turn the volume down first).
  * Every new build is a new ad-hoc signature, so macOS may ask for the
    Screen Recording permission again after you install a new build.
EOF

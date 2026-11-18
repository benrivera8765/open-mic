#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Type-checks the app crate for the owner's Mac (aarch64-apple-darwin) on Windows.
#
# This is `cargo check` only: linking a Mac binary needs Apple's SDK and linker,
# so the real Mac build runs on macOS (CI or scripts/build-mac.sh).
#
# One dependency (objc2-exception-helper, pulled in by wry) compiles a tiny
# Objective-C file in its build script, so even `cargo check` needs a C compiler
# that targets macOS. The file includes no headers, so no Apple SDK is involved:
# we use the clang bundled with Zig (PyPI package `ziglang`, MIT licence),
# installed per user into $JK_BUILD/tools-venv.
#
# Usage (Git Bash):  app/scripts/check-mac.sh [extra cargo args]
#                    JK_CARGO_CMD=clippy app/scripts/check-mac.sh
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=/dev/null
source "$repo/scripts/devenv.sh"

py="$JK_BUILD/tools-venv/Scripts/python.exe"
if ! "$py" -I -c "import ziglang" 2>/dev/null; then
  echo "== installing Zig (PyPI ziglang) into $JK_BUILD/tools-venv"
  uv venv "$JK_BUILD/tools-venv" --python 3.12
  uv pip install --python "$py" "ziglang==0.17.1"
fi
rustup target add aarch64-apple-darwin >/dev/null

cross="$repo/app/scripts/macos-cross"
JK_TOOLS_PY="$(cygpath -w "$py")"
CC_aarch64_apple_darwin="$(cygpath -w "$cross/zig-cc.cmd")"
AR_aarch64_apple_darwin="$(cygpath -w "$cross/zig-ar.cmd")"
export JK_TOOLS_PY CC_aarch64_apple_darwin AR_aarch64_apple_darwin
# Keep Zig's cache with the other heavy build files, not in %LOCALAPPDATA%.
export ZIG_GLOBAL_CACHE_DIR="$JK_BUILD/zig-cache"
export ZIG_LOCAL_CACHE_DIR="$JK_BUILD/zig-cache"

# JK_CARGO_CMD=clippy runs clippy instead of check.
cargo "${JK_CARGO_CMD:-check}" --target aarch64-apple-darwin -p jukeoke --manifest-path "$repo/Cargo.toml" "$@"

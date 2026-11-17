#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-setup-windows-gnu.sh - make a GitHub Windows runner build like the dev PC:
# Rust with the x86_64-pc-windows-gnu host, and the same WinLibs GCC 16.2
# (MSVCRT runtime, POSIX threads) that DECISIONS D-10 chose, so C/C++ (Rubber
# Band) is compiled by the same compiler as on the owner's PC.
#
# Runs in Git Bash on the runner. Adds the compiler to $GITHUB_PATH for the
# following steps. Downloads only from the official WinLibs GitHub release and
# checks its SHA-256.

set -euo pipefail

# Exactly the build installed on the dev PC (gcc --version: "MinGW-W64
# x86_64-msvcrt-posix-seh, built by Brecht Sanders, r3) 16.2.0").
WINLIBS_TAG="16.2.0posix-23.1.3-14.0.0-msvcrt-r3"
WINLIBS_ZIP="winlibs-x86_64-posix-seh-gcc-16.2.0-mingw-w64msvcrt-14.0.0-r3.zip"
WINLIBS_SHA256="62af32ea58acdea1318ea5dd8eb7017eaee694dd40b269d52a59c769dcfefb82"
WINLIBS_URL="https://github.com/brechtsanders/winlibs_mingw/releases/download/$WINLIBS_TAG/$WINLIBS_ZIP"

base="$(cygpath -u "${RUNNER_TEMP:-$TEMP}")/winlibs"
if [ ! -x "$base/mingw64/bin/gcc.exe" ]; then
    echo "Downloading WinLibs GCC ($WINLIBS_TAG) ..."
    mkdir -p "$base"
    curl -fsSL --retry 3 -o "$base/$WINLIBS_ZIP" "$WINLIBS_URL"
    echo "$WINLIBS_SHA256  $base/$WINLIBS_ZIP" | sha256sum -c -
    if command -v 7z >/dev/null 2>&1; then
        7z x -y -o"$(cygpath -w "$base")" "$(cygpath -w "$base/$WINLIBS_ZIP")" >/dev/null
    else
        unzip -q "$base/$WINLIBS_ZIP" -d "$base"
    fi
    rm -f "$base/$WINLIBS_ZIP"
fi
export PATH="$base/mingw64/bin:$PATH"
if [ -n "${GITHUB_PATH:-}" ]; then
    cygpath -w "$base/mingw64/bin" >> "$GITHUB_PATH"
fi

echo "Installing the Rust GNU toolchain ..."
rustup set default-host x86_64-pc-windows-gnu
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal --component clippy
rustup default stable-x86_64-pc-windows-gnu

echo "--- toolchain"
rustc -vV
cargo --version
gcc --version | head -n 1
g++ --version | head -n 1

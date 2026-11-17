#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-build-app-windows.sh - build the portable Windows app in CI and run the
# app workstream's portable-mode smoke test when it exists.
#
#   bash scripts/ci-build-app-windows.sh [out_dir]     (default: dist/Windows)
#
# Delegates to app/scripts/build-windows.sh (owned by the app workstream) so CI
# builds exactly the way the dev PC does. Skips cleanly while the app is absent.

set -euo pipefail
cd "$(dirname "$0")/.."
root="$(pwd)"
out="${1:-$root/dist/Windows}"

if [ ! -f app/src-tauri/tauri.conf.json ]; then
    echo "::notice::the app (app/src-tauri) is not present yet; skipping the Windows app build"
    exit 0
fi
if [ ! -f app/scripts/build-windows.sh ]; then
    echo "::warning::app/scripts/build-windows.sh is missing; cannot build the Windows app"
    exit 0
fi

echo "::group::app/scripts/build-windows.sh"
bash app/scripts/build-windows.sh "$out"
echo "::endgroup::"
ls -la "$out"

exe="$(find "$out" -maxdepth 1 -name '*.exe' | head -n 1)"
if [ -z "$exe" ]; then
    echo "::error::the Windows build produced no .exe in $out"
    exit 1
fi
echo "built: $exe ($(stat -c %s "$exe") bytes)"

if [ -f app/scripts/portable-smoke-test.sh ] && [ "${JK_SKIP_SMOKE:-0}" != 1 ]; then
    echo "::group::portable-mode smoke test (app/scripts/portable-smoke-test.sh)"
    smoke_root="$(cygpath -u "${RUNNER_TEMP:-$TEMP}")/portable-test"
    bash app/scripts/portable-smoke-test.sh "$exe" "$smoke_root"
    echo "::endgroup::"
    mkdir -p "$root/ci-out/windows"
    cp -r "$smoke_root-reports" "$root/ci-out/windows/portable-smoke-reports" 2>/dev/null || true
fi

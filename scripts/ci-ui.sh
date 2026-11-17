#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-ui.sh - install, build and test the web UI (app/ui) in CI.
# Skips cleanly with a notice when app/ui does not exist yet.
#
#   bash scripts/ci-ui.sh            npm ci, npm run build, npm test
#   bash scripts/ci-ui.sh --no-test  npm ci, npm run build (the Mac job: the
#                                    Windows job already runs the UI tests)
#
# The build matters to the Rust side too: the Tauri app crate embeds
# app/ui/dist at compile time, so `cargo test --workspace` needs it.

set -euo pipefail
cd "$(dirname "$0")/.."

ui="app/ui"
if [ ! -f "$ui/package.json" ]; then
    echo "::notice::$ui/package.json not present yet; skipping the UI build and tests"
    exit 0
fi
cd "$ui"

has_script() { node -e "process.exit((require('./package.json').scripts || {})[process.argv[1]] ? 0 : 1)" "$1"; }

echo "::group::npm install ($ui)"
node --version
npm --version
if [ -f package-lock.json ]; then
    npm ci --no-audit --no-fund
else
    echo "::warning::$ui has no package-lock.json; using npm install (not reproducible)"
    npm install --no-audit --no-fund
fi
echo "::endgroup::"

if has_script build; then
    echo "::group::npm run build"
    npm run build
    echo "::endgroup::"
else
    echo "::notice::$ui has no build script"
fi

if [ "${1:-}" != "--no-test" ]; then
    if has_script test; then
        echo "::group::npm test"
        npm test
        echo "::endgroup::"
    else
        echo "::notice::$ui has no test script; skipping UI tests"
    fi
fi

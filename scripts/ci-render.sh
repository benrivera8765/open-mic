#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-render.sh - render the generated test songs offline on this platform, for
# the cross-platform comparison (WORK_ORDER section 9, "Same audio on both
# systems": Windows and the macOS runner must match within float tolerance).
#
#   bash scripts/ci-render.sh OUT_DIR
#
# Contract with the audio-engine workstream: `jukeoke-cli` (crate jk-cli) gets
# a subcommand `ci-render OUT_DIR` that renders a fixed set of generated
# fixtures (all key shifts that matter, drums shifted and not) with the exact
# offline renderer the app uses, writing 32-bit float WAV files with stable
# names. scripts/ci-compare-renders.sh then compares the two platforms' files
# by name. Until that subcommand exists this script skips with a notice.

set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:?usage: ci-render.sh OUT_DIR}"
mkdir -p "$out"

if [ ! -f crates/jk-cli/Cargo.toml ]; then
    echo "::notice::crates/jk-cli does not exist yet; skipping the offline render (cross-platform comparison not possible yet)"
    echo "skipped: crates/jk-cli missing" > "$out/SKIPPED.txt"
    exit 0
fi
locked=""
[ -f Cargo.lock ] && locked="--locked"
# shellcheck disable=SC2086
cargo build $locked --release -p jk-cli
cli="$(find "${CARGO_TARGET_DIR:-target}/release" -maxdepth 1 -type f \( -name 'jukeoke-cli' -o -name 'jukeoke-cli.exe' \) | head -n 1)"
if [ -z "$cli" ]; then
    echo "::warning::jk-cli built but ${CARGO_TARGET_DIR:-target}/release/jukeoke-cli was not found; skipping"
    echo "skipped: no jukeoke-cli binary" > "$out/SKIPPED.txt"
    exit 0
fi
if ! "$cli" --help 2>&1 | grep -q 'ci-render'; then
    echo "::notice::jukeoke-cli has no ci-render subcommand yet; skipping the offline render"
    echo "skipped: no ci-render subcommand" > "$out/SKIPPED.txt"
    exit 0
fi
"$cli" ci-render "$out"
echo "rendered:"
ls -la "$out"

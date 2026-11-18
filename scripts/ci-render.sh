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
# by name. Fails if jk-cli, its binary or the subcommand is missing (it exists
# since Stage 1), so a broken render can never look like a skipped one. Skips
# only when explicitly disabled: JK_SKIP_RENDER="<reason>" (writes SKIPPED.txt,
# which makes ci-compare-renders.sh fail unless it is disabled too).

set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:?usage: ci-render.sh OUT_DIR}"
mkdir -p "$out"

if [ -n "${JK_SKIP_RENDER:-}" ]; then
    echo "::notice::offline render explicitly disabled: $JK_SKIP_RENDER"
    echo "skipped: explicitly disabled ($JK_SKIP_RENDER)" > "$out/SKIPPED.txt"
    exit 0
fi
if [ ! -f crates/jk-cli/Cargo.toml ]; then
    echo "::error::crates/jk-cli is missing; the offline render cannot run"
    exit 1
fi
locked=""
[ -f Cargo.lock ] && locked="--locked"
# shellcheck disable=SC2086
cargo build $locked --release -p jk-cli
cli="$(find "${CARGO_TARGET_DIR:-target}/release" -maxdepth 1 -type f \( -name 'jukeoke-cli' -o -name 'jukeoke-cli.exe' \) | head -n 1)"
if [ -z "$cli" ]; then
    echo "::error::jk-cli built but ${CARGO_TARGET_DIR:-target}/release/jukeoke-cli was not found"
    exit 1
fi
if ! "$cli" --help 2>&1 | grep -q 'ci-render'; then
    echo "::error::jukeoke-cli has no ci-render subcommand"
    exit 1
fi
"$cli" ci-render "$out"
echo "rendered:"
ls -la "$out"
ls "$out"/*.wav >/dev/null 2>&1 || { echo "::error::jukeoke-cli ci-render wrote no .wav files into $out"; exit 1; }

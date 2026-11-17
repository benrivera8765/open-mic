#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-test-crates.sh - run the Rust tests in CI.
#
#   bash scripts/ci-test-crates.sh             test each crate in crates/ on its own
#                                              (failures are attributed to a crate;
#                                              the app crate is not built, so this
#                                              works before the UI is built)
#   bash scripts/ci-test-crates.sh --workspace cargo test --workspace (needs
#                                              app/ui/dist when the app crate exists)
#
# Extra cargo arguments can be passed in JK_CARGO_TEST_ARGS (e.g. "--release").
# Uses --locked when Cargo.lock is committed, so CI tests what the lock file says.

set -euo pipefail
cd "$(dirname "$0")/.."

notice() { echo "::notice::$*"; }
error() { echo "::error::$*"; }

if [ ! -f Cargo.toml ]; then
    notice "no Cargo workspace yet; skipping Rust tests"
    exit 0
fi
locked=""
[ -f Cargo.lock ] && locked="--locked"
extra=${JK_CARGO_TEST_ARGS:-}

if [ "${1:-}" = "--workspace" ]; then
    echo "::group::cargo test --workspace"
    # shellcheck disable=SC2086
    cargo test $locked --workspace $extra
    echo "::endgroup::"
    exit 0
fi

failed=""
count=0
for manifest in crates/*/Cargo.toml; do
    [ -f "$manifest" ] || continue
    pkg="$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest" | head -n 1)"
    [ -n "$pkg" ] || continue
    count=$((count + 1))
    echo "::group::cargo test -p $pkg"
    # shellcheck disable=SC2086
    if cargo test $locked -p "$pkg" $extra; then
        echo "PASS $pkg"
    else
        error "cargo test -p $pkg failed"
        failed="$failed $pkg"
    fi
    echo "::endgroup::"
done

if [ "$count" -eq 0 ]; then
    notice "no crates in crates/ yet; nothing to test"
    exit 0
fi
if [ -n "$failed" ]; then
    error "failing crates:$failed"
    exit 1
fi
echo "all $count crate(s) passed"

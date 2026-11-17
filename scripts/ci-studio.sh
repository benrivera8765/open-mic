#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-studio.sh - run the Studio (Python) tests that do not need a GPU.
# Skips cleanly when studio/pyproject.toml does not exist yet.
#
# CI runners have no NVIDIA GPU, so tests marked `gpu` are deselected
# (pytest -m "not gpu"). Uses uv (installed by the workflow) with the lock
# file when there is one.

set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -f studio/pyproject.toml ]; then
    echo "::notice::studio/pyproject.toml not present yet; skipping the Studio tests"
    exit 0
fi
cd studio

frozen=""
[ -f uv.lock ] && frozen="--frozen"

echo "::group::uv sync (studio)"
uv --version
# shellcheck disable=SC2086
uv sync $frozen
echo "::endgroup::"

echo "::group::pytest -m \"not gpu\""
set +e
# shellcheck disable=SC2086
uv run $frozen python -m pytest -m "not gpu" -q
code=$?
set -e
echo "::endgroup::"
case "$code" in
    0) echo "Studio tests passed" ;;
    5) echo "::notice::pytest collected no tests (exit code 5); nothing to run yet" ;;
    *) echo "::error::Studio tests failed (pytest exit code $code)"; exit "$code" ;;
esac

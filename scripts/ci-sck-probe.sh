#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-sck-probe.sh - Stage 0 experiment 3 on the macOS runner: a first
# system-audio capture through ScreenCaptureKit, recording whether the runner
# could grant the Screen Recording permission at all (WORK_ORDER section 8: "If
# the runner cannot grant the capture permission, say so, and rely on the
# self-check for the real test").
#
#   bash scripts/ci-sck-probe.sh [OUT_DIR]        (default: ci-out/sck)
#
# Steps:
#   1. Build the probe (crates/jk-recorder/examples/sck_probe.rs) unless
#      dist/Mac/Tools/sck_probe already exists; check its minimum macOS.
#   2. Record the environment: macOS, SIP state, audio devices.
#   3. Run 1: as the runner image comes (GitHub pre-approves screen capture for
#      its own agent, which is usually the "responsible" process for CI jobs).
#   4. If run 1 reports "permission denied": approve the probe in the runner's
#      TCC database (scripts/ci-grant-screen-capture.sh) and run again.
#   5. Write a summary (also to the GitHub job summary).
#
# The job fails only for results that point at a bug in our code: a crash, an
# internal error with permission granted, or a sample count that does not
# match elapsed time. Environment limits (no permission possible, no audio
# device on the VM, tone not heard) are reported as warnings, because the
# real test is the owner's MacBook (self-check), not a VM.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
out="${1:-ci-out/sck}"
mkdir -p "$out"
summary="$out/SUMMARY.md"
: > "$summary"

target=aarch64-apple-darwin
probe="dist/Mac/Tools/sck_probe"
if [ ! -x "$probe" ]; then
    echo "::group::build sck_probe"
    locked=""
    [ -f Cargo.lock ] && locked="--locked"
    # shellcheck disable=SC2086
    MACOSX_DEPLOYMENT_TARGET=13.0 cargo build $locked --release -p jk-recorder --example sck_probe --target "$target" || {
        echo "::error::could not build sck_probe"
        exit 1
    }
    probe="target/$target/release/examples/sck_probe"
    codesign --force --sign - "$probe"
    echo "::endgroup::"
fi
bash scripts/check-macos-minos.sh --require-arm64 "$probe" | tee "$out/minos.txt"

{
    echo "== sw_vers"; sw_vers
    echo "== uname"; uname -a
    echo "== SIP"; csrutil status 2>&1
    echo "== codesign"; codesign -dv "$probe" 2>&1
    echo "== audio devices (system_profiler SPAudioDataType)"; system_profiler SPAudioDataType 2>&1 | head -n 80
} > "$out/environment.txt" 2>&1
cat "$out/environment.txt"

run_probe() { # $1 = label; prints exit code
    local label="$1" code
    echo "::group::sck_probe ($label)"
    "$probe" --seconds 3 --tone --report "$out/probe-$label.txt" > "$out/probe-$label.log" 2>&1
    code=$?
    cat "$out/probe-$label.log"
    echo "exit code: $code"
    echo "::endgroup::"
    return "$code"
}

describe() {
    case "$1" in
        0) echo "ok: audio captured, sample count matches elapsed time, tone came back" ;;
        1) echo "error (see log)" ;;
        2) echo "permission not granted" ;;
        3) echo "stream started but no audio arrived" ;;
        4) echo "sample count does not match elapsed time" ;;
        5) echo "audio captured but the test tone did not come back (no output device on the VM?)" ;;
        *) echo "crashed or killed (exit $1)" ;;
    esac
}

run_probe initial
code1=$?
granted_by="the runner image"
final=$code1
final_label=initial
if [ "$code1" -eq 2 ]; then
    echo "::group::approve Screen Recording for the probe in the runner's TCC database"
    bash scripts/ci-grant-screen-capture.sh "$probe" 2>&1 | tee "$out/tcc-grant.txt"
    echo "::endgroup::"
    run_probe after-tcc-grant
    final=$?
    final_label=after-tcc-grant
    granted_by="a TCC.db edit (scripts/ci-grant-screen-capture.sh)"
fi

# The result line of the LAST run. (Not a probe-*.log glob: it sorts
# "after-tcc-grant" before "initial" and would report the first run.)
line="$(grep -h '^SCK_PROBE ' "$out/probe-$final_label.log" 2>/dev/null | tail -n 1)"
{
    echo "### ScreenCaptureKit probe (Stage 0, experiment 3)"
    echo
    echo "- runner: $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(uname -m)"
    echo "- first run: exit $code1 - $(describe "$code1")"
    if [ "$code1" -eq 2 ]; then
        echo "- after approving in TCC.db: exit $final - $(describe "$final")"
    fi
    if [ "$final" -eq 2 ]; then
        echo "- **The runner could not grant the Screen Recording permission.** The real capture test is the self-check on the owner's MacBook."
    elif [ "$final" -ne 1 ] && [ "$final" -lt 128 ]; then
        echo "- permission: granted by $granted_by"
    fi
    echo "- probe result: \`${line:-no SCK_PROBE line}\`"
} >> "$summary"
cat "$summary"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && cat "$summary" >> "$GITHUB_STEP_SUMMARY"

case "$final" in
    0) exit 0 ;;
    2|3|5) echo "::warning::sck_probe: $(describe "$final") - environment limit, not failing the job"; exit 0 ;;
    *) echo "::error::sck_probe: $(describe "$final")"; exit 1 ;;
esac

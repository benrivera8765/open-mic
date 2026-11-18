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
#   bash scripts/ci-sck-probe.sh --verdict CODE   (print what CI makes of a probe exit
#                                                  code and exit with it; for the tests)
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
# Verdict: the step passes only when the probe captured audio, the sample count
# matches elapsed time and the test tone came back (exit 0). The ONE exception is
# "permission not granted" (exit 2) after the TCC.db approval was tried: the runner
# cannot grant Screen Recording, which is reported as a warning in the log and in
# the job summary, and the real capture test is the self-check on the owner's
# MacBook. Everything else fails: an error or crash, no audio arriving (3), a
# sample count that does not match elapsed time (4), the tone not coming back (5;
# scripts/ci-mac-audio-device.sh gives the runner an output device first).
# Skips only when explicitly disabled: JK_SKIP_SCK="<reason>".

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

describe() {
    case "$1" in
        0) echo "ok: audio captured, sample count matches elapsed time, tone came back" ;;
        1) echo "error (see log)" ;;
        2) echo "permission not granted" ;;
        3) echo "stream started but no audio arrived" ;;
        4) echo "sample count does not match elapsed time" ;;
        5) echo "audio captured but the test tone did not come back" ;;
        *) echo "crashed or killed (exit $1)" ;;
    esac
}

# verdict CODE: the CI decision for the final probe exit code (0 pass, 3 warn, 1 fail).
verdict() {
    case "$1" in
        0) echo "PASS: $(describe 0)"; return 0 ;;
        2) echo "WARN: $(describe 2): the runner could not grant Screen Recording; the real capture test is the self-check on the owner's MacBook"; return 3 ;;
        *) echo "FAIL: $(describe "$1")"; return 1 ;;
    esac
}

if [ "${1:-}" = "--verdict" ]; then
    verdict "${2:?--verdict needs an exit code}"
    exit $?
fi
if [ -n "${JK_SKIP_SCK:-}" ]; then
    echo "::notice::ScreenCaptureKit probe explicitly disabled: $JK_SKIP_SCK"
    exit 0
fi
if [ "$(uname -s)" != Darwin ]; then
    echo "::error::the ScreenCaptureKit probe needs macOS; this is $(uname -s)"
    exit 1
fi
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
bash scripts/check-macos-minos.sh --require-arm64 "$probe" > "$out/minos.txt" 2>&1
minos_code=$?
cat "$out/minos.txt"
[ "$minos_code" -eq 0 ] || echo "::error::sck_probe declares a minimum macOS newer than 13.0 (or no arm64 slice)"

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
        echo "- **The runner could not grant the Screen Recording permission** (warning, not a failure). The real capture test is the self-check on the owner's MacBook."
    elif [ "$final" -ne 1 ] && [ "$final" -lt 128 ]; then
        echo "- permission: granted by $granted_by"
    fi
    echo "- probe result: \`${line:-no SCK_PROBE line}\`"
} >> "$summary"
cat "$summary"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && cat "$summary" >> "$GITHUB_STEP_SUMMARY"

v="$(verdict "$final")"
vcode=$?
echo "- verdict: $v" | tee -a "$summary"
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && echo "- verdict: $v" >> "$GITHUB_STEP_SUMMARY"
if [ "$minos_code" -ne 0 ]; then exit 1; fi
case "$vcode" in
    0) exit 0 ;;
    3) echo "::warning::sck_probe: $v"; exit 0 ;;
    *) echo "::error::sck_probe: $v"; exit 1 ;;
esac

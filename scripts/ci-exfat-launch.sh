#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-exfat-launch.sh - prove the Mac build runs from an exFAT USB stick
# (WORK_ORDER section 7, trap 3; section 9 "Portability").
#
#   bash scripts/ci-exfat-launch.sh APP_BUNDLE [OUT_DIR]     (default: ci-out/exfat)
#
#   1. Creates an exFAT disk image (MBR, like a stick formatted on Windows),
#      mounts it, and lays it out like the stick: Mac/<App>.app, Library/,
#      Recordings/, Settings/ (Library/ makes the app use portable mode).
#   2. Copies the app onto it with ditto, records what exFAT did to it (the
#      ._ AppleDouble files that carry extended attributes), and re-checks the
#      code signature and every binary's minimum macOS on the stick copy.
#   3. Launches the app from the stick in its headless self-check mode, twice:
#        a. the executable directly (gives the exit code),
#        b. through LaunchServices (`open`), the way a double-click does.
#      The app writes its report into <stick>/Settings/ and quits by itself.
#   4. Collects the reports, crash logs and recent log lines into OUT_DIR.
#
# Self-check contract with the app workstream: JK_SELFCHECK_ARGS (default
# "--smoke-test") are passed to the app; it must write its report into
# <root>/Settings/ (default name smoke-test.json; JK_SELFCHECK_REPORT) and exit:
# 0 pass, 1 a check failed, 2 the UI never reported.
#
# Fails the job when the app cannot be launched from exFAT at all, crashes,
# or its self-check fails. Skips cleanly while no app bundle exists.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
root_repo="$(pwd)"
app="${1:-}"
out="${2:-ci-out/exfat}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
summary="$out/SUMMARY.md"
: > "$summary"
selfcheck_args="${JK_SELFCHECK_ARGS:---smoke-test}"
report_name="${JK_SELFCHECK_REPORT:-smoke-test.json}"
timeout_s="${JK_SELFCHECK_TIMEOUT:-120}"

note() { echo "$*" | tee -a "$summary"; }

if [ -z "$app" ] || [ ! -d "$app" ]; then
    echo "::notice::no app bundle (${app:-none given}); the app is not built yet, so the exFAT launch test is skipped"
    note "exFAT launch: skipped (no app bundle)"
    exit 0
fi
name="$(basename "$app")"
exe_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")"

# run_with_timeout SECONDS CMD...: bash 3.2 has no `timeout`; returns 124 on timeout.
run_with_timeout() {
    local secs="$1"; shift
    "$@" &
    local pid=$! waited=0
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$waited" -ge "$secs" ]; then
            kill -TERM "$pid" 2>/dev/null; sleep 2; kill -KILL "$pid" 2>/dev/null
            wait "$pid" 2>/dev/null
            return 124
        fi
        sleep 1; waited=$((waited + 1))
    done
    wait "$pid"
}

# --- 1. exFAT image -----------------------------------------------------------
img="${RUNNER_TEMP:-/tmp}/jk-exfat-stick.dmg"
rm -f "$img"
echo "::group::create and mount an exFAT image"
if ! hdiutil create -size 2g -fs ExFAT -volname JKSTICK -layout MBRSPUD "$img"; then
    echo "hdiutil create -fs ExFAT failed; formatting a blank image with diskutil instead"
    hdiutil create -size 2g -layout NONE "$img" || { echo "::error::cannot create a disk image"; exit 1; }
    dev="$(hdiutil attach -nomount -noverify "$img" | awk 'NR==1 {print $1}')"
    diskutil eraseDisk ExFAT JKSTICK MBRFormat "$dev" || { echo "::error::cannot format exFAT"; exit 1; }
    hdiutil detach "$dev" >/dev/null || true
fi
attach_out="$(hdiutil attach -nobrowse -noverify -noautoopen "$img")"
echo "$attach_out"
mnt="$(printf '%s\n' "$attach_out" | awk -F'\t' '$NF ~ /^\/Volumes\// {m = $NF} END {print m}' | sed 's/[[:space:]]*$//')"
[ -n "$mnt" ] || mnt="/Volumes/JKSTICK"
fs_line="$(mount | grep -F " on $mnt " || true)"
echo "mounted: $fs_line"
echo "::endgroup::"
note "exFAT image mounted: \`$fs_line\`"
case "$fs_line" in
    *exfat*) ;;
    *) echo "::error::the test volume is not exFAT: $fs_line"; exit 1 ;;
esac
cleanup() { hdiutil detach "$mnt" -force >/dev/null 2>&1 || true; }
trap cleanup EXIT

# --- 2. stick layout and copy ---------------------------------------------------
mkdir -p "$mnt/Mac" "$mnt/Library" "$mnt/Recordings" "$mnt/Settings"
ditto "$app" "$mnt/Mac/$name"
stick_app="$mnt/Mac/$name"
{
    echo "== AppleDouble (._) files exFAT created for extended attributes"
    find "$mnt" -name '._*' | head -n 50
    echo "== symlinks inside the app (exFAT cannot store them; must be none)"
    find "$app" -type l
    echo "== codesign on the stick copy"
    codesign --verify --deep --strict --verbose=2 "$stick_app" 2>&1
    echo "== quarantine attribute on the stick copy (none expected for a CI build)"
    xattr -p com.apple.quarantine "$stick_app" 2>&1 || true
} > "$out/stick-copy.txt" 2>&1
cat "$out/stick-copy.txt"
if ! codesign --verify --deep --strict "$stick_app" 2>/dev/null; then
    echo "::error::the code signature is not valid after copying to exFAT"
    note "codesign on exFAT copy: FAILED"
    exit 1
fi
note "codesign on exFAT copy: valid"
if find "$app" -type l | grep -q .; then
    echo "::error::the app bundle contains symlinks, which exFAT cannot store"
    exit 1
fi
bash "$root_repo/scripts/check-macos-minos.sh" --require-arm64 "$stick_app" > "$out/minos-on-stick.txt" 2>&1
minos_code=$?
cat "$out/minos-on-stick.txt"
note "minimum macOS check on the stick copy: $([ $minos_code -eq 0 ] && echo pass || echo FAIL)"

# --- 3. launch ------------------------------------------------------------------
start_epoch="$(date +%s)"
touch "$out/.start-marker"   # crash reports newer than this belong to this test
failures=0

collect_report() { # $1 = label
    local f="$mnt/Settings/$report_name"
    if [ -f "$f" ]; then
        cp "$f" "$out/$1-$report_name"
        note "$1: report written ($(wc -c < "$f" | tr -d ' ') bytes) -> $1-$report_name"
        return 0
    fi
    # Also accept any self-check report the app may write under another name.
    local other
    other="$(find "$mnt/Settings" -maxdepth 1 -type f \( -iname '*self*check*' -o -iname '*smoke*' \) | head -n 1)"
    if [ -n "$other" ]; then
        cp "$other" "$out/$1-$(basename "$other")"
        note "$1: report written -> $1-$(basename "$other")"
        return 0
    fi
    note "$1: NO report in Settings/"
    return 1
}

echo "::group::launch a: executable directly from exFAT"
rm -rf "$mnt/Settings"; mkdir -p "$mnt/Settings"
# shellcheck disable=SC2086  # selfcheck_args is a list of words
run_with_timeout "$timeout_s" "$stick_app/Contents/MacOS/$exe_name" $selfcheck_args > "$out/launch-direct.log" 2>&1
code_a=$?
cat "$out/launch-direct.log"
echo "::endgroup::"
note "launch a (direct exec from exFAT): exit $code_a"
collect_report launch-direct || true
case "$code_a" in
    0) ;;
    124) echo "::error::the app did not finish its self-check within ${timeout_s}s"; failures=$((failures + 1)) ;;
    *) echo "::error::the app's self-check exited with $code_a when started from exFAT"; failures=$((failures + 1)) ;;
esac

echo "::group::launch b: through LaunchServices (open), like a double-click"
rm -rf "$mnt/Settings"; mkdir -p "$mnt/Settings"
# shellcheck disable=SC2086
run_with_timeout "$timeout_s" open -W -n "$stick_app" --args $selfcheck_args > "$out/launch-open.log" 2>&1
code_b=$?
cat "$out/launch-open.log"
echo "::endgroup::"
note "launch b (open -W from exFAT): open exit $code_b"
if ! collect_report launch-open; then
    echo "::error::launched with open from exFAT, but no self-check report appeared"
    failures=$((failures + 1))
fi
pkill -f "$stick_app/Contents/MacOS/$exe_name" 2>/dev/null || true

# --- 4. diagnostics ---------------------------------------------------------------
mkdir -p "$out/crash-reports"
find "$HOME/Library/Logs/DiagnosticReports" /Library/Logs/DiagnosticReports -type f -newer "$out/.start-marker" 2>/dev/null \
    | grep -i "$exe_name" | while IFS= read -r f; do cp "$f" "$out/crash-reports/"; done
crashes="$(find "$out/crash-reports" -type f | wc -l | tr -d ' ')"
note "crash reports for $exe_name since the test started: $crashes"
[ "$crashes" -eq 0 ] || failures=$((failures + 1))
log show --style compact --start "$(date -r "$start_epoch" '+%Y-%m-%d %H:%M:%S')" \
    --predicate "process == \"$exe_name\"" 2>/dev/null | tail -n 400 > "$out/unified-log.txt" || true

[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "### exFAT launch test"; echo; sed 's/^/- /' "$summary"; } >> "$GITHUB_STEP_SUMMARY"
if [ "$failures" -gt 0 ] || [ "$minos_code" -ne 0 ]; then
    echo "::error::exFAT launch test: $failures failure(s); minimum-macOS check exit $minos_code"
    exit 1
fi
echo "exFAT launch test passed"

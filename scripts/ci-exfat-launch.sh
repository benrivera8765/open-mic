#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-exfat-launch.sh - prove the Mac build runs from an exFAT USB stick
# (WORK_ORDER section 7, trap 3; section 9 "Portability" and "The owner's Mac").
#
#   bash scripts/ci-exfat-launch.sh APP_BUNDLE [OUT_DIR]     (default: ci-out/exfat)
#
#   1. Creates an exFAT disk image (MBR, like a stick formatted on Windows),
#      mounts it, and lays it out like the stick: Mac/<App>.app, Library/,
#      Recordings/, Settings/ (Library/ makes the app use portable mode).
#   2. Copies the app onto it with ditto, records what exFAT did to it (the
#      ._ AppleDouble files that carry extended attributes), and re-checks the
#      code signature and every binary's minimum macOS on the stick copy.
#   3. Approves Screen Recording for the stick copy in the runner's TCC database
#      (scripts/ci-grant-screen-capture.sh; CI only), so the recorder can be tested.
#   4. Launches the app from the stick three times:
#        a. the real self-check (WORK_ORDER 4.10), headless and silent
#           (`--selfcheck --silent`: digital silence, never an audible tone), started
#           as the executable directly, which gives the exit code;
#        b. the same self-check through LaunchServices (`open`), like a double-click;
#        c. the UI smoke test (`--smoke-test`: the web view renders, smoke-test.json).
#      The app writes its reports into <stick>/Settings/ and quits by itself. Each
#      self-check report is judged by scripts/ci-selfcheck-verdict.sh.
#   5. Collects the reports, crash logs and recent log lines into OUT_DIR.
#
# Self-check contract (app/src-tauri/src/lib.rs, selfcheck.rs): `--selfcheck` runs the
# eight items, writes <root>/Settings/selfcheck-<local time>.txt and exits 0 (passed) or
# 1 (failed); `--silent` plays digital silence instead of the -60 dBFS tone.
#
# Fails the job when the app bundle the build step should have produced is missing, when
# the app cannot be launched from exFAT, crashes, or any self-check item fails. The ONE
# exception: if the only failed item is the recorder's Screen Recording permission (the
# runner could not grant it), the step passes with a warning and says so in the job
# summary; the real recorder test is then the self-check on the owner's MacBook.
# Skips (exit 0 with a notice) only when explicitly disabled: JK_SKIP_EXFAT="<reason>".
#
# Environment: JK_SELFCHECK_ARGS (default "--selfcheck --silent"), JK_SMOKE_ARGS (default
# "--smoke-test"; set to "" for no smoke launch), JK_SELFCHECK_TIMEOUT (default 240 s),
# JK_EXFAT_GRANT=0 (do not touch TCC.db).

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
root_repo="$(pwd)"
app="${1:-}"
out="${2:-ci-out/exfat}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
summary="$out/SUMMARY.md"
: > "$summary"
selfcheck_args="${JK_SELFCHECK_ARGS:---selfcheck --silent}"
smoke_args="${JK_SMOKE_ARGS---smoke-test}"
timeout_s="${JK_SELFCHECK_TIMEOUT:-240}"

note() { echo "$*" | tee -a "$summary"; }
write_summary() {
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
        { echo "### exFAT launch test"; echo; sed 's/^/- /' "$summary"; echo; } >> "$GITHUB_STEP_SUMMARY"
    fi
}

if [ -n "${JK_SKIP_EXFAT:-}" ]; then
    echo "::notice::exFAT launch test explicitly disabled: $JK_SKIP_EXFAT"
    note "exFAT launch: SKIPPED, explicitly disabled ($JK_SKIP_EXFAT)"
    write_summary
    exit 0
fi
if [ -z "$app" ] || [ ! -d "$app" ]; then
    echo "::error::no app bundle at '${app:-<none given>}': the build step should have produced it, so the exFAT launch test cannot run"
    note "exFAT launch: FAILED, no app bundle (${app:-none given})"
    write_summary
    exit 1
fi
if [ "$(uname -s)" != Darwin ]; then
    echo "::error::the exFAT launch test needs macOS (hdiutil, codesign); this is $(uname -s)"
    note "exFAT launch: FAILED, not on macOS"
    exit 1
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
    write_summary
    exit 1
fi
note "codesign on exFAT copy: valid"
if find "$app" -type l | grep -q .; then
    echo "::error::the app bundle contains symlinks, which exFAT cannot store"
    write_summary
    exit 1
fi
bash "$root_repo/scripts/check-macos-minos.sh" --require-arm64 "$stick_app" > "$out/minos-on-stick.txt" 2>&1
minos_code=$?
cat "$out/minos-on-stick.txt"
note "minimum macOS check on the stick copy: $([ $minos_code -eq 0 ] && echo pass || echo FAIL)"

start_epoch="$(date +%s)"
touch "$out/.start-marker"   # crash reports newer than this belong to this test
failures=0
permission_warnings=0

# --- 3. Screen Recording for the stick copy (CI only) -------------------------------
if [ "${JK_EXFAT_GRANT:-1}" != 0 ] && [ "${GITHUB_ACTIONS:-}" = true ]; then
    if bash "$root_repo/scripts/ci-grant-screen-capture.sh" "$stick_app" > "$out/tcc-grant.txt" 2>&1; then
        note "Screen Recording: approved for the stick copy in the runner's TCC.db (scripts/ci-grant-screen-capture.sh)"
    else
        note "Screen Recording: the TCC.db edit did not fully succeed (tcc-grant.txt); the recorder item may fail on the permission"
    fi
else
    note "Screen Recording: not pre-approved (not on GitHub Actions, or JK_EXFAT_GRANT=0)"
fi

# --- 4. launches ----------------------------------------------------------------
# judge_selfcheck LABEL EXIT_CODE: keep the newest self-check report and judge it.
judge_selfcheck() {
    local label="$1" code="$2" rep verdict vcode
    mkdir -p "$out/$label-settings"
    find "$mnt/Settings" -maxdepth 2 -type f \( -name '*.txt' -o -name '*.json' -o -name '*.log' \) \
        -exec cp {} "$out/$label-settings/" \; 2>/dev/null || true
    # shellcheck disable=SC2012  # names are selfcheck-<date>_<time>.txt, nothing odd in them
    rep="$(ls -t "$mnt/Settings"/selfcheck-*.txt 2>/dev/null | head -n 1)"
    if [ -n "$rep" ]; then cp "$rep" "$out/$label-selfcheck.txt"; fi
    verdict="$(bash "$root_repo/scripts/ci-selfcheck-verdict.sh" "${rep:-$mnt/Settings/no-selfcheck-report.txt}" "$code")"
    vcode=$?
    printf '%s\n' "$verdict" > "$out/$label-verdict.txt"
    note "$label: $(printf '%s\n' "$verdict" | head -n 1)"
    printf '%s\n' "$verdict" | sed -n '2,$p' >> "$summary"
    case "$vcode" in
        0) ;;
        3) permission_warnings=$((permission_warnings + 1))
           echo "::warning::$label: the self-check passed except the recorder's Screen Recording permission, which this runner could not grant" ;;
        *) failures=$((failures + 1))
           echo "::error::$label: the self-check from exFAT failed: $(printf '%s\n' "$verdict" | head -n 1)" ;;
    esac
}

echo "::group::launch a: self-check, executable directly from exFAT"
rm -rf "$mnt/Settings"; mkdir -p "$mnt/Settings"
# shellcheck disable=SC2086  # selfcheck_args is a list of words
run_with_timeout "$timeout_s" "$stick_app/Contents/MacOS/$exe_name" $selfcheck_args > "$out/launch-direct.log" 2>&1
code_a=$?
cat "$out/launch-direct.log"
echo "::endgroup::"
note "launch a (self-check \`$selfcheck_args\`, direct exec from exFAT): exit $code_a"
judge_selfcheck launch-direct "$code_a"

echo "::group::launch b: self-check through LaunchServices (open), like a double-click"
rm -rf "$mnt/Settings"; mkdir -p "$mnt/Settings"
# shellcheck disable=SC2086
run_with_timeout "$timeout_s" open -W -n "$stick_app" --args $selfcheck_args > "$out/launch-open.log" 2>&1
code_b=$?
cat "$out/launch-open.log"
echo "::endgroup::"
note "launch b (self-check, open -W from exFAT): open exit $code_b"
if [ "$code_b" -eq 124 ]; then judge_selfcheck launch-open 124; else judge_selfcheck launch-open -; fi
pkill -f "$stick_app/Contents/MacOS/$exe_name" 2>/dev/null || true

if [ -n "$smoke_args" ]; then
    echo "::group::launch c: UI smoke test, executable directly from exFAT"
    rm -rf "$mnt/Settings"; mkdir -p "$mnt/Settings"
    # shellcheck disable=SC2086
    run_with_timeout "$timeout_s" "$stick_app/Contents/MacOS/$exe_name" $smoke_args > "$out/launch-smoke.log" 2>&1
    code_c=$?
    cat "$out/launch-smoke.log"
    echo "::endgroup::"
    for f in smoke-test.json smoke-test.png; do
        if [ -f "$mnt/Settings/$f" ]; then cp "$mnt/Settings/$f" "$out/launch-smoke-$f"; fi
    done
    if [ "$code_c" -eq 0 ] && [ -f "$out/launch-smoke-smoke-test.json" ]; then
        note "launch c (UI smoke test \`$smoke_args\` from exFAT): passed, smoke-test.json written"
    else
        note "launch c (UI smoke test from exFAT): FAILED (exit $code_c; smoke-test.json $([ -f "$out/launch-smoke-smoke-test.json" ] && echo written || echo missing))"
        echo "::error::the UI smoke test from exFAT failed (exit $code_c)"
        failures=$((failures + 1))
    fi
fi

# --- 5. diagnostics ---------------------------------------------------------------
mkdir -p "$out/crash-reports"
find "$HOME/Library/Logs/DiagnosticReports" /Library/Logs/DiagnosticReports -type f -newer "$out/.start-marker" 2>/dev/null \
    | grep -i "$exe_name" | while IFS= read -r f; do cp "$f" "$out/crash-reports/"; done
crashes="$(find "$out/crash-reports" -type f | wc -l | tr -d ' ')"
note "crash reports for $exe_name since the test started: $crashes"
[ "$crashes" -eq 0 ] || failures=$((failures + 1))
log show --style compact --start "$(date -r "$start_epoch" '+%Y-%m-%d %H:%M:%S')" \
    --predicate "process == \"$exe_name\"" 2>/dev/null | tail -n 400 > "$out/unified-log.txt" || true

if [ "$permission_warnings" -gt 0 ]; then
    note "**The runner could not grant the Screen Recording permission.** The self-check's Recorder item failed on the permission only; every other item passed. The real recorder test is the self-check on the owner's MacBook (HANDOFF, MacBook checklist)."
fi
write_summary
if [ "$failures" -gt 0 ] || [ "$minos_code" -ne 0 ]; then
    echo "::error::exFAT launch test: $failures failure(s); minimum-macOS check exit $minos_code"
    exit 1
fi
if [ "$permission_warnings" -gt 0 ]; then
    echo "::warning::exFAT launch test passed with a warning: the runner could not grant Screen Recording, so no system-audio capture was tested"
else
    echo "exFAT launch test passed (every self-check item OK)"
fi

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-selfcheck-verdict.sh - read one report of the app's self-check (`--selfcheck`,
# app/src-tauri/src/selfcheck.rs: Settings/selfcheck-<time>.txt) and decide what CI makes
# of it. Used by scripts/ci-exfat-launch.sh; tested by scripts/tests/test-ci-scripts.sh.
#
#   bash scripts/ci-selfcheck-verdict.sh REPORT [EXIT_CODE|-]
#
# EXIT_CODE is the app's exit code (0 passed, 1 failed), or "-" when it is not known
# (a launch through `open`, which does not pass the app's exit code on).
#
# Exit status and the one-line verdict it prints:
#   0  PASS      every expected item is [ OK ] and the report says PASSED
#   3  WARN      the only failure is the Recorder item's Screen Recording permission branch
#                ("Screen Recording permission: ..."), i.e. the CI runner could not grant
#                the permission; everything else passed. CI reports it as a warning and
#                says so in the job summary; the real test is the owner's MacBook.
#   1  FAIL      anything else: no report, an item missing (the self-check did not run to
#                the end), any other failed item, a report that contradicts the exit code,
#                or an exit code other than 0/1 (crash, timeout).
#
# The expected items are the eight of WORK_ORDER 4.10 (override: JK_SELFCHECK_ITEMS, a
# comma-separated list).

set -uo pipefail
report="${1:-}"
code="${2:--}"
expected="${JK_SELFCHECK_ITEMS:-System,Web engine,Memory,Output device,Test tone,Recorder,Demo song,Key change}"

fail() { echo "FAIL: $*"; exit 1; }

[ -n "$report" ] || fail "no report path given"
[ -s "$report" ] || fail "no self-check report ($report missing or empty)"
case "$code" in
    -|0|1) ;;
    124) fail "the app did not finish the self-check in time (killed after the timeout)" ;;
    *) fail "the app exited with code $code (crash or signal), not 0/1" ;;
esac

# Normalise: the report has CRLF line ends (it is meant for Windows Notepad too).
text="$(tr -d '\r' < "$report")"
result="$(printf '%s\n' "$text" | sed -n 's/^result: //p' | head -n 1)"
case "$result" in
    PASSED|FAILED) ;;
    *) fail "the report has no 'result: PASSED/FAILED' line" ;;
esac

# Items: "[ OK ] Name" / "[FAIL] Name", detail on the next (indented) line.
items="$(printf '%s\n' "$text" | awk '
    /^\[ OK \] / { if (name != "") print st "\t" name "\t" det; st = "ok"; name = substr($0, 8); det = ""; next }
    /^\[FAIL\] / { if (name != "") print st "\t" name "\t" det; st = "fail"; name = substr($0, 8); det = ""; next }
    name != "" && det == "" && /^    / { det = substr($0, 5); next }
    END { if (name != "") print st "\t" name "\t" det }
')"
[ -n "$items" ] || fail "the report lists no items"

missing=""
IFS=','
for want in $expected; do
    if ! printf '%s\n' "$items" | awk -F'\t' -v w="$want" '$2 == w { found = 1 } END { exit !found }'; then
        missing="$missing${missing:+, }$want"
    fi
done
unset IFS
[ -z "$missing" ] || fail "the self-check did not run to the end: no item for $missing"

failed="$(printf '%s\n' "$items" | awk -F'\t' '$1 == "fail" { print $2 }')"
nfailed="$(printf '%s\n' "$failed" | grep -c . || true)"

if [ "$nfailed" -eq 0 ]; then
    [ "$result" = PASSED ] || fail "every item is OK but the report says $result"
    [ "$code" = 1 ] && fail "every item is OK but the app exited with 1"
    echo "PASS: all $(printf '%s\n' "$items" | grep -c .) items OK"
    exit 0
fi
[ "$result" = FAILED ] || fail "items failed ($(echo "$failed" | tr '\n' ',' | sed 's/,$//')) but the report says PASSED"
[ "$code" = 0 ] && fail "items failed but the app exited with 0"

if [ "$nfailed" -eq 1 ] && [ "$failed" = Recorder ]; then
    detail="$(printf '%s\n' "$items" | awk -F'\t' '$2 == "Recorder" { print $3 }')"
    case "$detail" in
        "Screen Recording permission: "*)
            echo "WARN: only the Recorder item failed, on the Screen Recording permission (${detail%%. *}); the CI runner could not grant it. Every other item passed."
            exit 3 ;;
    esac
fi
printf 'FAIL: %s item(s) failed:\n' "$nfailed"
printf '%s\n' "$items" | awk -F'\t' '$1 == "fail" { print "  [FAIL] " $2 ": " $3 }'
exit 1

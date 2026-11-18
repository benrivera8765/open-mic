#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# prove-dist.sh - prove that a dist/ folder from scripts/make-dist.sh works from a copy
# under another drive letter (WORK_ORDER 4.8, 9 "Portability", 13).
#
#   bash scripts/prove-dist.sh <input audio> [--dist DIR] [--keep]
#
#   <input audio>  a short Creative Commons excerpt for one real Studio job (lyrics are
#                  transcribed, so the whole pipeline runs)
#   --dist DIR     the dist folder (default: dist/ in the repository)
#   --keep         keep the copy and the drive mapping (default: both removed at the end)
#
# Steps, logs in $JK_BUILD/dist-proof/<time>/ (summary.json at the end):
#   1. snapshot of the per-user folders (scripts/user-dirs-audit.py)
#   2. dist copied to $JK_BUILD/dist-proof/<time>/copy, then dist renamed to dist.hidden for
#      the whole run (a hidden reference to the build location would fail instead of
#      silently working), the copy mapped to the first free drive letter with subst
#   3. <X:>\Windows\<name>.exe --smoke-test (report checked by verify-smoke-report.mjs)
#   4. <X:>\Windows\<name>.exe --selfcheck --silent (digital silence on the default output)
#   5. scripts/check-studio-runtime.sh <X:>/ (static and GPU checks, sentinel profile)
#   6. one real Studio job through the app, driven over WebView2 DevTools
#      (app/ui/scripts/real-app-e2e.mjs queue; null output device, master 0), the input
#      copied to <X:>\Recordings first; the new song checked with jukeoke-cli verify --strict
#   7. every Library song opened and played on the null device (real-app-e2e.mjs stage)
#   8. second snapshot and diff of the per-user folders
#   9. logs and reports copied out, mapping removed, copy deleted, dist renamed back
#
# Nothing is played aloud: the self-check runs --silent (master 0), the app runs use the
# engine's silent null:48000 device at master 0.

set -uo pipefail
# MSYS_NO_PATHCONV=1 only for subst: Git Bash would turn its "/D" into a path.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO/scripts/devenv.sh"

INPUT=""
DIST="$REPO/dist"
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dist) DIST="$2"; shift ;;
        --keep) KEEP=1 ;;
        -h|--help) sed -n '4,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) INPUT="$1" ;;
    esac
    shift
done
[ -f "$INPUT" ] || { echo "usage: prove-dist.sh <input audio> [--dist DIR] [--keep]" >&2; exit 2; }
DIST="$(cd "$DIST" && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$JK_BUILD/target-dist}"
PY="$JK_BUILD/python/cpython-3.12.16-windows-x86_64-none/python.exe"
NAME="$("$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["name"])' "$(cygpath -w "$REPO/branding.json")" | tr -d '\r')"
STAMP="$(date +%Y%m%d-%H%M%S)"
LOG="$JK_BUILD/dist-proof/$STAMP"
COPY="$LOG/copy"
mkdir -p "$LOG"
SUMMARY="$LOG/summary.txt"
: > "$SUMMARY"
note() { echo "$*" | tee -a "$SUMMARY"; }
T0=$SECONDS
FAILS=0
result() { # name rc detail
    local tag=PASS
    [ "$2" -eq 0 ] || { tag=FAIL; FAILS=$((FAILS + 1)); }
    note "$tag  $1 (exit $2, $((SECONDS - STEP_T)) s): $3"
}
step() { echo; echo "==== $* ($(date +%H:%M:%S))"; STEP_T=$SECONDS; }

CLI="$CARGO_TARGET_DIR/release/jukeoke-cli.exe"
if [ ! -x "$CLI" ]; then
    step "jukeoke-cli (for verify --strict)"
    cargo build --release -p jk-cli --manifest-path "$REPO/Cargo.toml" > "$LOG/build-cli.log" 2>&1 || { echo "jk-cli build failed" >&2; exit 1; }
fi

DRIVE=""
HIDDEN=""
# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() {
    if [ "$KEEP" -eq 0 ]; then
        [ -n "$DRIVE" ] && MSYS_NO_PATHCONV=1 subst "$DRIVE:" /D 2>/dev/null && echo "removed $DRIVE:"
        if [ -d "$COPY" ]; then
            for _ in $(seq 1 30); do rm -rf "$COPY" 2>/dev/null && break; sleep 1; done
            [ -d "$COPY" ] && echo "WARNING: could not delete $COPY" || echo "deleted the copy"
        fi
    fi
    if [ -n "$HIDDEN" ] && [ -d "$HIDDEN" ] && [ ! -e "$DIST" ]; then mv "$HIDDEN" "$DIST" && echo "dist renamed back"; fi
}
trap cleanup EXIT

note "dist proof $STAMP: $DIST -> copy, input $(basename "$INPUT")"
note "app: $NAME; logs: $LOG"

# ------------------------------------------------------------------ 1. audit before
step "1. per-user folders, before"
"$PY" -I "$(cygpath -w "$REPO/scripts/user-dirs-audit.py")" snapshot "$(cygpath -w "$LOG/user-before.json")" \
    --stick "$(cygpath -w "$COPY")" > "$LOG/audit-before.log" 2>&1
result "snapshot before" $? "$(tail -n 1 "$LOG/audit-before.log")"

# ------------------------------------------------------------------ 2. copy + subst
step "2. copy and map to a free drive letter"
cp -a "$DIST" "$COPY"
rc=$?
result "copy dist -> fresh folder" $rc "$(du -sb "$COPY" | cut -f1) bytes"
[ $rc -eq 0 ] || exit 1
HIDDEN="$DIST.hidden"
mv "$DIST" "$HIDDEN" || { echo "could not rename $DIST" >&2; exit 1; }
for l in Q R S T U V W X Y Z P O N M L K; do
    if [ ! -d "$l:/" ]; then DRIVE="$l"; break; fi
done
[ -n "$DRIVE" ] || { echo "no free drive letter" >&2; exit 1; }
subst "$DRIVE:" "$(cygpath -w "$COPY")"
result "subst $DRIVE: -> copy" $? "$(subst | tr -d '\r' | grep "^$DRIVE:" | sed 's/=>.*copy/=> ...\\copy/')"
X="$DRIVE:"
EXE="$X/Windows/$NAME.exe"
[ -x "$EXE" ] || { echo "no $EXE" >&2; exit 1; }

# ------------------------------------------------------------------ 3. smoke test
step "3. $NAME.exe --smoke-test from $X"
"$EXE" --smoke-test > "$LOG/smoke.stdout" 2>&1
code=$?
cp "$X/Settings/smoke-test.json" "$LOG/smoke-test.json" 2>/dev/null
node "$(cygpath -w "$REPO/app/scripts/verify-smoke-report.mjs")" "$(cygpath -w "$LOG/smoke-test.json")" "$X\\" "$code" > "$LOG/smoke-verify.log" 2>&1
result "--smoke-test (app exit $code)" $? "$(grep -c '^ *ok ' "$LOG/smoke-verify.log") checks ok, $(grep -c '^ *FAIL' "$LOG/smoke-verify.log") failed"

# ------------------------------------------------------------------ 4. self-check
step "4. $NAME.exe --selfcheck --silent from $X"
"$EXE" --selfcheck --silent > "$LOG/selfcheck.stdout" 2>&1
code=$?
rep="$(find "$X/Settings" -maxdepth 1 -name 'selfcheck-*.txt' -printf '%T@ %p\n' | sort -rn | head -n 1 | cut -d' ' -f2-)"
[ -n "$rep" ] && cp "$rep" "$LOG/"
result "--selfcheck --silent" "$code" "report $(basename "${rep:-none}"); $(grep -c '^\[ OK \]' "${rep:-/dev/null}") items OK, $(grep -c '^\[FAIL\]' "${rep:-/dev/null}") failed; $(grep -m 1 '^result:' "${rep:-/dev/null}")"

# ------------------------------------------------------------------ 5. runtime check
step "5. Studio runtime check from $X"
mkdir -p "$LOG/sentinel"
bash "$REPO/scripts/check-studio-runtime.sh" "$X/" --out "$LOG/runtime-check" --sentinel-profile "$LOG/sentinel" \
    > "$LOG/runtime-check.log" 2>&1
code=$?
result "check-studio-runtime.sh $X/" "$code" "$(tail -n 1 "$LOG/runtime-check.log")"
grep -E "^(FAIL|WARN) " "$LOG/runtime-check.log" | tee -a "$SUMMARY"

# ------------------------------------------------------------------ 6. real Studio job
step "6. one real Studio job through the app"
cp "$INPUT" "$X/Recordings/"
in_w="$(cygpath -w "$X/Recordings/$(basename "$INPUT")")"
cat > "$LOG/jobs.json" <<EOF
[{ "input": $("$PY" -I -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$in_w" | tr -d '\r'), "title": "Feel (Stripped) excerpt", "artist": "Cortez", "lyrics_text_file": null }]
EOF
(cd "$REPO/app/ui" && node scripts/real-app-e2e.mjs queue "$(cygpath -w "$EXE")" "$X\\" "$(cygpath -w "$LOG/queue")" "$(cygpath -w "$LOG/jobs.json")") \
    > "$LOG/queue.log" 2>&1
code=$?
result "Studio job via the app (real-app-e2e.mjs queue)" "$code" "$(grep -E '^(PASS|FAIL) job' "$LOG/queue.log" | head -n 1)"
grep -E '^(PASS|FAIL)' "$LOG/queue.log" | tee -a "$SUMMARY"
for s in "$X"/Library/*.jksong; do
    [ "$(basename "$s")" = demo-song.jksong ] && continue
    "$CLI" verify --strict "$s" > "$LOG/verify-$(basename "$s" .jksong).log" 2>&1
    result "jukeoke-cli verify --strict $(basename "$s")" $? "$(tail -n 1 "$LOG/verify-$(basename "$s" .jksong).log")"
    "$CLI" info "$s" > "$LOG/info-$(basename "$s" .jksong).txt" 2>&1
done

# ------------------------------------------------------------------ 7. play
step "7. play every Library song on the null device"
(cd "$REPO/app/ui" && node scripts/real-app-e2e.mjs stage "$(cygpath -w "$EXE")" "$X\\" "$(cygpath -w "$LOG/stage")") \
    > "$LOG/stage.log" 2>&1
code=$?
result "play the songs (real-app-e2e.mjs stage)" "$code" "$(grep -c '^PASS' "$LOG/stage.log") PASS, $(grep -c '^FAIL' "$LOG/stage.log") FAIL"
grep -E '^(PASS|FAIL)' "$LOG/stage.log" | tee -a "$SUMMARY"

# ------------------------------------------------------------------ 8. audit after
step "8. per-user folders, after"
sleep 3   # let the WebView2 helper processes exit
"$PY" -I "$(cygpath -w "$REPO/scripts/user-dirs-audit.py")" snapshot "$(cygpath -w "$LOG/user-after.json")" \
    --stick "$(cygpath -w "$COPY")" > "$LOG/audit-after.log" 2>&1
"$PY" -I "$(cygpath -w "$REPO/scripts/user-dirs-audit.py")" diff "$(cygpath -w "$LOG/user-before.json")" \
    "$(cygpath -w "$LOG/user-after.json")" --out "$(cygpath -w "$LOG/user-dirs-diff.json")" > "$LOG/audit-diff.log" 2>&1
result "per-user folder audit" $? "$(grep -i -E 'app_or_studio|summary|total' "$LOG/audit-diff.log" | head -n 3 | tr '\n' ' ')"
sentinel_files=$(find "$LOG/sentinel" -type f | wc -l)
note "sentinel profile (runtime check children): $sentinel_files files"

# ------------------------------------------------------------------ 9. collect
step "9. collect logs"
mkdir -p "$LOG/stick-settings"
cp -r "$X/Settings/logs" "$LOG/stick-settings/" 2>/dev/null
cp "$X"/Settings/*.txt "$X"/Settings/*.json "$LOG/stick-settings/" 2>/dev/null
du -s --block-size=1 "$X"/Settings/* 2>/dev/null | sed "s|$X/||" > "$LOG/stick-settings/sizes.txt"
ls -la "$X/Library" > "$LOG/stick-library.txt"
note "Settings on the copy after the run: $(tr '\n' ' ' < "$LOG/stick-settings/sizes.txt")"
note "total $((SECONDS - T0)) s, $FAILS failed"
exit $((FAILS > 0))

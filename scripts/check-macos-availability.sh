#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# check-macos-availability.sh - fail if any Mach-O in the Mac build strongly references an
# API that macOS 13.0 does not have (WORK_ORDER 4.10 and trap 14; DECISIONS D-37).
#
# Why: check-macos-minos.sh proves each binary SAYS it runs on 13.0. This proves it does
# not LINK anything newer: a strong import of a C function, constant or Objective-C class
# introduced in macOS 13.1 or later launches on the CI runner (macOS 15) and fails on the
# owner's MacBook (13.6.9) with "dyld: Symbol not found". cpal 0.17's process-tap code was
# exactly that (D-37).
#
# Usage:
#   scripts/check-macos-availability.sh [--max 13.0] [--sdk DIR] [--out DIR] PATH [PATH...]
#
#   --max V    newest allowed "introduced" version (default: $JK_MACOS_MIN or 13.0)
#   --sdk DIR  macOS SDK to read the headers from (default: xcrun --show-sdk-path)
#   --out DIR  keep the evidence there: sdk-availability.json (the parsed headers), one
#              nm/otool output per binary, availability-report.txt and .json
#
# Steps:
#   1. scripts/check-macos-availability.py scan-sdk: every header of the SDK's frameworks
#      and usr/include -> the macOS version each C function, constant, Objective-C class
#      and selector was introduced in. Refuses to go on (exit 2) if it finds suspiciously
#      little, or misses known canaries (AudioHardwareCreateProcessTap = 14.2,
#      SCContentSharingPicker = 14.0; override with JK_AVAIL_CANARIES).
#   2. For every Mach-O under PATH: `nm -m -u` (undefined symbols, weak or not; falls back
#      to `dyld_info -imports`), `otool -v -s __DATA __objc_selrefs` (+ __DATA_CONST) and
#      `otool -v -s __TEXT __cstring` (+ __objc_methname, __objc_classname).
#   3. scripts/check-macos-availability.py check: FAIL on a strong reference to a symbol or
#      class newer than --max; weak references, selectors and class/selector names found
#      as strings (objc2 looks classes up by name) are listed as REVIEW: each must sit
#      behind a runtime availability check (`available!(macos = 14.x)` and the like).
#
# Exit: 0 nothing strong and newer; 1 at least one FAIL; 2 tool, SDK or parser problem.
# Tools can be replaced for tests: NM, OTOOL, DYLD_INFO, PYTHON (scripts/tests/), and
# JK_AVAIL_MIN_NEWER lowers the "found enough new declarations" sanity limit (200).
#
# NOT YET RUN ON A REAL SDK OR A REAL APP: written and unit-tested on Windows with
# header snippets and fake tool output (scripts/tests/), and the header parser was run on
# the Apple usr/include headers that Zig bundles. The first macOS CI run is its real test.
#
# Portable to the bash 3.2 that ships with macOS. Never needs sudo.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
MAX="${JK_MACOS_MIN:-13.0}"
SDK=""
OUT=""
PATHS=()
NPATHS=0

usage() { sed -n '4,40p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
    case "$1" in
        --max) MAX="${2:?--max needs a version}"; shift 2 ;;
        --sdk) SDK="${2:?--sdk needs a folder}"; shift 2 ;;
        --out) OUT="${2:?--out needs a folder}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) PATHS+=("$1"); NPATHS=$((NPATHS + 1)); shift ;;
    esac
done
if [ "$NPATHS" -eq 0 ]; then usage >&2; exit 2; fi

PY="${PYTHON:-python3}"
command -v "$PY" >/dev/null 2>&1 || { echo "error: $PY not found (the Command Line Tools provide python3)" >&2; exit 2; }
NM="${NM:-nm}"
OTOOL="${OTOOL:-otool}"
if [ -z "${DYLD_INFO:-}" ]; then
    if command -v dyld_info >/dev/null 2>&1; then DYLD_INFO=dyld_info
    elif command -v xcrun >/dev/null 2>&1 && xcrun --find dyld_info >/dev/null 2>&1; then DYLD_INFO="$(xcrun --find dyld_info)"
    else DYLD_INFO=""
    fi
fi
for t in "$NM" "$OTOOL"; do
    command -v "$t" >/dev/null 2>&1 || { echo "error: $t not found; install the Xcode Command Line Tools (xcode-select --install)" >&2; exit 2; }
done

sdk_version=""
if [ -z "$SDK" ]; then
    command -v xcrun >/dev/null 2>&1 || { echo "error: no --sdk given and no xcrun to find the macOS SDK" >&2; exit 2; }
    SDK="$(xcrun --sdk macosx --show-sdk-path)"
    sdk_version="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null || true)"
fi
[ -d "$SDK" ] || { echo "error: SDK folder $SDK does not exist" >&2; exit 2; }
if [ -z "$sdk_version" ] && [ -f "$SDK/SDKSettings.json" ]; then
    sdk_version="$(sed -n 's/.*"Version"[[:space:]]*:[[:space:]]*"\([0-9.]*\)".*/\1/p' "$SDK/SDKSettings.json" | head -n 1)"
fi

for p in "${PATHS[@]}"; do
    [ -e "$p" ] || { echo "error: $p does not exist" >&2; exit 2; }
done

if [ -n "$OUT" ]; then
    mkdir -p "$OUT"
    WORK="$(cd "$OUT" && pwd)"
else
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi
DB="$WORK/sdk-availability.json"

echo "== 1. macOS SDK headers: $SDK (version ${sdk_version:-unknown})"
"$PY" -I "$HERE/check-macos-availability.py" scan-sdk --sdk "$SDK" --out "$DB" --max "$MAX" \
    --min-newer "${JK_AVAIL_MIN_NEWER:-200}" ${sdk_version:+--sdk-version "$sdk_version"}

# is_macho FILE: Mach-O or fat magic (same test as check-macos-minos.sh).
is_macho() {
    local magic
    magic="$(head -c 4 "$1" 2>/dev/null | od -An -tx1 | tr -d ' \n')" || return 1
    case "$magic" in
        cffaedfe|cefaedfe|feedfacf|feedface) return 0 ;;
        cafebabe|bebafeca|cafebabf|bfbafeca)
            if command -v lipo >/dev/null 2>&1; then lipo -info "$1" >/dev/null 2>&1; else return 0; fi ;;
        *) return 1 ;;
    esac
}

# run_arm64 OUTFILE TOOL ARGS... FILE: the arm64 slice of a fat file, else the file itself.
run_arm64() {
    local out="$1"; shift
    if ! "$@" > "$out" 2>/dev/null || [ ! -s "$out" ]; then
        # retry without "-arch arm64" (args 2 and 3) for tools/builds that reject it
        local tool="$1"; shift; shift; shift
        "$tool" "$@" > "$out" 2>/dev/null || true
    fi
}

echo "== 2. imports, selector references and strings of every Mach-O"
: > "$WORK/files.tsv"
n=0
for p in "${PATHS[@]}"; do
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        is_macho "$f" || continue
        n=$((n + 1))
        id="f$n"
        printf '%s\t%s\n' "$id" "$f" >> "$WORK/files.tsv"
        run_arm64 "$WORK/$id.nm" "$NM" -arch arm64 -m -u "$f"
        if [ -n "$DYLD_INFO" ]; then "$DYLD_INFO" -imports "$f" > "$WORK/$id.dyld" 2>/dev/null || true; fi
        : > "$WORK/$id.selrefs"
        for seg in __DATA __DATA_CONST; do
            run_arm64 "$WORK/$id.sel.tmp" "$OTOOL" -arch arm64 -v -s "$seg" __objc_selrefs "$f"
            cat "$WORK/$id.sel.tmp" >> "$WORK/$id.selrefs"
        done
        rm -f "$WORK/$id.sel.tmp"
        run_arm64 "$WORK/$id.cstrings" "$OTOOL" -arch arm64 -v -s __TEXT __cstring "$f"
        # Selector and class names: with chained fixups otool may not resolve the
        # __objc_selrefs pointers to names, but the names themselves are listed here.
        for sec in __objc_methname __objc_classname; do
            run_arm64 "$WORK/$id.str.tmp" "$OTOOL" -arch arm64 -v -s __TEXT "$sec" "$f"
            cat "$WORK/$id.str.tmp" >> "$WORK/$id.cstrings"
        done
        rm -f "$WORK/$id.str.tmp"
        echo "    $f"
    done <<EOF
$(find "$p" -type f 2>/dev/null | LC_ALL=C sort)
EOF
done
if [ "$n" -eq 0 ]; then
    echo "error: no Mach-O files found under: ${PATHS[*]}" >&2
    exit 2
fi

echo "== 3. references to APIs newer than macOS $MAX"
set +e
"$PY" -I "$HERE/check-macos-availability.py" check --db "$DB" --work "$WORK" --max "$MAX" \
    --json "$WORK/availability-report.json" | tee "$WORK/availability-report.txt"
rc=${PIPESTATUS[0]}
set -e

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### macOS API availability (nothing newer than macOS $MAX without a weak reference)"
        echo
        echo "- SDK: $(basename "$SDK") ${sdk_version:+($sdk_version)}; $n Mach-O file(s)"
        echo "- $(tail -n 1 "$WORK/availability-report.txt")"
        grep -E '^    (FAIL|REVIEW) ' "$WORK/availability-report.txt" | head -n 60 | sed 's/^    /- /' || true
        echo
    } >> "$GITHUB_STEP_SUMMARY"
fi
case "$rc" in
    0) echo "availability check passed (REVIEW lines, if any, need a runtime check in the code)" ;;
    1) echo "::error::a Mac binary strongly links an API newer than macOS $MAX (see FAIL lines above); it would not launch on macOS 13" ;;
    *) echo "::error::the availability check could not read every binary (exit $rc)" ;;
esac
exit "$rc"

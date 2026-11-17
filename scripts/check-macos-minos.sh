#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# check-macos-minos.sh - prove that every Mach-O file in an app bundle (or any
# set of files/folders) declares a minimum macOS version no newer than 13.0.
#
# Why: the owner's MacBook runs macOS 13.6.9 and CI runs macOS 15. A binary or
# bundled library built for a newer minimum launches fine on CI but fails on
# the MacBook ("You can't use this version of the application with this
# version of macOS", or a dyld error). WORK_ORDER section 4.10 and trap 14.
#
# Usage:
#   scripts/check-macos-minos.sh [--max 13.0] [--require-arm64] PATH [PATH...]
#
#   --max V          the newest allowed minimum (default: $JK_MACOS_MIN or 13.0)
#   --require-arm64  also fail if a Mach-O has no arm64 slice (Apple Silicon)
#
# For each Mach-O it prints: file, architecture, load command, minos, sdk,
# verdict. It reads LC_BUILD_VERSION ("minos") and the older
# LC_VERSION_MIN_MACOSX ("version") using `vtool -show-build` when available,
# else `otool -l` (both ship with the Xcode Command Line Tools).
#
# Exit status: 0 all good; 1 at least one file is newer than allowed, has no
# minimum at all, or (with --require-arm64) lacks arm64; 2 usage/tool error.
#
# Portable to the bash 3.2 that ships with macOS (no associative arrays, no
# mapfile, no ${var,,}). Never needs sudo.

set -euo pipefail

MAX="${JK_MACOS_MIN:-13.0}"
REQUIRE_ARM64=0
PATHS=()
NPATHS=0   # counted separately: bash 3.2 with `set -u` cannot expand an empty array

usage() {
    sed -n '4,27p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --max) MAX="${2:?--max needs a version}"; shift 2 ;;
        --require-arm64) REQUIRE_ARM64=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; while [ $# -gt 0 ]; do PATHS+=("$1"); NPATHS=$((NPATHS + 1)); shift; done ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) PATHS+=("$1"); NPATHS=$((NPATHS + 1)); shift ;;
    esac
done
if [ "$NPATHS" -eq 0 ]; then
    usage >&2
    exit 2
fi

if command -v vtool >/dev/null 2>&1; then
    TOOL=vtool
elif command -v otool >/dev/null 2>&1; then
    TOOL=otool
else
    echo "error: neither vtool nor otool found; install the Xcode Command Line Tools (xcode-select --install)" >&2
    exit 2
fi

# version_gt A B: true if version A is newer than version B (1-3 numeric parts).
version_gt() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        na = split(a, x, "."); nb = split(b, y, ".");
        for (i = 1; i <= 3; i++) {
            xi = (i <= na) ? x[i] + 0 : 0; yi = (i <= nb) ? y[i] + 0 : 0;
            if (xi > yi) exit 0; if (xi < yi) exit 1;
        }
        exit 1
    }'
}

# is_macho FILE: true if FILE starts with a Mach-O or fat (universal) magic.
is_macho() {
    local magic
    magic="$(head -c 4 "$1" 2>/dev/null | od -An -tx1 | tr -d ' \n')" || return 1
    case "$magic" in
        cffaedfe|cefaedfe|feedfacf|feedface) return 0 ;;   # thin 64/32-bit, either endianness
        cafebabe|bebafeca|cafebabf|bfbafeca)               # fat; Java class files share cafebabe,
            if command -v lipo >/dev/null 2>&1; then         # so confirm with lipo when we can
                lipo -info "$1" >/dev/null 2>&1
            else
                return 0
            fi ;;
        *) return 1 ;;
    esac
}

# build_info FILE: print "arch|cmd|minos|sdk" lines, one per architecture slice.
build_info() {
    if [ "$TOOL" = vtool ]; then
        vtool -show-build "$1" 2>/dev/null
    else
        # otool prints one file header per slice with -arch all.
        otool -arch all -l "$1" 2>/dev/null
    fi | awk '
        function flush() {
            if (arch != "") {
                if (cmd == "") cmd = "none";
                print arch "|" cmd "|" (minos == "" ? "?" : minos) "|" (sdk == "" ? "?" : sdk);
            }
        }
        # vtool: "path (architecture arm64):"   otool: "path (architecture arm64):" too;
        # a thin file has no "(architecture ...)" header, only "path:".
        /^[^ \t].*:$/ {
            flush();
            arch = "single"; cmd = ""; minos = ""; sdk = "";
            if (match($0, /\(architecture [^)]+\)/)) {
                arch = substr($0, RSTART + 14, RLENGTH - 15);
            }
            next
        }
        $1 == "cmd" && ($2 == "LC_BUILD_VERSION" || $2 == "LC_VERSION_MIN_MACOSX") {
            # Keep the first one found per slice; inside it read minos/version + sdk.
            if (cmd == "") { cmd = $2; incmd = 1 } else { incmd = 0 }
            next
        }
        $1 == "cmd" { incmd = 0; next }
        # LC_BUILD_VERSION: "minos X" / "sdk Y" (its "tool"/"version" lines describe
        # the linker and are ignored). LC_VERSION_MIN_MACOSX: "version X" / "sdk Y".
        incmd && $1 == "minos" { minos = $2; next }
        incmd && $1 == "version" && cmd == "LC_VERSION_MIN_MACOSX" { minos = $2; next }
        incmd && $1 == "sdk" { sdk = $2; next }
        incmd && $1 == "platform" && $2 != "MACOS" && $2 != "1" { cmd = cmd "(platform " $2 ")" }
        END { flush() }
    '
}

for p in "${PATHS[@]}"; do
    if [ ! -e "$p" ]; then
        echo "error: $p does not exist" >&2
        exit 2
    fi
done

checked=0
failed=0
printf '%-60s %-8s %-24s %-8s %-8s %s\n' "FILE" "ARCH" "LOAD COMMAND" "MINOS" "SDK" "VERDICT"

check_file() {
    local f="$1" arch cmd minos sdk verdict has_arm64=0 slices=0
    while IFS='|' read -r arch cmd minos sdk; do
        [ -n "$arch" ] || continue
        slices=$((slices + 1))
        case "$arch" in arm64|arm64e) has_arm64=1 ;; esac
        if [ "$arch" = single ] && command -v lipo >/dev/null 2>&1; then
            # Name the architecture of a thin file.
            arch="$(lipo -archs "$f" 2>/dev/null || echo single)"
            case "$arch" in arm64|arm64e) has_arm64=1 ;; esac
        fi
        if [ "$cmd" = none ] || [ "$minos" = "?" ]; then
            verdict="FAIL (no minimum macOS declared)"
            failed=$((failed + 1))
        elif version_gt "$minos" "$MAX"; then
            verdict="FAIL (newer than $MAX)"
            failed=$((failed + 1))
        else
            verdict="ok"
        fi
        printf '%-60s %-8s %-24s %-8s %-8s %s\n' "$f" "$arch" "$cmd" "$minos" "$sdk" "$verdict"
    done <<EOF
$(build_info "$f")
EOF
    if [ "$slices" -eq 0 ]; then
        printf '%-60s %-8s %-24s %-8s %-8s %s\n' "$f" "?" "?" "?" "?" "FAIL ($TOOL could not read it)"
        failed=$((failed + 1))
    elif [ "$REQUIRE_ARM64" -eq 1 ] && [ "$has_arm64" -eq 0 ]; then
        printf '%-60s %-8s %-24s %-8s %-8s %s\n' "$f" "-" "-" "-" "-" "FAIL (no arm64 slice)"
        failed=$((failed + 1))
    fi
    checked=$((checked + 1))
}

for p in "${PATHS[@]}"; do
    # -type f skips symlinks (a Tauri bundle has none, and exFAT cannot hold them).
    while IFS= read -r f; do
        if is_macho "$f"; then
            check_file "$f"
        fi
    done <<EOF
$(find "$p" -type f 2>/dev/null | LC_ALL=C sort)
EOF
done

echo
echo "checked $checked Mach-O file(s) with $TOOL; allowed minimum: macOS $MAX or older; failures: $failed"
if [ "$checked" -eq 0 ]; then
    echo "error: no Mach-O files found under: ${PATHS[*]}" >&2
    exit 1
fi
[ "$failed" -eq 0 ]

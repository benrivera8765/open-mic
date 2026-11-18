#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# check-macos-paths.sh - fail if a build-machine path (the builder's home folder, the
# checkout, the cargo folder) is embedded in any file of the Mac build. The Mac twin of
# the check in app/scripts/build-windows.sh: without --remap-path-prefix a Rust release
# binary holds hundreds of /Users/<name>/.cargo/registry/... panic locations, which
# publishes the builder's account name with every copy of the app.
#
#   scripts/check-macos-paths.sh --forbid PATH [--forbid PATH ...] FILE_OR_DIR...
#
# Every regular file under the given folders is searched (binaries included) for each
# forbidden path, case-insensitively (APFS is case-insensitive by default), as UTF-8 and
# as UTF-16LE. A forbidden path shorter than 6 characters (such as "/" for a home folder
# that could not be read) is refused rather than matching everything.
#
# Exit: 0 none found; 1 found (each file and count is printed); 2 usage error.
# Needs python3 (the Command Line Tools provide it). Tested by scripts/tests/.

set -euo pipefail
FORBID=()
NF=0
TARGETS=()
NT=0
while [ $# -gt 0 ]; do
    case "$1" in
        --forbid) FORBID+=("${2:?--forbid needs a path}"); NF=$((NF + 1)); shift 2 ;;
        -h|--help) sed -n '4,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "unknown option: $1" >&2; exit 2 ;;
        *) TARGETS+=("$1"); NT=$((NT + 1)); shift ;;
    esac
done
if [ "$NF" -eq 0 ] || [ "$NT" -eq 0 ]; then
    echo "usage: check-macos-paths.sh --forbid PATH [--forbid PATH ...] FILE_OR_DIR..." >&2
    exit 2
fi
PY="${PYTHON:-python3}"
# The forbidden paths travel in a file, not on the command line: Git Bash (where the tests
# also run) rewrites arguments that look like POSIX paths before a native program sees them.
list="$(mktemp)"
trap 'rm -f "$list"' EXIT
printf '%s\n' "${FORBID[@]}" > "$list"
"$PY" -I - "$list" "${TARGETS[@]}" <<'PYEOF'
import os
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    forbid = [p.rstrip("\n") for p in f if p.strip()]
forbid = [p.rstrip("/") if len(p) > 1 else p for p in forbid]
targets = sys.argv[2:]
for p in forbid:
    if len(p) < 6:
        print(f"check-macos-paths: refusing to search for '{p}' (too short to mean anything)", file=sys.stderr)
        sys.exit(2)
needles = []
for p in sorted(set(forbid)):
    low = p.lower()
    needles.append((p, low.encode("utf-8"), low.encode("utf-16-le")))

files = []
for t in targets:
    if not os.path.exists(t):
        print(f"check-macos-paths: {t} does not exist", file=sys.stderr)
        sys.exit(2)
    if os.path.isfile(t):
        files.append(t)
    for root, dirs, names in os.walk(t):
        dirs.sort()
        for n in sorted(names):
            fp = os.path.join(root, n)
            if os.path.isfile(fp) and not os.path.islink(fp):
                files.append(fp)

hits = 0
for fp in files:
    with open(fp, "rb") as f:
        data = f.read()
    low = data.lower()     # bytes.lower(): ASCII letters only, which is what a path needs
    for shown, u8, u16 in needles:
        n = low.count(u8) + low.count(u16)   # bytes.count ignores alignment: odd offsets too
        if n:
            hits += n
            print(f"FOUND {n:6d} x {shown}  in {fp}")
print(f"checked {len(files)} file(s) for {len(needles)} build-machine path(s): {hits} occurrence(s)")
sys.exit(1 if hits else 0)
PYEOF

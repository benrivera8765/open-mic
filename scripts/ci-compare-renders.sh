#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-compare-renders.sh - compare the offline renders made on Windows and on
# macOS (scripts/ci-render.sh) file by file: same names, same length, same
# channel count, and every sample within a float tolerance.
#
#   bash scripts/ci-compare-renders.sh WINDOWS_DIR MACOS_DIR
#
# Tolerance: JK_RENDER_TOL (absolute, default 1e-5, i.e. -100 dBFS). The
# report always prints the measured maximum difference per file, in linear
# units and dBFS, so "matches" is a number, not an opinion.
# Needs only python3 (no numpy): the WAV reader is inline below.

set -euo pipefail
win="${1:?usage: ci-compare-renders.sh WINDOWS_DIR MACOS_DIR}"
mac="${2:?usage: ci-compare-renders.sh WINDOWS_DIR MACOS_DIR}"

for d in "$win" "$mac"; do
    if [ ! -d "$d" ] || [ -f "$d/SKIPPED.txt" ] || ! ls "$d"/*.wav >/dev/null 2>&1; then
        echo "::notice::no renders in $d ($(cat "$d/SKIPPED.txt" 2>/dev/null || echo 'nothing there')); skipping the cross-platform comparison"
        exit 0
    fi
done

python3 - "$win" "$mac" "${JK_RENDER_TOL:-1e-5}" <<'PY'
import math, os, struct, sys
from array import array

def read_float_wav(path):
    """Minimal RIFF/RF64 reader for 32-bit float WAV (tag 3 or extensible float)."""
    with open(path, "rb") as f:
        data = f.read()
    if data[0:4] not in (b"RIFF", b"RF64") or data[8:12] != b"WAVE":
        raise ValueError(f"{path}: not a WAV file")
    pos, ds64_data, fmt = 12, None, None
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack_from("<I", data, pos + 4)[0]
        body = pos + 8
        if cid == b"ds64":
            ds64_data = struct.unpack_from("<Q", data, body + 8)[0]
        elif cid == b"fmt ":
            tag, ch, rate = struct.unpack_from("<HHI", data, body)
            bits = struct.unpack_from("<H", data, body + 14)[0]
            if tag == 0xFFFE:
                tag = struct.unpack_from("<H", data, body + 24)[0]
            fmt = (tag, ch, rate, bits)
        elif cid == b"data":
            if fmt is None or fmt[0] != 3 or fmt[3] != 32:
                raise ValueError(f"{path}: not 32-bit float ({fmt})")
            n = ds64_data if (size == 0xFFFFFFFF and ds64_data is not None) else size
            samples = array("f")
            samples.frombytes(data[body:body + n - n % 4])
            if sys.byteorder != "little":
                samples.byteswap()
            return fmt[1], fmt[2], samples
        pos = body + size + (size & 1)
    raise ValueError(f"{path}: no data chunk")

win, mac, tol = sys.argv[1], sys.argv[2], float(sys.argv[3])
names = sorted(set(n for n in os.listdir(win) if n.endswith(".wav")) | set(n for n in os.listdir(mac) if n.endswith(".wav")))
failures = 0
print(f"{'file':40} {'frames':>10} {'max |diff|':>12} {'dBFS':>8}  verdict (tolerance {tol:g})")
for name in names:
    a, b = os.path.join(win, name), os.path.join(mac, name)
    if not (os.path.exists(a) and os.path.exists(b)):
        print(f"{name:40} {'':>10} {'':>12} {'':>8}  FAIL (only on {'Windows' if os.path.exists(a) else 'macOS'})")
        failures += 1
        continue
    ca, ra, sa = read_float_wav(a)
    cb, rb, sb = read_float_wav(b)
    if (ca, ra, len(sa)) != (cb, rb, len(sb)):
        print(f"{name:40} {'':>10} {'':>12} {'':>8}  FAIL (format/length differ: {ca}ch {ra}Hz {len(sa)} vs {cb}ch {rb}Hz {len(sb)})")
        failures += 1
        continue
    worst = max((abs(x - y) for x, y in zip(sa, sb)), default=0.0)
    bad = not (worst <= tol) or any(math.isnan(x) for x in sa) or any(math.isnan(y) for y in sb)
    db = 20 * math.log10(worst) if worst > 0 else float("-inf")
    print(f"{name:40} {len(sa) // max(ca, 1):>10} {worst:>12.3e} {db:>8.1f}  {'FAIL' if bad else 'ok'}")
    failures += bad
print(f"{len(names)} file(s), {failures} failure(s)")
sys.exit(1 if failures else 0)
PY

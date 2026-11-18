# SPDX-License-Identifier: GPL-3.0-or-later
"""Size of a dist/ (stick) folder per top-level entry, against the WORK_ORDER 4.8 budget.

    python -I -B scripts/dist/dist_size.py <dist> [--json out.json]

For every top-level folder (and the root files together): bytes, files, folders, and the
space it takes on an exFAT stick with 32 KB clusters (what Windows uses for volumes up to
32 GB) and with 128 KB clusters (larger volumes). On exFAT every file takes whole clusters
(an empty file none) and every folder at least one cluster; this is the same estimate as
scripts/build-studio-runtime.sh. Budget (WORK_ORDER 4.8): both apps plus models under
12 GB, ideally under 9 GB; reported in GB (10^9 bytes) and GiB.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

CLUSTERS = (32768, 131072)


def measure(root: str) -> dict:
    files = dirs = size = 0
    alloc = {c: 0 for c in CLUSTERS}
    for dp, dns, fns in os.walk(root):
        dirs += 1
        for c in CLUSTERS:
            alloc[c] += c
        for f in fns:
            s = os.lstat(os.path.join(dp, f)).st_size
            files += 1
            size += s
            for c in CLUSTERS:
                alloc[c] += -(-s // c) * c
    return {"bytes": size, "files": files, "dirs": dirs, "exfat_32k": alloc[32768], "exfat_128k": alloc[131072]}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dist")
    ap.add_argument("--json")
    a = ap.parse_args()
    root = os.path.abspath(a.dist)
    rows: dict[str, dict] = {}
    root_files = {"bytes": 0, "files": 0, "dirs": 0, "exfat_32k": 0, "exfat_128k": 0}
    for name in sorted(os.listdir(root), key=str.lower):
        p = os.path.join(root, name)
        if os.path.isdir(p):
            rows[name + "/"] = measure(p)
            if name == "Windows":   # the two parts of the Windows app, separately
                for sub in sorted(os.listdir(p), key=str.lower):
                    sp = os.path.join(p, sub)
                    rows[f"  Windows/{sub}" + ("/" if os.path.isdir(sp) else "")] = (
                        measure(sp) if os.path.isdir(sp) else
                        {"bytes": os.path.getsize(sp), "files": 1, "dirs": 0,
                         **{f"exfat_{c // 1024}k": -(-os.path.getsize(sp) // c) * c for c in CLUSTERS}})
        else:
            s = os.path.getsize(p)
            root_files["bytes"] += s
            root_files["files"] += 1
            for c in CLUSTERS:
                root_files[f"exfat_{c // 1024}k"] += -(-s // c) * c
    rows["(root files)"] = root_files
    top = [v for k, v in rows.items() if not k.startswith("  ")]
    total = {k: sum(r[k] for r in top) for k in ("bytes", "files", "dirs", "exfat_32k", "exfat_128k")}
    total["dirs"] += 1                    # the root folder itself (on a stick: the volume root)
    rows["TOTAL"] = total

    gb = lambda b: b / 1e9          # noqa: E731
    gib = lambda b: b / 2**30       # noqa: E731
    print(f"{'folder':24s} {'bytes':>16s} {'files':>7s} {'dirs':>6s} {'exFAT 32 KB':>14s} {'exFAT 128 KB':>14s}")
    for k, r in rows.items():
        print(f"{k:24s} {r['bytes']:16,d} {r['files']:7,d} {r['dirs']:6,d} {gb(r['exfat_32k']):11.3f} GB {gb(r['exfat_128k']):11.3f} GB")
    apps_models = total["exfat_32k"] - rows.get("Library/", {"exfat_32k": 0})["exfat_32k"] \
        - rows.get("Recordings/", {"exfat_32k": 0})["exfat_32k"]
    verdict = {
        "total_bytes_GB": round(gb(total["bytes"]), 3),
        "total_bytes_GiB": round(gib(total["bytes"]), 3),
        "exfat_32k_GB": round(gb(total["exfat_32k"]), 3),
        "exfat_32k_GiB": round(gib(total["exfat_32k"]), 3),
        "apps_plus_models_exfat_32k_GB": round(gb(apps_models), 3),
        "under_9_GB": apps_models < 9e9,
        "under_12_GB": apps_models < 12e9,
        "note": "Mac/ holds no app yet (built on the Mac or by CI); add its size when it is copied in.",
    }
    print(json.dumps(verdict, indent=1))
    if a.json:
        with open(a.json, "w", encoding="utf-8") as fh:
            json.dump({"dist": root, "rows": rows, "verdict": verdict}, fh, indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main())

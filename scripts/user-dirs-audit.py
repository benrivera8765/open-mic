# SPDX-License-Identifier: GPL-3.0-or-later
"""Portable-mode audit (WORKING ORDER 4.8): did anything write to the per-user folders?

    python -I scripts/user-dirs-audit.py snapshot <out.json> [--stick <root>]
    python -I scripts/user-dirs-audit.py diff <before.json> <after.json> [--out report.json]

``snapshot`` records size and modification time of every file and folder under
%USERPROFILE%\\.cache, %APPDATA%, %LOCALAPPDATA% (which holds %TEMP%) and %TEMP% (if it
lies elsewhere), plus every HKCU registry key, value or data that mentions "jukeoke" or
the stick's folder name (``reg query HKCU /s /f``).

``diff`` lists what was created, changed or deleted between two snapshots and sorts each
entry into one of three groups by its path, because Windows and other programs write to
these folders all the time:

- ``app_or_studio``: names that the app, its WebView2 profile or the Studio's libraries
  would use (Jukeoke, EBWebView, Python, pip, uv, torch, Hugging Face, NVIDIA's CUDA
  ComputeCache, numba, matplotlib, CTranslate2, FFmpeg/PyAV, librosa, joblib, a Python
  ``tmp*`` folder). Any entry here is a finding.
- ``os_known``: what Windows writes for every program (D-41 / D-88: the WebView2
  runtime's EdgeUpdate ``lastrun``, Explorer's MuiCache, the audio service's
  PolicyConfig store) and other well-known OS paths (Windows Defender, Explorer caches,
  notifications, Search).
- ``test_tooling``: the measuring tools themselves (Claude Code's own temp folder,
  Playwright's artifacts folder, Node's compile cache).
- ``other``: everything else, listed so that a reader can check the attribution.

Registry differences are listed in full.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

APP_RE = re.compile(
    r"jukeoke|ebwebview|webview2|\\python|\\pip\\|\\pip$|\\uv\\|\\uv$|torch|huggingface|hf_xet|"
    r"\\nvidia\\computecache|\\nvidia corporation\\|numba|matplotlib|ctranslate2|ffmpeg|pyav|librosa|joblib|"
    r"\\temp\\tmp[a-z0-9_]{6,}|\\temp\\_mei|\\temp\\jk|\\.cache\\",
    re.I)
OS_RE = re.compile(
    r"\\microsoft\\windows\\explorer\\|\\microsoft\\windows\\notifications|\\microsoft\\windows defender|"
    r"\\packages\\microsoft\.windows|\\microsoft\\windows\\caches|\\connecteddevicesplatform|"
    r"\\microsoft\\edgeupdate|\\microsoft\\windows\\webcache|\\microsoft\\windows\\inetcache|"
    r"\\microsoft\\windows\\recent|\\microsoft\\windows\\history|\\packages\\microsoftwindows\.client|"
    r"\\d3dscache\\|\\microsoft\\windows\\shell|\\microsoft\\windows\\usrclass|\\microsoft\\credentials|"
    r"\\microsoft\\protect|\\microsoft\\vault|\\temp\\msedge_url_fetcher|\\microsoft\\windows\\powershell|"
    r"\\microsoft\\windows\\actioncenter|\\temp\\diagoutputdir|\\microsoft\\windows\\winx",
    re.I)
TOOL_RE = re.compile(r"\\temp\\claude\\|\\temp\\playwright|\\temp\\node-compile-cache|\\temp\\msys|\\anthropic|\\claude", re.I)
OS_REG_RE = re.compile(r"\\Shell\\MuiCache|\\Audio\\PolicyConfig\\PropertyStore|\\EdgeUpdate\\", re.I)


def roots() -> list[Path]:
    env = os.environ
    out = []
    for p in (Path(env["USERPROFILE"]) / ".cache", env.get("APPDATA"), env.get("LOCALAPPDATA"), env.get("TEMP")):
        if p:
            out.append(Path(p))
    uniq: list[Path] = []
    for p in out:
        if not any(str(p).lower().startswith(str(q).lower() + "\\") or str(p).lower() == str(q).lower() for q in uniq):
            uniq.append(p)
    return uniq


def walk(root: Path, files: dict) -> int:
    n = 0
    stack = [str(root)]
    while stack:
        d = stack.pop()
        try:
            it = os.scandir(d)
        except OSError:
            continue
        with it:
            for e in it:
                try:
                    st = e.stat(follow_symlinks=False)
                except OSError:
                    continue
                is_dir = e.is_dir(follow_symlinks=False)
                files[e.path] = [0 if is_dir else st.st_size, round(st.st_mtime, 3), 1 if is_dir else 0]
                n += 1
                if is_dir and not e.is_junction():  # junctions point elsewhere (counted once)
                    stack.append(e.path)
    return n


def registry(patterns: list[str]) -> list[str]:
    out: set[str] = set()
    for pat in patterns:
        r = subprocess.run(["reg", "query", "HKCU", "/s", "/f", pat], capture_output=True, text=True,
                           encoding="utf-8", errors="replace")
        key = None
        for line in r.stdout.splitlines():
            if line.startswith("HKEY_"):
                key = line.strip()
                out.add(key)
            elif line.startswith(" ") and key and line.strip():
                out.add(f"{key} | {line.strip()}")
    return sorted(out)


def snapshot(out: Path, stick: str | None) -> None:
    t0 = time.time()
    files: dict = {}
    counts = {str(r): walk(r, files) for r in roots()}
    pats = ["jukeoke"] + ([Path(stick).name] if stick else [])
    snap = {"taken": time.strftime("%Y-%m-%dT%H:%M:%S"), "roots": counts, "files": files,
            "registry": registry(pats), "registry_patterns": pats, "seconds": round(time.time() - t0, 1)}
    out.write_text(json.dumps(snap), encoding="utf-8")
    print(json.dumps({k: v for k, v in snap.items() if k not in ("files", "registry")} | {"registry_entries": len(snap["registry"])}))


def classify(path: str) -> str:
    if TOOL_RE.search(path):
        return "test_tooling"
    if APP_RE.search(path):
        return "app_or_studio"
    if OS_RE.search(path):
        return "os_known"
    return "other"


def diff(a: Path, b: Path, out: Path | None) -> None:
    A = json.loads(a.read_text(encoding="utf-8"))
    B = json.loads(b.read_text(encoding="utf-8"))
    fa, fb = A["files"], B["files"]
    rows = []
    for p, v in fb.items():
        if p not in fa:
            rows.append(("new", p, v))
        elif not v[2] and (fa[p][0] != v[0] or fa[p][1] != v[1]):
            rows.append(("changed", p, v))
    for p, v in fa.items():
        if p not in fb:
            rows.append(("deleted", p, v))
    groups: dict[str, list] = {"app_or_studio": [], "os_known": [], "test_tooling": [], "other": []}
    for kind, p, v in sorted(rows, key=lambda r: r[1].lower()):
        groups[classify(p)].append({"kind": kind, "path": p, "bytes": v[0], "dir": bool(v[2])})
    ra, rb = set(A["registry"]), set(B["registry"])
    reg_new = sorted(rb - ra)
    reg_gone = sorted(ra - rb)
    rep = {"before": A["taken"], "after": B["taken"],
           "counts": {g: len(v) for g, v in groups.items()},
           "registry_new": reg_new, "registry_removed": reg_gone,
           "registry_new_not_os_known": [r for r in reg_new if not OS_REG_RE.search(r)],
           "groups": groups}
    text = json.dumps(rep, indent=1)
    if out:
        out.write_text(text, encoding="utf-8")
    print(json.dumps({k: rep[k] for k in ("before", "after", "counts", "registry_new", "registry_removed",
                                         "registry_new_not_os_known")}, indent=1))
    for g in ("app_or_studio", "other"):
        for r in groups[g][:60]:
            print(f"{g:14s} {r['kind']:8s} {r['bytes']:>10} {r['path']}")


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("snapshot")
    s.add_argument("out", type=Path)
    s.add_argument("--stick")
    d = sub.add_parser("diff")
    d.add_argument("a", type=Path)
    d.add_argument("b", type=Path)
    d.add_argument("--out", type=Path)
    a = ap.parse_args()
    if a.cmd == "snapshot":
        snapshot(a.out, a.stick)
    else:
        diff(a.a, a.b, a.out)


if __name__ == "__main__":
    sys.exit(main())

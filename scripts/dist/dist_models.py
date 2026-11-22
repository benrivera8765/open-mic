# SPDX-License-Identifier: GPL-3.0-or-later
"""Copies the Studio's DEFAULT model weights into a dist/stick Models folder and has the
Studio's own code verify them (scripts/make-dist.sh, WORK_ORDER 4.8).

    python -I -B scripts/dist/dist_models.py --src <models cache> --dest <dist>/Models
                                             [--preset full] [--name "Open Mic"]

What "default" means comes from the Studio's registry in this checkout, not from a list
kept here: every weight file of the separation preset (``presets.PRESETS[preset]``),
plus every file of the default alignment and transcription models
(``hfmodels.DEFAULT_ALIGNMENT`` / ``DEFAULT_TRANSCRIPTION``). Layout, exactly what the
Studio looks for:

    <dest>/<separation weight file>
    <dest>/hf/models--<org>--<repo>/snapshots/<pinned commit>/<file>

Then ``manifest.json`` is rebuilt from scratch by the Studio's verification functions
(``download.ensure_model(verify=True, allow_download=False)`` and
``hfmodels.verify(force=True)``): every file is hashed against the registry's pinned
SHA-256 and size. Nothing is downloaded (``allow_download=False``); a missing or wrong
file stops the script. ``SHA256SUMS.txt`` (``sha256sum -c`` format) and ``README.txt``
are written for people. The manifest's per-file records hold the build drive's volume
serial, so on a stick the Studio hashes each file once more before first use (D-172);
that is intended.

Standard library plus the jukeoke_studio package of this checkout (lightweight modules
only: registry, download, hfmodels, presets; no PyTorch).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path

REPO = Path(os.path.abspath(__file__)).parents[2]
sys.path.insert(0, str(REPO / "studio"))

from jukeoke_studio.modelzoo import MODELS, hfmodels  # noqa: E402
from jukeoke_studio.modelzoo.download import MANIFEST, ensure_model, sha256_of  # noqa: E402
from jukeoke_studio.presets import PRESETS  # noqa: E402


def wanted(preset: str):
    """[(relative path, source-relative path, sha256, size, role, model id, licence, url)]"""
    out = []
    for role, mid in PRESETS[preset].items():
        spec = MODELS[mid]
        for wf in spec.files:
            out.append(dict(rel=wf.filename, sha256=wf.sha256, size=wf.size, role=f"separation: {role}",
                            model=mid, licence=spec.licence, url=wf.url, kind="sep"))
    for key, role in ((hfmodels.DEFAULT_ALIGNMENT, "lyrics alignment"),
                      (hfmodels.DEFAULT_TRANSCRIPTION, "lyrics transcription")):
        m = hfmodels.get(key)
        for f in m.files:
            out.append(dict(rel=hfmodels.manifest_key(m, f), sha256=f.sha256, size=f.size, role=role,
                            model=key, licence=m.licence, url=m.url(f), kind="hf"))
    return out


def copy_one(src: Path, dst: Path) -> str:
    dst.parent.mkdir(parents=True, exist_ok=True)
    try:
        s, d = src.stat(), dst.stat()
        if s.st_size == d.st_size and s.st_mtime_ns == d.st_mtime_ns:
            return "kept"
    except FileNotFoundError:
        pass
    tmp = dst.with_name(dst.name + ".copying")
    shutil.copy2(src, tmp)          # follows links: the cache's files become plain files
    os.replace(tmp, dst)
    return "copied"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True, help="models cache to copy from (e.g. $JK_BUILD/models-cache)")
    ap.add_argument("--dest", required=True, help="<dist>/Models")
    ap.add_argument("--preset", default="full")
    ap.add_argument("--name", default="Open Mic", help="app name for README.txt")
    ap.add_argument("--list", action="store_true", help="only print the relative paths this script owns")
    a = ap.parse_args()
    src, dest = Path(os.path.abspath(a.src)), Path(os.path.abspath(a.dest))
    files = wanted(a.preset)
    expected = {f["rel"] for f in files}
    if a.list:
        print("\n".join(sorted(expected | {MANIFEST, "SHA256SUMS.txt", "README.txt"})))
        return 0
    dest.mkdir(parents=True, exist_ok=True)

    # Anything else in the Models folder is not ours to keep: make-dist.sh has already
    # refused to run (or was told --force-clean) when it found foreign files.
    keep = expected | {MANIFEST, "SHA256SUMS.txt", "README.txt"}
    for dp, dns, fns in os.walk(dest, topdown=False):
        for fn in fns:
            rel = os.path.relpath(os.path.join(dp, fn), dest).replace("\\", "/")
            if rel not in keep:
                os.remove(os.path.join(dp, fn))
                print(f"removed {rel}")
        for dn in dns:
            p = os.path.join(dp, dn)
            if not os.listdir(p):
                os.rmdir(p)

    t0 = time.perf_counter()
    for f in files:
        s = src / f["rel"]
        if not s.is_file():
            print(f"MISSING in the cache: {s}", file=sys.stderr)
            return 1
        if s.stat().st_size != f["size"]:
            print(f"WRONG SIZE in the cache: {s} ({s.stat().st_size} != {f['size']})", file=sys.stderr)
            return 1
        print(f"{copy_one(s, dest / f['rel']):7s} {f['rel']}")
    t_copy = time.perf_counter() - t0

    # The Studio's own verification writes a fresh manifest.json.
    (dest / MANIFEST).unlink(missing_ok=True)
    t0 = time.perf_counter()
    for mid in PRESETS[a.preset].values():
        ensure_model(MODELS[mid], dest, verify=True, allow_download=False)
    for key in (hfmodels.DEFAULT_ALIGNMENT, hfmodels.DEFAULT_TRANSCRIPTION):
        m = hfmodels.get(key)
        gone = hfmodels.missing(m, dest)
        if gone:
            print(f"MISSING after copy: {key}: {[g.name for g in gone]}", file=sys.stderr)
            return 1
        hfmodels.verify(m, dest, force=True)
    t_verify = time.perf_counter() - t0
    manifest = json.loads((dest / MANIFEST).read_text(encoding="utf-8"))
    bad = sorted(k for k, v in manifest.items() if v.get("bad"))
    unrecorded = sorted(expected - set(manifest))
    if bad or unrecorded:
        print(f"verification failed: bad {bad}, not recorded {unrecorded}", file=sys.stderr)
        return 1

    # Independent second hash for the human-readable checksum list (not the Studio's code).
    sums = []
    for f in sorted(files, key=lambda f: f["rel"]):
        digest = sha256_of(dest / f["rel"])
        if digest != f["sha256"]:
            print(f"SHA-256 mismatch on re-read: {f['rel']}", file=sys.stderr)
            return 1
        sums.append(f"{digest} *{f['rel']}\n")
    (dest / "SHA256SUMS.txt").write_text("".join(sums), encoding="utf-8", newline="\n")

    total = sum(f["size"] for f in files)
    lines = [
        f"{a.name} Studio models",
        "",
        "Only the Studio uses this folder (making songs on a Windows PC with an NVIDIA graphics",
        "card). The player, on Windows and on the Mac, never needs it. Do not rename or move the",
        "files: the Studio looks for them exactly here.",
        "",
        f"These are the default models: separation preset \"{a.preset}\", the lyrics alignment model and",
        "the lyrics transcription model.",
        "",
        "Not included: the models of the \"Clean\" and \"Permissive\" presets. When you choose one",
        "of them in the Studio form, the form lists the models it still needs, with their size",
        "and licence, and downloads them into this folder only when you press \"Download\"",
        "(from the pinned sources in ../Licenses/MODELS.md; each file's SHA-256 is checked).",
        "Clean needs one more model, 204 MB, whose author has published no licence (commercial",
        "use unclear). Permissive needs one more model, 913 MB (MIT licence). Until then a song",
        "with that preset stops with \"models missing\". Nothing is ever downloaded by itself.",
        "The other models listed in MODELS.md are not offered by the app.",
        "",
        f"{len(files)} files, {total:,} bytes in total.",
        "",
        "manifest.json   the Studio's own record of which files it has checked. On a new drive",
        "                the Studio checks every file's SHA-256 once more before first use (a",
        "                few seconds on an SSD, up to a minute or two on a slow USB stick).",
        "SHA256SUMS.txt  the same SHA-256 values for people and tools (sha256sum -c SHA256SUMS.txt).",
        "",
        "The model licences allow copying them along with the app (details: ../Licenses/MODELS.md):",
        "",
    ]
    for f in sorted(files, key=lambda f: (f["kind"], f["rel"])):
        if f["size"] < 10_000_000:
            continue
        lines.append(f"- {f['rel']}")
        lines.append(f"    {f['role']}; model {f['model']}; licence {f['licence']}; {f['size']:,} bytes")
        lines.append(f"    source {f['url']}")
    lines += ["", "Small configuration and tokenizer files of the two lyrics models sit next to their",
              "weights (listed in SHA256SUMS.txt).", ""]
    (dest / "README.txt").write_text("\n".join(lines), encoding="utf-8", newline="\n")

    print(json.dumps({"files": len(files), "bytes": total, "copy_s": round(t_copy, 1),
                      "verify_s": round(t_verify, 1), "manifest_entries": len(manifest)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())

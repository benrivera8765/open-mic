# SPDX-License-Identifier: GPL-3.0-or-later
"""WO2 Stage 2 measurements (docs/benchmarks/practice-guide.md, "Stage 2"): adding a melody
guide to existing songs with ``jukeoke-cli guide add`` (the same jk-guide code as the app).

For each song (fresh copies; the originals are only read):

- the SHA-256 of every chunk and of both settings slot regions before and after: every
  chunk but the guide's (stems, lyrics, cover, manifest, unknown chunks) and both slots
  must be byte-identical, at the same offsets;
- the time of ``guide add`` (build + raw-copy rewrite; the build alone is the guide's
  ``provenance.seconds``), three runs (the first adds, two more with ``--force``);
- the bytes the guide adds (``guide`` JSON + ``guide.curve``) and the file growth;
- ``jukeoke-cli verify --strict --deep`` of the result.

    $JK_BUILD/studio-venv/Scripts/python.exe -I studio/bench/practice_guide_stage2.py

Writes ``$JK_BUILD/practice-stage2/measure/results.json``. Prints no lyrics.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "studio"))
from jukeoke_studio import jksong  # noqa: E402

JK_BUILD = Path(os.environ.get("JK_BUILD", REPO.parent / "jukeoke-build"))
CLI = JK_BUILD / "target-wo2-stage2" / "release" / ("jukeoke-cli.exe" if os.name == "nt" else "jukeoke-cli")
OUT = JK_BUILD / "practice-stage2" / "measure"
SOURCES = {
    "Cortez - Feel (Stripped)": JK_BUILD / "e2e-stick" / "Library" / "Cortez - Feel (Stripped).jksong",
    "coruscate - The Homeless Man": JK_BUILD / "e2e-stick" / "Library" / "coruscate - The Homeless Man.jksong",
    "Jason Miller - Crowdpleaser": JK_BUILD / "e2e-stick" / "Library" / "Jason Miller - Crowdpleaser.jksong",
    "owner's song": REPO / "dist" / "Library" / "Imagine Dragons - It's Time.jksong",
    "demo-song": REPO / "tests" / "fixtures" / "generated" / "demo-song.jksong",
}


def sha_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        while b := f.read(1 << 20):
            h.update(b)
    return h.hexdigest()


def fingerprint(p: Path) -> dict:
    s = jksong.Song(p)
    h = s.header
    out = {"chunks": {}, "offsets": {}}
    with open(p, "rb") as f:
        for c in s.toc["chunks"]:
            f.seek(c["offset"])
            out["chunks"][c["id"]] = hashlib.sha256(f.read(c["length"])).hexdigest()
            out["offsets"][c["id"]] = c["offset"]
        for name, off in (("slot_a", h["slot_a_offset"]), ("slot_b", h["slot_b_offset"])):
            f.seek(off)
            out[name] = hashlib.sha256(f.read(h["slot_size"])).hexdigest()
    out["lengths"] = {c["id"]: c["length"] for c in s.toc["chunks"]}
    return out


def add(song: Path, force: bool) -> tuple[dict, float]:
    t = time.perf_counter()
    r = subprocess.run([str(CLI), "guide", "add", str(song), "--json"] + (["--force"] if force else []),
                       capture_output=True, text=True)
    wall = time.perf_counter() - t
    if r.returncode != 0:
        raise RuntimeError(r.stdout + r.stderr)
    return json.loads(r.stdout.strip().splitlines()[-1]), wall


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    results = {}
    for name, src in SOURCES.items():
        src_sha = sha_file(src)
        dst = OUT / (src.name)
        shutil.copyfile(src, dst)
        before = fingerprint(dst)
        size0 = dst.stat().st_size
        runs = []
        for k in range(3):
            line, wall = add(dst, force=k > 0)
            g = jksong.Song(dst).guide()
            runs.append({"wall_s": round(wall, 3), "build_s": g["provenance"]["seconds"], "notes": line["notes"]})
            after = fingerprint(dst)
            kept = {cid: after["chunks"].get(cid) == sha and after["offsets"].get(cid) == before["offsets"][cid]
                    for cid, sha in before["chunks"].items()}
            if not all(kept.values()) or (after["slot_a"], after["slot_b"]) != (before["slot_a"], before["slot_b"]):
                raise SystemExit(f"{name}: something besides the guide changed: {kept}")
        v = subprocess.run([str(CLI), "verify", str(dst), "--strict", "--deep"], capture_output=True, text=True)
        verdict = [ln for ln in v.stdout.splitlines() if ln.startswith(("PASS", "FAIL"))]
        guide_line = [ln for ln in v.stdout.splitlines() if ln.startswith("guide:")]
        s = jksong.Song(dst)
        g = s.guide()
        counts = jksong.guide_counts(g)
        results[name] = {
            "source_sha256_unchanged": sha_file(src) == src_sha,
            "file_bytes_before": size0,
            "file_bytes_after": dst.stat().st_size,
            "guide_bytes": after["lengths"]["guide"],
            "curve_bytes": after["lengths"]["guide.curve"],
            "duration_s": s.manifest["duration_seconds"],
            "lead_bytes": before["lengths"][s.stem("lead_vocal")["chunk"]],
            "chunks_checked": sorted(before["chunks"]),
            "every_other_chunk_and_both_slots_identical": True,
            "runs": runs,
            "verify": verdict[-1] if verdict else v.stdout[-300:],
            "verify_guide": guide_line[-1] if guide_line else None,
            "counts": counts,
            "spans": len(g["spans"]),
            "notes_with_word": sum(1 for n in g["notes"] if "word" in n),
            "tuning": g["tuning"],
            "method": g["provenance"]["method"], "version": g["provenance"]["version"],
        }
        r = results[name]
        print(f"{name}: {r['duration_s']:.1f} s, build {min(x['build_s'] for x in runs):.2f} s, "
              f"add {min(x['wall_s'] for x in runs):.2f}-{max(x['wall_s'] for x in runs):.2f} s, "
              f"guide {r['guide_bytes'] / 1024:.1f} KiB + curve {r['curve_bytes'] / 1024:.1f} KiB, "
              f"{counts['notes']} notes ({counts['graded']} graded), {r['verify']}")
    (OUT / "results.json").write_text(json.dumps(results, indent=1))
    print("written", OUT / "results.json")


if __name__ == "__main__":
    main()

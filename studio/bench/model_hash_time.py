# SPDX-License-Identifier: GPL-3.0-or-later
"""First-run model re-hash time on a new drive (D-172: the Studio hashes each model file
once per drive before trusting it). Results: docs/TEST_RESULTS.md section 7.

Measures, on the Models folder given (default dist/Models), with the Studio's own hash
function (``modelzoo.download.sha256_of``: SHA-256 over 4 MiB reads, read then hash):

1. hash speed with the files in the OS cache (CPU only: the upper bound of any drive);
2. raw sequential read speed of this drive with the OS cache bypassed
   (FILE_FLAG_NO_BUFFERING, Windows only), so the figure is the disk, not RAM;
3. the same uncached read with each 4 MiB block hashed as it arrives (what a first run
   does on this drive).

Then estimates the time for USB sticks, assuming the Studio's serial read-then-hash
loop: t = bytes / read_speed + bytes / hash_speed, for 30 MB/s (USB 2), 100, 150 and
200 MB/s (USB 3 sticks). Also per job: the separation models plus alignment (lyrics
given) and plus Whisper (lyrics transcribed).

    python -I studio/bench/model_hash_time.py [Models folder] [--json out.json]
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import mmap
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from jukeoke_studio.modelzoo.download import sha256_of  # noqa: E402

CHUNK = 1 << 22
MB = 1e6


def files_of(models: Path) -> list[Path]:
    m = json.loads((models / "manifest.json").read_text(encoding="utf-8"))
    return [models / k for k in sorted(m)]


def read_uncached(path: Path, hash_too: bool) -> tuple[int, float, str | None]:
    """Sequential read with FILE_FLAG_NO_BUFFERING (page-aligned buffer, 4 MiB reads)."""
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateFileW.restype = ctypes.c_void_p
    k32.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    k32.ReadFile.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p]
    k32.CloseHandle.argtypes = [ctypes.c_void_p]
    GENERIC_READ, SHARE_READ, OPEN_EXISTING = 0x80000000, 1, 3
    NO_BUFFERING, SEQUENTIAL = 0x20000000, 0x08000000
    h = k32.CreateFileW(str(path), GENERIC_READ, SHARE_READ, None, OPEN_EXISTING, NO_BUFFERING | SEQUENTIAL, None)
    if h in (None, ctypes.c_void_p(-1).value):
        raise OSError(ctypes.get_last_error(), f"CreateFileW {path}")
    buf = mmap.mmap(-1, CHUNK)  # page-aligned
    addr = ctypes.addressof(ctypes.c_char.from_buffer(buf))
    hasher = hashlib.sha256() if hash_too else None
    total = 0
    got = ctypes.c_uint32(0)
    t = time.perf_counter()
    try:
        while True:
            if not k32.ReadFile(h, addr, CHUNK, ctypes.byref(got), None):
                raise OSError(ctypes.get_last_error(), f"ReadFile {path}")
            n = got.value
            if n == 0:
                break
            total += n
            if hasher:
                hasher.update(memoryview(buf)[:n])
            if n < CHUNK:
                break
    finally:
        k32.CloseHandle(h)
    dt = time.perf_counter() - t
    del addr
    buf.close()
    return total, dt, hasher.hexdigest() if hasher else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("models", nargs="?", type=Path, default=HERE.parents[1] / "dist" / "Models")
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    manifest = json.loads((a.models / "manifest.json").read_text(encoding="utf-8"))
    files = files_of(a.models)
    total = sum(p.stat().st_size for p in files)
    out = {"models": str(a.models), "files": len(files), "bytes": total}

    # 1. warm: read every file once into the cache, then time the Studio's hash.
    for p in files:
        sha256_of(p)
    t = time.perf_counter()
    for p in files:
        assert sha256_of(p) == manifest[p.relative_to(a.models).as_posix()]["sha256"], p
    warm = time.perf_counter() - t
    out["warm_hash_s"] = warm
    out["hash_MBps"] = total / warm / MB

    # 2. and 3. uncached (Windows only).
    if sys.platform == "win32":
        rt = sum(read_uncached(p, False)[1] for p in files)
        out["uncached_read_s"] = rt
        out["read_MBps"] = total / rt / MB
        ht = 0.0
        for p in files:
            n, dt, digest = read_uncached(p, True)
            assert digest == manifest[p.relative_to(a.models).as_posix()]["sha256"], p
            ht += dt
        out["uncached_read_and_hash_s"] = ht

    hash_bps = total / warm
    groups = {
        "all 15 files (the whole Models folder)": files,
        "separation (preset full: 3 files)": [p for p in files if p.suffix in (".ckpt", ".pth")],
        "alignment (wav2vec2)": [p for p in files if "wav2vec2" in p.as_posix()],
        "transcription (Whisper turbo)": [p for p in files if "whisper" in p.as_posix()],
    }
    out["groups"] = {}
    for name, fs in groups.items():
        b = sum(p.stat().st_size for p in fs)
        est = {f"{r} MB/s": b / (r * MB) + b / hash_bps for r in (30, 100, 150, 200)}
        out["groups"][name] = {"bytes": b, "estimate_s": est}
    print(json.dumps(out, indent=1))
    if a.json:
        a.json.write_text(json.dumps(out, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())

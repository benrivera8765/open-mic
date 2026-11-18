#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# build-studio-runtime.sh - build the relocatable Studio runtime (WORK_ORDER 4.8, trap 4).
#
#   scripts/build-studio-runtime.sh <out_root> [--keep-cuda] [--keep-all-transformers] [--pbs DIR] [--vcredist DIR]
#
# Produces, under <out_root> (the root of a stick laid out as in WORK_ORDER 4.8):
#
#   Windows/Studio/python/                 python-build-standalone CPython 3.12.16 with the
#                                          locked Studio dependencies and the jukeoke_studio
#                                          package installed straight into it (no venv, so
#                                          no pyvenv.cfg with an absolute "home" path)
#   Windows/Studio/runtime.json            what was built: versions, lock hash, package
#                                          list with licences, VC runtime, strip record
#   Windows/Studio/runtime-build.json      size after every build/strip step
#   Windows/Studio/runtime-binaries.tsv    every bundled .dll/.pyd/.exe with its owning
#                                          package and licence (input for
#                                          THIRD_PARTY_LICENSES.md)
#   Windows/Studio/requirements.pylock.toml  the exact artifact list that was installed
#
# The app starts it as  <root>/Windows/Studio/python/python.exe -I -X utf8 -m jukeoke_studio serve
# (DECISIONS D-83). -I never runs the Scripts/*.exe launchers, which embed the build
# machine's absolute python.exe path, so they are deleted.
#
# Steps (each measured, see docs/STUDIO_RUNTIME.md):
#   1. copy the interpreter tree;  2. install exactly the uv.lock artifacts (pylock.toml
#   export: pinned URLs + SHA-256) with uv, no dependency resolution, files copied (not
#   hard-linked to the uv cache);  3. install jukeoke_studio non-editable from a snapshot
#   of studio/;  4. strip: bytecode caches + installers + launchers, tests, development
#   files (headers, import libraries, Cython sources, CMake files, torch's protoc), the Tk
#   GUI toolkit, the transformers model families the Studio cannot load (unless
#   --keep-all-transformers; only the wav2vec2 / HuBERT CTC families stay), and (unless
#   --keep-cuda) the CUDA libraries proven unused;  5. bundle
#   the Microsoft Visual C++ runtime DLLs that the binaries import but a clean Windows 11
#   does not ship;  6. byte-compile everything (unchecked-hash .pyc, build path stripped);
#   7. metadata clean-up, runtime.json, binaries list.
#
# Nothing here touches the network except uv fetching a locked wheel that is missing from
# its cache. Needs: scripts/devenv.sh (uv), the interpreter tree in
# $JK_BUILD/python/cpython-3.12.16-windows-x86_64-none.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO/scripts/devenv.sh"

usage() { sed -n '4,6p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

OUT_ROOT=""
STRIP_CUDA=1
STRIP_TRANSFORMERS=1
PBS_DIR="${JK_PBS_DIR:-$JK_BUILD/python/cpython-3.12.16-windows-x86_64-none}"
VCREDIST_DIR="${JK_VCREDIST_DIR:-}"
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-cuda) STRIP_CUDA=0 ;;
        --keep-all-transformers) STRIP_TRANSFORMERS=0 ;;
        --pbs) PBS_DIR="$2"; shift ;;
        --vcredist) VCREDIST_DIR="$2"; shift ;;
        -h|--help) usage ;;
        -*) echo "unknown option $1" >&2; usage ;;
        *) [ -z "$OUT_ROOT" ] || usage; OUT_ROOT="$1" ;;
    esac
    shift
done
[ -n "$OUT_ROOT" ] || usage
[ -x "$PBS_DIR/python.exe" ] || { echo "interpreter tree not found: $PBS_DIR" >&2; exit 1; }
if [ -z "$VCREDIST_DIR" ]; then
    # The Visual C++ 2015-2022 redistributable as installed on the build machine.
    VCREDIST_DIR="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32"
fi

mkdir -p "$OUT_ROOT"
OUT_ROOT="$(cd "$OUT_ROOT" && pwd)"
STUDIO="$OUT_ROOT/Windows/Studio"
PY="$STUDIO/python"
PYEXE="$PY/python.exe"
PYW="$(cygpath -w "$PY")"
# Helper interpreter: the source tree, never the runtime being built (its vcruntime140.dll
# is replaced in step 5 and would be locked by a running copy of itself).
HPY="$PBS_DIR/python.exe"

# Only ever delete a runtime this script built (it leaves runtime-build.json behind).
if [ -e "$PY" ]; then
    if [ -f "$STUDIO/runtime-build.json" ] || [ -f "$STUDIO/runtime-build.partial" ]; then
        echo "removing the previous runtime in $STUDIO"
        rm -rf "$PY" "$STUDIO/runtime.json" "$STUDIO/runtime-build.json" \
               "$STUDIO/runtime-binaries.tsv" "$STUDIO/requirements.pylock.toml"
    else
        echo "$PY exists and was not built by this script; refusing to overwrite it" >&2
        exit 1
    fi
fi
mkdir -p "$STUDIO"
: > "$STUDIO/runtime-build.partial"

mkdir -p "$JK_BUILD/tmp"
WORK="$(mktemp -d "$JK_BUILD/tmp/studio-runtime.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------------------
# Python helper (standard library only; runs with the runtime's own interpreter, -I -B so
# it neither sees the user's environment nor writes bytecode into the tree it measures).
# ---------------------------------------------------------------------------------------
cat > "$WORK/rt.py" <<'PYEOF'
# SPDX-License-Identifier: GPL-3.0-or-later
"""Helper for scripts/build-studio-runtime.sh (measure, strip, VC runtime, metadata)."""
import ctypes
import fnmatch
import hashlib
import json
import mmap
import os
import re
import shutil
import struct
import sys
import time
from pathlib import Path

CLUSTERS = (32768, 131072)  # exFAT defaults: 32 KB (<= 32 GB volumes), 128 KB (larger)


def walk(root):
    for dp, dns, fns in os.walk(root):
        yield dp, dns, fns


def measure(root):
    files = dirs = size = 0
    alloc = {c: 0 for c in CLUSTERS}
    for dp, dns, fns in walk(root):
        dirs += 1
        for c in CLUSTERS:          # an exFAT directory takes at least one cluster
            alloc[c] += c
        for f in fns:
            s = os.lstat(os.path.join(dp, f)).st_size
            files += 1
            size += s
            for c in CLUSTERS:
                alloc[c] += -(-s // c) * c
    return {"files": files, "dirs": dirs, "bytes": size,
            "alloc_32k": alloc[32768], "alloc_128k": alloc[131072]}


def rm(path, removed):
    p = Path(path)
    if p.is_dir() and not p.is_symlink():
        m = measure(p)
        shutil.rmtree(p)
        removed["files"] += m["files"]
        removed["bytes"] += m["bytes"]
    elif p.exists() or p.is_symlink():
        removed["files"] += 1
        removed["bytes"] += p.lstat().st_size
        p.unlink()
    else:
        return
    removed["items"].append(str(p))


# ------------------------------------------------------------------------- strip rules
TEST_DIRS = {"tests", "test"}
# Not *.pyi: librosa (lazy_loader.attach_stub) reads its __init__.pyi at import time.
DEV_FILE_GLOBS = ("*.lib", "*.a", "*.h", "*.hh", "*.hpp", "*.hxx", "*.cuh", "*.pdb",
                  "*.pxd", "*.pxi", "*.pyx", "*.cmake")
# CUDA libraries the Studio never loads (proof: docs/STUDIO_RUNTIME.md "CUDA strip").
CUDA_UNUSED = (
    "Lib/site-packages/torch/lib/cusolverMg64_12.dll",   # multi-GPU dense solver
    "Lib/site-packages/torch/lib/curand64_10.dll",       # host RNG library (torch uses Philox kernels)
    "Lib/site-packages/torch/lib/nvperf_host.dll",       # profiler counters (CUPTI range profiling)
    "Lib/site-packages/nvidia/cublas/bin/nvblas64_12.dll",  # CPU-BLAS interposer, never loaded
    "Lib/site-packages/nvidia/cuda_nvrtc",               # CUDA 12 NVRTC: nothing CUDA-12 JIT-compiles
)
# transformers ships about 550 model families (5,000 files, 93 MB, 246 MB on a 32 KB-cluster
# exFAT stick once byte-compiled). The Studio loads only CTC character models of the
# wav2vec2 and HuBERT families (jukeoke_studio/align.py through AutoModelForCTC; registry
# in jukeoke_studio/modelzoo/hfmodels.py); Whisper runs on CTranslate2, not transformers.
# transformers finds its models by scanning models/ at import (define_import_structure) and
# the Auto classes import a family only when a config of that model_type is loaded, so
# the other folders are never touched. Proof: the check builds both kept families through
# AutoConfig/AutoModelForCTC, verifies every registered alignment model's model_type is
# importable, and the full pipeline gives bit-identical stems and aligned lyrics with and
# without this step (docs/STUDIO_RUNTIME.md section 3). A new alignment family must be
# added here (the check fails until it is).
TRANSFORMERS_KEEP = {"__init__.py", "__pycache__", "auto", "wav2vec2", "hubert"}


def strip(py, step):
    py = Path(py)
    sp = py / "Lib" / "site-packages"
    removed = {"files": 0, "bytes": 0, "items": []}

    def in_dist_info(p):  # licences live there; never strip inside a *.dist-info
        return any(part.endswith(".dist-info") for part in Path(p).parts)

    if step == "caches":
        # stale bytecode copied with the interpreter tree, pip and its bootstrapper,
        # and the console-script launchers (absolute python.exe path inside; -I -m
        # never uses them)
        for dp, dns, fns in walk(py):
            for d in list(dns):
                if d == "__pycache__":
                    rm(os.path.join(dp, d), removed)
                    dns.remove(d)
        rm(py / "Scripts", removed)
        rm(py / "Lib" / "ensurepip", removed)
        for p in sp.glob("pip"):
            rm(p, removed)
        for p in sp.glob("pip-*.dist-info"):
            rm(p, removed)
    elif step == "tests":
        for dp, dns, fns in walk(py / "Lib"):
            if in_dist_info(dp):
                dns[:] = []
                continue
            for d in list(dns):
                if d in TEST_DIRS or d == "idle_test":
                    rm(os.path.join(dp, d), removed)
                    dns.remove(d)
        for p in (py / "DLLs").glob("_test*.pyd"):
            rm(p, removed)
        rm(py / "DLLs" / "_ctypes_test.pyd", removed)
    elif step == "devfiles":
        rm(py / "include", removed)
        rm(py / "libs", removed)
        rm(sp / "torch" / "share", removed)   # CMake package files
        rm(sp / "torch" / "bin", removed)     # protoc.exe, a build tool
        for dp, dns, fns in walk(sp):
            if in_dist_info(dp):
                dns[:] = []
                continue
            for d in list(dns):
                if d == "include":
                    rm(os.path.join(dp, d), removed)
                    dns.remove(d)
            for f in fns:
                if any(fnmatch.fnmatch(f.lower(), g) for g in DEV_FILE_GLOBS):
                    rm(os.path.join(dp, f), removed)
        for dp, dns, fns in walk(py):
            for f in fns:
                if f.lower().endswith(".pdb"):
                    rm(os.path.join(dp, f), removed)
    elif step == "gui":
        # Tcl/Tk, IDLE and turtle: a GUI toolkit the Studio (a stdio process) never uses;
        # share/ holds man pages installed by packages.
        for rel in ("tcl", "share", "Lib/tkinter", "Lib/idlelib", "Lib/turtledemo",
                    "Lib/turtle.py", "DLLs/_tkinter.pyd", "DLLs/tcl86t.dll", "DLLs/tk86t.dll"):
            rm(py / rel, removed)
    elif step == "transformers":
        models = sp / "transformers" / "models"
        for p in sorted(models.iterdir()):
            if p.name not in TRANSFORMERS_KEEP:
                rm(p, removed)
    elif step == "cuda":
        for rel in CUDA_UNUSED:
            rm(py / rel, removed)
    else:
        raise SystemExit(f"unknown strip step {step}")
    base = str(py)
    removed["items"] = sorted(os.path.relpath(i, base).replace("\\", "/") for i in removed["items"])
    return removed


# ------------------------------------------------------------------- PE import tables
def pe_imports(path):
    """(static, delay) imported DLL names of a PE file, or None if it is not one."""
    try:
        f = open(path, "rb")
    except OSError:
        return None
    with f:
        try:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        except (OSError, ValueError):
            return None
    try:
        if mm[:2] != b"MZ":
            return None
        pe = struct.unpack_from("<I", mm, 0x3C)[0]
        if mm[pe:pe + 4] != b"PE\0\0":
            return None
        nsec = struct.unpack_from("<H", mm, pe + 6)[0]
        optsz = struct.unpack_from("<H", mm, pe + 20)[0]
        opt = pe + 24
        magic = struct.unpack_from("<H", mm, opt)[0]
        if magic == 0x20B:
            imagebase = struct.unpack_from("<Q", mm, opt + 24)[0]
            ndd = struct.unpack_from("<I", mm, opt + 108)[0]
            dd = opt + 112
        elif magic == 0x10B:
            imagebase = struct.unpack_from("<I", mm, opt + 28)[0]
            ndd = struct.unpack_from("<I", mm, opt + 92)[0]
            dd = opt + 96
        else:
            return None
        secs = []
        sh = opt + optsz
        for i in range(nsec):
            vsz, va, rsz, rptr = struct.unpack_from("<IIII", mm, sh + 40 * i + 8)
            secs.append((va, max(vsz, rsz), rptr, rsz))

        def off(rva):
            for va, sz, rptr, rsz in secs:
                if va <= rva < va + sz and rva - va < rsz:
                    return rptr + rva - va
            return None

        def cstr(rva):
            o = off(rva)
            if o is None:
                return None
            e = mm.find(b"\0", o, o + 512)
            return mm[o:e].decode("ascii", "replace") if e > o else None

        def entry(i):
            return struct.unpack_from("<II", mm, dd + 8 * i) if i < ndd else (0, 0)

        static, delay = [], []
        rva, _ = entry(1)
        o = off(rva) if rva else None
        while o is not None and o + 20 <= len(mm):
            d = struct.unpack_from("<IIIII", mm, o)
            if not any(d):
                break
            n = cstr(d[3])
            if n:
                static.append(n)
            o += 20
        rva, _ = entry(13)
        o = off(rva) if rva else None
        while o is not None and o + 32 <= len(mm):
            d = struct.unpack_from("<8I", mm, o)
            if not any(d):
                break
            name_rva = d[1] if d[0] & 1 else d[1] - imagebase
            n = cstr(name_rva)
            if n:
                delay.append(n)
            o += 32
        return static, delay
    except struct.error:
        return None
    finally:
        mm.close()


def _ver_dll():
    v = ctypes.WinDLL("version", use_last_error=True)
    v.GetFileVersionInfoSizeW.argtypes = [ctypes.c_wchar_p, ctypes.c_void_p]
    v.GetFileVersionInfoSizeW.restype = ctypes.c_uint32
    v.GetFileVersionInfoW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    v.VerQueryValueW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p,
                                 ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_uint)]
    return v


def version_info(path):
    """(file version tuple, ProductName, CompanyName) from the version resource, None if absent."""
    v = _ver_dll()
    n = v.GetFileVersionInfoSizeW(str(path), None)
    if not n:
        return None, None, None
    buf = ctypes.create_string_buffer(n)
    if not v.GetFileVersionInfoW(str(path), 0, n, buf):
        return None, None, None
    p, ln = ctypes.c_void_p(), ctypes.c_uint()
    ver = None
    if v.VerQueryValueW(buf, "\\", ctypes.byref(p), ctypes.byref(ln)) and ln.value >= 52:
        ff = struct.unpack("<13I", ctypes.string_at(p.value, 52))
        ver = (ff[2] >> 16, ff[2] & 0xFFFF, ff[3] >> 16, ff[3] & 0xFFFF)
    langs = []
    if v.VerQueryValueW(buf, "\\VarFileInfo\\Translation", ctypes.byref(p), ctypes.byref(ln)) and ln.value >= 4:
        langs.append(struct.unpack("<HH", ctypes.string_at(p.value, 4)))
    langs += [(0x0409, 0x04B0), (0x0409, 0x04E4), (0, 0x04B0)]
    strings = {}
    for key in ("ProductName", "CompanyName"):
        for lang, cp in langs:
            if v.VerQueryValueW(buf, "\\StringFileInfo\\%04x%04x\\%s" % (lang, cp, key),
                                ctypes.byref(p), ctypes.byref(ln)) and ln.value:
                strings[key] = ctypes.wstring_at(p.value, ln.value).rstrip("\0")
                break
    return ver, strings.get("ProductName"), strings.get("CompanyName")


VC_RE = re.compile(r"^(msvcp140(_\w+)?|vcruntime140(_\w+)?|vcomp140|concrt140|vccorlib140|mfc140\w*)\.dll$", re.I)
# Microsoft DLLs that are in System32 only when an installer put them there (VC++ and
# DirectX redistributables, old CRTs, debug CRTs): never counted as part of Windows.
REDIST_RE = re.compile(r"^(msvc[pr]\d+\w*|vcruntime\d+\w*|vcomp\d+|concrt\d+|vccorlib\d+|mfc\d+\w*|atl\d+|"
                       r"d3dx\w+|d3dcompiler_(?!47)\d+|xinput1_[123]|xaudio2_[0-7]|x3daudio\w+|ucrtbased)\.dll$",
                       re.I)
SYSTEM32 = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
# Imports allowed to stay unresolved: optional extension modules whose import fails
# cleanly and which their library treats as optional.
OPTIONAL_MISSING = {
    "tbb12.dll": "numba/np/ufunc/tbbpool: numba's optional Intel TBB threading layer; tbb is not a "
                 "Studio dependency and numba falls back to its OpenMP or workqueue layer "
                 "(same as in the development venv)",
}


def classify_system32(name):
    """os (in System32 with a Microsoft company name and not a redistributable), vc_redist,
    driver (NVIDIA), system32_other or missing."""
    p = SYSTEM32 / name
    if not p.exists():
        return "missing", None
    ver, prod, company = version_info(p)
    prod, company = prod or "", company or ""
    if VC_RE.match(name):
        return "vc_redist", prod
    if "NVIDIA" in prod or "NVIDIA" in company:
        return "driver", prod
    if company.startswith("Microsoft") and not REDIST_RE.match(name):
        return "os", prod
    return "system32_other", prod


def pe_files(py):
    out = []
    for dp, dns, fns in walk(py):
        for f in fns:
            if f.lower().endswith((".dll", ".pyd", ".exe")):
                out.append(Path(dp) / f)
    return out


def dll_dirs(py):
    """Folders every Studio process adds to its DLL search (os.add_dll_directory): torch
    adds torch/lib, jukeoke_studio.transcribe adds nvidia/cublas/bin, and delvewheel /
    scikit-learn wheels add their own *.libs / .libs folders when imported."""
    sp = Path(py) / "Lib" / "site-packages"
    return ([sp / "torch" / "lib"] + sorted(sp.glob("nvidia/*/bin")) + sorted(sp.glob("*.libs"))
            + sorted(sp.glob("*/.libs")))


def closure(py):
    """Every imported DLL name, resolved the way the Windows loader will find it.

    A dependency resolves inside the runtime if it sits next to the importing binary or
    in the interpreter's folder (the application directory), or - except for the Visual
    C++ runtime - in one of the package DLL folders (:func:`dll_dirs`). The VC runtime
    must come from the application directory: torch loads msvcp140.dll by bare name
    before any package folder that happens to carry a copy (scikit-learn's .libs) has
    been added, so on a clean Windows a copy elsewhere would not be found."""
    py = Path(py)
    files = pe_files(py)
    by_dir = {}
    for p in files:
        by_dir.setdefault(p.parent, set()).add(p.name.lower())
    root_names = by_dir.get(py, set())
    global_names = set()
    for d in dll_dirs(py):
        global_names |= by_dir.get(d, set())
    res = {"bundled": set(), "os": set(), "driver": set(), "vc_redist": {}, "system32_other": {},
           "missing_static": {}, "missing_delay": {}}
    cache = {}
    for p in files:
        imp = pe_imports(p)
        if imp is None:
            continue
        own = by_dir.get(p.parent, set())
        for kind, names in (("static", imp[0]), ("delay", imp[1])):
            for n in names:
                ln = n.lower()
                rel = os.path.relpath(p, py).replace("\\", "/")
                if ln.startswith(("api-ms-win-", "ext-ms-")):
                    res["os"].add(ln)
                    continue
                if ln in own or ln in root_names or (ln in global_names and not VC_RE.match(ln)):
                    res["bundled"].add(ln)
                    continue
                if ln not in cache:
                    cache[ln] = classify_system32(n)
                cls, prod = cache[ln]
                if cls == "os":
                    res["os"].add(ln)
                elif cls == "driver":
                    res["driver"].add(ln)
                elif cls in ("vc_redist", "system32_other"):
                    res[cls].setdefault(ln, []).append(rel)
                elif ln in OPTIONAL_MISSING:
                    res.setdefault("optional_missing", {}).setdefault(ln, []).append(rel)
                else:
                    res["missing_" + kind].setdefault(ln, []).append(rel)
    return res


def vcredist(py, src):
    """Copies the Visual C++ runtime DLLs the binaries import into the interpreter's
    folder (the application directory, searched first by every loader path Python and
    torch use), and replaces the interpreter's own older vcruntime140*.dll so the whole
    process uses one consistent runtime version."""
    py, src = Path(py), Path(src)
    copied, replaced = [], []
    bundled_vc = {}
    for p in pe_files(py):
        base = re.sub(r"-[0-9a-f]{32}(?=\.dll$)", "", p.name.lower())
        if VC_RE.match(base):
            bundled_vc[os.path.relpath(p, py).replace("\\", "/")] = version_info(p)[0]
    newest_bundled = max((v for v in bundled_vc.values() if v), default=(0, 0, 0, 0))
    for _ in range(5):
        res = closure(py)
        need = sorted(res["vc_redist"])
        if not need:
            break
        for n in need:
            s = src / n
            if not s.exists():
                raise SystemExit(f"VC runtime {n} is needed but not in {src}")
            shutil.copy2(s, py / n)
            copied.append(n)
    src_ver = version_info(src / "vcruntime140.dll")[0]
    for n in ("vcruntime140.dll", "vcruntime140_1.dll"):
        own = py / n
        if own.exists() and (src / n).exists():
            ov, sv = version_info(own)[0], version_info(src / n)[0]
            if ov and sv and sv > ov:
                shutil.copy2(src / n, own)
                replaced.append({"dll": n, "from": ".".join(map(str, ov)), "to": ".".join(map(str, sv))})
    final = {}
    for n in sorted(set(copied) | {"vcruntime140.dll", "vcruntime140_1.dll"}):
        if (py / n).exists():
            final[n] = ".".join(map(str, version_info(py / n)[0] or ()))
    if src_ver and src_ver < newest_bundled:
        print(f"WARNING: VC runtime source {src_ver} is older than one bundled by a wheel {newest_bundled}",
              file=sys.stderr)
    res = closure(py)
    return {"copied": sorted(set(copied)), "replaced": replaced, "versions": final,
            "wheel_bundled_vc": {k: ".".join(map(str, v or ())) for k, v in bundled_vc.items()},
            "unresolved_static": res["missing_static"], "unresolved_delay": res["missing_delay"],
            "system32_other": res["system32_other"], "vc_still_needed": res["vc_redist"],
            "optional_missing": {k: {"imported_by": v, "why_allowed": OPTIONAL_MISSING[k]}
                                 for k, v in res.get("optional_missing", {}).items()},
            "driver": sorted(res["driver"])}


# ------------------------------------------------------------------- metadata, report
def dists(sp):
    out = []
    for di in sorted(Path(sp).glob("*.dist-info")):
        meta = {}
        try:
            text = (di / "METADATA").read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        head = text.split("\n\n", 1)[0]
        classifiers = []
        for line in head.splitlines():
            if ":" in line and not line.startswith((" ", "\t")):
                k, v = line.split(":", 1)
                v = v.strip()
                if k == "Classifier" and v.startswith("License ::"):
                    classifiers.append(v.split("::")[-1].strip())
                elif k in ("Name", "Version", "License-Expression") and k not in meta:
                    meta[k] = v
                elif k == "License" and "License" not in meta:
                    meta["License"] = v
        lic = meta.get("License-Expression") or ""
        if not lic:
            l1 = (meta.get("License") or "").strip()
            lic = l1 if l1 and len(l1) < 80 and "\n" not in l1 else ""
        if not lic and classifiers:
            lic = " / ".join(classifiers)
        files = []
        try:
            for row in (di / "RECORD").read_text(encoding="utf-8", errors="replace").splitlines():
                if row:
                    files.append(row.rsplit(",", 2)[0])
        except OSError:
            pass
        lic_files = sorted(os.path.relpath(p, sp).replace("\\", "/")
                           for p in di.rglob("*") if p.is_file() and re.search(r"(?i)licen[cs]e|copying|notice", p.name))
        out.append({"name": meta.get("Name", di.name), "version": meta.get("Version", ""),
                    "licence": lic or "(see licence files)", "dist_info": di.name,
                    "licence_files": lic_files, "files": files})
    return out


def fix_metadata(py):
    """direct_url.json records the build machine's file:// URL for a package installed
    from a local folder: delete it (and its RECORD line). It is pip-freeze metadata only."""
    sp = Path(py) / "Lib" / "site-packages"
    fixed = []
    for du in sp.glob("*.dist-info/direct_url.json"):
        try:
            url = json.loads(du.read_text(encoding="utf-8")).get("url", "")
        except ValueError:
            url = ""
        if url.startswith("file:"):
            du.unlink()
            rec = du.parent / "RECORD"
            if rec.exists():
                lines = [l for l in rec.read_text(encoding="utf-8").splitlines()
                         if not l.split(",", 1)[0].endswith("/direct_url.json")]
                rec.write_text("\n".join(lines) + "\n", encoding="utf-8")
            fixed.append(du.parent.name)
    return fixed


COMPONENTS = [
    # (label, predicate on path relative to site-packages, lower-case, forward slashes)
    ("torch: NVIDIA CUDA 13 libraries (torch/lib cu*, cudnn*, nvrtc*, nvJitLink, cupti, nvperf, nvToolsExt)",
     lambda r: r.startswith("torch/lib/") and re.match(r"torch/lib/(cu|nv|caffe2_nvrtc)", r) is not None),
    ("torch: own binaries (torch_cuda/torch_cpu/torch_python/c10/...)",
     lambda r: r.startswith("torch/lib/")),
    ("torch: Python + other files", lambda r: r.startswith(("torch/", "torch-", "torchgen/", "functorch/"))),
    ("cuBLAS 12 (nvidia-cublas-cu12, for CTranslate2)", lambda r: r.startswith(("nvidia/cublas/", "nvidia_cublas"))),
    ("NVRTC 12 (nvidia-cuda-nvrtc-cu12)", lambda r: r.startswith(("nvidia/cuda_nvrtc/", "nvidia_cuda_nvrtc"))),
    ("ctranslate2", lambda r: r.startswith(("ctranslate2/", "ctranslate2-"))),
    ("faster_whisper", lambda r: r.startswith(("faster_whisper/", "faster_whisper-"))),
    ("onnxruntime", lambda r: r.startswith(("onnxruntime/", "onnxruntime-"))),
    ("transformers", lambda r: r.startswith(("transformers/", "transformers-"))),
    ("tokenizers + safetensors + hf_xet + huggingface_hub",
     lambda r: r.startswith(("tokenizers", "safetensors", "hf_xet", "huggingface_hub"))),
    ("scipy (+ scipy.libs)", lambda r: r.startswith(("scipy/", "scipy.libs/", "scipy-"))),
    ("numpy (+ numpy.libs)", lambda r: r.startswith(("numpy/", "numpy.libs/", "numpy-"))),
    ("llvmlite + numba", lambda r: r.startswith(("llvmlite", "numba"))),
    ("av (PyAV + FFmpeg)", lambda r: r.startswith(("av/", "av.libs/", "av-"))),
    ("scikit-learn", lambda r: r.startswith(("sklearn/", "scikit_learn"))),
    ("sympy + mpmath", lambda r: r.startswith(("sympy", "mpmath"))),
    ("librosa + soundfile + soxr + audio helpers",
     lambda r: r.startswith(("librosa", "soundfile", "_soundfile", "soxr", "lameenc", "sphn", "julius", "demucs"))),
    ("jukeoke_studio", lambda r: r.startswith(("jukeoke_studio",))),
]


def components(py):
    py = Path(py)
    sp = py / "Lib" / "site-packages"
    tot = {}

    def add(label, s):
        t = tot.setdefault(label, {"files": 0, "bytes": 0, "alloc_32k": 0, "alloc_128k": 0})
        t["files"] += 1
        t["bytes"] += s
        t["alloc_32k"] += -(-s // 32768) * 32768
        t["alloc_128k"] += -(-s // 131072) * 131072

    for dp, dns, fns in walk(py):
        for f in fns:
            p = os.path.join(dp, f)
            s = os.lstat(p).st_size
            rel_py = os.path.relpath(p, py).replace("\\", "/")
            if not rel_py.lower().startswith("lib/site-packages/"):
                label = ("Microsoft Visual C++ runtime (interpreter folder)" if VC_RE.match(f)
                         else "CPython interpreter + standard library")
                add(label, s)
                continue
            r = rel_py[len("lib/site-packages/"):]
            rl = r.lower()
            for label, pred in COMPONENTS:
                if pred(rl):
                    add(label, s)
                    break
            else:
                add("other site-packages", s)
    return dict(sorted(tot.items(), key=lambda kv: -kv[1]["bytes"]))


# Bundled third-party binaries that a wheel carries under its own name: the component
# and the licence of that component (from the component's identity; the owning wheel's
# licence files are listed next to it in runtime-binaries.tsv).
BIN_COMPONENTS = [
    (r"^(cublas|cublaslt)64_\d+\.dll$", "NVIDIA cuBLAS", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cufftw?64_", "NVIDIA cuFFT", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cusparse64_", "NVIDIA cuSPARSE", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cusolver(mg)?64_", "NVIDIA cuSOLVER", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^curand64_", "NVIDIA cuRAND", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^nvrtc", "NVIDIA NVRTC", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^nvjitlink", "NVIDIA nvJitLink", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cudart64_", "NVIDIA CUDA runtime", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cupti64_|^nvperf_", "NVIDIA CUPTI", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^nvtoolsext64_", "NVIDIA NVTX", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^nvblas64_", "NVIDIA NVBLAS", "NVIDIA CUDA Toolkit EULA (redistributable runtime)"),
    (r"^cudnn", "NVIDIA cuDNN 9", "NVIDIA cuDNN Software License Agreement (redistributable)"),
    (r"^libiomp5md|^libiompstubs5md", "Intel OpenMP runtime", "Intel Simplified Software License"),
    (r"^(msvcp140|vcruntime140|vcomp140|concrt140)", "Microsoft Visual C++ runtime",
     "Microsoft Visual C++ Redistributable (Visual Studio 'Distributable Code' terms)"),
    (r"^libscipy_openblas|^libopenblas|^openblas", "OpenBLAS", "BSD-3-Clause"),
    (r"^libgfortran|^libquadmath|^libgcc_s|^libwinpthread", "GCC / MinGW runtime",
     "GPL-3.0-or-later WITH GCC-exception-3.1 (libwinpthread: MIT/BSD)"),
    # PyAV 19.0.2's FFmpeg: its own av_license() string and configure line (both embedded in
    # avcodec/avutil) say "LGPL version 3 or later", --enable-version3, no --enable-gpl; it
    # links LAME statically and the separate GPL-2.0-or-later libx264/libx265 DLLs, so the
    # bundle as a whole is distributed under GPL terms (compatible with GPL-3.0-or-later).
    (r"^(avcodec|avformat|avutil|avdevice|avfilter|swresample|swscale|postproc)-", "FFmpeg (PyAV wheel build, libavcodec 63)",
     "LGPL-3.0-or-later (av_license(); --enable-version3; LAME linked statically: LGPL-2.0-or-later)"),
    (r"^libx264", "x264 (bundled with PyAV)", "GPL-2.0-or-later"),
    (r"^libx265", "x265 (bundled with PyAV)", "GPL-2.0-or-later"),
    (r"^libsvtav1", "SVT-AV1 encoder (bundled with PyAV)", "BSD-3-Clause-Clear (+ AOMedia patent licence)"),
    (r"^libdav1d", "dav1d AV1 decoder (bundled with PyAV)", "BSD-2-Clause"),
    (r"^libopus", "Opus (bundled with PyAV)", "BSD-3-Clause"),
    (r"^libvpx", "libvpx (bundled with PyAV)", "BSD-3-Clause"),
    (r"^libwebp|^libsharpyuv", "libwebp (bundled with PyAV)", "BSD-3-Clause"),
    (r"^libvmaf", "libvmaf (bundled with PyAV)", "BSD-2-Clause-Patent"),
    (r"^libvpl", "Intel oneVPL dispatcher (bundled with PyAV)", "MIT"),
    (r"^libiconv", "GNU libiconv (bundled with PyAV)", "LGPL-2.1-or-later"),
    (r"^libstdc\+\+", "GCC libstdc++ (bundled with PyAV)", "GPL-3.0-or-later WITH GCC-exception-3.1"),
    (r"^libsndfile|^sndfile", "libsndfile (+ FLAC, Ogg, Vorbis, Opus, mpg123, LAME)",
     "LGPL-2.1-or-later (codecs: BSD-3-Clause / LGPL)"),
    (r"^libcrypto|^libssl", "OpenSSL 3", "Apache-2.0"),
    (r"^libffi", "libffi", "MIT"),
    (r"^sqlite3", "SQLite", "blessing (public domain)"),
    (r"^zlib", "zlib", "Zlib"),
    (r"^python3?\d*\.dll$|^pythonw?\.exe$", "CPython", "PSF-2.0"),
    (r"^uv\.dll$", "libuv", "MIT"),
    (r"^ctranslate2\.dll", "CTranslate2", "MIT"),
    (r"^onnxruntime", "ONNX Runtime", "MIT"),
    (r"^(torch_|c10|shm\.dll|caffe2_nvrtc|fbgemm|asmjit)", "PyTorch", "BSD-3-Clause"),
    (r"^zlibwapi", "zlib (WINAPI build)", "Zlib"),
]


def binaries(py, sp_dists):
    py = Path(py)
    owner = {}
    for d in sp_dists:
        for f in d["files"]:
            owner[os.path.normpath(os.path.join("Lib/site-packages", f)).replace("\\", "/").lower()] = d
    rows = []
    for p in sorted(pe_files(py)):
        rel = os.path.relpath(p, py).replace("\\", "/")
        d = owner.get(os.path.normpath(rel).replace("\\", "/").lower())
        base = re.sub(r"-[0-9a-f]{8,}(?=\.(dll|pyd)$)", "", p.name.lower())
        comp = lic = ""
        for rx, c, l in BIN_COMPONENTS:
            if re.search(rx, base):
                comp, lic = c, l
                break
        if d is None:
            dist, dlic, lfiles = "CPython (python-build-standalone)", "PSF-2.0 (+ bundled libs, see LICENSE.txt)", "LICENSE.txt"
            if VC_RE.match(p.name):
                dist, dlic, lfiles = "(copied from the Visual C++ redistributable)", "", ""
        else:
            dist, dlic = f"{d['name']} {d['version']}", d["licence"]
            lfiles = " ".join(d["licence_files"][:4])
        rows.append((rel, str(p.stat().st_size), dist, dlic, comp or "(package's own code)", lic or dlic, lfiles))
    return rows


def main():
    cmd = sys.argv[1]
    if cmd == "measure":
        print(json.dumps(measure(sys.argv[2])))
    elif cmd == "strip":
        r = strip(sys.argv[2], sys.argv[3])
        r["items_count"] = len(r["items"])
        if len(r["items"]) > 60:   # keep the record readable
            r["items"] = r["items"][:60] + [f"... {len(r['items']) - 60} more"]
        print(json.dumps(r))
    elif cmd == "vcredist":
        print(json.dumps(vcredist(sys.argv[2], sys.argv[3])))
    elif cmd == "closure":
        res = closure(sys.argv[2])
        print(json.dumps({k: (sorted(v) if isinstance(v, set) else v) for k, v in res.items()}, indent=1))
    elif cmd == "fix-metadata":
        print(json.dumps(fix_metadata(sys.argv[2])))
    elif cmd == "finalize":
        py, studio, steps_file, extra_file = map(Path, sys.argv[2:6])
        steps = json.loads(steps_file.read_text(encoding="utf-8"))
        extra = json.loads(extra_file.read_text(encoding="utf-8"))
        sp = py / "Lib" / "site-packages"
        ds = dists(sp)
        comps = components(py)
        rows = binaries(py, ds)
        with open(studio / "runtime-binaries.tsv", "w", encoding="utf-8", newline="\n") as f:
            f.write("path\tbytes\tinstalled_by\tpackage_licence\tcomponent\tcomponent_licence\tlicence_files\n")
            for r in rows:
                f.write("\t".join(r) + "\n")
        build = (py / "BUILD").read_text(encoding="utf-8").strip() if (py / "BUILD").exists() else ""
        runtime = {
            "format": 1,
            "built_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "python": sys.version.split()[0],
            "python_build_standalone": build,
            "launch": "python/python.exe -I -X utf8 -m jukeoke_studio serve",
            **extra,
            "packages": [{k: d[k] for k in ("name", "version", "licence")} for d in ds],
            "size": steps[-1]["after"] if steps else None,
            "components": comps,
            "binaries": len(rows),
        }
        (studio / "runtime.json").write_text(json.dumps(runtime, indent=1), encoding="utf-8")
        (studio / "runtime-build.json").write_text(json.dumps({"steps": steps, "components": comps}, indent=1),
                                                   encoding="utf-8")
        print(json.dumps({"components": comps}, indent=1))
    else:
        raise SystemExit(f"unknown command {cmd}")


if __name__ == "__main__":
    main()
PYEOF

RT() { "$HPY" -I -B "$(cygpath -w "$WORK/rt.py")" "$@"; }
STEPS="$WORK/steps.jsonl"
: > "$STEPS"
now() { date +%s; }
# step <name> <seconds> [extra-json]: measure the tree and record the step.
LAST=""
step() {
    local name="$1" secs="$2" extra="${3:-null}" m
    m="$(RT measure "$PYW")"
    printf '{"step":"%s","seconds":%s,"before":%s,"after":%s,"detail":%s}\n' \
        "$name" "$secs" "${LAST:-null}" "$m" "$extra" >> "$STEPS"
    LAST="$m"
    "$HPY" -I -B -c "import json,sys; m=json.loads(sys.argv[1]); print(f\"  [{sys.argv[2]:<22}] {m['bytes']/1e9:6.3f} GB  {m['files']:6d} files  {m['dirs']:5d} dirs  on exFAT: {m['alloc_32k']/1e9:6.3f} GB (32 KB clusters) / {m['alloc_128k']/1e9:6.3f} GB (128 KB)\")" "$m" "$name"
}

echo "== 1/7 interpreter: $(cygpath -w "$PBS_DIR")"
t=$(now)
cp -a "$PBS_DIR" "$PY"
# The tree's EXTERNALLY-MANAGED marker protects a shared system interpreter; this copy is
# private to the stick, and installing into it is the point.
rm -f "$PY/Lib/EXTERNALLY-MANAGED"
step "copy-interpreter" $(( $(now) - t ))

echo "== 2/7 locked dependencies (studio/uv.lock -> pylock.toml, exact URLs + SHA-256)"
t=$(now)
# --no-header: uv's default header comment repeats the export command line, including the
# absolute --output-file path (the build machine's user folder), and this file ships on the
# stick. Verified 2026-11-17: the output is otherwise identical.
( cd "$REPO/studio" && uv export --frozen --no-dev --no-emit-project --format pylock.toml --preview-features pylock \
      --no-header --output-file "$(cygpath -w "$WORK/pylock.toml")" >/dev/null )
cp "$WORK/pylock.toml" "$STUDIO/requirements.pylock.toml"
UV_LINK_MODE=copy uv pip install --python "$(cygpath -w "$PYEXE")" --link-mode copy \
    --no-deps --preview-features pylock -r "$(cygpath -w "$WORK/pylock.toml")"
step "install-locked-deps" $(( $(now) - t ))

echo "== 3/7 jukeoke_studio (non-editable, from a snapshot of studio/)"
t=$(now)
mkdir -p "$WORK/studio-src"
cp "$REPO/studio/pyproject.toml" "$REPO/studio/README.md" "$WORK/studio-src/"
( cd "$REPO/studio" && find jukeoke_studio -name __pycache__ -prune -o -type f -name '*.py' -print \
      | tar -cf - -T - ) | ( cd "$WORK/studio-src" && tar -xf - )
# Any non-Python package data shipped later must be added above (the package has none yet).
TREE_SHA="$(cd "$WORK/studio-src" && find jukeoke_studio -type f | LC_ALL=C sort | xargs sha256sum | sha256sum | cut -d' ' -f1)"
UV_LINK_MODE=copy uv pip install --python "$(cygpath -w "$PYEXE")" --link-mode copy \
    --no-deps --no-build-isolation --reinstall-package jukeoke-studio "$(cygpath -w "$WORK/studio-src")"
step "install-jukeoke-studio" $(( $(now) - t ))

echo "== 4/7 strip"
STRIPS="caches tests devfiles gui"
[ "$STRIP_TRANSFORMERS" = 1 ] && STRIPS="$STRIPS transformers"
for s in $STRIPS; do
    t=$(now)
    d="$(RT strip "$PYW" "$s")"
    step "strip-$s" $(( $(now) - t )) "$d"
done
if [ "$STRIP_CUDA" = 1 ]; then
    t=$(now)
    d="$(RT strip "$PYW" cuda)"
    step "strip-cuda" $(( $(now) - t )) "$d"
fi

echo "== 5/7 Microsoft Visual C++ runtime (from $(cygpath -w "$VCREDIST_DIR"))"
t=$(now)
VC="$(RT vcredist "$PYW" "$(cygpath -w "$VCREDIST_DIR")")"
echo "$VC" | "$HPY" -I -B -c "import json,sys; d=json.load(sys.stdin); print('  copied:', d['copied'], ' replaced:', d['replaced']); print('  versions:', d['versions']); [print('  UNRESOLVED static import:', k, v[:3]) for k,v in d['unresolved_static'].items()]; [print('  unresolved delay-load import (only fails if called):', k, v[:3]) for k,v in d['unresolved_delay'].items()]; [print('  imported from System32 but not part of Windows:', k, v[:3]) for k,v in d['system32_other'].items()]; [print('  optional, allowed to be missing:', k, v['imported_by'][:3], '-', v['why_allowed']) for k,v in d['optional_missing'].items()]"
step "bundle-vc-runtime" $(( $(now) - t )) "$VC"
if echo "$VC" | "$HPY" -I -B -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if (d['unresolved_static'] or d['vc_still_needed'] or d['system32_other']) else 1)"; then
    echo "a binary imports a DLL that is neither in the runtime nor part of Windows (see above)" >&2
    exit 1
fi

echo "== 6/7 byte-compile (unchecked-hash .pyc, build path stripped from co_filename)"
t=$(now)
set +e
# -B: compileall's own process and its -j workers import modules from this very tree
# (concurrent.futures, ...); without -B the import system writes timestamp .pyc files for
# those at the same moment as the workers write theirs, and on Windows the two renames
# collide ("Access is denied", seen once in five builds). py_compile ignores -B, so every
# file is still compiled, as unchecked-hash; the check verifies no other kind exists.
"$PYEXE" -I -B -m compileall -q -f -j 0 --invalidation-mode unchecked-hash -s "$PYW" "$PYW\\Lib" \
    > "$WORK/compileall.log" 2>&1
rc=$?
set -e
NFAIL=$(grep -c '^\*\*\* Error compiling' "$WORK/compileall.log" || true)
FAILS="$({ grep '^\*\*\* Error compiling' "$WORK/compileall.log" || true; } | sed -e "s/.*compiling '//" -e "s/'.*//" \
          | "$HPY" -I -B -c "import json,sys,os; b=sys.argv[1]; print(json.dumps([os.path.relpath(l.strip(), b).replace(chr(92),'/') for l in sys.stdin if l.strip()]))" "$PYW")"
echo "  compileall exit $rc, $NFAIL file(s) not compilable (kept as source, never imported): $FAILS"
[ "$rc" -eq 0 ] || head -c 3000 "$WORK/compileall.log"
step "byte-compile" $(( $(now) - t )) "{\"exit\":$rc,\"not_compilable\":$FAILS}"

echo "== 7/7 metadata, runtime.json, binaries list"
FIXED="$(RT fix-metadata "$PYW")"
LOCK_SHA="$(sha256sum "$REPO/studio/uv.lock" | cut -d' ' -f1)"
GIT_COMMIT="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
GIT_DIRTY="$(git -C "$REPO" status --porcelain -- studio 2>/dev/null | wc -l | tr -d ' ')"
cat > "$WORK/extra.json" <<EOF
{"lock": {"file": "studio/uv.lock", "sha256": "$LOCK_SHA", "installed_from": "requirements.pylock.toml"},
 "jukeoke_studio": {"git_commit": "$GIT_COMMIT", "uncommitted_files_under_studio": $GIT_DIRTY,
                    "package_tree_sha256": "$TREE_SHA"},
 "cuda_stripped": $( [ "$STRIP_CUDA" = 1 ] && echo true || echo false ),
 "transformers_models_stripped": $( [ "$STRIP_TRANSFORMERS" = 1 ] && echo true || echo false ),
 "metadata_fixed": $FIXED,
 "vc_runtime": $VC,
 "bytecode": {"invalidation_mode": "unchecked-hash", "not_compilable": $FAILS}}
EOF
"$HPY" -I -B -c "import json,sys; print(json.dumps([json.loads(l) for l in open(sys.argv[1],encoding='utf-8') if l.strip()]))" \
    "$(cygpath -w "$STEPS")" > "$WORK/steps.json"
RT finalize "$PYW" "$(cygpath -w "$STUDIO")" "$(cygpath -w "$WORK/steps.json")" "$(cygpath -w "$WORK/extra.json")" \
    | "$HPY" -I -B -c "import json,sys; d=json.load(sys.stdin)['components']; [print(f'  {v[\"bytes\"]/1e6:9.1f} MB {v[\"files\"]:6d} files  {k}') for k,v in d.items()]"
rm -f "$STUDIO/runtime-build.partial"

"$PYEXE" -I -c "import jukeoke_studio, sys; print('  smoke: jukeoke_studio', jukeoke_studio.__version__, 'on Python', sys.version.split()[0])"
echo "built: $(cygpath -w "$STUDIO")"

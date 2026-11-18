#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# check-studio-runtime.sh - prove a built Studio runtime is relocatable and complete.
#
#   scripts/check-studio-runtime.sh <out_root> [options]
#
#   <out_root>             stick root holding Windows/Studio/python (from build-studio-runtime.sh)
#   --models DIR           models folder (default <out_root>/Models)
#   --separate AUDIO       also run `python -I -X utf8 -m jukeoke_studio separate AUDIO ... --preset full`
#   --pipeline AUDIO       also run the whole pipeline in one process (decode, separate,
#                          transcribe, align, analyse, write .jksong) and record hashes
#   --redirect-caches      point HF_HOME, TORCH_HOME, XDG_CACHE_HOME, CUDA_CACHE_PATH,
#                          NUMBA_CACHE_DIR, MPLCONFIGDIR, TMP/TEMP ... into <out_root>/Settings
#                          in the harness. Only for package builds without jukeoke_studio/env.py:
#                          when env.py exists, every check process calls env.setup() first, as
#                          the real entry points do, so the redirection under test is the code's
#   --snapshot-user-dirs   list %USERPROFILE%\.cache, %APPDATA%, %LOCALAPPDATA% (incl. %TEMP%)
#                          before and after, and report every new or changed entry (other
#                          programs write there too; entries are classified, see the JSON)
#   --sentinel-profile DIR give the check processes USERPROFILE, APPDATA, LOCALAPPDATA and
#                          TEMP/TMP inside DIR (empty folders outside the stick) and fail if
#                          anything appears there: exact attribution, no other program writes
#                          there (misses only writes through the shell's known-folder API,
#                          which --snapshot-user-dirs covers)
#   --skip-scan            skip the byte scan for build-machine paths (reads every file)
#   --static-only          only the file checks (paths, names, links, DLL imports); no Python runs
#   --out DIR              results (default <out_root>/Settings/tmp/runtime-check)
#   --forbid PATH          another absolute path that must not appear anywhere (repeatable)
#
# Checks (docs/STUDIO_RUNTIME.md): no build-machine path in any file (text metadata, .pyc,
# launchers, binaries; UTF-8 and UTF-16), no venv / ._pth / launchers, no symlinks, exFAT-safe
# names, MAX_PATH budget, every imported DLL resolvable inside the runtime or from Windows /
# the NVIDIA driver, then `python -I` child processes (clean environment, PATH = Windows
# folders only) for: the interpreter's own paths, torch (CUDA matmul, cuFFT STFT, cuDNN conv),
# transformers (wav2vec2 + HuBERT CTC forward on the GPU through the Auto classes, every
# registered alignment model's family importable), ctranslate2 / faster_whisper (CUDA 12 cuBLAS
# SGEMM on the GPU, and a real Whisper run when the model is present), librosa (key + tempo,
# numba JIT), av (bit-exact FLAC decode), soundfile (24-bit FLAC round trip), and the Studio
# itself (protocol hello + probe, or `probe`). Every module those processes loaded must come
# from the runtime, from Windows or from the NVIDIA driver. Nothing may be written into the
# runtime while it runs. Exit 0 when everything passes.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

[ $# -ge 1 ] || { sed -n '4,30p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
ROOT_U="$1"; shift
[ -d "$ROOT_U" ] || { echo "no such folder: $ROOT_U" >&2; exit 2; }
ROOT_W="$(cygpath -w "$(cd "$ROOT_U" && pwd)")"
PYEXE="$ROOT_U/Windows/Studio/python/python.exe"
[ -x "$PYEXE" ] || { echo "no Studio runtime at $ROOT_W\\Windows\\Studio\\python" >&2; exit 2; }

ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --models|--separate|--pipeline|--out|--forbid|--sentinel-profile)
            ARGS+=("$1" "$(cygpath -w "$2")"); shift ;;
        --redirect-caches|--snapshot-user-dirs|--skip-scan|--static-only) ARGS+=("$1") ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
# Paths of the build machine that must never appear inside the runtime.
FORBID=(--forbid "$(cygpath -w "$REPO")")
JKB="${JK_BUILD:-$REPO/../jukeoke-build}"
[ -d "$JKB" ] && FORBID+=(--forbid "$(cygpath -w "$(cd "$JKB" && pwd)")")
[ -n "${USERPROFILE:-}" ] && FORBID+=(--forbid "$USERPROFILE")
# Hash of studio/jukeoke_studio/*.py in this checkout, computed exactly like
# build-studio-runtime.sh step 3, so the check can warn when the runtime is stale.
TREE_ARGS=()
STUDIO_TREE="$(cd "$REPO/studio" 2>/dev/null && find jukeoke_studio -name __pycache__ -prune -o -type f -name '*.py' -print \
    | LC_ALL=C sort | xargs sha256sum | sha256sum | cut -d' ' -f1)" || STUDIO_TREE=""
[ -n "$STUDIO_TREE" ] && TREE_ARGS=(--expect-studio-tree "$STUDIO_TREE")

OUT_DEFAULT="$ROOT_U/Settings/tmp/runtime-check"
mkdir -p "$OUT_DEFAULT"
CHECK_PY="$OUT_DEFAULT/check_runtime.py"
cat > "$CHECK_PY" <<'PYEOF'
# SPDX-License-Identifier: GPL-3.0-or-later
"""Orchestrator for scripts/check-studio-runtime.sh. Standard library only; it runs with the
runtime's interpreter (-I -B) and starts every real check as a separate `python -I` child."""
import argparse
import ctypes
import hashlib
import json
import mmap
import os
import queue
import re
import struct
import subprocess
import sys
import threading
import time
from ctypes import wintypes
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("--models")
ap.add_argument("--separate")
ap.add_argument("--pipeline")
ap.add_argument("--out")
ap.add_argument("--forbid", action="append", default=[])
ap.add_argument("--redirect-caches", action="store_true")
ap.add_argument("--snapshot-user-dirs", action="store_true")
ap.add_argument("--sentinel-profile")
ap.add_argument("--skip-scan", action="store_true")
ap.add_argument("--static-only", action="store_true")
ap.add_argument("--expect-studio-tree", default=None)
A = ap.parse_args()

# os.path.abspath, never Path.resolve(): resolve() turns a subst / mapped drive letter back
# into the underlying path, which would hide exactly what this check is about.
ROOT = Path(os.path.abspath(A.root))
STUDIO = ROOT / "Windows" / "Studio"
PY = STUDIO / "python"
PYEXE = PY / "python.exe"
MODELS = Path(os.path.abspath(A.models)) if A.models else ROOT / "Models"
OUT = Path(os.path.abspath(A.out)) if A.out else ROOT / "Settings" / "tmp" / "runtime-check"
OUT.mkdir(parents=True, exist_ok=True)
SYSROOT = Path(os.environ.get("SystemRoot", r"C:\Windows"))

RESULTS = []


def record(name, status, summary="", **detail):
    """status: True (pass), False (fail), None (warning / information)."""
    RESULTS.append({"check": name, "status": {True: "pass", False: "fail", None: "warn"}[status],
                    "summary": summary, **detail})
    tag = {True: "PASS", False: "FAIL", None: "WARN"}[status]
    print(f"{tag}  {name}: {summary}", flush=True)


def inside(p, root):
    p = os.path.normcase(os.path.abspath(str(p)))
    r = os.path.normcase(os.path.abspath(str(root)))
    return p == r or p.startswith(r.rstrip("\\") + "\\")


# --------------------------------------------------------------------- environment
CACHE_VARS = {  # mirrors studio/jukeoke_studio/env.py
    "HF_HOME": "huggingface", "TORCH_HOME": "torch", "XDG_CACHE_HOME": "xdg",
    "CUDA_CACHE_PATH": "cuda", "NUMBA_CACHE_DIR": "numba", "MPLCONFIGDIR": "matplotlib",
    "PYTORCH_KERNEL_CACHE_PATH": "torch/kernels", "TRITON_CACHE_DIR": "triton",
    "TORCHINDUCTOR_CACHE_DIR": "inductor",
}
WIN_VARS = ("SystemRoot", "SYSTEMROOT", "windir", "SystemDrive", "COMSPEC", "PATHEXT", "OS",
            "PROCESSOR_ARCHITECTURE", "PROCESSOR_IDENTIFIER", "PROCESSOR_LEVEL", "PROCESSOR_REVISION",
            "NUMBER_OF_PROCESSORS", "USERPROFILE", "USERNAME", "USERDOMAIN", "HOMEDRIVE", "HOMEPATH",
            "APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "ProgramData", "ProgramFiles", "ProgramFiles(x86)",
            "ProgramW6432", "CommonProgramFiles", "CommonProgramFiles(x86)", "CommonProgramW6432",
            "ALLUSERSPROFILE", "PUBLIC", "COMPUTERNAME", "DriverData", "LOGONSERVER", "SESSIONNAME")


def child_env():
    """What a clean Windows 11 account gives a program, PATH reduced to Windows' own
    folders (so no CUDA toolkit, Python or VC runtime elsewhere on this PC can stand in for
    a missing file), plus the variables the app sets for the Studio."""
    e = {k: os.environ[k] for k in WIN_VARS if k in os.environ}
    sr = str(SYSROOT)
    e["PATH"] = ";".join([sr + r"\system32", sr, sr + r"\System32\Wbem",
                          sr + r"\System32\WindowsPowerShell\v1.0"])
    e["JUKEOKE_MODELS_DIR"] = str(MODELS)
    e["HF_HUB_OFFLINE"] = "1"
    e["TRANSFORMERS_OFFLINE"] = "1"
    e["HF_HUB_DISABLE_TELEMETRY"] = "1"
    if A.redirect_caches:
        cache = ROOT / "Settings" / "cache"
        tmp = ROOT / "Settings" / "tmp" / "studio"
        tmp.mkdir(parents=True, exist_ok=True)
        for k, sub in CACHE_VARS.items():
            e[k] = str(cache / sub)
        for k in ("TMP", "TEMP", "TMPDIR"):
            e[k] = str(tmp)
        e["HF_HUB_CACHE"] = str(MODELS / "hf")
        e["JUKEOKE_ROOT"] = str(ROOT)
    if SENTINEL:
        # A fresh, empty per-user profile outside the stick. Whatever a check process
        # writes through %USERPROFILE%, %APPDATA%, %LOCALAPPDATA% or %TEMP% lands here and
        # nowhere else, so every file found afterwards is the Studio's (other programs on
        # this PC keep writing to the real folders, which makes --snapshot-user-dirs noisy).
        prof = SENTINEL / "profile"
        dirs = {"USERPROFILE": prof, "APPDATA": prof / "AppData" / "Roaming",
                "LOCALAPPDATA": prof / "AppData" / "Local", "TEMP": prof / "AppData" / "Local" / "Temp",
                "TMP": prof / "AppData" / "Local" / "Temp"}
        for k, d in dirs.items():
            d.mkdir(parents=True, exist_ok=True)
            e[k] = str(d)
        for d in [prof, *prof.rglob("*")]:
            SENTINEL_DIRS.add(os.path.normcase(str(d)))
        e["HOMEDRIVE"], e["HOMEPATH"] = str(prof)[:2], str(prof)[2:]
    return e


SENTINEL = Path(os.path.abspath(A.sentinel_profile)) if A.sentinel_profile else None
SENTINEL_DIRS = set()   # the folders the harness itself created in the sentinel profile
if SENTINEL and (inside(SENTINEL, ROOT) or (SENTINEL.exists() and any(SENTINEL.iterdir()))):
    sys.exit(f"--sentinel-profile must be an empty or new folder outside the stick: {SENTINEL}")
ENV = child_env()

PRELUDE = r'''
import ctypes as _ct, json as _json, sys as _sys, os as _os, time as _time
from ctypes import wintypes as _wt
_RESULT_FD = _os.dup(1)   # the real stdout, even if a library later points fd 1 at stderr
# Start-up exactly like the Studio's own entry points (jukeoke_studio/__main__.py): env.setup()
# before numpy/torch/transformers/numba are imported, so the cache and temp redirection under
# test is the package's own. Package builds without env.py fall back to --redirect-caches.
try:
    from jukeoke_studio import env as _jenv
    _JENV = _jenv.setup()
except ImportError:
    _JENV = None
def _modules():
    k = _ct.WinDLL("kernel32", use_last_error=True)
    k.GetCurrentProcess.restype = _wt.HANDLE
    k.K32EnumProcessModulesEx.argtypes = [_wt.HANDLE, _ct.POINTER(_wt.HMODULE), _wt.DWORD, _ct.POINTER(_wt.DWORD), _wt.DWORD]
    k.K32GetModuleFileNameExW.argtypes = [_wt.HANDLE, _wt.HMODULE, _wt.LPWSTR, _wt.DWORD]
    h = k.GetCurrentProcess(); n = 4096
    while True:
        arr = (_wt.HMODULE * n)(); need = _wt.DWORD()
        if not k.K32EnumProcessModulesEx(h, arr, _ct.sizeof(arr), _ct.byref(need), 3):
            return []
        if need.value <= _ct.sizeof(arr):
            break
        n = need.value // _ct.sizeof(_wt.HMODULE) + 64
    buf = _ct.create_unicode_buffer(32768); out = []
    for i in range(need.value // _ct.sizeof(_wt.HMODULE)):
        if k.K32GetModuleFileNameExW(h, arr[i], buf, 32768):
            out.append(buf.value)
    return out
def _emit(d):
    d["modules"] = _modules()
    _os.write(_RESULT_FD, ("\n@@RESULT " + _json.dumps(d, default=str) + "\n").encode("utf-8"))
'''

MODULES = {}  # module path -> list of checks that loaded it


def run_child(name, code=None, args=(), module=None, timeout=900):
    cmd = [str(PYEXE), "-I", "-X", "utf8"]
    cmd += (["-m", module] if module else ["-c", PRELUDE + code]) + [str(x) for x in args]
    t = time.perf_counter()
    try:
        p = subprocess.run(cmd, cwd=str(STUDIO), env=ENV, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
        rc, so, se = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as e:
        rc, so, se = "timeout", (e.stdout or b"").decode("utf-8", "replace") if isinstance(e.stdout, bytes) else (e.stdout or ""), str(e)
    dt = time.perf_counter() - t
    (OUT / f"{name}.log").write_text(f"$ {' '.join(cmd[:4])} ...\nexit {rc}, {dt:.1f} s\n--- stdout\n{so}\n--- stderr\n{se}\n",
                                     encoding="utf-8")
    res = None
    for line in (so + "\n" + se).splitlines():
        if line.startswith("@@RESULT "):
            res = json.loads(line[9:])
    if res:
        for m in res.pop("modules", []):
            MODULES.setdefault(m, []).append(name)
    tail = "\n".join(se.strip().splitlines()[-6:])
    return rc, res, dt, tail


# --------------------------------------------------------------- PE / version info
def pe_imports(path):
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
            imagebase, ndd, dd = struct.unpack_from("<Q", mm, opt + 24)[0], struct.unpack_from("<I", mm, opt + 108)[0], opt + 112
        elif magic == 0x10B:
            imagebase, ndd, dd = struct.unpack_from("<I", mm, opt + 28)[0], struct.unpack_from("<I", mm, opt + 92)[0], opt + 96
        else:
            return None
        secs = [struct.unpack_from("<IIII", mm, opt + optsz + 40 * i + 8) for i in range(nsec)]

        def off(rva):
            for vsz, va, rsz, rptr in secs:
                if va <= rva < va + max(vsz, rsz) and rva - va < rsz:
                    return rptr + rva - va
            return None

        def cstr(rva):
            o = off(rva)
            if o is None:
                return None
            e = mm.find(b"\0", o, o + 512)
            return mm[o:e].decode("ascii", "replace") if e > o else None

        out = ([], [])
        for idx, size, name_at, kind in ((1, 20, 3, 0), (13, 32, 1, 1)):
            if idx >= ndd:
                continue
            rva = struct.unpack_from("<I", mm, dd + 8 * idx)[0]
            o = off(rva) if rva else None
            while o is not None and o + size <= len(mm):
                d = struct.unpack_from("<%dI" % (size // 4), mm, o)
                if not any(d):
                    break
                nr = d[name_at]
                if kind == 1 and not d[0] & 1:
                    nr -= imagebase
                n = cstr(nr)
                if n:
                    out[kind].append(n)
                o += size
        return out
    except struct.error:
        return None
    finally:
        mm.close()


_VER = None


def version_info(path):
    global _VER
    if _VER is None:
        v = ctypes.WinDLL("version", use_last_error=True)
        v.GetFileVersionInfoSizeW.argtypes = [ctypes.c_wchar_p, ctypes.c_void_p]
        v.GetFileVersionInfoSizeW.restype = ctypes.c_uint32
        v.GetFileVersionInfoW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
        v.VerQueryValueW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p),
                                     ctypes.POINTER(ctypes.c_uint)]
        _VER = v
    v = _VER
    n = v.GetFileVersionInfoSizeW(str(path), None)
    if not n:
        return None, None
    buf = ctypes.create_string_buffer(n)
    if not v.GetFileVersionInfoW(str(path), 0, n, buf):
        return None, None
    p, ln = ctypes.c_void_p(), ctypes.c_uint()
    langs = []
    if v.VerQueryValueW(buf, "\\VarFileInfo\\Translation", ctypes.byref(p), ctypes.byref(ln)) and ln.value >= 4:
        langs.append(struct.unpack("<HH", ctypes.string_at(p.value, 4)))
    langs += [(0x0409, 0x04B0), (0x0409, 0x04E4)]
    got = {}
    for key in ("ProductName", "CompanyName"):
        for lang, cp in langs:
            if v.VerQueryValueW(buf, "\\StringFileInfo\\%04x%04x\\%s" % (lang, cp, key),
                                ctypes.byref(p), ctypes.byref(ln)) and ln.value:
                got[key] = ctypes.wstring_at(p.value, ln.value).rstrip("\0")
                break
    return got.get("ProductName"), got.get("CompanyName")


VC_RE = re.compile(r"^(msvcp140(_\w+)?|vcruntime140(_\w+)?|vcomp140|concrt140|vccorlib140|mfc140\w*)\.dll$", re.I)
REDIST_RE = re.compile(r"^(msvc[pr]\d+\w*|vcruntime\d+\w*|vcomp\d+|concrt\d+|vccorlib\d+|mfc\d+\w*|atl\d+|"
                       r"d3dx\w+|d3dcompiler_(?!47)\d+|xinput1_[123]|xaudio2_[0-7]|x3daudio\w+|ucrtbased)\.dll$", re.I)
OPTIONAL_MISSING = {"tbb12.dll": "numba's optional TBB threading layer (tbbpool); falls back to OpenMP/workqueue"}


def classify_windows_file(path):
    """os / driver / redist / other for a file outside the runtime."""
    prod, comp = version_info(path)
    prod, comp = prod or "", comp or ""
    name = Path(path).name
    if VC_RE.match(name) or REDIST_RE.match(name):
        return "redist", prod
    store = SYSROOT / "System32" / "DriverStore" / "FileRepository"
    in_nv_store = inside(path, store) and Path(path).relative_to(store).parts[0].lower().startswith("nv")
    if "NVIDIA" in prod or "NVIDIA" in comp or in_nv_store:   # some driver DLLs carry no version resource
        return "driver", prod or "NVIDIA display driver package"
    if comp.startswith("Microsoft"):   # System32, or e.g. Defender's platform folder
        return "os", prod
    return "other", f"{prod} / {comp}"


# ------------------------------------------------------------------ static checks
def path_regex(paths, utf16):
    alts = []
    for p in sorted({p.rstrip("\\/") for p in paths if p and len(p.rstrip("\\/")) >= 6}, key=len, reverse=True):
        parts = re.split(r"[\\/]+", p)
        if utf16:
            enc = [b"".join(re.escape(ch.encode("ascii", "ignore")) + b"\\x00" for ch in part) for part in parts]
            sep = rb"(?:\\\x00\\\x00|\\\x00|/\x00)"
        else:
            enc = [re.escape(part.encode("utf-8")) for part in parts]
            sep = rb"(?:\\\\|\\|/)"
        alts.append(sep.join(enc))
    return re.compile(b"|".join(alts), re.I) if alts else None


META_NAMES = {"record", "installer", "direct_url.json", "entry_points.txt", "pyvenv.cfg"}
META_SUFFIX = (".pth", "._pth", ".cfg", ".json", ".toml", ".ini")
CRITICAL_META = (".pth", "._pth", ".cfg")
DRIVE_RE = re.compile(rb"(?<![A-Za-z0-9])[A-Za-z]:(?:\\\\|\\|/)[A-Za-z0-9_ .\-\\/]{2,}|\\\\[A-Za-z0-9_.\-]{2,}\\")
BAD_NAME = re.compile(r'[\x00-\x1f"*/:<>?\\|]')
LAUNCHER8 = re.compile(rb'[A-Za-z]:[\\/][^\x00\r\n"<>|*?]{0,400}?pythonw?\.exe', re.I)
LAUNCHER16 = re.compile(rb'[A-Za-z]\x00:\x00[\\/]\x00(?:[^\x00\r\n]\x00){0,400}?'
                        rb'p\x00y\x00t\x00h\x00o\x00n\x00(?:w\x00)?\.\x00e\x00x\x00e\x00', re.I)


def static_checks():
    t = time.perf_counter()
    forbid = list(A.forbid)
    rx8, rx16 = path_regex(forbid, False), path_regex(forbid, True)
    hits, meta_hits, crit_meta = [], [], []
    links, bad_names, launchers = [], [], []
    max_rel, max_rel_path, nbytes, nfiles = 0, "", 0, 0
    pe_list = []
    pyc_kinds, pyc_other = {}, []
    scan_roots = [PY] + ([MODELS] if MODELS.is_dir() else [])
    for top in scan_roots:
        for dp, dns, fns in os.walk(top):
            for d in list(dns):
                full = os.path.join(dp, d)
                if os.path.islink(full) or os.path.isjunction(full):
                    links.append(os.path.relpath(full, ROOT))
                    dns.remove(d)
            for n in dns + fns:
                if BAD_NAME.search(n) or n.endswith((" ", ".")):
                    bad_names.append(os.path.relpath(os.path.join(dp, n), ROOT))
            for f in fns:
                full = os.path.join(dp, f)
                rel = os.path.relpath(full, ROOT)
                if len(rel) > max_rel:
                    max_rel, max_rel_path = len(rel), rel
                if os.path.islink(full):
                    links.append(rel)
                    continue
                if top != PY:
                    continue
                low = f.lower()
                nfiles += 1
                if low.endswith((".dll", ".pyd", ".exe")):
                    pe_list.append(Path(full))
                if low.endswith(".pyc"):
                    # PEP 552 header: flags 1 = unchecked hash (what the build writes and what
                    # survives any copy); 0 = timestamp (written by an import at some point:
                    # Python would compare it with the source mtime, which a copy can change).
                    with open(full, "rb") as fh:
                        hdr = fh.read(8)
                    kind = {0: "timestamp", 1: "unchecked-hash", 3: "checked-hash"}.get(
                        struct.unpack("<I", hdr[4:8])[0] if len(hdr) == 8 else -1, "invalid")
                    pyc_kinds[kind] = pyc_kinds.get(kind, 0) + 1
                    if kind != "unchecked-hash" and len(pyc_other) < 20:
                        pyc_other.append(os.path.relpath(full, PY))
                size = os.path.getsize(full)
                if low.endswith(".exe") and low not in ("python.exe", "pythonw.exe"):
                    # a pip/uv console-script launcher carries an absolute python.exe path
                    # (uv: plain text before the appended zip; pip: a "#!" line)
                    with open(full, "rb") as fh:
                        blob = fh.read()
                    if LAUNCHER8.search(blob) or LAUNCHER16.search(blob):
                        launchers.append(os.path.relpath(full, PY))
                nbytes += size
                if size == 0:
                    continue
                if A.skip_scan and not (low in META_NAMES or low.endswith(META_SUFFIX)):
                    continue
                with open(full, "rb") as fh:
                    mm = mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ)
                    try:
                        for rx in (rx8, rx16):
                            m = rx.search(mm) if rx else None
                            if m:
                                hits.append({"file": os.path.relpath(full, PY), "offset": m.start(),
                                             "text": m.group(0)[:120].replace(b"\x00", b"").decode("utf-8", "replace")})
                                break
                        if low in META_NAMES or low.endswith(META_SUFFIX):
                            for m in DRIVE_RE.finditer(mm):
                                h = {"file": os.path.relpath(full, PY), "text": m.group(0)[:100].decode("utf-8", "replace")}
                                (crit_meta if (low.endswith(CRITICAL_META) or low in ("record", "direct_url.json"))
                                 else meta_hits).append(h)
                    finally:
                        mm.close()
    # The build's own metadata next to python/ (runtime.json, runtime-build.json,
    # runtime-binaries.tsv, requirements.pylock.toml) ships on the stick too. Verifier fix
    # 2026-11-17: the walk above covered python/ only, and requirements.pylock.toml carried
    # uv's header comment with the build machine's absolute --output-file path.
    for f in sorted(os.listdir(STUDIO)):
        full = os.path.join(STUDIO, f)
        if not os.path.isfile(full) or os.path.getsize(full) == 0:
            continue
        nfiles += 1
        nbytes += os.path.getsize(full)
        with open(full, "rb") as fh:
            blob = fh.read()
        for rx in (rx8, rx16):
            m = rx.search(blob) if rx else None
            if m:
                hits.append({"file": os.path.relpath(full, PY), "offset": m.start(),
                             "text": m.group(0)[:120].replace(b"\x00", b"").decode("utf-8", "replace")})
                break
    scanned = "names only (--skip-scan)" if A.skip_scan else f"{nfiles} files, {nbytes / 1e9:.2f} GB"
    record("every .pyc is unchecked-hash (immune to timestamp changes when the stick is copied)",
           bool(pyc_kinds) and set(pyc_kinds) == {"unchecked-hash"},
           f"{pyc_kinds}" + (f"; e.g. {pyc_other[:5]}" if pyc_other else ""), others=pyc_other)
    record("no build-machine path in any file", not hits,
           f"{len(hits)} file(s) contain one of {len(forbid)} forbidden prefixes ({scanned}, UTF-8 + UTF-16LE)",
           hits=hits[:50], forbidden=forbid)
    record("text metadata has no absolute paths (.pth/._pth/.cfg/RECORD/direct_url.json)", not crit_meta,
           f"{len(crit_meta)} hit(s) in files the interpreter or installers act on",
           hits=crit_meta[:50])
    if meta_hits:
        record("other .json/.toml/.ini mention a drive path", None,
               f"{len(meta_hits)} mention(s), none in a file Python acts on (listed in the JSON)",
               hits=meta_hits[:80])
    venv = [n for n in ("pyvenv.cfg", "python312._pth", "python._pth") if (PY / n).exists()]
    record("not a venv, no ._pth, no launchers", not venv and not launchers and not (PY / "Scripts").exists(),
           f"pyvenv/_pth: {venv or 'none'}; launcher exes: {launchers or 'none'}; Scripts/: "
           f"{'present' if (PY / 'Scripts').exists() else 'absent'}")
    record("no symlinks or junctions (exFAT has none)", not links, f"{len(links)} found", links=links[:20])
    record("all names valid on exFAT", not bad_names, f"{len(bad_names)} invalid", names=bad_names[:20])
    budget = 259 - max_rel - 1
    record("MAX_PATH budget", 3 + max_rel <= 259,
           f"longest path below the stick root is {max_rel} chars ({max_rel_path}); from a drive root "
           f"that is {3 + max_rel} <= 259; a copy into a folder works without Windows long-path "
           f"support while the folder's own path is at most {budget} chars",
           longest=max_rel_path, longest_len=max_rel, folder_budget=budget)
    record("static scan time", None, f"{time.perf_counter() - t:.1f} s")
    return pe_list


def import_closure(pe_list):
    by_dir = {}
    for p in pe_list:
        by_dir.setdefault(p.parent, set()).add(p.name.lower())
    sp = PY / "Lib" / "site-packages"
    gdirs = [sp / "torch" / "lib"] + sorted(sp.glob("nvidia/*/bin")) + sorted(sp.glob("*.libs")) + sorted(sp.glob("*/.libs"))
    gnames = set().union(*[by_dir.get(d, set()) for d in gdirs])
    root = by_dir.get(PY, set())
    sys32 = SYSROOT / "System32"
    cache, missing, delay_missing, redist, optional, driver = {}, {}, {}, {}, {}, set()
    for p in pe_list:
        imp = pe_imports(p)
        if not imp:
            continue
        own = by_dir.get(p.parent, set())
        for kind, names in enumerate(imp):
            for n in names:
                ln = n.lower()
                if ln.startswith(("api-ms-win-", "ext-ms-")) or ln in own or ln in root \
                        or (ln in gnames and not VC_RE.match(ln)):
                    continue
                if ln not in cache:
                    f = sys32 / n
                    cache[ln] = classify_windows_file(f)[0] if f.exists() else "missing"
                c = cache[ln]
                rel = str(p.relative_to(PY))
                if c == "os":
                    continue
                if c == "driver":
                    driver.add(ln)
                elif ln in OPTIONAL_MISSING:
                    optional.setdefault(ln, []).append(rel)
                elif c == "missing":
                    (delay_missing if kind else missing).setdefault(ln, []).append(rel)
                else:
                    redist.setdefault(ln, []).append(rel)
    record("every imported DLL is in the runtime, part of Windows, or the NVIDIA driver",
           not missing and not redist,
           f"{len(pe_list)} binaries; unresolved: {sorted(missing) or 'none'}; from a redistributable "
           f"installed on this PC: {sorted(redist) or 'none'}; driver: {sorted(driver) or 'none'}; optional "
           f"(allowed): {sorted(optional) or 'none'}; delay-load only: {sorted(delay_missing) or 'none'}",
           missing=missing, redist=redist, delay_missing=delay_missing, optional=optional)


# ------------------------------------------------------------------ snapshots
def snap_tree(roots, skip=()):
    out = {}
    skip = [os.path.normcase(str(s)) for s in skip]

    def rec(d):
        try:
            it = os.scandir(d)
        except OSError:
            return
        with it:
            for e in it:
                try:
                    if e.is_symlink() or e.is_junction():
                        continue
                    st = e.stat(follow_symlinks=False)
                    if e.is_dir(follow_symlinks=False):
                        if os.path.normcase(e.path) in skip:
                            continue
                        out[e.path + "\\"] = (0, st.st_mtime_ns)
                        rec(e.path)
                    else:
                        out[e.path] = (st.st_size, st.st_mtime_ns)
                except OSError:
                    continue
    for r in roots:
        if os.path.isdir(r):
            rec(r)
    return out


def diff(before, after):
    new = sorted(k for k in after if k not in before)
    changed = sorted(k for k in after if k in before and after[k] != before[k] and not k.endswith("\\"))
    gone = sorted(k for k in before if k not in after)
    return new, changed, gone


# The real per-user folders are shared with every other program on the PC (browsers, chat
# apps, OneDrive, and the other work streams' compilers and test runs on this build machine),
# so a before/after listing needs attribution. First match wins:
#   other  - known writers that are not the Studio (checked first: a pytest run of another
#            stream creates folders called "Models\hf" or "cache\cuda" under %TEMP%)
#   studio - what the Studio's libraries write when nothing redirects them
#   unattributed - everything else (reported, not failed)
OTHER_WRITERS = re.compile(r"(?i)\\temp\\pytest-of-|\\temp\\cc[a-z0-9]{6}$|\\temp\\claude\\|\\claude-data\\|"
                           r"\\anthropic\\|\\mozilla\\|\\google\\chrome\\|\\microsoft\\edge\\|\\discord\\|"
                           r"\\docker desktop\\|\\onedrive\\|\\appdata\\local\\packages\\|"
                           r"\\microsoft\\windows\\|\\temp\\diagoutputdir|\\code\\|\\slack\\|\\npm-cache\\|"
                           r"\\temp\\[0-9a-f-]{36}\.tmp$")
STUDIO_WRITERS = re.compile(r"(?i)\\nvidia\\computecache|\\nvidia\\dxcache|\\nvidia\\glcache|\\nv_cache|"
                            r"huggingface|\\torch\\|torch_extensions|\\numba|matplotlib|librosa|pooch|"
                            r"ctranslate2|\\python|\\pip\\|\\uv\\|triton|inductor|joblib|sklearn|onnxruntime|"
                            r"jukeoke|whisper|\\\.cache\\|__pycache__|\.nb[ic]$|\\temp\\tmp[a-z0-9_]{4,}")


def attribute(path):
    if OTHER_WRITERS.search(path):
        return "other"
    if STUDIO_WRITERS.search(path):
        return "studio"
    return "unattributed"


def user_roots():
    up = os.environ.get("USERPROFILE", "")
    roots = [os.path.join(up, ".cache"), os.environ.get("APPDATA", ""), os.environ.get("LOCALAPPDATA", "")]
    tmp = os.environ.get("TEMP", "")
    if tmp and not inside(tmp, roots[2]):
        roots.append(tmp)
    return [r for r in roots if r]


# ------------------------------------------------------------------ the children
CHECKS = {}

CHECKS["interpreter"] = r'''
import site, sysconfig
root = _os.path.normcase(_os.path.abspath(_sys.argv[1]))
def inside(p):
    p = _os.path.normcase(_os.path.abspath(p)); return p == root or p.startswith(root + _os.sep)
paths = {"executable": _sys.executable, "prefix": _sys.prefix, "base_prefix": _sys.base_prefix,
         "exec_prefix": _sys.exec_prefix, "base_exec_prefix": _sys.base_exec_prefix}
paths.update({"sysconfig." + k: v for k, v in sysconfig.get_paths().items()})
sp = [p for p in _sys.path if p]
outside = [f"{k}={v}" for k, v in paths.items() if not inside(v)] + [f"sys.path={p}" for p in sp if not inside(p)]
studio_env = None
if _JENV is not None:   # what jukeoke_studio.env.setup() decided (root, caches, temp)
    studio_env = {"root": str(_JENV.root), "models_dir": str(_JENV.models_dir), "cache_dir": str(_JENV.cache_dir),
                  "temp_dir": str(_JENV.temp_dir), "set_vars": _JENV.set_vars}
_emit({"ok": not outside and _sys.flags.isolated == 1 and not site.ENABLE_USER_SITE,
       "outside": outside, "isolated": _sys.flags.isolated, "user_site": site.ENABLE_USER_SITE,
       "sys_path": sp, "version": _sys.version.split()[0], "dont_write_bytecode": _sys.dont_write_bytecode,
       "studio_env": studio_env})
'''

CHECKS["torch"] = r'''
t0 = _time.perf_counter()
import torch, torch.nn.functional as F
t_import = _time.perf_counter() - t0
r = {"torch": torch.__version__, "cuda_build": torch.version.cuda, "cuda_available": torch.cuda.is_available(),
     "import_s": round(t_import, 2)}
if r["cuda_available"]:
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    dev = torch.device("cuda")
    r["gpu"] = torch.cuda.get_device_name(0)
    r["compute_capability"] = ".".join(map(str, torch.cuda.get_device_capability(0)))
    r["cudnn"] = torch.backends.cudnn.version()
    g = torch.Generator().manual_seed(0)
    a = torch.randn(1024, 1024, generator=g); b = torch.randn(1024, 1024, generator=g)
    ref = a.double() @ b.double()
    got = (a.to(dev) @ b.to(dev)).cpu().double()
    r["matmul_rel_err"] = float((got - ref).abs().max() / ref.abs().max())
    x = torch.randn(2, 44100, generator=g); w = torch.hann_window(2048)
    S = torch.stft(x.to(dev), 2048, 441, window=w.to(dev), return_complex=True).cpu().to(torch.complex128)
    Sr = torch.stft(x.double(), 2048, 441, window=w.double(), return_complex=True)
    r["stft_cufft_rel_err"] = float((S - Sr).abs().max() / Sr.abs().max())
    y = torch.randn(1, 1, 16000, generator=g); k = torch.randn(64, 1, 10, generator=g)
    o = F.conv1d(y.to(dev), k.to(dev), stride=5).cpu().double()
    orf = F.conv1d(y.double(), k.double(), stride=5)
    r["conv1d_cudnn_rel_err"] = float((o - orf).abs().max() / orf.abs().max())
    torch.cuda.synchronize()
    r["ok"] = max(r["matmul_rel_err"], r["stft_cufft_rel_err"], r["conv1d_cudnn_rel_err"]) < 1e-4
else:
    r["ok"] = False
r["seconds"] = round(_time.perf_counter() - t0, 2)
_emit(r)
'''

CHECKS["transformers"] = r'''
# Both CTC families the Studio can align with, built the way align.py loads them (Auto
# classes, so the model_type -> module lookup that a pruned transformers must still serve is
# exercised), small random weights, forward pass on the GPU. Then every alignment model in
# the Studio's registry: its family (config.json model_type of the pinned snapshot when it is
# on the stick, else the family named in the repository) must be importable.
t0 = _time.perf_counter()
import importlib, torch, transformers
from transformers import AutoConfig, AutoModelForCTC
shapes, ok = {}, True
for mt in ("wav2vec2", "hubert"):
    cfg = AutoConfig.for_model(mt, vocab_size=32, hidden_size=64, num_hidden_layers=2, num_attention_heads=2,
                               intermediate_size=128, conv_dim=(32,) * 7, num_conv_pos_embeddings=16,
                               num_conv_pos_embedding_groups=4)
    torch.manual_seed(0)
    m = AutoModelForCTC.from_config(cfg).eval().cuda()
    with torch.no_grad():
        out = m(torch.randn(1, 16000).cuda()).logits
    shapes[mt] = [type(m).__name__] + list(out.shape)
    ok = ok and bool(torch.isfinite(out).all()) and out.shape[-1] == 32
families = {}
try:
    from jukeoke_studio.modelzoo import hfmodels
    md = _os.environ.get("JUKEOKE_MODELS_DIR", "")
    for m in hfmodels.HF_MODELS.values():
        if m.task != "alignment":
            continue
        cj = _os.path.join(md, "hf", "models--" + m.repo.replace("/", "--"), "snapshots", m.revision, "config.json")
        mt = _json.load(open(cj, encoding="utf-8"))["model_type"] if _os.path.isfile(cj) else \
            m.repo.split("/")[-1].split("-")[0].lower()
        try:
            importlib.import_module("transformers.models." + mt)
            families[m.key] = mt
        except ImportError as e:
            families[m.key] = f"{mt}: NOT IMPORTABLE ({e})"
            ok = False
except ImportError:
    families = None
n_families = sum(1 for d in _os.listdir(_os.path.dirname(transformers.models.__file__))
                 if not d.startswith("_") and _os.path.isdir(_os.path.join(_os.path.dirname(transformers.models.__file__), d)))
_emit({"transformers": transformers.__version__, "ctc_models": shapes, "alignment_families": families,
       "model_families_present": n_families, "ok": ok, "seconds": round(_time.perf_counter() - t0, 2)})
'''

CHECKS["ctranslate2"] = r'''
t0 = _time.perf_counter()
import torch
from jukeoke_studio.transcribe import add_cuda12_dll_dirs
loaded = add_cuda12_dll_dirs()
import ctranslate2, faster_whisper
r = {"ctranslate2": ctranslate2.__version__, "faster_whisper": faster_whisper.__version__,
     "cublas12_loaded": [_os.path.basename(p) for p in loaded],
     "cuda_devices": ctranslate2.get_cuda_device_count(),
     "cuda_compute_types": sorted(ctranslate2.get_supported_compute_types("cuda"))}
# cuBLAS 12 SGEMM on the GPU through the DLL CTranslate2 uses, on torch-allocated memory
cb = _ct.WinDLL([p for p in loaded if p.lower().endswith("cublas64_12.dll")][0])
vp, i32 = _ct.c_void_p, _ct.c_int
cb.cublasCreate_v2.argtypes = [_ct.POINTER(vp)]
cb.cublasSgemm_v2.argtypes = [vp, i32, i32, i32, i32, i32, _ct.POINTER(_ct.c_float), vp, i32, vp, i32,
                              _ct.POINTER(_ct.c_float), vp, i32]
cb.cublasDestroy_v2.argtypes = [vp]
h = vp(); st = cb.cublasCreate_v2(_ct.byref(h))
g = torch.Generator().manual_seed(1)
a = torch.randn(256, 256, generator=g); b = torch.randn(256, 256, generator=g)
da, db, dc = a.cuda(), b.cuda(), torch.zeros(256, 256, device="cuda")
torch.cuda.synchronize()
one, zero = _ct.c_float(1.0), _ct.c_float(0.0)
# row-major C = A @ B is column-major C^T = B^T A^T
st2 = cb.cublasSgemm_v2(h, 0, 0, 256, 256, 256, _ct.byref(one), vp(db.data_ptr()), 256, vp(da.data_ptr()), 256,
                        _ct.byref(zero), vp(dc.data_ptr()), 256)
torch.cuda.synchronize()
ref = a.double() @ b.double()
r["cublas12_sgemm_status"] = [st, st2]
r["cublas12_sgemm_rel_err"] = float((dc.cpu().double() - ref).abs().max() / ref.abs().max())
cb.cublasDestroy_v2(h)
ok = st == 0 and st2 == 0 and r["cublas12_sgemm_rel_err"] < 1e-4 and r["cuda_devices"] > 0 and "float16" in r["cuda_compute_types"]
md = _os.environ.get("JUKEOKE_MODELS_DIR", "")
if _os.path.isdir(_os.path.join(md, "hf", "models--mobiuslabsgmbh--faster-whisper-large-v3-turbo")):
    import numpy as np
    from jukeoke_studio.transcribe import Transcriber
    t1 = _time.perf_counter()
    tr = Transcriber(models_dir=md)
    sr = 16000; tt = np.arange(sr * 3) / sr
    words, info = tr.transcribe_words((0.05 * np.sin(2 * np.pi * 220 * tt)).astype(np.float32), sr, "en")
    r["whisper"] = {"device": tr.device, "compute_type": tr.compute_type, "words": len(words),
                    "seconds": round(_time.perf_counter() - t1, 2)}
    ok = ok and tr.device == "cuda"
r["ok"] = ok
r["seconds"] = round(_time.perf_counter() - t0, 2)
_emit(r)
'''

CHECKS["librosa"] = r'''
t0 = _time.perf_counter()
import numpy as np, librosa
from jukeoke_studio.analyse import estimate_key, estimate_tempo
sr = 22050
def chord(fs, n):
    t = np.arange(n) / sr
    return sum(np.sin(2 * np.pi * f * t) for f in fs) * 0.2
# I - IV - V - I in C major, one second each
x = np.concatenate([chord(c, sr) for c in ((261.63, 329.63, 392.0), (349.23, 440.0, 523.25),
                                           (392.0, 493.88, 587.33), (261.63, 329.63, 392.0))]).astype(np.float32)
key = estimate_key(x, sr)
clicks = np.zeros(sr * 8, np.float32)
for i in range(0, len(clicks), sr // 2):            # 120 BPM click track
    clicks[i:i + 200] = np.hanning(200)
tempo = estimate_tempo(clicks, sr)
_emit({"librosa": librosa.__version__, "key": key, "tempo_bpm": tempo,
       "ok": bool(key) and key.get("tonic") == "C" and key.get("mode") == "major",
       "seconds": round(_time.perf_counter() - t0, 2)})
'''

CHECKS["av"] = r'''
t0 = _time.perf_counter()
import numpy as np, av, soundfile as sf
p = _os.path.join(_sys.argv[1], "av_check.flac")
rng = np.random.default_rng(0)
q = rng.integers(-2**23, 2**23, size=(44100, 2), dtype=np.int32)
sf.write(p, q << 8, 44100, subtype="PCM_24", format="FLAC")
parts = []
with av.open(p) as c:
    for f in c.decode(audio=0):
        a = f.to_ndarray()
        parts.append(a if f.format.is_planar else a.reshape(-1, len(f.layout.channels)).T)
dec = np.concatenate(parts, axis=1).T
exact = dec.shape == q.shape and bool(np.array_equal(dec.astype(np.int64) >> 8, q))
codecs = sorted(c for c in ("mp3", "aac", "flac", "vorbis", "opus", "libopus", "alac", "wmav2", "pcm_s24le",
                            "pcm_f32le", "h264") if c in av.codecs_available)
_os.remove(p)
_emit({"av": av.__version__, "libavcodec": ".".join(map(str, av.library_versions.get("libavcodec", ()))),
       "flac24_bit_exact": exact, "codecs": codecs,
       "ok": exact and {"mp3", "aac", "flac", "vorbis", "alac", "pcm_s24le"} <= set(codecs),
       "seconds": round(_time.perf_counter() - t0, 2)})
'''

CHECKS["soundfile"] = r'''
t0 = _time.perf_counter()
import numpy as np, soundfile as sf
p = _os.path.join(_sys.argv[1], "sf_check.flac")
q = np.random.default_rng(1).integers(-2**23, 2**23, size=(48000, 2), dtype=np.int32)
sf.write(p, q << 8, 48000, subtype="PCM_24", format="FLAC")
back, sr = sf.read(p, dtype="int32")
_os.remove(p)
exact = sr == 48000 and bool(np.array_equal(back >> 8, q))
_emit({"soundfile": sf.__version__, "libsndfile": sf.__libsndfile_version__, "flac24_roundtrip_exact": exact,
       "ok": exact, "seconds": round(_time.perf_counter() - t0, 2)})
'''

CHECKS["studio_import"] = r'''
t0 = _time.perf_counter()
import importlib, pkgutil, jukeoke_studio
names = sorted(m.name for m in pkgutil.walk_packages(jukeoke_studio.__path__, "jukeoke_studio.")
               if not m.name.endswith("__main__"))
bad = {}
for n in names:
    try:
        importlib.import_module(n)
    except Exception as e:
        bad[n] = repr(e)[:300]
import torch
from jukeoke_studio.modelzoo import MODELS
_emit({"version": jukeoke_studio.__version__, "modules": len(names), "import_errors": bad,
       "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
       "vram_mb": torch.cuda.get_device_properties(0).total_memory // 2**20 if torch.cuda.is_available() else None,
       "ok": not bad and torch.cuda.is_available(), "seconds": round(_time.perf_counter() - t0, 2)})
'''

CHECKS["stems_verify"] = r'''
import hashlib, json
import numpy as np, soundfile as sf
src, outdir = _sys.argv[1], _sys.argv[2]
mix, sr = sf.read(src, dtype="float32", always_2d=True)
stems = {}
for role in ("lead_vocal", "backing_vocals", "drums", "other"):
    x, sr2 = sf.read(_os.path.join(outdir, role + ".wav"), dtype="float32", always_2d=True)
    assert sr2 == sr and x.shape == mix.shape, (role, x.shape, mix.shape)
    stems[role] = x
total = sum(stems.values())
err = float(np.max(np.abs(total - mix)))
rms = {k: round(float(20 * np.log10(np.sqrt(np.mean(v.astype(np.float64) ** 2)) + 1e-12)), 2) for k, v in stems.items()}
sha = {k: hashlib.sha256(np.ascontiguousarray(v).tobytes()).hexdigest() for k, v in stems.items()}
info = json.load(open(_os.path.join(outdir, "separation.json"), encoding="utf-8"))
_emit({"sample_rate": sr, "frames": mix.shape[0], "sum_max_abs_err": err, "rms_dbfs": rms, "sha256": sha,
       "wall_seconds": info.get("wall_seconds"), "models": [m.get("id") or m.get("model") for m in info.get("models", [])],
       "ok": err < 1e-5})
'''

CHECKS["pipeline"] = r'''
import hashlib, dataclasses
import numpy as np
audio, out_dir, md = _sys.argv[1], _sys.argv[2], _sys.argv[3]
_os.makedirs(out_dir, exist_ok=True)
T = {}; t = _time.perf_counter()
def lap(k):
    global t
    n = _time.perf_counter(); T[k] = round(n - t, 2); t = n
import av, soundfile as sf
with av.open(audio) as c:
    s = c.streams.audio[0]; sr = s.rate; parts = []
    for f in c.decode(s):
        a = f.to_ndarray()
        parts.append(a if f.format.is_planar else a.reshape(-1, len(f.layout.channels)).T)
mix = np.ascontiguousarray(np.concatenate(parts, axis=1).astype(np.float32))
ref, sr2 = sf.read(audio, dtype="float32", always_2d=True)
decode_exact = sr2 == sr and bool(np.array_equal(ref.T, mix))
lap("decode")
import torch
from jukeoke_studio.separate import Separator
sep = Separator(md, "cuda")
res = sep.separate(mix, sr, preset="full")
torch.cuda.synchronize(); lap("separate")
H = lambda x: hashlib.sha256(np.ascontiguousarray(x, dtype="<f4").tobytes()).hexdigest()
stems_sha = {k: H(v) for k, v in res.stems.items()}
sum_err = float(np.max(np.abs(sum(res.stems.values()) - mix)))
prov = res.provenance()
del sep; torch.cuda.empty_cache()
from jukeoke_studio.transcribe import Transcriber
tr = Transcriber(models_dir=md)
lyr, words = tr.transcribe_lyrics(res.stems["lead_vocal"], sr, "en")
lap("transcribe")
from jukeoke_studio.align import Aligner, align_stage
al = Aligner(models_dir=md)
ar = align_stage(al, res.stems["lead_vocal"], sr, lyr)
lap("align")
from jukeoke_studio.analyse import analyse_stems
an = analyse_stems(res.stems, sr)
lap("analyse")
from jukeoke_studio import jksong
dur = mix.shape[1] / sr
problems = jksong.validate_lyrics(ar.lyrics, dur)
song_path = _os.path.join(out_dir, "runtime-check.jksong")
stems_in = [jksong.StemInput(r, a) for r, a in res.stems.items() if r != "other"] + [jksong.StemInput("other", None)]
jksong.write_song(song_path, stems=stems_in, sample_rate=sr, title="Runtime check", artist="Jukeoke",
                  lyrics=ar.lyrics, analysis=an, preset="full", stems_sum_to_source=True, provenance=prov,
                  source=mix, residual_role="other")
song = jksong.Song(song_path)
chunks = song.chunk_ids()
lap("write")
lyr_json = _json.dumps(ar.lyrics, sort_keys=True)
tw = [[w.text, round(w.start, 3), round(w.end, 3)] for w in words]
_emit({"decode_exact": decode_exact, "sample_rate": sr, "frames": int(mix.shape[1]),
       "stems_sha256": stems_sha, "stems_sum_max_abs_err": sum_err,
       "whisper": {"device": tr.device, "compute_type": tr.compute_type}, "transcript": " ".join(w[0] for w in tw),
       "transcript_words": tw, "aligned_lyrics_sha256": hashlib.sha256(lyr_json.encode()).hexdigest(),
       "aligned_words": sum(len(l.get("words", [])) for l in ar.lyrics.get("lines", [])),
       "align_warnings": ar.warnings, "analysis": an, "lyrics_problems": problems, "song_chunks": chunks,
       "song_bytes": _os.path.getsize(song_path), "seconds": T,
       "gpu": torch.cuda.get_device_name(0), "peak_vram_gb": round(torch.cuda.max_memory_allocated() / 1e9, 2),
       "ok": decode_exact and sum_err < 1e-5 and not problems and tr.device == "cuda" and len(tw) > 0})
'''


def run_check(name, args=(), timeout=900, show=()):
    rc, res, dt, tail = run_child(name, CHECKS[name], args=args, timeout=timeout)
    ok = bool(res and res.get("ok")) and rc == 0
    brief = {k: res[k] for k in show if res and k in res}
    record(f"python -I: {name}", ok, f"exit {rc}, {dt:.1f} s" + (f", {json.dumps(brief, default=str)}" if brief else "")
           + ("" if ok else f"; stderr tail: {tail}"), result=res)
    return res


def protocol_check():
    """hello + probe + models.status + shutdown over the real `serve` command."""
    cmd = [str(PYEXE), "-I", "-X", "utf8", "-m", "jukeoke_studio", "serve"]
    (ROOT / "Settings" / "tmp").mkdir(parents=True, exist_ok=True)
    errlog = open(OUT / "serve.stderr.log", "w", encoding="utf-8")
    t0 = time.perf_counter()
    p = subprocess.Popen(cmd, cwd=str(STUDIO), env=ENV, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=errlog, text=True, encoding="utf-8", bufsize=1)
    q = queue.Queue()
    threading.Thread(target=lambda: [q.put(l) for l in p.stdout] + [q.put(None)], daemon=True).start()
    seen, bad_lines = [], []

    def send(o):
        try:
            p.stdin.write(json.dumps(o) + "\n")
            p.stdin.flush()
        except OSError:   # the Studio exited; its stderr says why
            pass

    def wait(mid, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                line = q.get(timeout=max(0.1, end - time.monotonic()))
            except queue.Empty:
                break
            if line is None:
                return None
            try:
                m = json.loads(line)
            except ValueError:
                bad_lines.append(line[:200])
                continue
            seen.append(m)
            if m.get("id") == mid:
                return m
        return None

    try:
        send({"type": "hello", "id": "1", "protocol": 1, "app_version": "runtime-check", "root": str(ROOT),
              "models_dir": str(MODELS), "library_dir": str(ROOT / "Library"), "temp_dir": str(ROOT / "Settings" / "tmp")})
        hello = wait("1", 180)
        t_hello = time.perf_counter() - t0
        send({"type": "probe", "id": "2"})
        probe = wait("2", 240)
        t_probe = time.perf_counter() - t0
        send({"type": "models.status", "id": "3", "preset": "full"})
        ms = wait("3", 120)
        send({"type": "shutdown", "id": "4"})
        bye = wait("4", 60)
        try:
            p.wait(60)
        except subprocess.TimeoutExpired:
            p.kill()
    finally:
        if p.poll() is None:
            p.kill()
        errlog.close()
    ok = bool(hello and hello.get("type") == "hello.ok" and probe and probe.get("ok_for_studio") and probe.get("cuda"))
    err_tail = "" if ok else "; stderr tail: " + " | ".join(
        (OUT / "serve.stderr.log").read_text(encoding="utf-8", errors="replace").strip().splitlines()[-4:])
    record("protocol: serve hello + probe + models.status + shutdown", ok and not bad_lines,
           f"hello {t_hello:.1f} s -> {(hello or {}).get('type')}, probe {t_probe:.1f} s -> "
           f"ok_for_studio={(probe or {}).get('ok_for_studio')} gpu={(probe or {}).get('gpu')} "
           f"vram_mb={(probe or {}).get('vram_mb')}, models.status all_present={(ms or {}).get('all_present')}, "
           f"shutdown={(bye or {}).get('type')}, exit {p.returncode}, non-JSON stdout lines: {len(bad_lines)}"
           + err_tail,
           hello=hello, probe=probe, models_status=ms, shutdown=bye, non_json_stdout=bad_lines[:5])


def studio_tree_check():
    """Warns when the runtime's jukeoke_studio is not the one in this checkout (verifier
    addition 2026-11-17: the e2e stick passed every check while studio/ had already moved on).
    Same hash as build-studio-runtime.sh step 3: sha256 over `sha256sum` lines of every *.py
    under jukeoke_studio/, sorted by path (Git Bash's sha256sum writes "<hash> *<path>")."""
    import hashlib
    pkg = PY / "Lib" / "site-packages"
    files = sorted(str(p.relative_to(pkg)).replace("\\", "/") for p in (pkg / "jukeoke_studio").rglob("*.py")
                   if "__pycache__" not in p.parts)
    lines = "".join(f"{hashlib.sha256((pkg / f).read_bytes()).hexdigest()} *{f}\n" for f in files)
    installed = hashlib.sha256(lines.encode()).hexdigest()
    try:
        built = json.loads((STUDIO / "runtime.json").read_text(encoding="utf-8"))["jukeoke_studio"]["package_tree_sha256"]
    except (OSError, ValueError, KeyError, TypeError):
        built = None
    want = A.expect_studio_tree
    record("jukeoke_studio in the runtime matches studio/ in this checkout",
           None if (want and installed != want) or (built and installed != built) else True,
           f"installed {installed[:12]}, runtime.json {(built or 'n/a')[:12]}, checkout {(want or 'not given')[:12]}"
           + ("; REBUILD the runtime (scripts/build-studio-runtime.sh) before shipping" if want and installed != want else ""))


def main():
    print(f"runtime: {PY}\nmodels:  {MODELS}\nresults: {OUT}\ncaches redirected into the stick by this "
          f"harness: {'yes' if A.redirect_caches else 'no'}", flush=True)
    pe_list = static_checks()
    import_closure(pe_list)
    studio_tree_check()
    if A.static_only:
        return finish()

    rt_before = snap_tree([str(PY)])
    user_before = None
    if A.snapshot_user_dirs:
        t = time.perf_counter()
        user_before = snap_tree(user_roots(), skip=[str(ROOT)])
        record("user-folder snapshot (before)", None, f"{len(user_before)} entries in {time.perf_counter() - t:.0f} s")
    t_start = time.time()

    interp = run_check("interpreter", [PY], show=("version", "isolated", "user_site"))
    senv = (interp or {}).get("studio_env")
    studio_redirects = bool(senv and senv.get("set_vars", {}).get("CUDA_CACHE_PATH"))
    if senv is None:
        record("Studio start-up (jukeoke_studio.env.setup)", None,
               "this package build has no env.py: caches are only redirected with --redirect-caches")
    else:
        sroot = senv.get("root") or ""
        same = os.path.normcase(sroot) == os.path.normcase(str(ROOT))
        same_folder = same or (sroot and os.path.normcase(os.path.realpath(str(ROOT))) == os.path.normcase(sroot))
        outside_vars = {k: v for k, v in senv.get("set_vars", {}).items()
                        if not (inside(v, ROOT) or inside(v, MODELS) or (same_folder and inside(v, sroot)))}
        record("Studio start-up redirects every cache and temp folder into the stick",
               bool(studio_redirects and same_folder and not outside_vars),
               f"env.setup root {sroot}" + ("" if same else f" (launched as {ROOT}: same folder, reached through "
                                                              f"the drive mapping; Path.resolve() undoes subst)"
                                            if same_folder else f" is NOT the stick {ROOT}")
               + f"; {len(senv.get('set_vars', {}))} variables set, outside the stick: {outside_vars or 'none'}",
               studio_env=senv)
    run_check("torch", show=("torch", "gpu", "compute_capability", "cudnn", "matmul_rel_err",
                             "stft_cufft_rel_err", "conv1d_cudnn_rel_err", "import_s"))
    run_check("transformers", show=("transformers", "ctc_models", "alignment_families", "model_families_present"))
    run_check("ctranslate2", show=("ctranslate2", "faster_whisper", "cublas12_loaded", "cublas12_sgemm_rel_err",
                                   "cuda_compute_types", "whisper"))
    run_check("librosa", show=("librosa", "key", "tempo_bpm"))
    run_check("av", [OUT], show=("av", "libavcodec", "flac24_bit_exact", "codecs"))
    run_check("soundfile", [OUT], show=("soundfile", "libsndfile", "flac24_roundtrip_exact"))
    run_check("studio_import", show=("version", "modules", "import_errors", "gpu", "vram_mb"))

    rc, _, _, _ = run_child("studio_help", module="jukeoke_studio", args=["--help"], timeout=120)
    helptext = (OUT / "studio_help.log").read_text(encoding="utf-8")
    m = re.search(r"\{([a-z_,-]+)\}", helptext)          # argparse's "{models,separate,...}"
    commands = set(m.group(1).split(",")) if m else set()
    if "serve" in commands:
        protocol_check()
    elif "probe" in commands:
        rc, _, dt, tail = run_child("studio_probe", module="jukeoke_studio", args=["probe"], timeout=300)
        txt = (OUT / "studio_probe.log").read_text(encoding="utf-8")
        record("python -I -m jukeoke_studio probe", rc == 0 and '"ok_for_studio": true' in txt.replace("\n", " "),
               f"exit {rc}, {dt:.1f} s")
    else:
        record("protocol: serve", None, "this jukeoke_studio build has no `serve` or `probe` command yet; "
               "the studio_import check stands in for the probe")

    if A.separate:
        outdir = OUT / "separate"
        outdir.mkdir(exist_ok=True)
        rc, _, dt, tail = run_child("separate_cli", module="jukeoke_studio",
                                    args=["separate", A.separate, outdir, "--preset", "full"], timeout=1800)
        record("python -I -X utf8 -m jukeoke_studio separate ... --preset full", rc == 0,
               f"exit {rc}, {dt:.1f} s wall (process start to exit)" + ("" if rc == 0 else f"; {tail}"))
        if rc == 0:
            run_check("stems_verify", [A.separate, outdir], show=("sample_rate", "frames", "sum_max_abs_err",
                                                                  "rms_dbfs", "wall_seconds"))
    if A.pipeline:
        run_check("pipeline", [A.pipeline, OUT / "pipeline", MODELS], timeout=3600,
                  show=("decode_exact", "stems_sum_max_abs_err", "whisper", "transcript", "aligned_words",
                        "analysis", "song_chunks", "seconds", "peak_vram_gb"))

    # every module any check loaded: runtime, Windows, or the NVIDIA driver
    cls = {"runtime": [], "os": [], "driver": [], "redist": [], "other": []}
    for m in sorted(MODULES):
        if inside(m, PY):
            cls["runtime"].append(m)
        else:
            c, prod = classify_windows_file(m)
            cls[c].append(f"{m} [{prod}]")
    record("every loaded module came from the runtime, Windows or the NVIDIA driver",
           not cls["redist"] and not cls["other"],
           f"{len(MODULES)} modules: runtime {len(cls['runtime'])}, Windows {len(cls['os'])}, NVIDIA driver "
           f"{len(cls['driver'])}, redistributable from this PC {len(cls['redist'])}, other {len(cls['other'])}",
           redist=cls["redist"], other=cls["other"], driver=cls["driver"])
    vc = [m for m in cls["runtime"] if VC_RE.match(Path(m).name)]
    record("VC runtime came from the runtime folder", bool(vc), "; ".join(sorted({str(Path(m).relative_to(PY)) for m in vc})))

    rt_after = snap_tree([str(PY)])
    new, changed, gone = diff(rt_before, rt_after)
    record("nothing written into the runtime while it ran", not (new or changed or gone),
           f"new {len(new)}, changed {len(changed)}, deleted {len(gone)}",
           new=[os.path.relpath(p, PY) for p in new[:40]], changed=[os.path.relpath(p, PY) for p in changed[:40]],
           deleted=[os.path.relpath(p, PY) for p in gone[:40]])

    redirected = A.redirect_caches or studio_redirects
    if user_before is not None:
        t = time.perf_counter()
        user_after = snap_tree(user_roots(), skip=[str(ROOT)])
        new, changed, gone = diff(user_before, user_after)
        entries = [("new", p) for p in new] + [("changed", p) for p in changed]
        by = {"studio": [], "other": [], "unattributed": []}
        for k, p in entries:
            by[attribute(p)].append(f"{k}: {p}")
        (OUT / "user-dirs-diff.json").write_text(json.dumps({"window_s": round(time.time() - t_start), "new": new,
                                                             "changed": changed, "deleted": gone,
                                                             "attributed": by}, indent=1), encoding="utf-8")
        record("writes outside the stick during the run (real user folders)", not by["studio"] if redirected else None,
               f"{len(new)} new, {len(changed)} changed, {len(gone)} deleted in {round(time.time() - t_start)} s; "
               f"{len(by['studio'])} match what the Studio's libraries write (NVIDIA ComputeCache, Hugging Face, "
               f"torch, numba, pip, Python temp files ...), {len(by['other'])} are other programs on this PC "
               f"(browsers, OneDrive, other streams' compilers and pytest runs), {len(by['unattributed'])} "
               f"unattributed (after-snapshot {time.perf_counter() - t:.0f} s; lists in user-dirs-diff.json)",
               studio_like=by["studio"][:200], unattributed=by["unattributed"][:60], other_sample=by["other"][:20])

    if SENTINEL:
        found = []
        for dp, dns, fns in os.walk(SENTINEL):
            for n in fns:
                found.append(os.path.join(dp, n))
            for n in dns:   # an empty folder someone created counts too
                full = os.path.join(dp, n)
                if os.path.normcase(full) not in SENTINEL_DIRS and not any(os.scandir(full)):
                    found.append(full + "\\")
        record("nothing written to the per-user folders the Studio was given (sentinel profile)", not found,
               f"{len(found)} file(s) under {SENTINEL} (USERPROFILE, APPDATA, LOCALAPPDATA, TEMP of every check "
               f"process pointed there)", files=found[:100])

    # What the run left on the stick outside the runtime (caches count against the size budget).
    st = {}
    for sub in ("Settings/cache", "Settings/tmp/studio"):
        base = ROOT / sub
        if base.is_dir():
            for d in sorted(base.iterdir()):
                n = b = 0
                for dp, dns, fns in os.walk(d):
                    for f in fns:
                        n += 1
                        b += os.path.getsize(os.path.join(dp, f))
                st[f"{sub}/{d.name}"] = {"files": n, "bytes": b}
    record("what the Studio wrote on the stick (Settings/cache, Settings/tmp/studio)", None,
           ", ".join(f"{k}: {v['bytes'] / 1e6:.1f} MB in {v['files']} files" for k, v in st.items()) or "nothing",
           sizes=st)

    return finish()


def finish():
    fails = [r for r in RESULTS if r["status"] == "fail"]
    (OUT / "check-result.json").write_text(json.dumps({"root": str(ROOT), "models": str(MODELS),
                                                       "redirect_caches": A.redirect_caches,
                                                       "results": RESULTS}, indent=1, default=str), encoding="utf-8")
    print(f"\n{len(RESULTS) - len(fails)} passed or informational, {len(fails)} failed; details: {OUT / 'check-result.json'}")
    return 1 if fails else 0


sys.exit(main())
PYEOF

OUT_ARGS=()
case " ${ARGS[*]-} " in *" --out "*) ;; *) OUT_ARGS=(--out "$(cygpath -w "$OUT_DEFAULT")") ;; esac
# -B: the orchestrator itself must not write bytecode into the runtime it checks.
exec "$PYEXE" -I -B "$(cygpath -w "$CHECK_PY")" "$ROOT_W" "${FORBID[@]}" "${OUT_ARGS[@]}" ${TREE_ARGS[@]+"${TREE_ARGS[@]}"} \
    ${ARGS[@]+"${ARGS[@]}"}

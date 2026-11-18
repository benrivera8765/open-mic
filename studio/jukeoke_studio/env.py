# SPDX-License-Identifier: GPL-3.0-or-later
"""Process set-up that has to happen before any heavy library is imported.

Standard library only. ``__main__`` calls :func:`setup` first thing, before it imports
numpy, torch, transformers, huggingface_hub, numba, matplotlib or ctranslate2, because
those read their cache locations from the environment when they are imported (some at
first use, which is too late to be sure).

Two jobs:

1. **Portability (WORK_ORDER 4.8).** In portable mode nothing may be written to
   per-user folders. Every cache location the Studio's libraries know about is pointed
   into the stick (layout in ``studio/README.md``)::

       <root>/Models/              separation weights (manifest.json + files)
       <root>/Models/hf/           alignment + Whisper weights (Hugging Face layout)
       <root>/Settings/cache/      huggingface (HF_HOME), torch, xdg, cuda, numba,
                                   matplotlib, triton, inductor
       <root>/Settings/tmp/studio/ TMP / TEMP / TMPDIR, job scratch, extracted covers

   ``python -I`` ignores every ``PYTHON*`` variable, so bytecode writing is switched
   off in code (``sys.dont_write_bytecode``, also set in ``__init__``).
   The Hugging Face libraries are put in offline mode: the network is used only by the
   Studio's own downloader (``models.download``) and the LRCLIB lookup, both only on
   request.

2. **Clean stdout.** The protocol owns stdout (docs/STUDIO_PROTOCOL.md section 1). The
   real stdout file descriptor is duplicated for the protocol writer, and fd 1 itself is
   then pointed at stderr, so anything a library prints - Python ``print``, tqdm, C-level
   ``printf`` from CUDA, CTranslate2 or FFmpeg - lands in the log, never in the protocol.
"""

from __future__ import annotations

import os
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

sys.dont_write_bytecode = True

#: Cache variables and the sub-folder of ``<root>/Settings/cache`` each one gets.
CACHE_VARS = {
    "HF_HOME": "huggingface",
    "TORCH_HOME": "torch",
    "XDG_CACHE_HOME": "xdg",
    "CUDA_CACHE_PATH": "cuda",            # NVIDIA driver's JIT (PTX) cache
    "NUMBA_CACHE_DIR": "numba",           # librosa's numba functions
    "MPLCONFIGDIR": "matplotlib",
    "PYTORCH_KERNEL_CACHE_PATH": "torch/kernels",
    "TRITON_CACHE_DIR": "triton",
    "TORCHINDUCTOR_CACHE_DIR": "inductor",
}
TEMP_VARS = ("TMP", "TEMP", "TMPDIR")
#: Fixed values: no telemetry, no implicit Hugging Face network access.
FIXED_VARS = {
    "HF_HUB_OFFLINE": "1",
    "TRANSFORMERS_OFFLINE": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "HF_HUB_DISABLE_IMPLICIT_TOKEN": "1",
    "DO_NOT_TRACK": "1",
    "TQDM_DISABLE": "1",
    "KMP_WARNINGS": "0",
}


@dataclass
class StudioEnv:
    """Where this Studio process keeps things. ``root`` is None outside portable/app mode
    (a developer running ``make`` from a checkout), in which case nothing is redirected
    except bytecode and the Hugging Face offline switches."""

    root: Path | None
    models_dir: Path | None
    hf_dir: Path | None
    cache_dir: Path | None
    temp_dir: Path | None
    set_vars: dict[str, str] = field(default_factory=dict)


_ENV: StudioEnv | None = None


def packaged_root() -> Path | None:
    """``<root>`` when this interpreter is the packaged runtime
    ``<root>/Windows/Studio/python/python.exe``; None otherwise."""
    exe = Path(sys.executable).resolve()
    parts = exe.parents
    try:
        python_dir, studio_dir, windows_dir = parts[0], parts[1], parts[2]
    except IndexError:
        return None
    if (python_dir.name.lower() == "python" and studio_dir.name.lower() == "studio"
            and windows_dir.name.lower() == "windows"):
        return windows_dir.parent
    return None


def setup(root: str | os.PathLike | None = None, models_dir: str | os.PathLike | None = None,
          temp_dir: str | os.PathLike | None = None) -> StudioEnv:
    """Points every cache into ``<root>`` (see the module docstring). Idempotent.

    ``root`` comes from ``--root``, else ``$JUKEOKE_ROOT``, else the packaged-runtime
    location. ``models_dir`` from ``--models-dir``, else ``$JUKEOKE_MODELS_DIR``, else
    ``<root>/Models``. Existing values of the cache variables are overwritten in
    portable mode on purpose: a variable inherited from the user's environment would
    otherwise send a cache to their profile.
    """
    global _ENV
    sys.dont_write_bytecode = True
    for k, v in FIXED_VARS.items():
        os.environ[k] = v
    root_s = root or os.environ.get("JUKEOKE_ROOT") or packaged_root()
    root_p = Path(root_s).resolve() if root_s else None
    md = models_dir or os.environ.get("JUKEOKE_MODELS_DIR") or (root_p / "Models" if root_p else None)
    models_p = Path(md).resolve() if md else None
    set_vars: dict[str, str] = {}
    cache_p = tmp_p = hf_p = None
    if models_p is not None:
        os.environ["JUKEOKE_MODELS_DIR"] = str(models_p)
        hf_p = models_p / "hf"
        # Hugging Face weights live with the other weights, not in a cache.
        for k in ("HF_HUB_CACHE", "TRANSFORMERS_CACHE", "HUGGINGFACE_HUB_CACHE"):
            os.environ[k] = set_vars[k] = str(hf_p)
    if root_p is not None:
        os.environ["JUKEOKE_ROOT"] = str(root_p)
        cache_p = root_p / "Settings" / "cache"
        tmp_p = Path(temp_dir).resolve() if temp_dir else root_p / "Settings" / "tmp" / "studio"
        for k, sub in CACHE_VARS.items():
            d = cache_p / sub
            os.environ[k] = set_vars[k] = str(d)
        for k in TEMP_VARS:
            os.environ[k] = set_vars[k] = str(tmp_p)
        # Created lazily by the libraries, but TMP must exist before tempfile uses it.
        tmp_p.mkdir(parents=True, exist_ok=True)
        tempfile.tempdir = None  # forget any directory computed before the change
    _ENV = StudioEnv(root_p, models_p, hf_p, cache_p, tmp_p, set_vars)
    return _ENV


def current() -> StudioEnv:
    return _ENV if _ENV is not None else setup()


def set_temp_dir(temp_dir: str | os.PathLike) -> Path:
    """The app's ``hello.temp_dir`` (``<root>/Settings/tmp/studio``) for this session."""
    p = Path(temp_dir).resolve()
    p.mkdir(parents=True, exist_ok=True)
    env = current()
    if env.root is not None:
        for k in TEMP_VARS:
            os.environ[k] = env.set_vars[k] = str(p)
        tempfile.tempdir = None
    env.temp_dir = p
    return p


def temp_dir() -> Path:
    env = current()
    return env.temp_dir if env.temp_dir is not None else Path(tempfile.gettempdir())


# ------------------------------------------------------------------------- stdout

class ProtocolOut:
    """The protocol's private copy of stdout (binary, one JSON line per write)."""

    def __init__(self, fd: int):
        self._f = os.fdopen(fd, "wb", buffering=0)

    def write_line(self, line: str) -> None:
        self._f.write(line.encode("utf-8") + b"\n")

    def close(self) -> None:
        try:
            self._f.close()
        except OSError:
            pass


def claim_stdout() -> ProtocolOut:
    """Duplicates the real stdout for the protocol, then sends fd 1 (and ``sys.stdout``)
    to stderr. Call before importing any library that might print."""
    try:
        sys.stdout.flush()
    except (AttributeError, OSError, ValueError):
        pass
    proto_fd = os.dup(1)
    os.dup2(2, 1)
    try:
        sys.stdout = sys.stderr
    except Exception:  # noqa: BLE001
        pass
    if sys.platform == "win32":
        import ctypes
        import msvcrt

        msvcrt.setmode(proto_fd, os.O_BINARY)
        # DLLs with their own C runtime (CUDA, CTranslate2, FFmpeg) take their stdout
        # from the process's standard handle when they load: point it at stderr too.
        try:
            k32 = ctypes.windll.kernel32
            k32.SetStdHandle(ctypes.c_ulong(-11 & 0xFFFFFFFF),  # STD_OUTPUT_HANDLE
                             ctypes.c_void_p(msvcrt.get_osfhandle(2)))
        except (OSError, AttributeError):
            pass
    return ProtocolOut(proto_fd)


def claim_stdin():
    """Takes the request pipe away from the standard input handle and returns a private,
    non-inheritable binary reader for it; fd 0, ``sys.stdin`` and the process's standard
    input handle become NUL.

    Why: on Windows a thread blocked in a synchronous read of a pipe blocks every other
    operation on that pipe, including the ``GetFileType`` that ``CreateProcess`` performs
    on inherited standard handles. Any library that starts a helper process while the
    server waits for the next request (measured: the job's analyse stage stalled until
    the next request arrived) would hang. Child processes now inherit NUL instead.
    """
    proto_fd = os.dup(0)  # Python makes duplicates non-inheritable
    nul = os.open(os.devnull, os.O_RDONLY)
    os.dup2(nul, 0)
    os.close(nul)
    try:
        sys.stdin = open(os.devnull, "r", encoding="utf-8")  # noqa: SIM115
    except OSError:
        pass
    if sys.platform == "win32":
        import ctypes
        import msvcrt

        msvcrt.setmode(proto_fd, os.O_BINARY)
        try:
            ctypes.windll.kernel32.SetStdHandle(ctypes.c_ulong(-10 & 0xFFFFFFFF),  # STD_INPUT_HANDLE
                                                ctypes.c_void_p(msvcrt.get_osfhandle(0)))
        except (OSError, AttributeError):
            pass
    return os.fdopen(proto_fd, "rb")

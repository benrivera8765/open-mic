# SPDX-License-Identifier: GPL-3.0-or-later
"""Can this computer run the Studio? (protocol ``probe``, WORKING ORDER 4.9)

It must never hang and should not cost a PyTorch import on a computer that cannot run
the Studio anyway, so it works in two steps:

1. **Fast path, standard library only** (milliseconds): physical memory from the OS and
   the NVIDIA driver's own CUDA library (``nvcuda.dll`` / ``libcuda.so``) through ctypes:
   is there a CUDA device, its name, memory and compute capability. No GPU, too little
   VRAM or RAM -> answered at once with plain-language reasons, PyTorch never imported.
2. **PyTorch check** only when the hardware is good enough: import torch in a worker
   thread and run a real tensor operation on the GPU (this is what catches a PyTorch
   build without kernels for the card, WORKING ORDER trap 1). The probe waits at most
   ``timeout_s`` for it. If the import is slower than that (a cold start from a slow USB
   stick) the answer says so instead of hanging, and the check keeps running in the
   background; the next probe reuses its result.

Thresholds: ``MIN_VRAM_MB`` / ``MIN_RAM_MB`` below. They are set a little under the
nominal 6 GB / 12 GB so that cards and PCs sold with exactly that much pass (a "6 GB" card
reports 6144 MiB, a "12 GB" PC about 11.9 GiB usable). The default preset peaks at
4.1 GB of VRAM (docs/benchmarks/separation.md); Whisper large-v3-turbo and the alignment
model each need less, and stages run one after the other with VRAM freed in between.
"""

from __future__ import annotations

import ctypes
import os
import sys
import threading
import time

MIN_VRAM_MB = 5632      # 5.5 GiB: nominal 6 GB cards pass
MIN_RAM_MB = 11264      # 11 GiB: nominal 12 GB computers pass
NOMINAL = {"vram_gb": 6, "ram_gb": 12}

REASON_NO_GPU = ("No NVIDIA graphics card with CUDA was found. Songs are created on a stronger "
                 "computer and can be played anywhere.")
REASON_NO_DRIVER = ("The NVIDIA graphics driver could not be used (it may be missing or too old). "
                    "Songs are created on a stronger computer and can be played anywhere.")


def _reason_vram(mb: int) -> str:
    return (f"The graphics card has {mb / 1024:.1f} GB of memory; the Studio needs at least "
            f"{NOMINAL['vram_gb']} GB. Songs are created on a stronger computer and can be played anywhere.")


def _reason_ram(mb: int) -> str:
    return (f"This computer has {mb / 1024:.1f} GB of memory; the Studio needs at least "
            f"{NOMINAL['ram_gb']} GB. Songs are created on a stronger computer and can be played anywhere.")


def ram_mb() -> int | None:
    """Physical memory in MiB (OS call, no third-party package)."""
    try:
        if sys.platform == "win32":
            class MEMORYSTATUSEX(ctypes.Structure):
                _fields_ = [("dwLength", ctypes.c_ulong), ("dwMemoryLoad", ctypes.c_ulong),
                            ("ullTotalPhys", ctypes.c_ulonglong), ("ullAvailPhys", ctypes.c_ulonglong),
                            ("ullTotalPageFile", ctypes.c_ulonglong), ("ullAvailPageFile", ctypes.c_ulonglong),
                            ("ullTotalVirtual", ctypes.c_ulonglong), ("ullAvailVirtual", ctypes.c_ulonglong),
                            ("ullAvailExtendedVirtual", ctypes.c_ulonglong)]

            st = MEMORYSTATUSEX()
            st.dwLength = ctypes.sizeof(MEMORYSTATUSEX)
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(st)):
                return int(st.ullTotalPhys // (1024 * 1024))
            return None
        return int(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") // (1024 * 1024))
    except (OSError, ValueError, AttributeError):
        return None


def cuda_driver_info() -> dict:
    """Device 0 as the CUDA driver API sees it: {"driver": bool, "count", "name",
    "vram_mb", "compute_capability"} (driver False when nvcuda cannot be loaded)."""
    names = ["nvcuda.dll"] if sys.platform == "win32" else ["libcuda.so.1", "libcuda.so"]
    lib = None
    for n in names:
        try:
            lib = ctypes.WinDLL(n) if sys.platform == "win32" else ctypes.CDLL(n)
            break
        except OSError:
            continue
    if lib is None:
        return {"driver": False, "count": 0}
    try:
        if lib.cuInit(0) != 0:
            return {"driver": False, "count": 0}
        count = ctypes.c_int(0)
        if lib.cuDeviceGetCount(ctypes.byref(count)) != 0 or count.value < 1:
            return {"driver": True, "count": 0}
        dev = ctypes.c_int(0)
        lib.cuDeviceGet(ctypes.byref(dev), 0)
        name = ctypes.create_string_buffer(256)
        lib.cuDeviceGetName(name, 256, dev)
        mem = ctypes.c_size_t(0)
        f = getattr(lib, "cuDeviceTotalMem_v2", None) or lib.cuDeviceTotalMem
        f(ctypes.byref(mem), dev)
        major, minor = ctypes.c_int(0), ctypes.c_int(0)
        lib.cuDeviceGetAttribute(ctypes.byref(major), 75, dev)  # COMPUTE_CAPABILITY_MAJOR
        lib.cuDeviceGetAttribute(ctypes.byref(minor), 76, dev)  # COMPUTE_CAPABILITY_MINOR
        return {"driver": True, "count": count.value, "name": name.value.decode("utf-8", "replace"),
                "vram_mb": int(mem.value // (1024 * 1024)),
                "compute_capability": f"{major.value}.{minor.value}"}
    except (OSError, AttributeError) as e:
        return {"driver": False, "count": 0, "error": str(e)}


# --------------------------------------------------------------------- PyTorch check

class _TorchCheck:
    """Imports torch and runs a real GPU operation once, in a background thread."""

    def __init__(self):
        self.done = threading.Event()
        self.result: dict | None = None
        self._lock = threading.Lock()
        self._started = False

    def start(self) -> None:
        with self._lock:
            if self._started:
                return
            self._started = True
        threading.Thread(target=self._run, name="torch-check", daemon=True).start()

    def _run(self) -> None:
        t0 = time.perf_counter()
        try:
            import torch

            r: dict = {"torch": torch.__version__, "cuda_build": torch.version.cuda}
            if not torch.cuda.is_available():
                r.update(ok=False, error="torch.cuda.is_available() is False")
            else:
                cap = torch.cuda.get_device_capability(0)
                arch = f"sm_{cap[0]}{cap[1]}"
                archs = torch.cuda.get_arch_list()
                a = torch.arange(256, dtype=torch.float32, device="cuda").reshape(16, 16)
                v = float((a @ a).sum().item())
                torch.cuda.synchronize()
                expect = float((a.cpu() @ a.cpu()).sum().item())
                r.update(ok=abs(v - expect) <= 1e-3 * abs(expect), arch=arch, arch_list=archs,
                         device=torch.cuda.get_device_name(0))
                if not r["ok"]:
                    r["error"] = f"GPU matmul gave {v}, expected {expect}"
        except Exception as e:  # noqa: BLE001 - any failure means "cannot run"
            r = {"ok": False, "error": f"{type(e).__name__}: {e}"}
        r["seconds"] = round(time.perf_counter() - t0, 2)
        self.result = r
        self.done.set()


_TORCH = _TorchCheck()


def start_torch_check() -> None:
    """Begin the PyTorch check early (``hello``), so the first probe is quick."""
    _TORCH.start()


def probe(timeout_s: float = 45.0, fake: str | None = None) -> dict:
    """The protocol's ``probe.result`` body (without type/id).

    ``fake`` (tests, from ``$JUKEOKE_STUDIO_FAKE_HW``): ``none`` = no CUDA GPU,
    ``small`` = a 4 GB card in an 8 GB PC, ``ok`` = a capable stub GPU (no PyTorch check).
    """
    t0 = time.perf_counter()
    ram = ram_mb()
    if fake == "none":
        drv = {"driver": False, "count": 0}
    elif fake == "small":
        drv = {"driver": True, "count": 1, "name": "Small Test GPU", "vram_mb": 4096,
               "compute_capability": "7.5"}
        ram = 8192
    elif fake == "ok":
        drv = {"driver": True, "count": 1, "name": "Stub Test GPU", "vram_mb": 16303,
               "compute_capability": "12.0"}
        ram = 32652
    else:
        drv = cuda_driver_info()
    reasons: list[str] = []
    out = {"ok_for_studio": False, "cuda": False, "gpu": drv.get("name"), "vram_mb": drv.get("vram_mb"),
           "compute_capability": drv.get("compute_capability"), "ram_mb": ram,
           "thresholds": {"vram_mb": MIN_VRAM_MB, "ram_mb": MIN_RAM_MB, "nominal": dict(NOMINAL)},
           "checked_with": "driver", "reasons": reasons}
    if not drv.get("driver"):
        reasons.append(REASON_NO_GPU if fake == "none" or "error" not in drv else REASON_NO_DRIVER)
    elif not drv.get("count"):
        reasons.append(REASON_NO_GPU)
    if drv.get("vram_mb") is not None and drv["vram_mb"] < MIN_VRAM_MB:
        reasons.append(_reason_vram(drv["vram_mb"]))
    if ram is not None and ram < MIN_RAM_MB:
        reasons.append(_reason_ram(ram))
    if reasons or fake:
        out["cuda"] = bool(drv.get("count")) and not reasons
        out["ok_for_studio"] = not reasons
        out["seconds"] = round(time.perf_counter() - t0, 3)
        return out
    # Hardware is fine: confirm that PyTorch can really use the card.
    _TORCH.start()
    if not _TORCH.done.wait(timeout_s):
        out["cuda"] = True
        out["ok_for_studio"] = True
        out["checked_with"] = "driver (the PyTorch check is still running)"
        out["torch_check"] = "pending"
        out["seconds"] = round(time.perf_counter() - t0, 3)
        return out
    r = _TORCH.result or {"ok": False, "error": "no result"}
    out["checked_with"] = "torch"
    out["torch"] = r.get("torch")
    out["torch_check"] = {k: v for k, v in r.items() if k != "arch_list"}
    if r.get("ok"):
        out["cuda"] = True
        out["ok_for_studio"] = True
    else:
        reasons.append("The graphics card was found, but the Studio's PyTorch could not use it "
                       f"({r.get('error', 'unknown error')}). Songs are created on a stronger "
                       "computer and can be played anywhere.")
    out["seconds"] = round(time.perf_counter() - t0, 3)
    return out

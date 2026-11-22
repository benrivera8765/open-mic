# SPDX-License-Identifier: GPL-3.0-or-later
"""Download-on-first-use for model weights, verified by SHA-256.

Weights are never committed or bundled (WORK_ORDER section 10). The first time a model
is needed its files are streamed to ``<models_dir>/<name>.part`` while being hashed;
only a file whose SHA-256 matches the registry is renamed into place (atomic on the same
volume, so a pulled USB stick never leaves a half file under the real name).

``<models_dir>/manifest.json`` records, for every file we placed or hashed: model id,
source URL, size, modification time, the volume it was hashed on, SHA-256 and the UTC
time it was verified. Later runs trust a file whose size, modification time and volume
still match an entry whose SHA-256 is the registry's, so a 1.7 GB checkpoint is not
re-hashed on every song, but a models folder copied to another stick is hashed once
there (:func:`trusted`, D-172); ``verify=True`` forces a full re-hash. A file whose
hash was wrong is recorded as ``bad`` and counts as missing, so the Studio screen's
download replaces it (only when the user asks). Pickled weights are never trusted this
way: :func:`read_verified` hashes the exact bytes that are then unpickled (separate.py).

A download in flight writes ``<file>.part``. On Windows that file is marked
delete-on-close as soon as it is created, so the system removes it if the process ends
before the file is complete (an exit during ``models.download``, a crash, or the app
killing the Studio); the mark is cleared only after the SHA-256 matched, just before
the rename. :func:`abort_downloads` (called when the Studio shuts down) also closes the
connections of downloads in flight so their threads stop and delete their files
(D-170).

Nothing downloads unless the caller says so: ``ensure_model(..., allow_download=False)``
(the default) raises :class:`~jukeoke_studio.errors.ModelsMissing` for a missing file.
The protocol's ``models.download`` request is the only path in the Studio that passes
``allow_download=True`` (separation-stream verifier finding: no silent downloads).
"""

from __future__ import annotations

import atexit
import hashlib
import json
import logging
import os
import sys
import threading
import time
import urllib.request
from pathlib import Path
from typing import Callable

from ..errors import ModelChecksum, ModelsMissing
from .registry import ModelSpec, WeightFile

log = logging.getLogger("jukeoke_studio.download")

USER_AGENT = "jukeoke-studio/0.1 (open-source karaoke app; model download)"
MANIFEST = "manifest.json"

Progress = Callable[[str, int, int], None]  # (filename, bytes_done, bytes_total)


def default_models_dir() -> Path:
    """Where models live when the caller does not say.

    1. ``$JUKEOKE_MODELS_DIR`` (the app sets this to ``<root>/Models`` in portable mode);
    2. ``$JK_BUILD/models-cache`` (development checkout, see scripts/devenv.sh);
    3. ``jukeoke-build/models-cache`` next to the repository.
    """
    env = os.environ.get("JUKEOKE_MODELS_DIR")
    if env:
        return Path(env)
    jk = os.environ.get("JK_BUILD")
    if jk:
        return Path(jk) / "models-cache"
    return Path(__file__).resolve().parents[4] / "jukeoke-build" / "models-cache"


def sha256_of(path: Path, chunk: int = 1 << 22) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def _load_manifest(models_dir: Path) -> dict:
    try:
        return json.loads((models_dir / MANIFEST).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def _save_manifest(models_dir: Path, manifest: dict) -> None:
    # A temporary name per process and thread: a job loading its models and a
    # ``models.download`` thread can save at the same moment, and with one shared
    # "manifest.json.tmp" the second os.replace failed (FileNotFoundError -> the job
    # failed with write_failed). Verifier finding. Last writer wins; a lost entry only
    # means one extra re-hash later.
    tmp = models_dir / f"{MANIFEST}.tmp-{os.getpid()}-{threading.get_ident()}"
    tmp.write_text(json.dumps(manifest, indent=1, sort_keys=True), encoding="utf-8")
    os.replace(tmp, models_dir / MANIFEST)


# ------------------------------------------------------------------ verification cache

def _stat_key(st: os.stat_result) -> dict:
    """What a manifest entry records about the file it vouches for. ``dev`` is the
    volume (on Windows its serial number), so the record holds only on the drive where
    the file was hashed."""
    return {"size": st.st_size, "mtime_ns": st.st_mtime_ns, "dev": st.st_dev}


def trusted(entry: dict | None, sha256: str, st: os.stat_result) -> bool:
    """True when ``entry`` says this very file (same size, modification time and volume)
    was hashed before and matched ``sha256``.

    Trade-off (D-172): a file replaced by different bytes of exactly the same size, with
    its modification time set back and on the same drive, would be trusted until the next
    ``--verify``. Hashing every weight file at every start instead costs seconds on an
    SSD and up to a minute on a slow stick (2.9 GB for the default alignment and Whisper
    models). The record lives on the stick, so moving the models folder to another drive
    re-hashes them once there."""
    if not entry or entry.get("bad") or entry.get("sha256") != sha256:
        return False
    k = _stat_key(st)
    return all(entry.get(f) == k[f] for f in ("size", "mtime_ns", "dev"))


def recorded_bad(entry: dict | None, st: os.stat_result) -> bool:
    """True when ``entry`` says this very file failed its hash check."""
    if not entry or not entry.get("bad"):
        return False
    k = _stat_key(st)
    return all(entry.get(f) == k[f] for f in ("size", "mtime_ns", "dev"))


def _record(manifest: dict, key: str, st: os.stat_result, sha256: str, ok: bool, **extra) -> None:
    old = manifest.get(key, {})
    entry = {**extra, "sha256": sha256, **_stat_key(st)}
    if ok:
        kept = old.get("verified_utc") if not old.get("bad") and old.get("sha256") == sha256 else None
        entry["verified_utc"] = kept or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    else:
        entry["bad"] = True
        entry["checked_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    manifest[key] = entry


def _is_verified(path: Path, wf: WeightFile, manifest: dict, verify: bool) -> bool:
    if not path.exists():
        return False
    st = path.stat()
    if not verify and trusted(manifest.get(wf.filename), wf.sha256, st):
        return True
    return sha256_of(path) == wf.sha256


# ------------------------------------------------------------------ .part files

class _Active:
    """One download in flight: its ``.part`` path, connection and open file."""

    def __init__(self, part: Path):
        self.part = part
        self.response = None
        self.file = None


_ACTIVE: set[_Active] = set()
_ACTIVE_LOCK = threading.Lock()


def _open_part(part: Path):
    """Opens ``part`` for writing (truncated). Returns (file, handle): on Windows the file
    is delete-pending from the start (FileDispositionInfo), so the system deletes it when
    its handle closes - also when the process is killed - unless :func:`_keep_part` clears
    the mark first. ``handle`` is None where that is not available (other systems, or a
    file system that refuses it); the file is then an ordinary one."""
    if sys.platform == "win32":
        try:
            return _open_part_windows(part)
        except OSError as e:
            log.warning("could not mark %s delete-on-close (%s); using a plain file", part, e)
    return open(part, "wb"), None  # noqa: SIM115


def _win32():
    import ctypes
    from ctypes import wintypes

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateFileW.restype = wintypes.HANDLE
    k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                                wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    k32.SetFileInformationByHandle.restype = wintypes.BOOL
    k32.SetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    return ctypes, wintypes, k32


def _set_delete(handle, delete: bool) -> None:
    ctypes, wintypes, k32 = _win32()

    class FILE_DISPOSITION_INFO(ctypes.Structure):  # noqa: N801
        _fields_ = [("DeleteFile", wintypes.BOOLEAN)]

    info = FILE_DISPOSITION_INFO(1 if delete else 0)
    if not k32.SetFileInformationByHandle(handle, 4, ctypes.byref(info), ctypes.sizeof(info)):  # FileDispositionInfo
        raise ctypes.WinError(ctypes.get_last_error())


def _open_part_windows(part: Path):
    import msvcrt

    ctypes, wintypes, k32 = _win32()
    GENERIC_WRITE, DELETE = 0x40000000, 0x00010000
    FILE_SHARE_READ, FILE_SHARE_DELETE = 0x1, 0x4
    CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL = 2, 0x80
    h = k32.CreateFileW(str(part), GENERIC_WRITE | DELETE, FILE_SHARE_READ | FILE_SHARE_DELETE, None,
                        CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None)
    if h is None or h == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        _set_delete(h, True)
        fd = msvcrt.open_osfhandle(h, os.O_WRONLY | os.O_BINARY)
    except BaseException:
        k32.CloseHandle(h)
        part.unlink(missing_ok=True)
        raise
    return os.fdopen(fd, "wb"), h


def _keep_part(handle) -> None:
    """Clears the delete-on-close mark of a complete, verified ``.part`` file."""
    if handle is not None:
        _set_delete(handle, False)


def abort_downloads() -> int:
    """Stops every download in flight: closes its connection, so a thread blocked in a
    network read wakes up, fails and deletes its ``.part`` file. Returns how many."""
    with _ACTIVE_LOCK:
        active = list(_ACTIVE)
    for a in active:
        r = a.response
        if r is None:
            continue
        try:  # the socket itself first, for a read that is already blocked
            sock = getattr(getattr(getattr(r, "fp", None), "raw", None), "_sock", None)
            if sock is not None:
                import socket

                sock.shutdown(socket.SHUT_RDWR)
        except Exception:  # noqa: BLE001
            pass
        try:
            r.close()
        except Exception:  # noqa: BLE001
            pass
    return len(active)


def active_downloads() -> int:
    with _ACTIVE_LOCK:
        return len(_ACTIVE)


@atexit.register
def remove_leftover_parts() -> None:
    """Last resort at interpreter exit: closes and deletes the ``.part`` file of any
    download whose thread did not finish (daemon threads are not joined)."""
    with _ACTIVE_LOCK:
        active = list(_ACTIVE)
    for a in active:
        try:
            if a.file is not None:
                a.file.close()
        except Exception:  # noqa: BLE001
            pass
        try:
            a.part.unlink(missing_ok=True)
        except OSError:
            pass


def _download(wf: WeightFile, dest: Path, progress: Progress | None,
              should_stop: Callable[[], bool] | None = None) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    req = urllib.request.Request(wf.url, headers={"User-Agent": USER_AGENT})
    h = hashlib.sha256()
    done = 0
    act = _Active(part)
    with _ACTIVE_LOCK:
        _ACTIVE.add(act)
    try:
        with urllib.request.urlopen(req, timeout=120) as r:  # noqa: S310
            act.response = r
            out, handle = _open_part(part)
            act.file = out
            with out:
                total = int(r.headers.get("Content-Length") or wf.size or 0)
                last = 0.0
                while True:
                    if should_stop is not None and should_stop():
                        raise InterruptedError("download stopped")
                    block = r.read(1 << 22)
                    # A stop closes the connection (abort_downloads), which can look like
                    # the end of the file: ask first.
                    if should_stop is not None and should_stop():
                        raise InterruptedError("download stopped")
                    if not block:
                        break
                    out.write(block)
                    h.update(block)
                    done += len(block)
                    if progress and time.monotonic() - last > 0.5:
                        progress(wf.filename, done, total)
                        last = time.monotonic()
                digest = h.hexdigest()
                if digest != wf.sha256:
                    raise ModelChecksum(
                        "A downloaded model file was not the expected one, so it was not used. Try the "
                        "download again later.",
                        f"{wf.filename}: SHA-256 mismatch (got {digest}, expected {wf.sha256}); "
                        f"the file at {wf.url} is not the one Open Mic was tested with")
                if wf.size and done != wf.size:
                    raise IOError(f"{wf.filename}: size {done} != expected {wf.size}")
                out.flush()
                os.fsync(out.fileno())
                _keep_part(handle)
        os.replace(part, dest)
    except BaseException as e:
        # Network error, wrong file, stop or shutdown: never leave a half file behind
        # (on Windows closing the handle has already deleted it).
        part.unlink(missing_ok=True)
        if should_stop is not None and not isinstance(e, InterruptedError) and should_stop():
            # A stop closes the connection under the read: report the stop, not the socket.
            raise InterruptedError("download stopped") from e
        raise
    finally:
        with _ACTIVE_LOCK:
            _ACTIVE.discard(act)
    if progress:
        progress(wf.filename, done, done)


def missing_files(spec: ModelSpec, models_dir: Path | None = None) -> list[WeightFile]:
    """Files of ``spec`` that are not on disk (by name and size), or that failed their
    hash check (manifest ``bad``). No hashing, so it is fast enough for ``models.status``."""
    models_dir = Path(models_dir) if models_dir else default_models_dir()
    manifest = _load_manifest(models_dir)
    out = []
    for wf in spec.files:
        p = models_dir / wf.filename
        try:
            st = p.stat() if p.is_file() else None
            ok = (st is not None and (not wf.size or st.st_size == wf.size)
                  and not recorded_bad(manifest.get(wf.filename), st))
        except OSError:
            ok = False
        if not ok:
            out.append(wf)
    return out


def ensure_model(spec: ModelSpec, models_dir: Path | None = None, *, verify: bool = False,
                 progress: Progress | None = None, allow_download: bool = False,
                 should_stop: Callable[[], bool] | None = None) -> list[Path]:
    """Return local paths of ``spec``'s files, verifying them (SHA-256, or the manifest's
    size + mtime record). A missing or changed file is downloaded only with
    ``allow_download=True``; otherwise :class:`ModelsMissing` is raised."""
    models_dir = Path(models_dir) if models_dir else default_models_dir()
    if not allow_download:
        gone = [wf.filename for wf in spec.files if not (models_dir / wf.filename).is_file()]
        if gone:
            raise ModelsMissing([spec.id], f"{spec.id}: not in {models_dir}: {gone}")
    models_dir.mkdir(parents=True, exist_ok=True)
    manifest = _load_manifest(models_dir)
    paths = []
    for wf in spec.files:
        dest = models_dir / wf.filename
        extra = {"model": spec.id, "url": wf.url, "licence": spec.licence}
        if not _is_verified(dest, wf, manifest, verify):
            if not allow_download:
                # Recorded, so models.status lists it as missing and the Studio screen's
                # download replaces it; never replaced without being asked (D-151).
                _record(manifest, wf.filename, dest.stat(), wf.sha256, False, **extra)
                _save_manifest(models_dir, manifest)
                raise ModelChecksum(
                    "A model file on this drive is damaged or not the expected one. Download "
                    "the models again from the Studio screen.",
                    f"{dest}: SHA-256 differs from the registry ({wf.sha256})")
            _download(wf, dest, progress, should_stop)
        _record(manifest, wf.filename, dest.stat(), wf.sha256, True, **extra)
        paths.append(dest)
    _save_manifest(models_dir, manifest)
    return paths


def read_verified(path: Path, wf: WeightFile) -> bytes:
    """The bytes of ``path`` after checking their SHA-256 against ``wf``. Used for
    pickled weights (demucs ``.th``): the exact bytes that were hashed are the bytes that
    get unpickled, so a file swapped after an earlier check is never loaded."""
    data = Path(path).read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != wf.sha256:
        raise ModelChecksum(
            "A model file on this drive is damaged or not the expected one. Download the "
            "models again from the Studio screen.",
            f"{path}: SHA-256 {digest} != {wf.sha256}; refusing to unpickle it")
    return data

# SPDX-License-Identifier: GPL-3.0-or-later
"""Download-on-first-use for model weights, verified by SHA-256.

Weights are never committed or bundled (WORK_ORDER section 10). The first time a model
is needed its files are streamed to ``<models_dir>/<name>.part`` while being hashed;
only a file whose SHA-256 matches the registry is renamed into place (atomic on the same
volume, so a pulled USB stick never leaves a half file under the real name).

``<models_dir>/manifest.json`` records, for every file we placed: model id, source URL,
size, SHA-256 and the UTC time it was verified. Later runs trust a file whose size and
modification time still match the manifest entry, so a 1.7 GB checkpoint is not
re-hashed on every song; ``verify=True`` forces a full re-hash. Pickled weights are
never trusted this way: :func:`read_verified` hashes the exact bytes that are then
unpickled (separate.py).

Nothing downloads unless the caller says so: ``ensure_model(..., allow_download=False)``
(the default) raises :class:`~jukeoke_studio.errors.ModelsMissing` for a missing file.
The protocol's ``models.download`` request is the only path in the Studio that passes
``allow_download=True`` (separation-stream verifier finding: no silent downloads).
"""

from __future__ import annotations

import hashlib
import json
import os
import threading
import time
import urllib.request
from pathlib import Path
from typing import Callable

from ..errors import ModelChecksum, ModelsMissing
from .registry import ModelSpec, WeightFile

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


def _is_verified(path: Path, wf: WeightFile, manifest: dict, verify: bool) -> bool:
    if not path.exists():
        return False
    st = path.stat()
    entry = manifest.get(wf.filename)
    if (not verify and entry and entry.get("sha256") == wf.sha256
            and entry.get("size") == st.st_size and entry.get("mtime_ns") == st.st_mtime_ns):
        return True
    return sha256_of(path) == wf.sha256


def _download(wf: WeightFile, dest: Path, progress: Progress | None,
              should_stop: Callable[[], bool] | None = None) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    req = urllib.request.Request(wf.url, headers={"User-Agent": USER_AGENT})
    h = hashlib.sha256()
    done = 0
    try:
        with urllib.request.urlopen(req, timeout=120) as r, open(part, "wb") as out:  # noqa: S310
            total = int(r.headers.get("Content-Length") or wf.size or 0)
            last = 0.0
            while True:
                block = r.read(1 << 22)
                if not block:
                    break
                if should_stop is not None and should_stop():
                    raise InterruptedError("download stopped")
                out.write(block)
                h.update(block)
                done += len(block)
                if progress and time.monotonic() - last > 0.5:
                    progress(wf.filename, done, total)
                    last = time.monotonic()
            out.flush()
            os.fsync(out.fileno())
    except BaseException:
        # Network error, stop or shutdown: never leave a half file behind.
        part.unlink(missing_ok=True)
        raise
    digest = h.hexdigest()
    if digest != wf.sha256:
        part.unlink(missing_ok=True)
        raise ModelChecksum(
            "A downloaded model file was not the expected one, so it was not used. Try the "
            "download again later.",
            f"{wf.filename}: SHA-256 mismatch (got {digest}, expected {wf.sha256}); "
            f"the file at {wf.url} is not the one Jukeoke was tested with")
    if wf.size and done != wf.size:
        part.unlink(missing_ok=True)
        raise IOError(f"{wf.filename}: size {done} != expected {wf.size}")
    os.replace(part, dest)
    if progress:
        progress(wf.filename, done, done)


def missing_files(spec: ModelSpec, models_dir: Path | None = None) -> list[WeightFile]:
    """Files of ``spec`` that are not on disk (by name and size; no hashing, so it is
    fast enough for ``models.status``)."""
    models_dir = Path(models_dir) if models_dir else default_models_dir()
    out = []
    for wf in spec.files:
        p = models_dir / wf.filename
        try:
            ok = p.is_file() and (not wf.size or p.stat().st_size == wf.size)
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
        if not _is_verified(dest, wf, manifest, verify):
            if not allow_download:
                raise ModelChecksum(
                    "A model file on this drive is damaged or not the expected one. Download "
                    "the models again from the Studio screen.",
                    f"{dest}: SHA-256 differs from the registry ({wf.sha256})")
            _download(wf, dest, progress, should_stop)
        st = dest.stat()
        manifest[wf.filename] = {
            "model": spec.id, "url": wf.url, "sha256": wf.sha256, "size": st.st_size,
            "mtime_ns": st.st_mtime_ns, "licence": spec.licence,
            "verified_utc": manifest.get(wf.filename, {}).get("verified_utc")
            or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
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

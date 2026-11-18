# SPDX-License-Identifier: GPL-3.0-or-later
"""Download-on-first-use: SHA-256 verification, atomic placement, manifest (CPU, offline).

A local ``file://`` URL stands in for Hugging Face / GitHub, so these run without a
network; the real download path is exercised by ``python -m jukeoke_studio models fetch``.
"""

import hashlib
import json

import pytest

from jukeoke_studio.errors import ModelChecksum, ModelsMissing
from jukeoke_studio.modelzoo import download as D
from jukeoke_studio.modelzoo.registry import ModelSpec, WeightFile


def _spec(src, sha=None, size=None, name="weights.ckpt"):
    data = src.read_bytes()
    wf = WeightFile(src.as_uri(), name, sha or hashlib.sha256(data).hexdigest(),
                    len(data) if size is None else size)
    return ModelSpec(id="test_model", title="t", arch="bs_roformer", role="vocals", files=(wf,),
                     outputs=("vocals",), licence="MIT", commercial="yes",
                     source="https://example.invalid", author="test")


@pytest.fixture
def src(tmp_path):
    p = tmp_path / "upstream" / "model.ckpt"
    p.parent.mkdir()
    p.write_bytes(bytes(range(256)) * 20481)  # 5.2 MB: more than one 4 MB read block
    return p


def test_first_use_downloads_verifies_and_records(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    [path] = D.ensure_model(spec, models, allow_download=True)
    assert path == models / "weights.ckpt"
    assert path.read_bytes() == src.read_bytes()
    assert not list(models.glob("*.part"))  # nothing half-written left behind
    man = json.loads((models / D.MANIFEST).read_text(encoding="utf-8"))
    entry = man["weights.ckpt"]
    assert entry["sha256"] == spec.files[0].sha256 and entry["model"] == "test_model"
    assert entry["size"] == src.stat().st_size and entry["licence"] == "MIT"


def test_second_use_trusts_manifest_without_rehash_or_download(tmp_path, src, monkeypatch):
    models = tmp_path / "Models"
    spec = _spec(src)
    D.ensure_model(spec, models, allow_download=True)
    monkeypatch.setattr(D, "_download", lambda *a, **k: pytest.fail("downloaded again"))
    monkeypatch.setattr(D, "sha256_of", lambda *a, **k: pytest.fail("re-hashed although unchanged"))
    D.ensure_model(spec, models, allow_download=True)


def test_changed_file_is_rehashed_and_replaced(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    [path] = D.ensure_model(spec, models, allow_download=True)
    path.write_bytes(b"corrupted")  # size and mtime no longer match the manifest
    [path] = D.ensure_model(spec, models, allow_download=True)
    assert path.read_bytes() == src.read_bytes()


def test_hash_mismatch_is_rejected_and_nothing_is_placed(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src, sha="0" * 64)
    with pytest.raises(ModelChecksum) as e:
        D.ensure_model(spec, models, allow_download=True)
    assert "SHA-256 mismatch" in e.value.detail and e.value.code == "model_checksum"
    assert not (models / "weights.ckpt").exists()
    assert not list(models.glob("*.part"))


def test_size_mismatch_is_rejected(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src, size=123)
    with pytest.raises(IOError, match="size"):
        D.ensure_model(spec, models, allow_download=True)
    assert not (models / "weights.ckpt").exists()


def test_models_dir_from_environment(monkeypatch, tmp_path):
    monkeypatch.setenv("JUKEOKE_MODELS_DIR", str(tmp_path / "Models"))
    assert D.default_models_dir() == tmp_path / "Models"
    monkeypatch.delenv("JUKEOKE_MODELS_DIR")
    monkeypatch.setenv("JK_BUILD", str(tmp_path / "build"))
    assert D.default_models_dir() == tmp_path / "build" / "models-cache"


def test_no_download_unless_allowed(tmp_path, src, monkeypatch):
    """Verifier finding (a): nothing downloads silently; a missing file is models_missing."""
    models = tmp_path / "Models"
    spec = _spec(src)
    monkeypatch.setattr(D, "_download", lambda *a, **k: pytest.fail("downloaded without permission"))
    with pytest.raises(ModelsMissing) as e:
        D.ensure_model(spec, models)
    assert e.value.code == "models_missing" and e.value.missing == ["test_model"]
    assert [wf.filename for wf in D.missing_files(spec, models)] == ["weights.ckpt"]


def test_damaged_file_is_not_replaced_without_permission(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    [path] = D.ensure_model(spec, models, allow_download=True)
    path.write_bytes(b"corrupted")
    with pytest.raises(ModelChecksum):
        D.ensure_model(spec, models)
    assert path.read_bytes() == b"corrupted"  # left for the user's explicit re-download


def test_read_verified_hashes_the_bytes_it_returns(tmp_path, src):
    spec = _spec(src)
    assert D.read_verified(src, spec.files[0]) == src.read_bytes()
    bad = tmp_path / "bad.th"
    bad.write_bytes(b"not the file")
    with pytest.raises(ModelChecksum):
        D.read_verified(bad, spec.files[0])


def test_interrupted_download_leaves_no_part_file(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    with pytest.raises(InterruptedError):
        D.ensure_model(spec, models, allow_download=True, should_stop=lambda: True)
    assert not list(models.glob("*.part")) and not (models / "weights.ckpt").exists()


def test_hf_model_download_layout_progress_and_refusal(tmp_path, monkeypatch):
    """Alignment / Whisper files: missing -> ModelsMissing unless allowed; allowed ->
    .part + SHA-256 + rename into the Hugging Face snapshot layout, with progress."""
    from jukeoke_studio.modelzoo import hfmodels as H

    up = tmp_path / "upstream"
    up.mkdir()
    blobs = {"config.json": b'{"a": 1}', "model.bin": bytes(range(256)) * 30000}
    for n, b in blobs.items():
        (up / n).write_bytes(b)
    m = H.HfModel(key="t", task="alignment", title="t", repo="org/name", revision="0" * 40,
                  files=tuple(H.HfFile(n, len(b), hashlib.sha256(b).hexdigest()) for n, b in blobs.items()),
                  weight_file="model.bin", licence="MIT", commercial="yes", source_url="https://example.invalid")
    monkeypatch.setattr(H.HfModel, "url", lambda self, f: (up / f.name).as_uri())
    models = tmp_path / "Models"
    with pytest.raises(ModelsMissing):
        H.ensure(m, models)
    assert len(H.missing(m, models)) == 2
    seen = []
    snap = H.ensure(m, models, allow_download=True, progress=lambda f, a, b: seen.append((f, a, b)))
    assert snap == models / "hf" / "models--org--name" / "snapshots" / ("0" * 40)
    assert {p.name for p in snap.iterdir()} == set(blobs) and H.missing(m, models) == []
    assert ("model.bin", len(blobs["model.bin"]), len(blobs["model.bin"])) in seen
    (snap / "model.bin").write_bytes(b"short")  # wrong size: counted as missing again
    assert [f.name for f in H.missing(m, models)] == ["model.bin"]


# ----------------------------------------------------------- .part files (D-170)

import http.server  # noqa: E402
import os  # noqa: E402
import subprocess  # noqa: E402
import sys  # noqa: E402
import threading  # noqa: E402
import time  # noqa: E402
from pathlib import Path  # noqa: E402

SLOW_TOTAL = 200 * 1024 * 1024


class _Slow(http.server.BaseHTTPRequestHandler):
    """A local server that sends a 200 MB file at about 12 MB/s (never finished in a test)."""

    def do_GET(self):  # noqa: N802
        self.send_response(200)
        self.send_header("Content-Length", str(SLOW_TOTAL))
        self.end_headers()
        chunk = b"\0" * (256 * 1024)
        sent = 0
        try:
            while sent < SLOW_TOTAL and not self.server.stop.is_set():
                self.wfile.write(chunk)
                sent += len(chunk)
                time.sleep(0.02)
        except OSError:
            pass

    def log_message(self, *args):
        pass


@pytest.fixture
def slow_url():
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Slow)
    srv.daemon_threads = True
    srv.stop = threading.Event()
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_address[1]}/weights.bin"
    srv.stop.set()
    srv.shutdown()
    srv.server_close()


def _wait(pred, timeout=15.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        time.sleep(0.05)
    return pred()


def _listed(folder: Path, name: str) -> bool:
    """By directory listing: a delete-pending file cannot be opened or stat'ed reliably."""
    try:
        return name in os.listdir(folder)
    except OSError:
        return False


def test_abort_stops_a_download_blocked_on_the_network_and_deletes_its_part_file(tmp_path, slow_url):
    wf = WeightFile(slow_url, "weights.bin", "0" * 64, SLOW_TOTAL)
    dest = tmp_path / "Models" / "weights.bin"
    stop = threading.Event()
    errors = []

    def run():
        try:
            D._download(wf, dest, None, should_stop=stop.is_set)
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    t = threading.Thread(target=run)
    t.start()
    assert _wait(lambda: D.active_downloads() == 1 and _listed(dest.parent, "weights.bin.part"))
    time.sleep(0.5)  # some blocks written
    stop.set()
    t0 = time.monotonic()
    assert D.abort_downloads() == 1
    t.join(10)
    assert not t.is_alive() and time.monotonic() - t0 < 5
    assert errors and isinstance(errors[0], InterruptedError)
    assert not _listed(dest.parent, "weights.bin.part") and not dest.exists()
    assert D.active_downloads() == 0


@pytest.mark.skipif(sys.platform != "win32", reason="delete-on-close marks are a Windows feature")
def test_part_file_disappears_when_the_process_is_killed_mid_download(tmp_path, slow_url):
    """The app kills a Studio that does not exit in time: the half-downloaded file must
    not survive the process (FileDispositionInfo on the .part handle)."""
    dest = tmp_path / "Models" / "weights.bin"
    code = ("from pathlib import Path\n"
            "from jukeoke_studio.modelzoo import download as D\n"
            "from jukeoke_studio.modelzoo.registry import WeightFile\n"
            f"wf = WeightFile({slow_url!r}, 'weights.bin', '0' * 64, {SLOW_TOTAL})\n"
            f"D._download(wf, Path({str(dest)!r}), None)\n")
    p = subprocess.Popen([sys.executable, "-I", "-c", code], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        assert _wait(lambda: _listed(dest.parent, "weights.bin.part"), 30)
        time.sleep(1.0)  # several 4 MB blocks written
        assert p.poll() is None
    finally:
        p.kill()
        p.wait(10)
    assert _wait(lambda: not _listed(dest.parent, "weights.bin.part"), 5)
    assert not dest.exists()


def test_completed_download_keeps_its_file(tmp_path, src):
    """The delete-on-close mark is cleared before the rename: a finished file stays."""
    models = tmp_path / "Models"
    [path] = D.ensure_model(_spec(src), models, allow_download=True)
    time.sleep(0.1)
    assert path.read_bytes() == src.read_bytes() and not _listed(models, "weights.ckpt.part")


# ------------------------------------------------- verification before loading (D-172)

def _hf_model(blobs, H):
    return H.HfModel(key="t", task="alignment", title="t", repo="org/name", revision="1" * 40,
                     files=tuple(H.HfFile(n, len(b), hashlib.sha256(b).hexdigest()) for n, b in blobs.items()),
                     weight_file="model.bin", licence="MIT", commercial="yes", source_url="https://example.invalid")


def _count_hashes(monkeypatch, *modules):
    calls = []
    real = D.sha256_of

    def counting(p, *a, **k):
        calls.append(Path(p).name)
        return real(p, *a, **k)

    for m in modules:
        monkeypatch.setattr(m, "sha256_of", counting)
    return calls


def test_hf_files_are_hashed_before_loading_once_per_run_and_then_trusted_on_this_drive(tmp_path, monkeypatch):
    from jukeoke_studio.modelzoo import hfmodels as H

    blobs = {"config.json": b'{"a": 1}', "model.bin": bytes(range(256)) * 30000}
    m = _hf_model(blobs, H)
    models = tmp_path / "Models"
    snap = H.snapshot_dir(m, models)
    snap.mkdir(parents=True)
    for n, b in blobs.items():  # placed by hand (an older stick, or another tool): never hashed
        (snap / n).write_bytes(b)
    monkeypatch.setattr(H, "_CHECKED", {})
    calls = _count_hashes(monkeypatch, H, D)
    assert H.ensure(m, models) == snap
    assert sorted(calls) == ["config.json", "model.bin"]
    man = json.loads((models / D.MANIFEST).read_text(encoding="utf-8"))
    entry = man[H.manifest_key(m, m.files[1])]
    st = (snap / "model.bin").stat()
    assert (entry["sha256"], entry["size"], entry["mtime_ns"], entry["dev"]) == \
        (m.files[1].sha256, st.st_size, st.st_mtime_ns, st.st_dev)
    calls.clear()
    H.ensure(m, models)  # same process: not hashed again
    assert calls == []
    monkeypatch.setattr(H, "_CHECKED", {})  # a new run: the manifest vouches for the files
    H.ensure(m, models)
    assert calls == []
    # The same folder on another drive (volume differs): hashed again, once.
    man[H.manifest_key(m, m.files[1])]["dev"] = st.st_dev + 1
    (models / D.MANIFEST).write_text(json.dumps(man), encoding="utf-8")
    monkeypatch.setattr(H, "_CHECKED", {})
    H.ensure(m, models)
    assert calls == ["model.bin"]


def test_hf_damaged_file_is_refused_counted_as_missing_and_replaced_only_on_request(tmp_path, monkeypatch):
    from jukeoke_studio.modelzoo import hfmodels as H

    blobs = {"config.json": b'{"a": 1}', "model.bin": bytes(range(256)) * 30000}
    m = _hf_model(blobs, H)
    up = tmp_path / "upstream"
    up.mkdir()
    for n, b in blobs.items():
        (up / n).write_bytes(b)
    monkeypatch.setattr(H.HfModel, "url", lambda self, f: (up / f.name).as_uri())
    monkeypatch.setattr(H, "_CHECKED", {})
    models = tmp_path / "Models"
    snap = H.snapshot_dir(m, models)
    snap.mkdir(parents=True)
    (snap / "config.json").write_bytes(blobs["config.json"])
    damaged = bytearray(blobs["model.bin"])
    damaged[1000] ^= 0xFF  # same size, one byte flipped
    (snap / "model.bin").write_bytes(bytes(damaged))
    assert H.missing(m, models) == []  # by size it looks complete
    with pytest.raises(ModelChecksum) as e:
        H.ensure(m, models)
    assert e.value.code == "model_checksum" and "model.bin" in e.value.detail
    assert [f.name for f in H.missing(m, models)] == ["model.bin"]  # models.status: missing
    with pytest.raises(ModelsMissing):
        H.ensure(m, models)  # still not replaced behind the user's back
    assert (snap / "model.bin").read_bytes() == bytes(damaged)
    H.ensure(m, models, allow_download=True)  # the Studio screen's "Download"
    assert (snap / "model.bin").read_bytes() == blobs["model.bin"] and H.missing(m, models) == []
    monkeypatch.setattr(H, "_CHECKED", {})
    H.ensure(m, models)


def test_damaged_separation_file_counts_as_missing_until_downloaded_again(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    [path] = D.ensure_model(spec, models, allow_download=True)
    data = bytearray(path.read_bytes())
    data[10] ^= 0xFF
    path.write_bytes(bytes(data))  # same size: missing_files cannot see it by size alone
    assert D.missing_files(spec, models) == []
    with pytest.raises(ModelChecksum):
        D.ensure_model(spec, models)
    assert [wf.filename for wf in D.missing_files(spec, models)] == ["weights.ckpt"]
    D.ensure_model(spec, models, allow_download=True)
    assert path.read_bytes() == src.read_bytes() and D.missing_files(spec, models) == []


def test_manifest_from_another_drive_is_not_trusted(tmp_path, src, monkeypatch):
    models = tmp_path / "Models"
    spec = _spec(src)
    D.ensure_model(spec, models, allow_download=True)
    man = json.loads((models / D.MANIFEST).read_text(encoding="utf-8"))
    man["weights.ckpt"]["dev"] += 1
    (models / D.MANIFEST).write_text(json.dumps(man), encoding="utf-8")
    calls = _count_hashes(monkeypatch, D)
    D.ensure_model(spec, models)
    assert calls == ["weights.ckpt"]

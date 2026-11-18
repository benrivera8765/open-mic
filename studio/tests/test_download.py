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

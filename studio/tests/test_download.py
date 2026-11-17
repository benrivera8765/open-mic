# SPDX-License-Identifier: GPL-3.0-or-later
"""Download-on-first-use: SHA-256 verification, atomic placement, manifest (CPU, offline).

A local ``file://`` URL stands in for Hugging Face / GitHub, so these run without a
network; the real download path is exercised by ``python -m jukeoke_studio models fetch``.
"""

import hashlib
import json

import pytest

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
    [path] = D.ensure_model(spec, models)
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
    D.ensure_model(spec, models)
    monkeypatch.setattr(D, "_download", lambda *a, **k: pytest.fail("downloaded again"))
    monkeypatch.setattr(D, "sha256_of", lambda *a, **k: pytest.fail("re-hashed although unchanged"))
    D.ensure_model(spec, models)


def test_changed_file_is_rehashed_and_replaced(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src)
    [path] = D.ensure_model(spec, models)
    path.write_bytes(b"corrupted")  # size and mtime no longer match the manifest
    [path] = D.ensure_model(spec, models)
    assert path.read_bytes() == src.read_bytes()


def test_hash_mismatch_is_rejected_and_nothing_is_placed(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src, sha="0" * 64)
    with pytest.raises(IOError, match="SHA-256 mismatch"):
        D.ensure_model(spec, models)
    assert not (models / "weights.ckpt").exists()
    assert not list(models.glob("*.part"))


def test_size_mismatch_is_rejected(tmp_path, src):
    models = tmp_path / "Models"
    spec = _spec(src, size=123)
    with pytest.raises(IOError, match="size"):
        D.ensure_model(spec, models)
    assert not (models / "weights.ckpt").exists()


def test_models_dir_from_environment(monkeypatch, tmp_path):
    monkeypatch.setenv("JUKEOKE_MODELS_DIR", str(tmp_path / "Models"))
    assert D.default_models_dir() == tmp_path / "Models"
    monkeypatch.delenv("JUKEOKE_MODELS_DIR")
    monkeypatch.setenv("JK_BUILD", str(tmp_path / "build"))
    assert D.default_models_dir() == tmp_path / "build" / "models-cache"

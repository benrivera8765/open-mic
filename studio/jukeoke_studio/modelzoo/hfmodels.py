# SPDX-License-Identifier: GPL-3.0-or-later
"""Hugging Face models of the lyrics stages: the CTC alignment model and Whisper.

Pinned like the separation registry: a repository, a commit and the SHA-256 and size of
every file the Studio loads (computed on 2026-11-17 from the copies downloaded at those
commits; the large weight files' hashes are also Hugging Face's own LFS ids, and are
listed in MODELS.md).

Files live in the Hugging Face cache layout under ``<models_dir>/hf``::

    <models_dir>/hf/models--facebook--wav2vec2-large-960h-lv60-self/snapshots/<commit>/<file>

so a cache filled by ``huggingface_hub`` (the alignment benchmark's) is used as it is,
and our own downloader writes the same layout. Loading always passes that snapshot
*directory* to transformers / faster-whisper, so no Hugging Face code ever resolves a
name or contacts the network (``env.py`` also sets ``HF_HUB_OFFLINE=1``).

Downloads happen only through :func:`ensure` with ``allow_download=True`` (the
protocol's ``models.download``): ``<file>.part`` while hashing, SHA-256 and size check,
then an atomic rename (``download._download``).
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ..errors import ModelsMissing
from .download import Progress, _download, default_models_dir
from .registry import WeightFile

HF = "https://huggingface.co"


@dataclass(frozen=True)
class HfFile:
    name: str
    size: int
    sha256: str


@dataclass(frozen=True)
class HfModel:
    key: str            # short id used by the Studio ("wav2vec2-large-960h-lv60-self", "large-v3-turbo")
    task: str           # "alignment" | "transcription"
    title: str
    repo: str
    revision: str
    files: tuple[HfFile, ...]
    weight_file: str    # the main weights (its SHA-256 goes into the song's provenance)
    licence: str
    commercial: str
    source_url: str

    @property
    def size_bytes(self) -> int:
        return sum(f.size for f in self.files)

    def url(self, f: HfFile) -> str:
        return f"{HF}/{self.repo}/resolve/{self.revision}/{f.name}"

    def weight(self) -> HfFile:
        return next(f for f in self.files if f.name == self.weight_file)


HF_MODELS: dict[str, HfModel] = {}


def _add(m: HfModel) -> None:
    assert m.key not in HF_MODELS
    HF_MODELS[m.key] = m


_add(HfModel(
    key="wav2vec2-large-960h-lv60-self", task="alignment",
    title="wav2vec 2.0 Large (LV-60k, self-training) fine-tuned on LibriSpeech 960 h",
    repo="facebook/wav2vec2-large-960h-lv60-self",
    revision="54074b1c16f4de6a5ad59affb4caa8f2ea03a119",
    files=(
        HfFile("config.json", 1606, "bcb82b508db66c6772b34a978bcc6a411f9ea99c33cd9dee337119b5f6ddb797"),
        HfFile("feature_extractor_config.json", 157, "5415d0e5f57a6893552b1f916d6e9b9ce312a147fff3e65b260fefadd2f77dbf"),
        HfFile("preprocessor_config.json", 158, "c403ce09975b90dff0dd8302c42d422e9de1f166cd7772df23490069893cb0cf"),
        HfFile("pytorch_model.bin", 1262055246, "00b604cf4d28e86559e8adaeb3a186daa89dc37f5ab216771a0a15a26db0de9f"),
        HfFile("special_tokens_map.json", 85, "bb7068de1150661a10b55f9e4b12a0e77af8bf91f5e45e1b58afaf1d0e17f675"),
        HfFile("tokenizer_config.json", 162, "c9a86ca4c3d72d4cda976512cd786cc65d87fe22afcd4164c8470f1dde7c9758"),
        HfFile("vocab.json", 291, "19727f8944fe6459fc3f240ae2c198395b740f6a029bd23e06656266b83bcf64"),
    ),
    weight_file="pytorch_model.bin", licence="Apache-2.0", commercial="yes",
    source_url="https://huggingface.co/facebook/wav2vec2-large-960h-lv60-self",
))

_add(HfModel(  # option (measured about the same as wav2vec2, 3x the size; not used by default)
    key="hubert-xlarge-ls960-ft", task="alignment",
    title="HuBERT X-Large fine-tuned on LibriSpeech 960 h (character CTC)",
    repo="facebook/hubert-xlarge-ls960-ft",
    revision="86a09e67e0c8d074533992379242405825516eca",
    files=(
        HfFile("config.json", 1487, "2f642ecafe5bacc6682078c51786ed11ad042593e8ae6f5ca09ca1605c92b7fc"),
        HfFile("model.safetensors", 3850253320, "a4596d21af9a6842bdf6cbe28eb835ea33980c05efbdec65c39b3609943cd537"),
        HfFile("preprocessor_config.json", 212, "a2254a5b58f72cd4de3632f8eee64f3f098b7c1402128d2f419e7d00ae13e335"),
        HfFile("special_tokens_map.json", 85, "bb7068de1150661a10b55f9e4b12a0e77af8bf91f5e45e1b58afaf1d0e17f675"),
        HfFile("tokenizer_config.json", 138, "d773cde8c2ca89ab86892f0b5e670c5ebed4dc72e20ed647f6cb843ad2c0bd26"),
        HfFile("vocab.json", 292, "91be72ec00818d246b96e287630a7b03187e367907835177b618276798df725d"),
    ),
    weight_file="model.safetensors", licence="Apache-2.0", commercial="yes",
    source_url="https://huggingface.co/facebook/hubert-xlarge-ls960-ft",
))

_add(HfModel(
    key="large-v3-turbo", task="transcription",
    title="Whisper large-v3-turbo (CTranslate2 conversion by Mobius Labs)",
    repo="mobiuslabsgmbh/faster-whisper-large-v3-turbo",
    revision="0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf",
    files=(
        HfFile("config.json", 2263, "b0253ea6c0d3bea6b1e19e91a02acfd3b53f4467362efcb5a3e6b16c9b3a9b7e"),
        HfFile("model.bin", 1617884929, "e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da"),
        HfFile("preprocessor_config.json", 340, "7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711"),
        HfFile("tokenizer.json", 2710337, "297b13372ac43916285644fb9687add3cc62ee2a1adb60da3dc25cc94c1871fd"),
        HfFile("vocabulary.json", 1068114, "c69260f2ab26d659b7c398f9a2b2b48ed0df16c3b47d7326782fd9cba71690c1"),
    ),
    weight_file="model.bin", licence="MIT", commercial="yes",
    source_url="https://huggingface.co/mobiuslabsgmbh/faster-whisper-large-v3-turbo",
))

_add(HfModel(
    key="large-v3", task="transcription",
    title="Whisper large-v3 (CTranslate2 conversion by Systran)",
    repo="Systran/faster-whisper-large-v3",
    revision="edaa852ec7e145841d8ffdb056a99866b5f0a478",
    files=(
        HfFile("config.json", 2394, "a9306624f5ec14270a014b647e5c316b6e03a662c369758d1b90697a7b0655b9"),
        HfFile("model.bin", 3087284237, "69f74147e3334731bc3a76048724833325d2ec74642fb52620eda87352e3d4f1"),
        HfFile("preprocessor_config.json", 340, "7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711"),
        HfFile("tokenizer.json", 2480617, "6d8cbd7cd0d8d5815e478dac67b85a26bbe77c1f5e0c6d76d1ce2abc0e5f21ca"),
        HfFile("vocabulary.json", 1068114, "c69260f2ab26d659b7c398f9a2b2b48ed0df16c3b47d7326782fd9cba71690c1"),
    ),
    weight_file="model.bin", licence="MIT", commercial="yes",
    source_url="https://huggingface.co/Systran/faster-whisper-large-v3",
))

#: Repository name -> key (align.py's MODEL_INFO is keyed by repository).
BY_REPO = {m.repo: m for m in HF_MODELS.values()}

DEFAULT_ALIGNMENT = "wav2vec2-large-960h-lv60-self"
DEFAULT_TRANSCRIPTION = "large-v3-turbo"


def get(key_or_repo: str) -> HfModel:
    m = HF_MODELS.get(key_or_repo) or BY_REPO.get(key_or_repo)
    if m is None:
        raise KeyError(f"unknown Hugging Face model {key_or_repo!r}")
    return m


def hf_dir(models_dir: str | Path | None = None) -> Path:
    """``<models_dir>/hf`` (portable: ``<root>/Models/hf``)."""
    return Path(models_dir or default_models_dir()) / "hf"


def snapshot_dir(m: HfModel, models_dir: str | Path | None = None) -> Path:
    return hf_dir(models_dir) / f"models--{m.repo.replace('/', '--')}" / "snapshots" / m.revision


def missing(m: HfModel, models_dir: str | Path | None = None) -> list[HfFile]:
    """Files not on disk with the expected size (no hashing: fast, for ``models.status``)."""
    d = snapshot_dir(m, models_dir)
    out = []
    for f in m.files:
        p = d / f.name
        try:
            ok = p.is_file() and p.stat().st_size == f.size
        except OSError:
            ok = False
        if not ok:
            out.append(f)
    return out


def ensure(m: HfModel, models_dir: str | Path | None = None, *, allow_download: bool = False,
           progress: Progress | None = None, should_stop: Callable[[], bool] | None = None) -> Path:
    """The snapshot directory of ``m`` with every file present. Missing files are
    downloaded only with ``allow_download``; otherwise :class:`ModelsMissing`."""
    gone = missing(m, models_dir)
    if gone and not allow_download:
        raise ModelsMissing([m.key], f"{m.repo}@{m.revision[:8]}: missing {[f.name for f in gone]} "
                                     f"in {snapshot_dir(m, models_dir)}")
    d = snapshot_dir(m, models_dir)
    for f in gone:
        wf = WeightFile(m.url(f), f.name, f.sha256, f.size)
        _download(wf, d / f.name, progress, should_stop)
    return d

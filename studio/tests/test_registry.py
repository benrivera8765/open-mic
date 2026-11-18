# SPDX-License-Identifier: GPL-3.0-or-later
"""The model catalogue is complete, pinned, and documented in MODELS.md."""

import re
from pathlib import Path

import pytest

from jukeoke_studio.modelzoo import MODELS
from jukeoke_studio.separate import COMMERCIAL_USE_PRESETS, PERMISSIVE_LICENCES, PRESETS

ROOT = Path(__file__).resolve().parents[2]
ROLE_OF = {"vocals": {"vocals", "multistem"}, "instrumental": {"instrumental"},
           "karaoke": {"karaoke"}, "drums": {"multistem"}}


@pytest.mark.parametrize("mid", sorted(MODELS))
def test_spec_is_pinned(mid):
    m = MODELS[mid]
    assert m.commercial in {"yes", "no", "unclear"}
    assert m.licence and m.source.startswith("https://")
    for f in m.files:
        assert re.fullmatch(r"[0-9a-f]{64}", f.sha256), f
        assert f.url.startswith("https://")
        if "huggingface.co" in f.url:  # revision-pinned, never "main"
            assert re.search(r"/resolve/[0-9a-f]{40}/", f.url), f.url
    if m.arch != "htdemucs":
        assert m.chunk_size > 0 and m.model_kwargs


def test_filenames_unique():
    names = [f.filename for m in MODELS.values() for f in m.files]
    assert len(names) == len(set(names))


def test_default_preset_allows_commercial_use():
    # The default ("full") must never depend on a model without a usable licence.
    for mid in PRESETS["full"].values():
        assert MODELS[mid].commercial == "yes", mid


def test_best_measured_roles():
    from jukeoke_studio.separate import BEST_MEASURED

    for role, mid in BEST_MEASURED.items():
        assert MODELS[mid].role in ROLE_OF[role], (role, mid)


def test_presets_reference_models_in_the_right_role():
    for table in (PRESETS, COMMERCIAL_USE_PRESETS):
        for preset, roles in table.items():
            for role, mid in roles.items():
                assert MODELS[mid].role in ROLE_OF[role], (preset, role, mid)


def test_commercial_use_presets_allow_commercial_use():
    for roles in COMMERCIAL_USE_PRESETS.values():
        for mid in roles.values():
            assert MODELS[mid].commercial == "yes", mid


def test_models_md_lists_every_model_and_hash():
    text = (ROOT / "MODELS.md").read_text(encoding="utf-8")
    for mid, m in MODELS.items():
        assert f"`{mid}`" in text, f"MODELS.md is missing {mid}"
        for f in m.files:
            assert f.sha256 in text, f"MODELS.md is missing the SHA-256 of {f.filename}"


def test_permissive_preset_uses_only_mit_or_apache_weights():
    """Verifier finding (c): a real 'permissive' preset (no GPL, nothing unlicensed)."""
    roles = PRESETS["permissive"]
    assert "karaoke" not in roles  # no permissively licensed lead/backing splitter exists
    for mid in roles.values():
        assert MODELS[mid].licence in PERMISSIVE_LICENCES, (mid, MODELS[mid].licence)


def test_preset_info_describes_every_preset():
    from jukeoke_studio.separate import PRESET_INFO

    assert set(PRESET_INFO) == set(PRESETS)
    assert PRESET_INFO["permissive"]["backing_vocals"] is False
    assert "backing" in PRESET_INFO["permissive"]["description"].lower()
    assert [k for k, v in PRESET_INFO.items() if v["default"]] == ["full"]


def test_hf_models_pinned_and_documented():
    """Alignment / Whisper models: every file pinned (commit, size, SHA-256), the main
    weights listed in MODELS.md, and the same commits the alignment code was measured with."""
    from jukeoke_studio.align import MODEL_INFO
    from jukeoke_studio.modelzoo import hfmodels
    from jukeoke_studio.transcribe import WHISPER_MODELS

    text = (ROOT / "MODELS.md").read_text(encoding="utf-8")
    for m in hfmodels.HF_MODELS.values():
        assert re.fullmatch(r"[0-9a-f]{40}", m.revision), m.key
        assert m.weight_file in {f.name for f in m.files}
        for f in m.files:
            assert re.fullmatch(r"[0-9a-f]{64}", f.sha256) and f.size > 0, (m.key, f.name)
            assert m.url(f).startswith(f"https://huggingface.co/{m.repo}/resolve/{m.revision}/")
        assert m.weight().sha256 in text, f"MODELS.md is missing the SHA-256 of {m.repo}"
        assert m.licence in ("MIT", "Apache-2.0") and m.commercial == "yes"
    for repo, info in MODEL_INFO.items():
        if repo in hfmodels.BY_REPO:
            assert hfmodels.BY_REPO[repo].revision == info["revision"]
    for key, info in WHISPER_MODELS.items():
        assert hfmodels.get(key).repo == info["repo"] and hfmodels.get(key).revision == info["revision"]

# SPDX-License-Identifier: GPL-3.0-or-later
"""The model catalogue is complete, pinned, and documented in MODELS.md."""

import re
from pathlib import Path

import pytest

from jukeoke_studio.modelzoo import MODELS
from jukeoke_studio.separate import PERMISSIVE_PRESETS, PRESETS

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
    for table in (PRESETS, PERMISSIVE_PRESETS):
        for preset, roles in table.items():
            for role, mid in roles.items():
                assert MODELS[mid].role in ROLE_OF[role], (preset, role, mid)


def test_permissive_presets_allow_commercial_use():
    for roles in PERMISSIVE_PRESETS.values():
        for mid in roles.values():
            assert MODELS[mid].commercial == "yes", mid


def test_models_md_lists_every_model_and_hash():
    text = (ROOT / "MODELS.md").read_text(encoding="utf-8")
    for mid, m in MODELS.items():
        assert f"`{mid}`" in text, f"MODELS.md is missing {mid}"
        for f in m.files:
            assert f.sha256 in text, f"MODELS.md is missing the SHA-256 of {f.filename}"

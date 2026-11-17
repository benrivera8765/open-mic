# SPDX-License-Identifier: GPL-3.0-or-later
"""GPU tests with the real default models (``pytest -m gpu``; downloads ~0.5 GB once)."""

import numpy as np
import pytest
import torch

from jukeoke_studio import quality as q
from jukeoke_studio.modelzoo import MODELS
from jukeoke_studio.separate import PRESETS, Separator

pytestmark = [pytest.mark.gpu,
              pytest.mark.skipif(not torch.cuda.is_available(), reason="needs CUDA")]

SR = 44100


def _song(seconds=12, seed=0):
    """A small synthetic 'song': drum-like clicks, a bass line, a pad and a sung-like tone."""
    rng = np.random.default_rng(seed)
    n = SR * seconds
    t = np.arange(n) / SR
    drums = np.zeros(n)
    for beat in np.arange(0, seconds, 0.5):
        i = int(beat * SR)
        drums[i:i + 2000] += rng.standard_normal(min(2000, n - i)) * np.exp(-np.arange(min(2000, n - i)) / 300)
    bass = 0.3 * np.sin(2 * np.pi * 55 * t)
    pad = 0.1 * sum(np.sin(2 * np.pi * f * t) for f in (220, 277, 330))
    f0 = 440 * (1 + 0.01 * np.sin(2 * np.pi * 5.5 * t))
    voice = 0.2 * sum(np.sin(k * 2 * np.pi * np.cumsum(f0) / SR) / k for k in range(1, 10))
    mono = 0.3 * drums + bass + pad + voice
    return np.stack([mono, 0.9 * mono]).astype(np.float32)


@pytest.fixture(scope="module")
def sep():
    return Separator(device="cuda")


def test_full_preset_runs_fp32_sums_back_and_is_repeatable(sep):
    mix = _song()
    a = sep.separate(mix, SR, "full")
    b = sep.separate(mix, SR, "full")
    total = sum(a.stems[k].astype(np.float64) for k in a.stems)
    assert np.max(np.abs(total - mix)) < 1e-6
    for k in a.stems:
        assert a.stems[k].dtype == np.float32
        assert np.array_equal(a.stems[k], b.stems[k]), f"{k} not deterministic"
    assert a.models == PRESETS["full"]


def test_48k_input_keeps_native_rate(sep):
    from jukeoke_studio.separate import resample

    mix48 = resample(_song(6), SR, 48000)
    res = sep.separate(mix48, 48000, "full")
    assert res.sample_rate == 48000
    assert all(x.shape == mix48.shape for x in res.stems.values())
    assert all(r.resampled for r in res.runs)
    total = sum(res.stems[k].astype(np.float64) for k in res.stems)
    assert q.snr_db(mix48, total - mix48) > 130


def test_default_preset_quality_on_known_stems(sep):
    """A real quality floor, not just plumbing: one ccMixter song with true lead, backing
    and instrument stems (CC BY 2.5, fetched by bench/fetch_eval.py into $JK_BUILD/eval,
    never committed), 45 s from 30 % in, as in the benchmark. Measured on 2026-11-17:
    vocals 22.32 dB, lead 12.46 dB, karaoke mix 14.17 dB, lead residue -17.6 dB
    (docs/benchmarks/separation.md 5.3). The floors sit about 1 dB below, so a wrong
    output index, a lead/backing swap or a broken overlap-add fails here."""
    import os
    from pathlib import Path

    import soundfile as sf

    from jukeoke_studio.modelzoo.download import default_models_dir

    root = Path(os.environ.get("JK_BUILD") or default_models_dir().parent) / "eval" / "ccmixter_voices"
    d = root / "tracks" / "ccm-22364"
    if not (d / "instrumental.wav").exists():
        pytest.skip("ccMixter evaluation set not fetched (bench/fetch_eval.py ccmixter)")
    frames = sf.info(str(d / "lead.wav")).frames
    n = 45 * SR
    start = min(int(0.3 * frames), frames - n)

    def rd(name):
        a, sr = sf.read(str(d / f"{name}.wav"), start=start, frames=n, dtype="float32", always_2d=True)
        assert sr == SR
        return np.ascontiguousarray(a.T)

    lead, back, inst = rd("lead"), rd("backing"), rd("instrumental")
    vocals = (lead.astype(np.float64) + back).astype(np.float32)
    mix = (vocals.astype(np.float64) + inst).astype(np.float32)
    res = sep.separate(mix, SR, "full")
    karaoke_true = mix.astype(np.float64) - lead
    karaoke_est = mix.astype(np.float64) - res.stems["lead_vocal"]
    assert q.sdr(vocals, res.extras["vocals"]) > 21.0
    assert q.sdr(lead, res.stems["lead_vocal"]) > 11.5
    assert q.sdr(karaoke_true, karaoke_est) > 13.0
    assert q.leak_db(karaoke_est, karaoke_true, lead, SR) < -16.0


@pytest.mark.parametrize("model_id", sorted(MODELS))
def test_every_downloaded_checkpoint_matches_its_architecture(model_id):
    """Each registry entry whose weights are already in the models directory loads with
    no missing or unexpected parameters (load_model raises otherwise)."""
    from jukeoke_studio.modelzoo.download import default_models_dir
    from jukeoke_studio.separate import load_model

    spec = MODELS[model_id]
    if not all((default_models_dir() / f.filename).exists() for f in spec.files):
        pytest.skip("weights not downloaded")
    lm = load_model(spec, None, torch.device("cpu"))
    assert sum(p.numel() for p in lm.net.parameters()) > 1_000_000

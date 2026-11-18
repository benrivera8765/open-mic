# SPDX-License-Identifier: GPL-3.0-or-later
"""Separator plumbing with stand-in models (no weights, CPU only)."""

from dataclasses import dataclass

import numpy as np
import pytest
import torch

from jukeoke_studio import separate as S
from jukeoke_studio.quality import sdr

SR = 44100


@dataclass
class _Spec:
    id: str
    sample_rate: int = 44100
    arch: str = "fake"
    overlap: int = 2


class _FakeModel:
    """Linear stand-ins with known outputs: vocals = 0.4 x, lead = 0.7 x,
    instrumental = 0.55 x, drums = 0.3 x (plus the same outputs a real model has)."""

    def __init__(self, model_id):
        self.spec = _Spec(model_id)

    def __call__(self, audio, overlap, batch_size):
        role = self.spec.id
        if role == "vocals":
            return {"vocals": 0.4 * audio}
        if role == "karaoke":
            return {"lead": 0.7 * audio}
        if role == "instrumental":
            return {"instrumental": 0.55 * audio}
        if role == "drums":
            return {"drums": 0.3 * audio, "bass": 0 * audio, "other": 0 * audio, "vocals": 0 * audio}
        if role == "identity":
            return {"vocals": audio.clone()}
        raise KeyError(role)


@pytest.fixture
def sep(monkeypatch):
    s = S.Separator(device="cpu")
    monkeypatch.setattr(s, "model", lambda mid: _FakeModel(mid))
    return s


FAKE = {"vocals": "vocals", "karaoke": "karaoke", "instrumental": "instrumental", "drums": "drums"}


def _music(seconds=3.0, sr=SR, ch=2, seed=0):
    rng = np.random.default_rng(seed)
    return (rng.standard_normal((ch, int(sr * seconds))) * 0.3).astype(np.float32)


@pytest.mark.parametrize("preset", ["full", "clean", "permissive"])
def test_presets_sum_back_to_mix(sep, preset):
    mix = _music()
    res = sep.separate(mix, SR, preset, FAKE)
    assert set(res.stems) == set(S.STEMS)
    total = np.zeros_like(mix)
    for k in S.STEMS:
        assert res.stems[k].dtype == np.float32 and res.stems[k].shape == mix.shape
        total += res.stems[k]
    # float32 sum of four float32 stems: error at float32 rounding level only
    assert np.max(np.abs(total.astype(np.float64) - mix)) < 4 * np.finfo(np.float32).eps


@pytest.mark.parametrize("preset", ["full", "clean", "permissive"])
def test_song_file_round_trip_loses_nothing(sep, preset, tmp_path):
    """Stems -> .jksong (24-bit FLAC, writer's gain rule, exact-residual mode) -> decode:
    the four stems at unity sum back to the original within half an LSB of 24 bits."""
    from jukeoke_studio import jksong

    mix = _music() * 2.5  # peaks well above full scale: some stems need a gain < 1
    res = sep.separate(mix, SR, preset, FAKE)
    path = tmp_path / "t.jksong"
    jksong.write_song(path, stems=[jksong.StemInput(k, None if k == "other" else res.stems[k]) for k in S.STEMS],
                      sample_rate=SR, title="t", artist="t", preset=preset, stems_sum_to_source=True, source=mix)
    song = jksong.open_song(path)
    total = sum(song.read_stem_int(k).T * (float(song.stem(k)["gain"]) / jksong.FULL_SCALE) for k in S.STEMS)
    gains = [float(song.stem(k)["gain"]) for k in S.STEMS]
    assert max(gains) > 1.0  # the headroom rule was exercised
    if preset == "permissive":
        assert not song.read_stem_int("backing_vocals").any()  # silent: no lead/backing split
    lsb = 1 / jksong.FULL_SCALE
    other_gain = float(song.stem("other")["gain"])
    assert np.max(np.abs(total - mix.astype(np.float64))) <= 0.5 * lsb * other_gain * (1 + 1e-9)


def test_full_preset_stem_definitions(sep):
    mix = _music()
    res = sep.separate(mix, SR, "full", FAKE)
    v = 0.4 * mix.astype(np.float64)
    np.testing.assert_allclose(res.stems["lead_vocal"], 0.7 * v, atol=1e-6)
    np.testing.assert_allclose(res.stems["backing_vocals"], 0.3 * v, atol=1e-6)
    np.testing.assert_allclose(res.stems["drums"], 0.3 * (mix - v), atol=1e-6)  # drums from mix - V


def test_clean_preset_uses_instrumental_model(sep):
    mix = _music()
    res = sep.separate(mix, SR, "clean", FAKE)
    m = mix.astype(np.float64)
    inst = 0.55 * m
    np.testing.assert_allclose(res.extras["instrumental"], inst, atol=1e-6)
    np.testing.assert_allclose(res.stems["drums"] + res.stems["other"], inst, atol=1e-6)
    # backing still comes from vocal model + karaoke model; the lead stem takes the rest
    np.testing.assert_allclose(res.stems["backing_vocals"], 0.3 * 0.4 * m, atol=1e-6)
    np.testing.assert_allclose(res.stems["lead_vocal"], (1 - 0.55 - 0.12) * m, atol=1e-6)
    np.testing.assert_allclose(res.extras["lead_estimate"], 0.7 * 0.4 * m, atol=1e-6)


def test_48k_source_only_model_io_is_resampled(sep):
    # 1 kHz tone at 48 kHz through an identity "model" running at 44.1 kHz: the stem
    # comes back at 48 kHz, same length, and the double conversion is near-transparent.
    sr = 48000
    t = np.arange(sr * 3) / sr
    x = np.stack([np.sin(2 * np.pi * 1000 * t), 0.5 * np.sin(2 * np.pi * 997 * t)]).astype(np.float32) * 0.5
    runs = []
    out = sep.run_model("identity", x, sr, runs)["vocals"]
    assert out.shape == x.shape and out.dtype == np.float32
    assert runs[0].resampled is True
    core = slice(sr // 10, -sr // 10)  # ignore the resampler's edge transients
    assert sdr(x[:, core], out[:, core]) > 100  # soxr VHQ round trip, measured ~ 130 dB


def test_native_rate_is_not_resampled(sep):
    x = _music()
    runs = []
    out = sep.run_model("identity", x, SR, runs)["vocals"]
    assert runs[0].resampled is False
    assert np.array_equal(out, x)  # bit-exact: no conversion anywhere


@pytest.mark.parametrize("channels", [1, 3])
def test_channel_layouts(sep, channels):
    x = _music(ch=channels)
    out = sep.run_model("vocals", x, SR)["vocals"]
    assert out.shape == x.shape
    np.testing.assert_allclose(out, 0.4 * x, atol=1e-6)


class _ByRegistry(_FakeModel):
    """Stand-in for a real registry model: behaves like the fake of its role."""

    def __init__(self, model_id):
        from jukeoke_studio.modelzoo import MODELS

        m = MODELS[model_id]
        role = "drums" if m.role == "multistem" else m.role
        super().__init__(role)
        self.spec = _Spec(role, arch=m.arch, overlap=m.overlap)


def test_provenance_lists_only_models_that_ran_in_song_format_shape(monkeypatch):
    """SONG_FORMAT 4.3: provenance.models is a list (task, name, file, sha256, licence,
    source_url) and provenance.resampling lists every resample. An override table with a
    role the preset does not use (BEST_MEASURED's instrumental with "full") must not make
    the song file claim a model that never ran."""
    from jukeoke_studio.modelzoo import MODELS

    sep = S.Separator(device="cpu")
    monkeypatch.setattr(sep, "model", lambda mid: _ByRegistry(mid))
    mix48 = _music(sr=48000)
    res = sep.separate(mix48, 48000, "full", S.BEST_MEASURED)
    assert set(res.models) == set(S.PRESETS["full"])  # no "instrumental"
    prov = res.provenance()
    assert [m["task"] for m in prov["models"]] == list(S.PRESETS["full"])
    for m in prov["models"]:
        spec = MODELS[m["name"]]
        assert m["name"] == S.BEST_MEASURED[m["task"]]
        assert m["sha256"] == spec.files[0].sha256 and m["source_url"] == spec.files[0].url
        assert m["licence"] == spec.licence and m["precision"] == "fp32"
        assert m["overlap"] == (None if spec.arch == "htdemucs" else spec.overlap)
    # The mix once down to the model rate, then vocals, lead and drums once each back up.
    assert [(r["from_hz"], r["to_hz"]) for r in prov["resampling"]] == [(48000, 44100)] + [(44100, 48000)] * 3
    native = sep.separate(_music(), SR, "full").provenance()  # 44.1 kHz source: no resampling
    assert native["resampling"] == [] and len(native["models"]) == 3
    with pytest.raises(ValueError, match="unknown model role"):
        sep.separate(_music(), SR, "full", {"vocal": "bs_roformer_anvuew_ft1"})  # typo


def test_precision_mode_restores_flags():
    before = (torch.backends.cuda.matmul.allow_tf32, torch.backends.cudnn.allow_tf32,
              torch.get_float32_matmul_precision())
    with S.precision_mode("tf32", torch.device("cpu")):
        assert torch.backends.cuda.matmul.allow_tf32
    with S.precision_mode("fp32", torch.device("cpu")):
        assert not torch.backends.cuda.matmul.allow_tf32
        assert not torch.backends.cudnn.allow_tf32
        assert torch.get_float32_matmul_precision() == "highest"
    after = (torch.backends.cuda.matmul.allow_tf32, torch.backends.cudnn.allow_tf32,
             torch.get_float32_matmul_precision())
    assert before == after


def test_unknown_preset_and_precision_rejected():
    with pytest.raises(ValueError):
        S.Separator(device="cpu", precision="int8")
    with pytest.raises(ValueError):
        S.Separator(device="cpu").separate(_music(), SR, "nope")


def test_permissive_preset_puts_all_vocals_in_lead(sep):
    mix = _music()
    res = sep.separate(mix, SR, "permissive", FAKE)
    assert set(res.models) == {"vocals", "drums"}
    v = 0.4 * mix.astype(np.float64)
    np.testing.assert_allclose(res.stems["lead_vocal"], v, atol=1e-6)
    assert not res.stems["backing_vocals"].any()
    np.testing.assert_allclose(res.stems["drums"], 0.3 * (mix - v), atol=1e-6)


class _RecordingModel(_FakeModel):
    """A fake that records the length of what it was given (to see the rate it ran at)."""

    seen: dict = {}

    def __call__(self, audio, overlap, batch_size, **kw):
        _RecordingModel.seen[self.spec.id] = audio.shape[-1]
        return super().__call__(audio, overlap, batch_size)


@pytest.mark.parametrize("preset,expect", [
    ("full", ["mix", "vocals", "lead", "drums"]),
    ("clean", ["mix", "vocals", "lead", "instrumental", "drums"]),
    ("permissive", ["mix", "vocals", "drums"]),
])
def test_48k_each_estimate_is_resampled_exactly_once(monkeypatch, preset, expect):
    """Verifier finding (b): the karaoke model runs on the vocal estimate at the model rate
    and every final estimate goes back to the native rate once; the stems still sum."""
    sep = S.Separator(device="cpu")
    monkeypatch.setattr(sep, "model", lambda mid: _RecordingModel(mid))
    calls = []
    real = S.resample

    def counting(audio, a, b):
        if a != b:
            calls.append((a, b))
        return real(audio, a, b)

    monkeypatch.setattr(S, "resample", counting)
    mix = _music(seconds=2.0, sr=48000)
    res = sep.separate(mix, 48000, preset, FAKE)
    assert calls == [(48000, 44100)] + [(44100, 48000)] * (len(expect) - 1)
    n44 = int(round(mix.shape[1] * 44100 / 48000))
    assert _RecordingModel.seen["karaoke" if preset != "permissive" else "vocals"] == n44
    assert [r["to_hz"] for r in res.resampling] == [44100] + [48000] * (len(expect) - 1)
    total = sum(res.stems[k].astype(np.float64) for k in S.STEMS)
    assert np.max(np.abs(total - mix)) < 4 * np.finfo(np.float32).eps
    assert all(x.shape == mix.shape for x in res.stems.values())


def test_progress_and_cancel_hooks(sep):
    from jukeoke_studio.errors import Cancelled, Hooks

    seen = []
    res = sep.separate(_music(), SR, "full", FAKE, hooks=Hooks(lambda f, m: seen.append((f, m))))
    fr = [f for f, _ in seen]
    assert fr == sorted(fr) and fr[-1] == 1.0 and res.stems
    assert any(m and "lead vocal" in m for _, m in seen)
    h = Hooks()
    h.cancel()
    with pytest.raises(Cancelled):
        sep.separate(_music(), SR, "full", FAKE, hooks=h)


def test_missing_weights_raise_models_missing(tmp_path):
    from jukeoke_studio.errors import ModelsMissing

    sep = S.Separator(models_dir=tmp_path, device="cpu")
    assert sep.missing_models("full") == list(dict.fromkeys(S.PRESETS["full"].values()))
    with pytest.raises(ModelsMissing):
        sep.separate(_music(), SR, "full")
    assert not list(tmp_path.iterdir())  # nothing was downloaded or created

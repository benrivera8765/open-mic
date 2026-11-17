# SPDX-License-Identifier: GPL-3.0-or-later
"""The measuring sticks must be right before anything is measured with them."""

import numpy as np
import pytest

from jukeoke_studio import quality as q

SR = 44100


def _music_like(seconds=20, seed=0):
    """An 'instrumental' (brown-ish noise) and a 'vocal' (harmonic tone with vibrato,
    sung in phrases), both stereo."""
    rng = np.random.default_rng(seed)
    n = SR * seconds
    t = np.arange(n) / SR
    inst = np.cumsum(rng.standard_normal((2, n)), axis=1)
    inst -= inst.mean(axis=1, keepdims=True)
    inst = inst / np.abs(inst).max() * 0.3 + 0.05 * rng.standard_normal((2, n))
    f0 = 220 * (1 + 0.02 * np.sin(2 * np.pi * 5 * t))
    ph = 2 * np.pi * np.cumsum(f0) / SR
    voc = sum(np.sin(k * ph) / k for k in range(1, 15)) * (np.sin(2 * np.pi * 0.25 * t) > 0)
    voc = np.stack([voc, 0.8 * voc]) * 0.2
    return inst, voc, rng


def test_sdr_known_values():
    rng = np.random.default_rng(1)
    ref = rng.standard_normal((2, 10000))
    assert q.sdr(ref, 0.9 * ref) == pytest.approx(20.0, abs=1e-6)  # error = 0.1 ref
    assert q.sdr(ref, np.zeros_like(ref)) == pytest.approx(0.0, abs=1e-6)
    assert q.sdr(ref, ref) > 100


def test_chunk_sdr_skips_silence():
    rng = np.random.default_rng(2)
    ref = rng.standard_normal((2, SR * 4))
    ref[:, : SR * 2] = 0.0  # two silent seconds: must not count
    assert q.chunk_sdr(ref, 0.9 * ref, SR) == pytest.approx(20.0, abs=1e-6)


@pytest.mark.parametrize("leak_db", [-10, -20, -30])
def test_leak_db_measures_known_leak(leak_db):
    inst, voc, rng = _music_like()
    noise = rng.standard_normal(inst.shape) * np.sqrt(np.mean(inst**2)) * 10 ** (-15 / 20)
    est = inst + 10 ** (leak_db / 20) * voc + noise
    assert q.leak_db(est, inst, voc, SR) == pytest.approx(leak_db, abs=1.0)


def test_leak_db_floor_with_independent_error():
    # No vocal at all in the estimate, only independent error at -15 dB re the
    # instrumental: the metric's floor must be far below the levels we report.
    inst, voc, rng = _music_like()
    noise = rng.standard_normal(inst.shape) * np.sqrt(np.mean(inst**2)) * 10 ** (-15 / 20)
    assert q.leak_db(inst + noise, inst, voc, SR) < -35


def test_band_energy_detects_lost_highs():
    rng = np.random.default_rng(3)
    ref = rng.standard_normal((2, SR * 5))
    spec = np.fft.rfft(ref, axis=-1)
    freqs = np.fft.rfftfreq(ref.shape[-1], 1 / SR)
    spec[:, freqs > 8000] *= 0.1  # -20 dB above 8 kHz: "dull cymbals"
    est = np.fft.irfft(spec, n=ref.shape[-1], axis=-1)
    b = q.band_energy_db(est, ref, SR)
    assert b["cymbal_8k_16k"] == pytest.approx(-20.0, abs=0.5)
    for k in ("low_lt250", "mid_250_4k", "high_4k_8k"):
        assert abs(b[k]) < 0.3


def test_24bit_round_trip_is_at_quantisation_level():
    """The song writer's rule (jksong.quantize_stem: gain 1 unless the stem would clip,
    then peak 0.99 and the inverse gain stored): decode error <= half an LSB x gain."""
    from jukeoke_studio import jksong

    rng = np.random.default_rng(4)
    for peak in (0.5, 1.7):  # below full scale (gain 1) and above it (stored gain > 1)
        x = (rng.standard_normal((2, SR)) * 0.1).astype(np.float32)  # |x| < 0.5
        x[0, 10] = peak  # separated stems can exceed full scale
        qi, gain, _ = jksong.quantize_stem(x)
        assert qi.min() >= jksong.INT24_MIN and qi.max() <= jksong.INT24_MAX
        assert (gain == 1.0) == (peak < 1.0)
        y = qi.T.astype(np.float64) * (gain / jksong.FULL_SCALE)
        assert np.max(np.abs(y - x)) <= 0.5 * gain / jksong.FULL_SCALE * (1 + 1e-9)

# SPDX-License-Identifier: GPL-3.0-or-later
"""Key and tempo analysis on generated audio, and shifted-key names.

The audio is synthesised here (harmonic tones, bass, noise drums); nothing is committed.
The full accuracy figures (all progressions, GuitarSet) are in
docs/benchmarks/analysis.md, produced by studio/bench/alignment_bench.py ``keys``.
"""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
pytest.importorskip("librosa")

from jukeoke_studio import analyse  # noqa: E402

SR = 44100


def tone(f, seconds, sr=SR, harmonics=6, rng=None):
    t = np.arange(int(seconds * sr)) / sr
    env = np.minimum(1, t / 0.01) * np.exp(-t * 1.2)
    return env * sum(np.sin(2 * np.pi * f * k * t) / k for k in range(1, harmonics + 1) if f * k < sr / 2)


def cadence(tonic: int, mode: str, seconds_per_chord=1.5, repeats=2):
    """I-IV-V-I (major) or i-iv-V-i (minor, major V) with bass, in a known key."""
    third = 4 if mode == "major" else 3
    sixth = 9 if mode == "major" else 8
    chords = [(0, third, 7), (5, sixth, 12), (7, 11, 14), (0, third, 7)]
    base = 261.63 * 2 ** (tonic / 12)
    out = []
    for _ in range(repeats):
        for ch in chords:
            seg = sum(0.2 * tone(base * 2 ** (n / 12), seconds_per_chord) for n in ch)
            seg = seg + 0.35 * tone(base / 4 * 2 ** (ch[0] / 12), seconds_per_chord, harmonics=3)
            out.append(seg)
    x = np.concatenate(out)
    return (0.5 * x / np.abs(x).max()).astype(np.float32)


@pytest.mark.parametrize("mode", ["major", "minor"])
def test_key_all_tonics(mode):
    wrong = []
    for tonic in range(12):
        k = analyse.estimate_key(cadence(tonic, mode), SR)
        if (k["tonic"], k["mode"]) != (analyse.PITCHES[tonic], mode):
            wrong.append((analyse.PITCHES[tonic], k["name"]))
        assert 0.0 <= k["confidence"] <= 1.0
    assert not wrong, wrong


def test_key_stereo_layouts_and_silence():
    x = cadence(9, "major")
    st = np.stack([x, x])                      # [channels, frames]
    assert analyse.estimate_key(st, SR)["tonic"] == "A"
    assert analyse.estimate_key(st.T, SR)["tonic"] == "A"   # [frames, channels]
    assert analyse.estimate_key(np.zeros(SR * 5, np.float32), SR) is None


def click_track(bpm, seconds=20.0, sr=SR):
    x = np.zeros(int(seconds * sr), np.float32)
    rng = np.random.default_rng(0)
    beat = 60.0 / bpm
    for b in range(int(seconds / beat)):
        i = int(b * beat * sr)
        n = min(len(x) - i, int(0.05 * sr))
        x[i:i + n] += (rng.standard_normal(n) * np.exp(-np.arange(n) / (0.01 * sr))).astype(np.float32)
    return x


@pytest.mark.parametrize("bpm", [80.0, 100.0, 128.0, 150.0])
def test_tempo_click(bpm):
    est = analyse.estimate_tempo(click_track(bpm), SR)
    assert est is not None
    # Within 4 %, allowing the usual double/half ambiguity.
    assert any(abs(est - bpm * f) <= 0.04 * bpm * f for f in (1.0, 2.0, 0.5)), est


def test_analyse_stems_shapes():
    harm = np.stack([cadence(2, "minor")] * 2)
    n = harm.shape[1]
    drums = np.stack([click_track(120.0, seconds=n / SR)[:n]] * 2)
    res = analyse.analyse_stems({"other": harm, "drums": drums,
                                 "lead_vocal": np.zeros_like(harm)}, SR)
    assert res["key"]["tonic"] == "D" and res["key"]["mode"] == "minor"
    assert res["key"]["name"] == "D minor"
    assert res["tempo_bpm"] == pytest.approx(120.0, rel=0.04) or res["tempo_bpm"] == pytest.approx(60.0, rel=0.04)


def test_describe_shift():
    a = {"tonic": "A", "mode": "major"}
    assert analyse.describe_shift(a, 2) == "A major → B major (+2)"
    assert analyse.describe_shift(a, 2, arrow="->") == "A major -> B major (+2)"
    assert analyse.describe_shift(a, -3) == "A major → F# major (-3)"
    assert analyse.describe_shift(a, 1) == "A major → Bb major (+1)"
    assert analyse.describe_shift(a, 12) == "A major → A major (+12)"
    assert analyse.describe_shift(a, 0) == "A major (original key)"
    assert analyse.describe_shift(("C#", "minor"), 2) == "C# minor → Eb minor (+2)"
    assert analyse.describe_shift(("A#", "major"), 0, unicode=True) == "B♭ major (original key)"
    assert analyse.describe_shift(("C", "major"), 1, spelling="sharps") == "C major → C# major (+1)"
    assert analyse.describe_shift(None, -2) == "-2 semitones"
    assert analyse.describe_shift(None, 1) == "+1 semitone"
    assert analyse.shift_key("B", "minor", 1) == ("C", "minor")
    assert analyse.shift_key("C", "major", -13) == ("B", "major")

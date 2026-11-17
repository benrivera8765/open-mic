# SPDX-License-Identifier: GPL-3.0-or-later
"""Objective audio measures used by the separation benchmark and the tests.

All functions take float arrays shaped ``[channels, frames]`` and work in float64.

* :func:`sdr` - utterance-level signal-to-distortion ratio, the "SDR" of the Music
  Demixing Challenge and of MVSEP's leaderboards (``uSDR`` when averaged over tracks):
  ``10 log10(sum(ref^2) / sum((ref - est)^2))`` over all channels and samples.
* :func:`chunk_sdr` - the same ratio on 1-second chunks, median over the chunks whose
  reference is not silent (a BSS-Eval-free stand-in for SiSEC's ``cSDR``, which is the
  median over 1 s frames of BSS-Eval v4 SDR; dataset value = median over tracks).
* :func:`leak_db` - how much of a *specific* source is still present in an estimate,
  in dB relative to that source (e.g. ghost vocal left in an instrumental).
* :func:`band_energy_db` - estimate-vs-reference energy per frequency band (dull
  cymbals show up as a negative 8-16 kHz value).
"""

from __future__ import annotations

import numpy as np

EPS = 1e-7  # same delta as the MDX challenge / MVSEP SDR

BANDS = {  # name: (low Hz, high Hz)
    "low_lt250": (0.0, 250.0),
    "mid_250_4k": (250.0, 4000.0),
    "high_4k_8k": (4000.0, 8000.0),
    "cymbal_8k_16k": (8000.0, 16000.0),
}


def sdr(ref: np.ndarray, est: np.ndarray) -> float:
    ref = np.asarray(ref, dtype=np.float64)
    est = np.asarray(est, dtype=np.float64)
    num = np.sum(ref**2) + EPS
    den = np.sum((ref - est) ** 2) + EPS
    return float(10 * np.log10(num / den))


def chunk_sdr(ref: np.ndarray, est: np.ndarray, sr: int, seconds: float = 1.0,
              silence_db: float = -80.0) -> float:
    """Median over ``seconds``-long chunks of :func:`sdr`, skipping silent-reference chunks."""
    ref = np.asarray(ref, dtype=np.float64)
    est = np.asarray(est, dtype=np.float64)
    n = int(sr * seconds)
    vals = []
    for s in range(0, ref.shape[-1] - n + 1, n):
        r, e = ref[..., s:s + n], est[..., s:s + n]
        if 10 * np.log10(np.mean(r**2) + 1e-20) < silence_db:
            continue
        vals.append(sdr(r, e))
    return float(np.median(vals)) if vals else float("nan")


def energy_db(x: np.ndarray, ref: np.ndarray) -> float:
    """Energy of ``x`` relative to ``ref``, in dB."""
    return float(10 * np.log10((np.sum(np.asarray(x, np.float64) ** 2) + 1e-30)
                               / (np.sum(np.asarray(ref, np.float64) ** 2) + 1e-30)))


def _n_frames(n: int, n_fft: int, hop: int) -> int:
    return 1 + max(0, (n - n_fft) // hop)


def _stft(x: np.ndarray, n_fft: int, hop: int, f0: int = 0, f1: int | None = None) -> np.ndarray:
    """STFT frames ``f0..f1`` of ``[channels, samples]`` -> complex ``[channels, bins, frames]``.

    Hann window, no centring. Computed for a frame range so long tracks never need the
    whole spectrogram in memory.
    """
    x = np.asarray(x, dtype=np.float64)
    f1 = _n_frames(x.shape[-1], n_fft, hop) if f1 is None else f1
    win = np.hanning(n_fft + 1)[:-1]
    idx = np.arange(n_fft)[None, :] + hop * np.arange(f0, f1)[:, None]
    frames = x[..., idx] * win  # [ch, frames, n_fft]
    return np.fft.rfft(frames, axis=-1).transpose(0, 2, 1)


def leak_db(est: np.ndarray, target: np.ndarray, source: np.ndarray, sr: int,
            segment_seconds: float = 4.0, n_fft: int = 2048, hop: int = 512) -> float:
    """Energy of ``source`` that leaked into ``est``, in dB relative to ``source``.

    The estimation error ``e = est - target`` is projected, per STFT bin and per
    ``segment_seconds`` segment, onto ``source`` with a least-squares complex gain:
    ``R = g * S``, ``g = sum_t E conj(S) / sum_t |S|^2``. Returns
    ``10 log10(sum |R|^2 / sum |S|^2)``. A residual that is merely *correlated* with
    the source counts (that is the ghost you hear); independent error mostly does not,
    down to a floor of roughly ``error energy / frames per segment``
    (``tests/test_quality.py`` measures the floor and the accuracy on known leaks).
    """
    err = np.asarray(est, np.float64) - np.asarray(target, np.float64)
    src = np.asarray(source, np.float64)
    n = _n_frames(src.shape[-1], n_fft, hop)
    seg = max(1, int(round(segment_seconds * sr / hop)))
    leak = total = 0.0
    for t0 in range(0, n, seg):
        e = _stft(err, n_fft, hop, t0, min(n, t0 + seg))
        s = _stft(src, n_fft, hop, t0, min(n, t0 + seg))
        ss = np.sum(np.abs(s) ** 2, axis=-1)
        g = np.sum(e * np.conj(s), axis=-1) / (ss + 1e-20)
        leak += float(np.sum(np.abs(g) ** 2 * ss))
        total += float(np.sum(ss))
    return float(10 * np.log10(leak / (total + 1e-30) + 1e-30))


def band_energy_db(est: np.ndarray, ref: np.ndarray, sr: int, n_fft: int = 4096,
                   hop: int = 1024) -> dict[str, float]:
    """``10 log10(E_band(est) / E_band(ref))`` for each band in :data:`BANDS`."""
    est = np.asarray(est, np.float64)
    ref = np.asarray(ref, np.float64)
    n = _n_frames(ref.shape[-1], n_fft, hop)
    pe = np.zeros(n_fft // 2 + 1)
    pr = np.zeros(n_fft // 2 + 1)
    for t0 in range(0, n, 256):
        t1 = min(n, t0 + 256)
        pe += np.sum(np.abs(_stft(est, n_fft, hop, t0, t1)) ** 2, axis=(0, 2))
        pr += np.sum(np.abs(_stft(ref, n_fft, hop, t0, t1)) ** 2, axis=(0, 2))
    freqs = np.fft.rfftfreq(n_fft, 1.0 / sr)
    out = {}
    for name, (lo, hi) in BANDS.items():
        m = (freqs >= lo) & (freqs < hi)
        out[name] = float(10 * np.log10((pe[m].sum() + 1e-30) / (pr[m].sum() + 1e-30)))
    return out


def snr_db(ref: np.ndarray, err: np.ndarray) -> float:
    """Signal-to-noise ratio of ``ref`` over an error signal, dB."""
    return energy_db(ref, err)


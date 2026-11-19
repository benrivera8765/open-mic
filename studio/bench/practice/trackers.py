# SPDX-License-Identifier: GPL-3.0-or-later
"""Neural pitch trackers for the guide benchmark (evaluation only, not shipped).

Each wrapper takes mono float32 audio at any rate and returns ``(t, f0_hz, conf)`` at the
tracker's own frame rate, with the tracker's recommended voicing decision applied
(``f0_hz`` = 0 where it says unvoiced). The curves then go through the SAME note
segmentation as the Rust pYIN curve (``guide_wav --curve``), so only the tracker differs.

Installed in an overlay venv (``$JK_BUILD/practice-guide/venv``) that reuses the Studio
venv's torch; nothing here is part of the Studio. Licences: docs/benchmarks/practice-guide.md.
"""

from __future__ import annotations

import time

import numpy as np
import soxr

FMIN, FMAX = 60.0, 1100.0


def to16k(x: np.ndarray, sr: int) -> np.ndarray:
    return x if sr == 16000 else soxr.resample(x, sr, 16000, quality="VHQ").astype(np.float32)


def crepe(x: np.ndarray, sr: int, device: str, model: str = "full"):
    import torch
    import torchcrepe

    a = torch.from_numpy(to16k(x, sr))[None]
    with torch.inference_mode():
        f0, per = torchcrepe.predict(a, 16000, hop_length=160, fmin=FMIN, fmax=FMAX, model=model,
                                     batch_size=1024, device=device, return_periodicity=True,
                                     decoder=torchcrepe.decode.viterbi, pad=True)
        per = torchcrepe.filter.median(per, 3)
        # README recipe: periodicity threshold 0.21 after a 3-frame median. Its silence
        # gate (Silence(-60), A-weighted) is left out: it read a -18.7 dBFS RMS vocal (the
        # demo song's) as below -60 and silenced all of it; the note segmentation has its
        # own level gate relative to the vocal, the same for every tracker.
    f0 = f0[0].cpu().numpy()
    per = per[0].cpu().numpy()
    f0 = np.where(per >= 0.21, f0, 0.0)
    t = np.arange(len(f0)) * 0.01
    return t, f0, per


def pesto(x: np.ndarray, sr: int, device: str):
    import torch
    import pesto as _p

    a = torch.from_numpy(x.astype(np.float32))
    with torch.inference_mode():
        t, f0, conf, _act = _p.predict(a, sr, step_size=10.0, convert_to_freq=True)
    t = t.cpu().numpy() / 1000.0 if t.max() > 1000 else t.cpu().numpy()
    f0 = f0.cpu().numpy()
    conf = conf.cpu().numpy()
    # PESTO has no voicing decision of its own; its confidence thresholded at 0.5 is the
    # usual choice (the paper's evaluation uses the confidence as voicing).
    f0 = np.where((conf >= 0.5) & (f0 >= FMIN) & (f0 <= FMAX), f0, 0.0)
    return t, f0, conf


_fcpe_model = {}


def fcpe(x: np.ndarray, sr: int, device: str):
    import torch
    from torchfcpe import spawn_bundled_infer_model

    if device not in _fcpe_model:
        _fcpe_model[device] = spawn_bundled_infer_model(device=device)
    m = _fcpe_model[device]
    a16 = to16k(x, sr)
    n = len(a16) // 160 + 1
    a = torch.from_numpy(a16)[None, :, None].to(device)
    with torch.inference_mode():
        f0 = m.infer(a, sr=16000, decoder_mode="local_argmax", threshold=0.006, f0_min=FMIN, f0_max=FMAX,
                     interp_uv=False, output_interp_target_length=n)
    f0 = f0.squeeze().float().cpu().numpy()
    conf = (f0 > 0).astype(np.float32)  # FCPE returns its voicing decision only
    t = np.arange(len(f0)) * 0.01
    return t, f0, conf


def swiftf0(x: np.ndarray, sr: int, device: str):
    from swift_f0 import SwiftF0

    # swift-f0 0.3: ONNX on the CPU, 16 ms hop; "voiced when its confidence is at least 0.5".
    r = SwiftF0().detect(x.astype(np.float32), sr, fmin=FMIN, fmax=FMAX)
    f0 = np.where(r.confidence >= 0.5, r.pitch_hz, 0.0)
    return np.asarray(r.timestamps), f0, np.asarray(r.confidence)


TRACKERS = {"crepe": crepe, "pesto": pesto, "fcpe": fcpe, "swiftf0": swiftf0}


def run(name: str, x: np.ndarray, sr: int, device: str):
    """Runs one tracker; returns (t, f0, conf, seconds)."""
    import torch

    if device.startswith("cuda"):
        torch.cuda.synchronize()
    t0 = time.perf_counter()
    t, f0, conf = TRACKERS[name](x, sr, device)
    if device.startswith("cuda"):
        torch.cuda.synchronize()
    return np.asarray(t, float), np.asarray(f0, float), np.asarray(conf, float), time.perf_counter() - t0


def write_curve(path, t, f0, conf):
    with open(path, "w", encoding="ascii") as f:
        f.write("t,f0_hz,conf\n")
        for a, b, c in zip(t, f0, conf):
            f.write(f"{a:.4f},{b:.3f},{c:.4f}\n")

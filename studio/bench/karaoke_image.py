# SPDX-License-Identifier: GPL-3.0-or-later
"""Loudness and stereo image of the karaoke mix against the true instrumental
(WORK_ORDER 4.1: "the stereo image and loudness unchanged"). Results:
docs/benchmarks/separation.md section 6.9.

Uses only the separation benchmark's cached estimates (no GPU, no model run):

- ccMixter mixes (18 tracks, 45 s excerpts at 30 %): mix = lead + backing + instrumental.
  Backing on (the default karaoke mix): "full" = mix - lead_hat, true = mix - lead
  (= instrumental + backing); "clean" = inst_hat + (vocals_hat - lead_hat). Backing off:
  "full" = mix - vocals_hat, true = instrumental. Estimates at the shipped overlaps
  (cache folders ``*-ovdef-ex45``).
- MUSDB18-HQ test (50 tracks, 60 s excerpts at 30 %): MUSDB has no lead/backing split,
  so the lead is all vocals: "full" = mix - vocals_hat (bs_roformer_anvuew_ft1, overlap 2
  as shipped), true = mix - vocals.

Measures, per track, estimate minus truth:

- integrated loudness, ITU-R BS.1770-4 / EBU R128 (K-weighting, 400 ms blocks with 75 %
  overlap, -70 LUFS absolute and -10 LU relative gates), implemented here and checked
  against the BS.1770 48 kHz filter coefficients and EBU Tech 3341 sine cases;
- inter-channel correlation (zero lag, whole excerpt);
- side-to-mid energy ratio, M = (L+R)/2, S = (L-R)/2, and the mid and side levels;
- left/right balance (energy ratio).

    python -I studio/bench/karaoke_image.py [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import lfilter

SR = 44100
HERE = Path(__file__).resolve().parent


def jk_build() -> Path:
    env = os.environ.get("JK_BUILD")
    return Path(env) if env else HERE.parents[2] / "jukeoke-build"


EVAL = jk_build() / "eval"
CACHE = EVAL / "cache"


# ----------------------------------------------------------------- BS.1770-4 loudness

def k_weighting(rate: int):
    """The two BS.1770 K-weighting biquads for any rate (pre-filter shelf, RLB high-pass)."""
    # Brecht De Man's parametrisation of the two biquads (it reproduces the standard's
    # 48 kHz coefficients; checked in self_check). Stage 1: high shelf, about +4 dB.
    g, q, fc = 3.999843853973347, 0.7071752369554196, 1681.974450955533
    k = math.tan(math.pi * fc / rate)
    vh = 10 ** (g / 20)
    vb = vh ** 0.4996667741545416
    a0 = 1 + k / q + k * k
    b1 = [(vh + vb * k / q + k * k) / a0, 2 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0]
    a1 = [1.0, 2 * (k * k - 1) / a0, (1 - k / q + k * k) / a0]
    # Stage 2: RLB high-pass at about 38 Hz; numerator [1, -2, 1] as in the standard.
    q2, fc2 = 0.5003270373238773, 38.13547087602444
    k = math.tan(math.pi * fc2 / rate)
    a0 = 1 + k / q2 + k * k
    b2 = [1.0, -2.0, 1.0]
    a2 = [1.0, 2 * (k * k - 1) / a0, (1 - k / q2 + k * k) / a0]
    return (np.array(b1), np.array(a1)), (np.array(b2), np.array(a2))


def integrated_loudness(x: np.ndarray, rate: int = SR) -> float:
    """x: (channels, frames), L and R weighted 1.0. Returns LUFS (-inf when gated out)."""
    (b1, a1), (b2, a2) = k_weighting(rate)
    y = lfilter(b2, a2, lfilter(b1, a1, x.astype(np.float64), axis=1), axis=1)
    block, step = int(round(0.4 * rate)), int(round(0.1 * rate))
    n = (y.shape[1] - block) // step + 1
    if n < 1:
        return -math.inf
    sq = y * y
    cs = np.concatenate([np.zeros((y.shape[0], 1)), np.cumsum(sq, axis=1)], axis=1)
    starts = np.arange(n) * step
    z = (cs[:, starts + block] - cs[:, starts]) / block  # (channels, blocks)
    zsum = z.sum(axis=0)
    with np.errstate(divide="ignore"):
        lj = -0.691 + 10 * np.log10(zsum)
    keep = lj > -70.0
    if not keep.any():
        return -math.inf
    gamma_r = -0.691 + 10 * math.log10(zsum[keep].mean()) - 10.0
    keep &= lj > gamma_r
    return -0.691 + 10 * math.log10(zsum[keep].mean())


def self_check() -> dict:
    """BS.1770 48 kHz coefficients and EBU Tech 3341 cases 1-2 (stereo 1 kHz sine at -23 / -33 dBFS)."""
    (b1, a1), (b2, a2) = k_weighting(48000)
    ref1b = np.array([1.53512485958697, -2.69169618940638, 1.19839281085285])
    ref1a = np.array([1.0, -1.69065929318241, 0.73248077421585])
    ref2a = np.array([1.0, -1.99004745483398, 0.99007225036621])
    coeff_err = float(max(np.abs(b1 - ref1b).max(), np.abs(a1 - ref1a).max(), np.abs(a2 - ref2a).max()))
    out = {"k_weighting_48k_max_coeff_error": coeff_err}
    for rate in (44100, 48000):
        t = np.arange(20 * rate) / rate
        for level in (-23.0, -33.0):
            s = 10 ** (level / 20) * np.sin(2 * math.pi * 1000 * t)
            out[f"sine_1k_{int(level)}dBFS_{rate}"] = round(integrated_loudness(np.stack([s, s]), rate), 3)
    return out


# ----------------------------------------------------------------- stereo image

def image(x: np.ndarray) -> dict:
    x = x.astype(np.float64)
    l, r = x[0], x[-1]
    el, er = float(np.dot(l, l)), float(np.dot(r, r))
    m, s = (l + r) / 2, (l - r) / 2
    em, es = float(np.dot(m, m)), float(np.dot(s, s))
    db = lambda v: 10 * math.log10(v) if v > 0 else -math.inf  # noqa: E731
    return {
        "corr": float(np.dot(l, r) / math.sqrt(el * er)) if el > 0 and er > 0 else float("nan"),
        "side_mid_db": db(es) - db(em) if em > 0 else float("nan"),
        "mid_db": db(em / len(m)),
        "side_db": db(es / len(s)),
        "balance_db": db(el) - db(er) if er > 0 else float("nan"),
    }


def compare(est: np.ndarray, true: np.ndarray) -> dict:
    ie, it = image(est), image(true)
    le, lt = integrated_loudness(est), integrated_loudness(true)
    mono = it["side_mid_db"] < -60  # true mix (nearly) mono: side/mid differences meaningless
    return {
        "lufs_true": lt, "lufs_est": le, "d_lufs": le - lt,
        "corr_true": it["corr"], "corr_est": ie["corr"], "d_corr": ie["corr"] - it["corr"],
        "sm_true_db": it["side_mid_db"], "sm_est_db": ie["side_mid_db"],
        "d_sm_db": None if mono else ie["side_mid_db"] - it["side_mid_db"],
        "d_mid_db": ie["mid_db"] - it["mid_db"],
        "d_side_db": None if mono else ie["side_db"] - it["side_db"],
        "d_balance_db": ie["balance_db"] - it["balance_db"],
        "true_mono": mono,
    }


# ----------------------------------------------------------------- data

def _window(frames: int, excerpt: float) -> tuple[int, int]:
    n = int(excerpt * SR)
    if frames <= n:
        return 0, frames
    return min(int(0.3 * frames), frames - n), n


def _read(path: Path, start: int, n: int) -> np.ndarray:
    a, sr = sf.read(str(path), start=start, frames=n, dtype="float32", always_2d=True)
    assert sr == SR, path
    a = np.ascontiguousarray(a.T)
    return np.vstack([a, a]) if a.shape[0] == 1 else a


def _npy(kind: str, model: str, track: str) -> np.ndarray:
    a = np.load(CACHE / kind / model / f"{track}.npy")
    return np.vstack([a, a]) if a.shape[0] == 1 else a


def ccmixter_rows() -> list[dict]:
    root = EVAL / "ccmixter_voices"
    meta = json.loads((root / "tracks.json").read_text(encoding="utf-8"))
    rows = []
    for name in sorted(r["track"] for r in meta):
        d = root / "tracks" / name
        if not (d / "instrumental.wav").exists():
            continue
        s, n = _window(sf.info(str(d / "lead.wav")).frames, 45.0)
        lead, back, inst = (_read(d / f"{k}.wav", s, n) for k in ("lead", "backing", "instrumental"))
        f64 = np.float64
        mix = (lead.astype(f64) + back + inst).astype(np.float32)
        v_hat = _npy("ccm-vocals-ovdef-ex45", "bs_roformer_anvuew_ft1", name)
        l_hat = _npy("ccm-lead-bs_roformer_anvuew_ft1-ovdef-ex45", "bs_karaoke_anvuew", name)
        i_hat = _npy("ccm-inst-ovdef-ex45", "bs_inst_resurrection_unwa", name)
        true_on = (inst.astype(f64) + back).astype(np.float32)
        full_on = (mix.astype(f64) - l_hat).astype(np.float32)
        clean_on = (i_hat.astype(f64) + (v_hat.astype(f64) - l_hat)).astype(np.float32)
        full_off = (mix.astype(f64) - v_hat).astype(np.float32)
        rows.append({
            "set": "ccmixter", "track": name,
            "lead_level_db": 10 * math.log10(float(np.sum(lead.astype(f64) ** 2)) / float(np.sum(mix.astype(f64) ** 2))),
            "full_on": compare(full_on, true_on),
            "clean_on": compare(clean_on, true_on),
            "full_off": compare(full_off, inst),
        })
        print(name, {k: round(rows[-1][k]["d_lufs"], 2) for k in ("full_on", "clean_on", "full_off")}, flush=True)
    return rows


def musdb_rows() -> list[dict]:
    root = EVAL / "musdb18hq" / "test"
    rows = []
    for name in sorted(p.name for p in root.iterdir() if p.is_dir()):
        d = root / name
        s, n = _window(sf.info(str(d / "mixture.wav")).frames, 60.0)
        mix, voc = _read(d / "mixture.wav", s, n), _read(d / "vocals.wav", s, n)
        v_hat = _npy("musdb-vocals-ex60", "bs_roformer_anvuew_ft1", name)
        f64 = np.float64
        true = (mix.astype(f64) - voc).astype(np.float32)
        est = (mix.astype(f64) - v_hat).astype(np.float32)
        ev, em = float(np.sum(voc.astype(f64) ** 2)), float(np.sum(mix.astype(f64) ** 2))
        rows.append({"set": "musdb", "track": name, "vocal_level_db": 10 * math.log10(ev / em) if ev > 0 else -math.inf,
                     "full": compare(est, true)})
    return rows


def summarise(rows: list[dict], variant: str) -> dict:
    out = {"tracks": len(rows)}
    for key in ("d_lufs", "d_corr", "d_sm_db", "d_mid_db", "d_side_db", "d_balance_db"):
        vals = [(r[variant][key], r["track"]) for r in rows if r[variant][key] is not None and math.isfinite(r[variant][key])]
        if not vals:
            continue
        xs = np.array([v for v, _ in vals])
        worst = max(vals, key=lambda v: abs(v[0]))
        out[key] = {"n": len(vals), "mean": float(xs.mean()), "mean_abs": float(np.abs(xs).mean()),
                    "median": float(np.median(xs)), "worst": float(worst[0]), "worst_track": worst[1]}
    out["true_mono_tracks"] = sum(1 for r in rows if r[variant]["true_mono"])
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    report = {"self_check": self_check()}
    print("self-check:", report["self_check"], flush=True)
    ccm = ccmixter_rows()
    mus = musdb_rows()
    report["summary"] = {
        "ccmixter_full_backing_on": summarise(ccm, "full_on"),
        "ccmixter_clean_backing_on": summarise(ccm, "clean_on"),
        "ccmixter_full_backing_off": summarise(ccm, "full_off"),
        "musdb_full": summarise(mus, "full"),
        "musdb_full_vocals_present": summarise([r for r in mus if r["vocal_level_db"] > -60], "full"),
    }
    report["rows"] = ccm + mus
    print(json.dumps(report["summary"], indent=1))
    if a.json:
        a.json.write_text(json.dumps(report, indent=1, default=float), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())

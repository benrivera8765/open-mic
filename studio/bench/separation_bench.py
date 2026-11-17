# SPDX-License-Identifier: GPL-3.0-or-later
"""Reproducible separation benchmark for Jukeoke (results: docs/benchmarks/separation.md).

Evaluation data comes from ``fetch_eval.py`` (MUSDB18-HQ test split, ccMixter lead /
backing set); results are written as JSON to ``$JK_BUILD/eval/results`` and turned
into Markdown tables by the ``report`` command. Every number is measured on this
machine; nothing is copied from a leaderboard.

Commands (run from the repo root with the Studio venv, ``python -I``)::

    separation_bench.py musdb     --models A,B,...   vocal / instrumental / 4-stem models
    separation_bench.py drums     --models A,B --vocals-model V
    separation_bench.py karaoke   --models A,B --vocals-model V
    separation_bench.py presets   --models I1,I2      full vs clean karaoke mixes (ccMixter)
    separation_bench.py precision --model A          fp32 vs tf32 / fp16 / bf16
    separation_bench.py overlap   --model A          overlap 1/2/4/8
    separation_bench.py lossless                      "nothing lost" check of the presets
    separation_bench.py resample48                    48 kHz source path
    separation_bench.py timing                        full + clean preset on a 4-minute song
    separation_bench.py speed     --models A,B,...   GPU s per audio minute on one 60 s clip
    separation_bench.py batch     --model A          chunks per network call: speed, VRAM, output change
    separation_bench.py report                        Markdown tables from the JSON files

Excerpts: unless ``--excerpt 0``, each track is evaluated on one window of
``--excerpt`` seconds starting at 30 % of its length (deterministic, unbiased by
content), the same window for every model. MVSEP's Multisong set also uses
one-minute clips.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics as st
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))  # the jukeoke_studio package (run with python -I)

from jukeoke_studio import quality as q  # noqa: E402
from jukeoke_studio.modelzoo import registry  # noqa: E402
from jukeoke_studio.separate import PRESETS, Separator, resample  # noqa: E402

SR = 44100
#: An excerpt whose true vocals are this far below the mix has no vocals to separate
#: (MUSDB "Skelpolu - Resurrection" at our window is digital silence); vocal and
#: instrumental separation scores are undefined there and the track is left out of them.
NO_VOCALS_DB = -60.0


def jk_build() -> Path:
    env = os.environ.get("JK_BUILD")
    return Path(env) if env else HERE.parents[2] / "jukeoke-build"


EVAL = jk_build() / "eval"
RESULTS = EVAL / "results"
CACHE = EVAL / "cache"


# ------------------------------------------------------------------------------ data

def _window(frames: int, excerpt: float) -> tuple[int, int]:
    if excerpt <= 0 or frames <= excerpt * SR:
        return 0, frames
    n = int(excerpt * SR)
    start = int(0.3 * frames)
    start = min(start, frames - n)
    return start, n


def _read(path: Path, start: int, n: int) -> np.ndarray:
    a, sr = sf.read(str(path), start=start, frames=n, dtype="float32", always_2d=True)
    assert sr == SR, f"{path}: {sr} Hz"
    return np.ascontiguousarray(a.T)


def musdb_tracks(excerpt: float, limit: int | None = None):
    """Yield (name, dict of stems) for the MUSDB18-HQ test split."""
    root = EVAL / "musdb18hq" / "test"
    names = sorted(p.name for p in root.iterdir() if p.is_dir())
    for name in names[:limit]:
        d = root / name
        frames = sf.info(str(d / "mixture.wav")).frames
        s, n = _window(frames, excerpt)
        stems = {k: _read(d / f"{k}.wav", s, n) for k in ("mixture", "vocals", "drums", "bass", "other")}
        stems["instrumental"] = (stems["drums"].astype(np.float64) + stems["bass"] + stems["other"]).astype(np.float32)
        yield name, stems


def ccmixter_tracks(excerpt: float, mixes_only: bool = False):
    root = EVAL / "ccmixter_voices"
    meta = {r["track"]: r for r in json.loads((root / "tracks.json").read_text(encoding="utf-8"))}
    for name in sorted(meta):
        d = root / "tracks" / name
        has_mix = (d / "instrumental.wav").exists()
        if mixes_only and not has_mix:
            continue
        frames = sf.info(str(d / "lead.wav")).frames
        s, n = _window(frames, excerpt)
        stems = {k: _read(d / f"{k}.wav", s, n) for k in ("lead", "backing")}
        if has_mix:
            stems["instrumental"] = _read(d / "instrumental.wav", s, n)
        yield name, stems, meta[name]


# ---------------------------------------------------------------------------- helpers

def env_info(sep: Separator | None = None) -> dict:
    info = {
        "torch": torch.__version__, "cuda": torch.version.cuda,
        "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
        "python": platform.python_version(), "date": time.strftime("%Y-%m-%d %H:%M"),
    }
    if sep:
        info.update(precision=sep.precision, overlap=sep.overlap, batch_size=sep.batch_size)
    return info


def save(name: str, data: dict) -> Path:
    RESULTS.mkdir(parents=True, exist_ok=True)
    p = RESULTS / f"{name}.json"
    p.write_text(json.dumps(data, indent=1), encoding="utf-8")
    print(f"-> {p}", flush=True)
    return p


def load(name: str) -> dict | None:
    p = RESULTS / f"{name}.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


class Meter:
    """GPU seconds and peak VRAM of the model calls made through a Separator."""

    def __init__(self):
        self.runs = []
        self.peak_alloc = 0
        self.peak_reserved = 0

    def reset_peak(self):
        if torch.cuda.is_available():
            torch.cuda.reset_peak_memory_stats()

    def update_peak(self):
        if torch.cuda.is_available():
            self.peak_alloc = max(self.peak_alloc, torch.cuda.max_memory_allocated())
            self.peak_reserved = max(self.peak_reserved, torch.cuda.max_memory_reserved())

    def summary(self) -> dict:
        audio = sum(r.input_seconds for r in self.runs)
        gpu = sum(r.seconds for r in self.runs)
        return {"gpu_seconds": round(gpu, 2), "audio_seconds": round(audio, 1),
                "seconds_per_audio_minute": round(gpu / (audio / 60), 2) if audio else None,
                "peak_vram_allocated_mb": self.peak_alloc // 2**20,
                "peak_vram_reserved_mb": self.peak_reserved // 2**20}


def mean(xs):
    xs = [x for x in xs if x is not None and np.isfinite(x)]
    return round(float(np.mean(xs)), 3) if xs else None


def median(xs):
    xs = [x for x in xs if x is not None and np.isfinite(x)]
    return round(float(np.median(xs)), 3) if xs else None


def inst_metrics(est_inst, true_inst, true_vocals, sr=SR) -> dict:
    return {
        "inst_sdr": q.sdr(true_inst, est_inst),
        "inst_csdr": q.chunk_sdr(true_inst, est_inst, sr),
        "vocal_leak_db": q.leak_db(est_inst, true_inst, true_vocals, sr),
        "bands_db": q.band_energy_db(est_inst, true_inst, sr),
    }


def summarise(rows: list[dict], keys: list[str]) -> dict:
    out = {}
    for k in keys:
        vals = [r.get(k) for r in rows]
        out[k] = {"mean": mean(vals), "median": median(vals)}
    band_rows = [r["bands_db"] for r in rows if "bands_db" in r]
    if band_rows:
        out["bands_db_mean"] = {b: mean([br[b] for br in band_rows]) for b in band_rows[0]}
    return out


def _cache_path(kind: str, model: str, track: str, excerpt: float) -> Path:
    return CACHE / f"{kind}-ex{int(excerpt)}" / model / f"{track}.npy"


# --------------------------------------------------------------------------- commands

def cmd_musdb(a):
    """Vocal, instrumental and multi-stem models on MUSDB18-HQ test."""
    for mid in a.models.split(","):
        spec = registry.get(mid)
        sep = Separator(precision=a.precision, overlap=a.overlap, batch_size=a.batch_size)
        meter = Meter()
        rows = []
        for name, s in musdb_tracks(a.excerpt, a.limit):
            cached = _cache_path("musdb-vocals", mid, name, a.excerpt)
            if a.from_cache and cached.exists():
                # Re-score a vocal estimate saved by an earlier (interrupted) run.
                out = {"vocals": np.load(cached)}
            else:
                meter.reset_peak()
                out = sep.run_model(mid, s["mixture"], SR, meter.runs)
                meter.update_peak()
            mix64 = s["mixture"].astype(np.float64)
            row = {"track": name, "vocal_level_db": q.energy_db(s["vocals"], s["mixture"])}
            if "vocals" in out:
                v = out["vocals"]
                inst = (mix64 - v).astype(np.float32)
                row["vocals_sdr"] = q.sdr(s["vocals"], v)
                row["vocals_csdr"] = q.chunk_sdr(s["vocals"], v, SR)
                if a.cache:
                    p = _cache_path("musdb-vocals", mid, name, a.excerpt)
                    p.parent.mkdir(parents=True, exist_ok=True)
                    np.save(p, v)
            elif "instrumental" in out:
                inst = out["instrumental"]
                v = (mix64 - inst).astype(np.float32)
                row["vocals_sdr"] = q.sdr(s["vocals"], v)
                row["vocals_csdr"] = q.chunk_sdr(s["vocals"], v, SR)
            else:
                inst = None
            if inst is not None:
                row.update(inst_metrics(inst, s["instrumental"], s["vocals"]))
            for stem in ("drums", "bass", "other"):
                if stem in out and spec.role == "multistem":
                    row[f"{stem}_sdr"] = q.sdr(s[stem], out[stem])
            if row["vocal_level_db"] < NO_VOCALS_DB:  # nothing to separate in this excerpt
                for k in ("vocals_sdr", "vocals_csdr", "vocal_leak_db", "inst_sdr", "inst_csdr"):
                    row[k] = None
            row["gpu_seconds"] = meter.runs[-1].seconds if meter.runs and not (a.from_cache and cached.exists()) else None
            rows.append(row)
            print(time.strftime("%H:%M:%S"), mid, name,
                  {k: round(v, 2) for k, v in row.items() if isinstance(v, float)}, flush=True)
        keys = [k for k in rows[0] if k not in ("track", "bands_db", "gpu_seconds", "vocal_level_db")]
        save(f"musdb-{mid}-ex{int(a.excerpt)}-ov{a.overlap}-{a.precision}", {
            "task": "musdb", "model": mid, "role": spec.role, "excerpt_s": a.excerpt,
            "tracks": len(rows), "env": env_info(sep), "speed": meter.summary(),
            "summary": summarise(rows, keys), "rows": rows})
        del sep
        torch.cuda.empty_cache()


def cmd_drums(a):
    """Drum models fed (1) the mix, (2) mix minus estimated vocals, (3) the true instrumental."""
    for mid in a.models.split(","):
        sep = Separator(precision=a.precision, overlap=a.overlap, batch_size=a.batch_size)
        meter = Meter()
        rows = []
        for name, s in musdb_tracks(a.excerpt, a.limit):
            vpath = _cache_path("musdb-vocals", a.vocals_model, name, a.excerpt)
            v = np.load(vpath)
            est_inst = (s["mixture"].astype(np.float64) - v).astype(np.float32)
            row = {"track": name}
            for label, x in (("mix", s["mixture"]), ("est_inst", est_inst), ("true_inst", s["instrumental"])):
                meter.reset_peak()
                d = sep.run_model(mid, x, SR, meter.runs if label == "mix" else None)["drums"]
                meter.update_peak()
                row[f"drums_sdr_from_{label}"] = q.sdr(s["drums"], d)
                row[f"drums_csdr_from_{label}"] = q.chunk_sdr(s["drums"], d, SR)
                if label == "est_inst":
                    row["drums_bands_db"] = q.band_energy_db(d, s["drums"], SR)
            rows.append(row)
            print(time.strftime("%H:%M:%S"), mid, name,
                  {k: round(v, 2) for k, v in row.items() if isinstance(v, float)}, flush=True)
        keys = [k for k in rows[0] if k.startswith("drums_") and k != "drums_bands_db"]
        summ = summarise(rows, keys)
        summ["drums_bands_db_mean"] = {b: mean([r["drums_bands_db"][b] for r in rows]) for b in q.BANDS}
        save(f"drums-{mid}-ex{int(a.excerpt)}-ov{a.overlap}-{a.precision}", {
            "task": "drums", "model": mid, "vocals_model": a.vocals_model, "excerpt_s": a.excerpt,
            "tracks": len(rows), "env": env_info(sep), "speed": meter.summary(),
            "summary": summ, "rows": rows})
        del sep
        torch.cuda.empty_cache()


def cmd_karaoke(a):
    """Lead / backing split on the ccMixter set.

    acappella: input = true lead + true backing (isolates the karaoke model).
    mix:       input = lead + backing + instrumental; (a) vocal model then karaoke model
               (what the presets do), (b) karaoke model straight on the mix.
    Residual lead in the karaoke mix is measured for (a).
    """
    for mid in a.models.split(","):
        sep = Separator(precision=a.precision, overlap=a.overlap, batch_size=a.batch_size)
        meter = Meter()
        rows = []
        for name, s, meta in ccmixter_tracks(a.excerpt):
            lead, back = s["lead"], s["backing"]
            vocals = (lead.astype(np.float64) + back).astype(np.float32)
            back_db = q.energy_db(back, vocals)
            score_back = back_db >= -20.0  # as musetric: backing scored only if within 20 dB
            row = {"track": name, "backing_db_re_vocals": back_db, "license": meta["license"]}
            meter.reset_peak()
            l_hat = sep.run_model(mid, vocals, SR, meter.runs)["lead"]
            meter.update_peak()
            row["acap_lead_sdr"] = q.sdr(lead, l_hat)
            row["acap_backing_sdr"] = q.sdr(back, vocals - l_hat) if score_back else None
            row["acap_lead_leak_in_backing_db"] = q.leak_db(vocals - l_hat, back, lead, SR)
            if "instrumental" in s:
                inst = s["instrumental"]
                mix = (vocals.astype(np.float64) + inst).astype(np.float32)
                vp = _cache_path("ccm-vocals", a.vocals_model, name, a.excerpt)
                if vp.exists():
                    v_hat = np.load(vp)
                else:
                    v_hat = sep.run_model(a.vocals_model, mix, SR)["vocals"]
                    vp.parent.mkdir(parents=True, exist_ok=True)
                    np.save(vp, v_hat)
                l_pipe = sep.run_model(mid, v_hat, SR)["lead"]
                b_pipe = v_hat - l_pipe
                karaoke_true = (mix.astype(np.float64) - lead).astype(np.float32)
                karaoke_est = (mix.astype(np.float64) - l_pipe).astype(np.float32)
                row["mix_vocals_sdr"] = q.sdr(vocals, v_hat)
                row["mix_lead_sdr"] = q.sdr(lead, l_pipe)
                row["mix_backing_sdr"] = q.sdr(back, b_pipe) if score_back else None
                row["mix_karaoke_sdr"] = q.sdr(karaoke_true, karaoke_est)
                row["mix_lead_leak_in_karaoke_db"] = q.leak_db(karaoke_est, karaoke_true, lead, SR)
                l_direct = sep.run_model(mid, mix, SR)["lead"]
                row["mix_direct_lead_sdr"] = q.sdr(lead, l_direct)
            rows.append(row)
            print(time.strftime("%H:%M:%S"), mid, name,
                  {k: (round(v, 2) if isinstance(v, float) else v) for k, v in row.items()
                   if k not in ("track", "license")}, flush=True)
        keys = [k for k in rows[0] if k not in ("track", "license")]
        keys += [k for k in ("mix_vocals_sdr", "mix_lead_sdr", "mix_backing_sdr", "mix_karaoke_sdr",
                             "mix_lead_leak_in_karaoke_db", "mix_direct_lead_sdr") if k not in keys]
        save(f"karaoke-{mid}-ex{int(a.excerpt)}-ov{a.overlap}-{a.precision}", {
            "task": "karaoke", "model": mid, "vocals_model": a.vocals_model, "excerpt_s": a.excerpt,
            # overlap actually used by the karaoke model (0 on the command line = its default)
            "karaoke_overlap": a.overlap or spec_overlap(mid),
            "tracks": len(rows), "mix_tracks": sum("mix_lead_sdr" in r for r in rows),
            "env": env_info(sep), "speed": meter.summary(),
            "summary": summarise(rows, keys), "rows": rows})
        del sep
        torch.cuda.empty_cache()


def _cached(sep, kind, model, name, excerpt, x, out_key):
    p = _cache_path(kind, model, name, excerpt)
    if p.exists():
        return np.load(p)
    y = sep.run_model(model, x, SR)[out_key]
    p.parent.mkdir(parents=True, exist_ok=True)
    np.save(p, y)
    return y


def cmd_presets(a):
    """'full' versus 'clean' karaoke mixes on the ccMixter mixes (true lead/backing known).

    Backing vocals on:  full = mix - lead_hat          clean = inst_hat + (V_hat - lead_hat)
    Backing vocals off: full = mix - V_hat             clean = inst_hat
    """
    vm = a.vocals_model or PRESETS["full"]["vocals"]
    # Estimates are cached per overlap setting ("" = overlap 2, the screening setting;
    # "-ovdef" = each model's registry default, i.e. what the app runs).
    tag = "" if a.overlap == 2 else f"-ov{a.overlap or 'def'}"
    km = a.karaoke_model or PRESETS["full"]["karaoke"]
    sep = Separator(precision=a.precision, overlap=a.overlap or None, batch_size=a.batch_size)
    rows = []
    inst_models = a.models.split(",")
    for name, s, meta in ccmixter_tracks(a.excerpt, mixes_only=True):
        lead, back, inst = s["lead"], s["backing"], s["instrumental"]
        vocals = lead.astype(np.float64) + back
        mix = (vocals + inst).astype(np.float32)
        v_hat = _cached(sep, f"ccm-vocals{tag}", vm, name, a.excerpt, mix, "vocals")
        l_hat = _cached(sep, f"ccm-lead-{vm}{tag}", km, name, a.excerpt, v_hat, "lead")
        b_hat = v_hat.astype(np.float64) - l_hat
        on_true = (inst.astype(np.float64) + back).astype(np.float32)
        variants_on = {"full": (mix.astype(np.float64) - l_hat).astype(np.float32)}
        variants_off = {"full": (mix.astype(np.float64) - v_hat).astype(np.float32)}
        for im in inst_models:
            i_hat = _cached(sep, f"ccm-inst{tag}", im, name, a.excerpt, mix, "instrumental")
            variants_on[f"clean:{im}"] = (i_hat.astype(np.float64) + b_hat).astype(np.float32)
            variants_off[f"clean:{im}"] = i_hat
        row = {"track": name, "on": {}, "off": {}}
        for k, est in variants_on.items():
            row["on"][k] = {"sdr": q.sdr(on_true, est), "lead_leak_db": q.leak_db(est, on_true, lead, SR),
                            "bands_db": q.band_energy_db(est, on_true, SR)}
        vt = vocals.astype(np.float32)
        for k, est in variants_off.items():
            row["off"][k] = {"sdr": q.sdr(inst, est), "vocal_leak_db": q.leak_db(est, inst, vt, SR),
                             "bands_db": q.band_energy_db(est, inst, SR)}
        rows.append(row)
        print(time.strftime("%H:%M:%S"), name, {m: {k: round(v["sdr"], 2) for k, v in row[m].items()}
                                                for m in ("on", "off")}, flush=True)
    summ = {}
    for mode in ("on", "off"):
        summ[mode] = {}
        for k in rows[0][mode]:
            vals = [r[mode][k] for r in rows]
            leak_key = "lead_leak_db" if mode == "on" else "vocal_leak_db"
            summ[mode][k] = {"sdr_mean": mean([v["sdr"] for v in vals]),
                             "sdr_median": median([v["sdr"] for v in vals]),
                             f"{leak_key}_mean": mean([v[leak_key] for v in vals]),
                             "bands_db_mean": {b: mean([v["bands_db"][b] for v in vals]) for b in q.BANDS}}
    overlaps = {m: (a.overlap or registry.get(m).overlap) for m in [vm, km, *inst_models]}
    save(f"presets-ccm-ex{int(a.excerpt)}{tag}", {"task": "presets", "vocals_model": vm, "karaoke_model": km,
                                              "overlaps": overlaps,
                                              "inst_models": inst_models, "tracks": len(rows),
                                              "env": env_info(sep), "summary": summ, "rows": rows})


def _target_of(model_id: str) -> tuple[str, str | None]:
    """(output to compare, MUSDB stem that is its truth) for a model's role."""
    return {"vocals": ("vocals", "vocals"), "instrumental": ("instrumental", "instrumental"),
            "karaoke": ("lead", None), "multistem": ("drums", "drums")}[registry.get(model_id).role]


def cmd_precision(a):
    """Same model, same audio: fp32 (reference) vs tf32 / fp16 / bf16."""
    ref_out, rows = {}, {p: [] for p in ("fp32", "tf32", "fp16", "bf16")}
    speed = {}
    tracks = list(musdb_tracks(a.excerpt, a.limit))
    target, truth_key = _target_of(a.model)
    errors = {}
    for prec in ("fp32", "tf32", "fp16", "bf16"):
        sep = Separator(precision=prec, overlap=a.overlap)
        meter = Meter()
        for name, s in tracks:
            meter.reset_peak()
            try:
                out = sep.run_model(a.model, s["mixture"], SR, meter.runs)
            except Exception as exc:  # e.g. an op without a bf16 kernel: record, keep going
                errors[prec] = repr(exc)[:300]
                print(prec, "failed:", errors[prec], flush=True)
                break
            meter.update_peak()
            est = out[target]
            truth = s[truth_key] if truth_key else None
            r = {"track": name}
            if prec == "fp32":
                ref_out[name] = est
            else:
                d = est.astype(np.float64) - ref_out[name]
                r["diff_db_re_fp32_output"] = q.energy_db(d, ref_out[name])
                r["max_abs_diff"] = float(np.max(np.abs(d)))
                r["delta_sdr"] = (q.sdr(truth, est) - q.sdr(truth, ref_out[name])) if truth is not None else None
            if truth is not None:
                r["sdr"] = q.sdr(truth, est)
            rows[prec].append(r)
            print(time.strftime("%H:%M:%S"), a.model, prec, r, flush=True)
        speed[prec] = meter.summary()
        del sep
        torch.cuda.empty_cache()
    rows = {p: rs for p, rs in rows.items() if rs}
    summ = {p: summarise(rs, [k for k in rs[0] if k != "track"]) for p, rs in rows.items()}
    save(f"precision-{a.model}-ex{int(a.excerpt)}-n{len(tracks)}", {
        "task": "precision", "model": a.model, "target": target, "tracks": len(tracks),
        "overlap": a.overlap, "env": env_info(), "speed": speed, "summary": summ, "rows": rows,
        "errors": errors})


def cmd_overlap(a):
    """SDR and speed of one model at several chunk overlaps (same tracks for each).

    Vocal / instrumental / multi-stem models: MUSDB18-HQ mixes and their true stem.
    Karaoke models: ccMixter a cappellas (lead + backing in, true lead as the target).
    Per-track SDRs are kept so the paired difference between overlaps can be judged.
    """
    target, truth_key = _target_of(a.model)
    if truth_key is None:  # karaoke: lead vs backing on the ccMixter a cappellas
        tracks = []
        for name, s, _meta in ccmixter_tracks(a.excerpt):
            vocals = (s["lead"].astype(np.float64) + s["backing"]).astype(np.float32)
            tracks.append((name, {"input": vocals, "truth": s["lead"]}))
        tracks = tracks[:a.limit]
        data = "ccMixter a cappellas"
    else:
        tracks = [(name, {"input": s["mixture"], "truth": s[truth_key]})
                  for name, s in musdb_tracks(a.excerpt, a.limit)]
        data = "MUSDB18-HQ test"
    overlaps = [int(x) for x in a.overlaps.split(",")]
    res = {}
    for ov in overlaps:
        sep = Separator(overlap=ov)
        meter = Meter()
        sdrs = []
        for name, s in tracks:
            meter.reset_peak()
            out = sep.run_model(a.model, s["input"], SR, meter.runs)
            meter.update_peak()
            sdrs.append(q.sdr(s["truth"], out[target]))
        res[ov] = {"sdr_mean": mean(sdrs), "sdr_per_track": [round(x, 4) for x in sdrs],
                   "speed": meter.summary()}
        print(a.model, "overlap", ov, {k: v for k, v in res[ov].items() if k != "sdr_per_track"}, flush=True)
        del sep
        torch.cuda.empty_cache()
    save(f"overlap-{a.model}-ex{int(a.excerpt)}-n{len(tracks)}" + ("" if overlaps == [1, 2, 4, 8] else
                                                                 "-ov" + "_".join(map(str, overlaps))), {
        "task": "overlap", "model": a.model, "target": target, "data": data, "tracks": len(tracks),
        "excerpt_s": a.excerpt, "env": env_info(), "results": res})


def _gpu_snapshot() -> str:
    """GPU utilisation and memory in use (all processes), as nvidia-smi reports them.

    Under Windows WDDM nvidia-smi cannot attribute memory to processes, so this is the
    whole card: anything above our own model's footprint is another program.
    """
    import subprocess

    try:
        util = subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu,memory.used",
                               "--format=csv,noheader"], capture_output=True, text=True, timeout=20).stdout.strip()
        return f"utilisation, memory used = {util}"
    except Exception as exc:  # nvidia-smi missing: say so, do not fail the benchmark
        return f"unavailable ({exc})"


def cmd_speed(a):
    """GPU seconds per minute of audio for each model on one fixed 60 s clip (3 runs, median)."""
    name, s = next(iter(musdb_tracks(60.0, 1)))
    res = {"clip": name, "overlap": a.overlap, "gpu_before": _gpu_snapshot(), "env": env_info(), "models": {}}
    for mid in a.models.split(","):
        sep = Separator(overlap=a.overlap, batch_size=a.batch_size)
        sep.run_model(mid, s["mixture"][:, : SR * 10], SR)  # load + warm up
        torch.cuda.reset_peak_memory_stats()
        times = []
        for _ in range(3):
            runs = []
            sep.run_model(mid, s["mixture"], SR, runs)
            times.append(runs[0].seconds)
        res["models"][mid] = {"seconds_per_audio_minute": round(st.median(times), 2), "runs": times,
                              "peak_vram_reserved_mb": torch.cuda.max_memory_reserved() // 2**20}
        print(mid, res["models"][mid], flush=True)
        del sep
        torch.cuda.empty_cache()
    res["gpu_after"] = _gpu_snapshot()
    save(f"speed-ov{a.overlap}" + (f"-{a.tag}" if a.tag else ""), res)


def cmd_batch(a):
    """Chunks per network call (batch size): speed, VRAM and output difference vs batch 1.

    One 60 s clip, each model at ``--overlap`` (0 = its default), fp32, best of 2 runs.
    """
    name, s = next(iter(musdb_tracks(60.0, 1)))
    x = s["mixture"]
    res = {"clip": name, "model": a.model, "overlap": a.overlap or registry.get(a.model).overlap,
           "gpu_before": _gpu_snapshot(), "env": env_info(), "batches": {}}
    ref = None
    for bs in (int(v) for v in a.batches.split(",")):
        sep = Separator(overlap=a.overlap or None, batch_size=bs)
        sep.run_model(a.model, x[:, : SR * 10], SR)  # load + warm up
        torch.cuda.reset_peak_memory_stats()
        times = []
        for _ in range(2):
            runs = []
            out = sep.run_model(a.model, x, SR, runs)
            times.append(runs[0].seconds)
        est = next(iter(out.values())).astype(np.float64)
        ref = est if ref is None else ref
        d = est - ref
        res["batches"][bs] = {"seconds_per_audio_minute": round(min(times), 2), "runs": times,
                              "peak_vram_reserved_mb": torch.cuda.max_memory_reserved() // 2**20,
                              "max_abs_diff_vs_first": float(np.max(np.abs(d))),
                              "diff_db_vs_first": q.energy_db(d, ref) if np.any(d) else None}
        print(a.model, "batch", bs, res["batches"][bs], flush=True)
        del sep
        torch.cuda.empty_cache()
    save(f"batch-{a.model}", res)


def _preset_models(preset: str, a) -> dict | None:
    m = dict(PRESETS[preset])
    for role in ("vocals", "instrumental", "karaoke", "drums"):
        v = getattr(a, f"{role}_model", None)
        if v and role in m:
            m[role] = v
    return m


def _err_stats(prefix: str, ref: np.ndarray, err: np.ndarray) -> dict:
    """Max abs error, SNR and error RMS (dBFS) of ``err`` against ``ref``."""
    return {f"{prefix}_max_abs_err": float(np.max(np.abs(err))),
            f"{prefix}_snr_db": q.snr_db(ref, err),
            f"{prefix}_err_rms_dbfs": float(10 * np.log10(np.mean(err**2) + 1e-30))}


def cmd_lossless(a):
    """'Nothing lost': the four stems at unity sum back to the input.

    Checked three ways for each preset, on real separations of MUSDB excerpts:

    float32   the four float32 stems summed in float32, as a player mixes them.
    q24       every stem stored on its own with the song writer's rule
              (``jksong.quantize_stem``: 24-bit integers, gain 1.0 unless the stem would
              clip, then peak 0.99 and the inverse gain stored), decoded, summed in float64.
    file      a real ``.jksong`` written with ``write_song(source=mix)`` (exact-residual
              mode: "other" is computed from the decoded other stems), read back through
              the reader (FLAC decode + stored gain) and summed: in float64 ("file") and
              as float32 stems summed in float32 ("file_f32", what a player gets).

    ``--mix-gain-db G`` multiplies every MUSDB mixture by 10^(G/20) before separating it.
    MUSDB mixtures are 16-bit and peak below full scale, so at G = 0 no stem needs a
    stored gain and every source sample is exactly representable in 24 bits. A gain
    that is not a power of two (e.g. +9 dB) makes the source a true float signal whose
    stems exceed full scale, which exercises the stored-gain path on real separations.
    """
    from jukeoke_studio import jksong

    sep = Separator(overlap=a.overlap or None)
    rows = []
    tmp = EVAL / "tmp-lossless"
    tmp.mkdir(parents=True, exist_ok=True)
    stems_order = ("lead_vocal", "backing_vocals", "drums", "other")
    mix_gain = float(10 ** (a.mix_gain_db / 20))
    for preset in ("full", "clean"):
        for name, s in musdb_tracks(a.excerpt, a.limit):
            mix = (s["mixture"] * mix_gain).astype(np.float32) if a.mix_gain_db else s["mixture"]
            m64 = mix.astype(np.float64)
            res = sep.separate(mix, SR, preset, _preset_models(preset, a))
            row = {"preset": preset, "track": name}

            st32 = np.zeros_like(mix)
            for k in stems_order:
                st32 += res.stems[k]  # float32 accumulation, as a player would mix
            row.update(_err_stats("float32", m64, st32.astype(np.float64) - m64))

            deq = np.zeros(mix.shape, np.float64)
            gains = {}
            for k in stems_order:
                qi, g, _peak = jksong.quantize_stem(res.stems[k])
                gains[k] = g
                deq += qi.T * (g / jksong.FULL_SCALE)
            row.update(_err_stats("q24", m64, deq - m64))

            path = tmp / f"{preset}.jksong"
            jksong.write_song(
                path, stems=[jksong.StemInput(k, None if k == "other" else res.stems[k]) for k in stems_order],
                sample_rate=SR, title=name, artist="MUSDB18-HQ test excerpt", preset=preset,
                stems_sum_to_source=True, source=mix)
            song = jksong.open_song(path)
            dec64 = np.zeros(mix.shape, np.float64)
            dec32 = np.zeros_like(mix)
            file_gains = {}
            for k in stems_order:
                g = float(song.stem(k)["gain"])
                file_gains[k] = g
                dec64 += song.read_stem_int(k).T * (g / jksong.FULL_SCALE)
                dec32 += song.read_stem(k)
            row.update(_err_stats("file", m64, dec64 - m64))
            row.update(_err_stats("file_f32", m64, dec32.astype(np.float64) - m64))
            row["file_bytes"] = path.stat().st_size
            path.unlink()
            row["mix_peak"] = float(np.max(np.abs(mix)))
            row["stem_peaks"] = {k: float(np.max(np.abs(x))) for k, x in res.stems.items()}
            row["gains_db"] = {k: float(20 * np.log10(g)) for k, g in file_gains.items()}
            # The error bound of the exact-residual file path: half an LSB of the residual
            # stem's grid, which the stored gain scales (gain > 1 when "other" was loud).
            row["file_bound_half_lsb_x_gain"] = 0.5 / 2**23 * file_gains["other"]
            rows.append(row)
            print(time.strftime("%H:%M:%S"), {k: (f"{v:.3g}" if isinstance(v, float) else v)
                                               for k, v in row.items() if k not in ("stem_peaks", "gains_db")},
                  flush=True)
    suffix = f"-g{a.mix_gain_db:+g}dB" if a.mix_gain_db else ""
    save(f"lossless-ex{int(a.excerpt)}-n{a.limit}{suffix}", {
        "task": "lossless", "env": env_info(sep), "excerpt_s": a.excerpt, "mix_gain_db": a.mix_gain_db,
        "lsb": 1 / 2**23, "models": {p: _preset_models(p, a) for p in ("full", "clean")}, "rows": rows})


def cmd_resample48(a):
    """A 48 kHz source: only model input/estimate are resampled; stems stay at 48 kHz."""
    sep = Separator(overlap=a.overlap or None)
    rows = []
    vm = a.vocals_model or PRESETS["full"]["vocals"]
    for name, s in musdb_tracks(a.excerpt, a.limit):
        # Make a "48 kHz recording" of the same music (truth at 48 kHz too).
        mix48 = resample(s["mixture"], SR, 48000)
        voc48 = resample(s["vocals"], SR, 48000)
        v44 = sep.run_model(vm, s["mixture"], SR)["vocals"]
        v48 = sep.run_model(vm, mix48, 48000)["vocals"]
        res = sep.separate(mix48, 48000, "full", _preset_models("full", a))
        total = sum(res.stems[k].astype(np.float64) for k in res.stems)
        rows.append({
            "track": name,
            "sdr_native_44k1": q.sdr(s["vocals"], v44),
            "sdr_48k_path": q.sdr(voc48, v48),
            "out_rate": res.sample_rate, "out_frames": int(res.stems["other"].shape[-1]),
            "in_frames": int(mix48.shape[-1]),
            "sum_max_abs_err_48k": float(np.max(np.abs(total - mix48))),
            "resampled_runs": [r.resampled for r in res.runs],
        })
        print(rows[-1], flush=True)
    save(f"resample48-ex{int(a.excerpt)}-n{a.limit}", {"task": "resample48", "vocals_model": vm,
                                                         "env": env_info(sep), "rows": rows})


def cmd_timing(a):
    """Wall time of each preset on one ~4-minute stereo song (first run excluded)."""
    root = EVAL / "musdb18hq" / "test"
    # The MUSDB test track whose length is closest to 4:00.
    best = min((abs(sf.info(str(p / "mixture.wav")).frames / SR - 240), p) for p in root.iterdir())[1]
    mix = _read(best / "mixture.wav", 0, sf.info(str(best / "mixture.wav")).frames)
    out = {"track": best.name, "seconds": mix.shape[1] / SR, "presets": {}, "gpu_before": _gpu_snapshot()}
    for preset in ("full", "clean"):
        sep = Separator(overlap=a.overlap or None)
        models = _preset_models(preset, a)
        t0 = time.perf_counter()
        for mid in dict.fromkeys(models.values()):  # load (from the OS file cache) onto the GPU
            sep.model(mid)
        load = time.perf_counter() - t0
        sep.separate(mix[:, : SR * 10], SR, preset, models)  # warm up (CUDA kernels, allocator)
        torch.cuda.reset_peak_memory_stats()
        t0 = time.perf_counter()
        res = sep.separate(mix, SR, preset, models)
        wall = time.perf_counter() - t0
        out["presets"][preset] = {
            "models": models, "wall_seconds": round(wall, 1), "load_seconds": round(load, 1),
            "per_model": [{"model": r.model, "gpu_seconds": round(r.seconds, 1),
                           "overlap": a.overlap or registry.get(r.model).overlap} for r in res.runs],
            "peak_vram_reserved_mb": torch.cuda.max_memory_reserved() // 2**20,
        }
        print(preset, out["presets"][preset], flush=True)
        del sep
        torch.cuda.empty_cache()
    out["env"] = env_info()
    out["overlap"] = a.overlap
    out["gpu_after"] = _gpu_snapshot()
    save(f"timing-ov{a.overlap}", out)


# ---------------------------------------------------------------------------- report

def _fmt(x, d=2):
    return "-" if x is None else f"{x:.{d}f}"


def _lic(mid):
    spec = registry.get(mid)
    return f"{spec.licence.split(' (')[0].split(';')[0]} ({spec.commercial})"


def _speed_cells(res: dict, r: dict) -> str:
    """'GPU s/min | VRAM MB' for a model: from the dedicated speed run (one model at a time,
    GPU otherwise idle) when there is one, else from the quality run itself (marked *,
    GPU shared with other jobs, so an upper bound)."""
    for k in sorted(res):
        if k.startswith("speed-") and r["model"] in res[k]["models"]:
            v = res[k]["models"][r["model"]]
            return f"{_fmt(v['seconds_per_audio_minute'], 1)} | {v['peak_vram_reserved_mb']}"
    return f"{_fmt(r['speed']['seconds_per_audio_minute'], 1)}* | {r['speed']['peak_vram_reserved_mb']}*"


def _has_vocals(r):
    if "vocal_level_db" in r:
        return r["vocal_level_db"] >= NO_VOCALS_DB
    return r.get("vocal_leak_db") is None or r["vocal_leak_db"] > -200  # -300 dB = silent reference


def _subset_mean(rows, key, names):
    vals = [r.get(key) for r in rows if r["track"] in names and _has_vocals(r)]
    return mean(vals), len(vals)


def _bands_mean(rows, names, key="bands_db"):
    rs = [r[key] for r in rows if r["track"] in names and key in r and _has_vocals(r)]
    return {b: mean([x[b] for x in rs]) for b in q.BANDS} if rs else {}


def _report_musdb(res, excerpt):
    out = []
    musdb = [r for k, r in res.items() if k.startswith("musdb-") and r["excerpt_s"] == excerpt]
    if not musdb:
        return out
    names = {r["track"] for r in min(musdb, key=lambda r: r["tracks"])["rows"]}
    all_names = {r["track"] for r in max(musdb, key=lambda r: r["tracks"])["rows"]}
    silent = sorted({r["track"] for m in musdb for r in m["rows"] if not _has_vocals(r)})
    groups = (({"vocals", "multistem"}, "Vocal models (instrumental = mix - vocals: the 'full' strategy)"),
              ({"instrumental"}, "Instrumental models (the 'clean' strategy)"))
    for role_set, title in groups:
        sel = [r for r in musdb if r["role"] in role_set and any("vocals_sdr" in x for x in r["rows"])]
        if not sel:
            continue
        out += [f"#### {title}", "",
                f"MUSDB18-HQ test, {excerpt:.0f} s excerpts, the {len(names)} tracks every model ran on "
                "(columns marked 50: all 50 test tracks, where run). Bands: estimated instrumental energy "
                "relative to the true instrumental, dB. Left out (no vocals in the excerpt): "
                f"{', '.join(silent) or 'none'}.", "",
                "| model | licence (commercial) | vocals SDR | vocals SDR 50 | vocals cSDR | inst SDR | inst SDR 50 "
                "| inst cSDR | vocal residue in inst, dB | <250 Hz | 250-4k | 4-8k | 8-16k | GPU s/min | VRAM MB |",
                "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]

        def order(r):
            return -(_subset_mean(r["rows"], "inst_sdr", names)[0] or -99)

        for r in sorted(sel, key=order):
            rows = r["rows"]
            v, _ = _subset_mean(rows, "vocals_sdr", names)
            i, _ = _subset_mean(rows, "inst_sdr", names)
            c = median([x.get("vocals_csdr") for x in rows if x["track"] in names and _has_vocals(x)])
            ic = median([x.get("inst_csdr") for x in rows if x["track"] in names and _has_vocals(x)])
            leak, _ = _subset_mean(rows, "vocal_leak_db", names)
            b = _bands_mean(rows, names)
            all50 = r["tracks"] >= 50
            v50 = _fmt(_subset_mean(rows, "vocals_sdr", all_names)[0]) if all50 else "-"
            i50 = _fmt(_subset_mean(rows, "inst_sdr", all_names)[0]) if all50 else "-"
            out.append(
                f"| `{r['model']}` | {_lic(r['model'])} | {_fmt(v)} | {v50} | {_fmt(c)} | {_fmt(i)} | {i50} | "
                f"{_fmt(ic)} | {_fmt(leak, 1)} | {_fmt(b.get('low_lt250'))} | {_fmt(b.get('mid_250_4k'))} | "
                f"{_fmt(b.get('high_4k_8k'))} | {_fmt(b.get('cymbal_8k_16k'))} | "
                f"{_speed_cells(res, r)} |")
        out.append("")
    return out


def spec_overlap(model_id: str) -> int:
    """The chunk overlap a model runs at in the app (its registry default)."""
    return registry.get(model_id).overlap


def _report_karaoke(res):
    """One table per overlap setting: the screening runs (overlap 2 for every model) and
    the runs at each model's own default overlap (what the app does)."""
    out = []
    kar = [r for k, r in res.items() if k.startswith("karaoke-")]
    groups: dict[str, list] = {}
    for r in kar:
        groups.setdefault("2" if r.get("env", {}).get("overlap") == 2 else "default", []).append(r)
    for g in ("2", "default"):
        if g in groups:
            out += _report_karaoke_group(res, groups[g], g)
    return out


def _report_karaoke_group(res, kar, group):
    out = []
    s0 = kar[0]
    if group == "2":
        ovtxt = "overlap 2 for every model (screening)"
    else:
        ovtxt = "each model at its default overlap, as the app runs it: " + ", ".join(
            f"`{r['model']}` {r.get('karaoke_overlap', spec_overlap(r['model']))}" for r in kar)
    out += [f"#### Lead / backing (karaoke models), ccMixter set, {ovtxt}", "",
            f"{s0['tracks']} a cappellas ({s0['excerpt_s']:.0f} s excerpts), {s0['mix_tracks']} of them also over "
            f"their own instrument stems. Pipeline: vocal model `{s0['vocals_model']}`, then the karaoke model on "
            "its vocals. Direct: karaoke model on the full mix. Karaoke mix = mix - estimated lead "
            "(backing vocals kept).", "",
            "| model | licence (commercial) | a cappella: lead SDR (mean / median) | backing SDR | lead residue in backing, dB "
            "| mix: lead SDR (pipeline) | lead SDR (direct) | backing SDR | karaoke-mix SDR "
            "| lead residue in karaoke mix, dB | GPU s/min | VRAM MB |",
            "|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in sorted(kar, key=lambda r: -(r["summary"]["mix_lead_sdr"]["mean"] or -99)):
        sm = r["summary"]

        def g(k, d=2):
            return _fmt(sm.get(k, {}).get("mean"), d)

        out.append(
            f"| `{r['model']}` | {_lic(r['model'])} | {g('acap_lead_sdr')} / {_fmt(sm['acap_lead_sdr']['median'])} | "
            f"{g('acap_backing_sdr')} | "
            f"{g('acap_lead_leak_in_backing_db', 1)} | {g('mix_lead_sdr')} | {g('mix_direct_lead_sdr')} | "
            f"{g('mix_backing_sdr')} | {g('mix_karaoke_sdr')} | {g('mix_lead_leak_in_karaoke_db', 1)} | "
            # The dedicated speed runs are at overlap 2; at the default overlap use this
            # run's own timing of the a cappella passes (marked +).
            + (f"{_speed_cells(res, r)} |" if group == "2" else
               f"{_fmt(r['speed']['seconds_per_audio_minute'], 1)}+ | {r['speed']['peak_vram_reserved_mb']}+ |"))
    out.append("")
    return out


def _report_presets(r):
    ovs = r.get("overlaps")
    ovtxt = ("overlap " + ", ".join(f"{m} {o}" for m, o in ovs.items())) if ovs else "overlap 2 for every model"
    out = [f"#### 'full' versus 'clean' karaoke mixes (ccMixter mixes, true stems known; {ovtxt})", "",
           f"{r['tracks']} mixes. Vocal model `{r['vocals_model']}`, karaoke model `{r['karaoke_model']}`. "
           "Backing on: full = mix - lead; clean = instrumental model + backing. Backing off: full = mix - vocals; "
           "clean = instrumental model. Residue: lead (backing on) or all vocals (backing off) left in the mix, dB "
           "relative to that source (mean / median over tracks). Bands: estimated vs true karaoke mix, dB, "
           "**median** over tracks (the mean is dominated by one track whose instrument stem the models "
           "largely take for voice: down to -51 dB in a band).", "",
           "| variant | backing | SDR mean | SDR median | residue, dB mean / median | <250 Hz | 250-4k | 4-8k | 8-16k |",
           "|---|---|---|---|---|---|---|---|---|"]
    for mode, leak in (("on", "lead_leak_db"), ("off", "vocal_leak_db")):
        for k, v in r["summary"][mode].items():
            per = [row[mode][k] for row in r["rows"]]
            b = {band: median([x["bands_db"][band] for x in per]) for band in q.BANDS}
            out.append(f"| {k} | {mode} | {_fmt(v['sdr_mean'])} | {_fmt(v['sdr_median'])} | "
                       f"{_fmt(v[leak + '_mean'], 1)} / {_fmt(median([x[leak] for x in per]), 1)} | "
                       f"{_fmt(b['low_lt250'])} | {_fmt(b['mid_250_4k'])} | {_fmt(b['high_4k_8k'])} | "
                       f"{_fmt(b['cymbal_8k_16k'])} |")
    out.append("")
    return out


def _report_drums(res):
    out = []
    dr = [r for k, r in res.items() if k.startswith("drums-")]
    if not dr:
        return out
    out += ["#### Drums", "",
            f"MUSDB18-HQ test, first {dr[0]['tracks']} tracks, {dr[0]['excerpt_s']:.0f} s excerpts. Inputs: the mix; "
            f"mix - vocals estimated by `{dr[0]['vocals_model']}` (what the presets do); the true instrumental "
            "(upper bound). Bands: drum estimate vs true drums (from mix - vocals), dB.", "",
            "| model | licence (commercial) | drums SDR from mix | from mix - vocals | from true inst "
            "| cSDR (mix - vocals) | <250 Hz | 250-4k | 4-8k | 8-16k | GPU s/min | VRAM MB |",
            "|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in sorted(dr, key=lambda r: -(r["summary"]["drums_sdr_from_est_inst"]["mean"] or -99)):
        sm = r["summary"]
        b = sm["drums_bands_db_mean"]
        out.append(
            f"| `{r['model']}` | {_lic(r['model'])} | {_fmt(sm['drums_sdr_from_mix']['mean'])} | "
            f"{_fmt(sm['drums_sdr_from_est_inst']['mean'])} | {_fmt(sm['drums_sdr_from_true_inst']['mean'])} | "
            f"{_fmt(sm['drums_csdr_from_est_inst']['median'])} | {_fmt(b['low_lt250'])} | {_fmt(b['mid_250_4k'])} | "
            f"{_fmt(b['high_4k_8k'])} | {_fmt(b['cymbal_8k_16k'])} | "
            f"{_speed_cells(res, r)} |")
    out.append("")
    return out


def _report_misc(k, r):
    out = []
    if k.startswith("precision-"):
        out += [f"#### Precision: `{r['model']}` ({r['tracks']} tracks, output `{r['target']}`)", "",
                "| precision | output difference vs fp32, dB (mean / worst track) | max abs difference "
                "| SDR change vs fp32, dB (mean, largest) | GPU s/min |",
                "|---|---|---|---|---|"]
        for p, rows in r["rows"].items():
            spd = _fmt(r["speed"][p]["seconds_per_audio_minute"], 1)
            if p == "fp32":
                out.append(f"| fp32 | reference | 0 | 0 | {spd} |")
                continue
            if not rows:
                continue
            d = [x["diff_db_re_fp32_output"] for x in rows]
            ds = [x.get("delta_sdr") for x in rows if x.get("delta_sdr") is not None]
            dsdr = f"{float(np.mean(ds)):+.1e} (largest {max(ds, key=abs):+.1e})" if ds else "-"
            out.append(f"| {p} | {_fmt(mean(d), 1)} / {_fmt(max(d), 1)} | {max(x['max_abs_diff'] for x in rows):.2e} | "
                       f"{dsdr} | {spd} |")
        for p, e in r.get("errors", {}).items():
            out.append(f"| {p} | failed: {e} | | | |")
        out.append("")
    elif k.startswith("overlap-"):
        target = r.get("target") or _target_of(r["model"])[0]
        out += [f"#### Overlap: `{r['model']}` ({r['tracks']} tracks of {r.get('data', 'MUSDB18-HQ test')}, "
                f"{r.get('excerpt_s', 60):.0f} s, run {r.get('env', {}).get('date', '')})", "",
                f"| overlap | {target} SDR (mean) | change vs overlap 2, dB (paired mean) | tracks better / worse "
                "than overlap 2 | GPU s/min |", "|---|---|---|---|---|"]
        base = r["results"].get("2", {}).get("sdr_per_track")
        for ov, v in r["results"].items():
            per = v.get("sdr_per_track")
            if base and per and ov != "2":
                d = [x - y for x, y in zip(per, base)]
                diff = f"{np.mean(d):+.3f}"
                wl = f"{sum(x > 0 for x in d)} / {sum(x < 0 for x in d)}"
            else:
                diff, wl = ("0" if ov == "2" else "-"), "-"
            out.append(f"| {ov} | {_fmt(v['sdr_mean'], 3)} | {diff} | {wl} | "
                       f"{_fmt(v['speed']['seconds_per_audio_minute'], 1)} |")
        out.append("")
    elif k.startswith("lossless-"):
        lsb = r.get("lsb", 1 / 2**23)
        out += [f"#### Nothing lost ({k})", "",
                "Worst track per column. Error = sum of the four stems at unity minus the original. "
                f"One 24-bit LSB = {lsb:.3g} (-138.5 dBFS). Four stems rounded to 24 bits independently "
                "give an error RMS of sqrt(4/12) LSB = -143.2 dBFS (quantisation level).", "",
                "| preset | tracks | path | max abs error | max abs error, LSB | SNR, dB | error RMS, dBFS |",
                "|---|---|---|---|---|---|---|"]
        paths = (("float32", "float32 stems, float32 sum"),
                 ("q24", "each stem 24-bit (writer's gain rule), float64 sum"),
                 ("file", ".jksong written + read back, exact residual, float64 sum"),
                 ("file_f32", ".jksong read back as float32 stems, float32 sum"))
        for preset in ("full", "clean"):
            rows = [x for x in r["rows"] if x["preset"] == preset]
            for key, label in paths:
                if rows and f"{key}_max_abs_err" in rows[0]:
                    mx = max(x[f"{key}_max_abs_err"] for x in rows)
                    if mx == 0:  # bit-exact: SNR / RMS would only show the numeric floor
                        out.append(f"| {preset} | {len(rows)} | {label} | 0 (bit-exact) | 0 | exact | exact |")
                        continue
                    out.append(f"| {preset} | {len(rows)} | {label} | {mx:.2e} | {mx / lsb:.2f} | "
                               f"{min(x[f'{key}_snr_db'] for x in rows):.1f} | "
                               f"{max(x[f'{key}_err_rms_dbfs'] for x in rows):.1f} |")
        gains = [(x["preset"], st, g) for x in r["rows"] for st, g in x.get("gains_db", {}).items() if g != 0]
        out.append("")
        if r.get("mix_gain_db"):
            peaks = [x["mix_peak"] for x in r["rows"] if "mix_peak" in x]
            out.append(f"Input: each MUSDB mixture x {r['mix_gain_db']:+g} dB (a float source, peaks "
                       f"{min(peaks):.2f} to {max(peaks):.2f}).")
        by_stem: dict[tuple[str, str], list[float]] = {}
        for p, st, g in gains:
            by_stem.setdefault((p, st), []).append(g)
        n_stems = sum(len(x.get("gains_db", {})) for x in r["rows"])
        out.append(f"Stems stored with a gain (peak above full scale, so scaled to a 0.99 peak and the "
                   f"inverse gain stored; the 24-bit step of such a stem is gain x LSB): {len(gains)} of "
                   f"{n_stems}" + (": " + ", ".join(
                       f"{p}/{st} {len(v)}x ({min(v):+.2f} to {max(v):+.2f} dB)"
                       for (p, st), v in by_stem.items()) if gains else "") + ".")
        within = [x["file_max_abs_err"] <= x["file_bound_half_lsb_x_gain"] * (1 + 1e-9)
                  for x in r["rows"] if "file_bound_half_lsb_x_gain" in x]
        if within:
            out.append(f"`.jksong` path within its bound (half an LSB of the residual stem x its stored "
                       f"gain) on {sum(within)} of {len(within)} tracks.")
        out.append("")
    elif k.startswith("resample48-"):
        rows = r["rows"]
        ok = all(x["out_rate"] == 48000 and x["out_frames"] == x["in_frames"] for x in rows)
        out += ["#### 48 kHz source", "",
                "| tracks | vocals SDR, native 44.1 kHz | vocals SDR, 48 kHz path | worst abs difference "
                "| stems at 48 kHz, input length | stems sum, max abs error |",
                "|---|---|---|---|---|---|",
                f"| {len(rows)} | {_fmt(mean([x['sdr_native_44k1'] for x in rows]), 3)} | "
                f"{_fmt(mean([x['sdr_48k_path'] for x in rows]), 3)} | "
                f"{max(abs(x['sdr_48k_path'] - x['sdr_native_44k1']) for x in rows):.3f} dB | {ok} | "
                f"{max(x['sum_max_abs_err_48k'] for x in rows):.2e} |", ""]
    elif k.startswith("speed-"):
        pass  # all speed runs are merged into one table by _report_speed
    elif k.startswith("timing-"):
        out += [f"#### Whole separation of one song: `{r['track']}` ({r['seconds']:.0f} s, stereo, 44.1 kHz, "
                f"overlap {r['overlap'] or 'per model'})", "", f"GPU before: {r.get('gpu_before')}. After: {r.get('gpu_after')}.", "",
                "| preset | separation wall time, s | model loading, s | per model, GPU s | peak VRAM MB |",
                "|---|---|---|---|---|"]
        for p, v in r["presets"].items():
            per = ", ".join(f"`{m['model']}` {m['gpu_seconds']}" + (f" (overlap {m['overlap']})" if "overlap" in m else "")
                            for m in v["per_model"])
            out.append(f"| {p} | {v['wall_seconds']} | {v.get('load_seconds', '-')} | {per} | "
                       f"{v['peak_vram_reserved_mb']} |")
        out.append("")
    return out


def _report_speed(res):
    """One table from every dedicated speed run (one model at a time, same 60 s clip)."""
    runs = [(k, r) for k, r in sorted(res.items()) if k.startswith("speed-")]
    if not runs:
        return []
    out = [f"#### Model speed: GPU seconds per minute of audio, one 60 s clip (`{runs[0][1]['clip']}`)", "",
           "Each model alone on the GPU, fp32, median of 3 runs after a warm-up. GPU at the start of each run: "
           + "; ".join(f"{r.get('env', {}).get('date', '')}: {r['gpu_before']}" for _, r in runs) + ".", "",
           "| model | role | overlap | GPU s per audio minute | 4-minute song, s | peak VRAM MB |",
           "|---|---|---|---|---|---|"]
    for k, r in runs:
        ov = r.get("overlap") or k.split("-ov")[-1].split("-")[0]
        for mid, v in r["models"].items():
            s = v["seconds_per_audio_minute"]
            out.append(f"| `{mid}` | {registry.get(mid).role} | {ov} | {s:.2f} | {4 * s:.0f} | "
                       f"{v['peak_vram_reserved_mb']} |")
    out.append("")
    return out


def cmd_report(a):
    """Print every results table as Markdown (pasted into docs/benchmarks/separation.md)."""
    files = sorted(RESULTS.glob("*.json"))
    res = {f.stem: json.loads(f.read_text(encoding="utf-8")) for f in files}
    out = _report_musdb(res, a.excerpt) + _report_karaoke(res)
    for k, r in res.items():
        if k.startswith("presets-"):
            out += _report_presets(r)
    out += _report_drums(res)
    out += _report_speed(res)
    for k, r in res.items():
        out += _report_misc(k, r)
    text = "\n".join(out)
    if a.out:
        Path(a.out).write_text(text, encoding="utf-8")
    print(text)


# ------------------------------------------------------------------------------ main

def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p, models=False, model=False):
        p.add_argument("--excerpt", type=float, default=60.0, help="seconds per track (0 = whole track)")
        p.add_argument("--limit", type=int, default=None, help="first N tracks only")
        p.add_argument("--overlap", type=int, default=2,
                       help="chunk overlap for every model; 0 = each model's default (registry)")
        p.add_argument("--precision", default="fp32")
        p.add_argument("--batch-size", type=int, default=1)
        if models:
            p.add_argument("--models", required=True)
        if model:
            p.add_argument("--model", required=True)
        for role in ("vocals", "instrumental", "karaoke", "drums"):
            p.add_argument(f"--{role}-model", default=None)

    p = sub.add_parser("musdb"); common(p, models=True)
    p.add_argument("--cache", action="store_true", help="keep vocal estimates for drums/karaoke runs")
    p.add_argument("--from-cache", action="store_true", help="score cached vocal estimates instead of re-running")
    p.set_defaults(fn=cmd_musdb)
    p = sub.add_parser("drums"); common(p, models=True); p.set_defaults(fn=cmd_drums)
    p = sub.add_parser("karaoke"); common(p, models=True); p.set_defaults(fn=cmd_karaoke)
    p = sub.add_parser("presets"); common(p, models=True); p.set_defaults(fn=cmd_presets)
    p = sub.add_parser("precision"); common(p, model=True); p.set_defaults(fn=cmd_precision)
    p = sub.add_parser("overlap"); common(p, model=True)
    p.add_argument("--overlaps", default="1,2,4,8")
    p.set_defaults(fn=cmd_overlap)
    p = sub.add_parser("lossless"); common(p)
    p.add_argument("--mix-gain-db", type=float, default=0.0,
                   help="scale each mixture by this gain first (stems above full scale -> stored gain)")
    p.set_defaults(fn=cmd_lossless)
    p = sub.add_parser("resample48"); common(p); p.set_defaults(fn=cmd_resample48)
    p = sub.add_parser("timing"); common(p); p.set_defaults(fn=cmd_timing)
    p = sub.add_parser("speed"); common(p, models=True)
    p.add_argument("--tag", default="", help="suffix for the result file (several speed runs)")
    p.set_defaults(fn=cmd_speed)
    p = sub.add_parser("batch"); common(p, model=True)
    p.add_argument("--batches", default="1,2,4")
    p.set_defaults(fn=cmd_batch)
    p = sub.add_parser("report")
    p.add_argument("--excerpt", type=float, default=60.0)
    p.add_argument("--out", default=None, help="also write the Markdown here")
    p.set_defaults(fn=cmd_report)
    a = ap.parse_args(argv)
    if a.cmd == "karaoke" and not a.vocals_model:
        a.vocals_model = PRESETS["full"]["vocals"]
    if a.cmd == "drums" and not a.vocals_model:
        a.vocals_model = PRESETS["full"]["vocals"]
    a.fn(a)


if __name__ == "__main__":
    main()

# SPDX-License-Identifier: GPL-3.0-or-later
"""Melody-guide benchmark (WO2 sections 4.1, 6 "Guide builder", 9 "Guide accuracy" and
"Guide on real songs"). Results: docs/benchmarks/practice-guide.md.

Compares the plain-DSP Rust builder (jk-pitch::guide, pYIN + note segmentation) with
neural pitch trackers fed through the SAME note segmentation (the Rust tool's ``--curve``
mode), on:

- a synthetic singer with known notes (studio/bench/practice/synth.py), alone and mixed
  with Creative Commons instrumentals from ccMixter and put through the real Studio
  separation (``Separator.separate``, preset "full");
- the lead stems of the three Creative Commons end-to-end songs, the public-domain demo
  song and the owner's song (copied first; read only), which have no ground truth.

Run in the overlay venv (Studio venv + the neural trackers, see trackers.py)::

    $JK_BUILD/practice-guide/venv/Scripts/python.exe -I studio/bench/practice_guide_bench.py all

Stages (each caches its files under $JK_BUILD/practice-guide; ``--force`` redoes):
synth, mix, songs, track, guide, eval, speed. Nothing is written into the repository.
Lyrics are never read.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "studio"))

from practice import metrics, plots, synth, trackers  # noqa: E402

JK_BUILD = Path(os.environ.get("JK_BUILD", REPO.parent / "jukeoke-build"))
G = JK_BUILD / "practice-guide"
EXE = JK_BUILD / "target-wo2-guide" / "release" / "examples" / "guide_wav.exe"
CCM = JK_BUILD / "eval" / "ccmixter_voices" / "tracks"
# Instrumental per voice (ccMixter, CC BY; licences in eval/ccmixter_voices/tracks.json);
# two voices also get that upload's backing vocals at -6 dB re the lead.
MIXES = {"bass": ("ccm-29611", False), "tenor": ("ccm-31416", True),
         "alto": ("ccm-59840", True), "soprano": ("ccm-67424", False)}
METHODS = ["pyin", "crepe", "fcpe", "pesto", "swiftf0"]
OWNER_SRC = REPO / "dist" / "Library"
# vocadito (Bittner et al. 2021, CC BY 4.0, zenodo.org/records/5578807): 40 solo vocal
# excerpts with note annotations by two annotators. Evaluation only, outside the repo.
VOC = G / "vocadito"


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


def write_mono(path: Path, x: np.ndarray, sr: int):
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(path), np.asarray(x, np.float32), sr, subtype="FLOAT")


def read_mono(path: Path):
    x, sr = sf.read(str(path), dtype="float32", always_2d=True)
    return x.mean(axis=1), sr


# ----------------------------------------------------------------------------- stages

def stage_synth(force=False):
    for v in synth.VOICES:
        wav, tj = G / "synth" / f"{v.name}.wav", G / "synth" / f"{v.name}.truth.json"
        if wav.exists() and tj.exists() and not force:
            continue
        t0 = time.perf_counter()
        y, truth = synth.sing(v)
        write_mono(wav, y, synth.SR)
        tj.write_text(json.dumps(truth, indent=1))
        log(f"synth {v.name}: {truth['duration_s']:.1f} s, {len(truth['notes'])} notes, "
            f"{len(truth['unpitched'])} spoken syllables, {time.perf_counter() - t0:.1f} s")


def stage_mix(force=False):
    import soxr
    from jukeoke_studio.separate import Separator

    sep = None
    for v in synth.VOICES:
        out = G / "sep" / f"{v.name}.lead.wav"
        if out.exists() and not force:
            continue
        voc, sr = read_mono(G / "synth" / f"{v.name}.wav")
        tid, backing = MIXES[v.name]
        inst, isr = sf.read(str(CCM / tid / "instrumental.wav"), dtype="float32", always_2d=True)
        inst = soxr.resample(inst, isr, sr, quality="VHQ").astype(np.float32)
        start = 20 * sr
        seg = inst[start:start + len(voc)]
        if len(seg) < len(voc):
            seg = np.pad(seg, ((0, len(voc) - len(seg)), (0, 0)))
        active = np.abs(voc) > 0.02
        v_rms = np.sqrt(np.mean(voc[active] ** 2))
        i_rms = np.sqrt(np.mean(seg ** 2)) + 1e-9
        mix = np.stack([voc, voc], axis=1) + seg * (v_rms / i_rms)  # vocal = accompaniment (0 dB)
        if backing:
            bk, bsr = sf.read(str(CCM / tid / "backing.wav"), dtype="float32", always_2d=True)
            bk = soxr.resample(bk, bsr, sr, quality="VHQ").astype(np.float32)[start:start + len(voc)]
            bk = np.pad(bk, ((0, len(voc) - len(bk)), (0, 0)))
            b_act = np.abs(bk).max(axis=1) > 0.01
            b_rms = np.sqrt(np.mean(bk[b_act] ** 2)) + 1e-9 if b_act.any() else 1.0
            mix += bk * (v_rms / b_rms) * 10 ** (-6 / 20)
        peak = np.abs(mix).max()
        if peak > 0.89:
            mix *= 0.89 / peak
        (G / "sep").mkdir(parents=True, exist_ok=True)
        sf.write(str(G / "sep" / f"{v.name}.mix.wav"), mix, sr, subtype="FLOAT")
        if sep is None:
            sep = Separator(device="cuda")
        t0 = time.perf_counter()
        res = sep.separate(np.ascontiguousarray(mix.T), sr, preset="full")
        lead = res.stems["lead_vocal"].mean(axis=0)
        write_mono(out, lead, sr)
        log(f"mix+separate {v.name} ({tid}{' + backing' if backing else ''}): {time.perf_counter() - t0:.1f} s")
    if sep is not None:
        sep.unload()


def song_sources():
    srcs = {Path(p).stem: Path(p) for p in sorted(glob.glob(str(JK_BUILD / "e2e-stick" / "Library" / "*.jksong")))}
    srcs["demo-song"] = REPO / "tests" / "fixtures" / "generated" / "demo-song.jksong"
    owner = [p for p in OWNER_SRC.glob("*.jksong") if p.name != "demo-song.jksong"]
    if owner:
        # Read only: copy first, analyse the copy (WO2 3 and this workstream's brief).
        dst = G / "owner" / owner[0].name
        if not dst.exists():
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(owner[0], dst)
        srcs["owner"] = dst
    return srcs


def stage_songs(force=False):
    from jukeoke_studio.jksong import Song

    for name, p in song_sources().items():
        out = G / "songs" / f"{name}.lead.wav"
        if out.exists() and not force:
            continue
        s = Song(p)
        x = s.read_stem("lead_vocal")
        x = x if x.ndim == 1 else (x.mean(axis=0) if x.shape[0] <= 8 else x.mean(axis=1))
        write_mono(out, x, int(s.manifest["sample_rate"]))
        log(f"song {name}: {len(x) / s.manifest['sample_rate']:.1f} s lead vocal")


def inputs():
    """name -> wav path for every vocal the guides are built on."""
    d = {}
    for v in synth.VOICES:
        d[f"synth-{v.name}"] = G / "synth" / f"{v.name}.wav"
        d[f"sep-{v.name}"] = G / "sep" / f"{v.name}.lead.wav"
    for p in sorted((G / "songs").glob("*.lead.wav")):
        d["song-" + p.name[:-len(".lead.wav")]] = p
    for p in sorted((VOC / "Audio").glob("vocadito_*.wav"), key=lambda q: int(q.stem.split("_")[1])):
        d["voc-" + p.stem.split("_")[1]] = p
    return d


def vocadito_truth(num: str, annotator: str = "A1") -> dict:
    """vocadito note annotations (onset, Hz, duration) in the synthetic truth's shape.
    Lyrics files are never extracted or read."""
    rows = np.loadtxt(VOC / "Annotations" / "Notes" / f"vocadito_{num}_notes{annotator}.csv", delimiter=",", ndmin=2)
    dur = sf.info(str(VOC / "Audio" / f"vocadito_{num}.wav")).duration
    notes = [{"onset": float(a), "offset": float(a + c), "hz": float(b),
              "midi": float(69 + 12 * np.log2(b / 440))} for a, b, c in rows if b > 0 and c > 0]
    return {"notes": notes, "unpitched": [], "breaths": [], "duration_s": dur, "tuning_cents": 0.0}


def stage_track(force=False, device="cuda"):
    timing_path = G / "timing-neural.json"
    timing = json.loads(timing_path.read_text()) if timing_path.exists() else {}
    for tr in trackers.TRACKERS:
        warmed = False
        for name, wav in inputs().items():
            out = G / "curves" / tr / f"{name}.csv"
            if out.exists() and not force:
                continue
            out.parent.mkdir(parents=True, exist_ok=True)
            x, sr = read_mono(wav)
            if not warmed:
                trackers.run(tr, x[: sr * 5], sr, device)  # model load + kernel warm-up
                warmed = True
            t, f0, conf, secs = trackers.run(tr, x, sr, device)
            trackers.write_curve(out, t, f0, conf)
            timing.setdefault(tr, {})[name] = {"seconds": round(secs, 3), "audio_s": round(len(x) / sr, 2),
                                               "device": device if tr != "swiftf0" else "cpu (onnxruntime)"}
            log(f"track {tr} {name}: {secs:.2f} s for {len(x) / sr:.1f} s")
        timing_path.write_text(json.dumps(timing, indent=1))


def run_guide(wav: Path, out: Path, curve_in: Path | None = None, curve_out: Path | None = None, repeat=1):
    cmd = [str(EXE), "--wav", str(wav), "--out", str(out), "--repeat", str(repeat)]
    if curve_in:
        cmd += ["--curve", str(curve_in)]
    if curve_out:
        cmd += ["--curve-out", str(curve_out)]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"guide_wav failed: {r.stderr}")
    return json.loads(out.read_text())


def stage_guide(force=False):
    for name, wav in inputs().items():
        for m in METHODS:
            out = G / "guides" / m / f"{name}.json"
            if out.exists() and not force:
                continue
            out.parent.mkdir(parents=True, exist_ok=True)
            if m == "pyin":
                (G / "curves" / "pyin").mkdir(parents=True, exist_ok=True)
                run_guide(wav, out, curve_out=G / "curves" / "pyin" / f"{name}.csv")
            else:
                run_guide(wav, out, curve_in=G / "curves" / m / f"{name}.csv")
    log("guides built")


def load_guide(m, name):
    return json.loads((G / "guides" / m / f"{name}.json").read_text())


def concat(items):
    """Pools several (truth, guide) pairs into one by shifting times."""
    T = {"notes": [], "unpitched": [], "breaths": [], "duration_s": 0.0, "tuning_cents": None}
    E = {"notes": [], "tuning_cents": None}
    off = 0.0
    for truth, guide in items:
        for n in truth["notes"]:
            T["notes"].append({**n, "onset": n["onset"] + off, "offset": n["offset"] + off})
        for k in ("unpitched", "breaths"):
            T[k] += [{"onset": s["onset"] + off, "offset": s["offset"] + off} for s in truth[k]]
        E["notes"] += [{**n, "start": n["start"] + off, "end": n["end"] + off} for n in guide["notes"]]
        T["tuning_cents"] = truth["tuning_cents"]
        off += truth["duration_s"] + 1.0
    T["duration_s"] = off
    tun = [g["tuning_cents"] for _, g in items]
    E["tuning_cents"] = float(np.mean(tun))
    return T, E


def stage_eval():
    res = {"synthetic": {}, "songs": {}, "agreement": {}}
    truths = {v.name: json.loads((G / "synth" / f"{v.name}.truth.json").read_text()) for v in synth.VOICES}
    for cond in ("synth", "sep"):
        for m in METHODS:
            per = {}
            items = []
            for v in synth.VOICES:
                g = load_guide(m, f"{cond}-{v.name}")
                per[v.name] = metrics.note_metrics(truths[v.name], g)
                per[v.name]["tuning_cents"] = g["tuning_cents"]
                items.append((truths[v.name], g))
            T, E = concat(items)
            pooled = metrics.note_metrics(T, E)
            pooled["tuning_by_voice"] = [per[v.name]["tuning_cents"] for v in synth.VOICES]
            res["synthetic"][f"{cond}/{m}"] = {"pooled": pooled, "per_voice": per}
    voc = [n.split("-", 1)[1] for n in inputs() if n.startswith("voc-")]
    if voc:
        for m in METHODS + ["annotator2"]:
            items = []
            for num in voc:
                t = vocadito_truth(num)
                g = ({"notes": [{"start": n["onset"], "end": n["offset"], "midi": n["midi"], "kind": "pitched"}
                                for n in vocadito_truth(num, "A2")["notes"]], "tuning_cents": 0.0}
                     if m == "annotator2" else load_guide(m, f"voc-{num}"))
                items.append((t, g))
            T, E = concat(items)
            res["synthetic"][f"vocadito/{m}"] = {"pooled": metrics.note_metrics(T, E)}
    song_names = [n for n in inputs() if n.startswith("song-")]
    for name in song_names:
        res["songs"][name] = {m: metrics.counts(load_guide(m, name)) for m in METHODS}
        res["agreement"][name] = {m: metrics.agreement(load_guide("pyin", name), load_guide(m, name))
                                  for m in METHODS if m != "pyin"}
    # Agreement on the synthetic sets too (to relate agreement to accuracy).
    for cond in ("synth", "sep"):
        for v in synth.VOICES:
            nm = f"{cond}-{v.name}"
            res["agreement"][nm] = {m: metrics.agreement(load_guide("pyin", nm), load_guide(m, nm))
                                    for m in METHODS if m != "pyin"}
    (G / "results.json").write_text(json.dumps(res, indent=1, default=float))
    log("results written:", G / "results.json")
    return res


def stage_plots(best_neural="swiftf0"):
    pics = G / "pictures"
    pics.mkdir(exist_ok=True)
    for name in inputs():
        if name.startswith("voc-") and int(name[4:]) > 3:
            continue  # three vocadito pictures are enough to look at
        truth = None
        if name.startswith(("synth-", "sep-")):
            truth = json.loads((G / "synth" / f"{name.split('-', 1)[1]}.truth.json").read_text())
        elif name.startswith("voc-"):
            truth = vocadito_truth(name[4:])
        g = load_guide("pyin", name)
        o = load_guide(best_neural, name)
        plots.guide_picture(pics / f"{name}.pyin.png", f"{name}: Rust pYIN guide", G / "curves" / "pyin" / f"{name}.csv",
                            g, other=o, other_label=f"{best_neural} guide", truth=truth)
        plots.guide_picture(pics / f"{name}.{best_neural}.png", f"{name}: {best_neural} guide",
                            G / "curves" / best_neural / f"{name}.csv", o, truth=truth)
    log("pictures in", pics)


def stage_speed():
    """Rust builder on a 4-minute vocal (5 runs, fastest), neural trackers on GPU and CPU."""
    import torch

    out = {}
    name = "song-Cortez - Feel (Stripped)"
    wav = inputs()[name]
    g = run_guide(wav, G / "speed-rust.json", repeat=5)
    out["rust"] = {"vocal": name, "audio_s": g["duration_s"], **g["timing_s"]}
    x, sr = read_mono(wav)
    for tr in trackers.TRACKERS:
        for dev in ("cuda", "cpu"):
            if tr == "swiftf0" and dev == "cuda":
                continue
            trackers.run(tr, x[: sr * 5], sr, dev)
            secs = trackers.run(tr, x, sr, dev)[3]
            out[f"{tr}/{dev}"] = {"seconds": round(secs, 2), "audio_s": round(len(x) / sr, 1)}
            log(f"speed {tr} {dev}: {secs:.2f} s")
    out["cpu"] = {"torch_threads": torch.get_num_threads()}
    (G / "speed.json").write_text(json.dumps(out, indent=1))
    log("speed written")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("stage", choices=["synth", "mix", "songs", "track", "guide", "eval", "plots", "speed", "all"])
    ap.add_argument("--force", action="store_true")
    a = ap.parse_args()
    G.mkdir(parents=True, exist_ok=True)
    st = a.stage
    if st in ("synth", "all"):
        stage_synth(a.force)
    if st in ("mix", "all"):
        stage_mix(a.force)
    if st in ("songs", "all"):
        stage_songs(a.force)
    if st in ("track", "all"):
        stage_track(a.force)
    if st in ("guide", "all"):
        stage_guide(a.force)
    if st in ("eval", "all"):
        stage_eval()
    if st in ("plots", "all"):
        stage_plots()
    if st in ("speed", "all"):
        stage_speed()


if __name__ == "__main__":
    main()

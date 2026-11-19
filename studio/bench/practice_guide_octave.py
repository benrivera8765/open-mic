# SPDX-License-Identifier: GPL-3.0-or-later
"""WO2 Stage 2: the phrase-context octave check (jk-pitch builder 0.2.0) before and after,
on the Stage 0 benchmark subset (docs/benchmarks/practice-guide.md, "Stage 2").

For each setting the Rust builder (``guide_wav`` example, this checkout's code) is run on
the synthetic singer alone and through the real separation, the 40 vocadito excerpts and
the real songs' lead stems already extracted by Stage 0 (``$JK_BUILD/practice-guide``):

- ``before``: ``--set octave_context=0`` (the spectral check alone, as builder 0.1.0);
- ``run``: the default (the spectral check, then the run rule);
- ``run+isolated``: ``--set octave_context=2`` (also the isolated-jump rule).

The octave check changes no note: it only flags. So note F is the same in every setting;
what moves is which notes are flagged (drawn dim, not graded). Reported per set: octave
errors (each true note against the guide note covering most of it, at least 30 %, 11-13
semitones away: Stage 0's rule), how many of them are flagged, octave errors still graded
(unflagged and confidence >= 0.5), and correct notes (within 50 cents) newly flagged.

    $JK_BUILD/practice-guide/venv/Scripts/python.exe -I studio/bench/practice_guide_octave.py

Writes ``$JK_BUILD/practice-stage2/octave/<setting>/*.json`` and ``results.json``. Lyrics
are never read.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import practice_guide_bench as B  # noqa: E402
from practice import metrics, synth  # noqa: E402

EXE = B.JK_BUILD / "target-wo2-stage2" / "release" / "examples" / "guide_wav.exe"
OUT = B.JK_BUILD / "practice-stage2" / "octave"
SETTINGS = {"before": ["--set", "octave_context=0"], "run": [], "run+isolated": ["--set", "octave_context=2"]}


def build(setting: str, name: str, wav: Path) -> dict:
    out = OUT / setting / f"{name}.json"
    if not out.exists():
        out.parent.mkdir(parents=True, exist_ok=True)
        r = subprocess.run([str(EXE), "--wav", str(wav), "--out", str(out), *SETTINGS[setting]],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError(r.stderr)
    return json.loads(out.read_text())


def octave_view(truth: dict, guide: dict, low: float = 0.5) -> dict:
    """Octave errors and flags of one guide against its truth (Stage 0's matching)."""
    est = [n for n in guide["notes"] if n["kind"] == "pitched"]
    if not est:
        return {"octave_errors": 0, "octave_flagged": 0, "octave_graded": 0, "correct": 0, "correct_flagged": 0}
    iv = np.array([[n["start"], max(n["end"], n["start"] + 1e-3)] for n in est])
    hz = metrics.midi_to_hz([n["midi"] for n in est])
    flagged = np.array([n["octave"] in ("maybe_high", "maybe_low") for n in est])
    conf = np.array([n["conf"] for n in est])
    oe = of = og = 0
    for t in truth["notes"]:
        a, b = t["onset"], t["offset"]
        ov = np.clip(np.minimum(iv[:, 1], b) - np.maximum(iv[:, 0], a), 0, None)
        j = int(np.argmax(ov))
        if ov[j] < 0.3 * (b - a):
            continue
        e = 1200 * np.log2(hz[j] / t["hz"])
        k = round(e / 1200)
        if k != 0 and abs(e - 1200 * k) < 100:
            oe += 1
            of += int(flagged[j])
            og += int(not flagged[j] and conf[j] >= low)
    # Guide notes judged against the truth note covering most of them.
    ref_iv = np.array([[t["onset"], t["offset"]] for t in truth["notes"]]).reshape(-1, 2)
    ref_hz = np.array([t["hz"] for t in truth["notes"]])
    correct = correct_flagged = 0
    for j, n in enumerate(est):
        a, b = iv[j]
        ov = np.clip(np.minimum(ref_iv[:, 1], b) - np.maximum(ref_iv[:, 0], a), 0, None)
        if len(ov) == 0:
            continue
        i = int(np.argmax(ov))
        if ov[i] < 0.3 * (b - a):
            continue
        if abs(1200 * np.log2(hz[j] / ref_hz[i])) < 50:
            correct += 1
            correct_flagged += int(flagged[j])
    return {"octave_errors": oe, "octave_flagged": of, "octave_graded": og, "correct": correct,
            "correct_flagged": correct_flagged}


def add(a: dict, b: dict) -> dict:
    return {k: a.get(k, 0) + b[k] for k in b}


def main() -> None:
    if not EXE.exists():
        raise SystemExit(f"build the example first: cargo build --release -p jk-pitch --example guide_wav ({EXE})")
    inputs = B.inputs()
    truths = {v.name: json.loads((B.G / "synth" / f"{v.name}.truth.json").read_text()) for v in synth.VOICES}
    res: dict = {}
    for setting in SETTINGS:
        r: dict = {"sets": {}, "songs": {}}
        for cond in ("synth", "sep"):
            tot: dict = {}
            items = []
            for v in synth.VOICES:
                g = build(setting, f"{cond}-{v.name}", inputs[f"{cond}-{v.name}"])
                tot = add(tot, octave_view(truths[v.name], g))
                items.append((truths[v.name], g))
            T, E = B.concat(items)
            tot["f"] = metrics.note_metrics(T, E)["f"]
            r["sets"][cond] = tot
        tot = {}
        items = []
        for name in [n for n in inputs if n.startswith("voc-")]:
            t = B.vocadito_truth(name.split("-", 1)[1])
            g = build(setting, name, inputs[name])
            tot = add(tot, octave_view(t, g))
            items.append((t, g))
        if items:
            T, E = B.concat(items)
            tot["f"] = metrics.note_metrics(T, E)["f"]
            r["sets"]["vocadito"] = tot
        for name in [n for n in inputs if n.startswith("song-")]:
            g = build(setting, name, inputs[name])
            r["songs"][name] = metrics.counts(g)
        # The owner's stretches (Stage 0 open item): the state of every pitched note there.
        if "song-owner" in inputs:
            g = build(setting, "song-owner", inputs["song-owner"])
            win = []
            for n in g["notes"]:
                if n["kind"] == "pitched" and any(a <= n["start"] <= b for a, b in ((46.0, 46.8), (158.5, 159.4), (213.4, 215.8))):
                    win.append({"start": n["start"], "end": n["end"], "midi": round(n["midi"], 2),
                                "octave": n["octave"], "graded": n["octave"] in ("ok", "unchecked") and n["conf"] >= 0.5})
            r["owner_windows"] = win
        res[setting] = r
        print(setting, json.dumps(r["sets"]))
    (OUT / "results.json").write_text(json.dumps(res, indent=1))
    print("written", OUT / "results.json")


if __name__ == "__main__":
    main()

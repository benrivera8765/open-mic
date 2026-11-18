# SPDX-License-Identifier: GPL-3.0-or-later
"""Measurements on the song files the real app made in the end-to-end run
(docs/benchmarks/end-to-end.md).

For every ``.jksong`` in the stick's Library:

- ``jukeoke-cli verify --strict`` (the Rust reader: format checklist and a full decode);
- offline renders at keys 0, +2 and -5 (``jukeoke-cli render``, the playback graph):
  length against the song, peak, and the key-0 render against the plain sum of the
  karaoke-mix stems (the 0-semitone path is meant to be bit-transparent). The key of the
  +2 / -5 renders is checked on the music itself: the circular chroma shift (librosa CQT,
  60 s from 30 %) that best maps each frame of the key-0 render onto the same frame of the
  shifted render must be +2 and +7 (= -5) semitones;
- the stems against the decoded source (``stems_sum_to_source``);
- word-onset error of the stored lyrics against the JamendoLyrics annotation
  (|predicted start - annotated start| per word, words matched in order; the Studio's
  ``lyrics.from_text`` keeps the dataset's word count): median, p90, worst, share within
  0.3 s;
- for the ccMixter mix (true stems known): residual lead vocal in the karaoke mix
  (``quality.leak_db``, the separation benchmark's "lead residue"), with the backing
  vocals on (default mix) and off, and SDR of the karaoke mix and of each vocal stem.

Usage (Git Bash)::

    $JK_BUILD/studio-venv/Scripts/python.exe -I studio/bench/e2e_metrics.py \
        --library $JK_BUILD/e2e-stick/Library --cli $JK_BUILD/target-e2e/release/jukeoke-cli.exe \
        --eval $JK_BUILD/eval --inputs $JK_BUILD/e2e/inputs --out $JK_BUILD/e2e/metrics
"""

from __future__ import annotations

import argparse
import csv
import json
import subprocess
import time
from pathlib import Path

import numpy as np
import soundfile as sf

from jukeoke_studio import jksong, media, quality

# Which Library song is which evaluation item (by the title the job gave it).
TRUTH = {
    "Feel (Stripped)": {"jamendo": "Cortez_-_Feel__Stripped_", "source": "solo-48k.flac"},
    "Crowdpleaser": {"jamendo": "JASON_MILLER_-_CROWD_PLEASER", "source": "wordy.mp4"},
    "The Homeless Man": {"ccmixter": "backing-mix", "source": "backing-mix.wav"},
}
KEYS = (0, 2, -5)


def run(cmd: list[str]) -> tuple[int, str, float]:
    t = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.returncode, (r.stdout + r.stderr), time.perf_counter() - t


def word_onsets(lyrics: dict) -> list[tuple[str, float]]:
    return [("".join(c[0] for c in w["chars"]), float(w["start"])) for line in lyrics["lines"] for w in line["words"]]


def onset_errors(pred: list[tuple[str, float]], ann_csv: Path) -> dict:
    gt = [float(r["word_start"]) for r in csv.DictReader(open(ann_csv, encoding="utf-8"))]
    n = min(len(pred), len(gt))
    err = np.abs(np.array([p[1] for p in pred[:n]]) - np.array(gt[:n]))
    worst = int(np.argmax(err))
    return {"words_pred": len(pred), "words_truth": len(gt), "matched": n,
            "median_s": float(np.median(err)), "mean_s": float(np.mean(err)), "p90_s": float(np.percentile(err, 90)),
            "p95_s": float(np.percentile(err, 95)), "worst_s": float(err[worst]), "worst_word": pred[worst][0],
            "worst_at_truth_s": gt[worst], "within_0_3s": float(np.mean(err <= 0.3)),
            "within_0_1s": float(np.mean(err <= 0.1))}


def chroma_shift(ref: np.ndarray, est: np.ndarray, sr: int) -> dict:
    import librosa

    def chroma(x):
        mono = x.mean(axis=0)
        n0 = int(0.3 * len(mono))
        seg = mono[n0:n0 + 60 * sr]
        return librosa.feature.chroma_cqt(y=seg.astype(np.float32), sr=sr)  # [12, frames]

    # Frame by frame: the key change keeps the timing, so frame t of the shifted render
    # should be frame t of the key-0 render rotated by the shift. Score of shift k = mean
    # over frames of the correlation of roll(C0[:, t], k) with Ck[:, t].
    a, b = chroma(ref), chroma(est)
    n = min(a.shape[1], b.shape[1])
    a, b = a[:, :n], b[:, :n]

    def corr(u, v):
        u = u - u.mean(axis=0)
        v = v - v.mean(axis=0)
        return float(np.mean(np.sum(u * v, axis=0) / (np.linalg.norm(u, axis=0) * np.linalg.norm(v, axis=0) + 1e-12)))

    scores = [corr(np.roll(a, k, axis=0), b) for k in range(12)]
    order = np.argsort(scores)[::-1]
    return {"best_shift": int(order[0]), "score": scores[order[0]], "runner_up_shift": int(order[1]),
            "runner_up_score": scores[order[1]], "score_unshifted": scores[0], "frames": int(n)}


def read_wav(path: Path) -> np.ndarray:
    x, _ = sf.read(path, dtype="float32", always_2d=True)
    return x.T


def song_metrics(path: Path, cli: str, eval_dir: Path, inputs: Path, out: Path) -> dict:
    s = jksong.open_song(path)
    m = s.manifest
    title = s.effective_metadata()["title"]
    sr = int(m["sample_rate"])
    r: dict = {"file": path.name, "bytes": path.stat().st_size, "title": title, "artist": m.get("artist"),
               "sample_rate": sr, "channels": m["channels"], "duration_s": m["duration_seconds"],
               "preset": m.get("preset"), "key": (m.get("analysis") or {}).get("key"),
               "tempo_bpm": (m.get("analysis") or {}).get("tempo_bpm"),
               "resampling": m["provenance"].get("resampling"), "source": m["provenance"].get("source"),
               "models": [x.get("name") for x in m["provenance"].get("models", [])],
               "stems": [{k: st.get(k) for k in ("role", "gain", "peak", "in_karaoke_mix", "pitched")} for st in m["stems"]]}
    code, text, secs = run([cli, "verify", "--strict", str(path)])
    r["verify"] = {"exit": code, "seconds": round(secs, 1), "tail": text.strip().splitlines()[-6:]}

    lyr = s.lyrics()
    if lyr:
        r["lyrics"] = {"source": lyr.get("source"), "checked": lyr.get("checked"), "lines": len(lyr["lines"]),
                       "words": sum(len(line["words"]) for line in lyr["lines"]),
                       "graphemes": sum(len(w["chars"]) for line in lyr["lines"] for w in line["words"]),
                       "alignment": lyr.get("alignment")}
    else:
        r["lyrics"] = None
    t = TRUTH.get(title, {})
    if lyr and "jamendo" in t:
        r["word_onsets"] = onset_errors(word_onsets(lyr), eval_dir / "jamendolyrics" / "annotations" / "words" / f"{t['jamendo']}.csv")

    stems = {st["role"]: s.read_stem(st["role"]) for st in m["stems"]}
    karaoke = sum(stems[st["role"]] for st in m["stems"] if st.get("in_karaoke_mix"))
    if "source" in t:
        src = media.decode(inputs / t["source"]).audio
        n = min(src.shape[1], karaoke.shape[1])
        total = sum(stems.values())
        r["stems_sum_vs_source_max"] = float(np.max(np.abs(total[:, :n] - src[:, :n])))
        r["frames_song_vs_source"] = [int(karaoke.shape[1]), int(src.shape[1])]

    renders = {}
    for k in KEYS:
        wav = out / f"{path.stem}.key{k:+d}.wav"
        code, text, secs = run([cli, "render", str(path), "--out", str(wav), "--format", "wav32f", "--semitones", str(k)])
        info = {"exit": code, "seconds": round(secs, 1)}
        if code == 0:
            x = read_wav(wav)
            info.update(frames=int(x.shape[1]), peak=float(np.max(np.abs(x))),
                        length_equal=int(x.shape[1]) == int(m["duration_frames"]))
            renders[k] = x
        else:
            info["tail"] = text.strip().splitlines()[-4:]
        r.setdefault("renders", {})[f"{k:+d}"] = info
    if 0 in renders:
        x0 = renders[0]
        n = min(x0.shape[1], karaoke.shape[1])
        r["renders"]["+0"]["max_diff_vs_stem_sum"] = float(np.max(np.abs(x0[:, :n] - karaoke[:, :n])))
        for k in (2, -5):
            if k in renders:
                r["renders"][f"{k:+d}"]["chroma"] = chroma_shift(x0, renders[k], sr)
                r["renders"][f"{k:+d}"]["chroma"]["expected_shift"] = k % 12
    for k in KEYS:  # renders are large; the numbers are kept
        (out / f"{path.stem}.key{k:+d}.wav").unlink(missing_ok=True)

    if "ccmixter" in t:
        tr = inputs / "truth"
        lead = np.load(tr / "backing-mix.lead.npy")
        back = np.load(tr / "backing-mix.backing.npy")
        inst = np.load(tr / "backing-mix.instrumental.npy")
        n = min(lead.shape[1], karaoke.shape[1])
        lead, back, inst = lead[:, :n], back[:, :n], inst[:, :n]
        est_k = karaoke[:, :n]
        est_off = (stems["drums"] + stems["other"])[:, :n]
        r["residual"] = {
            "lead_residue_karaoke_db": quality.leak_db(est_k, back + inst, lead, sr),
            "lead_residue_backing_off_db": quality.leak_db(est_off, inst, lead, sr),
            "karaoke_sdr_db": quality.sdr(back + inst, est_k),
            "backing_off_sdr_db": quality.sdr(inst, est_off),
            "lead_sdr_db": quality.sdr(lead, stems["lead_vocal"][:, :n]),
            "backing_sdr_db": quality.sdr(back, stems["backing_vocals"][:, :n]),
            "backing_energy_kept_db": quality.energy_db(stems["backing_vocals"][:, :n], back),
            "band_energy_karaoke_db": quality.band_energy_db(est_k, back + inst, sr),
            "seconds": n / sr,
        }
    return r


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--library", required=True, type=Path)
    ap.add_argument("--cli", required=True)
    ap.add_argument("--eval", required=True, type=Path)
    ap.add_argument("--inputs", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    results = []
    for p in jksong.list_song_files(a.library):
        print(f"== {p.name}", flush=True)
        r = song_metrics(p, a.cli, a.eval, a.inputs, a.out)
        print(json.dumps({k: r.get(k) for k in ("verify", "word_onsets", "renders", "residual", "stems_sum_vs_source_max")}, indent=1), flush=True)
        results.append(r)
    (a.out / "metrics.json").write_text(json.dumps(results, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()

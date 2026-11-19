# SPDX-License-Identifier: GPL-3.0-or-later
"""Prepare the REAL-VOICE evaluation set for the live pitch tracker (WORK_ORDER-2 4.4,
docs/benchmarks/practice-tracker.md section 13). Evaluation only: nothing here ships, and
every output goes under $JK_BUILD/practice-tracker/eval (outside the repository).

Sources (all local, never copied into the repository):
- vocadito (CC BY 4.0, 40 solo a-cappella recordings, 29 singers) with its MANUALLY
  CORRECTED f0 annotations (Annotations/F0, unpacked from the downloaded zip);
- ccMixter lead stems (eval set, Creative Commons);
- MUSDB18-HQ test vocal stems (research licence, local evaluation only);
- the isolated lead vocal (Studio separation) of the three Creative Commons test songs.

For every source but vocadito the reference is an AGREED offline reference: librosa pYIN
and torchcrepe "full" (Viterbi decoding), both on 10 ms frames; a frame is
  1 = voiced  when pYIN calls it voiced and CREPE is within 50 cents of it (f0 = pYIN's),
  0 = unvoiced when pYIN calls it unvoiced and CREPE's periodicity is under 0.21,
  2 = undecided otherwise (left out of every frame metric).
vocadito frames are 1 where the annotation has an f0 and 0 where it has none.

Each clip is ONE channel of the source (channel 0; channels are never summed, as in the
app), at the source's own sample rate, written as raw little-endian f32. Long tracks are cut
to the 60 s window with the most active 100 ms frames. Lyrics are never read.

Split: by singer / artist group (a stable hash, about 45 % "dev"); the ccMixter artist
"coruscate" and the three test songs are always "heldout" (one of the songs is coruscate's).
Tune on "dev" only; report "heldout".

Run with the guide benchmark's overlay venv (torch + torchcrepe + librosa + soxr):
  $JK_BUILD/practice-guide/venv/Scripts/python.exe -I crates/jk-pitch/examples/tracker_eval_prep.py
"""

from __future__ import annotations

import csv
import hashlib
import json
import os
import sys
from multiprocessing import Pool

import numpy as np
import soundfile as sf
import soxr

BUILD = os.environ.get("JK_BUILD") or os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "..", "jukeoke-build"))
OUT = os.path.join(BUILD, "practice-tracker", "eval")
EXCERPT_S = 60.0
HOP_S = 0.01


def split_of(group: str) -> str:
    """Stable split by singer / artist group (about 45 % dev)."""
    if group in ("ccmixter:coruscate",) or group.startswith("songs:"):
        return "heldout"
    h = int(hashlib.md5(group.encode()).hexdigest()[:8], 16)
    return "dev" if h % 100 < 45 else "heldout"


def sources() -> list[dict]:
    out = []
    voc = os.path.join(BUILD, "practice-guide", "vocadito")
    singers = {r["track_id"]: r["singer_id"] for r in csv.DictReader(open(os.path.join(voc, "vocadito_metadata.csv")))}
    f0dir = os.path.join(BUILD, "practice-tracker", "data", "vocadito-f0")
    for tid, singer in sorted(singers.items(), key=lambda kv: int(kv[0])):
        out.append(dict(dataset="vocadito", id=f"vocadito_{tid}", group=f"vocadito:{singer}",
                        path=os.path.join(voc, "Audio", f"vocadito_{tid}.wav"),
                        annotation=os.path.join(f0dir, f"vocadito_{tid}_f0.csv")))
    ccm = os.path.join(BUILD, "eval", "ccmixter_voices")
    for t in json.load(open(os.path.join(ccm, "tracks.json"))):
        p = os.path.join(ccm, "tracks", t["track"], "lead.wav")
        if os.path.exists(p):
            out.append(dict(dataset="ccmixter", id=t["track"], group="ccmixter:" + t["artist"].lower(), path=p))
    mus = os.path.join(BUILD, "eval", "musdb18hq", "test")
    for i, name in enumerate(sorted(os.listdir(mus))):
        p = os.path.join(mus, name, "vocals.wav")
        if os.path.exists(p):
            # Track names are "Artist - Title"; the id avoids the title (a number).
            out.append(dict(dataset="musdb", id=f"musdb_{i:02d}", group="musdb:" + name.split(" - ")[0].lower(),
                            path=p, name=name))
    songs = os.path.join(BUILD, "practice-guide", "songs")
    for i, name in enumerate(["Cortez - Feel (Stripped)", "Jason Miller - Crowdpleaser", "coruscate - The Homeless Man"]):
        out.append(dict(dataset="songs", id=f"song_{i}", group="songs:" + name.split(" - ")[0].lower(),
                        path=os.path.join(songs, f"{name}.lead.wav"), name=name))
    return out


def excerpt(x: np.ndarray, sr: int) -> tuple[np.ndarray, float]:
    """The EXCERPT_S window with the most active 100 ms frames (within 30 dB of the loud level)."""
    if len(x) <= EXCERPT_S * sr:
        return x, 0.0
    hop = sr // 10
    n = len(x) // hop
    rms = np.sqrt(np.mean(x[: n * hop].reshape(n, hop).astype(np.float64) ** 2, axis=1) + 1e-20)
    db = 20 * np.log10(rms)
    act = (db > np.percentile(db, 95) - 30).astype(float)
    w = int(EXCERPT_S * 10)
    best = int(np.argmax(np.convolve(act, np.ones(w), "valid")))
    return x[best * hop: best * hop + int(EXCERPT_S * sr)], best * hop / sr


def pyin_job(args):
    path16, n_frames = args
    import librosa
    y = np.fromfile(path16, dtype="<f4")
    f0, vflag, vprob = librosa.pyin(y, fmin=60.0, fmax=1400.0, sr=16000, frame_length=1024,
                                    hop_length=160, center=True)
    f0 = np.nan_to_num(f0)[:n_frames]
    vflag = vflag[:n_frames]
    return f0, vflag


def main() -> None:
    os.makedirs(os.path.join(OUT, "clips"), exist_ok=True)
    os.makedirs(os.path.join(OUT, "ref"), exist_ok=True)
    os.makedirs(os.path.join(OUT, "tmp16k"), exist_ok=True)
    todo = sources()
    rows = []
    pyin_jobs = []
    for s in todo:
        x, sr = sf.read(s["path"], dtype="float32", always_2d=True)
        x = x[:, 0].copy()  # one channel, never a sum
        if s["dataset"] == "vocadito":
            start = 0.0
        else:
            x, start = excerpt(x, sr)
        clip = os.path.join(OUT, "clips", s["id"] + ".f32")
        x.astype("<f4").tofile(clip)
        n_frames = int(len(x) / sr / HOP_S) + 1
        s.update(rate=sr, clip=clip, start_s=start, seconds=len(x) / sr, split=split_of(s["group"]), n_frames=n_frames)
        if s["dataset"] != "vocadito":
            p16 = os.path.join(OUT, "tmp16k", s["id"] + ".f32")
            soxr.resample(x, sr, 16000, quality="VHQ").astype("<f4").tofile(p16)
            s["p16"] = p16
            pyin_jobs.append((p16, n_frames))
        rows.append(s)
        print(s["id"], s["dataset"], s["split"], f"{s['seconds']:.1f}s", file=sys.stderr)

    # pYIN on 8 processes, CREPE on the GPU in this one.
    with Pool(8) as pool:
        pyin_res = pool.map(pyin_job, pyin_jobs)
    pyin_by = {p16: r for (p16, _), r in zip(pyin_jobs, pyin_res)}

    import torch
    import torchcrepe
    for s in rows:
        t = np.arange(s["n_frames"]) * HOP_S
        if s["dataset"] == "vocadito":
            a = np.loadtxt(s["annotation"], delimiter=",")
            idx = np.clip(np.searchsorted(a[:, 0], t), 0, len(a) - 1)
            prev = np.clip(idx - 1, 0, len(a) - 1)
            idx = np.where(np.abs(a[prev, 0] - t) < np.abs(a[idx, 0] - t), prev, idx)
            f0 = a[idx, 1]
            flag = (f0 > 0).astype(int)
            pf = cf = cp = np.zeros_like(t)
        else:
            pf, pv = pyin_by[s["p16"]]
            pf = np.pad(pf, (0, max(0, len(t) - len(pf))))[: len(t)]
            pv = np.pad(pv, (0, max(0, len(t) - len(pv))))[: len(t)].astype(bool)
            y = torch.from_numpy(np.fromfile(s["p16"], dtype="<f4"))[None]
            with torch.inference_mode():
                cf, cp = torchcrepe.predict(y, 16000, hop_length=160, fmin=50.0, fmax=1500.0, model="full",
                                            batch_size=2048, device="cuda", return_periodicity=True,
                                            decoder=torchcrepe.decode.viterbi, pad=True)
                cp = torchcrepe.filter.median(cp, 3)
            cf = np.pad(cf[0].cpu().numpy(), (0, len(t)))[: len(t)]
            cp = np.pad(cp[0].cpu().numpy(), (0, len(t)))[: len(t)]
            agree = pv & (pf > 0) & (cf > 0) & (np.abs(1200 * np.log2(np.maximum(pf, 1e-3) / np.maximum(cf, 1e-3))) <= 50)
            unv = (~pv) & (cp < 0.21)
            flag = np.where(agree, 1, np.where(unv, 0, 2))
            f0 = np.where(agree, pf, 0.0)
        ref = os.path.join(OUT, "ref", s["id"] + ".csv")
        with open(ref, "w", newline="") as fh:
            fh.write("t,f0_hz,flag\n")
            for ti, fi, gi in zip(t, f0, flag):
                fh.write(f"{ti:.3f},{fi:.3f},{int(gi)}\n")
        np.savez_compressed(os.path.join(OUT, "ref", s["id"] + "-methods.npz"), pyin=pf, crepe=cf, crepe_per=cp)
        s["ref"] = ref
        s["ref_voiced"] = int(np.sum(flag == 1))
        s["ref_unvoiced"] = int(np.sum(flag == 0))
        s["ref_undecided"] = int(np.sum(flag == 2))
        print(s["id"], "voiced", s["ref_voiced"], "unvoiced", s["ref_unvoiced"], "undecided", s["ref_undecided"], file=sys.stderr)

    with open(os.path.join(OUT, "manifest.tsv"), "w", newline="") as fh:
        fh.write("dataset\tsplit\tid\tgroup\trate\tclip\tref\n")
        for s in rows:
            fh.write(f"{s['dataset']}\t{s['split']}\t{s['id']}\t{s['group']}\t{s['rate']}\t{s['clip']}\t{s['ref']}\n")
    # Names stay outside the repository, next to the clips (for the evidence trail).
    json.dump([{k: v for k, v in s.items() if k not in ("p16",)} for s in rows],
              open(os.path.join(OUT, "manifest.json"), "w"), indent=1)


if __name__ == "__main__":
    main()

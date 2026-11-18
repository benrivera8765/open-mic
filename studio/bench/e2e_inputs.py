# SPDX-License-Identifier: GPL-3.0-or-later
"""Input files for the end-to-end run through the real app (docs/benchmarks/end-to-end.md).

Builds, from Creative Commons evaluation material that stays outside the repository
(``$JK_BUILD/eval``), the queue the app is given:

1. ``solo-48k.flac``    Cortez "Feel (Stripped)" (JamendoLyrics, CC BY): the MP3 decoded
                        with the Studio's own decoder, converted to 48 kHz once (soxr VHQ)
                        and written as 24-bit FLAC with title/artist tags. A 48 kHz
                        lossless source, so the Studio's single 48 -> 44.1 -> 48 path runs.
2. ``corrupt.m4a``      2,000,000 seeded random bytes: no container can be found. Must fail
                        with a plain message while the queue goes on.
3. ``backing-mix.wav``  coruscate "The Homeless Man" (ccMixter, CC BY 3.0): the upload's
                        own lead, backing and instrument stems summed (float32 WAV), so the
                        true lead vocal is known for the residual measurement.
4. ``wordy.mp4``        Jason Miller "Crowdpleaser" (JamendoLyrics, CC BY-NC-SA): a video
                        file, a static 320x180 H.264 picture at 1 frame/s plus the audio
                        encoded as AAC 256 kbit/s (what a phone or screen recorder makes).

Ground truth for the residual measurement is written next to them (``truth/``): the
ccMixter mix's lead vocal, backing vocals and instruments (word timings come from the
JamendoLyrics annotations). The lag between the MP4's decoded audio and the MP3 it came
from is measured (AAC priming must be removed by the edit list, else every lyric time
would be off by it).

Usage (Git Bash)::

    source scripts/devenv.sh
    $JK_BUILD/studio-venv/Scripts/python.exe -I studio/bench/e2e_inputs.py --eval $JK_BUILD/eval --out $JK_BUILD/e2e/inputs
"""

from __future__ import annotations

import argparse
import json
from fractions import Fraction
from pathlib import Path

import numpy as np
import soundfile as sf

from jukeoke_studio import media

JAMENDO_SOLO = "Cortez_-_Feel__Stripped_"
JAMENDO_WORDY = "JASON_MILLER_-_CROWD_PLEASER"
CCMIXTER_TRACK = "ccm-68853"   # coruscate - The Homeless Man, CC BY 3.0


def _peak(x: np.ndarray) -> float:
    return float(np.max(np.abs(x)))


def make_solo_flac(src: Path, out: Path, truth: Path) -> dict:
    import soxr
    from mutagen.flac import FLAC

    d = media.decode(src)
    x = d.audio  # float32 [ch, n] at 44.1 kHz
    y = soxr.resample(x.T.astype(np.float64), d.sample_rate, 48000, quality="VHQ").astype(np.float32)
    peak = _peak(y)
    # 24-bit FLAC cannot hold more than full scale; the decoded MP3 may exceed it slightly.
    scale = 1.0 if peak < 0.999 else 0.999 / peak
    sf.write(out, y * scale, 48000, subtype="PCM_24", format="FLAC")
    tags = FLAC(out)
    tags["title"] = "Feel (Stripped)"
    tags["artist"] = "Cortez"
    tags.save()
    return {"file": out.name, "from": src.name, "rate": 48000, "peak_before": peak, "scale": scale,
            "frames": int(y.shape[0])}


def make_corrupt(out: Path) -> dict:
    rng = np.random.default_rng(20261117)
    out.write_bytes(rng.bytes(2_000_000))
    return {"file": out.name, "bytes": 2_000_000, "content": "seeded random bytes (numpy default_rng 20261117)"}


def make_backing_mix(track_dir: Path, out: Path, truth: Path) -> dict:
    lead, sr = sf.read(track_dir / "lead.wav", dtype="float32", always_2d=True)
    back, sr2 = sf.read(track_dir / "backing.wav", dtype="float32", always_2d=True)
    inst, sr3 = sf.read(track_dir / "instrumental.wav", dtype="float32", always_2d=True)
    assert sr == sr2 == sr3, (sr, sr2, sr3)
    n = min(len(lead), len(back), len(inst))
    lead, back, inst = lead[:n].T, back[:n].T, inst[:n].T
    karaoke = back + inst
    mix = lead + karaoke
    sf.write(out, mix.T, sr, subtype="FLOAT", format="WAV")
    np.save(truth / "backing-mix.lead.npy", lead)
    np.save(truth / "backing-mix.backing.npy", back)
    np.save(truth / "backing-mix.instrumental.npy", inst)
    return {"file": out.name, "rate": sr, "frames": n, "peak": _peak(mix),
            "lead_rms_db": float(10 * np.log10(np.mean(lead.astype(np.float64) ** 2))),
            "backing_rms_db": float(10 * np.log10(np.mean(back.astype(np.float64) ** 2)))}


def make_video(src: Path, out: Path, truth: Path) -> dict:
    import av

    d = media.decode(src)
    x = d.audio  # [2, n] float32
    sr = d.sample_rate
    with av.open(str(out), "w", format="mp4") as c:
        c.metadata["title"] = "Crowdpleaser"
        c.metadata["artist"] = "Jason Miller"
        v = c.add_stream("libx264", rate=1)
        v.width, v.height, v.pix_fmt = 320, 180, "yuv420p"
        a = c.add_stream("aac", rate=sr)
        a.layout = "stereo"
        a.bit_rate = 256_000
        # A static picture: a dark vertical gradient.
        img = np.zeros((180, 320, 3), np.uint8)
        img[:, :, 0] = np.linspace(20, 90, 180, dtype=np.uint8)[:, None]
        img[:, :, 2] = 60
        frame_v = av.VideoFrame.from_ndarray(img, format="rgb24")
        seconds = int(np.ceil(x.shape[1] / sr))
        hop = 1024
        vi = 0
        for start in range(0, x.shape[1], hop):
            t = start / sr
            while vi <= t and vi < seconds:
                fv = frame_v.reformat(format="yuv420p")
                fv.pts = vi
                fv.time_base = Fraction(1, 1)
                for p in v.encode(fv):
                    c.mux(p)
                vi += 1
            chunk = np.ascontiguousarray(x[:, start:start + hop])
            fa = av.AudioFrame.from_ndarray(chunk, format="fltp", layout="stereo")
            fa.sample_rate = sr
            fa.pts = start
            fa.time_base = Fraction(1, sr)
            for p in a.encode(fa):
                c.mux(p)
        for p in v.encode(None):
            c.mux(p)
        for p in a.encode(None):
            c.mux(p)
    # What the Studio will decode from the MP4, against the MP3 it came from.
    back = media.decode(out)
    info = media.probe(out)
    ref = x[0].astype(np.float64)
    got = back.audio[0].astype(np.float64)
    # Cross-correlation (FFT) of 30 s at 30..60 s against the same span +-4096 samples.
    from scipy.signal import correlate

    m = 4096
    a0 = ref[sr * 30:sr * 60]
    b0 = got[sr * 30 - m:sr * 60 + m]
    cc = correlate(b0, a0, mode="valid", method="fft")  # index k = lag k - m
    best = int(np.argmax(cc)) - m
    return {"file": out.name, "from": src.name, "rate": back.sample_rate, "frames_mp3": int(x.shape[1]),
            "frames_mp4": int(back.audio.shape[1]), "lag_samples_mp4_vs_mp3": int(best),
            "has_video": info.has_video, "container": info.container, "codec": info.codec}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--eval", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    a = ap.parse_args()
    out = a.out
    truth = out / "truth"
    truth.mkdir(parents=True, exist_ok=True)
    jam = a.eval / "jamendolyrics"
    report = {
        "solo": make_solo_flac(jam / "mp3" / f"{JAMENDO_SOLO}.mp3", out / "solo-48k.flac", truth),
        "corrupt": make_corrupt(out / "corrupt.m4a"),
        "backing": make_backing_mix(a.eval / "ccmixter_voices" / "tracks" / CCMIXTER_TRACK, out / "backing-mix.wav", truth),
        "wordy": make_video(jam / "mp3" / f"{JAMENDO_WORDY}.mp3", out / "wordy.mp4", truth),
    }
    # The corrupt file must not decode with the Studio's decoder.
    try:
        media.decode(out / "corrupt.m4a")
        report["corrupt"]["studio_decode"] = "DECODED (not corrupt enough)"
    except Exception as e:  # noqa: BLE001
        report["corrupt"]["studio_decode"] = f"{type(e).__name__}: {e}"
    (out / "inputs.json").write_text(json.dumps(report, indent=1), encoding="utf-8")
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()

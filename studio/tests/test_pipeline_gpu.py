# SPDX-License-Identifier: GPL-3.0-or-later
"""The whole Studio with the real models on the GPU (``pytest -m gpu``).

Input: a 30-second excerpt of "Bad Side" by Rxbyn (JamendoLyrics, CC BY 4.0,
https://www.jamendo.com/track/... listed in JamendoLyrics.csv), cut at test time from
``$JK_BUILD/eval/jamendolyrics`` into the test's temporary folder; nothing is committed.
Skipped when the GPU, the evaluation copy or the model weights are not on this machine
(weights in ``$JK_BUILD/models-cache``, Hugging Face models in its ``hf`` folder).

The Studio runs exactly as the app starts it: ``python -I -m jukeoke_studio serve
--root <tmp> --models-dir <models>``, real probe, real models, and every song it writes
must pass the Rust reader's ``jukeoke-cli verify --strict``.
"""

from __future__ import annotations

import csv
import os
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from studio_client import StudioProc, submit  # noqa: E402
from test_server import _cli, verify_strict  # noqa: E402

from jukeoke_studio import jksong  # noqa: E402

JK = Path(os.environ.get("JK_BUILD", Path(__file__).resolve().parents[3] / "jukeoke-build"))
JAMENDO = JK / "eval" / "jamendolyrics"
MODELS = Path(os.environ.get("JUKEOKE_TEST_MODELS", JK / "models-cache"))
SONG = "Rxbyn_-_Bad_Side"          # CC BY (JamendoLyrics.csv)
T0, T1 = 6.0, 36.0                 # the excerpt, seconds

torch = pytest.importorskip("torch")
pytestmark = [
    pytest.mark.gpu,
    pytest.mark.skipif(not torch.cuda.is_available(), reason="needs a CUDA GPU"),
    pytest.mark.skipif(not (JAMENDO / "mp3" / f"{SONG}.mp3").is_file(), reason="JamendoLyrics not here"),
    pytest.mark.skipif(not (MODELS / "bs_roformer_ft1_anvuew_sdr_12.55.ckpt").is_file(),
                       reason="separation weights not downloaded"),
]


def _lines_in_window():
    rows = list(csv.DictReader(open(JAMENDO / "annotations" / "lines" / f"{SONG}.csv", encoding="utf-8")))
    words = list(csv.DictReader(open(JAMENDO / "annotations" / "words" / f"{SONG}.csv", encoding="utf-8")))
    out, onsets, k = [], [], 0
    for r in rows:
        n = len(r["lyrics_line"].split())
        s, e = float(r["start_time"]), float(r["end_time"])
        if s >= T0 + 0.5 and e <= T1 - 0.5:
            out.append((s - T0, r["lyrics_line"]))
            onsets += [float(w["word_start"]) - T0 for w in words[k:k + n]]
        k += n
    return out, onsets, k == len(words)


def _excerpt(folder: Path, rate: int = 44100) -> Path:
    """The 30 s excerpt as 24-bit FLAC (decoded MP3, cut; resampled only for the 48 kHz
    variant, which tests the Studio's 48 kHz path)."""
    import soundfile as sf

    from jukeoke_studio import media

    d = media.decode(JAMENDO / "mp3" / f"{SONG}.mp3")
    x = d.audio[:, int(T0 * d.sample_rate): int(T1 * d.sample_rate)]
    if rate != d.sample_rate:
        import soxr

        x = soxr.resample(x.T.astype(np.float64), d.sample_rate, rate, quality="VHQ").T.astype(np.float32)
    p = folder / f"Rxbyn - Bad Side (excerpt {rate}).flac"
    sf.write(str(p), x.T, rate, subtype="PCM_24")
    return p


@pytest.fixture(scope="module")
def stick(tmp_path_factory):
    root = tmp_path_factory.mktemp("gpu") / "stick"
    (root / "Library").mkdir(parents=True)
    return root


@pytest.fixture(scope="module")
def studio(stick):
    st = StudioProc(stick, stub=False, fake_hw=None, models_dir=MODELS)
    yield st
    st.close(60)
    assert not st.bad_lines, f"non-JSON on stdout: {st.bad_lines[:3]}"


def test_probe_and_models_on_this_pc(studio):
    ok = studio.hello(models_dir=str(MODELS))
    assert ok["torch"].startswith("2.")
    r = studio.request({"type": "probe", "timeout_s": 120}, timeout=180)
    assert r["ok_for_studio"] is True and r["cuda"] is True and r["checked_with"] == "torch", r
    assert r["vram_mb"] > 6000 and r["reasons"] == []
    st = studio.request({"type": "models.status", "preset": "full"})
    assert st["all_present"] is True, [m for m in st["models"] if not m["present"]]


def test_full_pipeline_lrc_excerpt(studio, stick, tmp_path):
    src = _excerpt(tmp_path)
    lines, onsets, consistent = _lines_in_window()
    lrc = "".join(f"[{int(t // 60):02d}:{t % 60:05.2f}]{text}\n" for t, text in lines)
    submit(studio, "gpu-full", src, stick / "Library", metadata={"title": "Bad Side (excerpt)", "artist": "Rxbyn"},
           lyrics={"mode": "lrc", "lrc": lrc})
    done = studio.job_end("gpu-full", 900)
    assert done["type"] == "job.done", done
    out = Path(done["output"])
    verify_strict(out)
    assert _cli() is not None, "jukeoke-cli not built: set JUKEOKE_CLI"
    rep = done["report"]
    print("report", rep)
    assert rep["lyrics_source"] == "lrc" and rep["stems_sum_max_error"] <= 0.5 / jksong.FULL_SCALE * 1.0001
    song = jksong.open_song(out)
    m = song.manifest
    assert m["stems_sum_to_source"] is True and m["preset"] == "full" and m["sample_rate"] == 44100
    names = [x["name"] for x in m["provenance"]["models"]]
    assert names == ["bs_roformer_anvuew_ft1", "bs_karaoke_anvuew", "bs_roformer_4stems_syh",
                     "facebook/wav2vec2-large-960h-lv60-self"]
    assert all(len(x["sha256"]) == 64 for x in m["provenance"]["models"])
    assert m["provenance"]["resampling"] == [] and m["provenance"]["studio"]["device"].startswith("NVIDIA")
    assert m["analysis"]["key"] is not None
    # Word onsets against the JamendoLyrics annotation (sanity check, not the benchmark).
    lyr = song.lyrics()
    got = [w["start"] for ln in lyr["lines"] for w in ln["words"]]
    if consistent and len(got) == len(onsets):
        err = np.abs(np.array(got) - np.array(onsets))
        print(f"word onsets: n={len(err)} median {np.median(err):.3f} s, within 0.3 s {np.mean(err < 0.3):.0%}")
        assert np.median(err) < 0.3
    # Realign with the real model: one good anchor and one impossible one.
    r = studio.request({"type": "realign", "song": str(out),
                        "lyrics": {"mode": "text", "text": "\n".join(t for _, t in lines)},
                        "anchors": [{"line": 1, "start": round(lines[1][0], 2)},
                                    {"line": len(lines) - 1, "start": 29.99}]}, timeout=600)
    assert r["type"] == "realign.result", r
    assert r["lyrics"]["lines"][1]["start"] == round(lines[1][0], 2)
    assert r["dropped_anchors"] == [len(lines) - 1]


def test_48k_permissive_and_transcription(studio, stick, tmp_path):
    src48 = _excerpt(tmp_path, 48000)
    submit(studio, "gpu-48k", src48, stick / "Library", metadata={"title": "Bad Side 48k", "artist": "Rxbyn"},
           lyrics={"mode": "none"})
    d = studio.job_end("gpu-48k", 900)
    assert d["type"] == "job.done", d
    m = jksong.open_song(d["output"]).manifest
    assert m["sample_rate"] == 48000 and m["lyrics"] is None
    hz = [(r["from_hz"], r["to_hz"]) for r in m["provenance"]["resampling"]]
    assert hz == [(48000, 44100), (44100, 48000), (44100, 48000), (44100, 48000)], hz
    verify_strict(Path(d["output"]))

    src = _excerpt(tmp_path)
    submit(studio, "gpu-perm", src, stick / "Library", preset="permissive",
           metadata={"title": "Bad Side permissive", "artist": "Rxbyn"}, lyrics={"mode": "auto"})
    d = studio.job_end("gpu-perm", 900)
    assert d["type"] == "job.done", d
    song = jksong.open_song(d["output"])
    m = song.manifest
    assert m["preset"] == "permissive"
    assert {x["name"] for x in m["provenance"]["models"] if x["task"] in ("vocals", "drums")} == \
        {"mel_roformer_kim", "bs_roformer_4stems_syh"}
    assert all(x["licence"] in ("MIT", "Apache-2.0") for x in m["provenance"]["models"])
    assert not song.read_stem_int("backing_vocals").any()
    lyr = song.lyrics()
    assert lyr["source"] == "transcribed" and lyr["checked"] is False and len(lyr["lines"]) >= 2
    assert any(x["task"] == "transcription" for x in m["provenance"]["models"])
    verify_strict(Path(d["output"]))

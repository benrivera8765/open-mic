# SPDX-License-Identifier: GPL-3.0-or-later
"""Alignment and transcription with the real models (GPU, slow; skipped when the model
cache or the JamendoLyrics evaluation copy is not on this machine).

Environment:
  JK_ALIGN_MODELS  model cache folder (contains hf/), default $JK_BUILD/align-venv/models
  JK_JAMENDO       JamendoLyrics checkout, default $JK_BUILD/eval/jamendolyrics
The Jamendo songs are Creative Commons; they stay outside the repository.
"""

import csv
import os
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jukeoke_studio import align as A  # noqa: E402
from jukeoke_studio import transcribe as TR  # noqa: E402
from jukeoke_studio.lyrics import from_text  # noqa: E402

JK = os.environ.get("JK_BUILD", "")
MODELS = Path(os.environ.get("JK_ALIGN_MODELS", Path(JK) / "align-venv" / "models"))
JAMENDO = Path(os.environ.get("JK_JAMENDO", Path(JK) / "eval" / "jamendolyrics"))
SONG = "Quentin_Hannappe_-_Keep_On"   # CC BY-NC-ND (JamendoLyrics.csv), used for evaluation only

torch = pytest.importorskip("torch")
needs_gpu = pytest.mark.skipif(not torch.cuda.is_available(), reason="needs a CUDA GPU")
needs_models = pytest.mark.skipif(not (MODELS / "hf").is_dir(), reason="alignment models not downloaded")
lead = JAMENDO / "_work" / "vocals" / f"{SONG}.lead.wav"
needs_song = pytest.mark.skipif(not lead.is_file(), reason="separated JamendoLyrics vocal not present")


@pytest.fixture(scope="module")
def aligner():
    return A.Aligner(A.DEFAULT_MODEL, models_dir=MODELS, device="cuda")


@needs_gpu
@needs_models
def test_log_probs_shape_and_silence(aligner):
    x = np.zeros(16000 * 70, np.float32)    # 70 s: more than two 30 s chunks
    lp = aligner.am.log_probs(x)
    assert lp.shape == (len(x) // 320, aligner.am.model.config.vocab_size)
    assert np.allclose(np.exp(lp).sum(1), 1.0, atol=1e-3)
    # Silence is blank.
    assert np.mean(np.argmax(lp, 1) == aligner.am.blank_id) > 0.99


@needs_gpu
@needs_models
@needs_song
def test_real_song_word_onsets(aligner):
    import soundfile as sf

    x, sr = sf.read(str(lead), dtype="float32", always_2d=True)
    lyr = from_text((JAMENDO / "lyrics" / f"{SONG}.txt").read_text(encoding="utf-8"))
    with open(JAMENDO / "annotations" / "words" / f"{SONG}.csv", encoding="utf-8") as f:
        gt = np.array([float(r["word_start"]) for r in csv.DictReader(f)])
    res = aligner.align(x, sr, lyr)
    on = np.array([w["start"] for ln in res.lyrics["lines"] for w in ln["words"]])
    err = np.abs(on - gt)
    assert np.median(err) < 0.1 and np.mean(err < 0.3) > 0.9, (np.median(err), np.mean(err < 0.3))
    # Realign with the user fixing line 3 at its true start: honoured exactly.
    with open(JAMENDO / "annotations" / "lines" / f"{SONG}.csv", encoding="utf-8") as f:
        l3 = float(list(csv.DictReader(f))[3]["start_time"])
    res2 = aligner.realign(x, sr, lyr.text(), anchors=[{"line": 3, "start": l3}])
    assert res2.lyrics["lines"][3]["start"] == round(l3, 3)


@needs_gpu
@needs_song
def test_whisper_transcribes_unchecked():
    if not (MODELS / "hf" / "models--mobiuslabsgmbh--faster-whisper-large-v3-turbo").is_dir():
        pytest.skip("Whisper turbo not downloaded")
    import soundfile as sf

    x, sr = sf.read(str(lead), dtype="float32", always_2d=True)
    tr = TR.Transcriber("large-v3-turbo", models_dir=MODELS)
    lyr, words = tr.transcribe_lyrics(x[: sr * 60], sr)
    assert lyr.source == "transcribed" and lyr.checked is False
    assert len(words) > 20 and all(w.start <= w.end for w in words)

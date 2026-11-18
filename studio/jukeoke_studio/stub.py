# SPDX-License-Identifier: GPL-3.0-or-later
"""Fast stand-ins for the separation, alignment and transcription models (tests only).

Selected with ``JUKEOKE_STUDIO_STUB=1`` in the Studio's environment, so the protocol
server and the whole job pipeline (decode, lyrics, align, analyse, write, cancellation,
queue) can be tested on the CPU in seconds, without model weights. Never used for a
real song: the song file's provenance names the stubs (``"name": "stub-..."``), and
``probe`` keeps reporting the real hardware unless ``JUKEOKE_STUDIO_FAKE_HW`` is set too.

* ``StubSeparator``: fixed linear stems (vocals = 0.4 x mix, lead = 0.7 x vocals, drums
  = 0.3 x band), processed in 1-second chunks with progress, cooperative cancellation
  and an optional delay per chunk (``JUKEOKE_STUDIO_STUB_DELAY`` seconds) so tests can
  cancel a job in the middle of the separate stage.
* ``stub_aligner()``: the real :class:`~jukeoke_studio.align.Aligner` (anchors, line
  windows, JSON output) on a flat log-probability matrix instead of wav2vec2.
* ``StubTranscriber``: hears "la la la" every 4 seconds.
"""

from __future__ import annotations

import os
import time
from dataclasses import dataclass, field

import numpy as np

from .errors import NO_HOOKS, Hooks

STUB_ENV = "JUKEOKE_STUDIO_STUB"
DELAY_ENV = "JUKEOKE_STUDIO_STUB_DELAY"


def enabled() -> bool:
    return os.environ.get(STUB_ENV) == "1"


def _delay() -> float:
    try:
        return float(os.environ.get(DELAY_ENV, "0") or 0)
    except ValueError:
        return 0.0


@dataclass
class StubResult:
    stems: dict[str, np.ndarray]
    sample_rate: int
    preset: str
    models: dict[str, str]
    extras: dict[str, np.ndarray] = field(default_factory=dict)
    resampling: list[dict] = field(default_factory=list)
    runs: list = field(default_factory=list)

    def provenance(self) -> dict:
        return {"preset": self.preset, "resampling": [],
                "models": [{"task": role, "name": mid, "file": None, "sha256": None,
                            "licence": "test stub (no model)", "commercial": "yes",
                            "source_url": None} for role, mid in self.models.items()]}


class StubSeparator:
    load_seconds = 0.0

    def __init__(self, *a, **k):
        pass

    def missing_models(self, preset: str = "full", models=None) -> list[str]:
        return []

    def unload(self) -> None:
        pass

    def separate(self, mix: np.ndarray, sr: int, preset: str = "full", models=None,
                 hooks: Hooks = NO_HOOKS) -> StubResult:
        from .presets import PRESETS

        if preset not in PRESETS:
            raise ValueError(f"unknown preset {preset!r}")
        mix = np.ascontiguousarray(mix, np.float32)
        n = mix.shape[1]
        delay = _delay()
        step = max(1, sr)
        for i in range(0, n, step):
            hooks.progress(i / n, "Separating (test stand-in)")
            if delay:
                time.sleep(delay)
        m64 = mix.astype(np.float64)
        vocals = (0.4 * m64).astype(np.float32)
        if "karaoke" in PRESETS[preset]:
            lead = (0.7 * vocals.astype(np.float64)).astype(np.float32)
            backing = (vocals.astype(np.float64) - lead).astype(np.float32)
        else:
            lead, backing = vocals, np.zeros_like(mix)
        band = (m64 - vocals).astype(np.float32)
        drums = (0.3 * band.astype(np.float64)).astype(np.float32)
        other = (m64 - lead - backing.astype(np.float64) - drums).astype(np.float32)
        hooks.progress(1.0)
        return StubResult({"lead_vocal": lead, "backing_vocals": backing, "drums": drums, "other": other},
                          sr, preset, {r: f"stub-{r}" for r in PRESETS[preset]},
                          extras={"vocals": vocals, "instrumental": band})


class _StubAM:
    """Looks like align.AcousticModel: letters a-z and ', blank 0, separator '|'."""

    name = "stub-aligner"

    def __init__(self):
        letters = list("abcdefghijklmnopqrstuvwxyz'")
        self.vocab = {"<pad>": 0, "|": 1, **{c.upper(): i + 2 for i, c in enumerate(letters)}}
        self.blank_id = 0
        self.sep_id = 1
        self.letter_ids = {k: v for k, v in self.vocab.items() if len(k) == 1 and k != "|"}
        self.spec = None

    def log_probs(self, x16: np.ndarray, *, norm: str = "global", hooks=None, **_) -> np.ndarray:
        from .align import HOP

        n = max(1, len(x16) // HOP)
        lp = np.full((n, len(self.vocab)), -3.0, np.float32)
        lp[:, 0] = -0.2
        if hooks is not None:
            hooks.progress(1.0)
        return lp


def stub_aligner():
    from .align import AlignConfig, Aligner

    al = Aligner.__new__(Aligner)
    al.am = _StubAM()
    al.config = AlignConfig(time_shift_s=0.0)
    return al


class StubTranscriber:
    name = "stub-transcriber"
    spec = None

    def __init__(self, *a, **k):
        pass

    def transcribe_lyrics(self, audio: np.ndarray, sr: int, language: str | None = "en", hooks=None):
        from .lyrics import SOURCE_TRANSCRIBED, Lyrics, make_line

        dur = audio.shape[-1] / sr
        lines = []
        t = 0.5
        while t + 2.0 < dur:
            ln = make_line("la la la", start=None)
            lines.append(ln)
            t += 4.0
        if hooks is not None:
            hooks.progress(1.0)
        return Lyrics(lines=lines, source=SOURCE_TRANSCRIBED, checked=False, language=language), []

# SPDX-License-Identifier: GPL-3.0-or-later
"""Transcribe the isolated lead vocal with word timestamps (Whisper via faster-whisper).

Used for two things:

* the last lyrics source (WORK_ORDER 4.1 step 3): the result is labelled
  ``source="transcribed"``, ``checked=False``, so the app shows it as unchecked;
* optionally, coarse anchors for the aligner when the lyrics carry no timestamps (the
  transcript is matched to the lyrics by sequence alignment in ``align.py``; off by
  default because it measured no better, docs/benchmarks/alignment.md).

The default model is large-v3-turbo; see the benchmark for its word error rate against
large-v3 on separated vocals.

Backend: faster-whisper on CTranslate2 (MIT). CTranslate2 4.9 runs on the RTX 5070 Ti
(sm_120) with CUDA; see docs/benchmarks/alignment.md for the measured speed and word
error rate. Whisper weights: OpenAI Whisper large-v3 / large-v3-turbo (MIT), in the
CTranslate2 conversions published by the faster-whisper maintainers (Systran, MIT) and
Mobius Labs (MIT).

Input audio is downmixed to mono and resampled to 16 kHz (soxr VHQ) for the model only.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from .align import TranscriptWord, to_model_input
from .lyrics import SOURCE_TRANSCRIBED, Line, Lyrics, Word, split_graphemes, clean_line

log = logging.getLogger(__name__)

WHISPER_MODELS = {
    "large-v3": {
        "repo": "Systran/faster-whisper-large-v3",
        "revision": "edaa852ec7e145841d8ffdb056a99866b5f0a478",
        "licence": "MIT", "commercial": "yes",
        "source_url": "https://huggingface.co/Systran/faster-whisper-large-v3",
        "size_bytes": 3_087_000_000,
    },
    "large-v3-turbo": {
        "repo": "mobiuslabsgmbh/faster-whisper-large-v3-turbo",
        "revision": "0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf",
        "licence": "MIT", "commercial": "yes",
        "source_url": "https://huggingface.co/mobiuslabsgmbh/faster-whisper-large-v3-turbo",
        "size_bytes": 1_618_000_000,
    },
}
DEFAULT_WHISPER = "large-v3-turbo"


@dataclass
class TranscribeConfig:
    beam_size: int = 5
    # Silero VAD off: on separated vocals it dropped whole sung sections (word error
    # rate 75 % -> 29-58 % on the worst songs without it, docs/benchmarks/alignment.md).
    vad_filter: bool = False
    # Silero VAD tuned for singing: a lower threshold than speech, long padding so
    # that soft onsets and held notes are not cut.
    vad_threshold: float = 0.3
    vad_min_silence_ms: int = 1000
    vad_speech_pad_ms: int = 400
    condition_on_previous_text: bool = False   # avoids repetition loops in songs
    hallucination_silence_threshold: float | None = 2.0
    max_line_words: int = 10
    min_line_words: int = 3      # below this, only a long pause ends a line
    line_gap_s: float = 1.2      # a pause this long ends a line
    long_gap_s: float = 3.0      # a pause this long always ends a line


def add_cuda12_dll_dirs() -> list[str]:
    """CTranslate2 4.x wheels are built against CUDA 12 and look for cublas64_12.dll,
    while the PyTorch cu130 wheel ships CUDA 13 libraries. The ``nvidia-cublas-cu12``
    wheel provides the CUDA 12 cuBLAS. CTranslate2 loads it by bare name with the
    default search order (``os.add_dll_directory`` is not consulted), so the DLLs are
    preloaded here; a later load by name then finds the module already in memory."""
    import ctypes
    import os
    import sys

    loaded = []
    if sys.platform != "win32":
        return loaded
    for base in sys.path:
        root = Path(base) / "nvidia" / "cublas" / "bin"
        if not root.is_dir():
            continue
        os.add_dll_directory(str(root))
        for name in ("cublasLt64_12.dll", "cublas64_12.dll"):
            p = root / name
            if p.is_file():
                ctypes.WinDLL(str(p))
                loaded.append(str(p))
        break
    return loaded


class Transcriber:
    def __init__(self, model: str = DEFAULT_WHISPER, *, models_dir: str | Path | None = None,
                 device: str | None = None, compute_type: str | None = None,
                 allow_download: bool = False, config: TranscribeConfig | None = None):
        # Import torch first: on Windows it registers its CUDA DLL folder, which
        # CTranslate2 can then use as well.
        try:
            import torch  # noqa: F401
            cuda = torch.cuda.is_available()
        except Exception:  # noqa: BLE001
            cuda = False
        add_cuda12_dll_dirs()
        from faster_whisper import WhisperModel

        from .modelzoo import hfmodels

        spec = WHISPER_MODELS.get(model)
        if spec is None:
            raise ValueError(f"unknown Whisper model {model!r}")
        # The pinned snapshot directory under <models_dir>/hf (modelzoo/hfmodels.py); a
        # missing file is ModelsMissing unless allow_download. No name resolution, no
        # network.
        self.spec = hfmodels.get(model)
        path = str(hfmodels.ensure(self.spec, models_dir, allow_download=allow_download))
        self.device = device or ("cuda" if cuda else "cpu")
        # float16 on the GPU is Whisper's own reference precision (OpenAI runs it in
        # fp16); int8 on CPU only as a slow fallback.
        self.compute_type = compute_type or ("float16" if self.device == "cuda" else "int8")
        self.name = model
        self.model = WhisperModel(path, device=self.device, compute_type=self.compute_type)
        self.config = config or TranscribeConfig()

    def transcribe_words(self, audio: np.ndarray, sr: int, language: str | None = "en", hooks=None):
        """Words with times. Returns (list[TranscriptWord], info dict). ``hooks`` gets
        progress (decoded seconds / duration) and can cancel between segments."""
        cfg = self.config
        x16 = to_model_input(audio, sr)
        t0 = time.perf_counter()
        kw = dict(language=language, beam_size=cfg.beam_size, word_timestamps=True,
                  condition_on_previous_text=cfg.condition_on_previous_text,
                  vad_filter=cfg.vad_filter)
        if cfg.vad_filter:
            kw["vad_parameters"] = {"threshold": cfg.vad_threshold,
                                    "min_silence_duration_ms": cfg.vad_min_silence_ms,
                                    "speech_pad_ms": cfg.vad_speech_pad_ms}
        if cfg.hallucination_silence_threshold is not None:
            kw["hallucination_silence_threshold"] = cfg.hallucination_silence_threshold
        segments, info = self.model.transcribe(x16, **kw)
        words: list[TranscriptWord] = []
        seg_breaks: list[int] = []
        dur = max(1e-6, len(x16) / 16000.0)
        for seg in segments:  # generator: decoding happens here
            if hooks is not None:
                hooks.progress(min(1.0, float(seg.end) / dur))
            seg_breaks.append(len(words))
            for w in seg.words or []:
                text = w.word.strip()
                if text:
                    words.append(TranscriptWord(text, float(w.start), float(w.end), float(w.probability)))
        info_d = {"language": info.language, "language_probability": float(info.language_probability),
                  "duration": float(info.duration), "seconds": time.perf_counter() - t0,
                  "model": self.name, "compute_type": self.compute_type, "segment_starts": seg_breaks}
        return words, info_d

    def transcribe_lyrics(self, audio: np.ndarray, sr: int, language: str | None = "en", hooks=None):
        """Transcribed lyrics (unchecked) plus the words, for anchoring the aligner."""
        words, info = self.transcribe_words(audio, sr, language, hooks=hooks)
        lines = words_to_lines(words, info.get("segment_starts", []), self.config)
        lyr = Lyrics(lines=lines, source=SOURCE_TRANSCRIBED, checked=False,
                     language=info.get("language") or language,
                     meta={"transcription": {k: v for k, v in info.items() if k != "segment_starts"}})
        return lyr, words


def words_to_lines(words: list[TranscriptWord], seg_starts: list[int],
                   cfg: TranscribeConfig | None = None) -> list[Line]:
    """Group transcript words into lyric lines.

    A line ends at a pause of ``long_gap_s``; once it has ``min_line_words`` words it
    also ends at a Whisper segment start, a pause of ``line_gap_s``, after a word
    ending in . ! ? or before a capitalised word (Whisper capitalises lyric lines).
    A short line is not ended by a pause after a held note (Whisper's word end times
    cut held notes short, so held notes look like pauses). Lines longer than
    ``max_line_words`` are split at their largest internal pause."""
    cfg = cfg or TranscribeConfig()
    groups: list[list[TranscriptWord]] = []
    starts = set(seg_starts)
    cur: list[TranscriptWord] = []
    for i, w in enumerate(words):
        if cur:
            gap = w.start - cur[-1].end
            long_enough = len(cur) >= cfg.min_line_words
            first = w.text.lstrip("\"'(¿¡")[:1]
            capital = first.isupper() and not w.text.startswith(("I ", "I'", "I’")) and w.text != "I"
            if gap >= cfg.long_gap_s or (long_enough and (
                    i in starts or gap >= cfg.line_gap_s or capital
                    or cur[-1].text.rstrip("\"')").endswith((".", "!", "?")))):
                groups.append(cur)
                cur = []
        cur.append(w)
    if cur:
        groups.append(cur)

    def split(g):
        if len(g) <= cfg.max_line_words:
            return [g]
        # largest pause in the middle half of the line
        lo, hi = len(g) // 4, len(g) - len(g) // 4
        k = max(range(max(1, lo), max(lo + 1, hi)), key=lambda j: g[j].start - g[j - 1].end)
        return split(g[:k]) + split(g[k:])

    lines: list[Line] = []
    for g in groups:
        for part in split(g):
            ws = []
            for w in part:
                for t in clean_line(w.text).split(" "):
                    if t:
                        ws.append(Word(t, split_graphemes(t), start=w.start, end=w.end))
            if ws:
                lines.append(Line(words=ws))
    return lines

# SPDX-License-Identifier: GPL-3.0-or-later
"""Align lyrics to the isolated lead vocal, down to the letter (Studio step 4).

Method (own implementation, no torchaudio):

1. **Acoustic model.** A character-level CTC model (default
   ``facebook/wav2vec2-large-960h-lv60-self``, 1.26 GB; ``facebook/hubert-xlarge-ls960-ft``,
   3.85 GB, measured about the same; both Apache-2.0) turns the lead-vocal stem,
   downmixed to mono and resampled to 16 kHz for the model only, into per-frame
   log-probabilities (20 ms frames). Long songs are processed in overlapping chunks.
2. **Tokens.** Every grapheme cluster of the lyrics is mapped to the model's letters
   (accents stripped, digits spelled out in English, ``&`` -> "and"). Graphemes the model
   has no letter for (punctuation, symbols, other scripts) get no token and are timed
   by interpolation inside their word afterwards. The word separator ``|`` is folded
   into the CTC blank, so word boundaries are optional and come from the letters.
3. **Constrained CTC Viterbi.** One global, monotonic CTC forced alignment over the
   whole song (trellis + Viterbi backtrack in numpy). Every token can be restricted to
   a frame window, which is how anchors work:

   * *hard anchors* (line starts fixed by the user, protocol ``realign``): the line's
     first letter starts exactly at the anchor, everything before ends before it;
   * *line windows* (LRC / online timestamps): a line must lie within its LRC start and
     the next LRC time, plus a margin. A constant offset of the whole LRC (another edit
     of the song, a longer intro) is measured first and removed, because windows in the
     wrong place are worse than none (``AlignConfig.lrc_offset_check``);
   * *transcript anchors* (optional, off by default): a coarse Whisper transcript is
     matched to the lyrics by sequence alignment (Needleman-Wunsch on words); runs of at
     least three matching words become word windows, either all of them ("windows") or
     only where an unconstrained pass disagrees by seconds ("verify"). Measured not to
     help on JamendoLyrics (docs/benchmarks/alignment.md), hence off.

   Line windows and anchors are what stop the aligner jumping to the wrong repeat of a
   chorus. Without them the alignment is still global and monotonic, so it cannot
   reorder lines, only shift them. In silent stretches of the vocal stem letters get a
   penalty, so lyrics are not pulled into long instrumental breaks.
4. **Held notes.** CTC marks a letter where it starts and then emits blanks. A letter
   is extended through the following blank frames *while the vocal stem is voiced*,
   up to the next letter, so a held note covers the hold but not the silence after it.
5. **Calibration.** A measured per-model constant (``MODEL_INFO[..]["time_shift_s"]``)
   corrects CTC's habit of firing slightly after the sung onset.

Output: the lyrics JSON of docs/SONG_FORMAT.md section 5.
"""

from __future__ import annotations

import logging
import math
import time
import unicodedata
from dataclasses import dataclass, field
from difflib import SequenceMatcher
from pathlib import Path

import numpy as np

from .lyrics import Line, Lyrics, Word, make_line, parse_plain_text

log = logging.getLogger(__name__)

MODEL_SR = 16000
HOP = 320                    # wav2vec2 conv stack stride (samples at 16 kHz) = 20 ms
FRAME_S = HOP / MODEL_SR
NEG = -1e30

DEFAULT_MODEL = "facebook/wav2vec2-large-960h-lv60-self"
# Registry information for MODELS.md / models.status (the separation stream owns the
# registry; this is what the alignment step needs). ``time_shift_s`` is the measured
# onset calibration of each model (median signed word-onset error on the JamendoLyrics
# dev songs, docs/benchmarks/alignment.md), subtracted from every time. Loading
# wav2vec2 logs "masked_spec_embed MISSING": that tensor is only used in training.
MODEL_INFO = {
    "facebook/hubert-xlarge-ls960-ft": {
        "task": "alignment",
        "title": "HuBERT X-Large fine-tuned on LibriSpeech 960 h (character CTC)",
        "licence": "Apache-2.0",
        "commercial": "yes",
        "source_url": "https://huggingface.co/facebook/hubert-xlarge-ls960-ft",
        "revision": "86a09e67e0c8d074533992379242405825516eca",
        "files": ["config.json", "preprocessor_config.json", "vocab.json", "model.safetensors",
                  "special_tokens_map.json", "tokenizer_config.json"],
        "size_bytes": 3_850_260_000,
        "time_shift_s": -0.03,
    },
    "facebook/wav2vec2-large-960h-lv60-self": {
        "task": "alignment",
        "title": "wav2vec 2.0 Large (LV-60k, self-training) fine-tuned on LibriSpeech 960 h",
        "licence": "Apache-2.0",
        "commercial": "yes",
        "source_url": "https://huggingface.co/facebook/wav2vec2-large-960h-lv60-self",
        "revision": "54074b1c16f4de6a5ad59affb4caa8f2ea03a119",
        "files": ["config.json", "preprocessor_config.json", "vocab.json", "pytorch_model.bin",
                  "special_tokens_map.json", "tokenizer_config.json", "feature_extractor_config.json"],
        "size_bytes": 1_262_060_000,
        "time_shift_s": -0.04,
    },
}


# --------------------------------------------------------------------------------------
# Text -> model tokens
# --------------------------------------------------------------------------------------

_ONES = ("zero one two three four five six seven eight nine ten eleven twelve thirteen "
         "fourteen fifteen sixteen seventeen eighteen nineteen").split()
_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()
_SPECIAL = {"ß": "ss", "æ": "ae", "œ": "oe", "ø": "o", "đ": "d", "ð": "d", "þ": "th",
            "ł": "l", "ı": "i", "&": "and", "%": "percent"}
_APOSTROPHES = set("'’‘ʼ`´′")


def number_to_words(n: int) -> str:
    """English cardinal, e.g. 342 -> 'three hundred forty two'."""
    if n < 20:
        return _ONES[n]
    if n < 100:
        return _TENS[n // 10] + ("" if n % 10 == 0 else " " + _ONES[n % 10])
    if n < 1000:
        return _ONES[n // 100] + " hundred" + ("" if n % 100 == 0 else " " + number_to_words(n % 100))
    for div, name in ((10**9, "billion"), (10**6, "million"), (1000, "thousand")):
        if n >= div:
            return (number_to_words(n // div) + " " + name
                    + ("" if n % div == 0 else " " + number_to_words(n % div)))
    raise AssertionError


def spell_digits(s: str) -> str:
    """How a run of digits is most likely sung (English)."""
    n = int(s)
    if len(s) > 1 and s[0] == "0" or len(s) > 9:
        return " ".join(_ONES[int(c)] for c in s)
    if len(s) == 4 and (1100 <= n <= 1999 or 2010 <= n <= 2099):
        hi, lo = divmod(n, 100)
        if lo == 0:
            return number_to_words(hi) + " hundred"
        return number_to_words(hi) + " " + ("oh " + _ONES[lo] if lo < 10 else number_to_words(lo))
    return number_to_words(n)


def grapheme_letters(g: str, vocab: set[str]) -> str:
    """The model letters for one grapheme ('' if none). Accents are stripped (é -> E)."""
    out = []
    for ch in unicodedata.normalize("NFKD", g.lower()):
        if unicodedata.combining(ch):
            continue
        ch = _SPECIAL.get(ch, ch)
        for c in ch.upper():
            if c in vocab:
                out.append(c)
    return "".join(out)


def grapheme_weight(g: str) -> float:
    """Share of time a grapheme gets when it has to be interpolated: letters, digits and
    marks are sung (1), punctuation, symbols and spaces are not (0, zero length)."""
    cat = unicodedata.category(g[0]) if g else "Zs"
    return 1.0 if cat[0] in "LNM" else 0.0


@dataclass
class Unit:
    """A run of graphemes inside a word that maps to a run of tokens (maybe none)."""

    g0: int
    g1: int
    tokens: list[int]
    weight: float


def word_units(graphemes: list[str], vocab_ids: dict[str, int], language: str | None) -> list[Unit]:
    """Split a word into units. Usually one grapheme per unit; a run of ASCII digits is
    one unit whose tokens spell the number (English only)."""
    vocab = set(vocab_ids)
    units: list[Unit] = []
    i, n = 0, len(graphemes)
    english = (language or "en").lower().startswith("en")
    letter_idx = [k for k, g in enumerate(graphemes) if grapheme_letters(g, vocab) and not g.isdigit()]
    while i < n:
        g = graphemes[i]
        if g.isascii() and g.isdigit():
            j = i
            while j < n and graphemes[j].isascii() and graphemes[j].isdigit():
                j += 1
            letters = ""
            if english:
                letters = "".join(c for c in spell_digits("".join(graphemes[i:j])).upper() if c in vocab)
            units.append(Unit(i, j, [vocab_ids[c] for c in letters], float(j - i)))
            i = j
            continue
        if g in _APOSTROPHES:
            # Only a word-internal apostrophe ("don't") is something the model emits.
            inner = bool(letter_idx) and letter_idx[0] < i < letter_idx[-1]
            toks = [vocab_ids["'"]] if inner and "'" in vocab_ids else []
            units.append(Unit(i, i + 1, toks, 0.0))
            i += 1
            continue
        letters = grapheme_letters(g, vocab)
        units.append(Unit(i, i + 1, [vocab_ids[c] for c in letters], grapheme_weight(g)))
        i += 1
    return units


# --------------------------------------------------------------------------------------
# Acoustic model
# --------------------------------------------------------------------------------------


def to_model_input(audio: np.ndarray, sr: int) -> np.ndarray:
    """Downmix to mono and resample to 16 kHz (soxr VHQ). This analysis copy feeds the
    aligner only; the stems are never resampled here."""
    x = np.asarray(audio, dtype=np.float32)
    if x.ndim == 2:
        # Accept (frames, channels) or (channels, frames).
        x = x.mean(axis=1) if x.shape[0] > x.shape[1] else x.mean(axis=0)
    if sr != MODEL_SR:
        import soxr

        x = soxr.resample(x, sr, MODEL_SR, quality="VHQ").astype(np.float32)
    return np.ascontiguousarray(x)


def frame_levels_db(x16: np.ndarray, n_frames: int) -> np.ndarray:
    """RMS level (dBFS) of each 20 ms model frame, over a 40 ms window centred on it."""
    win = 2 * HOP
    pad = np.pad(x16.astype(np.float64), (HOP // 2, win))
    out = np.empty(n_frames)
    # Sum of squares via cumulative sum (fast, exact enough in float64).
    cs = np.concatenate([[0.0], np.cumsum(pad * pad)])
    starts = np.arange(n_frames) * HOP
    ends = np.minimum(starts + win, len(pad))
    e = (cs[ends] - cs[starts]) / np.maximum(1, ends - starts)
    out[:] = 10.0 * np.log10(e + 1e-12)
    return out


class AcousticModel:
    """A HF CTC character model (wav2vec2 / HuBERT family) on GPU (or CPU), fp32."""

    def __init__(self, model_name: str = DEFAULT_MODEL, *, models_dir: str | Path | None = None,
                 device: str | None = None, allow_download: bool = False):
        import json

        import torch
        from huggingface_hub import hf_hub_download
        from transformers import AutoModelForCTC

        self.torch = torch
        self.name = model_name
        self.device = device or ("cuda" if torch.cuda.is_available() else "cpu")
        cache = str(Path(models_dir) / "hf") if models_dir else None
        kw = {"cache_dir": cache, "local_files_only": not allow_download}
        rev = MODEL_INFO.get(model_name, {}).get("revision")
        if rev:
            kw["revision"] = rev
        self.model = AutoModelForCTC.from_pretrained(model_name, **kw).to(self.device).eval()
        # Vocabulary: token string -> id. Blank is the pad token for HF CTC models.
        with open(hf_hub_download(model_name, "vocab.json", **kw), encoding="utf-8") as f:
            vocab = json.load(f)
        self.vocab: dict[str, int] = {k: int(v) for k, v in vocab.items()}
        self.blank_id = int(self.model.config.pad_token_id)
        self.sep_id = self.vocab.get("|")
        # Letters the aligner may use (single characters only, no special tokens).
        self.letter_ids = {k: v for k, v in self.vocab.items()
                           if len(k) == 1 and k != "|" and v != self.blank_id}
        # Whether the model was trained on zero-mean unit-variance input.
        try:
            with open(hf_hub_download(model_name, "preprocessor_config.json", **kw), encoding="utf-8") as f:
                self.normalise = bool(json.load(f).get("do_normalize", True))
        except Exception:  # noqa: BLE001
            self.normalise = True
        log.info("alignment model %s on %s (normalise=%s)", model_name, self.device, self.normalise)

    def log_probs(self, x16: np.ndarray, *, chunk_s: float = 30.0, context_s: float = 2.0,
                  norm: str = "global") -> np.ndarray:
        """Frame log-probabilities, shape (n_frames, vocab), n_frames = len(x)//320.

        ``norm='global'`` standardises with statistics of the louder half of the song,
        so silent passages stay quiet instead of being blown up to unit variance;
        ``'chunk'`` standardises each chunk on its own (what HF's processor does).
        """
        torch = self.torch
        n = len(x16)
        n_frames = max(1, n // HOP)
        x = x16.astype(np.float32)
        if not self.normalise:
            norm = "none"
        if norm == "global":
            lv = frame_levels_db(x, n_frames)
            loud = lv >= np.percentile(lv, 50)
            idx = np.repeat(loud, HOP)[: n]
            sel = x[: len(idx)][idx] if idx.any() else x
            mu, sd = float(sel.mean()), float(sel.std()) + 1e-7
            x = (x - mu) / sd
        core = int(round(chunk_s / FRAME_S))
        ctx = int(round(context_s / FRAME_S))
        out = np.empty((n_frames, self.model.config.vocab_size), np.float32)
        with torch.inference_mode():
            for f0 in range(0, n_frames, core):
                f1 = min(n_frames, f0 + core)
                a = (f0 - ctx) * HOP
                b = (f1 + ctx) * HOP + (400 - HOP)
                seg = np.zeros(b - a, np.float32)
                s0, s1 = max(0, a), min(n, b)
                seg[s0 - a: s1 - a] = x[s0:s1]
                if norm == "chunk":
                    seg = (seg - seg.mean()) / (seg.std() + 1e-7)
                t = torch.from_numpy(seg)[None].to(self.device)
                logits = self.model(t).logits[0].float()
                lp = torch.log_softmax(logits, dim=-1).cpu().numpy()
                # Output frame j of this chunk is global frame (f0 - ctx) + j.
                j0 = ctx
                take = f1 - f0
                got = lp[j0: j0 + take]
                if len(got) < take:  # numerical edge: pad with the last frame
                    got = np.concatenate([got, np.repeat(got[-1:], take - len(got), 0)])
                out[f0:f1] = got
        return out


# --------------------------------------------------------------------------------------
# Constrained CTC Viterbi (trellis + backtrack)
# --------------------------------------------------------------------------------------


def ctc_viterbi(lp: np.ndarray, tokens: np.ndarray, blank_id: int, *,
                sep_id: int | None = None, lo: np.ndarray | None = None,
                hi: np.ndarray | None = None, token_penalty: np.ndarray | None = None,
                blank_hi: dict[int, int] | None = None,
                blank_cost: np.ndarray | None = None) -> np.ndarray | None:
    """Best CTC path of ``tokens`` through the log-probs ``lp`` (T, V).

    States are the usual CTC expansion: blank, tok0, blank, tok1, ..., blank (2N+1).
    The word separator ``sep_id`` (if given) counts as blank. ``lo[n]``/``hi[n]`` restrict
    token n to frames ``lo..hi`` (inclusive). ``blank_hi[n]`` caps the blank *before*
    token n (used by hard anchors: token n then starts exactly at ``lo[n]``).
    ``blank_cost[n]`` (<= 0) is added, every frame, to the blank *before* token n (used to
    make long pauses inside a lyric line expensive).
    ``token_penalty[t]`` (<= 0) is added to every token emission at frame t. Returns the
    state index for every frame, or None when no path satisfies the constraints.
    """
    T = lp.shape[0]
    N = len(tokens)
    S = 2 * N + 1
    if N == 0:
        return np.zeros(T, dtype=np.int64)
    if T < N:
        return None
    blank = lp[:, blank_id].astype(np.float64)
    if sep_id is not None:
        blank = np.logaddexp(blank, lp[:, sep_id].astype(np.float64))
    pen = np.zeros(T) if token_penalty is None else token_penalty.astype(np.float64)

    tok_lo = np.zeros(N, np.int64) if lo is None else np.asarray(lo, np.int64).copy()
    tok_hi = np.full(N, T - 1, np.int64) if hi is None else np.asarray(hi, np.int64).copy()
    # Monotone constraints: lo non-decreasing, hi non-decreasing (from the right).
    tok_lo = np.maximum.accumulate(np.clip(tok_lo, 0, T - 1))
    tok_hi = np.minimum.accumulate(np.clip(tok_hi, 0, T - 1)[::-1])[::-1]
    if np.any(tok_lo > tok_hi):
        return None
    # State windows. Blank before token n lives between token n-1 and token n.
    s_lo = np.empty(S, np.int64)
    s_hi = np.empty(S, np.int64)
    s_lo[1::2], s_hi[1::2] = tok_lo, tok_hi
    s_lo[0], s_hi[0] = 0, tok_hi[0] - 1
    s_lo[2:-1:2], s_hi[2:-1:2] = tok_lo[:-1] + 1, tok_hi[1:] - 1
    s_lo[-1], s_hi[-1] = tok_lo[-1] + 1, T - 1
    for n, h in (blank_hi or {}).items():
        s_hi[2 * n] = min(s_hi[2 * n], h)
    s_lo = np.maximum.accumulate(s_lo)
    s_hi = np.minimum.accumulate(s_hi[::-1])[::-1]

    is_tok = np.zeros(S, bool)
    is_tok[1::2] = True
    bcost = np.zeros(S, np.float64)       # per-frame cost of each blank state
    if blank_cost is not None:
        bcost[0:-1:2] = np.asarray(blank_cost[:N], np.float64)
    state_tok = np.zeros(S, np.int64)
    state_tok[1::2] = tokens
    # Skip transition s-2 -> s allowed into a token state whose token differs from the
    # previous token (the standard CTC rule).
    can_skip = np.zeros(S, bool)
    can_skip[3::2] = tokens[1:] != tokens[:-1]

    # Active band per frame: states whose window contains t (contiguous, since both
    # window ends are monotone in s).
    a_t = np.searchsorted(s_hi, np.arange(T), side="left")
    b_t = np.searchsorted(s_lo, np.arange(T), side="right") - 1

    prev = np.full(S + 2, NEG)  # 2 guard cells at the front: index s+2
    bps: list[np.ndarray] = []
    a0, b0 = a_t[0], b_t[0]
    cur = np.full(S + 2, NEG)
    for s in (0, 1):
        if a0 <= s <= b0 and s < S:
            cur[s + 2] = blank[0] if s == 0 else lp[0, tokens[0]] + pen[0]
    bps.append(np.zeros(max(0, b0 - a0 + 1), np.int8))
    prev = cur
    for t in range(1, T):
        a, b = a_t[t], b_t[t]
        cur = np.full(S + 2, NEG)
        if b < a:
            return None
        sl = slice(a + 2, b + 3)
        stay = prev[sl]
        step = prev[a + 1: b + 2]
        skip = np.where(can_skip[a: b + 1], prev[a: b + 1], NEG)
        best = np.maximum(stay, step)
        choice = (step > stay).astype(np.int8)
        better = skip > best
        best = np.where(better, skip, best)
        choice[better] = 2
        tk = is_tok[a: b + 1]
        em = np.where(tk, lp[t, state_tok[a: b + 1]] + pen[t], blank[t] + bcost[a: b + 1])
        cur[sl] = best + em
        bps.append(choice)
        prev = cur
    # End in the last token or the final blank.
    ends = [(prev[S - 1 + 2], S - 1), (prev[S - 2 + 2], S - 2)]
    score, s = max(ends)
    if score <= NEG / 2:
        return None
    path = np.empty(T, np.int64)
    for t in range(T - 1, -1, -1):
        path[t] = s
        if t == 0:
            break
        c = bps[t][s - a_t[t]]
        s -= int(c)
    if path[0] not in (0, 1):
        return None
    return path


# --------------------------------------------------------------------------------------
# Anchors and windows
# --------------------------------------------------------------------------------------


def _norm_word(w: str) -> str:
    w = unicodedata.normalize("NFKD", w.lower())
    return "".join(c for c in w if c.isalnum() and not unicodedata.combining(c))


def match_transcript(lyric_words: list[str], tr_words: list[str]) -> list[tuple[int, int]]:
    """Global word alignment (Needleman-Wunsch) of lyrics to a transcript.

    Returns matched index pairs (lyrics i, transcript j), monotonic. Scores: identical
    normalised words +2, similar (SequenceMatcher ratio >= 0.75) +1, other -1, gap -1.
    """
    A = [_norm_word(w) for w in lyric_words]
    B = [_norm_word(w) for w in tr_words]
    n, m = len(A), len(B)
    if n == 0 or m == 0:
        return []
    # Similarity matrix (n x m); exact matches vectorised, near matches checked only
    # where cheap first-letter test passes.
    sim = np.full((n, m), -1.0)
    index: dict[str, list[int]] = {}
    for j, b in enumerate(B):
        index.setdefault(b, []).append(j)
    for i, a in enumerate(A):
        for j in index.get(a, ()):
            sim[i, j] = 2.0
    for i, a in enumerate(A):
        if not a:
            continue
        for j, b in enumerate(B):
            if sim[i, j] < 0 and b and (a[0] == b[0] or a[-1] == b[-1]) and abs(len(a) - len(b)) <= 3:
                if SequenceMatcher(None, a, b).ratio() >= 0.75:
                    sim[i, j] = 1.0
    gap = -1.0
    D = np.zeros((n + 1, m + 1))
    D[:, 0] = gap * np.arange(n + 1)
    D[0, :] = gap * np.arange(m + 1)
    P = np.zeros((n + 1, m + 1), np.int8)  # 0 diag, 1 up (skip lyric), 2 left (skip transcript)
    P[1:, 0] = 1
    P[0, 1:] = 2
    for i in range(1, n + 1):
        diag = D[i - 1, :-1] + sim[i - 1]
        up = D[i - 1, 1:] + gap
        # 'left' depends on the current row, so do a running pass.
        row = np.maximum(diag, up)
        prow = np.where(diag >= up, 0, 1).astype(np.int8)
        Di = np.empty(m + 1)
        Di[0] = D[i, 0]
        for j in range(1, m + 1):
            left = Di[j - 1] + gap
            if left > row[j - 1]:
                Di[j] = left
                P[i, j] = 2
            else:
                Di[j] = row[j - 1]
                P[i, j] = prow[j - 1]
        D[i] = Di
    pairs = []
    i, j = n, m
    while i > 0 and j > 0:
        p = P[i, j]
        if p == 0:
            if sim[i - 1, j - 1] > 0:
                pairs.append((i - 1, j - 1))
            i, j = i - 1, j - 1
        elif p == 1:
            i -= 1
        else:
            j -= 1
    pairs.reverse()
    return pairs


@dataclass
class TranscriptWord:
    text: str
    start: float
    end: float
    probability: float = 1.0


def transcript_word_windows(lyric_words: list[str], transcript: list[TranscriptWord], *,
                            min_run: int = 3, margin_s: float = 1.0,
                            min_prob: float = 0.3) -> dict[int, tuple[float, float]]:
    """Word windows from a coarse transcript: lyrics word index -> (lo, hi) seconds.

    Only words inside runs of ``min_run`` consecutive matches count (single matches of
    common words such as "the" are unreliable), and only if the transcript times in
    the run increase."""
    pairs = match_transcript(lyric_words, [w.text for w in transcript])
    out: dict[int, tuple[float, float]] = {}
    run: list[tuple[int, int]] = []

    def flush():
        if len(run) >= min_run:
            for i, j in run:
                w = transcript[j]
                if w.probability >= min_prob:
                    out[i] = (w.start - margin_s, w.end + margin_s)
    for p in pairs:
        if run and (p[0] == run[-1][0] + 1 and p[1] == run[-1][1] + 1
                    and transcript[p[1]].start >= transcript[run[-1][1]].start):
            run.append(p)
        else:
            flush()
            run = [p]
    flush()
    return out


@dataclass
class Anchor:
    """A hard constraint: line ``line`` (0-based) starts exactly at ``start`` seconds."""

    line: int
    start: float


# --------------------------------------------------------------------------------------
# Alignment
# --------------------------------------------------------------------------------------


@dataclass
class AlignConfig:
    line_margin_s: float = 1.0        # slack around LRC line windows
    word_margin_s: float = 1.0        # slack around transcript word windows
    transcript_mode: str = "verify"   # "verify": only fix gross disagreements; "windows": all
    verify_gap_s: float = 1.0         # beyond the window slack, i.e. > 2 s off in total
    verify_min_run: int = 3           # consecutive disagreeing words needed
    voiced_rel_db: float = -15.0      # voiced if within this of the robust vocal peak (held notes)
    silent_rel_db: float = -50.0      # "silent" (letters penalised) below this
    silence_penalty: float = 4.0      # nats subtracted from letters in silent frames
    hold_gap_frames: int = 3          # unvoiced frames tolerated inside a held note
    line_gap_cost: float = 0.0        # nats per frame of pause inside a lyric line
    onset_backtrack_s: float = 0.0    # move a word start back over voiced pre-onset frames
    time_shift_s: float | None = None # added to every time; None = the model's calibration
    norm: str = "global"              # acoustic model input normalisation
    use_sep_as_blank: bool = True
    # LRC / online line times that are all off by the same amount (another edit of the
    # song, a longer intro, an LRC timed for display) would put every line window in the
    # wrong place. Before the windows are used, an unconstrained pass measures the
    # median offset between where each line is sung and its LRC time; when the offset is
    # at least ``lrc_offset_min_s`` and at least ``lrc_offset_min_share`` of the lines
    # agree with it within ``lrc_offset_agree_s``, the LRC times are moved by it.
    lrc_offset_check: bool = True
    lrc_offset_min_s: float = 0.3
    lrc_offset_agree_s: float = 0.5
    lrc_offset_min_share: float = 0.5


@dataclass
class _Tok:
    line: int
    word: int
    unit: int


@dataclass
class AlignResult:
    lyrics: dict
    warnings: list[str] = field(default_factory=list)
    seconds: dict = field(default_factory=dict)
    constraint_level: str = "none"


def _robust_peak_db(levels: np.ndarray) -> float:
    return float(np.percentile(levels, 99)) if len(levels) else -120.0


class Aligner:
    """Holds the acoustic model; align many songs with one instance."""

    def __init__(self, model_name: str = DEFAULT_MODEL, *, models_dir=None, device=None,
                 allow_download: bool = False, config: AlignConfig | None = None):
        self.am = AcousticModel(model_name, models_dir=models_dir, device=device,
                                allow_download=allow_download)
        self.config = config or AlignConfig()

    # ---- public API ------------------------------------------------------------------

    def align(self, vocal: np.ndarray, sr: int, lyrics: Lyrics, *,
              anchors: list[Anchor] | None = None, use_line_times: bool = True,
              transcript: list[TranscriptWord] | None = None,
              log_probs: np.ndarray | None = None) -> AlignResult:
        """Align ``lyrics`` to the lead vocal ``vocal`` (float, any rate, mono or stereo).

        ``anchors``: hard line-start constraints. ``use_line_times``: use line times
        carried by the lyrics (LRC) as windows. ``transcript``: coarse words (e.g. from
        Whisper) used for windows when the lyrics have no times.
        """
        cfg = self.config
        t0 = time.perf_counter()
        x16 = to_model_input(vocal, sr)
        duration = len(x16) / MODEL_SR
        lp = log_probs if log_probs is not None else self.am.log_probs(x16, norm=cfg.norm)
        t1 = time.perf_counter()
        levels = frame_levels_db(x16, lp.shape[0])
        res = align_with_log_probs(lp, levels, lyrics, vocab_ids=self.am.letter_ids,
                                   blank_id=self.am.blank_id,
                                   sep_id=self.am.sep_id if cfg.use_sep_as_blank else None,
                                   duration=duration, anchors=anchors,
                                   use_line_times=use_line_times, transcript=transcript,
                                   config=cfg, model_name=self.am.name)
        res.seconds = {"emissions": t1 - t0, "viterbi": time.perf_counter() - t1}
        return res

    def realign(self, vocal: np.ndarray, sr: int, text: str, anchors: list[dict] | None = None,
                language: str = "en", source: str = "user_text") -> AlignResult:
        """Protocol ``realign``: corrected text + user anchors ([{"line", "start"}]).

        ``line`` is taken as a 0-based index into the lines of ``text`` after
        normalisation (blank lines and section headers removed): the same numbering as
        ``lines[]`` in the lyrics JSON the app holds. (STUDIO_PROTOCOL.md section 8 does
        not say which base; this is the reading used here.)
        """
        from .lyrics import from_text

        lyr = from_text(text, source=source, language=language)
        lyr.checked = True
        anc = [Anchor(int(a["line"]), float(a["start"])) for a in (anchors or [])]
        return self.align(vocal, sr, lyr, anchors=anc, use_line_times=lyr.synced)


def _build_tokens(lyrics: Lyrics, vocab_ids: dict[str, int]):
    """Flatten lyrics into tokens; returns (tokens, owners, units_per_word)."""
    tokens: list[int] = []
    owners: list[_Tok] = []
    units: list[list[list[Unit]]] = []
    unit_tok: dict[tuple[int, int, int], tuple[int, int]] = {}
    for li, line in enumerate(lyrics.lines):
        lu = []
        for wi, w in enumerate(line.words):
            us = word_units(w.graphemes, vocab_ids, lyrics.language)
            for ui, u in enumerate(us):
                if u.tokens:
                    unit_tok[(li, wi, ui)] = (len(tokens), len(tokens) + len(u.tokens))
                    for tk in u.tokens:
                        tokens.append(tk)
                        owners.append(_Tok(li, wi, ui))
            lu.append(us)
        units.append(lu)
    return np.asarray(tokens, np.int64), owners, units, unit_tok


def _first_token_of_line(owners: list[_Tok], n_lines: int) -> list[int | None]:
    first: list[int | None] = [None] * n_lines
    for k, o in enumerate(owners):
        if first[o.line] is None:
            first[o.line] = k
    return first


def align_with_log_probs(lp: np.ndarray, levels_db: np.ndarray, lyrics: Lyrics, *,
                         vocab_ids: dict[str, int], blank_id: int, sep_id: int | None,
                         duration: float, anchors: list[Anchor] | None = None,
                         use_line_times: bool = True,
                         transcript: list[TranscriptWord] | None = None,
                         config: AlignConfig | None = None,
                         model_name: str = DEFAULT_MODEL) -> AlignResult:
    """Model-independent part of the alignment (testable with synthetic log-probs)."""
    cfg = config or AlignConfig()
    warnings: list[str] = []
    T = lp.shape[0]
    tokens, owners, units, unit_tok = _build_tokens(lyrics, vocab_ids)
    n_lines = len(lyrics.lines)
    fps = 1.0 / FRAME_S

    # ---- voicing from the vocal stem ---------------------------------------------------
    peak = _robust_peak_db(levels_db)
    voiced = levels_db >= peak + cfg.voiced_rel_db
    silent = levels_db < max(peak + cfg.silent_rel_db, -90.0)
    penalty = np.where(silent, -cfg.silence_penalty, 0.0)
    # Pauses inside a line (between its letters) cost line_gap_cost per frame; pauses
    # between lines are free. Keeps a line's words together instead of letting its last
    # words drift to a later repeat (e.g. an outro the lyrics do not list).
    blank_cost = np.zeros(len(tokens))
    if cfg.line_gap_cost:
        for n in range(1, len(tokens)):
            if owners[n].line == owners[n - 1].line:
                blank_cost[n] = -cfg.line_gap_cost

    # Output time of frame f is f * FRAME_S + shift (the model's onset calibration), so a
    # time t given in output seconds (LRC line, user anchor) is frame (t - shift) * fps.
    shift = (cfg.time_shift_s if cfg.time_shift_s is not None
             else MODEL_INFO.get(model_name, {}).get("time_shift_s", 0.0))

    # ---- constraints -------------------------------------------------------------------
    first_tok = _first_token_of_line(owners, n_lines)
    tok_line = np.array([o.line for o in owners], np.int64)
    lrc_shift = 0.0   # seconds added to every LRC line time (see AlignConfig.lrc_offset_check)

    def frames(t_s: float) -> int:
        return int(math.floor((t_s - shift) * fps))

    hard: list[Anchor] = []
    for a in sorted(anchors or [], key=lambda a: a.line):
        if not (0 <= a.line < n_lines) or first_tok[a.line] is None:
            warnings.append(f"anchor for line {a.line} ignored (no such line or nothing to align)")
            continue
        if hard and a.start <= hard[-1].start:
            warnings.append(f"anchor for line {a.line} ignored (not after the previous anchor)")
            continue
        hard.append(a)

    def build(level: str):
        lo = np.zeros(len(tokens), np.int64)
        hi = np.full(len(tokens), T - 1, np.int64)
        if level in ("lines", "words"):
            if level == "lines":
                times = [(ln.start, ln.end) for ln in lyrics.lines]
                starts = [s for s, _ in times]
                for li, (s, e) in enumerate(times):
                    if s is None:
                        continue
                    if e is None:
                        nxt = [x for x in starts[li + 1:] if x is not None]
                        e = nxt[0] if nxt else None
                    sel = tok_line == li
                    lo[sel] = np.maximum(lo[sel], frames(s + lrc_shift - cfg.line_margin_s))
                    if e is not None:
                        hi[sel] = np.minimum(hi[sel], frames(e + lrc_shift + cfg.line_margin_s))
            else:
                for widx, (s, e) in word_windows.items():
                    sel = tok_word == widx
                    lo[sel] = np.maximum(lo[sel], frames(s))
                    hi[sel] = np.minimum(hi[sel], frames(e))
        blank_hi: dict[int, int] = {}
        for a in hard:
            k = first_tok[a.line]
            # Note: the anchor frame is taken without the onset calibration (``shift``).
            # Using (a.start - shift) would be the consistent time base, but measured
            # neutral for wav2vec2 and worse for HuBERT (one outro path flip) on
            # JamendoLyrics, so the measured behaviour is kept; the output line start is
            # set to the anchor exactly either way (_set_line_start).
            af = min(T - 1, max(0, int(round(a.start * fps))))
            lo[k:] = np.maximum(lo[k:], af)
            if k > 0:
                hi[:k] = np.minimum(hi[:k], af - 1)
            # The anchored letter starts exactly at the anchor frame: nothing before it
            # (not even the blank between the lines) may run past af - 1.
            hi[k] = max(hi[k], af)
            blank_hi[k] = af - 1
        return lo, hi, blank_hi

    # Word windows from a coarse transcript (only used when the lyrics carry no times).
    flat_words = [(li, wi) for li, ln in enumerate(lyrics.lines) for wi in range(len(ln.words))]
    wkey = {k: i for i, k in enumerate(flat_words)}
    tok_word = np.array([wkey[(o.line, o.word)] for o in owners], np.int64)
    word_windows: dict[int, tuple[float, float]] = {}

    levels = []
    has_line_times = use_line_times and any(ln.start is not None for ln in lyrics.lines)
    if has_line_times:
        if cfg.lrc_offset_check:
            lo0, hi0, bhi0 = build("none")
            p0 = ctc_viterbi(lp, tokens, blank_id, sep_id=sep_id, lo=lo0, hi=hi0,
                             token_penalty=penalty, blank_hi=bhi0, blank_cost=blank_cost)
            if p0 is not None:
                lrc_shift = _lrc_offset(p0, len(tokens), first_tok, lyrics, shift, cfg)
                if lrc_shift:
                    warnings.append(f"the line times were {-lrc_shift:+.2f} s off the vocal; "
                                    f"moved by {lrc_shift:+.2f} s before aligning")
        levels.append("lines")
    elif transcript:
        all_windows = transcript_word_windows(
            [lyrics.lines[li].words[wi].text for li, wi in flat_words], transcript,
            margin_s=cfg.word_margin_s)
        if cfg.transcript_mode == "windows":
            word_windows = all_windows
        else:
            # "verify": align without the transcript first, then constrain only the
            # runs of words where the transcript disagrees by seconds.
            lo0, hi0, bhi0 = build("none")
            p0 = ctc_viterbi(lp, tokens, blank_id, sep_id=sep_id, lo=lo0, hi=hi0,
                             token_penalty=penalty, blank_hi=bhi0, blank_cost=blank_cost)
            if p0 is not None:
                word_windows = _disagreeing_windows(p0, tok_word, all_windows, cfg, fps)
        if word_windows:
            levels.append("words")
    levels.append("none")
    path = None
    used = "none"
    for level in levels:
        lo, hi, bhi = build(level)
        path = ctc_viterbi(lp, tokens, blank_id, sep_id=sep_id, lo=lo, hi=hi,
                           token_penalty=penalty, blank_hi=bhi, blank_cost=blank_cost)
        if path is not None:
            used = level
            break
        warnings.append(f"alignment with '{level}' constraints was impossible; relaxing")
    if path is None and len(tokens):
        # Last resort: no constraints and no silence penalty.
        path = ctc_viterbi(lp, tokens, blank_id, sep_id=sep_id)
        used = "none"
        hard = []
        if path is None:
            raise RuntimeError("The lyrics are longer than the song can hold (too many letters).")
        warnings.append("anchors could not be honoured")

    # ---- frames of every token ------------------------------------------------------------
    N = len(tokens)
    tok_first = np.full(N, -1, np.int64)
    tok_last = np.full(N, -1, np.int64)
    if N:
        tframes = np.nonzero(path % 2 == 1)[0]
        tidx = path[tframes] // 2
        # first occurrence: iterate reversed so the earliest wins
        tok_first[tidx[::-1]] = tframes[::-1]
        tok_last[tidx] = tframes
    tok_conf = np.zeros(N)
    for n in range(N):
        tok_conf[n] = float(np.exp(lp[tok_first[n]: tok_last[n] + 1, tokens[n]].max()))

    # ---- unit onsets / letter ends --------------------------------------------------------
    # Onset frame of every aligned unit in song order, to bound held notes.
    aligned_units: list[tuple[int, int, int]] = sorted(unit_tok, key=lambda k: unit_tok[k][0])
    onset = {k: int(tok_first[unit_tok[k][0]]) for k in aligned_units}
    tok_end = {k: int(tok_last[unit_tok[k][1] - 1]) + 1 for k in aligned_units}
    next_onset: dict[tuple[int, int, int], int] = {}
    for a, b in zip(aligned_units, aligned_units[1:]):
        next_onset[a] = onset[b]
    if aligned_units:
        next_onset[aligned_units[-1]] = T

    def extend_hold(f_end: int, limit: int) -> int:
        """Extend through voiced frames (tolerating short dips) up to ``limit``."""
        f = f_end
        gap = 0
        last_voiced = f_end
        while f < limit:
            if voiced[f]:
                last_voiced = f + 1
                gap = 0
            else:
                gap += 1
                if gap > cfg.hold_gap_frames:
                    break
            f += 1
        return max(f_end, last_voiced)

    def sec(f: float) -> float:
        return min(duration, max(0.0, f * FRAME_S + shift))

    # Build per-word grapheme times.
    line_out = []
    prev_word_end_f = 0
    for li, line in enumerate(lyrics.lines):
        words_out = []
        for wi, w in enumerate(line.words):
            us = units[li][wi]
            keys = [(li, wi, ui) for ui in range(len(us)) if (li, wi, ui) in unit_tok]
            if not keys:
                words_out.append(None)  # interpolated below
                continue
            # Onset back-tracking over voiced pre-onset frames (after the previous word).
            spans: dict[int, list[float]] = {}
            for idx, k in enumerate(keys):
                s_f = onset[k]
                if idx == 0 and cfg.onset_backtrack_s > 0:
                    lim = max(prev_word_end_f, s_f - int(cfg.onset_backtrack_s * fps))
                    f = s_f
                    while f - 1 >= lim and voiced[f - 1] and path[f - 1] % 2 == 0:
                        f -= 1
                    s_f = f
                if idx + 1 < len(keys):
                    e_f = onset[keys[idx + 1]]
                else:
                    e_f = extend_hold(tok_end[k], next_onset[k])
                spans[k[2]] = [s_f, max(s_f, e_f)]
            prev_word_end_f = spans[keys[-1][2]][1]
            words_out.append(_interpolate_units(us, spans))
        line_out.append(words_out)

    # ---- words with no alignable letters: place between neighbours ----------------------
    flat = [(li, wi) for li in range(n_lines) for wi in range(len(lyrics.lines[li].words))]
    # Sung words (letters, digits, other scripts) the model has no letters for are only
    # guessed; say so (e.g. lyrics in a script the English model cannot read).
    n_guessed = sum(1 for li, wi in flat if line_out[li][wi] is None
                    and any(grapheme_weight(g) > 0 for g in lyrics.lines[li].words[wi].graphemes))
    if n_guessed:
        warnings.append(f"{n_guessed} of {len(flat)} words have no letters the alignment model "
                        f"knows; their times are interpolated, not aligned")
    for idx, (li, wi) in enumerate(flat):
        if line_out[li][wi] is not None:
            continue
        # previous end / next start in frames
        p_end = 0.0
        for j in range(idx - 1, -1, -1):
            pl, pw = flat[j]
            if line_out[pl][pw] is not None:
                p_end = line_out[pl][pw][-1][1]
                break
        n_start = None
        run_end = idx
        for j in range(idx + 1, len(flat)):
            nl, nw = flat[j]
            if line_out[nl][nw] is not None:
                n_start = line_out[nl][nw][0][0]
                break
            run_end = j
        # The LRC line time in frames (same time base as the aligned frames).
        ls = lyrics.lines[li].start
        ls_f = None if ls is None else (ls + lrc_shift - shift) * fps
        if n_start is None:
            n_start = T if ls_f is None else max(p_end, ls_f)
        # Use the LRC line time if the line has one and it fits.
        if wi == 0 and ls_f is not None and p_end <= ls_f <= n_start:
            p_end = ls_f
        run = flat[idx: run_end + 1]
        wts = [max(1.0, sum(grapheme_weight(g) for g in lyrics.lines[l].words[w].graphemes)) for l, w in run]
        # Do not stretch over a long gap: at most 0.3 s per grapheme weight.
        span = n_start - p_end
        span = min(span, sum(wts) * 0.3 * fps)
        pos = p_end
        for (l, w), wt in zip(run, wts):
            d = span * wt / sum(wts)
            us = units[l][w]
            line_out[l][w] = _interpolate_units(us, {}, whole=(pos, pos + d))
            pos += d

    # ---- assemble JSON -------------------------------------------------------------------
    out_lines = []
    for li, line in enumerate(lyrics.lines):
        words_json = []
        for wi, w in enumerate(line.words):
            g_spans = line_out[li][wi]
            chars = []
            for g, (s, e) in zip(w.graphemes, g_spans):
                chars.append([g, round(sec(s), 3), round(sec(e), 3)])
            keys = [k for k in unit_tok if k[0] == li and k[1] == wi]
            confs = [tok_conf[n] for k in keys for n in range(*unit_tok[k])]
            words_json.append({
                "start": chars[0][1], "end": chars[-1][2],
                "confidence": round(float(np.mean(confs)), 3) if confs else 0.0,
                "chars": chars,
            })
        if words_json:
            out_lines.append({"start": words_json[0]["start"], "end": words_json[-1]["end"],
                              "words": words_json})
    _enforce_order(out_lines)
    # Hard anchors are exact: line start = anchor time.
    for a in hard:
        if a.line < len(out_lines):
            _set_line_start(out_lines, a.line, a.start)

    doc = {
        "lyrics_version": 1,
        "language": lyrics.language,
        "source": lyrics.source,
        "checked": bool(lyrics.checked),
        "alignment": {"method": "ctc-forced", "model": model_name,
                      "anchored_lines": bool(hard) or used in ("lines",),
                      "constraints": used},
        "lines": out_lines,
    }
    return AlignResult(lyrics=doc, warnings=warnings, constraint_level=used)


def _disagreeing_windows(path: np.ndarray, tok_word: np.ndarray,
                         windows: dict[int, tuple[float, float]], cfg: AlignConfig,
                         fps: float) -> dict[int, tuple[float, float]]:
    """Transcript windows for the words where an unconstrained alignment and the
    transcript disagree badly: runs of at least ``verify_min_run`` consecutive matched
    words whose aligned start lies outside the transcript window (which already has
    ``word_margin_s`` of slack) by more than ``verify_gap_s``, all in the same direction."""
    tframes = np.nonzero(path % 2 == 1)[0]
    first_tok = {}
    for f in tframes[::-1]:
        first_tok[int(path[f] // 2)] = int(f)
    word_start: dict[int, float] = {}
    for n in sorted(first_tok):
        w = int(tok_word[n])
        if w not in word_start:
            word_start[w] = first_tok[n] / fps
    off = {}
    for w, (lo, hi) in windows.items():
        if w not in word_start:
            continue
        s = word_start[w]
        if s < lo - cfg.verify_gap_s:
            off[w] = -1
        elif s > hi + cfg.verify_gap_s:
            off[w] = 1
    out: dict[int, tuple[float, float]] = {}
    run: list[int] = []
    for w in sorted(off) + [None]:
        if run and (w is None or w != run[-1] + 1 or off[w] != off[run[-1]]):
            if len(run) >= cfg.verify_min_run:
                for x in run:
                    out[x] = windows[x]
            run = []
        if w is not None:
            run.append(w)
    return out


def _lrc_offset(path: np.ndarray, n_tokens: int, first_tok: list[int | None], lyrics: Lyrics,
                shift: float, cfg: AlignConfig) -> float:
    """Constant offset (seconds) to add to the LRC line times, or 0.0.

    ``path`` is an alignment without line windows. For every line with an LRC time, the
    difference between its aligned first letter and its LRC time is taken; the median
    is used only when it is large enough to matter and most lines agree with it, so a
    correct LRC (or one with independent per-line errors) is left alone.
    """
    tf = np.full(n_tokens, -1, np.int64)
    tframes = np.nonzero(path % 2 == 1)[0]
    tidx = path[tframes] // 2
    tf[tidx[::-1]] = tframes[::-1]          # first frame of every token
    d = []
    for li, ln in enumerate(lyrics.lines):
        k = first_tok[li]
        if ln.start is None or k is None or tf[k] < 0:
            continue
        d.append(tf[k] * FRAME_S + shift - ln.start)
    if len(d) < 3:
        return 0.0
    d = np.asarray(d)
    med = float(np.median(d))
    share = float(np.mean(np.abs(d - med) <= cfg.lrc_offset_agree_s))
    if abs(med) < cfg.lrc_offset_min_s or share < cfg.lrc_offset_min_share:
        return 0.0
    return round(med, 3)


def _interpolate_units(us: list[Unit], spans: dict[int, list[float]], whole=None) -> list[tuple[float, float]]:
    """Per-grapheme (start, end) in frames for one word.

    ``spans``: unit index -> [start, end] for aligned units. Unaligned units are timed
    by interpolation: zero-weight units (punctuation) get zero length at their place;
    weighted units (digits without spelling, other scripts) take a share of the
    neighbouring aligned unit's span. ``whole=(s, e)`` times a word with no aligned
    units by splitting (s, e) by weight.
    """
    n = len(us)
    st = [None] * n
    en = [None] * n
    if whole is not None:
        s, e = whole
        wts = [u.weight for u in us]
        tot = sum(wts)
        pos = s
        for i, u in enumerate(us):
            d = (e - s) * (wts[i] / tot) if tot > 0 else 0.0
            st[i], en[i] = pos, pos + d
            pos += d
        if tot == 0:
            st = [s] * n
            en = [s] * n
    else:
        for i, sp in spans.items():
            st[i], en[i] = sp
        # Runs of unaligned units: leading, between, trailing.
        i = 0
        while i < n:
            if st[i] is not None:
                i += 1
                continue
            j = i
            while j < n and st[j] is None:
                j += 1
            run = list(range(i, j))
            w_run = sum(us[k].weight for k in run)
            left = i - 1 if i > 0 else None       # aligned unit before the run
            right = j if j < n else None          # aligned unit after the run
            if w_run == 0:
                at = en[left] if left is not None else st[right]
                for k in run:
                    st[k] = en[k] = at
            else:
                # Share the neighbouring aligned unit's span (prefer the left one).
                donor = left if left is not None else right
                ds, de = st[donor], en[donor]
                wd = max(us[donor].weight, 1.0)
                frac = wd / (wd + w_run)
                if donor == left:
                    cut = ds + (de - ds) * frac
                    en[donor] = cut
                    pos = cut
                    for k in run:
                        d = (de - cut) * us[k].weight / w_run
                        st[k], en[k] = pos, pos + d
                        pos += d
                else:
                    cut = ds + (de - ds) * (1 - frac)
                    pos = ds
                    for k in run:
                        d = (cut - ds) * us[k].weight / w_run
                        st[k], en[k] = pos, pos + d
                        pos += d
                    st[donor] = cut
            i = j
    out: list[tuple[float, float]] = []
    for u, s, e in zip(us, st, en):
        g_n = u.g1 - u.g0
        if g_n == 1:
            out.append((s, e))
        else:  # a digit run: split evenly over its graphemes
            d = (e - s) / g_n
            out += [(s + d * q, s + d * (q + 1)) for q in range(g_n)]
    return out


def _enforce_order(lines: list[dict]) -> None:
    """Make every time non-decreasing and non-overlapping after rounding."""
    t = 0.0
    for ln in lines:
        for w in ln["words"]:
            for c in w["chars"]:
                c[1] = max(c[1], t)
                c[2] = max(c[2], c[1])
                t = c[2]
            w["start"], w["end"] = w["chars"][0][1], w["chars"][-1][2]
        ln["start"], ln["end"] = ln["words"][0]["start"], ln["words"][-1]["end"]


def _set_line_start(lines: list[dict], li: int, start: float) -> None:
    """Put line ``li``'s first letter exactly at ``start`` (a user anchor)."""
    start = round(start, 3)
    w = lines[li]["words"][0]
    c = w["chars"][0]
    c[1] = start
    c[2] = max(c[2], start)
    if li > 0:
        prev = lines[li - 1]
        pw = prev["words"][-1]
        for ch in pw["chars"]:
            ch[1] = min(ch[1], start)
            ch[2] = min(ch[2], start)
    _enforce_order(lines)
    # _enforce_order may have pushed the start later only if earlier times exceed it;
    # those were clamped above, so the anchor holds.


def align_stage(aligner: Aligner, vocal: np.ndarray, sr: int, lyrics: Lyrics, *,
                transcriber=None, transcript: list[TranscriptWord] | None = None,
                anchors: list[Anchor] | None = None,
                coarse_anchors: bool = False) -> AlignResult:
    """Pipeline step 4 as the job runner calls it.

    * Lyrics with line times (LRC, SYLT, online) are aligned within their line windows.
    * Everything else (plain text, transcribed lyrics) is aligned globally over the
      whole song: on JamendoLyrics this measured better than windows taken from
      Whisper's word timestamps, whose occasional errors of several seconds become
      hard constraints (docs/benchmarks/alignment.md). ``coarse_anchors=True`` (with a
      ``transcriber`` or a ``transcript``) turns the Whisper windows on anyway.
    * ``anchors`` (line starts fixed by the user) are always hard constraints.
    """
    if coarse_anchors and transcript is None and not lyrics.synced and transcriber is not None:
        transcript, _ = transcriber.transcribe_words(vocal, sr, lyrics.language)
    return aligner.align(vocal, sr, lyrics, anchors=anchors,
                         transcript=transcript if coarse_anchors else None)


def lyrics_from_plain(text: str, language: str = "en", source: str = "user_text") -> Lyrics:
    return Lyrics(parse_plain_text(text), source=source, checked=True, language=language)


__all__ = [
    "Aligner", "AlignConfig", "AlignResult", "Anchor", "TranscriptWord", "AcousticModel",
    "ctc_viterbi", "align_with_log_probs", "match_transcript", "transcript_word_windows",
    "word_units", "grapheme_letters", "spell_digits", "number_to_words", "to_model_input",
    "frame_levels_db", "DEFAULT_MODEL", "MODEL_INFO", "Line", "Word", "make_line",
]

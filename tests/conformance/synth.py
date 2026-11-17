# SPDX-License-Identifier: GPL-3.0-or-later
"""Deterministic synthetic "song" for format tests: four stems, a source that they sum
to, and letter-timed lyrics that match the generated melody. numpy only.

Everything here is generated from formulas and a fixed seed: no recordings, no
copyrighted material. Used by tests/conformance/make_song.py and by
tests/fixtures/generated/make_tiny.py.
"""

from __future__ import annotations

import struct
import zlib

import numpy as np

# Lyrics: (line, [(word graphemes, [note per grapheme as semitones above A3])]).
# "café" uses a precomposed é (U+00E9); "naïve" uses i + U+0308 COMBINING DIAERESIS,
# which is ONE grapheme cluster of two code points.
LINES = [
    [(["H", "e", "l", "l", "o"], [0, 2, 4, 4, 7]), (["c", "a", "f", "é"], [9, 7, 4, 4])],
    [(["n", "a", "ï", "v", "e"], [2, 4, 5, 4, 2]), (["w", "o", "r", "l", "d", ","], [0, 4, 7, 9, 7, 7])],
]
VOWELS = set("aeiouAEIOUé") | {"ï"}
A3 = 220.0


def lyric_timing(line_starts=(0.30, 2.10), grapheme=0.08, vowel=0.16, gap=0.12, hold=0.45):
    """Times for every grapheme: consonants short, vowels longer, the last grapheme of a
    line held (a held note), a gap between words. Returns (lyrics dict, notes) where
    notes = [(start, end, freq)]."""
    lines = []
    notes = []
    for li, words in enumerate(LINES):
        t = line_starts[li]
        line_start = t
        wj = []
        for wi, (gs, semis) in enumerate(words):
            w_start = t
            chars = []
            for gi, (g, s) in enumerate(zip(gs, semis)):
                last = wi == len(words) - 1 and gi == len(gs) - 1
                d = hold if last else (vowel if g in VOWELS else grapheme)
                a, b = round(t, 3), round(t + d, 3)
                chars.append([g, a, b])
                notes.append((a, b, A3 * 2 ** (s / 12)))
                t = b
            wj.append({"start": round(w_start, 3), "end": round(t, 3), "confidence": 1.0, "chars": chars})
            t = round(t + gap, 3)
        lines.append({"start": round(line_start, 3), "end": wj[-1]["end"], "words": wj})
    lyrics = {
        "lyrics_version": 1, "language": "en", "source": "user_text", "checked": True,
        "alignment": {"method": "synthetic", "model": None, "anchored_lines": False},
        "lines": lines,
    }
    return lyrics, notes


def _env(n, sr, fade=0.005):
    e = np.ones(n)
    k = min(int(fade * sr), n // 2)
    if k:
        ramp = np.linspace(0.0, 1.0, k, endpoint=False)
        e[:k] = ramp
        e[n - k:] = ramp[::-1]
    return e


def synth(sr=44100, seconds=4.0, seed=7, other_peak=1.08):
    """Returns (stems {role: float32 [2, frames]}, source float32 [2, frames], lyrics).

    ``other`` is computed like the Studio's "full" preset, as source minus the other
    three stems in float32, so the stems sum back to the source; it is scaled to peak
    at ``other_peak`` (> 1 exercises the gain/headroom rule)."""
    n = int(round(seconds * sr))
    t = np.arange(n) / sr
    rng = np.random.default_rng(seed)
    lyrics, notes = lyric_timing()

    # Lead vocal: a sine with 5 Hz vibrato (+-20 cents) and a little 2nd harmonic per
    # grapheme; centred (identical channels).
    lead = np.zeros(n)
    for a, b, f in notes:
        i0, i1 = int(a * sr), min(int(b * sr), n)
        tt = t[i0:i1] - a
        inst = f * 2 ** (20 / 1200 * np.sin(2 * np.pi * 5 * tt))
        ph = 2 * np.pi * np.cumsum(inst) / sr
        lead[i0:i1] = (0.30 * np.sin(ph) + 0.06 * np.sin(2 * ph)) * _env(i1 - i0, sr)
    lead2 = np.stack([lead, lead])

    # Backing vocals: a sustained third above the melody's tonic during each line,
    # slightly wider on the left.
    back = np.zeros(n)
    for line in lyrics["lines"]:
        i0, i1 = int(line["start"] * sr), min(int(line["end"] * sr), n)
        back[i0:i1] = 0.12 * np.sin(2 * np.pi * A3 * 2 ** (4 / 12) * t[i0:i1]) * _env(i1 - i0, sr, 0.05)
    back2 = np.stack([back, 0.85 * back])

    # Drums at 120 BPM: kick on every beat (pitch sweep), hi-hat (short noise burst) on
    # the off-beats, slightly to the right.
    drums = np.zeros((2, n))
    beat = 0.5
    for k in range(int(seconds / beat) + 1):
        i0 = int(k * beat * sr)
        m = min(int(0.25 * sr), n - i0)
        if m <= 0:
            continue
        tt = np.arange(m) / sr
        kick = 0.7 * np.sin(2 * np.pi * (50 * tt + 70 / 30 * (1 - np.exp(-30 * tt)))) * np.exp(-12 * tt)
        drums[:, i0:i0 + m] += kick
        j0 = int((k + 0.5) * beat * sr)
        h = min(int(0.03 * sr), n - j0)
        if h > 0:
            hat = 0.05 * rng.standard_normal(h) * np.exp(-np.arange(h) / (0.008 * sr))
            drums[0, j0:j0 + h] += 0.8 * hat
            drums[1, j0:j0 + h] += hat

    # Accompaniment: bass + a three-note pad (A major, then D major after 2 s).
    roots = np.where(t < 2.0, 110.0, 110.0 * 2 ** (5 / 12))
    bass = 0.45 * np.sin(2 * np.pi * np.cumsum(roots) / sr)
    pad = sum(0.18 * np.sin(2 * np.pi * np.cumsum(roots * 2 * 2 ** (s / 12)) / sr + s) for s in (0, 4, 7))
    acc = np.stack([bass + pad, bass + 0.9 * pad])
    acc *= other_peak / np.max(np.abs(acc))

    source = (lead2 + back2 + drums + acc).astype(np.float32)
    stems = {
        "lead_vocal": lead2.astype(np.float32),
        "backing_vocals": back2.astype(np.float32),
        "drums": drums.astype(np.float32),
    }
    # The "full" preset's residual stem, in float32 like the Studio computes it.
    stems["other"] = source - stems["lead_vocal"] - stems["backing_vocals"] - stems["drums"]
    return stems, source, lyrics


def png_gradient(w=32, h=32) -> bytes:
    """A small generated RGB PNG (pink-to-mint gradient) for a cover chunk."""
    y, x = np.mgrid[0:h, 0:w]
    r = (255 - 80 * x / (w - 1)).astype(np.uint8)
    g = (90 + 150 * y / (h - 1)).astype(np.uint8)
    b = (170 + 40 * x / (w - 1)).astype(np.uint8)
    rows = np.concatenate([np.zeros((h, 1), np.uint8), np.stack([r, g, b], -1).reshape(h, -1)], axis=1)

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows.tobytes(), 9)) + chunk(b"IEND", b""))

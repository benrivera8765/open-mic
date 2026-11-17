# SPDX-License-Identifier: GPL-3.0-or-later
"""Musical key and tempo of a song, from its instrumental stems (Studio step 5).

Key: a constant-Q chromagram (librosa, ISC licence) of the harmonic stems (``other``
plus ``backing_vocals``; drums are left out because they smear the chroma) is summed
over the song and correlated with the 24 rotations of a major and a minor key profile
(template matching, Krumhansl & Schmuckler). The profile set is chosen by the
benchmark in docs/benchmarks/analysis.md. ``confidence`` is the softmax weight of the
best key among all 24 (0..1).

Tempo: librosa's onset-strength autocorrelation tempo estimate on the drum stem (or on
the whole instrumental when the drum stem is silent).

All analysis works on a mono 22.05 kHz copy (soxr VHQ); the stems are not touched.
Nothing here is AGPL or non-commercial: librosa (ISC), numpy, scipy (BSD), soxr (LGPL).

:func:`describe_shift` names a shifted key for the key control, e.g.
``describe_shift({"tonic": "A", "mode": "major"}, 2) == "A major -> B major (+2)"``
(with a real arrow).
"""

from __future__ import annotations

import numpy as np

PITCHES = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B")
ANALYSIS_SR = 22050

# Key profiles, index 0 = tonic. Published values:
#   KK: Krumhansl & Kessler (1982) probe-tone ratings.
#   TEMPERLEY: Temperley (2005), Kostka-Payne corpus.
#   AS: Albrecht & Shanahan (2013), corpus of Western art music.
PROFILES = {
    "kk": (
        [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88],
        [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17],
    ),
    "temperley": (
        [0.748, 0.060, 0.488, 0.082, 0.670, 0.460, 0.096, 0.715, 0.104, 0.366, 0.057, 0.400],
        [0.712, 0.084, 0.474, 0.618, 0.049, 0.460, 0.105, 0.747, 0.404, 0.067, 0.133, 0.330],
    ),
    "as": (
        [0.238, 0.006, 0.111, 0.006, 0.137, 0.094, 0.016, 0.214, 0.009, 0.080, 0.008, 0.081],
        [0.220, 0.006, 0.104, 0.123, 0.019, 0.103, 0.012, 0.214, 0.062, 0.022, 0.061, 0.052],
    ),
}
DEFAULT_PROFILE = "as"
SOFTMAX_BETA = 30.0  # sharpness of the confidence over correlation values

# Conventional spelling (fewest accidentals in the key signature).
_FLAT = {"C#": "Db", "D#": "Eb", "F#": "Gb", "G#": "Ab", "A#": "Bb"}
_MAJOR_FLATS = {"C#", "D#", "G#", "A#"}         # Db, Eb, Ab, Bb major; F# major stays
_MINOR_FLATS = {"D#", "A#"}                     # Eb, Bb minor; C#, F#, G# minor stay


# ------------------------------------------------------------------------------- names

def spell(tonic: str, mode: str, spelling: str = "conventional", unicode: bool = False) -> str:
    """Tonic as displayed. ``spelling``: 'sharps', 'flats' or 'conventional'."""
    t = tonic
    if spelling == "flats" or (spelling == "conventional" and
                               tonic in (_MAJOR_FLATS if mode == "major" else _MINOR_FLATS)):
        t = _FLAT.get(tonic, tonic)
    if unicode:
        t = t.replace("#", "♯").replace("b", "♭") if len(t) > 1 else t
    return t


def key_name(tonic: str, mode: str, spelling: str = "conventional", unicode: bool = False) -> str:
    return f"{spell(tonic, mode, spelling, unicode)} {mode}"


def shift_key(tonic: str, mode: str, semitones: int) -> tuple[str, str]:
    """Key after transposing by ``semitones`` (mode unchanged)."""
    return PITCHES[(PITCHES.index(tonic) + int(semitones)) % 12], mode


def describe_shift(key, semitones: int, *, spelling: str = "conventional",
                   arrow: str = "→", unicode: bool = False) -> str:
    """Name a key change for the key control: "A major → B major (+2)".

    ``key`` is the manifest's ``analysis.key`` dict (or a (tonic, mode) tuple, or None
    when the key is unknown, which gives "+2 semitones").
    """
    s = int(semitones)
    sign = f"{s:+d}" if s else "0"
    if key is None:
        return "original key" if s == 0 else f"{sign} semitone{'s' if abs(s) != 1 else ''}"
    tonic, mode = (key["tonic"], key["mode"]) if isinstance(key, dict) else key
    before = key_name(tonic, mode, spelling, unicode)
    if s == 0:
        return f"{before} (original key)"
    after = key_name(*shift_key(tonic, mode, s), spelling, unicode)
    return f"{before} {arrow} {after} ({sign})"


# ------------------------------------------------------------------------- key finding

def _mono22(audio: np.ndarray, sr: int) -> np.ndarray:
    x = np.asarray(audio, dtype=np.float32)
    if x.ndim == 2:
        x = x.mean(axis=0) if x.shape[0] <= 8 and x.shape[0] < x.shape[1] else x.mean(axis=1)
    if sr != ANALYSIS_SR:
        import soxr

        x = soxr.resample(x, sr, ANALYSIS_SR, quality="VHQ").astype(np.float32)
    return x


def song_chroma(x22: np.ndarray, *, hop: int = 2048, compress: str = "log10",
                skip_octaves: int = 1) -> np.ndarray:
    """Song-level 12-bin chroma.

    Constant-Q magnitude from C1 over 7 octaves at 36 bins per octave, tuned to the
    recording (librosa ``estimate_tuning``), compressed per bin with ``log(1 + 10|X|)``
    (``compress='log10'``; ``'lin'`` keeps magnitudes), the lowest ``skip_octaves``
    octaves (below C2 = 65 Hz by default) left out, folded to 12 pitch classes and
    summed over the song. The log compression keeps the thirds, which decide major
    against minor, from being drowned by roots and fifths (bass and the low
    harmonics); docs/benchmarks/analysis.md has the comparison that chose it.
    """
    import librosa

    if len(x22) < hop * 4:
        return np.zeros(12)
    bpo = 36
    tuning = float(librosa.estimate_tuning(y=x22, sr=ANALYSIS_SR, bins_per_octave=bpo))
    fmin = librosa.note_to_hz("C1")
    C = np.abs(librosa.cqt(x22, sr=ANALYSIS_SR, hop_length=hop, fmin=fmin, n_bins=7 * bpo,
                           bins_per_octave=bpo, tuning=tuning))
    if compress == "log10":
        C = np.log1p(10.0 * C)
    elif compress == "log100":
        C = np.log1p(100.0 * C)
    elif compress != "lin":
        raise ValueError(f"unknown compression {compress!r}")
    C[: skip_octaves * bpo] = 0.0
    chroma = librosa.feature.chroma_cqt(C=C, sr=ANALYSIS_SR, hop_length=hop, fmin=fmin,
                                        bins_per_octave=bpo, n_octaves=7, norm=None)
    return chroma.sum(axis=1)


def key_scores(chroma12: np.ndarray, profile: str = DEFAULT_PROFILE) -> np.ndarray:
    """Pearson correlation of the chroma with each key: shape (2, 12) = [major, minor] x tonic."""
    out = np.zeros((2, 12))
    c = np.asarray(chroma12, float)
    if not np.any(c > 0) or np.std(c) == 0:
        return out
    profs = PROFILES[profile] if isinstance(profile, str) else profile
    for m, prof in enumerate(profs):
        p = np.asarray(prof, float)
        for t in range(12):
            out[m, t] = np.corrcoef(c, np.roll(p, t))[0, 1]
    return out


def estimate_key(audio: np.ndarray, sr: int, *, profile: str = DEFAULT_PROFILE,
                 compress: str = "log10") -> dict | None:
    """Key of ``audio`` (float, mono or [channels, frames]); None if it is silent."""
    x = _mono22(audio, sr)
    if not np.any(np.abs(x) > 1e-5):
        return None
    sc = key_scores(song_chroma(x, compress=compress), profile)
    if not np.any(sc):
        return None
    flat = sc.ravel()
    w = np.exp(SOFTMAX_BETA * (flat - flat.max()))
    w /= w.sum()
    best = int(np.argmax(flat))
    mode = "major" if best < 12 else "minor"
    tonic = PITCHES[best % 12]
    return {"tonic": tonic, "mode": mode, "name": key_name(tonic, mode),
            "confidence": round(float(w[best]), 3)}


# ------------------------------------------------------------------------------- tempo

def estimate_tempo(audio: np.ndarray, sr: int, *, min_bpm: float = 60.0,
                   max_bpm: float = 200.0) -> float | None:
    """Tempo in BPM (librosa onset autocorrelation with its log-normal prior around
    120 BPM), folded into [min_bpm, max_bpm). None for silence."""
    import librosa

    x = _mono22(audio, sr)
    if len(x) < ANALYSIS_SR * 4 or not np.any(np.abs(x) > 1e-5):
        return None
    # Mean over frequency bands (not median): measured better on both test sets
    # (docs/benchmarks/analysis.md).
    oenv = librosa.onset.onset_strength(y=x, sr=ANALYSIS_SR, hop_length=512, aggregate=np.mean)
    if not np.any(oenv > 0):
        return None
    bpm = float(np.atleast_1d(librosa.feature.tempo(onset_envelope=oenv, sr=ANALYSIS_SR,
                                                    hop_length=512))[0])
    if bpm <= 0:
        return None
    while bpm < min_bpm:
        bpm *= 2
    while bpm >= max_bpm:
        bpm /= 2
    return round(bpm, 1)


# --------------------------------------------------------------------------- the stage

def _rms(x: np.ndarray) -> float:
    return float(np.sqrt(np.mean(np.square(x, dtype=np.float64)))) if x.size else 0.0


def analyse_stems(stems: dict[str, np.ndarray], sr: int) -> dict:
    """The manifest's ``analysis`` object from the separated stems.

    ``stems``: role -> float32 [channels, frames] (as returned by the separator).
    Key from other + backing vocals; tempo from drums (or the whole instrumental if the
    drums are silent).
    """
    def mix(roles):
        arrs = [stems[r] for r in roles if r in stems]
        if not arrs:
            return None
        return np.sum([np.asarray(a, np.float32) for a in arrs], axis=0)

    harm = mix(["other", "backing_vocals"])
    if harm is None:
        harm = mix([r for r in stems if r not in ("drums", "lead_vocal")])
    key = estimate_key(harm, sr) if harm is not None else None
    drums = stems.get("drums")
    if drums is not None and _rms(np.asarray(drums)) > 1e-4:
        tempo = estimate_tempo(drums, sr)
    else:
        inst = mix([r for r in stems if r != "lead_vocal"])
        tempo = estimate_tempo(inst, sr) if inst is not None else None
    return {"key": key, "tempo_bpm": tempo}

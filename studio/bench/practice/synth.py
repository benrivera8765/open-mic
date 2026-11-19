# SPDX-License-Identifier: GPL-3.0-or-later
"""A deterministic synthetic singer with known notes (WO2 section 9, "Guide accuracy").

The voice is a source-filter model, not samples of anyone:

- source: a Rosenberg glottal-flow pulse train (differentiated, i.e. with lip radiation)
  driven by a phase accumulator, with per-cycle jitter (frequency) and shimmer
  (amplitude), plus glottal-modulated aspiration noise;
- filter: four time-varying formant resonators per vowel (a e i o u), scaled per voice
  type, with F1 raised above the fundamental on high notes (as sopranos do);
- unvoiced consonants (s, sh, f, t, k) as filtered noise before the vowel, voiced
  consonant boundaries as level dips, and breaths between phrases.

The melody has everything WO2 asks the guide to cope with: vibrato (with delayed onset),
portamento slides, scoops, held notes, fast runs, several notes on one syllable, leaps,
spoken/rapped stretches (voiced, speech-like contours: ground truth "unpitched"), and a
tuning offset (A = 436 Hz), across E2..C6 in four voice types.

Everything is seeded: the same call gives the same samples and the same ground truth.
No lyrics exist anywhere in this module; syllables are vowels only.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import numpy as np
from numba import njit

SR = 48_000
CR = 1_000  # control rate (Hz) of the pitch / level / formant tracks
A4_HZ = 436.0
TUNING_SEMITONES = 12.0 * math.log2(A4_HZ / 440.0)  # -0.1586

# Formants (Hz) and bandwidths for an adult male voice (Peterson & Barney style averages).
VOWELS = {
    "a": ((730, 1090, 2440, 3400), (90, 110, 170, 250)),
    "e": ((530, 1840, 2480, 3500), (70, 100, 160, 250)),
    "i": ((300, 2250, 3000, 3700), (60, 100, 170, 250)),
    "o": ((570, 840, 2410, 3300), (80, 90, 160, 250)),
    "u": ((320, 870, 2240, 3300), (60, 90, 150, 250)),
}


@dataclass
class Voice:
    name: str
    low: int        # MIDI range of the melody
    high: int
    formant_scale: float
    speech_base: float  # MIDI of the speaking voice
    seed: int


VOICES = [
    Voice("bass", 40, 60, 0.95, 45.0, 11),      # E2..C4
    Voice("tenor", 48, 69, 1.0, 50.0, 12),      # C3..A4
    Voice("alto", 55, 76, 1.12, 57.0, 13),      # G3..E5
    Voice("soprano", 62, 84, 1.2, 62.0, 14),    # D4..C6
]


@dataclass
class SNote:
    on: float
    off: float
    midi: float           # intended pitch (A=440 grid; the singer is tuned to A4_HZ)
    glide_s: float = 0.0  # portamento from the previous note over the first glide_s
    glide_from: float | None = None
    scoop_cents: float = 0.0
    scoop_s: float = 0.0
    vib_rate: float = 0.0
    vib_cents: float = 0.0
    vib_delay: float = 0.0


@dataclass
class Syllable:
    on: float
    off: float
    vowel: str
    notes: list[SNote] = field(default_factory=list)
    spoken: bool = False
    contour: tuple[float, float] = (0.0, 0.0)  # spoken: start/end offsets (semitones)
    consonant: str | None = None   # unvoiced consonant before `on`
    cons_s: float = 0.0
    voiced_join: bool = False      # joined to the previous syllable through a level dip
    gain_db: float = 0.0


# ------------------------------------------------------------------------- the score

class Composer:
    """Writes phrases for one voice. All randomness comes from one seeded generator."""

    def __init__(self, voice: Voice):
        self.v = voice
        self.rng = np.random.default_rng(voice.seed)
        root = voice.low + int(self.rng.integers(0, 5))
        steps = [0, 2, 4, 5, 7, 9, 11]
        self.scale = [root + 12 * o + s for o in range(-1, 4) for s in steps
                      if voice.low <= root + 12 * o + s <= voice.high]
        self.deg = len(self.scale) // 2
        self.t = 0.4
        self.syl: list[Syllable] = []
        self.breaths: list[tuple[float, float]] = []

    # -- helpers
    def u(self, a, b):
        return float(self.rng.uniform(a, b))

    def step(self, spread=2, lo=None, hi=None):
        lo = 0 if lo is None else lo
        hi = len(self.scale) - 1 if hi is None else hi
        d = int(self.rng.integers(-spread, spread + 1))
        if d == 0:
            d = 1 if self.rng.random() < 0.5 else -1
        if not lo <= self.deg + d <= hi:
            d = -d  # never repeat the same pitch at the edge of the range
        self.deg = int(np.clip(self.deg + d, lo, hi))
        return float(self.scale[self.deg])

    def vib(self, n: SNote):
        if n.off - n.on >= 0.45:
            n.vib_rate = self.u(5.0, 6.5)
            n.vib_cents = self.u(30, 70)
            n.vib_delay = self.u(0.2, 0.35)

    def consonant(self):
        r = self.rng.random()
        if r < 0.45:
            return str(self.rng.choice(["s", "sh", "f", "t", "k"])), self.u(0.05, 0.11)
        return None, 0.0

    def breath(self):
        d = self.u(0.3, 0.5)
        self.breaths.append((self.t, self.t + d))
        self.t += d + self.u(0.05, 0.15)

    def add_syllable(self, durs_midis, legato_from=None, vowel=None, join=False, extra=None):
        """One syllable with one or more notes (durations, pitches)."""
        cons, cd = (None, 0.0) if join else self.consonant()
        if cons:
            self.t += cd
        on = self.t
        s = Syllable(on, on, vowel or str(self.rng.choice(list("aeiou"))), consonant=cons,
                     cons_s=cd, voiced_join=join, gain_db=self.u(-1.5, 1.5))
        prev = legato_from
        for d, m in durs_midis:
            n = SNote(self.t, self.t + d, m)
            if prev is not None and prev != m:
                n.glide_from = prev
                n.glide_s = min(0.035, d / 3)
            self.t += d
            s.notes.append(n)
            prev = m
        s.off = self.t
        if extra:
            extra(s)
        self.syl.append(s)
        return s

    def rest(self, a=0.08, b=0.2):
        self.t += self.u(a, b)

    # -- phrase types
    def legato(self):
        self.breath()
        prev, join = None, False
        for i in range(6):
            m = self.step()
            s = self.add_syllable([(self.u(0.3, 0.6), m)], legato_from=prev if join else None, join=join)
            n = s.notes[0]
            if self.rng.random() < 0.3 and not join:
                n.scoop_cents, n.scoop_s = self.u(100, 200), self.u(0.06, 0.12)
            self.vib(n)
            prev = m
            join = self.rng.random() < 0.3  # the next syllable joins through a voiced consonant
            if not join and i < 5:
                self.rest(0.03, 0.08)

    def held(self):
        self.breath()
        for _ in range(2):
            self.add_syllable([(self.u(0.25, 0.4), self.step())])
            self.rest(0.03, 0.08)
        s = self.add_syllable([(self.u(1.5, 3.0), self.step())])
        self.vib(s.notes[0])
        self.rest(0.2, 0.4)

    def run(self):
        self.breath()
        self.add_syllable([(self.u(0.25, 0.4), self.step())])
        self.rest(0.03, 0.06)
        k = int(self.rng.integers(6, 10))
        direction = 1 if self.rng.random() < 0.5 else -1
        notes = []
        for j in range(k):
            if j > 0:
                d = direction if self.rng.random() < 0.8 else -direction
                if not 0 <= self.deg + d < len(self.scale):
                    d, direction = -d, -direction
                self.deg += d
            notes.append((self.u(0.09, 0.14), float(self.scale[self.deg])))
        notes.append((self.u(0.5, 0.8), self.step(1)))
        assert all(a[1] != b[1] for a, b in zip(notes, notes[1:]))
        s = self.add_syllable(notes, vowel=str(self.rng.choice(["a", "o"])))
        self.vib(s.notes[-1])
        self.rest(0.2, 0.4)

    def melisma(self):
        self.breath()
        for i in range(3):
            k = int(self.rng.integers(2, 4))
            s = self.add_syllable([(self.u(0.2, 0.35), self.step()) for _ in range(k)])
            for n in s.notes:
                self.vib(n)
            self.rest(0.03, 0.1)

    def leaps(self):
        self.breath()
        prev, slide = None, False
        for i in range(5):
            jump = int(self.rng.integers(3, 8)) * (1 if self.rng.random() < 0.5 else -1)
            self.deg = int(np.clip(self.deg + jump, 0, len(self.scale) - 1))
            m = float(self.scale[self.deg])
            if slide and m == prev:
                m = float(self.scale[max(self.deg - 3, 0)] if self.deg >= 3 else self.scale[self.deg + 3])
            s = self.add_syllable([(self.u(0.4, 0.7), m)], legato_from=prev if slide else None, join=slide)
            if slide:
                s.notes[0].glide_s = self.u(0.12, 0.25)
            self.vib(s.notes[0])
            prev = m
            slide = self.rng.random() < 0.6  # the next note slides in from this one
            if not slide:
                self.rest(0.05, 0.12)

    def spoken(self):
        self.breath()
        k = int(self.rng.integers(8, 13))
        decl = self.u(1.5, 3.0)
        for i in range(k):
            cons, cd = self.consonant()
            if cons:
                self.t += cd
            d = self.u(0.12, 0.26)
            base = self.v.speech_base - decl * i / k
            s = Syllable(self.t, self.t + d, str(self.rng.choice(list("aeiou"))), spoken=True,
                         contour=(base + self.u(-1.5, 1.5), base + self.u(-3.0, 2.5)),
                         consonant=cons, cons_s=cd, gain_db=self.u(-2, 2))
            self.syl.append(s)
            self.t += d + self.u(0.02, 0.07)
        self.rest(0.2, 0.4)

    def compose(self, cycles=2):
        plan = [self.legato, self.held, self.run, self.melisma, self.spoken, self.leaps]
        for _ in range(cycles):
            for p in plan:
                p()
        self.t += 0.6
        return self.syl, self.breaths, self.t


# --------------------------------------------------------------------- control tracks

def _raised_cos(x):
    x = np.clip(x, 0.0, 1.0)
    return 0.5 - 0.5 * np.cos(np.pi * x)


def control_tracks(voice: Voice, syls: list[Syllable], total: float, rng):
    c = int(math.ceil(total * CR)) + 2
    t = np.arange(c) / CR
    midi = np.full(c, np.nan)
    amp = np.zeros(c)
    dip = np.ones(c)
    form = np.zeros((4, c))
    bw = np.zeros((4, c))
    form_set = np.zeros(c, dtype=bool)
    for k, s in enumerate(syls):
        a, b = int(s.on * CR), int(math.ceil(s.off * CR))
        tt = t[a:b]
        g = 10 ** (s.gain_db / 20)
        if s.spoken:
            x = np.clip((tt - s.on) / max(s.off - s.on, 1e-3), 0.0, 1.0)
            midi[a:b] = s.contour[0] + (s.contour[1] - s.contour[0]) * (x ** self_curve(rng)) \
                + np.cumsum(rng.normal(0, 0.03, b - a))
            env = _raised_cos((tt - s.on) / 0.015) * _raised_cos((s.off - tt) / 0.04)
            amp[a:b] = np.maximum(amp[a:b], 0.8 * g * env)
        else:
            drift = np.cumsum(rng.normal(0, 0.004, b - a))
            drift -= drift.mean()
            for n in s.notes:
                na, nb = int(n.on * CR), int(math.ceil(n.off * CR))
                nt = t[na:nb] - n.on
                m = np.full(nb - na, n.midi, dtype=float)
                if n.glide_from is not None and n.glide_s > 0:
                    w = _raised_cos(nt / n.glide_s)
                    m = n.glide_from + (n.midi - n.glide_from) * w
                if n.scoop_s > 0:
                    m = m - (n.scoop_cents / 100) * (1 - _raised_cos(nt / n.scoop_s))
                if n.vib_rate > 0:
                    ramp = _raised_cos((nt - n.vib_delay) / 0.2)
                    m = m + ramp * (n.vib_cents / 100) * np.sin(2 * np.pi * n.vib_rate * nt)
                midi[na:nb] = m + drift[na - a:nb - a]
            joined_next = k + 1 < len(syls) and syls[k + 1].voiced_join and abs(syls[k + 1].on - s.off) < 1e-9
            env = np.ones(b - a)
            if not s.voiced_join:
                env *= _raised_cos((tt - s.on) / 0.03)
            if not joined_next:
                env *= _raised_cos((s.off - tt) / 0.06)
            amp[a:b] = np.maximum(amp[a:b], g * env)
            if s.voiced_join and k > 0:
                # A voiced consonant between legato syllables: about 10 dB down for 60 ms.
                bt = t[max(a - 40, 0):a + 40]
                dip[max(a - 40, 0):a + 40] = np.minimum(dip[max(a - 40, 0):a + 40],
                                                        1 - 0.7 * np.exp(-0.5 * ((bt - s.on) / 0.015) ** 2))
        f, w = VOWELS[s.vowel]
        for i in range(4):
            form[i, a:b] = f[i] * voice.formant_scale
            bw[i, a:b] = w[i]
        form_set[a:b] = True
    # Hold the last pitch and formants through gaps (no jumps when voicing resumes).
    last_m, last_f = 60.0, form[:, 0].copy()
    for i in range(c):
        if np.isnan(midi[i]):
            midi[i] = last_m
        else:
            last_m = midi[i]
        if form_set[i]:
            last_f = form[:, i].copy()
        else:
            form[:, i] = last_f
            bw[:, i] = (90, 110, 170, 250)
    if not form_set[0]:
        form[:, :] = np.where(form == 0, np.array(VOWELS["a"][0])[:, None] * voice.formant_scale, form)
        bw[:, :] = np.where(bw == 0, np.array(VOWELS["a"][1])[:, None], bw)
    # Formant transitions over about 30 ms.
    ker = np.ones(31) / 31
    for i in range(4):
        form[i] = np.convolve(np.pad(form[i], 15, mode="edge"), ker, mode="valid")
    f0 = A4_HZ * 2 ** ((midi - 69) / 12)
    # Singers raise F1 above the fundamental on high notes.
    form[0] = np.maximum(form[0], 1.12 * f0)
    form[1] = np.maximum(form[1], form[0] + 250)
    return f0, amp * dip, form, bw


def self_curve(rng):
    return float(rng.uniform(0.6, 1.6))


# ------------------------------------------------------------------------- synthesis

@njit(cache=True)
def _synth(f0c, ampc, formc, bwc, jit, shim, noise, n, sr, cr):
    out = np.zeros(n)
    phase = 0.0
    cyc = 0
    jf = 1.0
    sf = 1.0
    g_prev = 0.0
    y1 = np.zeros(4)
    y2 = np.zeros(4)
    tp, tn = 0.40, 0.16
    nc = f0c.shape[0]
    for i in range(n):
        x = i * cr / sr
        c0 = int(x)
        if c0 >= nc - 1:
            c0 = nc - 2
        w = x - c0
        f0 = f0c[c0] * (1 - w) + f0c[c0 + 1] * w
        a = ampc[c0] * (1 - w) + ampc[c0 + 1] * w
        phase += f0 * jf / sr
        if phase >= 1.0:
            phase -= 1.0
            cyc += 1
            jf = 1.0 + jit[cyc % jit.shape[0]]
            sf = 1.0 + shim[cyc % shim.shape[0]]
        if phase < tp:
            g = 0.5 * (1 - math.cos(math.pi * phase / tp))
        elif phase < tp + tn:
            g = math.cos(0.5 * math.pi * (phase - tp) / tn)
        else:
            g = 0.0
        e = (g - g_prev) * a * sf * 8.0
        g_prev = g
        e += a * noise[i] * 0.025 * (0.3 + g)
        v = e
        for k in range(4):
            fk = formc[k, c0] * (1 - w) + formc[k, c0 + 1] * w
            bk = bwc[k, c0] * (1 - w) + bwc[k, c0 + 1] * w
            if fk > 0.45 * sr:
                continue
            r = math.exp(-math.pi * bk / sr)
            cth = math.cos(2 * math.pi * fk / sr)
            b0 = 1 - 2 * r * cth + r * r
            y = b0 * v + 2 * r * cth * y1[k] - r * r * y2[k]
            y2[k] = y1[k]
            y1[k] = y
            v = y
        out[i] = v
    return out


def _bandnoise(rng, n, lo, hi):
    from scipy.signal import butter, sosfilt

    sos = butter(4, [lo / (SR / 2), min(hi / (SR / 2), 0.99)], btype="band", output="sos")
    return sosfilt(sos, rng.standard_normal(n))


CONSONANT_BANDS = {"s": (4500, 10000), "sh": (2200, 5500), "f": (1200, 9000), "t": (2500, 8000), "k": (1200, 4000)}


def sing(voice: Voice, cycles: int = 3):
    """Renders one voice's section. Returns (mono float32 at SR, ground truth dict)."""
    comp = Composer(voice)
    syls, breaths, total = comp.compose(cycles)
    rng = np.random.default_rng(voice.seed + 1000)
    f0c, ampc, formc, bwc = control_tracks(voice, syls, total, rng)
    n = int(total * SR)
    jit = rng.normal(0, 0.006, 1 << 16)
    shim = rng.normal(0, 0.05, 1 << 16)
    noise = rng.standard_normal(n)
    y = _synth(f0c, ampc, formc, bwc, jit, shim, noise, n, SR, CR)
    peak_voice = np.percentile(np.abs(y), 99.9) + 1e-9
    y /= peak_voice
    # Unvoiced consonants (before each syllable's voicing) and breaths.
    for s in syls:
        if s.consonant:
            a, b = int((s.on - s.cons_s) * SR), int(s.on * SR)
            lo, hi = CONSONANT_BANDS[s.consonant]
            burst = _bandnoise(rng, b - a, lo, hi)
            tt = np.linspace(0, 1, b - a)
            if s.consonant in ("t", "k"):
                env = np.where(tt > 0.6, np.exp(-(tt - 0.6) * 20), 0.0)  # closure, then burst
            else:
                env = np.sin(np.pi * tt) ** 0.7
            y[a:b] += 0.12 * env * burst / (np.std(burst) + 1e-9)
    for a_s, b_s in breaths:
        a, b = int(a_s * SR), int(b_s * SR)
        br = _bandnoise(rng, b - a, 400, 2500)
        env = np.sin(np.pi * np.linspace(0, 1, b - a)) ** 1.5
        y[a:b] += 0.05 * env * br / (np.std(br) + 1e-9)
    y = (0.5 * y / (np.max(np.abs(y)) + 1e-9)).astype(np.float32)  # peak -6 dBFS

    notes, unpitched = [], []
    for k, s in enumerate(syls):
        if s.spoken:
            unpitched.append({"onset": round(s.on, 4), "offset": round(s.off - 0.02, 4)})
            continue
        joined_next = k + 1 < len(syls) and syls[k + 1].voiced_join and abs(syls[k + 1].on - s.off) < 1e-9
        for j, nt in enumerate(s.notes):
            last = j == len(s.notes) - 1 and not joined_next
            off = nt.off - (0.03 if last else 0.0)  # middle of the release ramp
            sung = nt.midi + TUNING_SEMITONES
            notes.append({"onset": round(nt.on, 4), "offset": round(off, 4), "midi": round(sung, 4),
                          "hz": round(440.0 * 2 ** ((sung - 69) / 12), 3),
                          "vibrato": nt.vib_rate > 0, "scoop": nt.scoop_s > 0,
                          "slide": nt.glide_s >= 0.1, "run": (nt.off - nt.on) < 0.15,
                          "multi": len(s.notes) > 1})
    truth = {"voice": voice.name, "sample_rate": SR, "duration_s": round(total, 3),
             "a4_hz": A4_HZ, "tuning_cents": round(100 * TUNING_SEMITONES, 2),
             "notes": notes, "unpitched": unpitched,
             "breaths": [{"onset": round(a, 4), "offset": round(b, 4)} for a, b in breaths]}
    return y, truth

# SPDX-License-Identifier: GPL-3.0-or-later
"""CTC forced alignment on synthetic log-probabilities (no model, no GPU).

Each test builds a frame x vocabulary log-probability matrix in which chosen letters
"fire" at chosen frames, and a vocal level track, then checks the aligner's times.
This covers WORK_ORDER section 7 trap 8: held notes, repeated choruses (wrong-repeat
jumps), long instrumental breaks, plus anchors and untokenised characters.
"""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jukeoke_studio import align as A  # noqa: E402
from jukeoke_studio.lyrics import from_lrc, from_text  # noqa: E402

LETTERS = list("ABCDEFGHIJKLMNOPQRSTUVWXYZ'")
VOCAB = {c: i + 1 for i, c in enumerate(LETTERS)}   # blank = 0
SEP = len(LETTERS) + 1                               # word separator '|'
V = SEP + 1
FPS = 50


def make_lp(T, events, base_blank=-0.02, other=-9.0, peak=-0.05):
    """events: list of (frame, letter). Returns log-probs (T, V)."""
    lp = np.full((T, V), other, np.float32)
    lp[:, 0] = base_blank
    for f, ch in events:
        lp[f, :] = other
        lp[f, VOCAB[ch]] = peak
    return lp


def sing(text_frames):
    """[(start_frame, 'WORD', frames_per_letter)] -> events."""
    ev = []
    for f0, word, step in text_frames:
        for k, ch in enumerate(word):
            ev.append((f0 + k * step, ch))
    return ev


def levels(T, voiced_ranges, loud=-12.0, quiet=-80.0):
    lv = np.full(T, quiet)
    for a, b in voiced_ranges:
        lv[a:b] = loud
    return lv


def run(lp, lv, lyr, **kw):
    # No onset calibration in synthetic tests: the letters fire exactly where placed.
    cfg = kw.pop("config", A.AlignConfig(time_shift_s=0.0))
    return A.align_with_log_probs(lp, lv, lyr, vocab_ids=VOCAB, blank_id=0, sep_id=SEP,
                                  duration=lp.shape[0] / FPS, config=cfg, **kw)


def word_starts(doc):
    return [w["start"] for ln in doc["lines"] for w in ln["words"]]


def check_schema(doc, lyr):
    """docs/SONG_FORMAT.md section 5 invariants."""
    t = 0.0
    assert doc["lyrics_version"] == 1
    for ln, src in zip(doc["lines"], lyr.lines):
        assert ln["start"] <= ln["end"]
        assert " ".join("".join(c[0] for c in w["chars"]) for w in ln["words"]) == src.text
        for w in ln["words"]:
            assert w["chars"], "chars must be non-empty"
            assert w["start"] == w["chars"][0][1] and w["end"] == w["chars"][-1][2]
            for g, s, e in w["chars"]:
                assert t - 1e-9 <= s <= e, (g, s, e, t)
                t = e


def test_viterbi_recovers_onsets():
    T = 400
    lp = make_lp(T, sing([(50, "HELLO", 5), (150, "WORLD", 6)]))
    lyr = from_text("hello world")
    res = run(lp, levels(T, [(48, 80), (148, 190)]), lyr)
    check_schema(res.lyrics, lyr)
    assert word_starts(res.lyrics) == pytest.approx([1.0, 3.0], abs=0.021)
    chars = res.lyrics["lines"][0]["words"][0]["chars"]
    assert [c[1] for c in chars] == pytest.approx([1.0, 1.1, 1.2, 1.3, 1.4], abs=0.021)


def test_repeated_letters_need_blank():
    # "LL" must be two separate emissions; the path cannot merge them.
    T = 200
    lp = make_lp(T, [(20, "A"), (40, "L"), (41, "L"), (60, "L"), (80, "Y")])
    lyr = from_text("ally")
    path = A.ctc_viterbi(lp, np.array([VOCAB[c] for c in "ALLY"]), 0, sep_id=SEP)
    toks = path[path % 2 == 1] // 2
    assert sorted(set(toks.tolist())) == [0, 1, 2, 3]
    res = run(lp, levels(T, [(15, 100)]), lyr)
    s = [c[1] for c in res.lyrics["lines"][0]["words"][0]["chars"]]
    assert s[0] == pytest.approx(0.4, abs=0.021) and s[3] == pytest.approx(1.6, abs=0.021)


def test_held_note_extends_through_hold_not_silence():
    # "go" : G at 1.0 s, O at 1.1 s, sung (voiced) until 3.0 s, silence until the next
    # word at 5.0 s. The O must end at ~3.0 s, not at 5.0 s and not at 1.12 s.
    T = 400
    lp = make_lp(T, [(50, "G"), (55, "O"), (250, "N"), (255, "O")])
    lv = levels(T, [(48, 150), (248, 300)])
    lyr = from_text("go\nno")
    res = run(lp, lv, lyr)
    check_schema(res.lyrics, lyr)
    o = res.lyrics["lines"][0]["words"][0]["chars"][1]
    assert o[0] == "o"
    assert o[1] == pytest.approx(1.1, abs=0.021)
    assert o[2] == pytest.approx(3.0, abs=0.05)
    # A dip of a few frames inside the hold (vibrato, breath noise) is bridged.
    lv2 = lv.copy()
    lv2[100:102] = -80.0
    o2 = run(lp, lv2, lyr).lyrics["lines"][0]["words"][0]["chars"][1]
    assert o2[2] == pytest.approx(3.0, abs=0.05)


def test_held_vowel_inside_word():
    # "love": L, O at 1.0 s held to 2.5 s where V, E come: O covers the hold.
    T = 300
    lp = make_lp(T, [(50, "L"), (52, "O"), (125, "V"), (128, "E")])
    res = run(lp, levels(T, [(48, 140)]), from_text("love"))
    ch = res.lyrics["lines"][0]["words"][0]["chars"]
    assert ch[1][0] == "o" and ch[1][1] == pytest.approx(1.04, abs=0.021)
    assert ch[1][2] == pytest.approx(2.5, abs=0.021)


def _chorus_song():
    """Audio: chorus at 2 s, 12 s and 22 s (identical); lyrics list it only twice:
    the second written chorus is the THIRD sung one (the singer repeated it)."""
    T = 30 * FPS
    ev = []
    for t0 in (2, 12, 22):
        ev += sing([(t0 * FPS, "SHINE", 4), (t0 * FPS + 40, "ON", 4)])
    ev += sing([(7 * FPS, "RHYME", 4)])
    lv = levels(T, [(t * FPS - 5, t * FPS + 60) for t in (2, 12, 22)] + [(7 * FPS - 5, 7 * FPS + 30)])
    return make_lp(T, ev), lv


def test_repeated_chorus_needs_anchors():
    lp, lv = _chorus_song()
    # With LRC line times the right repeats are chosen.
    lyr = from_lrc("[00:02.00]shine on\n[00:07.00]rhyme\n[00:22.00]shine on\n")
    res = run(lp, lv, lyr)
    check_schema(res.lyrics, lyr)
    assert res.constraint_level == "lines"
    starts = [ln["start"] for ln in res.lyrics["lines"]]
    assert starts == pytest.approx([2.0, 7.0, 22.0], abs=0.03)
    # Hard anchors (user fixed line 3) pick the third chorus as well, exactly.
    lyr2 = from_text("shine on\nrhyme\nshine on")
    res2 = run(lp, lv, lyr2, anchors=[A.Anchor(2, 22.013)])
    assert res2.lyrics["lines"][2]["start"] == 22.013
    assert [ln["start"] for ln in res2.lyrics["lines"]][:2] == pytest.approx([2.0, 7.0], abs=0.03)


def test_lrc_with_constant_offset_is_corrected():
    # The same song with an LRC whose times are all 3 s late (e.g. made for an edit with
    # a longer intro). Used as-is, line 1's window [4 s, 11 s] holds no "shine on" at all.
    lp, lv = _chorus_song()
    lyr = from_lrc("[00:05.00]shine on\n[00:10.00]rhyme\n[00:25.00]shine on\n")
    res = run(lp, lv, lyr)
    check_schema(res.lyrics, lyr)
    assert res.constraint_level == "lines"
    assert [ln["start"] for ln in res.lyrics["lines"]] == pytest.approx([2.0, 7.0, 22.0], abs=0.03)
    assert any("off the vocal" in w for w in res.warnings)
    # Without the offset check the windows put line 1 in the wrong place.
    bad = run(lp, lv, lyr, config=A.AlignConfig(time_shift_s=0.0, lrc_offset_check=False))
    assert abs(bad.lyrics["lines"][0]["start"] - 2.0) > 1.0
    # A correct LRC is left alone (no shift, no warning).
    ok = run(lp, lv, from_lrc("[00:02.00]shine on\n[00:07.00]rhyme\n[00:22.00]shine on\n"))
    assert not ok.warnings


@pytest.mark.parametrize("mode", ["windows", "verify"])
def test_transcript_anchors_pick_the_right_repeat(mode):
    # Chorus "shine on me" sung at 2, 12 and 22 s; written twice; the transcript says
    # the second written one is the one at 22 s.
    T = 30 * FPS
    ev = []
    for t0 in (2, 12, 22):
        ev += sing([(t0 * FPS, "SHINE", 4), (t0 * FPS + 30, "ON", 4), (t0 * FPS + 45, "ME", 4)])
    ev += sing([(7 * FPS, "RHYME", 4)])
    lv = levels(T, [(t * FPS - 5, t * FPS + 60) for t in (2, 12, 22)] + [(7 * FPS - 5, 7 * FPS + 30)])
    lp = make_lp(T, ev)
    lyr = from_text("shine on me\nrhyme\nshine on me")
    tr = [A.TranscriptWord("shine", 2.0, 2.3), A.TranscriptWord("on", 2.6, 2.8),
          A.TranscriptWord("me", 2.9, 3.1), A.TranscriptWord("rhyme", 7.0, 7.4),
          A.TranscriptWord("shine", 22.1, 22.4), A.TranscriptWord("on", 22.6, 22.8),
          A.TranscriptWord("me", 22.9, 23.1)]
    cfg = A.AlignConfig(time_shift_s=0.0, transcript_mode=mode)
    res = run(lp, lv, lyr, transcript=tr, config=cfg)
    starts = [ln["start"] for ln in res.lyrics["lines"]]
    assert starts == pytest.approx([2.0, 7.0, 22.0], abs=0.03)
    if mode == "windows":
        assert res.constraint_level == "words"
    else:
        # Only constrained if the unconstrained pass picked the 12 s repeat.
        assert res.constraint_level in ("words", "none")


def test_hard_anchor_overrides_acoustics():
    T = 300
    lp = make_lp(T, sing([(50, "AB", 5), (100, "CD", 5)]))
    lyr = from_text("ab\ncd")
    res = run(lp, levels(T, [(45, 120)]), lyr, anchors=[A.Anchor(1, 3.5)])
    check_schema(res.lyrics, lyr)
    assert res.lyrics["lines"][1]["start"] == 3.5
    assert res.lyrics["lines"][0]["end"] <= 3.5
    assert res.lyrics["alignment"]["anchored_lines"] is True


def test_long_instrumental_break_not_filled():
    # Weak spurious letter activity inside a silent 20 s break must not attract the
    # second line; the silence penalty keeps it at its real place at 25 s.
    T = 30 * FPS
    ev = sing([(1 * FPS, "RAIN", 5), (25 * FPS, "FALLS", 5)])
    lp = make_lp(T, ev)
    rng = np.random.default_rng(0)
    for f in rng.integers(4 * FPS, 24 * FPS, 60):
        ch = "FALS"[int(rng.integers(0, 4))]
        lp[f, VOCAB[ch]] = -1.0   # bleed: plausible but weaker than the real singing
        lp[f, 0] = -0.5
    lv = levels(T, [(1 * FPS - 3, 1 * FPS + 30), (25 * FPS - 3, 25 * FPS + 35)])
    res = run(lp, lv, from_text("rain\nfalls"))
    assert res.lyrics["lines"][1]["start"] == pytest.approx(25.0, abs=0.03)


def test_untokenised_characters_get_interpolated_times():
    # Digits are spelled ("2" -> TWO), punctuation gets zero length at its place,
    # an emoji word between aligned words is placed in the gap.
    T = 500
    ev = sing([(50, "TWO", 4), (150, "CAFE", 4), (300, "GO", 4)])
    lp = make_lp(T, ev)
    lyr = from_text("2, café 🎵 go!")
    res = run(lp, levels(T, [(45, 70), (145, 175), (295, 320)]), lyr)
    check_schema(res.lyrics, lyr)
    w = res.lyrics["lines"][0]["words"]
    two, comma = w[0]["chars"]
    assert two[0] == "2" and two[1] == pytest.approx(1.0, abs=0.021)
    assert comma[1] == comma[2] == two[2]
    assert [c[0] for c in w[1]["chars"]] == ["c", "a", "f", "é"]
    assert w[1]["chars"][3][1] == pytest.approx(3.24, abs=0.021)
    note = w[2]
    assert w[1]["end"] <= note["start"] <= note["end"] <= w[3]["start"]
    bang = w[3]["chars"][-1]
    assert bang[0] == "!" and bang[1] == bang[2]


def test_unalignable_words_are_reported():
    # Words in a script the English model has no letters for get interpolated times
    # only; the result must say so instead of looking aligned. Punctuation-only words
    # ("...") are not counted.
    T = 400
    lp = make_lp(T, sing([(50, "HELLO", 5)]))
    res = run(lp, levels(T, [(48, 80)]), from_text("hello ...\nこんにちは 世界"))
    check_schema(res.lyrics, from_text("hello ...\nこんにちは 世界"))
    assert any(w.startswith("2 of 4 words have no letters") for w in res.warnings), res.warnings
    assert not run(lp, levels(T, [(48, 80)]), from_text("hello ...")).warnings


def test_infeasible_windows_relax_with_warning():
    T = 300
    lp = make_lp(T, sing([(50, "AB", 5), (150, "CD", 5)]))
    # Contradictory line times (second line before the first): the windows cannot be
    # met, so the aligner falls back to unconstrained alignment and says so.
    lyr = from_text("ab\ncd")
    lyr.lines[0].start, lyr.lines[1].start = 4.0, 0.1
    res = run(lp, levels(T, [(45, 170)]), lyr)
    check_schema(res.lyrics, lyr)
    assert res.warnings and res.constraint_level == "none"
    assert word_starts(res.lyrics) == pytest.approx([1.0, 3.0], abs=0.021)


def test_spell_digits_and_units():
    assert A.spell_digits("1999") == "nineteen ninety nine"
    assert A.spell_digits("2005") == "two thousand five"
    assert A.spell_digits("1905") == "nineteen oh five"
    assert A.spell_digits("42") == "forty two"
    assert A.spell_digits("007") == "zero zero seven"
    us = A.word_units(list("don't"), VOCAB, "en")
    assert [len(u.tokens) for u in us] == [1, 1, 1, 1, 1]
    us = A.word_units(list("'cause"), VOCAB, "en")
    assert us[0].tokens == [] and us[0].weight == 0.0
    assert A.grapheme_letters("é", set(VOCAB)) == "E"
    assert A.grapheme_letters("ß", set(VOCAB)) == "SS"
    assert A.grapheme_letters("😀", set(VOCAB)) == ""


def test_match_transcript_monotonic():
    lyr = "the sun is shining on the sea the sun is shining".split()
    tr = "the sun is shinin on sea and the son is shining".split()
    pairs = A.match_transcript(lyr, tr)
    assert pairs == sorted(pairs)
    assert {(3, 3), (6, 5), (9, 9), (10, 10)} <= set(pairs)


def test_align_stage_whisper_anchors_are_opt_in():
    calls = []

    class FakeAligner:
        def align(self, vocal, sr, lyr, anchors=None, transcript=None):
            calls.append(transcript)
            return "aligned"

    class FakeTranscriber:
        def transcribe_words(self, *a, **k):
            return [A.TranscriptWord("a", 0.0, 0.1)], {}

    lyr = from_text("a")
    assert A.align_stage(FakeAligner(), np.zeros(16000), 16000, lyr,
                         transcriber=FakeTranscriber()) == "aligned"
    assert calls == [None]          # default: no Whisper run, no transcript windows
    A.align_stage(FakeAligner(), np.zeros(16000), 16000, lyr, transcriber=FakeTranscriber(),
                  coarse_anchors=True)
    assert calls[-1] and calls[-1][0].text == "a"

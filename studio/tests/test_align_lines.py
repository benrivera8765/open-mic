# SPDX-License-Identifier: GPL-3.0-or-later
"""Grouping of transcript words into lyric lines (no model needed).

The words below are invented for these tests (no lyrics from any song)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jukeoke_studio.align import TranscriptWord as W  # noqa: E402
from jukeoke_studio.transcribe import TranscribeConfig, words_to_lines  # noqa: E402


def test_held_note_gap_does_not_split_short_line():
    # "Over ... the blue hills": Whisper ends "Over" early because the note is held,
    # which looks like a 1.5 s pause; a one-word line would be wrong.
    ws = [W("Over", 10.0, 10.3), W("the", 11.8, 12.0), W("blue", 12.0, 12.2),
          W("hills,", 12.2, 13.0), W("I", 14.6, 14.7), W("walk", 14.7, 15.0),
          W("on", 15.0, 15.2), W("my", 15.2, 15.3), W("way", 15.3, 16.0)]
    lines = words_to_lines(ws, [0])
    assert [ln.text for ln in lines] == ["Over the blue hills,", "I walk on my way"]
    assert lines[0].words[0].start == 10.0


def test_capitals_punctuation_and_long_lines():
    ws = [W(t, i * 0.4, i * 0.4 + 0.3) for i, t in enumerate(
        "Paint the town tonight Dancing until I see the morning light again.".split())]
    ws += [W(t, 10 + i * 0.3, 10 + i * 0.3 + 0.2) for i, t in enumerate(
        "one two three four five six seven eight nine ten eleven twelve".split())]
    lines = [ln.text for ln in words_to_lines(ws, [0], TranscribeConfig(max_line_words=10))]
    assert lines[0] == "Paint the town tonight"
    # A capitalised "I" inside a line does not start a new line.
    assert lines[1] == "Dancing until I see the morning light again."
    assert all(len(t.split()) <= 10 for t in lines) and len(lines) == 4
    assert " ".join(lines[2:]) == "one two three four five six seven eight nine ten eleven twelve"


def test_long_pause_always_splits():
    ws = [W("oh", 1.0, 1.2), W("yeah", 5.0, 5.3)]
    assert [ln.text for ln in words_to_lines(ws, [])] == ["oh", "yeah"]

# SPDX-License-Identifier: GPL-3.0-or-later
"""LRC parsing/formatting and lyrics text normalisation (no audio, no network)."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jukeoke_studio import lrc  # noqa: E402
from jukeoke_studio import lyrics as L  # noqa: E402


def test_basic_timestamps_and_formats():
    doc = lrc.parse_lrc("[00:01]a\n[00:02.5]b\n[00:03.25]c\n[00:04.125]d\n[00:05:50]e\n[1:00:01.00]f\n")
    assert [round(x.time, 3) for x in doc.lines] == [1.0, 2.5, 3.25, 4.125, 5.5, 3601.0]
    assert [x.text for x in doc.lines] == list("abcdef")


def test_multiple_timestamps_sorted():
    doc = lrc.parse_lrc("[00:10.00][01:10.00]Chorus line\n[00:05.00]Verse\n[00:40.00]Bridge\n")
    assert [(round(x.time, 2), x.text) for x in doc.lines] == [
        (5.0, "Verse"), (10.0, "Chorus line"), (40.0, "Bridge"), (70.0, "Chorus line")]


def test_tags_and_offset():
    text = "[ti:Song]\n[ar:Someone]\n[offset:+500]\n[00:10.00]Hello\n"
    doc = lrc.parse_lrc(text)
    assert doc.tags["ti"] == "Song" and doc.tags["ar"] == "Someone"
    assert doc.offset_ms == 500
    # Positive offset: lyrics appear sooner.
    assert doc.lines[0].time == pytest.approx(9.5)
    raw = lrc.parse_lrc(text, apply_offset=False)
    assert raw.lines[0].time == pytest.approx(10.0)
    neg = lrc.parse_lrc("[offset:-250]\n[00:01.00]x\n")
    assert neg.lines[0].time == pytest.approx(1.25)
    # Offsets never produce negative times.
    assert lrc.parse_lrc("[offset:5000]\n[00:01.00]x\n").lines[0].time == 0.0


def test_enhanced_word_tags():
    doc = lrc.parse_lrc("[00:12.00]<00:12.00>Hello <00:12.60>big <00:13.10>world, <00:14.00>\n")
    ln = doc.lines[0]
    assert ln.text == "Hello big world,"
    assert [(w.text, w.start, w.end) for w in ln.words] == [
        ("Hello", 12.0, 12.6), ("big", 12.6, 13.1), ("world,", 13.1, 14.0)]
    # Repeated enhanced line keeps relative word times.
    doc2 = lrc.parse_lrc("[00:10.00][00:20.00]<00:10.00>a <00:10.50>b\n")
    assert [w.start for w in doc2.lines[1].words] == [20.0, 20.5]


def test_empty_timed_line_marks_end():
    lines = L.lines_from_lrc_doc(lrc.parse_lrc("[00:01.00]one two\n[00:03.00]\n[00:30.00]three\n"))
    assert len(lines) == 2
    assert lines[0].start == 1.0 and lines[0].end == 3.0
    assert lines[1].start == 30.0 and lines[1].end is None


def test_format_roundtrip():
    text = "[ti:T]\n[00:01.00]<00:01.00>a <00:01.50>b <00:02.00>\n[00:03.10]c d\n"
    doc = lrc.parse_lrc(text)
    out = lrc.format_lrc(doc, enhanced=True)
    again = lrc.parse_lrc(out)
    assert [(x.time, x.text) for x in again.lines] == [(x.time, x.text) for x in doc.lines]
    assert [(w.start, w.end) for w in again.lines[0].words] == [(1.0, 1.5), (1.5, 2.0)]
    assert lrc.format_timestamp(61.235) == "[01:01.24]" or lrc.format_timestamp(61.235) == "[01:01.23]"
    assert lrc.format_timestamp(3599.999) == "[60:00.00]"


def test_lyrics_json_to_lrc():
    doc = {"lines": [{"start": 1.0, "end": 2.0, "words": [
        {"start": 1.0, "end": 1.4, "chars": [["H", 1.0, 1.2], ["i", 1.2, 1.4]]},
        {"start": 1.5, "end": 2.0, "chars": [["y", 1.5, 1.7], ["o", 1.7, 2.0], ["u", 2.0, 2.0]]}]}]}
    out = lrc.lyrics_json_to_lrc(doc)
    assert out == "[00:01.00]<00:01.00>Hi <00:01.50>you <00:02.00>\n"


def test_has_timestamps():
    assert lrc.has_timestamps("\ufeff[00:01.00]x")
    assert not lrc.has_timestamps("just [words] here\nand (more)")


# --------------------------------------------------------------------------- lyrics

def test_graphemes_and_punctuation():
    lyr = L.from_text("Hello, wörld!  It's  café\u0301 time\n\n[Chorus]\nnaïve 👍🏽 e\u0301\n")
    assert [ln.text for ln in lyr.lines] == ["Hello, wörld! It's café\u0301 time", "naïve 👍🏽 é"]
    w = lyr.lines[0].words
    assert w[0].text == "Hello," and w[0].graphemes == ["H", "e", "l", "l", "o", ","]
    assert w[1].graphemes == ["w", "ö", "r", "l", "d", "!"]
    # 'é' + combining acute stays one grapheme cluster
    assert w[3].graphemes == ["c", "a", "f", "é\u0301"]
    assert lyr.lines[1].words[1].graphemes == ["👍🏽"]
    # NFC: decomposed e + acute is composed
    assert lyr.lines[1].words[2].graphemes == ["é"]
    assert lyr.source == "user_text" and lyr.checked is True


def test_section_headers_dropped_but_sung_parentheses_kept():
    text = "[Verse 1: Someone]\nfirst line\n(Chorus)\nChorus:\nVerse 2\n(ooh, baby)\nbreak\nBridge x2\nlast"
    assert [ln.text for ln in L.from_text(text).lines] == ["first line", "(ooh, baby)", "break", "last"]


def test_invisible_characters_removed():
    lyr = L.from_text("a\u200bb\u00a0c\u00add")
    assert lyr.lines[0].text == "ab cd"


def test_text_that_is_lrc_becomes_lrc():
    lyr = L.from_text("[00:01.00]one\n[00:02.00]two\n")
    assert lyr.source == "lrc" and lyr.synced
    assert [ln.start for ln in lyr.lines] == [1.0, 2.0]
    assert lyr.lines[0].end == 2.0


def test_resolve_order():
    trans_called = []

    def tr():
        trans_called.append(1)
        return L.Lyrics([L.make_line("from whisper")], "transcribed", checked=False)

    assert L.resolve_lyrics("auto", lrc="[00:01.00]x", text="y", transcribe=tr).source == "lrc"
    assert L.resolve_lyrics("auto", text="y", transcribe=tr).source == "user_text"
    out = L.resolve_lyrics("auto", transcribe=tr)
    assert out.source == "transcribed" and out.checked is False and trans_called
    assert L.resolve_lyrics("none", text="x") is None
    with pytest.raises(L.LyricsMissing):
        L.resolve_lyrics("text", text="  ")
    with pytest.raises(L.LyricsMissing):
        L.resolve_lyrics("lrc")
    with pytest.raises(L.LyricsMissing):
        L.resolve_lyrics("auto")

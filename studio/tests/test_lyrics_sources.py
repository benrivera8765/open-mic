# SPDX-License-Identifier: GPL-3.0-or-later
"""Embedded lyrics (mutagen) and the LRCLIB client (with a fake network).

Audio files are generated in a temporary folder (a few hundred ms of silence); nothing
is committed. The live LRCLIB test only runs with JK_NETWORK_TESTS=1.
"""

import io
import json
import os
import sys
import urllib.error
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jukeoke_studio import lyrics as L  # noqa: E402

sf = pytest.importorskip("soundfile")
mutagen = pytest.importorskip("mutagen")


def _wav(path, sr=44100, seconds=0.3, fmt="WAV", subtype=None):
    sf.write(str(path), np.zeros((int(sr * seconds), 2), np.float32), sr, format=fmt, subtype=subtype)
    return path


def test_flac_vorbis_lyrics(tmp_path):
    from mutagen.flac import FLAC

    p = _wav(tmp_path / "a.flac", fmt="FLAC", subtype="PCM_16")
    f = FLAC(str(p))
    f["LYRICS"] = "Line one\nLine two"
    f.save()
    info = L.embedded_lyrics_info(p)
    assert info == {"present": True, "synced": False, "text": "Line one\nLine two"}
    lyr = L.from_embedded(p)
    assert lyr.source == "embedded" and [x.text for x in lyr.lines] == ["Line one", "Line two"]


def test_flac_synced_lrc_in_unsyncedlyrics(tmp_path):
    from mutagen.flac import FLAC

    p = _wav(tmp_path / "b.flac", fmt="FLAC", subtype="PCM_16")
    f = FLAC(str(p))
    f["UNSYNCEDLYRICS"] = "[00:01.00]Hello\n[00:02.50]World"
    f.save()
    info = L.embedded_lyrics_info(p)
    assert info["present"] and info["synced"] and info["text"].startswith("[00:01.00]")
    lyr = L.from_embedded(p)
    assert [ln.start for ln in lyr.lines] == [1.0, 2.5]


def test_id3_uslt_and_sylt(tmp_path):
    from mutagen.id3 import SYLT, USLT
    from mutagen.wave import WAVE

    p = _wav(tmp_path / "c.wav")
    w = WAVE(str(p))
    w.add_tags()
    w.tags.add(USLT(encoding=3, lang="eng", desc="", text="plain words"))
    w.tags.add(SYLT(encoding=3, lang="eng", format=2, type=1, desc="",
                    text=[("First line", 1000), ("Second line", 2500)]))
    w.save()
    e = L.read_embedded_lyrics(p)
    # Synced (SYLT) wins over plain (USLT): it gives line anchors.
    assert e.synced and e.tag == "ID3 SYLT"
    lyr = L.from_embedded(p)
    assert [(ln.text, ln.start) for ln in lyr.lines] == [("First line", 1.0), ("Second line", 2.5)]


def test_id3_uslt_language_preference(tmp_path):
    from mutagen.id3 import ID3, SYLT, USLT

    p = tmp_path / "lang.mp3"
    p.write_bytes(b"\x00" * 64)
    t = ID3()
    # A longer translation must not win over the English lyrics.
    t.add(USLT(encoding=3, lang="deu", desc="de", text="eine viel laengere deutsche Uebersetzung hier"))
    t.add(USLT(encoding=3, lang="eng", desc="en", text="short english words"))
    t.save(str(p))
    assert L.read_embedded_lyrics(p).text == "short english words"
    # A synced tag with no stated language still beats plain English text (anchors).
    t.add(SYLT(encoding=3, lang="XXX", format=2, type=1, desc="s", text=[("short english words", 1000)]))
    t.save(str(p))
    e = L.read_embedded_lyrics(p)
    assert e.synced and e.tag == "ID3 SYLT"


def test_id3_sylt_syllables(tmp_path):
    from mutagen.id3 import ID3, SYLT

    p = tmp_path / "d.mp3"
    p.write_bytes(b"\x00" * 64)  # mutagen can tag any file with ID3
    t = ID3()
    t.add(SYLT(encoding=3, lang="eng", format=2, type=1, desc="",
               text=[("Hel", 1000), ("lo", 1200), (" you", 1500), ("\nNext", 3000), (" line", 3300)]))
    t.save(str(p))
    e = L.read_embedded_lyrics(p)
    assert e is not None and e.synced
    assert e.text.splitlines() == ["Hello you", "Next line"]


def test_mp4_lyr(tmp_path):
    av = pytest.importorskip("av")
    from mutagen.mp4 import MP4

    p = tmp_path / "e.m4a"
    with av.open(str(p), "w", format="mp4") as c:
        s = c.add_stream("aac", rate=44100)
        frame = av.AudioFrame.from_ndarray(np.zeros((1, 1024), np.float32), format="fltp", layout="mono")
        frame.sample_rate = 44100
        for _ in range(4):
            for pkt in s.encode(frame):
                c.mux(pkt)
        for pkt in s.encode(None):
            c.mux(pkt)
    m = MP4(str(p))
    m["\xa9lyr"] = ["Sung words here"]
    m.save()
    assert L.embedded_lyrics_info(p) == {"present": True, "synced": False, "text": "Sung words here"}


def test_no_lyrics(tmp_path):
    p = _wav(tmp_path / "f.wav")
    assert L.read_embedded_lyrics(p) is None
    assert L.embedded_lyrics_info(p)["present"] is False
    assert L.read_embedded_lyrics(tmp_path / "missing.flac") is None


# ---------------------------------------------------------------------------- LRCLIB

class _Resp(io.BytesIO):
    status = 200

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def _fake_opener(routes, seen):
    def opener(req, timeout=None):
        seen.append(req)
        url = req.full_url
        for prefix, payload in routes.items():
            if url.startswith(prefix):
                if payload == 404:
                    raise urllib.error.HTTPError(url, 404, "nf", {}, None)
                return _Resp(json.dumps(payload).encode())
        raise AssertionError(url)
    return opener


REC = {"id": 7, "trackName": "Song", "artistName": "Band", "albumName": "LP", "duration": 200.0,
       "instrumental": False, "plainLyrics": "a\nb", "syncedLyrics": "[00:01.00]a\n[00:02.00]b"}


def test_lrclib_get_then_search(monkeypatch):
    seen = []
    other = dict(REC, id=8, duration=203.0, syncedLyrics=None)
    far = dict(REC, id=9, duration=320.0)
    op = _fake_opener({"https://lrclib.net/api/get?": REC,
                       "https://lrclib.net/api/search?": [REC, far, other]}, seen)
    res = L.lookup_lrclib("Band", "Song", "LP", 199.6, opener=op, user_agent="TestApp/1 (test)")
    assert [r["id"] for r in res] == [7, 8]  # duplicate removed, 320 s version dropped
    assert res[0] == {"service": "lrclib", "synced_lrc": REC["syncedLyrics"], "plain": "a\nb",
                      "duration_seconds": 200.0, "instrumental": False, "artist": "Band",
                      "title": "Song", "album": "LP", "id": 7}
    q = seen[0].full_url
    assert "artist_name=Band" in q and "track_name=Song" in q and "duration=200" in q
    # Generic UA naming the app; nothing personal.
    assert seen[0].get_header("User-agent") == "TestApp/1 (test)"
    lyr = L.from_lookup_result(res[0])
    assert lyr.source == "online:lrclib" and lyr.synced


def test_lrclib_not_found_and_errors():
    seen = []
    op = _fake_opener({"https://lrclib.net/api/get?": 404, "https://lrclib.net/api/search?": []}, seen)
    assert L.lookup_lrclib("X", "Y", opener=op) == []

    def broken(req, timeout=None):
        raise urllib.error.URLError("offline")
    with pytest.raises(L.LyricsLookupError):
        L.lookup_lrclib("X", "Y", opener=broken)


def test_default_user_agent_is_generic():
    ua = L.default_user_agent()
    assert "/" in ua and "@" not in ua


@pytest.mark.skipif(os.environ.get("JK_NETWORK_TESTS") != "1", reason="network test (set JK_NETWORK_TESTS=1)")
def test_lrclib_live():
    res = L.lookup_lrclib(None, "Amazing Grace")  # public-domain hymn
    assert res and (res[0]["synced_lrc"] or res[0]["plain"])


def test_auto_falls_through_when_given_lyrics_normalise_to_nothing(tmp_path):
    """Verifier finding (f): LRC/text that hold no singable line (only headers, blank
    lines, bare timestamps) fall through to the next source, not straight to
    transcription."""
    from mutagen.flac import FLAC

    p = _wav(tmp_path / "e.flac", fmt="FLAC", subtype="PCM_16")
    f = FLAC(str(p))
    f["LYRICS"] = "Embedded line"
    f.save()
    called = []

    def tr():
        called.append(1)
        return L.Lyrics([L.make_line("heard")], L.SOURCE_TRANSCRIBED, checked=False)

    empty_lrc = "[00:01.00]\n[00:05.00][Chorus]\n"
    empty_text = "[Verse 1]\n\n(Chorus)\n"
    # lrc empty -> text
    out = L.resolve_lyrics("auto", lrc=empty_lrc, text="Typed line", media_path=p, transcribe=tr)
    assert out.source == "user_text" and out.text() == "Typed line" and not called
    # lrc and text empty -> embedded
    out = L.resolve_lyrics("auto", lrc=empty_lrc, text=empty_text, media_path=p, transcribe=tr)
    assert out.source == "embedded" and out.text() == "Embedded line" and not called
    # all empty -> transcription
    out = L.resolve_lyrics("auto", lrc=empty_lrc, text=empty_text, media_path=_wav(tmp_path / "n.wav"),
                           transcribe=tr)
    assert out.source == "transcribed" and called


def test_source_label_from_the_app():
    """Protocol: job.submit lyrics.source (e.g. online:lrclib for a lookup result)."""
    lrc = "[00:01.00]Hello there\n[00:03.00]General\n"
    assert L.resolve_lyrics("lrc", lrc=lrc, source="online:lrclib").source == "online:lrclib"
    assert L.resolve_lyrics("auto", text="Hi", source="online:lrclib").source == "online:lrclib"
    assert L.resolve_lyrics("auto", text="Hi", source="embedded").source == "embedded"
    assert L.resolve_lyrics("auto", lrc=lrc).source == "lrc"
    with pytest.raises(ValueError):
        L.resolve_lyrics("auto", text="Hi", source="made up")
    assert L.valid_source("online:lrclib") and not L.valid_source("transcribed")

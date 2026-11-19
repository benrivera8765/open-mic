# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for the 1.1 part of the Python .jksong implementation (Work Order 2): the melody
guide (SONG_FORMAT sections 7.1, 9, 10) and the practice data (section 11).

Run:  $JK_BUILD/studio-venv/Scripts/python.exe -m pytest studio/tests/test_jksong_guide.py
Needs numpy, soundfile, pytest. All audio generated. The Rust side of the same rules is
crates/jk-format/tests/guide.rs; tests/conformance checks the two against each other.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from jukeoke_studio import jksong  # noqa: E402
from test_jksong import SR, lyrics, tones  # noqa: E402


@pytest.fixture
def song(tmp_path: Path) -> Path:
    p = tmp_path / "Artist - Title.jksong"
    jksong.write_song(p, stems=tones(), sample_rate=SR, title="Title", artist="Artist", lyrics=lyrics(),
                      stems_sum_to_source=True, extra_chunks=[("x.future", "application/octet-stream", b"\x00opaque\xff")])
    jksong.update_user_state(p, lambda s: s["settings"].update(semitones=-2))
    jksong.update_user_state(p, lambda s: s.update(x_unknown={"kept": True}))
    return p


def make_guide(p: Path, notes=((0.1, 0.4, 69.0), (0.5, 0.9, 71.0))) -> tuple[bytes, bytes]:
    s = jksong.Song(p)
    lead = s.chunk_entry("stem.lead_vocal")
    doc = {
        "guide_version": 1, "hop_s": 0.01, "low_confidence": 0.5,
        "tuning": {"cents": -15.8, "strength": 0.91, "use_cents": -15.8},
        "range": None, "counts": None, "curve": "guide.curve",
        "notes": [{"start": a, "end": b, "midi": m, "confidence": 0.9, "kind": "pitched", "octave": "ok",
                   "glide_in": 0.0} for a, b, m in notes],
        "spans": [], "words": None,
        "provenance": {"method": "test", "version": "0.1.0", "parameters": {"tuning_min_strength": 0.3},
                       "source": {"role": "lead_vocal", "chunk": "stem.lead_vocal", "length": lead["length"],
                                  "crc32": lead["crc32"], "sha256": hashlib.sha256(s.read_chunk("stem.lead_vocal")).hexdigest(),
                                  "sample_rate": SR, "channels": 2}},
    }
    jksong.set_word_refs(doc, s.effective_lyrics(), False, 0)
    doc["counts"] = jksong.guide_counts(doc)
    n = jksong.curve_frames_for(s.manifest["duration_seconds"])
    curve = jksong.encode_curve(np.full(n, 69.0), np.full(n, 0.8), np.full(n, -20.0))
    return jksong._json_bytes(doc), curve


def fingerprints(p: Path):
    b = p.read_bytes()
    s = jksong.Song(p)
    h = s.header
    chunks = [(c["id"], c["offset"], hashlib.sha256(b[c["offset"]:c["offset"] + c["length"]]).hexdigest())
              for c in s.toc["chunks"]]
    a = hashlib.sha256(b[h["slot_a_offset"]:h["slot_a_offset"] + h["slot_size"]]).hexdigest()
    bb = hashlib.sha256(b[h["slot_b_offset"]:h["slot_b_offset"] + h["slot_size"]]).hexdigest()
    return chunks, a, bb


def no_guide(chunks):
    return [c for c in chunks if c[0] not in ("guide", "guide.curve")]


# ----------------------------------------------------------------------------- pieces

def test_rounding_matches_the_rust_rule():
    assert jksong.round3(12.3456) == 12.346
    assert jksong.round3(-0.0005) == -0.001
    assert jksong.round_half_away(87.45, 1) == 87.5
    assert jksong.round_half_away(2.5, 0) == 3.0          # Python's round() would say 2
    assert jksong.round_half_away(-2.5, 0) == -3.0


def test_curve_round_trip_and_checks():
    n = jksong.curve_frames_for(40.0)
    assert n == 4001
    b = jksong.encode_curve(np.full(n, 60.5), np.full(n, 0.9), np.full(n, -20.0))
    assert len(b) == 32 + 24 + 12 * n
    c = jksong.decode_curve(b)
    assert c["frames"] == n and c["hop_us"] == 10_000 and set(c["fields"]) == {"MIDI", "VOIC", "LVDB"}
    assert jksong.validate_curve(c, 40.0) == []
    assert jksong.validate_curve(c, 41.0)                      # does not cover the song
    with pytest.raises(jksong.Damaged):
        jksong.decode_curve(b[:-1])
    v2 = bytearray(b)
    v2[4] = 2
    with pytest.raises(jksong.NewerVersion):
        jksong.decode_curve(bytes(v2))
    bad = jksong.decode_curve(jksong.encode_curve([5.0, 60.0], [0.5, 1.5], [0, 0]))
    errs = jksong.validate_curve(bad, 0.01)
    assert any("MIDI" in e for e in errs) and any("VOIC" in e for e in errs)


def test_word_rule_matches_the_rust_cases():
    # The same cases as crates/jk-pitch/src/guide/words.rs.
    words = [(1.0, 2.0), (2.0, 2.5), (5.0, 6.0)]
    notes = [(1.1, 1.9), (1.75, 2.25), (2.05, 2.15), (3.0, 4.0), (4.9, 5.05), (5.95, 7.0)]
    assert jksong.map_words(notes, words) == [0, 0, 1, None, 2, None]
    assert jksong.map_words([(0.0, 1.0), (1.0, 2.0), (2.0, 2.98)], [(0.0, 3.0), (3.0, 3.1)]) == [0, 0, 0]
    assert jksong.map_words([(1.0, 2.0)], [(1.5, 1.5)]) == [None]
    assert jksong.map_words([(0.5, 2.5)], [(1.0, 1.1)]) == [0]


def test_word_refs_cache_rules(song: Path):
    s = jksong.Song(song)
    g, _ = make_guide(song)
    doc = json.loads(g)
    lyr = s.effective_lyrics()
    assert doc["words"] == {"lyrics": "chunk", "offset_ms": 0, "word_count": 2}
    assert [n.get("word") for n in doc["notes"]] == [[0, 0, 0.1], [0, 1, 0.5]]
    assert jksong.word_refs_valid(doc, lyr, 0)
    assert not jksong.word_refs_valid(doc, lyr, 250)           # another offset: recompute
    lyr2 = json.loads(json.dumps(lyr))
    lyr2["lines"][0]["words"][1]["start"] = 0.52               # the sync editor moved a word
    assert not jksong.word_refs_valid(doc, lyr2, 0)
    # A note is never moved or removed by the mapping.
    before = [(n["start"], n["end"], n["midi"]) for n in doc["notes"]]
    jksong.set_word_refs(doc, lyr, False, 3000)
    assert [(n["start"], n["end"], n["midi"]) for n in doc["notes"]] == before
    assert all("word" not in n for n in doc["notes"])          # 3 s late: no word overlaps


def test_validation_and_graded_rule():
    doc = json.loads(make_guide_doc_only())
    errs, warns = jksong.validate_guide(doc, 3.0)
    assert errs == [] and warns == []
    for mutate, needle in [
        (lambda d: d["notes"][1].update(start=0.05), "not ordered"),
        (lambda d: d["notes"][1].update(start=0.35), "overlaps"),
        (lambda d: d["notes"][0].update(confidence=1.5), "confidence"),
        (lambda d: d["notes"][0].update(midi=130), "midi"),
        (lambda d: d["tuning"].update(cents=70), "tuning.cents"),
        (lambda d: d["provenance"].pop("method"), "provenance.method"),
    ]:
        d = json.loads(make_guide_doc_only())
        mutate(d)
        errs, _ = jksong.validate_guide(d, 3.0)
        assert any(needle in e for e in errs), (needle, errs)
    n = {"kind": "pitched", "confidence": 0.5, "octave": "unchecked"}
    assert jksong.note_graded(n, 0.5)
    assert not jksong.note_graded({**n, "octave": "maybe_high"}, 0.5)
    assert not jksong.note_graded({**n, "octave": "sideways"}, 0.5)   # unknown = flagged
    assert not jksong.note_graded({**n, "kind": "shouted"}, 0.5)      # unknown = unpitched
    assert not jksong.note_graded({**n, "confidence": 0.49}, 0.5)


def make_guide_doc_only() -> bytes:
    doc = {"guide_version": 1, "hop_s": 0.01, "low_confidence": 0.5,
           "tuning": {"cents": 0.0, "strength": 1.0, "use_cents": 0.0}, "curve": "guide.curve",
           "notes": [{"start": 0.1, "end": 0.4, "midi": 60.0, "confidence": 0.9, "kind": "pitched", "octave": "ok",
                      "glide_in": 0.0},
                     {"start": 0.4, "end": 0.9, "midi": 62.0, "confidence": 0.9, "kind": "pitched", "octave": "ok",
                      "glide_in": 0.05}],
           "spans": [], "provenance": {"method": "m", "version": "1"}}
    return json.dumps(doc).encode()


# ----------------------------------------------------------------------------- 7.1

def test_add_guide_keeps_every_other_byte(song: Path):
    before = fingerprints(song)
    g, c = make_guide(song)
    jksong.add_guide(song, g, c)
    after = fingerprints(song)
    assert no_guide(after[0]) == before[0]                     # same bytes, same offsets
    assert after[1:] == before[1:]                              # both slot regions
    assert [x[0] for x in after[0][-2:]] == ["guide", "guide.curve"]
    s = jksong.Song(song)
    assert s.header["minor"] == 1
    assert s.read_chunk("guide") == g and s.read_chunk("guide.curve") == c
    assert s.effective_settings()["semitones"] == -2 and s.user_state["x_unknown"] == {"kept": True}
    assert jksong.guide_state(s, "test", "0.1.0") == "ready"
    assert jksong.guide_state(s, "test", "0.2.0") == "outdated"
    assert jksong.validate_guide(s.guide(), s.manifest["duration_seconds"]) == ([], [])
    assert jksong.validate_curve(s.guide_curve(), s.manifest["duration_seconds"]) == []
    assert not list(song.parent.glob("*.tmp-*"))


def test_replace_keeps_the_guide_in_place(song: Path):
    g, c = make_guide(song)
    jksong.add_guide(song, g, c)
    first = fingerprints(song)
    g2, c2 = make_guide(song, notes=((0.1, 0.3, 60.0),))
    jksong.add_guide(song, g2, c2)
    second = fingerprints(song)
    assert [x[0] for x in first[0]] == [x[0] for x in second[0]]
    assert no_guide(first[0]) == no_guide(second[0]) and first[1:] == second[1:]


@pytest.mark.parametrize("point", jksong.INTERRUPT_POINTS)
def test_an_interruption_leaves_the_old_file(song: Path, point: str):
    original = song.read_bytes()
    g, c = make_guide(song)
    with pytest.raises(jksong.GuideInterrupted):
        jksong.add_guide(song, g, c, _interrupt=point)
    assert song.read_bytes() == original
    left = list(song.parent.glob("*.tmp-*"))
    assert len(left) == 1                                       # a crash leaves its temp file
    assert jksong.list_song_files(song.parent) == [song]        # which is never listed
    jksong.add_guide(song, g, c)                                # done again: complete
    assert jksong.Song(song).guide() is not None


def test_settings_saved_during_the_build_are_kept(song: Path):
    g, c = make_guide(song)
    prep = jksong.prepare_guide_rewrite(song, g, c)
    jksong.update_user_state(song, lambda s: s["settings"].update(semitones=5))
    before = fingerprints(song)
    jksong.finish_guide_rewrite(prep)
    assert fingerprints(song)[1:] == before[1:]
    assert jksong.Song(song).effective_settings()["semitones"] == 5


def test_a_damaged_chunk_stops_the_rewrite(song: Path):
    s = jksong.Song(song)
    e = s.chunk_entry("stem.drums")
    b = bytearray(song.read_bytes())
    b[e["offset"] + e["length"] // 2] ^= 0xFF
    song.write_bytes(bytes(b))
    g, c = make_guide(song)
    with pytest.raises(jksong.Damaged):
        jksong.add_guide(song, g, c)
    assert song.read_bytes() == bytes(b) and not list(song.parent.glob("*.tmp-*"))


# ----------------------------------------------------------------------------- practice

def result(overall=87.4321, rows=2):
    return {"date": "2026-11-20T19:05:00Z", "key": -3, "key_changes": 0, "seeks": 0, "octave": -1,
            "octave_mode": "auto", "strictness": "standard", "overall": overall, "grade": "B+",
            "graded": 212, "total": 240, "tendency_cents": -12.3456, "sung_share": 0.98123, "voiced_share": 0.9149,
            "completed": True, "trust": [], "guide": "g", "scorer": "s", "app": "a",
            "lines": {"basis": "lyrics", "rows": [[i, 12.3456 + i, 91.6, 8, -4.6] for i in range(rows)]}}


def test_practice_precision_best_and_recent():
    r = jksong.round_practice_result(result())
    assert (r["overall"], r["sung_share"], r["voiced_share"], r["tendency_cents"]) == (87.4, 0.98, 0.91, -12)
    assert r["lines"]["rows"][1] == [1, 13.3, 92, 8, -5]
    assert json.dumps(r["lines"]["rows"][0]) == "[0, 12.3, 92, 8, -5]"   # whole numbers as integers
    p = {"recent": []}
    for i in range(25):
        jksong.practice_add_result(p, result(50.0 + i))
    assert len(p["recent"]) == 20 and p["recent"][0]["overall"] == 74.0 and p["best"]["overall"] == 74.0
    short = result(99.0)
    short["sung_share"] = 0.5
    jksong.practice_add_result(p, short)
    assert p["best"]["overall"] == 74.0
    while jksong.practice_shrink_once(p):
        pass
    assert p["recent"] == [] and p["best"]["overall"] == 74.0


def test_save_practice_never_rewrites_the_file(song: Path):
    size = song.stat().st_size
    big = lambda i: {**result(50.0 + i, 0), "date": f"2026-11-20T19:{i:02d}:00Z",
                     "lines": {"basis": "lyrics", "rows": [[k, k * 0.731 + i, (k * 37 + i) % 101, k % 9,
                                                            (k * 13 + i) % 200 - 100] for k in range(3000)]}}
    for i in range(20):
        jksong.save_practice(song, lambda p, i=i: jksong.practice_add_result(p, big(i)))
    assert song.stat().st_size == size
    p = jksong.Song(song).practice()
    assert p["recent"][0]["date"] == "2026-11-20T19:19:00Z" and p["best"] is not None
    assert len(p["recent"][0]["lines"]["rows"]) == 3000
    assert any(not r["lines"]["rows"] for r in p["recent"])     # the oldest rows were dropped
    assert jksong.validate_practice(p) == []
    assert jksong.Song(song).effective_settings()["semitones"] == -2


# ----------------------------------------------------------------------------- make --guide

def _built_cli() -> Path | None:
    """The jukeoke-cli this checkout built, if any ($JUKEOKE_CLI or a known target folder)."""
    import os
    cands = [os.environ.get("JUKEOKE_CLI", "")]
    build = Path(__file__).resolve().parents[2].parent / "jukeoke-build"
    for t in ("target-wo2-stage2", "target"):
        for prof in ("release", "debug"):
            cands.append(str(build / t / prof / "jukeoke-cli.exe"))
            cands.append(str(build / t / prof / "jukeoke-cli"))
    for c in cands:
        if c and Path(c).is_file():
            return Path(c)
    return None


def test_make_guide_off_and_require_without_cli(song: Path, monkeypatch):
    from jukeoke_studio.__main__ import add_guide_with_cli, find_jukeoke_cli
    monkeypatch.delenv("JUKEOKE_CLI", raising=False)
    monkeypatch.setenv("PATH", "")
    assert add_guide_with_cli(song, "off", None, None) == (None, 0)
    if find_jukeoke_cli(None, None) is None:
        line, code = add_guide_with_cli(song, "auto", None, None)
        assert code == 0 and line["error_code"] == "cli_not_found"
        line, code = add_guide_with_cli(song, "require", None, None)
        assert code == 3
    assert jksong.Song(song).guide() is None                    # the song is untouched


def test_make_guide_runs_jukeoke_cli(song: Path):
    from jukeoke_studio.__main__ import add_guide_with_cli
    cli = _built_cli()
    if cli is None:
        pytest.skip("no jukeoke-cli built in this checkout")
    before = fingerprints(song)
    line, code = add_guide_with_cli(song, "require", str(cli), None)
    assert code == 0 and line["ok"] and line["state"] == "ready", line
    s = jksong.Song(song)
    g = s.guide()
    assert jksong.validate_guide(g, s.manifest["duration_seconds"]) == ([], [])
    assert g["notes"], "the 440 Hz lead tone gives notes"
    assert all(abs(n["midi"] - 69.0) < 0.2 for n in g["notes"] if n["kind"] == "pitched")
    after = fingerprints(song)
    assert no_guide(after[0]) == before[0] and after[1:] == before[1:]

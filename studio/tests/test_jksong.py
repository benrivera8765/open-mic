# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for the Python .jksong writer/reader (studio/jukeoke_studio/jksong.py).

Run:  $JK_BUILD/studio-venv/Scripts/python.exe -m pytest studio/tests/test_jksong.py
Needs numpy, soundfile, pytest. No GPU, no network, all audio generated.
"""

from __future__ import annotations

import hashlib
import io
import json
import struct
import sys
import zlib
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from jukeoke_studio import jksong  # noqa: E402

SR = 44100


def tones(frames: int = SR, channels: int = 2, seed: int = 0) -> list[jksong.StemInput]:
    """Four generated stems; 'other' peaks above full scale to exercise the gain rule."""
    t = np.arange(frames) / SR
    rng = np.random.default_rng(seed)

    def st(x, wide=0.0):
        return np.stack([x * (1 + wide), x * (1 - wide)])[:channels].astype(np.float32)

    lead = 0.4 * np.sin(2 * np.pi * 440 * t) * (t % 0.5 < 0.3)
    back = 0.2 * np.sin(2 * np.pi * 660 * t + 0.3)
    drums = 0.6 * np.exp(-(t % 0.25) * 30) * np.sin(2 * np.pi * 60 * t) + 0.01 * rng.standard_normal(frames)
    other = 1.3 * np.sin(2 * np.pi * 110 * t) + 0.1 * np.sin(2 * np.pi * 330 * t)
    return [jksong.StemInput("lead_vocal", st(lead)), jksong.StemInput("backing_vocals", st(back, 0.2)),
            jksong.StemInput("drums", st(drums)), jksong.StemInput("other", st(other, 0.05))]


def lyrics() -> dict:
    return {
        "lyrics_version": 1, "language": "en", "source": "user_text", "checked": False,
        "lines": [{"start": 0.1, "end": 0.9, "x_future": "kept", "words": [
            {"start": 0.1, "end": 0.4, "chars": [["c", 0.1, 0.15], ["a", 0.15, 0.2], ["f", 0.2, 0.3], ["é", 0.3, 0.4]]},
            {"start": 0.5, "end": 0.9, "chars": [["n", 0.5, 0.55], ["a", 0.55, 0.6], ["ï", 0.6, 0.7],
                                                  ["v", 0.7, 0.8], ["e", 0.8, 0.9]]}]}],
    }


@pytest.fixture
def song_path(tmp_path: Path) -> Path:
    p = tmp_path / "Artist - Title.jksong"
    jksong.write_song(p, stems=tones(), sample_rate=SR, title="Title", artist="Artist", lyrics=lyrics(),
                      stems_sum_to_source=True, extra_chunks=[("x.future", "application/octet-stream", b"\x00opaque\xff")],
                      manifest_extra={"x_manifest": {"a": 1}})
    return p


# ------------------------------------------------------------------ quantisation and FLAC

def test_gain_rule_keeps_unity_when_no_clip():
    x = np.array([[0.5, -0.25, 1e-7]], dtype=np.float32)
    q, gain, peak = jksong.quantize_stem(x)
    assert gain == 1.0 and peak == pytest.approx(0.5)
    assert q[:, 0].tolist() == [4194304, -2097152, 1]  # round(1e-7 * 2^23) = 1


def test_gain_rule_scales_clipping_stems_to_099():
    x = np.array([[0.0, 1.5, -1.2]], dtype=np.float32)
    q, gain, peak = jksong.quantize_stem(x)
    assert peak == pytest.approx(1.5)
    assert gain == pytest.approx(1.5 / 0.99)
    assert int(np.abs(q).max()) == round(0.99 * 8388608)
    # Reconstruction within half an LSB at the stored gain.
    back = q[:, 0] / 8388608 * gain
    assert np.max(np.abs(back - x[0].astype(np.float64))) <= 0.5 / 8388608 * gain + 1e-12


def test_full_scale_edge_cases():
    # +1.0 is not representable in 24 bits (max 8388607), so it is scaled; -1.0 alone is
    # also treated as a clip (the rule looks at |peak|), which costs nothing audible.
    for v in (1.0, -1.0):
        _, gain, _ = jksong.quantize_stem(np.array([[v]], dtype=np.float32))
        assert gain > 1.0
    # Just below full scale stays unscaled.
    _, gain, _ = jksong.quantize_stem(np.array([[8388607 / 8388608]], dtype=np.float32))
    assert gain == 1.0
    with pytest.raises(ValueError):
        jksong.quantize_stem(np.array([[0.0, np.nan]], dtype=np.float32))
    with pytest.raises(ValueError):
        jksong.quantize_stem(np.zeros(4, dtype=np.float32))


def test_never_below_24_bits():
    # Every 24-bit code in a range survives encode/decode exactly (no 16-bit path).
    codes = np.arange(-3000, 3000, dtype=np.int32)
    x = (codes / 8388608).astype(np.float32)[None, :]
    q, gain, _ = jksong.quantize_stem(x)
    assert gain == 1.0 and np.array_equal(q[:, 0], codes)
    data = jksong.encode_flac24(q, SR)
    back, _ = sf.read(io.BytesIO(data), dtype="int32", always_2d=True)
    assert np.array_equal(back[:, 0] >> 8, codes)
    assert np.all((back & 0xFF) == 0)


def _md5_24(samples: np.ndarray) -> bytes:
    le = samples.astype("<i4").view(np.uint8).reshape(-1, 4)[:, :3]
    return hashlib.md5(le.tobytes()).digest()


def test_flac_streaminfo_md5_and_seektable():
    rng = np.random.default_rng(3)
    frames = SR * 7 + 123
    x = (0.3 * rng.standard_normal((2, frames))).clip(-0.99, 0.99).astype(np.float32)  # noise: many false sync codes
    q, _, _ = jksong.quantize_stem(x)
    data = jksong.encode_flac24(q, SR)
    blocks, first = jksong._metadata_blocks(data)
    assert [b[0] for b in blocks][:2] == [0, 3], "STREAMINFO then SEEKTABLE"
    si = jksong.parse_streaminfo(blocks[0][1])
    assert si["total_samples"] == frames and si["bits_per_sample"] == 24 and si["channels"] == 2
    assert si["md5"] == _md5_24(q)
    pts = [struct.unpack(">QQH", blocks[1][1][i:i + 18]) for i in range(0, len(blocks[1][1]), 18)]
    samples = [p[0] for p in pts]
    assert samples[0] == 0 and samples == sorted(set(samples))
    gaps = np.diff(samples + [frames])
    assert gaps.max() <= SR + 4096, "a seek point at least every second (+ one frame)"
    frames_found = jksong.flac_frame_offsets(data)
    starts = {off: (s, bs) for off, s, bs in frames_found}
    for s, off, bs in pts:  # every point lands on a real frame header
        assert starts[off] == (s, bs)
        assert data[first + off] == 0xFF and data[first + off + 1] & 0xFE == 0xF8
    # Decodes to exactly the input; libsndfile seeking agrees with the linear decode.
    back, _ = sf.read(io.BytesIO(data), dtype="int32", always_2d=True)
    assert np.array_equal(back >> 8, q)
    with sf.SoundFile(io.BytesIO(data)) as f:
        for pos in (0, 4095, 4096, 100_000, frames - 10):
            f.seek(pos)
            got = f.read(10, dtype="int32", always_2d=True) >> 8
            assert np.array_equal(got, q[pos:pos + 10])


def test_seektable_replaces_existing_one():
    q, _, _ = jksong.quantize_stem(np.zeros((1, SR * 3), dtype=np.float32))
    once = jksong.encode_flac24(q, SR)
    twice = jksong.add_seektable(once, SR // 2)
    assert [b[0] for b in jksong._metadata_blocks(twice)[0]].count(3) == 1
    back, _ = sf.read(io.BytesIO(twice), dtype="int32")
    assert np.array_equal(back, np.zeros(SR * 3, dtype=np.int32))


# ------------------------------------------------------------------ container round trip

def test_round_trip(song_path: Path):
    s = jksong.Song(song_path)
    m = s.manifest
    assert m["format"] == "jksong" and m["format_version"] == "1.1"
    assert m["duration_frames"] == SR and m["sample_rate"] == SR and m["channels"] == 2
    assert m["x_manifest"] == {"a": 1}
    assert [x["role"] for x in m["stems"]] == ["lead_vocal", "backing_vocals", "drums", "other"]
    assert [x["in_karaoke_mix"] for x in m["stems"]] == [False, True, True, True]
    assert [x["pitched"] for x in m["stems"]] == [True, True, False, True]
    other = s.stem("other")
    assert other["gain"] > 1.0 and other["peak"] > 1.0
    assert all(x["gain"] == 1.0 for x in m["stems"][:3])
    # Layout: header, slots, 4096-aligned chunks with the manifest first, TOC last.
    h = s.header
    assert (h["slot_size"], h["slot_a_offset"], h["slot_b_offset"]) == (262144, 4096, 4096 + 262144)
    assert s.chunk_ids()[0] == "manifest"
    assert all(c["offset"] % 4096 == 0 for c in s.toc["chunks"])
    assert h["toc_offset"] % 4096 == 0 and h["toc_offset"] + h["toc_length"] == h["file_length"] == song_path.stat().st_size
    # Slots: A sequence 1, B empty.
    assert s.slots[0][0] == "valid" and s.slots[0][1] == 1 and s.slots[1][0] == "empty"
    assert s.active_slot == 0
    assert s.effective_settings()["semitones"] == 0
    # Lyrics as written, grapheme strings intact.
    assert s.lyrics() == lyrics()
    assert s.read_chunk("x.future") == b"\x00opaque\xff"
    # Stems decode to the input within half an LSB at the stored gain.
    for st in tones():
        got = s.read_stem(st.role)
        gain = s.stem(st.role)["gain"]
        err = np.max(np.abs(got.astype(np.float64) - st.audio.astype(np.float64)))
        assert err <= 0.5 / 8388608 * gain + 2 ** -24, (st.role, err)
    # No temp files left.
    assert [p.name for p in song_path.parent.iterdir()] == [song_path.name]


def test_stems_sum_back_to_source(tmp_path: Path):
    st = tones(frames=SR * 2, seed=5)
    src32 = np.sum([x.audio.astype(np.float64) for x in st], axis=0).astype(np.float32)
    # Like the Studio's "full" preset: other = source - lead - backing - drums, in float32.
    other = src32 - st[0].audio - st[1].audio - st[2].audio
    st[3] = jksong.StemInput("other", other)
    p = tmp_path / "s.jksong"
    jksong.write_song(p, stems=st, sample_rate=SR, title="t", artist="a", stems_sum_to_source=True)
    s = jksong.Song(p)
    total = sum(s.read_stem(x.role).astype(np.float64) for x in st)
    err = float(np.max(np.abs(total - src32.astype(np.float64))))
    # Half an LSB per stem at its gain, plus float32 rounding of the inputs (5 values
    # of magnitude < 2, half an ulp = 2^-24 each).
    bound = sum(0.5 / 8388608 * x["gain"] for x in s.manifest["stems"]) + 5 * 2 ** -24
    assert err <= bound, (err, bound)


def test_mono_and_cover(tmp_path: Path):
    png = (b"\x89PNG\r\n\x1a\n" + b"\0" * 20)
    p = tmp_path / "m.jksong"
    jksong.write_song(p, stems=tones(frames=5000, channels=1), sample_rate=48000, title="m", artist="a",
                      cover=png, analysis={"key": {"tonic": "D", "mode": "major", "name": "D major", "confidence": 0.7},
                                           "tempo_bpm": 100.0})
    s = jksong.Song(p)
    assert s.manifest["channels"] == 1 and s.manifest["cover"] == "cover"
    assert s.chunk_entry("cover")["type"] == "image/png" and s.read_chunk("cover") == png
    assert s.read_stem("drums").shape == (1, 5000)
    with pytest.raises(ValueError):
        jksong.write_song(tmp_path / "bad.jksong", stems=tones(frames=100), sample_rate=SR, title="", artist="",
                          cover=b"GIF89a")
    assert not (tmp_path / "bad.jksong").exists()


def test_invalid_lyrics_are_refused(tmp_path: Path):
    bad = lyrics()
    bad["lines"][0]["words"][0]["chars"][1][1] = 0.12  # starts before the previous char ends (0.15)
    with pytest.raises(ValueError, match="overlaps"):
        jksong.write_song(tmp_path / "x.jksong", stems=tones(), sample_rate=SR, title="", artist="", lyrics=bad)
    assert list(tmp_path.iterdir()) == []


def test_failed_write_leaves_old_file_and_no_temp(tmp_path: Path, monkeypatch):
    p = tmp_path / "s.jksong"
    jksong.write_song(p, stems=tones(frames=2000), sample_rate=SR, title="old", artist="a")
    before = p.read_bytes()

    def boom(*a, **k):
        raise OSError("stick pulled")

    monkeypatch.setattr(jksong, "_encode_header", boom)
    with pytest.raises(OSError):
        jksong.write_song(p, stems=tones(frames=2000), sample_rate=SR, title="new", artist="a")
    assert p.read_bytes() == before
    assert [x.name for x in tmp_path.iterdir()] == ["s.jksong"]


# ------------------------------------------------------------------ corruption

def _err(tmp_path: Path, data: bytes) -> type:
    p = tmp_path / "c.jksong"
    p.write_bytes(data)
    with pytest.raises(jksong.JkSongError) as e:
        jksong.Song(p)
    return type(e.value)


def _fix_header_crc(b: bytearray) -> None:
    struct.pack_into("<I", b, 124, zlib.crc32(bytes(b[:124])) & 0xFFFFFFFF)


def test_corruption_cases(song_path: Path, tmp_path: Path):
    good = song_path.read_bytes()
    h = jksong.Song(song_path).header
    b = bytearray(good)
    b[1] ^= 0x20
    assert _err(tmp_path, bytes(b)) is jksong.NotASongFile
    assert _err(tmp_path, good.replace(b"\r\n", b"\n", 1)) is jksong.NotASongFile
    for i in (8, 20, 40, 60, 100, 126):
        b = bytearray(good)
        b[i] ^= 0x01
        assert _err(tmp_path, bytes(b)) is jksong.Damaged, i
    for cut in (0, 5, 100, 4096, len(good) // 2, len(good) - 1):
        assert _err(tmp_path, good[:cut]) is jksong.Truncated, cut
    b = bytearray(good)
    b[h["toc_offset"] + 3] ^= 0x01
    assert _err(tmp_path, bytes(b)) is jksong.Damaged
    # Chunk out of bounds, with valid TOC and header CRCs.
    toc = json.loads(good[h["toc_offset"]:])
    toc["chunks"][-1]["offset"] = len(good) + 4096
    t = json.dumps(toc).encode()
    b = bytearray(good[:h["toc_offset"]] + t)
    struct.pack_into("<QI", b, 24, len(t), zlib.crc32(t) & 0xFFFFFFFF)
    struct.pack_into("<Q", b, 56, len(b))
    _fix_header_crc(b)
    assert _err(tmp_path, bytes(b)) is jksong.Damaged
    # Newer major refused; newer minor opens.
    b = bytearray(good)
    struct.pack_into("<H", b, 8, 2)
    _fix_header_crc(b)
    assert _err(tmp_path, bytes(b)) is jksong.NewerVersion
    b = bytearray(good)
    struct.pack_into("<H", b, 10, 9)
    b[70:74] = b"NEW!"
    _fix_header_crc(b)
    p = tmp_path / "minor.jksong"
    p.write_bytes(bytes(b))
    assert jksong.Song(p).header["minor"] == 9


# ------------------------------------------------------------------ slots

def test_slot_alternation_and_unknown_keys(song_path: Path):
    def edit(n):
        def f(st):
            st["settings"]["semitones"] = n
            st.setdefault("x_unknown", []).append(n)
            st["settings"]["x_new"] = {"n": n}
        return f

    expected_slots = [1, 0, 1, 0]
    for i, want in enumerate(expected_slots):
        slot, seq = jksong.update_user_state(song_path, edit(i - 1))
        assert (slot, seq) == (want, i + 2)
        s = jksong.Song(song_path)
        assert s.active_slot == want
        assert s.effective_settings()["semitones"] == i - 1
        assert s.user_state["x_unknown"] == list(range(-1, i))
        assert s.user_state["settings"]["x_new"] == {"n": i - 1}


def test_torn_slot_write_falls_back(song_path: Path):
    jksong.update_user_state(song_path, lambda st: st["settings"].update(semitones=3))  # B, seq 2
    s = jksong.Song(song_path)
    h = s.header
    img = jksong.encode_slot(3, jksong._json_bytes({"settings": {"semitones": -7}}), h["slot_size"])
    plen = struct.unpack_from("<I", img, 16)[0]
    pristine = song_path.read_bytes()
    for name, patch in [("header only", img[:32]), ("half payload", img[:32 + plen // 2]),
                        ("garbage", b"\xa5" * 3000)]:
        b = bytearray(pristine)
        b[h["slot_a_offset"]:h["slot_a_offset"] + len(patch)] = patch
        song_path.write_bytes(bytes(b))
        s = jksong.Song(song_path)
        assert s.active_slot == 1, name
        assert s.effective_settings()["semitones"] == 3, name
        slot, seq = jksong.update_user_state(song_path, lambda st: st["settings"].update(guide_vocal=0.5))
        assert (slot, seq) == (0, 3), name
        s = jksong.Song(song_path)
        assert s.effective_settings()["semitones"] == 3 and s.effective_settings()["guide_vocal"] == 0.5


def test_large_payload_zlib_and_too_large(song_path: Path):
    big_lyrics = {"lyrics_version": 1, "lines": [lyrics()["lines"][0]] * 1}
    big = {"lines_blob": ["la " * 50] * 6000}  # ~900 KB of very compressible JSON
    slot, seq = jksong.update_user_state(song_path, lambda st: st.update(x_big=big, lyrics_override=big_lyrics))
    s = jksong.Song(song_path)
    assert s.slots[slot][0] == "valid"
    off = s.header["slot_a_offset"] if slot == 0 else s.header["slot_b_offset"]
    raw = song_path.read_bytes()[off:off + 32]
    assert struct.unpack_from("<I", raw, 20)[0] == 1, "zlib encoding used"
    assert s.user_state["x_big"] == big
    assert s.effective_lyrics()["lines"][0]["words"][0]["chars"][3][0] == "é"
    rng = np.random.default_rng(0)
    noise = rng.integers(0, 2 ** 63, 40000).astype(str).tolist()  # incompressible
    before = song_path.read_bytes()
    with pytest.raises(jksong.SlotPayloadTooLarge):
        jksong.update_user_state(song_path, lambda st: st.update(x_noise=noise))
    assert song_path.read_bytes() == before


def test_effective_metadata_override(song_path: Path):
    jksong.update_user_state(song_path, lambda st: st.update(metadata_override={"artist": "Fixed", "album": None}))
    s = jksong.Song(song_path)
    assert s.effective_metadata() == {"title": "Title", "artist": "Fixed", "album": None}


def test_library_listing(tmp_path: Path):
    for n in ["b.jksong", "A.JKSONG", "._b.jksong", ".x.jksong", "b.jksong.tmp-00ff", "x.flac"]:
        (tmp_path / n).write_bytes(b"x")
    (tmp_path / "dir.jksong").mkdir()
    assert [p.name for p in jksong.list_song_files(tmp_path)] == ["A.JKSONG", "b.jksong"]


def test_exact_residual_mode(tmp_path: Path):
    st = tones(frames=SR * 2, seed=9)
    src = np.sum([x.audio.astype(np.float64) for x in st], axis=0).astype(np.float32)
    p = tmp_path / "r.jksong"
    stems = [jksong.StemInput(x.role, None if x.role == "other" else x.audio) for x in st]
    m = jksong.write_song(p, stems=stems, source=src, residual_role="other", sample_rate=SR,
                          title="t", artist="a", stems_sum_to_source=True)
    assert [x["role"] for x in m["stems"]] == ["lead_vocal", "backing_vocals", "drums", "other"]
    s = jksong.Song(p)
    g = {x["role"]: x["gain"] for x in s.manifest["stems"]}
    assert g["other"] > 1.0  # the residual (~1.4 peak) still gets the gain rule
    total = sum(s.read_stem_int(r).T.astype(np.float64) * (g[r] / 8388608) for r in g)
    err = float(np.max(np.abs(total - src.astype(np.float64))))
    assert err <= 0.5 / 8388608 * g["other"] * (1 + 1e-9), err
    with pytest.raises(ValueError):
        jksong.write_song(tmp_path / "x.jksong", stems=stems[:3], source=src, sample_rate=SR, title="", artist="")

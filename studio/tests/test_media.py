# SPDX-License-Identifier: GPL-3.0-or-later
"""Decoding every supported container (generated with PyAV's encoders), exactness for
lossless sources, tags, covers and the multichannel fold-down (CPU, no models)."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import media_fixtures as MF  # noqa: E402

from jukeoke_studio import media  # noqa: E402
from jukeoke_studio.errors import DecodeFailed, Hooks, NotFound, Cancelled  # noqa: E402

# kind -> (container, codec, lossless bits or None, has_video)
KINDS = {
    "wav16": ("wav", "pcm_s16le", 16, False), "wav24": ("wav", "pcm_s24le", 24, False),
    "flac24": ("flac", "flac", 24, False), "aiff": ("aiff", "pcm_s16be", 16, False),
    "mp3": ("mp3", "mp3", None, False), "m4a": ("m4a", "aac", None, False),
    "ogg": ("ogg", "vorbis", None, False), "opus": ("opus", "opus", None, False),
    "wma": ("wma", "wmav2", None, False), "mp4": ("mp4", "aac", None, True),
    "mkv": ("mkv", "flac", 24, True), "mov": ("mov", "aac", None, True),
    "webm": ("webm", "opus", None, True), "avi": ("avi", "pcm_s16le", 16, True),
}


@pytest.mark.parametrize("kind", sorted(KINDS))
def test_decode_every_container(tmp_path, kind):
    container, codec, bits, video = KINDS[kind]
    p = MF.make(kind, tmp_path, seconds=1.0)
    d = media.decode(p)
    i = d.info
    assert (i.codec, i.has_video, i.bits_per_sample, i.lossy) == (codec, video, bits, bits is None)
    if kind != "opus":  # Ogg Opus: FFmpeg names the demuxer "ogg"
        assert i.container == container
    assert d.audio.dtype == np.float32 and d.audio.shape[0] == 2
    rate = 48000 if codec == "opus" else 44100
    assert d.sample_rate == rate  # native rate, never converted
    assert abs(d.audio.shape[1] - rate) < 0.03 * rate  # ~1 s (codec padding aside)
    if bits:
        # Lossless: the decoded float32 is exactly the integer samples / 2^(bits-1).
        x = MF.signal(1.0)
        full = 2.0 ** (bits - 1)
        q = np.round(np.clip(x, -1, (full - 1) / full) * full) / full
        assert np.array_equal(d.audio, q.astype(np.float32))


def test_decode_errors_and_cancel(tmp_path):
    with pytest.raises(NotFound):
        media.decode(tmp_path / "missing.wav")
    junk = tmp_path / "junk.mp3"
    junk.write_bytes(b"\x00" * 1000)
    with pytest.raises(DecodeFailed) as e:
        media.decode(junk)
    assert e.value.code == "decode_failed" and e.value.message
    silent_video = tmp_path / "v.mp4"
    import av

    with av.open(str(silent_video), "w") as c:  # a video without any audio stream
        st, frames = MF._add_video(c, "libx264", 0.5)
        for fr in frames:
            for pkt in st.encode(fr):
                c.mux(pkt)
        for pkt in st.encode():
            c.mux(pkt)
    with pytest.raises(DecodeFailed, match="This video has no sound track"):
        media.decode(silent_video)
    # Random bytes (the end-to-end run's corrupt file): FFmpeg's probe read them as an
    # "LRC lyrics" subtitle file; the user must hear "damaged", not "no sound track".
    noise = tmp_path / "corrupt.m4a"
    noise.write_bytes(np.random.default_rng(20261117).bytes(2_000_000))
    with pytest.raises(DecodeFailed, match="could not be read as audio or video") as e:
        media.decode(noise)
    assert "damaged" in e.value.message
    h = Hooks()
    h.cancel()
    with pytest.raises(Cancelled):
        media.decode(MF.make("wav16", tmp_path, seconds=1.0), h)


def test_truncated_file_keeps_what_decodes(tmp_path):
    p = MF.make("mp3", tmp_path, seconds=3.0)
    data = p.read_bytes()
    cut = tmp_path / "cut.mp3"
    cut.write_bytes(data[: len(data) // 2])
    d = media.decode(cut)
    assert 1.0 < d.audio.shape[1] / d.sample_rate < 2.0


def test_multichannel_is_folded_to_stereo(tmp_path):
    p = MF.make("flac24", tmp_path, seconds=0.5, channels=6)
    d = media.decode(p)
    assert d.audio.shape[0] == 2 and d.downmix["from_channels"] == 6
    assert d.downmix["method"].startswith("ITU-R BS.775") and d.warnings
    x = np.arange(12, dtype=np.float32).reshape(6, 2)
    y, info = media.downmix_to_stereo(x, ["FL", "FR", "FC", "LFE", "BL", "BR"])
    c = 0.7071067811865476
    np.testing.assert_allclose(y[0], x[0] + c * x[2] + c * x[4], rtol=1e-6)
    np.testing.assert_allclose(y[1], x[1] + c * x[2] + c * x[5], rtol=1e-6)


def test_mono_stays_mono(tmp_path):
    d = media.decode(MF.make("wav16", tmp_path, seconds=0.5, channels=1))
    assert d.audio.shape[0] == 1 and d.downmix is None


def test_tags_and_covers(tmp_path):
    p = MF.mp3_with_lyrics(tmp_path, title="Café ☕", artist="Ünïcode")
    i = media.probe(p)
    assert (i.title, i.artist) == ("Café ☕", "Ünïcode")
    data, warn = media.cover_for_song(p)
    assert data[:3] == b"\xff\xd8\xff" and warn is None
    png = MF.png_bytes()
    f = MF.flac_with_tags(tmp_path, cover=png, TITLE="T", ARTIST="A", ALBUM="Al")
    assert media.cover_for_song(f)[0] == png  # kept byte for byte
    i = media.probe(f)
    assert (i.title, i.artist, i.album) == ("T", "A", "Al")
    assert media.cover_for_song(MF.make("wav16", tmp_path, seconds=0.2))[0] is None


def test_unusual_cover_is_converted_to_jpeg(tmp_path):
    import io

    import av

    img = np.zeros((40, 60, 3), np.uint8)
    img[..., 0] = 200
    out = io.BytesIO()
    with av.open(out, "w", format="image2pipe") as c:  # a BMP cover
        st = c.add_stream("bmp", rate=1)
        st.width, st.height, st.pix_fmt = 60, 40, "bgr24"
        for pkt in st.encode(av.VideoFrame.from_ndarray(img, format="rgb24").reformat(format="bgr24")):
            c.mux(pkt)
    bmp = out.getvalue()
    assert bmp[:2] == b"BM"
    f = MF.flac_with_tags(tmp_path, cover=None)
    from mutagen.flac import FLAC, Picture

    ff = FLAC(str(f))
    pic = Picture()
    pic.type, pic.mime, pic.data = 3, "image/bmp", bmp
    ff.add_picture(pic)
    ff.save()
    data, warn = media.cover_for_song(f)
    assert data is not None and data[:3] == b"\xff\xd8\xff" and warn is None
    path = media.extract_cover(f, tmp_path / "covers")
    assert Path(path).read_bytes() == data and path.endswith(".jpg")


def test_user_cover_small_jpeg_and_png_are_kept_byte_for_byte(tmp_path):
    """protocol 7 metadata.cover_path: a JPEG / PNG up to 2 MB goes in unchanged (and a
    small one is never enlarged)."""
    for name, data in (("c.jpg", MF.jpeg_bytes(24, 16)), ("c.png", MF.png_bytes(10, 6))):
        p = tmp_path / name
        p.write_bytes(data)
        assert media.user_cover(p) == data
        assert media.is_jpeg_or_png_file(p)


@pytest.mark.parametrize("w,h,same_size", [(1000, 800, True), (1800, 1800, True), (2600, 2600, False)])
def test_user_cover_over_2mb_is_reencoded_to_jpeg_quality_first(tmp_path, w, h, same_size):
    """Over 2 MB (SONG_FORMAT 4.1): re-encoded once to JPEG <= 2 MB, lowering the quality
    first; the pixel size only drops when even the lowest quality does not fit. Never
    larger than the original."""
    big = MF.noise_png_bytes(w, h)
    assert len(big) > media.MAX_COVER_BYTES
    p = tmp_path / "big.png"
    p.write_bytes(big)
    out = media.user_cover(p)
    assert out[:3] == b"\xff\xd8\xff" and len(out) <= media.MAX_COVER_BYTES
    w2, h2 = MF.image_size(out)
    assert w2 <= w and h2 <= h
    assert ((w2, h2) == (w, h)) is same_size
    if not same_size:  # shrunk only as far as needed: the first size step that fits
        assert max(w2, h2) == 2000


def test_user_cover_refuses_other_files_with_plain_messages(tmp_path):
    with pytest.raises(ValueError, match="could not be found"):
        media.user_cover(tmp_path / "missing.png")
    with pytest.raises(ValueError, match="could not be found"):
        media.user_cover(tmp_path)  # a folder
    gif = tmp_path / "c.gif"
    gif.write_bytes(b"GIF89a" + bytes(64))
    with pytest.raises(ValueError, match="JPEG or PNG"):
        media.user_cover(gif)
    assert not media.is_jpeg_or_png_file(gif) and not media.is_jpeg_or_png_file(tmp_path / "missing.png")
    broken = tmp_path / "broken.jpg"
    broken.write_bytes(b"\xff\xd8\xff\xe0" + bytes(100))
    with pytest.raises(ValueError, match="damaged"):
        media.user_cover(broken)


def test_inspect_has_the_protocol_fields(tmp_path):
    r = media.inspect(MF.make("m4a", tmp_path, seconds=0.5), tmp_path / "covers")
    for k in ("ok", "title", "artist", "album", "duration_seconds", "container", "codec", "sample_rate",
              "channels", "bits_per_sample", "lossy", "has_video", "embedded_lyrics", "cover_path"):
        assert k in r
    assert r["title"] == "test-m4a" and r["title_from_tags"] is False
    assert r["embedded_lyrics"] == {"present": False, "synced": False, "text": None}


def test_concurrent_inspects_write_the_cover_through_unique_temporary_files(tmp_path, monkeypatch):
    """Inspects run on their own threads; tracks of one album share a cover. Every writer
    gets its own temporary name, and each inspect returns the finished cover (D-171)."""
    import os
    import threading
    import time

    src = MF.flac_with_tags(tmp_path, cover=MF.png_bytes(), TITLE="Album track")
    out = tmp_path / "covers"
    tmp_names: list[str] = []
    real_replace = os.replace

    def slow_replace(a, b, *args, **kw):
        if ".tmp-" in str(a):
            tmp_names.append(os.path.basename(a))
            time.sleep(0.01)  # widen the window in which writers overlap
        return real_replace(a, b, *args, **kw)

    monkeypatch.setattr(media.os, "replace", slow_replace)
    for _ in range(10):
        for p in out.glob("cover-*"):
            p.unlink()
        barrier = threading.Barrier(8)
        results, errors = [], []

        def run():
            barrier.wait()
            try:
                results.append(media.extract_cover(src, out))
            except Exception as e:  # noqa: BLE001
                errors.append(e)

        threads = [threading.Thread(target=run) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(30)
        assert errors == [] and len(results) == 8 and len(set(results)) == 1
        assert Path(results[0]).read_bytes() == MF.png_bytes()
        assert [p.name for p in out.iterdir()] == [Path(results[0]).name]  # no temporary file left
    assert tmp_names and len(tmp_names) == len(set(tmp_names))

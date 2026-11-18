# SPDX-License-Identifier: GPL-3.0-or-later
"""Generated audio / video test files (PyAV encoders), made in a temporary folder.

Nothing here is committed audio: every file is synthesised (tones and noise) at test
time. ``make(kind, folder)`` returns the path of a file of that kind.
"""

from __future__ import annotations

import io
from fractions import Fraction
from pathlib import Path

import numpy as np

SR = 44100


def signal(seconds: float = 1.0, sr: int = SR, channels: int = 2, seed: int = 0) -> np.ndarray:
    """A deterministic test signal, float32 [channels, frames], peak about 0.5."""
    rng = np.random.default_rng(seed)
    t = np.arange(int(sr * seconds)) / sr
    base = 0.3 * np.sin(2 * np.pi * 440 * t) + 0.1 * np.sin(2 * np.pi * 97 * t)
    chans = [base * (1.0 - 0.1 * c) + 0.02 * rng.standard_normal(len(t)) for c in range(channels)]
    return np.stack(chans).astype(np.float32)


def _encode_audio(container, codec: str, x: np.ndarray, sr: int, fmt: str, layout: str,
                  bitrate: int | None = None, options: dict | None = None):
    import av

    st = container.add_stream(codec, rate=sr)
    st.layout = layout
    if fmt:
        st.format = fmt
    if bitrate:
        st.bit_rate = bitrate
    if options:
        st.codec_context.options = options
    frame_size = st.codec_context.frame_size or 1024
    planar = fmt.endswith("p")
    if fmt.startswith("s16"):
        data = np.round(np.clip(x, -1, 32767 / 32768) * 32768).astype(np.int16)
    elif fmt.startswith("s32"):
        data = (np.round(np.clip(x, -1, 8388607 / 8388608) * 8388608).astype(np.int32) << 8)
    else:
        data = x.astype(np.float32)
    pts = 0
    for i in range(0, data.shape[1], frame_size):
        part = np.ascontiguousarray(data[:, i:i + frame_size])
        arr = part if planar else part.T.reshape(1, -1)
        fr = av.AudioFrame.from_ndarray(np.ascontiguousarray(arr), format=fmt, layout=layout)
        fr.sample_rate = sr
        fr.pts = pts
        fr.time_base = Fraction(1, sr)
        pts += part.shape[1]
        for pkt in st.encode(fr):
            container.mux(pkt)
    for pkt in st.encode():
        container.mux(pkt)
    return st


def _add_video(container, codec: str, seconds: float, pix_fmt: str = "yuv420p"):
    import av

    st = container.add_stream(codec, rate=10)
    st.width, st.height, st.pix_fmt = 64, 48, pix_fmt
    frames = []
    for i in range(int(seconds * 10)):
        img = np.full((48, 64, 3), (i * 20) % 255, np.uint8)
        frames.append(av.VideoFrame.from_ndarray(img, format="rgb24").reformat(format=pix_fmt))
    return st, frames


def make(kind: str, folder: str | Path, seconds: float = 1.0, sr: int = SR, channels: int = 2,
         name: str | None = None) -> Path:
    """kinds: wav16 wav24 flac24 mp3 m4a ogg opus aiff wma mp4 mkv mov webm avi."""
    import av

    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    x = signal(seconds, sr, channels)
    layout = {1: "mono", 2: "stereo", 6: "5.1"}[channels]
    ext = {"wav16": "wav", "wav24": "wav", "flac24": "flac"}.get(kind, kind)
    path = folder / (name or f"test-{kind}.{ext}")
    audio_spec = {
        "wav16": ("pcm_s16le", "s16", None), "wav24": ("pcm_s24le", "s32", None),
        "flac24": ("flac", "s32", None), "mp3": ("libmp3lame", "fltp", 192000),
        "m4a": ("aac", "fltp", 192000), "ogg": ("libvorbis" if "libvorbis" in av.codecs_available else "vorbis",
                                                  "fltp", None),
        "opus": ("libopus", "flt", 128000), "aiff": ("pcm_s16be", "s16", None),
        "wma": ("wmav2", "fltp", 128000),
        "mp4": ("aac", "fltp", 192000), "mov": ("aac", "fltp", 192000), "mkv": ("flac", "s32", None),
        "webm": ("libopus", "flt", 128000), "avi": ("pcm_s16le", "s16", None),
    }[kind]
    video_codec = {"mp4": "libx264", "mov": "libx264", "mkv": "libx264", "webm": "libvpx-vp9",
                   "avi": "mpeg4"}.get(kind)
    fmt_name = {"wma": "asf", "m4a": "ipod", "opus": "ogg", "mkv": "matroska", "aiff": "aiff"}.get(kind, ext)
    with av.open(str(path), "w", format=fmt_name) as c:
        vst = vframes = None
        if video_codec:
            vst, vframes = _add_video(c, video_codec, seconds)
        codec, sfmt, br = audio_spec
        if kind == "ogg" and codec == "vorbis":
            sfmt = "fltp"
        opts = {"strict": "-2"} if codec == "vorbis" else None
        xa, ra = x, sr
        if codec == "libopus" and sr != 48000:  # Opus only runs at 48 kHz
            xa, ra = _resample48(x, sr), 48000
        _encode_audio(c, codec, xa, ra, sfmt, layout, br, opts)
        if vst is not None:
            for fr in vframes:
                for pkt in vst.encode(fr):
                    c.mux(pkt)
            for pkt in vst.encode():
                c.mux(pkt)
    return path


def _resample48(x: np.ndarray, sr: int) -> np.ndarray:
    import soxr

    return soxr.resample(x.T, sr, 48000).T.astype(np.float32)


def jpeg_bytes(w: int = 32, h: int = 32) -> bytes:
    """A tiny generated JPEG (a colour gradient)."""
    import av

    img = np.zeros((h, w, 3), np.uint8)
    img[..., 0] = np.linspace(0, 255, w, dtype=np.uint8)[None, :]
    img[..., 2] = np.linspace(255, 0, h, dtype=np.uint8)[:, None]
    out = io.BytesIO()
    with av.open(out, "w", format="mjpeg") as c:
        st = c.add_stream("mjpeg", rate=1)
        st.width, st.height, st.pix_fmt = w, h, "yuvj420p"
        fr = av.VideoFrame.from_ndarray(img, format="rgb24").reformat(format="yuvj420p")
        for pkt in st.encode(fr):
            c.mux(pkt)
        for pkt in st.encode():
            c.mux(pkt)
    return out.getvalue()


def png_bytes(w: int = 16, h: int = 16) -> bytes:
    import av

    img = np.zeros((h, w, 3), np.uint8)
    img[..., 1] = 200
    out = io.BytesIO()
    with av.open(out, "w", format="image2pipe") as c:
        st = c.add_stream("png", rate=1)
        st.width, st.height, st.pix_fmt = w, h, "rgb24"
        for pkt in st.encode(av.VideoFrame.from_ndarray(img, format="rgb24")):
            c.mux(pkt)
        for pkt in st.encode():
            c.mux(pkt)
    return out.getvalue()


def noise_png_bytes(w: int, h: int, seed: int = 0) -> bytes:
    """A PNG of random RGB noise: about 3 bytes per pixel, so large ones exceed 2 MB."""
    import av

    img = np.random.default_rng(seed).integers(0, 256, (h, w, 3), dtype=np.uint8)
    out = io.BytesIO()
    with av.open(out, "w", format="image2pipe") as c:
        st = c.add_stream("png", rate=1)
        st.width, st.height, st.pix_fmt = w, h, "rgb24"
        for pkt in st.encode(av.VideoFrame.from_ndarray(img, format="rgb24")):
            c.mux(pkt)
        for pkt in st.encode():
            c.mux(pkt)
    return out.getvalue()


def image_size(data: bytes) -> tuple[int, int]:
    """(width, height) of a picture, decoded with FFmpeg."""
    import av

    with av.open(io.BytesIO(data)) as c:
        f = next(c.decode(video=0))
    return f.width, f.height


def mp3_with_lyrics(folder: str | Path, seconds: float = 1.0, synced: bool = True,
                    title: str = "Test Title", artist: str = "Test Artist") -> Path:
    """An MP3 with ID3 title/artist, USLT plain lyrics, optionally SYLT, and an APIC cover."""
    from mutagen.id3 import APIC, ID3, SYLT, TIT2, TPE1, USLT

    p = make("mp3", folder, seconds, name="lyrics.mp3")
    tags = ID3()
    tags.add(TIT2(encoding=3, text=title))
    tags.add(TPE1(encoding=3, text=artist))
    tags.add(USLT(encoding=3, lang="eng", desc="", text="First line here\nSecond line there"))
    if synced:
        tags.add(SYLT(encoding=3, lang="eng", format=2, type=1, desc="",
                      text=[("First line here", 100), ("Second line there", 600)]))
    tags.add(APIC(encoding=3, mime="image/jpeg", type=3, desc="cover", data=jpeg_bytes()))
    tags.save(str(p))
    return p


def flac_with_tags(folder: str | Path, seconds: float = 1.0, cover: bytes | None = None,
                   lyrics: str | None = "Line one\nLine two", **tags) -> Path:
    from mutagen.flac import FLAC, Picture

    p = make("flac24", folder, seconds, name="tagged.flac")
    f = FLAC(str(p))
    for k, v in tags.items():
        f[k] = v
    if lyrics:
        f["LYRICS"] = lyrics
    if cover:
        pic = Picture()
        pic.type, pic.mime, pic.data = 3, "image/png" if cover[:4] == b"\x89PNG" else "image/jpeg", cover
        f.add_picture(pic)
    f.save()
    return p

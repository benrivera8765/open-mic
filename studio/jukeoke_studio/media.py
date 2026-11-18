# SPDX-License-Identifier: GPL-3.0-or-later
"""Reading the user's file: decode to float32 at the native rate, tags, cover, facts.

Pipeline step 1 (WORKING ORDER 4.1) and the protocol's ``inspect`` (section 5).

* **Decoding** uses PyAV (FFmpeg, bundled in the ``av`` wheel; BSD-3 wrapper, LGPL/GPL
  FFmpeg build). Audio files (mp3 m4a flac wav ogg opus aiff wma ...) and video files
  (mp4 mkv mov webm avi ...) alike: the first audio stream is decoded, the video is
  never decoded. Samples are converted to float32 *here*, by explicit scaling of the
  decoder's own output (``int / 2^(bits-1)``), never through FFmpeg's resampler, so
  there is no rate conversion, no dither and no filtering: a 16- or 24-bit source maps
  to float32 exactly. Lossy decoders already output float.
* More than two channels (a 5.1 film soundtrack) are folded to stereo with the
  ITU-R BS.775 coefficients, because the song format holds 1 or 2 channels; this is
  reported as a warning and in the song's provenance (``downmix``). Mono stays mono.
* Tags: container + audio-stream metadata (FFmpeg reads ID3, Vorbis comments, MP4
  atoms, ASF, RIFF INFO, Matroska tags), mutagen as a fallback.
* Cover: an attached picture (ID3 APIC, MP4 covr, FLAC PICTURE, Matroska attachment)
  through PyAV, then mutagen. JPEG and PNG up to 2 MB are kept byte for byte; anything
  else (BMP, WebP, a 5 MB scan) is re-encoded once to JPEG at most 1400 px, because the
  song format allows JPEG/PNG up to 2 MB (docs/SONG_FORMAT.md 4.1). A cover is artwork,
  not audio; the audio rules do not apply to it.
"""

from __future__ import annotations

import hashlib
import io
import logging
import os
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from .errors import DecodeFailed, Hooks, NO_HOOKS, NotFound

log = logging.getLogger(__name__)

AUDIO_EXTENSIONS = (".mp3", ".m4a", ".aac", ".flac", ".wav", ".ogg", ".oga", ".opus", ".aiff",
                    ".aif", ".aifc", ".wma", ".wv", ".ape", ".alac", ".caf", ".mka")
VIDEO_EXTENSIONS = (".mp4", ".m4v", ".mkv", ".mov", ".webm", ".avi", ".wmv", ".mpg", ".mpeg", ".ts")

#: Codecs that store the waveform exactly. Everything else counts as lossy.
_LOSSLESS = {"flac", "alac", "wavpack", "ape", "tta", "truehd", "mlp", "wmalossless", "tak",
             "shorten", "als", "s302m"}

MAX_COVER_BYTES = 2 * 1024 * 1024
COVER_MAX_PX = 1400

# ITU-R BS.775 stereo fold-down, by FFmpeg channel name.
_C = 0.7071067811865476
_DOWNMIX = {
    "FL": (1.0, 0.0), "FR": (0.0, 1.0), "FC": (_C, _C), "LFE": (0.0, 0.0),
    "BL": (_C, 0.0), "BR": (0.0, _C), "SL": (_C, 0.0), "SR": (0.0, _C),
    "BC": (0.5, 0.5), "FLC": (_C, 0.0), "FRC": (0.0, _C), "TC": (0.5, 0.5),
}


#: FFmpeg decoder names -> the codec's name ("mp3float" is FFmpeg's float MP3 decoder).
_CODEC_ALIASES = {"mp3float": "mp3", "mp2float": "mp2", "mp1float": "mp1", "ac3_fixed": "ac3",
                  "libopus": "opus", "libvorbis": "vorbis"}


def is_lossy(codec: str) -> bool:
    c = (codec or "").lower()
    return not (c.startswith("pcm_") or c in _LOSSLESS)


@dataclass
class MediaInfo:
    """What ``inspect`` reports (protocol section 5) plus what the pipeline needs."""

    path: str
    container: str
    codec: str
    sample_rate: int
    channels: int
    bits_per_sample: int | None
    lossy: bool
    has_video: bool
    duration_seconds: float | None
    title: str | None
    artist: str | None
    album: str | None
    tags: dict = field(default_factory=dict)
    audio_stream_index: int = 0
    channel_names: list[str] = field(default_factory=list)


@dataclass
class Decoded:
    audio: np.ndarray        # float32 [channels, frames], channels 1 or 2, native rate
    sample_rate: int
    info: MediaInfo
    downmix: dict | None = None   # set when > 2 channels were folded to stereo
    warnings: list[str] = field(default_factory=list)


def _container_name(c, path: str) -> str:
    """A short container name: the file's own extension when FFmpeg's demuxer name is a
    list ("mov,mp4,m4a,3gp,3g2,mj2", "matroska,webm")."""
    name = (c.format.name or "").split(",")
    ext = Path(path).suffix.lower().lstrip(".")
    if ext and ext in name:
        return ext
    if ext in ("aif", "aifc") and "aiff" in name:
        return "aiff"
    if ext in ("mkv", "mka") and "matroska" in name:
        return ext
    if ext == "wma" and "asf" in name:
        return "wma"
    return name[0] if name and name[0] else ext


def _meta(dicts: list[dict], *keys: str) -> str | None:
    for k in keys:
        for d in dicts:
            for dk, dv in d.items():
                if str(dk).lower() == k and str(dv).strip():
                    return str(dv).strip().replace("\x00", "")
    return None


def _bits(cc, fmt_name: str | None, codec: str) -> int | None:
    if is_lossy(codec):
        return None
    b = getattr(cc, "bits_per_raw_sample", 0) or 0
    if b:
        return int(b)
    f = (fmt_name or "").rstrip("p")
    return {"u8": 8, "s16": 16, "s32": 32, "flt": 32, "dbl": 64, "s64": 64}.get(f)


def _open(path: str):
    import av

    if not os.path.isfile(path):
        raise NotFound("That file could not be found.", path)
    try:
        return av.open(path)
    except Exception as e:  # noqa: BLE001 - every FFmpeg error means "not readable"
        raise DecodeFailed("This file could not be read as audio.", f"{type(e).__name__}: {e}") from e


def probe(path: str | os.PathLike) -> MediaInfo:
    """Facts about the file without decoding the audio."""
    path = str(path)
    with _open(path) as c:
        return _probe_open(c, path)


def _probe_open(c, path: str) -> MediaInfo:
    import av

    audio = list(c.streams.audio)
    if not audio:
        # A video without sound says so. Anything else is most likely damaged or not a
        # media file at all: FFmpeg's probe accepts almost any bytes as *some* format
        # (seeded random bytes were read as an "LRC lyrics" subtitle file in the
        # end-to-end run), so "no sound track" alone would mislead the user.
        if list(c.streams.video):
            raise DecodeFailed("This video has no sound track.", f"no audio stream in {path}")
        raise DecodeFailed("This file could not be read as audio or video. It may be damaged, or it is "
                           "not a music or video file.",
                           f"no audio stream in {path} (FFmpeg read it as {c.format.name!r})")
    s = audio[0]
    cc = s.codec_context
    attached = av.stream.Disposition.attached_pic
    has_video = any(not (v.disposition & attached) for v in c.streams.video)
    duration = None
    if s.duration is not None and s.time_base is not None:
        duration = float(s.duration * s.time_base)
    elif c.duration is not None:
        duration = c.duration / 1_000_000.0
    metas = [dict(c.metadata), dict(s.metadata)]
    codec = _CODEC_ALIASES.get(cc.name, cc.name) or ""
    layout = getattr(cc, "layout", None)
    names = []
    if layout is not None:
        try:
            names = [ch.name for ch in layout.channels]
        except Exception:  # noqa: BLE001
            names = []
    title = _meta(metas, "title")
    artist = _meta(metas, "artist", "album_artist", "author", "performer", "composer")
    album = _meta(metas, "album")
    if not (title and artist):
        mt, ma, mal = _mutagen_tags(path)
        title, artist, album = title or mt, artist or ma, album or mal
    return MediaInfo(
        path=path, container=_container_name(c, path), codec=codec,
        sample_rate=int(cc.sample_rate or 0), channels=int(cc.channels or len(names) or 0),
        bits_per_sample=_bits(cc, cc.format.name if cc.format else None, codec),
        lossy=is_lossy(codec), has_video=has_video, duration_seconds=duration,
        title=title, artist=artist, album=album,
        tags={str(k): str(v) for m in metas for k, v in m.items()},
        audio_stream_index=s.index, channel_names=names)


def _mutagen_tags(path: str) -> tuple[str | None, str | None, str | None]:
    try:
        import mutagen

        f = mutagen.File(path, easy=True)
        if f is None or f.tags is None:
            return None, None, None

        def one(k):
            v = f.tags.get(k)
            if isinstance(v, (list, tuple)):
                v = v[0] if v else None
            return str(v).strip() if v else None

        return one("title"), one("artist") or one("albumartist"), one("album")
    except Exception:  # noqa: BLE001 - tags are optional
        return None, None, None


# ------------------------------------------------------------------------------ decode

def _frame_to_float(frame) -> np.ndarray:
    """One decoded frame -> float32 [channels, samples], by explicit exact scaling."""
    fmt = frame.format.name
    arr = frame.to_ndarray()
    ch = len(frame.layout.channels)
    if not frame.format.is_planar:
        arr = arr.reshape(-1, ch).T
    base = fmt.rstrip("p")
    if base == "flt":
        return np.asarray(arr, np.float32)
    if base == "dbl":
        return arr.astype(np.float32)
    if base == "s16":
        return arr.astype(np.float32) * np.float32(1.0 / 32768.0)
    if base == "s32":
        # 24-bit decoders deliver s32 with the low byte zero: exact in float32. True
        # 32-bit sources keep float32's 24-bit precision (WORKING ORDER 5: float32).
        return (arr.astype(np.float64) * (1.0 / 2147483648.0)).astype(np.float32)
    if base == "u8":
        return (arr.astype(np.float32) - 128.0) * np.float32(1.0 / 128.0)
    if base == "s64":
        return (arr.astype(np.float64) * (1.0 / 9223372036854775808.0)).astype(np.float32)
    raise DecodeFailed("This file uses a sample format the Studio cannot read.", f"sample format {fmt}")


def downmix_to_stereo(x: np.ndarray, names: list[str]) -> tuple[np.ndarray, dict]:
    """ITU-R BS.775 fold-down of ``x`` [channels, frames]; returns (stereo, description)."""
    ch = x.shape[0]
    if len(names) != ch:
        names = ["FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR"][:ch]
    m = np.zeros((2, ch))
    for i, n in enumerate(names):
        m[:, i] = _DOWNMIX.get(n, (0.5, 0.5))
    out = (m @ x.astype(np.float64)).astype(np.float32)
    return out, {"from_channels": ch, "channel_names": names, "method": "ITU-R BS.775 fold-down",
                 "matrix": [[round(v, 6) for v in row] for row in m.tolist()]}


def decode(path: str | os.PathLike, hooks: Hooks = NO_HOOKS) -> Decoded:
    """The first audio stream as float32 ``[channels, frames]`` at its own rate."""
    import av

    path = str(path)
    with _open(path) as c:
        info = _probe_open(c, path)
        stream = next(s for s in c.streams.audio if s.index == info.audio_stream_index)
        # Only the audio stream is decoded (PyAV tells FFmpeg to discard the others).
        chunks: list[np.ndarray] = []
        rate = None
        nch = None
        n = 0
        dur = info.duration_seconds or 0.0
        try:
            for i, frame in enumerate(c.decode(stream)):
                if rate is None:
                    rate, nch = int(frame.sample_rate), len(frame.layout.channels)
                    if not info.channel_names:
                        info.channel_names = [ch.name for ch in frame.layout.channels]
                elif int(frame.sample_rate) != rate or len(frame.layout.channels) != nch:
                    raise DecodeFailed(
                        "The sound in this file changes its format part-way through, which the Studio "
                        "cannot handle.", f"{rate} Hz/{nch} ch -> {frame.sample_rate} Hz/"
                                         f"{len(frame.layout.channels)} ch at frame {n}")
                x = _frame_to_float(frame)
                chunks.append(x)
                n += x.shape[1]
                if i % 64 == 0:
                    hooks.progress(min(0.99, n / rate / dur) if dur else 0.5)
        except (DecodeFailed, KeyboardInterrupt):
            raise
        except Exception as e:  # noqa: BLE001
            if hooks.cancelled:
                hooks.check()
            if not chunks:
                raise DecodeFailed("This file could not be read as audio.", f"{type(e).__name__}: {e}") from e
            # A damaged tail (truncated download): keep what decoded, say so.
            log.warning("decoding stopped early in %s: %s", path, e)
            info.tags["_decode_error"] = f"{type(e).__name__}: {e}"
    if not chunks or n == 0:
        raise DecodeFailed("This file contains no sound.", f"0 samples decoded from {path}")
    audio = np.concatenate(chunks, axis=1)
    del chunks
    warnings: list[str] = []
    if "_decode_error" in info.tags:
        warnings.append("The end of the file seems damaged; the Studio used the part that could be read "
                        f"({n / rate:.1f} s).")
    if not np.all(np.isfinite(audio)):
        raise DecodeFailed("This file could not be read as audio.", "decoder returned NaN/Inf")
    info.sample_rate, info.channels = rate, nch
    downmix = None
    if nch > 2:
        audio, downmix = downmix_to_stereo(audio, info.channel_names)
        warnings.append(f"The file has {nch} audio channels; they were folded down to stereo "
                        "(ITU-R BS.775), because a song holds one or two channels.")
    return Decoded(np.ascontiguousarray(audio, dtype=np.float32), rate, info, downmix, warnings)


def sha256_file(path: str | os.PathLike, hooks: Hooks = NO_HOOKS, chunk: int = 1 << 22) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            hooks.check()
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


# ------------------------------------------------------------------------------- cover

def _image_kind(data: bytes) -> str | None:
    if data.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    return None


def raw_cover(path: str | os.PathLike) -> bytes | None:
    """The embedded picture's bytes as stored (any image format), or None."""
    path = str(path)
    try:
        import av

        with av.open(path) as c:
            attached = av.stream.Disposition.attached_pic
            pics = [s for s in c.streams.video if s.disposition & attached]
            for s in pics:
                for pkt in c.demux(s):
                    data = bytes(pkt)
                    if data:
                        return data
                    break
    except Exception as e:  # noqa: BLE001 - a cover is optional
        log.info("no cover through PyAV for %s: %s", path, e)
    try:
        import mutagen

        f = mutagen.File(path)
        if f is None:
            return None
        pics = getattr(f, "pictures", None)  # FLAC
        if pics:
            front = [p for p in pics if getattr(p, "type", 3) == 3] or list(pics)
            return bytes(front[0].data)
        tags = f.tags
        if tags is None:
            return None
        if hasattr(tags, "getall"):  # ID3
            apic = tags.getall("APIC")
            if apic:
                front = [p for p in apic if getattr(p, "type", 3) == 3] or apic
                return bytes(front[0].data)
        if "covr" in tags:  # MP4
            return bytes(tags["covr"][0])
        for k in ("metadata_block_picture",):  # Ogg Vorbis / Opus
            vals = tags.get(k) if hasattr(tags, "get") else None
            if vals:
                import base64

                from mutagen.flac import Picture

                return bytes(Picture(base64.b64decode(vals[0])).data)
    except Exception as e:  # noqa: BLE001
        log.info("no cover through mutagen for %s: %s", path, e)
    return None


def _to_jpeg(data: bytes) -> bytes | None:
    """Re-encodes an image (any format FFmpeg reads) as JPEG, at most COVER_MAX_PX."""
    import av

    try:
        with av.open(io.BytesIO(data), "r") as c:
            frame = next(c.decode(video=0))
        w, h = frame.width, frame.height
        scale = min(1.0, COVER_MAX_PX / max(w, h))
        w2, h2 = max(2, int(w * scale) // 2 * 2), max(2, int(h * scale) // 2 * 2)
        for q in (3, 5, 8, 12):
            out = io.BytesIO()
            with av.open(out, "w", format="mjpeg") as oc:
                st = oc.add_stream("mjpeg", rate=1)
                st.width, st.height, st.pix_fmt = w2, h2, "yuvj420p"
                st.codec_context.qscale = q
                st.codec_context.options = {"qmin": str(q), "qmax": str(q)}
                f2 = frame.reformat(width=w2, height=h2, format="yuvj420p")
                for pkt in st.encode(f2):
                    oc.mux(pkt)
                for pkt in st.encode():
                    oc.mux(pkt)
            b = out.getvalue()
            if _image_kind(b) == "jpeg" and len(b) <= MAX_COVER_BYTES:
                return b
    except Exception as e:  # noqa: BLE001
        log.info("cover could not be converted to JPEG: %s", e)
    return None


def cover_for_song(path: str | os.PathLike) -> tuple[bytes | None, str | None]:
    """(JPEG/PNG bytes <= 2 MB or None, a warning or None)."""
    data = raw_cover(path)
    if not data:
        return None, None
    if _image_kind(data) and len(data) <= MAX_COVER_BYTES:
        return data, None
    j = _to_jpeg(data)
    if j is None:
        return None, "The cover picture in the file could not be read, so the song has no cover."
    return j, None


def extract_cover(path: str | os.PathLike, out_dir: str | os.PathLike) -> str | None:
    """Writes the song-ready cover into ``out_dir`` (``cover-<hash>.jpg|png``) and returns
    its path, or None. Used by ``inspect`` so the app can show it."""
    data, _ = cover_for_song(path)
    if not data:
        return None
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    ext = "png" if _image_kind(data) == "png" else "jpg"
    p = out_dir / f"cover-{hashlib.sha256(data).hexdigest()[:16]}.{ext}"
    if not p.exists():
        tmp = p.with_name(p.name + f".tmp-{os.getpid()}")
        tmp.write_bytes(data)
        os.replace(tmp, p)
    return str(p)


def inspect(path: str | os.PathLike, temp_dir: str | os.PathLike, language: str | None = "en") -> dict:
    """The protocol's ``inspect.result`` body (without type/id)."""
    from .lyrics import embedded_lyrics_info

    info = probe(path)
    try:
        cover = extract_cover(path, temp_dir)
    except Exception as e:  # noqa: BLE001
        log.info("cover extraction failed: %s", e)
        cover = None
    return {
        "ok": True,
        "title": info.title or Path(path).stem,
        "artist": info.artist,
        "album": info.album,
        "duration_seconds": round(info.duration_seconds, 3) if info.duration_seconds else None,
        "container": info.container,
        "codec": info.codec,
        "sample_rate": info.sample_rate,
        "channels": info.channels,
        "bits_per_sample": info.bits_per_sample,
        "lossy": info.lossy,
        "has_video": info.has_video,
        "embedded_lyrics": embedded_lyrics_info(path, language),
        "cover_path": cover,
        "title_from_tags": info.title is not None,
    }

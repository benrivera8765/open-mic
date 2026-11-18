# SPDX-License-Identifier: GPL-3.0-or-later
"""Reader and writer for the ``.jksong`` song file, version 1.0 (docs/SONG_FORMAT.md).

This is the Python reference implementation, used by the Studio to write songs. It is
independent of the Rust one (``crates/jk-format``); the two are tested against each
other by ``tests/conformance``.

Dependencies: numpy and soundfile (libsndfile with libFLAC). Nothing else from the
Studio is imported, so the module can also be used outside the Studio package.

Audio rules (SONG_FORMAT 4.2, WORK_ORDER section 5):

* Stems come in as float32 ``[channels, frames]`` at the song's native rate.
* Each stem is stored as 24-bit FLAC. Quantisation is done here in float64 with
  round-to-nearest, never below 24 bits, and the integers are handed to libsndfile,
  so the rounding rule is explicit and does not depend on the libsndfile version.
  (Measured with libsndfile 1.2.2: its own float -> 24-bit FLAC path also maps
  k / 2^23 to k exactly for 200,000 random codes and clips +1.0 to 8388607; the
  integer path used here is an exact ``>> 8``.)
* Optional exact-residual mode for the "full" preset: ``write_song(source=...)``
  computes the residual stem from the source minus the *decoded* other stems, so the
  stems sum back to the source within half an LSB (see :func:`write_song`).
* Gain / headroom: a stem whose float peak would clip in 24 bits (peak above
  8388607/8388608) is scaled down so its peak is 0.99, and ``gain = 1/scale`` is
  stored. Otherwise gain is exactly 1.0 and the samples are stored unscaled.
  The true sample is ``decoded_integer / 8388608 * gain``.
* STREAMINFO carries the total sample count and the MD5 (libFLAC writes both).
  libsndfile does not write a SEEKTABLE, so this module adds one (a point every
  second) by locating the frame boundaries in the encoded stream; see
  :func:`add_seektable`.

The container is written atomically: ``<name>.tmp-<random>`` in the destination
folder, ``fsync``, then ``os.replace`` over the destination.
"""

from __future__ import annotations

import io
import json
import os
import secrets
import struct
import zlib
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable

import numpy as np
import soundfile as sf

__all__ = [
    "MAGIC", "FORMAT_MAJOR", "FORMAT_MINOR", "FORMAT_VERSION", "DEFAULT_SLOT_SIZE",
    "JkSongError", "NotASongFile", "NewerVersion", "Truncated", "Damaged", "MissingChunk",
    "SlotPayloadTooLarge", "StemInput", "quantize_stem", "encode_flac24", "add_seektable",
    "write_song", "write_container", "Song", "open_song", "update_user_state",
    "default_user_state", "validate_lyrics", "list_song_files", "is_song_file_name",
]

# ----------------------------------------------------------------------------- constants

MAGIC = b"\x89JKS\r\n\x1a\n"
FORMAT_MAJOR = 1
FORMAT_MINOR = 0
FORMAT_VERSION = "1.0"
HEADER_LENGTH = 128
ALIGN = 4096
DEFAULT_SLOT_SIZE = 262_144
SLOT_MAGIC = b"JKSL"
SLOT_VERSION = 1
SLOT_HEADER_LEN = 32
ENC_JSON = 0
ENC_ZLIB = 1

FULL_SCALE = 8_388_608          # 2**23: the true float is integer / FULL_SCALE * gain
INT24_MAX = 8_388_607
INT24_MIN = -8_388_608
HEADROOM_PEAK = 0.99            # peak after scaling when a stem would clip
MAX_COVER_BYTES = 2 * 1024 * 1024
SEEK_INTERVAL_SECONDS = 1.0     # spec recommends at least one point every 10 s
# libFLAC compression level 0..8 (soundfile takes it as 0..1; libsndfile: level = round(x*8)).
# Measured (2026-11-17, this PC): 60 s of 24-bit stereo noise+tone encodes in 0.51 s at
# level 8 vs 0.17 s at level 5, same size; the four tonal stems of
# tests/conformance/synth.py (4 s) are 489 KB at level 8 vs 540 KB at level 5 (-9.4%).
# Encoding time is negligible next to separation and USB space is not: use level 8.
FLAC_COMPRESSION_LEVEL = 8

KNOWN_ROLES = ("lead_vocal", "backing_vocals", "drums", "other")
TONICS = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B")


# ----------------------------------------------------------------------------- errors

class JkSongError(Exception):
    """Base class. ``kind`` matches the Rust ``ErrorKind`` names in snake case."""
    kind = "error"
    user_message = "The song file could not be read."


class NotASongFile(JkSongError):
    kind = "not_a_song_file"
    user_message = "This is not a Jukeoke song file (or it was damaged when it was copied as text)."


class NewerVersion(JkSongError):
    kind = "newer_version"
    user_message = "This song was made by a newer version of Jukeoke; please update."


class Truncated(JkSongError):
    kind = "truncated"
    user_message = "This song file is incomplete, probably an unfinished copy. Copy it again from the original."


class Damaged(JkSongError):
    kind = "damaged"
    user_message = "This song file is damaged. Copy it again from the original, or make it again in the Studio."


class MissingChunk(Damaged):
    kind = "missing_chunk"


class SlotPayloadTooLarge(JkSongError):
    kind = "slot_payload_too_large"
    user_message = "The song's saved settings are too large for a quick save; the file needs a full rewrite."


# ----------------------------------------------------------------------------- helpers

def _crc32(data: bytes) -> int:
    return zlib.crc32(data) & 0xFFFFFFFF


def _json_bytes(obj: Any) -> bytes:
    """Compact UTF-8 JSON, no BOM, no NaN/Infinity (not valid JSON)."""
    return json.dumps(obj, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _pad_to(f, align: int = ALIGN) -> None:
    pad = (-f.tell()) % align
    if pad:
        f.write(b"\0" * pad)


# ----------------------------------------------------------------------------- stems

@dataclass
class StemInput:
    """One stem to write: float32 ``[channels, frames]`` at the song's rate."""
    role: str
    audio: np.ndarray | None  # None only for the residual stem when write_song gets `source`
    in_karaoke_mix: bool | None = None   # default: every role except lead_vocal
    pitched: bool | None = None          # default: every role except drums
    extra: dict = field(default_factory=dict)  # extra manifest keys for this stem


def quantize_stem(audio: np.ndarray) -> tuple[np.ndarray, float, float]:
    """Applies the gain/headroom rule and quantises to 24-bit integers.

    ``audio``: float ``[channels, frames]``. Returns ``(int32 [frames, channels] with
    values in -8388608..8388607, gain, peak)``. Raises ``ValueError`` on NaN/Inf.
    """
    x = np.asarray(audio)
    if x.ndim != 2:
        raise ValueError("stem audio must be [channels, frames]")
    if not np.issubdtype(x.dtype, np.floating):
        raise ValueError("stem audio must be floating point")
    x = x.astype(np.float64, copy=False)       # exact widening of float32
    if x.size and not np.all(np.isfinite(x)):
        raise ValueError("stem audio contains NaN or infinity")
    peak = float(np.max(np.abs(x))) if x.size else 0.0
    if peak * FULL_SCALE > INT24_MAX:
        # Would clip: scale so the peak sits at 0.99 and store the inverse as the gain.
        scale = HEADROOM_PEAK / peak
        gain = 1.0 / scale
    else:
        scale = 1.0
        gain = 1.0
    q = np.rint(x * (scale * FULL_SCALE))
    # Safety net only: with the rule above nothing reaches the limits.
    np.clip(q, INT24_MIN, INT24_MAX, out=q)
    return np.ascontiguousarray(q.T.astype(np.int32)), gain, peak


def _blocksize_from_code(code: int, data: bytes, pos: int) -> tuple[int, int] | None:
    """Block size for a frame-header code; returns (size, bytes consumed) or None."""
    if code == 0:
        return None
    if code == 1:
        return 192, 0
    if 2 <= code <= 5:
        return 576 << (code - 2), 0
    if code == 6:
        return (data[pos] + 1, 1) if pos < len(data) else None
    if code == 7:
        return (int.from_bytes(data[pos:pos + 2], "big") + 1, 2) if pos + 2 <= len(data) else None
    return 256 << (code - 8), 0


def _crc8(data: bytes) -> int:
    crc = 0
    for b in data:
        crc = _CRC8_TABLE[crc ^ b]
    return crc


def _make_crc8_table() -> list[int]:
    t = []
    for i in range(256):
        c = i
        for _ in range(8):
            c = ((c << 1) ^ 0x07) & 0xFF if c & 0x80 else (c << 1) & 0xFF
        t.append(c)
    return t


_CRC8_TABLE = _make_crc8_table()


def _parse_frame_header(data: bytes, pos: int, bits_code_ok: tuple[int, ...]) -> tuple[int, int, bool] | None:
    """Parses a FLAC frame header at ``pos``. Returns (coded number, block size,
    variable_blocking) when the header is well formed and its CRC-8 matches, else None."""
    if pos + 6 > len(data) or data[pos] != 0xFF or (data[pos + 1] & 0xFE) != 0xF8:
        return None
    variable = bool(data[pos + 1] & 1)
    bs_code = data[pos + 2] >> 4
    sr_code = data[pos + 2] & 0x0F
    ch = data[pos + 3] >> 4
    ss = (data[pos + 3] >> 1) & 7
    if sr_code == 15 or ch > 10 or (data[pos + 3] & 1) or ss not in bits_code_ok:
        return None
    # UTF-8-like coded number.
    p = pos + 4
    b0 = data[p]
    if b0 < 0x80:
        n, extra = b0, 0
    elif b0 >= 0xC0 and b0 != 0xFF:
        extra = 0
        mask = 0x40
        while b0 & mask:
            extra += 1
            mask >>= 1
        if extra > 6:
            return None
        n = b0 & (mask - 1)
        if p + 1 + extra > len(data):
            return None
        for k in range(1, extra + 1):
            c = data[p + k]
            if (c & 0xC0) != 0x80:
                return None
            n = (n << 6) | (c & 0x3F)
    else:
        return None
    p += 1 + extra
    r = _blocksize_from_code(bs_code, data, p)
    if r is None:
        return None
    blocksize, used = r
    p += used
    if sr_code == 12:
        p += 1
    elif sr_code in (13, 14):
        p += 2
    if p >= len(data) or _crc8(data[pos:p]) != data[p]:
        return None
    return n, blocksize, variable


def _metadata_blocks(flac: bytes) -> tuple[list[tuple[int, bytes]], int]:
    """Splits a FLAC stream into its metadata blocks [(type, body)] and the offset of
    the first audio frame."""
    if flac[:4] != b"fLaC":
        raise ValueError("not a FLAC stream")
    pos = 4
    blocks = []
    while True:
        if pos + 4 > len(flac):
            raise ValueError("FLAC metadata truncated")
        hdr = flac[pos]
        length = int.from_bytes(flac[pos + 1:pos + 4], "big")
        blocks.append((hdr & 0x7F, flac[pos + 4:pos + 4 + length]))
        pos += 4 + length
        if hdr & 0x80:
            return blocks, pos


def parse_streaminfo(body: bytes) -> dict:
    packed = int.from_bytes(body[10:18], "big")
    return {
        "min_block": int.from_bytes(body[0:2], "big"),
        "max_block": int.from_bytes(body[2:4], "big"),
        "min_frame": int.from_bytes(body[4:7], "big"),
        "max_frame": int.from_bytes(body[7:10], "big"),
        "sample_rate": packed >> 44,
        "channels": ((packed >> 41) & 7) + 1,
        "bits_per_sample": ((packed >> 36) & 31) + 1,
        "total_samples": packed & 0xFFFFFFFFF,
        "md5": body[18:34],
    }


def flac_frame_offsets(flac: bytes) -> list[tuple[int, int, int]]:
    """Locates every audio frame: returns [(byte offset from the first frame, first
    sample, block size)].

    A frame starts with a sync code that can also occur by chance inside audio data,
    so a candidate is accepted only when its header is well formed, its CRC-8 matches,
    its bit depth matches STREAMINFO, and its coded frame/sample number is exactly the
    next one expected. The scan must account for every sample in STREAMINFO, else
    ``ValueError``.
    """
    blocks, first = _metadata_blocks(flac)
    if not blocks or blocks[0][0] != 0:
        raise ValueError("STREAMINFO is not the first metadata block")
    si = parse_streaminfo(blocks[0][1])
    total = si["total_samples"]
    bits_code = {8: 1, 12: 2, 16: 4, 20: 5, 24: 6, 32: 7}.get(si["bits_per_sample"])
    ok_codes = (0,) if bits_code is None else (0, bits_code)
    arr = np.frombuffer(flac, dtype=np.uint8, offset=first)
    cands = np.flatnonzero((arr[:-1] == 0xFF) & ((arr[1:] & 0xFE) == 0xF8)) + first
    min_len = max(si["min_frame"], 6)
    frames: list[tuple[int, int, int]] = []
    sample = 0
    index = 0
    next_min = first
    for c in cands.tolist():
        if c < next_min:
            continue
        h = _parse_frame_header(flac, c, ok_codes)
        if h is None:
            continue
        n, bs, variable = h
        if (variable and n != sample) or (not variable and n != index):
            continue
        frames.append((c - first, sample, bs))
        sample += bs
        index += 1
        next_min = c + min_len
        if total and sample >= total:
            break
    if frames and frames[0][0] != 0:
        raise ValueError("first audio frame not found where the metadata ends")
    if total and sample != total:
        raise ValueError(f"frame scan found {sample} samples, STREAMINFO says {total}")
    return frames


def add_seektable(flac: bytes, interval_samples: int) -> bytes:
    """Returns ``flac`` with a SEEKTABLE (one point per ``interval_samples``, at the
    frame containing each multiple) inserted right after STREAMINFO, replacing any
    existing one. The audio frames are not touched, so their byte offsets stay valid.
    """
    blocks, first = _metadata_blocks(flac)
    frames = flac_frame_offsets(flac)
    points = []
    last_sample = -1
    fi = 0
    target = 0
    total = frames[-1][1] + frames[-1][2] if frames else 0
    while target < total:
        while fi + 1 < len(frames) and frames[fi + 1][1] <= target:
            fi += 1
        off, s, bs = frames[fi]
        if s != last_sample:
            points.append(struct.pack(">QQH", s, off, bs))
            last_sample = s
        target += interval_samples
    seek_body = b"".join(points)
    out_blocks = [blocks[0], (3, seek_body)] + [b for b in blocks[1:] if b[0] != 3]
    meta = bytearray(b"fLaC")
    for i, (t, body) in enumerate(out_blocks):
        last = 0x80 if i == len(out_blocks) - 1 else 0
        meta += bytes([last | t]) + len(body).to_bytes(3, "big") + body
    return bytes(meta) + flac[first:]


def encode_flac24(samples: np.ndarray, sample_rate: int, *,
                  compression_level: int = FLAC_COMPRESSION_LEVEL,
                  seek_interval_seconds: float | None = SEEK_INTERVAL_SECONDS) -> bytes:
    """Encodes int32 ``[frames, channels]`` 24-bit values as a 24-bit FLAC stream with
    STREAMINFO total samples + MD5, and a SEEKTABLE when ``seek_interval_seconds``.

    The integers are handed to libsndfile left-justified (``<< 8``); libsndfile's
    ``int -> FLAC 24`` path is an exact ``>> 8``, so no rounding happens in the encoder.
    """
    s = np.asarray(samples)
    if s.dtype != np.int32 or s.ndim != 2:
        raise ValueError("samples must be int32 [frames, channels]")
    if s.size and (s.min() < INT24_MIN or s.max() > INT24_MAX):
        raise ValueError("samples are not 24-bit")
    buf = io.BytesIO()
    sf.write(buf, np.left_shift(s, 8), int(sample_rate), format="FLAC", subtype="PCM_24",
             compression_level=compression_level / 8.0)
    data = buf.getvalue()
    blocks, _ = _metadata_blocks(data)
    si = parse_streaminfo(blocks[0][1])
    if si["total_samples"] != s.shape[0] or si["bits_per_sample"] != 24 or si["channels"] != s.shape[1]:
        raise RuntimeError(f"libsndfile wrote unexpected STREAMINFO {si}")
    if s.shape[0] and si["md5"] == b"\0" * 16:
        raise RuntimeError("libsndfile wrote no MD5 into STREAMINFO")
    if seek_interval_seconds and s.shape[0]:
        data = add_seektable(data, max(1, int(round(seek_interval_seconds * sample_rate))))
    return data


# ----------------------------------------------------------------------------- slots

def encode_slot(sequence: int, payload_json: bytes, slot_size: int) -> bytes:
    """A complete slot image. Plain JSON when it fits, else zlib; raises
    ``SlotPayloadTooLarge`` when neither fits."""
    capacity = slot_size - SLOT_HEADER_LEN
    if len(payload_json) <= capacity:
        enc, payload = ENC_JSON, payload_json
    else:
        enc, payload = ENC_ZLIB, zlib.compress(payload_json, 9)
        if len(payload) > capacity:
            raise SlotPayloadTooLarge(f"settings payload {len(payload)} bytes > slot capacity {capacity}")
    head = SLOT_MAGIC + struct.pack("<IQIII", SLOT_VERSION, sequence, len(payload), enc, 0)
    crc = _crc32(head + payload)
    img = head + struct.pack("<I", crc) + payload
    return img + b"\0" * (slot_size - len(img))


def decode_slot(raw: bytes) -> tuple[str, int, bytes | str]:
    """Returns ("empty", 0, b""), ("valid", sequence, json_bytes) or
    ("invalid", 0, reason)."""
    if len(raw) < SLOT_HEADER_LEN:
        return "invalid", 0, "slot shorter than its header"
    h = raw[:SLOT_HEADER_LEN]
    if h == b"\0" * SLOT_HEADER_LEN:
        return "empty", 0, b""
    if h[:4] != SLOT_MAGIC:
        return "invalid", 0, "bad slot magic"
    version, seq, length, enc, _res, crc = struct.unpack("<IQIIII", h[4:32])
    if version != SLOT_VERSION:
        return "invalid", 0, f"slot version {version} not supported"
    if length > len(raw) - SLOT_HEADER_LEN:
        return "invalid", 0, "payload length exceeds the slot"
    payload = raw[SLOT_HEADER_LEN:SLOT_HEADER_LEN + length]
    if _crc32(h[:28] + payload) != crc:
        return "invalid", 0, "slot checksum mismatch"
    if seq == 0:
        return "empty", 0, b""
    if enc == ENC_JSON:
        data = payload
    elif enc == ENC_ZLIB:
        try:
            d = zlib.decompressobj()
            data = d.decompress(payload, 64 * 1024 * 1024)
            if d.unconsumed_tail:
                return "invalid", 0, "slot payload inflates beyond 64 MiB"
        except zlib.error as e:
            return "invalid", 0, f"slot zlib payload does not inflate: {e}"
    else:
        return "invalid", 0, f"unknown slot encoding {enc}"
    return "valid", seq, data


def _choose_active(slots: list[tuple[str, int, Any]]) -> int | None:
    best, best_seq = None, -1
    for i, (state, seq, data) in enumerate(slots):
        if state != "valid":
            continue
        try:
            obj = json.loads(data.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if isinstance(obj, dict) and seq > best_seq:
            best, best_seq = i, seq
    return best


def default_user_state() -> dict:
    return {
        "user_state_version": 1,
        "updated_utc": utc_now(),
        "settings": {
            "semitones": 0, "shift_drums": False, "guide_vocal": 0.0, "backing_on": True,
            "backing_level": 1.0, "levels": {}, "lyrics_offset_ms": 0,
        },
        "metadata_override": {},
        "lyrics_override": None,
    }


# ----------------------------------------------------------------------------- header

def _encode_header(toc_offset: int, toc_length: int, toc_crc: int, slot_size: int, file_length: int) -> bytes:
    b = bytearray(HEADER_LENGTH)
    b[0:8] = MAGIC
    struct.pack_into("<HHIQQIIQQQ", b, 8, FORMAT_MAJOR, FORMAT_MINOR, HEADER_LENGTH, toc_offset, toc_length,
                     toc_crc, slot_size, ALIGN, ALIGN + slot_size, file_length)
    struct.pack_into("<I", b, 124, _crc32(bytes(b[:124])))
    return bytes(b)


def _parse_header(b: bytes, actual_len: int) -> dict:
    """Header checks in the order of SONG_FORMAT section 2: magic, CRC, major, length.
    The CRC is read as the last 4 bytes of header_length (124..128 in v1.0)."""
    n = min(len(b), 8)
    if b[:n] != MAGIC[:n]:
        raise NotASongFile("bad magic")
    if len(b) < 16:
        raise Truncated(f"only {actual_len} bytes")
    hl = struct.unpack_from("<I", b, 12)[0]
    if hl < HEADER_LENGTH or hl > ALIGN or hl % 4:
        raise Damaged(f"header_length {hl} not valid")
    if len(b) < hl:
        raise Truncated(f"only {actual_len} bytes")
    if _crc32(b[:hl - 4]) != struct.unpack_from("<I", b, hl - 4)[0]:
        raise Damaged("header checksum mismatch")
    major, minor = struct.unpack_from("<HH", b, 8)
    if major > FORMAT_MAJOR:
        raise NewerVersion(f"version {major}.{minor}")
    if major == 0:
        raise Damaged("major version 0")
    toc_offset, toc_length, toc_crc, slot_size, sa, sb, file_length = struct.unpack_from("<QQIIQQQ", b, 16)
    if actual_len < file_length:
        raise Truncated(f"header says {file_length} bytes, file has {actual_len}")
    h = dict(major=major, minor=minor, header_length=hl, toc_offset=toc_offset, toc_length=toc_length,
             toc_crc32=toc_crc, slot_size=slot_size, slot_a_offset=sa, slot_b_offset=sb, file_length=file_length)
    if slot_size < ALIGN or slot_size % ALIGN:
        raise Damaged("slot_size not valid")

    def within(off, ln):
        return off >= ALIGN and off + ln <= file_length

    def overlap(a, al, c, cl):
        return al > 0 and cl > 0 and a < c + cl and c < a + al

    if not within(sa, slot_size) or not within(sb, slot_size) or overlap(sa, slot_size, sb, slot_size):
        raise Damaged("settings slots out of bounds or overlapping")
    if toc_length == 0 or not within(toc_offset, toc_length) or overlap(toc_offset, toc_length, sa, slot_size) \
            or overlap(toc_offset, toc_length, sb, slot_size):
        raise Damaged("table of contents out of bounds")
    return h


# ----------------------------------------------------------------------------- writing

def write_container(path: str | os.PathLike, manifest: dict,
                    chunks: Iterable[tuple[str, str, bytes | os.PathLike]],
                    user_state: dict | None = None, slot_size: int = DEFAULT_SLOT_SIZE,
                    toc_extra: dict | None = None) -> dict:
    """Writes a complete song file atomically. ``chunks`` are (id, media type, data)
    where data is bytes or a path to a file to copy; the manifest is written first
    as the ``manifest`` chunk. Returns the TOC dict.

    Order (section 7): header placeholder, slot A (``user_state``, sequence 1), slot B
    (empty, zeros), chunks 4096-aligned, TOC, final header; fsync; rename.
    """
    path = Path(path)
    chunks = list(chunks)
    ids = [c[0] for c in chunks]
    if "manifest" in ids or len(set(ids)) != len(ids) or any(not i for i in ids):
        raise ValueError("chunk ids must be unique, non-empty, and not 'manifest'")
    referenced = [s["chunk"] for s in manifest.get("stems", [])]
    referenced += [manifest[k] for k in ("cover", "lyrics") if manifest.get(k)]
    missing = [r for r in referenced if r not in ids]
    if missing:
        raise ValueError(f"the manifest refers to chunks that were not given: {missing}")
    if slot_size < ALIGN or slot_size % ALIGN:
        raise ValueError("slot_size must be a multiple of 4096")

    state_json = _json_bytes(user_state if user_state is not None else default_user_state())
    while True:
        try:
            slot_a = encode_slot(1, state_json, slot_size)
            break
        except SlotPayloadTooLarge:
            slot_size = max(DEFAULT_SLOT_SIZE, -(-(len(state_json) + SLOT_HEADER_LEN) * 2 // ALIGN) * ALIGN)

    tmp = path.with_name(f"{path.name}.tmp-{secrets.token_hex(8)}")
    try:
        with open(tmp, "xb") as f:
            f.write(b"\0" * ALIGN)
            f.write(slot_a)
            f.write(b"\0" * slot_size)
            entries = []
            for cid, mime, data in [("manifest", "application/json", _json_bytes(manifest))] + chunks:
                _pad_to(f)
                off = f.tell()
                if isinstance(data, (bytes, bytearray, memoryview)):
                    data = bytes(data)
                    f.write(data)
                    length, crc = len(data), _crc32(data)
                else:
                    length, crc = 0, 0
                    with open(data, "rb") as src:
                        while block := src.read(1 << 20):
                            f.write(block)
                            crc = zlib.crc32(block, crc)
                            length += len(block)
                    crc &= 0xFFFFFFFF
                entries.append({"id": cid, "type": mime, "offset": off, "length": length, "crc32": crc})
            toc = {"chunks": entries, **(toc_extra or {})}
            toc_bytes = _json_bytes(toc)
            _pad_to(f)
            toc_off = f.tell()
            f.write(toc_bytes)
            file_len = f.tell()
            f.seek(0)
            f.write(_encode_header(toc_off, len(toc_bytes), _crc32(toc_bytes), slot_size, file_len))
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            tmp.unlink()
        except FileNotFoundError:
            pass
        raise
    if hasattr(os, "O_DIRECTORY"):  # POSIX: make the rename itself durable
        try:
            fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        except OSError:
            pass
    return toc


def validate_lyrics(lyrics: dict, duration: float) -> list[str]:
    """The timing rules of section 5 / checklist of section 9. Returns problems."""
    errs: list[str] = []
    eps = 1e-9
    tmax = duration + 0.001

    def in_song(t):
        return isinstance(t, (int, float)) and t == t and -eps <= t <= tmax

    prev_line = float("-inf")
    for li, line in enumerate(lyrics.get("lines", [])):
        if not (line["start"] <= line["end"] + eps):
            errs.append(f"line {li} ends before it starts")
        if not (in_song(line["start"]) and in_song(line["end"])):
            errs.append(f"line {li} outside the song")
        if line["start"] + eps < prev_line:
            errs.append(f"line {li} overlaps the previous line")
        prev_line = line["end"]
        prev_word = float("-inf")
        for wi, w in enumerate(line.get("words", [])):
            if not w.get("chars"):
                errs.append(f"line {li} word {wi} has no chars")
            if not (w["start"] <= w["end"] + eps) or not (in_song(w["start"]) and in_song(w["end"])):
                errs.append(f"line {li} word {wi} has bad times")
            if w["start"] + eps < prev_word:
                errs.append(f"line {li} word {wi} overlaps the previous word")
            prev_word = w["end"]
            prev_char = float("-inf")
            for ci, c in enumerate(w.get("chars", [])):
                g, s, e = c[0], c[1], c[2]
                if not isinstance(g, str) or not g:
                    errs.append(f"line {li} word {wi} char {ci} is empty")
                if not (s <= e + eps) or not (in_song(s) and in_song(e)):
                    errs.append(f"line {li} word {wi} char {ci} has bad times")
                if s + eps < prev_char:
                    errs.append(f"line {li} word {wi} char {ci} overlaps the previous char")
                prev_char = e
    return errs


def _image_type(data: bytes) -> str:
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    raise ValueError("cover must be a JPEG or PNG image")


def write_song(path: str | os.PathLike, *, stems: list[StemInput], sample_rate: int,
               title: str, artist: str, album: str | None = None,
               lyrics: dict | None = None, cover: bytes | None = None,
               analysis: dict | None = None, preset: str = "full",
               stems_sum_to_source: bool = False, provenance: dict | None = None,
               user_state: dict | None = None, extra_chunks: Iterable[tuple[str, str, bytes]] = (),
               manifest_extra: dict | None = None, slot_size: int = DEFAULT_SLOT_SIZE,
               compression_level: int = FLAC_COMPRESSION_LEVEL,
               source: np.ndarray | None = None, residual_role: str = "other",
               on_stem: Callable[[str], None] | None = None) -> dict:
    """Encodes the stems (gain rule, 24-bit FLAC with seek table) and writes a complete
    ``.jksong`` atomically. Returns the manifest that was written.

    ``lyrics`` is the section-5 dict (validated; ``ValueError`` lists the problems).
    ``provenance`` is the section-4.3 dict.

    Exact residual ("full" preset): pass the original audio as ``source`` (float
    ``[channels, frames]``). The stem whose role is ``residual_role`` is then computed
    here as ``source`` minus the *decoded* values of the other stems (after their own
    24-bit quantisation and gain), and its ``audio`` may be ``None``. All stems at
    unity then sum back to the source within half an LSB of the residual stem, instead
    of accumulating each stem's quantisation error and the float32 rounding of a
    residual computed upstream.

    ``on_stem(role)`` is called after each stem is encoded (progress; it may raise to
    stop, e.g. a cancelled job: nothing has been written to disk at that point).
    """
    if not stems:
        raise ValueError("at least one stem is required")
    if source is not None:
        source = np.asarray(source)
        if not any(s.role == residual_role for s in stems):
            raise ValueError(f"source given but no stem has the residual role {residual_role!r}")
    shapes = {np.asarray(s.audio).shape for s in stems if s.audio is not None}
    if source is not None:
        shapes.add(source.shape)
    if len(shapes) != 1:
        raise ValueError(f"all stems must have the same shape, got {shapes}")
    channels, frames = shapes.pop()
    if channels not in (1, 2):
        raise ValueError("channels must be 1 or 2")
    roles = [s.role for s in stems]
    if len(set(roles)) != len(roles):
        raise ValueError("stem roles must be unique")
    duration = frames / sample_rate
    if lyrics is not None:
        problems = validate_lyrics(lyrics, duration)
        if problems:
            raise ValueError("lyrics are not valid: " + "; ".join(problems[:10]))

    # Quantise every stem; with a source, the residual stem last, from what the others
    # will decode to.
    encoded: dict[str, tuple[bytes, float, float]] = {}
    order = [s for s in stems if source is None or s.role != residual_role]
    decoded_sum = np.zeros((channels, frames)) if source is not None else None
    for s in order:
        if s.audio is None:
            raise ValueError(f"stem {s.role!r} has no audio")
        q, gain, peak = quantize_stem(s.audio)
        if decoded_sum is not None:
            decoded_sum += q.T * (gain / FULL_SCALE)
        encoded[s.role] = (encode_flac24(q, sample_rate, compression_level=compression_level), gain, peak)
        del q
        if on_stem is not None:
            on_stem(s.role)
    if source is not None:
        residual = source.astype(np.float64) - decoded_sum
        del decoded_sum
        q, gain, peak = quantize_stem(residual)
        encoded[residual_role] = (encode_flac24(q, sample_rate, compression_level=compression_level), gain, peak)
        del q, residual
        if on_stem is not None:
            on_stem(residual_role)

    chunks: list[tuple[str, str, bytes]] = []
    stem_entries = []
    for s in stems:
        flac, gain, peak = encoded[s.role]
        chunk_id = f"stem.{s.role}"
        chunks.append((chunk_id, "audio/flac", flac))
        stem_entries.append({
            "role": s.role, "chunk": chunk_id, "codec": "flac", "bits": 24,
            "gain": gain, "peak": peak,
            "in_karaoke_mix": s.in_karaoke_mix if s.in_karaoke_mix is not None else s.role != "lead_vocal",
            "pitched": s.pitched if s.pitched is not None else s.role != "drums",
            **s.extra,
        })
    front: list[tuple[str, str, bytes]] = []
    if lyrics is not None:
        front.append(("lyrics", "application/json", _json_bytes(lyrics)))
    if cover is not None:
        if len(cover) > MAX_COVER_BYTES:
            raise ValueError("cover is larger than 2 MB")
        front.append(("cover", _image_type(cover), bytes(cover)))

    manifest = {
        "format": "jksong",
        "format_version": FORMAT_VERSION,
        "title": title,
        "artist": artist,
        "album": album,
        "sample_rate": int(sample_rate),
        "channels": int(channels),
        "duration_frames": int(frames),
        "duration_seconds": duration,
        "cover": "cover" if cover is not None else None,
        "lyrics": "lyrics" if lyrics is not None else None,
        "analysis": analysis if analysis is not None else {"key": None, "tempo_bpm": None},
        "preset": preset,
        "stems_sum_to_source": bool(stems_sum_to_source),
        "stems": stem_entries,
        "provenance": provenance if provenance is not None else {},
        **(manifest_extra or {}),
    }
    write_container(path, manifest, front + chunks + list(extra_chunks), user_state, slot_size)
    return manifest


# ----------------------------------------------------------------------------- reading

class Song:
    """An open song file (header, TOC, manifest and slots read and checked)."""

    def __init__(self, path: str | os.PathLike):
        self.path = Path(path)
        with open(self.path, "rb") as f:
            actual = os.fstat(f.fileno()).st_size
            head = f.read(ALIGN)
            self.header = _parse_header(head, actual)
            h = self.header
            f.seek(h["toc_offset"])
            toc_bytes = f.read(h["toc_length"])
            if _crc32(toc_bytes) != h["toc_crc32"]:
                raise Damaged("table of contents checksum mismatch")
            try:
                self.toc = json.loads(toc_bytes.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                raise Damaged(f"table of contents is not JSON: {e}") from None
            seen = set()
            chunks = self.toc.get("chunks") if isinstance(self.toc, dict) else None
            if not isinstance(chunks, list):
                raise Damaged("table of contents has no chunk list")
            for c in chunks:
                ok = (isinstance(c, dict) and isinstance(c.get("id"), str) and isinstance(c.get("type"), str)
                      and all(isinstance(c.get(k), int) and not isinstance(c.get(k), bool) and c[k] >= 0
                              for k in ("offset", "length", "crc32")))
                if not ok:
                    raise Damaged(f"table of contents entry not valid: {c!r:.200}")
                if c["id"] in seen:
                    raise Damaged(f"chunk id {c['id']!r} twice")
                seen.add(c["id"])
                if c["offset"] < ALIGN or c["offset"] + c["length"] > h["file_length"]:
                    raise Damaged(f"chunk {c['id']!r} outside the file")
                for so, sl in ((h["slot_a_offset"], h["slot_size"]), (h["slot_b_offset"], h["slot_size"]),
                               (h["toc_offset"], h["toc_length"])):
                    if c["length"] and c["offset"] < so + sl and so < c["offset"] + c["length"]:
                        raise Damaged(f"chunk {c['id']!r} overlaps a settings slot or the table of contents")
            self._chunks = {c["id"]: c for c in chunks}
            if "manifest" not in self._chunks:
                raise MissingChunk("manifest")
            try:
                self.manifest = json.loads(self.read_chunk("manifest").decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as e:
                raise Damaged(f"manifest is not JSON: {e}") from None
            m = self.manifest
            if not isinstance(m, dict) or m.get("format") != "jksong":
                raise Damaged("manifest format is not 'jksong'")
            if not isinstance(m.get("stems"), list) or not all(
                    isinstance(s, dict) and isinstance(s.get("role"), str) and isinstance(s.get("chunk"), str)
                    for s in m["stems"]):
                raise Damaged("manifest stems are not valid")
            try:
                fmajor = int(str(m.get("format_version", "")).split(".")[0])
            except ValueError:
                raise Damaged("manifest format_version not valid") from None
            if fmajor > FORMAT_MAJOR:
                raise NewerVersion(f"format_version {m['format_version']}")
            for ref in [s["chunk"] for s in m.get("stems", [])] + [m.get(k) for k in ("cover", "lyrics")]:
                if ref and ref not in self._chunks:
                    raise MissingChunk(ref)
            self.slots = []
            for off in (h["slot_a_offset"], h["slot_b_offset"]):
                f.seek(off)
                self.slots.append(decode_slot(f.read(h["slot_size"])))
        self.active_slot = _choose_active(self.slots)
        self.user_state = (json.loads(self.slots[self.active_slot][2].decode("utf-8"))
                           if self.active_slot is not None else default_user_state())

    # -- chunks
    def chunk_ids(self) -> list[str]:
        return [c["id"] for c in self.toc.get("chunks", [])]

    def chunk_entry(self, cid: str) -> dict:
        try:
            return self._chunks[cid]
        except KeyError:
            raise MissingChunk(cid) from None

    def read_chunk(self, cid: str, check_crc: bool = True) -> bytes:
        c = self.chunk_entry(cid)
        with open(self.path, "rb") as f:
            f.seek(c["offset"])
            data = f.read(c["length"])
        if len(data) != c["length"]:
            raise Truncated(f"chunk {cid!r} cut short")
        if check_crc and _crc32(data) != c["crc32"]:
            raise Damaged(f"chunk {cid!r} checksum mismatch")
        return data

    # -- content
    def lyrics(self) -> dict | None:
        lid = self.manifest.get("lyrics")
        return json.loads(self.read_chunk(lid).decode("utf-8")) if lid else None

    def effective_lyrics(self) -> dict | None:
        lo = self.user_state.get("lyrics_override")
        return lo if isinstance(lo, dict) and isinstance(lo.get("lines"), list) else self.lyrics()

    def effective_metadata(self) -> dict:
        o = self.user_state.get("metadata_override") or {}
        out = {k: self.manifest.get(k) for k in ("title", "artist", "album")}
        for k in ("title", "artist"):
            if isinstance(o.get(k), str):
                out[k] = o[k]
        if "album" in o and (o["album"] is None or isinstance(o["album"], str)):
            out["album"] = o["album"]
        return out

    def effective_settings(self) -> dict:
        d = default_user_state()["settings"]
        s = self.user_state.get("settings")
        if isinstance(s, dict):
            for k, v in s.items():
                d[k] = v
        return d

    def stem(self, role: str) -> dict:
        for s in self.manifest["stems"]:
            if s["role"] == role:
                return s
        raise MissingChunk(f"stem.{role}")

    def read_stem_int(self, role: str) -> np.ndarray:
        """Decoded true 24-bit integers, int32 ``[frames, channels]``."""
        data = self.read_chunk(self.stem(role)["chunk"])
        x, rate = sf.read(io.BytesIO(data), dtype="int32", always_2d=True)
        if rate != self.manifest["sample_rate"]:
            raise Damaged(f"stem {role!r} rate {rate} != {self.manifest['sample_rate']}")
        return np.right_shift(x, 8)

    def read_stem(self, role: str) -> np.ndarray:
        """Decoded float32 ``[channels, frames]`` with the stored gain applied."""
        gain = float(self.stem(role).get("gain", 1.0))
        q = self.read_stem_int(role)
        return (q.T.astype(np.float64) * (gain / FULL_SCALE)).astype(np.float32)


def open_song(path: str | os.PathLike) -> Song:
    return Song(path)


def update_user_state(path: str | os.PathLike, edit: Callable[[dict], None]) -> tuple[int, int]:
    """Read-merge-write into the inactive slot (section 6). ``edit`` mutates the active
    payload dict in place (unknown keys are naturally kept). Returns (slot 0/1,
    sequence). Raises ``SlotPayloadTooLarge`` if it does not fit."""
    with open(path, "r+b") as f:
        actual = os.fstat(f.fileno()).st_size
        h = _parse_header(f.read(ALIGN), actual)
        slots = []
        for off in (h["slot_a_offset"], h["slot_b_offset"]):
            f.seek(off)
            slots.append(decode_slot(f.read(h["slot_size"])))
        active = _choose_active(slots)
        state = json.loads(slots[active][2].decode("utf-8")) if active is not None else default_user_state()
        edit(state)
        state["updated_utc"] = utc_now()
        target = 1 if active == 0 else 0
        seq = max([s[1] for s in slots if s[0] == "valid"] + [0]) + 1
        image = encode_slot(seq, _json_bytes(state), h["slot_size"])
        f.seek(h["slot_a_offset"] if target == 0 else h["slot_b_offset"])
        f.write(image)
        f.flush()
        os.fsync(f.fileno())
    return target, seq


# ----------------------------------------------------------------------------- library

def is_song_file_name(name: str) -> bool:
    return (not name.startswith(".") and ".tmp-" not in name
            and len(name) > 7 and name.lower().endswith(".jksong"))


def list_song_files(folder: str | os.PathLike) -> list[Path]:
    """``*.jksong`` in ``folder`` (not recursive), skipping dotfiles, ``._*`` and
    ``*.tmp-*`` files."""
    folder = Path(folder)
    return sorted((p for p in folder.iterdir() if p.is_file() and is_song_file_name(p.name)),
                  key=lambda p: p.name.lower())

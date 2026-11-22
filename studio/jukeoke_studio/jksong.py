# SPDX-License-Identifier: GPL-3.0-or-later
"""Reader and writer for the ``.jksong`` song file, version 1.1 (docs/SONG_FORMAT.md).

Version 1.1 (Work Order 2, practice mode) adds the melody guide (chunks ``guide`` and
``guide.curve``, section 10), its raw-copy rewrite into an existing file (section 7.1:
:func:`add_guide`) and the practice data in the settings slot (section 11); see the
"1.1" part at the end of this module.

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
    # 1.1
    "GUIDE_CHUNK", "CURVE_CHUNK", "round3", "round_half_away", "encode_curve", "decode_curve",
    "validate_guide", "validate_curve", "note_graded", "map_words", "set_word_refs",
    "word_refs_valid", "guide_counts", "guide_state", "prepare_guide_rewrite", "finish_guide_rewrite",
    "add_guide", "GuideInterrupted", "round_practice_result", "practice_add_result",
    "practice_shrink_once", "save_practice", "validate_practice",
]

# ----------------------------------------------------------------------------- constants

MAGIC = b"\x89JKS\r\n\x1a\n"
FORMAT_MAJOR = 1
FORMAT_MINOR = 1          # 1.1 writers write minor 1 (Work Order 2)
FORMAT_VERSION = "1.1"
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
    user_message = "This is not an Open Mic song file (or it was damaged when it was copied as text)."


class NewerVersion(JkSongError):
    kind = "newer_version"
    user_message = "This song was made by a newer version of Open Mic; please update."


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

def _encode_header(toc_offset: int, toc_length: int, toc_crc: int, slot_size: int, file_length: int,
                   minor: int = FORMAT_MINOR) -> bytes:
    b = bytearray(HEADER_LENGTH)
    b[0:8] = MAGIC
    struct.pack_into("<HHIQQIIQQQ", b, 8, FORMAT_MAJOR, minor, HEADER_LENGTH, toc_offset, toc_length,
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


    # -- 1.1: the melody guide (section 10)
    def guide(self) -> dict | None:
        """The ``guide`` chunk parsed, or None."""
        if GUIDE_CHUNK not in self._chunks:
            return None
        return json.loads(self.read_chunk(GUIDE_CHUNK).decode("utf-8"))

    def guide_curve(self) -> dict | None:
        """The curve the guide names, decoded (see :func:`decode_curve`), or None."""
        g = self.guide()
        cid = g.get("curve") if isinstance(g, dict) else None
        if not cid or cid not in self._chunks:
            return None
        return decode_curve(self.read_chunk(cid))

    def practice(self) -> dict | None:
        """The active slot's ``practice`` value (section 11), or None."""
        p = self.user_state.get(PRACTICE_KEY)
        return p if isinstance(p, dict) else None


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


# ============================================================================= 1.1
#
# The melody guide (SONG_FORMAT sections 7.1, 9, 10) and the practice data in the
# settings slot (section 11). Written independently of the Rust implementation
# (crates/jk-format/src/guide.rs, guide_write.rs, practice.rs and the word rule in
# crates/jk-pitch/src/guide/words.rs); tests/conformance checks the two against each other.

GUIDE_CHUNK = "guide"
CURVE_CHUNK = "guide.curve"
GUIDE_MIME = "application/json"
CURVE_MIME = "application/octet-stream"
GUIDE_VERSION = 1
CURVE_MAGIC = b"JKPC"
CURVE_VERSION = 1
CURVE_HEADER_LEN = 32
CURVE_F32 = 1
END_TOLERANCE_S = 0.05
OVERLAP_TOLERANCE_S = 0.001
WORD_START_TOLERANCE_S = 0.0015
DEFAULT_TUNING_MIN_STRENGTH = 0.3
PRACTICE_KEY = "practice"
MAX_RECENT = 20


def round_half_away(x: float, decimals: int) -> float:
    """Rounds half away from zero to ``decimals`` places, the same arithmetic as the Rust
    writer's ``(x * 10^d).round() / 10^d`` (Python's ``round`` rounds half to even)."""
    import math
    p = 10.0 ** decimals
    y = x * p
    a = abs(y)
    r = math.floor(a)
    if a - r >= 0.5:
        r += 1.0
    return math.copysign(r, y) / p


def round3(x: float) -> float:
    """Guide times and pitches are written to 3 decimals (section 10.1)."""
    return round_half_away(x, 3)


# ----------------------------------------------------------------------------- curve

def encode_curve(midi, voic, lvdb, hop_us: int = 10_000, t0_us: int = 0, extra_fields=()) -> bytes:
    """The ``guide.curve`` chunk (section 10.3): header, field table (tag, format code),
    then each field's ``N`` float32 values (planar). ``extra_fields``: more
    ``(tag, format, values)`` after the three defined ones."""
    fields = [(b"MIDI", CURVE_F32, midi), (b"VOIC", CURVE_F32, voic), (b"LVDB", CURVE_F32, lvdb)]
    fields += list(extra_fields)
    n = len(fields[0][2])
    if any(len(f[2]) != n for f in fields):
        raise ValueError("every curve field needs the same number of frames")
    out = bytearray(CURVE_MAGIC + struct.pack("<HHIIi", CURVE_VERSION, len(fields), n, hop_us, t0_us) + b"\0" * 12)
    for tag, fmt, _ in fields:
        if len(tag) != 4:
            raise ValueError("curve tags are 4 bytes")
        out += tag + struct.pack("<I", fmt)
    for _, fmt, vals in fields:
        out += np.asarray(vals, dtype="<f4").tobytes()
    return bytes(out)


def decode_curve(b: bytes) -> dict:
    """Parses a curve. Returns ``{"hop_us", "t0_us", "frames", "fields": {tag: array}}``
    (float fields only; others are skipped). Raises ``Damaged`` for a bad magic or length,
    ``NewerVersion`` for a curve version above 1 (readers treat it as absent)."""
    if len(b) < CURVE_HEADER_LEN or b[:4] != CURVE_MAGIC:
        raise Damaged("pitch curve: bad magic")
    version, f, n, hop_us, t0_us = struct.unpack_from("<HHIIi", b, 4)
    if version != CURVE_VERSION:
        raise NewerVersion(f"pitch curve version {version}")
    if f == 0 or hop_us == 0:
        raise Damaged("pitch curve: no fields or zero frame step")
    if len(b) != CURVE_HEADER_LEN + 8 * f + 4 * n * f:
        raise Damaged(f"pitch curve: length {len(b)}, expected {CURVE_HEADER_LEN + 8 * f + 4 * n * f}")
    fields = {}
    base = CURVE_HEADER_LEN + 8 * f
    for i in range(f):
        tag = b[CURVE_HEADER_LEN + 8 * i:CURVE_HEADER_LEN + 8 * i + 4].decode("ascii", "replace")
        fmt = struct.unpack_from("<I", b, CURVE_HEADER_LEN + 8 * i + 4)[0]
        if fmt == CURVE_F32 and tag not in fields:
            fields[tag] = np.frombuffer(b, dtype="<f4", count=n, offset=base + 4 * n * i)
    return {"hop_us": hop_us, "t0_us": t0_us, "frames": n, "fields": fields}


def curve_frames_for(duration_s: float, hop_us: int = 10_000) -> int:
    """``N = floor(duration / hop) + 1`` (the same tiny epsilon as the Rust writer)."""
    import math
    return int(math.floor(duration_s / (hop_us * 1e-6) + 1e-9)) + 1


# ----------------------------------------------------------------------------- notes

def note_is_pitched(n: dict) -> bool:
    return n.get("kind") == "pitched"            # an unknown kind reads as unpitched


def note_octave_ok(n: dict) -> bool:
    return n.get("octave") in ("ok", "unchecked")  # an unknown value counts as flagged


def note_graded(n: dict, low_confidence: float) -> bool:
    """Section 10.1 "Which notes are graded", before the singer's exclusions."""
    return note_is_pitched(n) and float(n.get("confidence", 0.0)) >= low_confidence and note_octave_ok(n)


def guide_counts(doc: dict) -> dict:
    notes = doc.get("notes", [])
    low = float(doc.get("low_confidence", 0.5))
    return {
        "notes": len(notes),
        "pitched": sum(1 for n in notes if note_is_pitched(n)),
        "unpitched": sum(1 for n in notes if not note_is_pitched(n)),
        # Pitched notes only (the contract's example: 397 pitched - 64 low = 333 graded).
        "low_confidence": sum(1 for n in notes if note_is_pitched(n) and float(n.get("confidence", 0)) < low),
        "octave_flags": sum(1 for n in notes if note_is_pitched(n) and not note_octave_ok(n)),
        "graded": sum(1 for n in notes if note_graded(n, low)),
    }


def map_words(notes: list[tuple[float, float]], words: list[tuple[float, float]]) -> list[int | None]:
    """Section 10.4: for each note (start, end) the index of the word (start, end, the
    lyrics offset already added) with the largest positive overlap (the earlier word on a
    tie), kept when the overlap is at least 30 % of the note or 50 % of the word.
    Same arithmetic as ``jk_pitch::guide::words::map_words``."""
    out: list[int | None] = []
    for ns, ne in notes:
        best, best_ov = None, 0.0
        for i, (ws, we) in enumerate(words):
            ov = min(ne, we) - max(ns, ws)
            if ov <= 0.0:
                continue
            if best is None or ov > best_ov:
                best, best_ov = i, ov
        if best is None:
            out.append(None)
            continue
        ws, we = words[best]
        out.append(best if (best_ov >= 0.3 * (ne - ns) or best_ov >= 0.5 * (we - ws)) else None)
    return out


def _flat_words(lyrics: dict | None, offset_ms: int) -> list[tuple[int, int, float, float]]:
    if not lyrics:
        return []
    off = offset_ms / 1000.0
    return [(li, wi, w["start"] + off, w["end"] + off)
            for li, line in enumerate(lyrics.get("lines", []))
            for wi, w in enumerate(line.get("words", []))]


def set_word_refs(doc: dict, lyrics: dict | None, is_override: bool, offset_ms: int) -> None:
    """Attaches each note to a word of the effective lyrics (section 10.4) in place."""
    for n in doc.get("notes", []):
        n.pop("word", None)
    if lyrics is None:
        doc["words"] = None
        return
    flat = _flat_words(lyrics, offset_ms)
    m = map_words([(n["start"], n["end"]) for n in doc["notes"]], [(w[2], w[3]) for w in flat])
    for n, i in zip(doc["notes"], m):
        if i is not None:
            li, wi, ws, _ = flat[i]
            n["word"] = [li, wi, round3(ws)]
    doc["words"] = {"lyrics": "override" if is_override else "chunk", "offset_ms": int(offset_ms),
                    "word_count": len(flat)}


def word_refs_valid(doc: dict, lyrics: dict | None, offset_ms: int) -> bool:
    """Section 10.4: may the stored references be used as they are?"""
    if not lyrics:
        return all("word" not in n for n in doc.get("notes", []))
    w = doc.get("words")
    if not isinstance(w, dict):
        return False
    flat = _flat_words(lyrics, offset_ms)
    if w.get("word_count") != len(flat) or w.get("offset_ms") != offset_ms:
        return False
    lines = lyrics.get("lines", [])
    off = offset_ms / 1000.0
    for n in doc.get("notes", []):
        ref = n.get("word")
        if ref is None:
            continue
        if _malformed_word_ref(ref):
            return False      # a broken cache entry: recompute them all (verifier fix)
        li, wi, ws = ref
        if not (0 <= li < len(lines) and 0 <= wi < len(lines[li].get("words", []))):
            return False
        if abs(lines[li]["words"][wi]["start"] + off - ws) > WORD_START_TOLERANCE_S:
            return False
    return True


# ----------------------------------------------------------------------------- validation

def _num(x) -> bool:
    return isinstance(x, (int, float)) and not isinstance(x, bool) and x == x and abs(x) != float("inf")


def _malformed_word_ref(ref) -> bool:
    """A note's ``word`` that is present but not ``[line, word, word_start]``
    (non-negative integers and a finite number). Readers treat it as no word; the Rust
    reader (``jk_format::guide::malformed_word_ref``) and ``verify`` agree."""
    if ref is None:
        return False
    if not (isinstance(ref, list) and len(ref) == 3):
        return True
    li, wi, ws = ref
    ok_int = lambda x: isinstance(x, int) and not isinstance(x, bool) and x >= 0  # noqa: E731
    return not (ok_int(li) and ok_int(wi) and _num(ws))


def validate_guide(doc, duration: float) -> tuple[list[str], list[str]]:
    """The guide checks of section 9 on a parsed ``guide`` chunk. Returns (errors,
    warnings); the same split as ``jk_format::guide::validate_doc``."""
    errs: list[str] = []
    warns: list[str] = []
    if not isinstance(doc, dict):
        return ["guide: not a JSON object"], warns
    gv = doc.get("guide_version")
    if isinstance(gv, int) and gv > GUIDE_VERSION:
        return errs, [f"guide: guide_version {gv} was made by a newer version (not checked further)"]
    if gv != GUIDE_VERSION:
        return ["guide: guide_version must be 1"], warns
    if not (_num(doc.get("hop_s", 0.01)) and doc.get("hop_s", 0.01) > 0):
        errs.append("guide: hop_s must be a positive number")
    low = doc.get("low_confidence", 0.5)
    if not (_num(low) and 0 <= low <= 1):
        errs.append("guide: low_confidence must be within 0..1")
    tmax = duration + END_TOLERANCE_S
    prev = None
    for i, n in enumerate(doc.get("notes", [])):
        who = f"guide: note {i}"
        if not isinstance(n, dict):
            errs.append(f"{who}: not an object")
            continue
        s, e = n.get("start"), n.get("end")
        if not (_num(s) and _num(e) and 0 <= s < e <= tmax):
            errs.append(f"{who}: start / end not within 0 <= start < end <= song length + 0.05")
            prev = None
            continue
        if prev is not None:
            if s < prev["start"]:
                errs.append(f"{who}: notes are not ordered by start")
            elif prev["end"] - s > OVERLAP_TOLERANCE_S + 1e-9:
                errs.append(f"{who}: overlaps the previous note by more than 1 ms")
        c = n.get("confidence")
        if not (_num(c) and 0 <= c <= 1):
            errs.append(f"{who}: confidence not within 0..1")
        g = n.get("glide_in", 0.0)
        if not (_num(g) and 0 <= g <= (e - s) + 0.0015):
            errs.append(f"{who}: glide_in not within 0..(end - start)")
        m = n.get("midi")
        midi_ok = _num(m) and 12 <= m <= 120
        if n.get("kind") == "pitched":
            if not midi_ok:
                errs.append(f"{who}: midi not within 12..120")
        elif n.get("kind") == "unpitched":
            if not (midi_ok or m == 0):
                errs.append(f"{who}: unpitched midi is neither 0 nor within 12..120")
        else:
            warns.append(f"{who}: unknown kind {n.get('kind')!r} (read as unpitched)")
        if n.get("octave") not in ("ok", "maybe_high", "maybe_low", "unchecked"):
            warns.append(f"{who}: unknown octave value {n.get('octave')!r} (read as flagged)")
        if _malformed_word_ref(n.get("word")):
            warns.append(f"{who}: word reference is not [line, word, word_start] (ignored)")
        prev = n
    prev_start = float("-inf")
    for i, sp in enumerate(doc.get("spans", [])):
        s, e = sp.get("start"), sp.get("end")
        if not (_num(s) and _num(e) and 0 <= s <= e <= tmax):
            errs.append(f"guide: span {i} is not inside the song")
            continue
        if s < prev_start:
            errs.append(f"guide: span {i} is not ordered by start")
        prev_start = s
        if sp.get("kind") not in ("spoken", "breath"):
            warns.append(f"guide: span {i}: unknown kind {sp.get('kind')!r} (drawn as a gap)")
    t = doc.get("tuning")
    if not isinstance(t, dict):
        errs.append("guide: tuning missing")
    else:
        if not (_num(t.get("cents")) and -50 <= t["cents"] <= 50):
            errs.append("guide: tuning.cents not within -50..50")
        if not (_num(t.get("strength")) and 0 <= t["strength"] <= 1):
            errs.append("guide: tuning.strength not within 0..1")
        if not (_num(t.get("use_cents")) and -50 <= t["use_cents"] <= 50):
            errs.append("guide: tuning.use_cents not within -50..50")
        elif _num(t.get("cents")) and _num(t.get("strength")):
            ms = ((doc.get("provenance") or {}).get("parameters") or {}).get("tuning_min_strength",
                                                                             DEFAULT_TUNING_MIN_STRENGTH)
            want = t["cents"] if t["strength"] >= ms else 0.0
            if abs(t["use_cents"] - want) > 0.05:
                warns.append("guide: tuning.use_cents disagrees with the 0.3 strength rule")
    p = doc.get("provenance")
    if not (isinstance(p, dict) and isinstance(p.get("method"), str) and p["method"].strip()):
        errs.append("guide: provenance.method missing")
    if not (isinstance(p, dict) and isinstance(p.get("version"), str) and p["version"].strip()):
        errs.append("guide: provenance.version missing")
    c = doc.get("counts")
    if isinstance(c, dict):
        want = guide_counts(doc)
        if any(c.get(k) != v for k, v in want.items()):
            warns.append("guide: counts disagree with the notes (informational only)")
    return errs, warns


def validate_curve(curve: dict, duration: float) -> list[str]:
    """Section 9 curve checks on a decoded curve: values finite, MIDI 0 or 12..120,
    VOIC 0..1, frames cover the song."""
    errs = []
    f = curve["fields"]
    if "MIDI" not in f:
        errs.append("guide.curve: no MIDI field")
    for tag, v in f.items():
        v = np.asarray(v, dtype=np.float64)
        if not np.all(np.isfinite(v)):
            errs.append(f"guide.curve: {tag} has a value that is not finite")
            continue
        if tag == "MIDI" and np.any((v != 0) & ((v < 12) | (v > 120))):
            errs.append("guide.curve: MIDI values must be 0 or within 12..120")
        if tag == "VOIC" and np.any((v < 0) | (v > 1)):
            errs.append("guide.curve: VOIC values must be within 0..1")
    hop = curve["hop_us"] * 1e-6
    covered = (max(curve["frames"], 1) - 1) * hop + curve["t0_us"] * 1e-6
    if abs(covered - duration) > hop + 1e-6:
        errs.append(f"guide.curve: frames cover {covered:.3f} s, the song is {duration:.3f} s")
    return errs


def _cmp_versions(a: str, b: str) -> int:
    def parse(s):
        out = []
        for part in str(s).split("."):
            try:
                out.append(int(part.strip()))
            except ValueError:
                out.append(0)
        return out
    x, y = parse(a), parse(b)
    for i in range(max(len(x), len(y))):
        p = x[i] if i < len(x) else 0
        q = y[i] if i < len(y) else 0
        if p != q:
            return -1 if p < q else 1
    return 0


def guide_state(song: "Song", method: str, version: str) -> str:
    """``none`` | ``ready`` | ``outdated`` | ``stale`` | ``newer`` (section 10.2), for a
    builder ``method`` / ``version``."""
    if GUIDE_CHUNK not in song.chunk_ids():
        return "none"
    try:
        doc = json.loads(song.read_chunk(GUIDE_CHUNK).decode("utf-8"))
    except (JkSongError, UnicodeDecodeError, json.JSONDecodeError):
        return "stale"
    if isinstance(doc.get("guide_version"), int) and doc["guide_version"] > GUIDE_VERSION:
        return "newer"
    src = (doc.get("provenance") or {}).get("source") or {}
    try:
        lead = song.chunk_entry(song.stem("lead_vocal")["chunk"])
    except JkSongError:
        return "stale"
    if src.get("length") != lead["length"] or src.get("crc32") != lead["crc32"]:
        return "stale"
    p = doc.get("provenance") or {}
    if p.get("method") != method or _cmp_versions(p.get("version", ""), version) < 0:
        return "outdated"
    return "ready"


# ----------------------------------------------------------------------------- 7.1 rewrite

class GuideInterrupted(Exception):
    """Raised by the rewrite at a simulated interruption point (tests): like a crash,
    the temporary file is left as it is."""


INTERRUPT_POINTS = ("after_placeholder", "after_slots", "mid_chunks", "before_toc", "before_header",
                    "before_second_slot_copy", "before_rename", "truncated_temp")


def _read_header_toc(f) -> tuple[dict, dict]:
    actual = os.fstat(f.fileno()).st_size
    f.seek(0)
    h = _parse_header(f.read(ALIGN), actual)
    f.seek(h["toc_offset"])
    tb = f.read(h["toc_length"])
    if _crc32(tb) != h["toc_crc32"]:
        raise Damaged("table of contents checksum mismatch")
    try:
        toc = json.loads(tb.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise Damaged(f"table of contents is not JSON: {e}") from None
    if not isinstance(toc, dict) or not isinstance(toc.get("chunks"), list):
        raise Damaged("table of contents has no chunk list")
    return h, toc


def _slot_regions(f, h: dict) -> tuple[bytes, bytes]:
    f.seek(h["slot_a_offset"])
    a = f.read(h["slot_size"])
    f.seek(h["slot_b_offset"])
    b = f.read(h["slot_size"])
    if len(a) != h["slot_size"] or len(b) != h["slot_size"]:
        raise Truncated("settings slots cut short")
    return a, b


def _kept(toc: dict) -> list[tuple]:
    return [(c["id"], c["type"], c["length"], c["crc32"]) for c in toc["chunks"]
            if c["id"] not in (GUIDE_CHUNK, CURVE_CHUNK)]


def prepare_guide_rewrite(path, guide_json: bytes, curve: bytes, *, _interrupt: str | None = None) -> dict:
    """Section 7.1 step 2: writes ``<name>.tmp-<random>`` next to the song (header
    placeholder; both slot regions raw; every chunk raw in TOC order with its CRC checked,
    an existing guide replaced in place, a new one appended; the TOC; the final header
    with minor ``max(old, 1)``) and flushes it. The original is only read. Returns the
    state :func:`finish_guide_rewrite` needs."""
    path = Path(path)
    tmp = path.with_name(f"{path.name}.tmp-{secrets.token_hex(8)}")
    with open(path, "rb") as src:
        oh, otoc = _read_header_toc(src)
        if oh["header_length"] != HEADER_LENGTH:
            raise ValueError("header_length is newer than this writer; not rewritten")
        sa, sb = _slot_regions(src, oh)
        ids = [c["id"] for c in otoc["chunks"]]

        def stop(point):
            if _interrupt == point:
                raise GuideInterrupted(point)

        done = False
        try:
            with open(tmp, "xb") as f:
                f.write(b"\0" * ALIGN)
                stop("after_placeholder")
                f.write(sa)
                f.write(sb)
                stop("after_slots")
                entries = []

                def put(cid, mime, data):
                    _pad_to(f)
                    off = f.tell()
                    f.write(data)
                    entries.append({"id": cid, "type": mime, "offset": off, "length": len(data),
                                    "crc32": _crc32(data)})

                for i, c in enumerate(otoc["chunks"]):
                    if c["id"] == GUIDE_CHUNK:
                        put(GUIDE_CHUNK, GUIDE_MIME, guide_json)
                    elif c["id"] == CURVE_CHUNK:
                        put(CURVE_CHUNK, CURVE_MIME, curve)
                    else:
                        _pad_to(f)
                        off = f.tell()
                        src.seek(c["offset"])
                        left, crc = c["length"], 0
                        while left:
                            block = src.read(min(left, 1 << 20))
                            if not block:
                                raise Damaged(f"chunk {c['id']!r} ended early")
                            crc = zlib.crc32(block, crc)
                            f.write(block)
                            left -= len(block)
                        if (crc & 0xFFFFFFFF) != c["crc32"]:
                            raise Damaged(f"chunk {c['id']!r} checksum mismatch while copying; the song was left as it was")
                        e = dict(c)          # id, type, length, crc32 and unknown keys kept
                        e["offset"] = off
                        entries.append(e)
                    if i == 0:
                        stop("mid_chunks")
                if GUIDE_CHUNK not in ids:
                    put(GUIDE_CHUNK, GUIDE_MIME, guide_json)
                if CURVE_CHUNK not in ids:
                    put(CURVE_CHUNK, CURVE_MIME, curve)
                stop("before_toc")
                # The TOC keeps "chunks" first, then the unknown top-level keys.
                toc = {"chunks": entries, **{k: v for k, v in otoc.items() if k != "chunks"}}
                tb = _json_bytes(toc)
                _pad_to(f)
                toc_off = f.tell()
                f.write(tb)
                file_len = f.tell()
                stop("before_header")
                f.seek(0)
                header = _encode_header(toc_off, len(tb), _crc32(tb), oh["slot_size"], file_len,
                                        minor=max(oh["minor"], 1))
                f.write(header)
                f.flush()
                os.fsync(f.fileno())
            done = True
        except GuideInterrupted:
            done = True          # a crash leaves the temporary file behind
            raise
        finally:
            if not done:
                try:
                    tmp.unlink()
                except FileNotFoundError:
                    pass
    return {"original": path, "tmp": tmp, "slot_size": oh["slot_size"], "toc": toc,
            "header": header, "file_length": file_len, "guide_crc": _crc32(guide_json), "curve_crc": _crc32(curve)}


def finish_guide_rewrite(prep: dict, *, _interrupt: str | None = None) -> None:
    """Section 7.1 steps 3-5: copy both slot regions raw again from the original as it is
    now, check the temporary file against it (same slot size, every chunk but the guide's
    with the same id/type/length/crc32, the new guide's CRCs, the slots byte-identical),
    then rename over the original. Any difference: the temporary file is deleted and
    ``Damaged`` / ``ChangedOnDisk``-like ``Damaged`` is raised; the original stays."""
    path, tmp = prep["original"], prep["tmp"]
    keep = False
    try:
        if _interrupt == "before_second_slot_copy":
            keep = True
            raise GuideInterrupted(_interrupt)
        with open(path, "rb") as orig:
            oh, otoc = _read_header_toc(orig)
            if oh["slot_size"] != prep["slot_size"] or _kept(otoc) != _kept(prep["toc"]):
                raise Damaged("the song changed on disk while its guide was written; build again")
            sa, sb = _slot_regions(orig, oh)
            with open(tmp, "r+b") as t:
                t.seek(ALIGN)
                t.write(sa)
                t.write(sb)
                t.flush()
                os.fsync(t.fileno())
            if _interrupt == "truncated_temp":
                with open(tmp, "r+b") as t:
                    t.truncate(prep["file_length"] // 2)
                keep = True
                raise GuideInterrupted(_interrupt)
            with open(tmp, "rb") as t:
                th, ttoc = _read_header_toc(t)
                if ttoc != prep["toc"] or os.fstat(t.fileno()).st_size != prep["file_length"]:
                    raise Damaged("the temporary file does not read back as written")
                for cid, want in ((GUIDE_CHUNK, prep["guide_crc"]), (CURVE_CHUNK, prep["curve_crc"])):
                    e = next(c for c in ttoc["chunks"] if c["id"] == cid)
                    t.seek(e["offset"])
                    if _crc32(t.read(e["length"])) != want:
                        raise Damaged(f"chunk {cid!r} did not read back")
                if _slot_regions(t, th) != (sa, sb):
                    raise Damaged("settings slots did not read back")
            oh2, _ = _read_header_toc(orig)
            if oh2 != oh or _slot_regions(orig, oh2) != (sa, sb):
                raise Damaged("the song changed on disk while its guide was written; build again")
        if _interrupt == "before_rename":
            keep = True
            raise GuideInterrupted(_interrupt)
        os.replace(tmp, path)
        keep = True
        if hasattr(os, "O_DIRECTORY"):
            try:
                fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)
            except OSError:
                pass
    finally:
        if not keep:
            try:
                tmp.unlink()
            except FileNotFoundError:
                pass


def add_guide(path, guide_json: bytes, curve: bytes, *, _interrupt: str | None = None) -> None:
    """Adds or replaces the guide by the raw-copy rewrite of section 7.1 (for a song
    nobody else has open). Every other chunk and both slot regions stay byte-identical."""
    prep = prepare_guide_rewrite(path, guide_json, curve, _interrupt=_interrupt)
    finish_guide_rewrite(prep, _interrupt=_interrupt)


# ----------------------------------------------------------------------------- practice

def round_practice_result(r: dict) -> dict:
    """The number-precision rules of section 11 (part of the format): ``overall`` one
    decimal; ``sung_share`` / ``voiced_share`` two; ``tendency_cents`` and every row's
    score and tendency whole numbers; row ``start`` one decimal."""
    r = json.loads(json.dumps(r))
    if _num(r.get("overall")):
        r["overall"] = round_half_away(r["overall"], 1)
    for k in ("sung_share", "voiced_share"):
        if _num(r.get(k)):
            r[k] = round_half_away(r[k], 2)
    if _num(r.get("tendency_cents")):
        r["tendency_cents"] = int(round_half_away(r["tendency_cents"], 0))
    lines = r.get("lines")
    if isinstance(lines, dict):
        rows = []
        for row in lines.get("rows", []):
            row = list(row)
            row[1] = round_half_away(row[1], 1)
            if _num(row[2]):
                row[2] = int(round_half_away(row[2], 0))
            if _num(row[4]):
                row[4] = int(round_half_away(row[4], 0))
            rows.append(row)
        lines["rows"] = rows
    return r


def _eligible_best(r: dict) -> bool:
    return (_num(r.get("sung_share")) and r["sung_share"] >= 0.9 and _num(r.get("overall"))
            and "untrusted" not in r.get("trust", []))


def practice_add_result(p: dict, r: dict) -> None:
    """Adds a take: newest first in ``recent`` (at most 20); ``best`` = the highest
    ``overall`` among results with ``sung_share >= 0.9`` that are not ``untrusted``."""
    r = round_practice_result(r)
    best = p.get("best")
    if _eligible_best(r) and (not isinstance(best, dict) or r["overall"] > (best.get("overall") or float("-inf"))):
        p["best"] = r
    p["recent"] = ([r] + list(p.get("recent", [])))[:MAX_RECENT]


def practice_shrink_once(p: dict) -> bool:
    """One step of the size rule: the rows of the oldest ``recent`` result that has rows,
    else the oldest ``recent`` result. ``best`` is never dropped."""
    for r in reversed(p.get("recent", [])):
        lines = r.get("lines")
        if isinstance(lines, dict) and lines.get("rows"):
            lines["rows"] = []
            return True
    if p.get("recent"):
        p["recent"].pop()
        return True
    return False


def save_practice(path, edit: Callable[[dict], None]) -> tuple[int, int]:
    """Saves practice data by the slot mechanism (never a whole-file rewrite): when the
    state does not fit a slot, results are dropped by the size rule until it does."""
    for steps in range(2 * MAX_RECENT + 2):
        def apply(state, steps=steps):
            p = state.get(PRACTICE_KEY)
            if not isinstance(p, dict):
                p = {"practice_version": 1, "octave": {"mode": "auto", "shift": 0}, "strictness": None,
                     "excluded": [], "lane": {}, "best": None, "recent": []}
            edit(p)
            for _ in range(steps):
                if not practice_shrink_once(p):
                    break
            if isinstance(p.get("best"), dict):
                p["best"] = round_practice_result(p["best"])
            p["recent"] = [round_practice_result(r) for r in p.get("recent", [])][:MAX_RECENT]
            state[PRACTICE_KEY] = p
        try:
            return update_user_state(path, apply)
        except SlotPayloadTooLarge:
            continue
    raise SlotPayloadTooLarge("practice data does not fit a slot even with every recent result dropped")


def validate_practice(v) -> list[str]:
    """The section 11 type checks (``jukeoke-cli verify``); unknown keys are allowed."""
    e = []
    if not isinstance(v, dict):
        return ["practice: not a JSON object"]
    oc = v.get("octave")
    if oc is not None:
        if not isinstance(oc, dict):
            e.append("practice: octave must be an object")
        else:
            if oc.get("mode") not in ("auto", "fixed", "strict"):
                e.append("practice: octave.mode must be auto, fixed or strict")
            if not (isinstance(oc.get("shift"), int) and -2 <= oc["shift"] <= 2):
                e.append("practice: octave.shift must be an integer -2..2")
    if v.get("strictness") not in (None, "relaxed", "standard", "strict"):
        e.append("practice: strictness must be relaxed, standard, strict or null")
    ex = v.get("excluded")
    if ex is not None:
        if not isinstance(ex, list):
            e.append("practice: excluded must be a list")
        else:
            for i, s in enumerate(ex):
                if not (isinstance(s, dict) and _num(s.get("start")) and _num(s.get("end")) and s["start"] <= s["end"]):
                    e.append(f"practice: excluded[{i}] needs numbers start <= end")
    rec = v.get("recent")
    if rec is not None:
        if not isinstance(rec, list):
            e.append("practice: recent must be a list")
        elif len(rec) > MAX_RECENT:
            e.append(f"practice: {len(rec)} recent results (at most {MAX_RECENT})")
    return e

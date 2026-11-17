# SPDX-License-Identifier: GPL-3.0-or-later
"""Makes damaged copies of a good song file, checks the Python reader's verdict on
each, and lists them for the Rust verifier (run.sh checks Rust reaches the same
verdict).

    python -I tests/conformance/corrupt.py <good.jksong> <out-dir>

Writes <out-dir>/cases.tsv: file name, expected Rust error tag (or "-" when the file
must verify), expected verifier exit status.
"""

from __future__ import annotations

import json
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python -I does not add it
from common import check, finish, jksong


def fix_header_crc(b: bytearray) -> None:
    struct.pack_into("<I", b, 124, zlib.crc32(bytes(b[:124])) & 0xFFFFFFFF)


def with_toc(good: bytes, edit) -> bytes:
    """Applies ``edit`` to the TOC JSON and re-stamps TOC length/CRC, file length and
    header CRC, so only the edit itself is wrong."""
    hdr = jksong._parse_header(good[:4096], len(good))
    toc = json.loads(good[hdr["toc_offset"]:hdr["toc_offset"] + hdr["toc_length"]])
    edit(toc)
    t = json.dumps(toc).encode()
    b = bytearray(good[:hdr["toc_offset"]] + t)
    struct.pack_into("<QI", b, 24, len(t), zlib.crc32(t) & 0xFFFFFFFF)
    struct.pack_into("<Q", b, 56, len(b))
    fix_header_crc(b)
    return bytes(b)


def main(good_path: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    good = good_path.read_bytes()
    s = jksong.Song(good_path)
    hdr = s.header
    drums = s.chunk_entry("stem.drums")
    cases = []

    def flip(i, mask=0x01):
        b = bytearray(good)
        b[i] ^= mask
        return bytes(b)

    def header_field(fmt, off, value, extra=None):
        b = bytearray(good)
        struct.pack_into(fmt, b, off, value)
        if extra:
            extra(b)
        fix_header_crc(b)
        return bytes(b)

    def oob(toc):
        for c in toc["chunks"]:
            if c["id"] == "stem.other":
                c["offset"] = len(good) + 8192

    def drop_drums(toc):
        toc["chunks"] = [c for c in toc["chunks"] if c["id"] != "stem.drums"]

    def future(toc):
        toc["x_future_toc_key"] = {"a": 1}

    def reserved(b):
        b[64:72] = b"FUTURE!!"

    # (name, bytes, python exception or None, rust tag, rust exit status)
    cases = [
        ("bad_magic", flip(1, 0x20), jksong.NotASongFile, "[NotASongFile]", 1),
        ("crlf_damage", good.replace(b"\r\n", b"\n", 1), jksong.NotASongFile, "[NotASongFile]", 1),
        ("header_byte_flip", flip(44), jksong.Damaged, "[Damaged]", 1),
        ("header_crc_flip", flip(125), jksong.Damaged, "[Damaged]", 1),
        ("truncated_half", good[: len(good) // 2], jksong.Truncated, "[Truncated]", 1),
        ("truncated_100", good[:100], jksong.Truncated, "[Truncated]", 1),
        ("truncated_last_byte", good[:-1], jksong.Truncated, "[Truncated]", 1),
        ("toc_crc", flip(hdr["toc_offset"] + 7), jksong.Damaged, "[Damaged]", 1),
        ("chunk_out_of_bounds", with_toc(good, oob), jksong.Damaged, "[Damaged]", 1),
        ("missing_stem_chunk", with_toc(good, drop_drums), jksong.MissingChunk, "[MissingChunk]", 1),
        ("newer_major", header_field("<H", 8, 2), jksong.NewerVersion, "[NewerVersion]", 1),
        ("stem_bit_rot", flip(drums["offset"] + drums["length"] // 2, 0x10), None, "chunk 'stem.drums' CRC-32 mismatch", 1),
        ("newer_minor", with_toc(header_field("<H", 10, 5, reserved), future), None, "-", 0),
    ]
    lines = []
    for name, data, exc, tag, status in cases:
        p = out / f"{name}.jksong"
        p.write_bytes(data)
        try:
            song = jksong.Song(p)
            verdict = None
            if name == "stem_bit_rot":
                try:
                    song.read_chunk("stem.drums")
                except jksong.Damaged:
                    verdict = "crc"
        except jksong.JkSongError as e:
            song, verdict = None, type(e)
        if exc is not None:
            check(verdict is exc, f"python: {name} -> {exc.__name__} ({exc.kind})")
        elif name == "stem_bit_rot":
            check(verdict == "crc", "python: stem_bit_rot opens, and reading the stem reports a CRC mismatch")
        else:
            check(song is not None and song.header["minor"] == 5, "python: newer minor version opens")
        lines.append(f"{p.name}\t{tag}\t{status}")
    (out / "cases.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    finish("corrupt")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))

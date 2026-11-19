# SPDX-License-Identifier: GPL-3.0-or-later
"""The Python side of the 1.1 conformance steps (melody guide, SONG_FORMAT 7.1 / 9 / 10).

    python -I tests/conformance/guide_py.py fingerprint SONG OUT.json
    python -I tests/conformance/guide_py.py add SONG
    python -I tests/conformance/guide_py.py check SONG BEFORE.json
    python -I tests/conformance/guide_py.py interrupt SONG DIR
    python -I tests/conformance/guide_py.py check-interrupted DIR

``add`` writes a guide with the Python writer (notes at the synthetic melody of
synth.py, word references by the Python rule, a curve of the right length) by the
raw-copy rewrite. ``check`` reads a guide another implementation wrote: the section 9
checks, the curve, the word references recomputed by the Python rule, and every other
chunk and both slot regions against BEFORE by SHA-256. ``interrupt`` stops the Python
rewrite at every point of section 7.1 on a copy each; ``check-interrupted`` checks that
each copy is still the old file and opens. The Rust twin is
crates/jk-guide/examples/guide_conformance.rs. Prints no lyrics.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python -I does not add it
from common import check, finish, jksong
from synth import lyric_timing


def fingerprint(song: Path) -> dict:
    b = song.read_bytes()
    s = jksong.Song(song)
    h = s.header

    def region(o):
        return hashlib.sha256(b[o:o + h["slot_size"]]).hexdigest()

    chunks = [[c["id"], c["offset"], hashlib.sha256(b[c["offset"]:c["offset"] + c["length"]]).hexdigest()]
              for c in s.toc["chunks"]]
    return {"chunks": chunks, "slot_a": region(h["slot_a_offset"]), "slot_b": region(h["slot_b_offset"])}


def without_guide(fp: dict) -> list:
    return [c for c in fp["chunks"] if c[0] not in ("guide", "guide.curve")]


def python_guide(song: Path) -> tuple[bytes, bytes]:
    """A guide written by the Python writer: the synthetic melody's notes (truth from
    synth.py), word references by the Python rule, the real lead stem's identity."""
    s = jksong.Song(song)
    dur = s.manifest["duration_seconds"]
    _, notes = lyric_timing()
    lead = s.chunk_entry(s.stem("lead_vocal")["chunk"])
    doc = {
        "guide_version": 1, "hop_s": 0.01, "low_confidence": 0.5,
        "tuning": {"cents": 0.0, "strength": 1.0, "use_cents": 0.0},
        "range": None, "counts": None, "curve": "guide.curve",
        "notes": [], "spans": [], "words": None,
        "provenance": {
            "method": "py-conformance", "version": "0.1.0", "parameters": {"tuning_min_strength": 0.3},
            "source": {"role": "lead_vocal", "chunk": s.stem("lead_vocal")["chunk"], "length": lead["length"],
                       "crc32": lead["crc32"], "sha256": hashlib.sha256(s.read_chunk(lead["id"])).hexdigest(),
                       "sample_rate": s.manifest["sample_rate"], "channels": s.manifest["channels"]},
            "analysis_rate_hz": None, "app": {"name": "conformance", "version": "0", "platform": sys.platform,
                                               "arch": "any"},
            "created_utc": jksong.utc_now(), "seconds": 0.0,
        },
    }
    prev_end = 0.0
    for n in notes:
        start, end, hz = float(n[0]), float(n[1]), float(n[2])      # synth.py: (start, end, Hz)
        midi = float(69.0 + 12.0 * np.log2(hz / 440.0))
        start = max(jksong.round3(start), prev_end)
        end = min(jksong.round3(end), dur)
        if end <= start:
            continue
        doc["notes"].append({"start": start, "end": end, "midi": jksong.round3(midi), "confidence": 0.95,
                             "kind": "pitched", "octave": "ok", "glide_in": 0.0})
        prev_end = end
    jksong.set_word_refs(doc, s.effective_lyrics(), s.user_state.get("lyrics_override") is not None,
                         int(s.effective_settings().get("lyrics_offset_ms", 0)))
    pitched = [n["midi"] for n in doc["notes"]]
    doc["range"] = {"low": min(pitched), "high": max(pitched)} if pitched else None
    doc["counts"] = jksong.guide_counts(doc)
    n = jksong.curve_frames_for(dur)
    t = np.arange(n) * 0.01
    midi = np.zeros(n)
    for x in doc["notes"]:
        midi[(t >= x["start"]) & (t < x["end"])] = x["midi"]
    curve = jksong.encode_curve(midi, (midi > 0).astype(float), np.where(midi > 0, -18.0, -120.0))
    return jksong._json_bytes(doc), curve


POINTS = jksong.INTERRUPT_POINTS


def main(argv: list[str]) -> None:
    cmd = argv[0]
    if cmd == "fingerprint":
        Path(argv[2]).write_text(json.dumps(fingerprint(Path(argv[1])), indent=1))
        return
    if cmd == "add":
        song = Path(argv[1])
        g, c = python_guide(song)
        jksong.add_guide(song, g, c)
        doc = json.loads(g)
        print(f"  Python added a guide: {len(doc['notes'])} notes, "
              f"{sum(1 for n in doc['notes'] if 'word' in n)} with a word")
        return
    if cmd == "check":
        song, before = Path(argv[1]), json.loads(Path(argv[2]).read_text())
        s = jksong.Song(song)
        doc = s.guide()
        dur = s.manifest["duration_seconds"]
        errs, warns = jksong.validate_guide(doc, dur)
        check(not errs and not warns, f"Python's section 9 checks of the other writer's guide: {errs} {warns}")
        curve = s.guide_curve()
        check(curve is not None and not jksong.validate_curve(curve, dur), "the curve decodes and checks in Python")
        lyr = s.effective_lyrics()
        off = int(s.effective_settings().get("lyrics_offset_ms", 0))
        check(jksong.word_refs_valid(doc, lyr, off), "stored word references are valid by the cache rule")
        mine = json.loads(json.dumps(doc))
        jksong.set_word_refs(mine, lyr, s.user_state.get("lyrics_override") is not None, off)
        a = [n.get("word") for n in doc["notes"]]
        b = [n.get("word") for n in mine["notes"]]
        with_word = sum(1 for x in a if x is not None)
        check(a == b and with_word > 0, f"word references: Python's rule gives the same {with_word} of {len(a)} notes")
        check(doc.get("counts") == jksong.guide_counts(doc), "counts agree with the notes")
        p = doc["provenance"]
        check(jksong.guide_state(s, p["method"], p["version"]) == "ready", "guide_state is ready (source length/CRC match)")
        check(hashlib.sha256(s.read_chunk(p["source"]["chunk"])).hexdigest() == p["source"]["sha256"],
              "provenance.source.sha256 is the lead chunk's SHA-256 (deep check)")
        after = fingerprint(song)
        check(without_guide(after) == without_guide(before), "every other chunk: same SHA-256 at the same offset")
        check((after["slot_a"], after["slot_b"]) == (before["slot_a"], before["slot_b"]), "both slot regions: same SHA-256")
        check(s.header["minor"] >= 1, f"header minor {s.header['minor']} (1.1)")
        finish("guide_py check")
        return
    if cmd == "interrupt":
        song, d = Path(argv[1]), Path(argv[2])
        d.mkdir(parents=True, exist_ok=True)
        g, c = python_guide(song)
        for point in POINTS:
            copy = d / f"{point}.jksong"
            shutil.copyfile(song, copy)
            (d / f"{point}.sha256").write_text(hashlib.sha256(copy.read_bytes()).hexdigest())
            try:
                jksong.add_guide(copy, g, c, _interrupt=point)
                check(False, f"Python interrupted at {point}")
            except jksong.GuideInterrupted:
                check(True, f"Python interrupted at {point}")
        finish("guide_py interrupt")
        return
    if cmd == "check-interrupted":
        d = Path(argv[1])
        n = 0
        for sh in sorted(d.glob("*.sha256")):
            song = sh.with_suffix(".jksong")
            same = hashlib.sha256(song.read_bytes()).hexdigest() == sh.read_text().strip()
            try:
                jksong.Song(song)
                opens = True
            except jksong.JkSongError:
                opens = False
            check(same and opens, f"Python: {song.name} is the old file, byte for byte, and opens")
            n += 1
        listed = jksong.list_song_files(d)
        check(len(listed) == n == len(POINTS), f"Python lists {len(listed)} songs and no temporary file")
        check(len(list(d.glob("*.tmp-*"))) >= 1, "the interrupted writes left their temporary files (ignored)")
        finish("guide_py check-interrupted")
        return
    raise SystemExit("usage: guide_py.py fingerprint|add|check|interrupt|check-interrupted ...")


if __name__ == "__main__":
    main(sys.argv[1:])

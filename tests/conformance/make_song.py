# SPDX-License-Identifier: GPL-3.0-or-later
"""Python -> Rust direction, step 1: write a synthetic song with the Python writer.

    python -I tests/conformance/make_song.py <out.jksong>

Writes two songs from the same synthetic stems:

* <out.jksong> in exact-residual mode (``write_song(source=...)``: the writer computes
  "other" from the source and the other stems' decoded values);
* <out-stem>-plain.jksong with the stems exactly as given (the "other" stem computed
  upstream in float32, as a separation stage would).

For each, <file>.source.f32 holds the source (raw float32 little-endian interleaved)
for the Rust sum check, and <out>.expect.json what the Rust reader must find. Also
measures, with libsndfile/libFLAC as the decoder, how far the stems at unity are from
the source.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python -I does not add it
from common import check, finish, jksong
from synth import png_gradient, synth

SR = 44100


def main(out: Path) -> None:
    stems, source, lyrics = synth(sr=SR, seconds=4.0, seed=11, other_peak=1.21)
    user_state = jksong.default_user_state()
    user_state["settings"]["semitones"] = 2
    user_state["settings"]["levels"] = {"drums": 0.9}
    user_state["metadata_override"] = {"album": "Python album override"}
    user_state["x_py_key"] = {"made_by": "python", "word": "café"}
    user_state["settings"]["x_py_setting"] = [1, 2, 3]
    common_args = dict(
        sample_rate=SR, title="Synthetic café song", artist="Conformance generator", album=None,
        lyrics=lyrics, cover=png_gradient(),
        analysis={"key": {"tonic": "A", "mode": "major", "name": "A major", "confidence": 1.0}, "tempo_bpm": 120.0},
        preset="full", stems_sum_to_source=True,
        provenance={"app": {"name": "conformance", "version": "0"}, "models": [], "resampling": [],
                    "created_utc": jksong.utc_now()},
        user_state=user_state,
        extra_chunks=[("x.python-extra", "application/octet-stream", b"python opaque \x00\xff")],
        manifest_extra={"x_python_note": {"unknown": True}},
    )
    # Main file: exact-residual mode ("other" computed by the writer from the source and
    # the other stems' decoded values).
    t0 = time.perf_counter()
    jksong.write_song(out, stems=[jksong.StemInput(r, None if r == "other" else a) for r, a in stems.items()],
                      source=source, residual_role="other", **common_args)
    print(f"wrote {out} ({out.stat().st_size} bytes) in {time.perf_counter() - t0:.2f} s")
    # Plain file: stems exactly as an upstream stage would hand them over ("other" =
    # source - lead - backing - drums in float32, computed in synth()).
    plain = out.with_name(out.stem + "-plain.jksong")
    jksong.write_song(plain, stems=[jksong.StemInput(r, a) for r, a in stems.items()], **common_args)
    print(f"wrote {plain} ({plain.stat().st_size} bytes)")
    for p in (out, plain):
        np.ascontiguousarray(source.T).astype("<f4").tofile(str(p) + ".source.f32")

    src64 = source.astype(np.float64)
    for p, exact in ((out, True), (plain, False)):
        s = jksong.Song(p)
        check(s.manifest["stems_sum_to_source"] is True, f"{p.name}: manifest stems_sum_to_source")
        gains = {x["role"]: x["gain"] for x in s.manifest["stems"]}
        print("  gains:", gains)
        check(gains["other"] > 1.0 and all(g == 1.0 for r, g in gains.items() if r != "other"),
              f"{p.name}: gain rule: only the clipping stem ('other', peak 1.21) is scaled")
        # libsndfile/libFLAC decode, true integers, summed in float64.
        total = sum(s.read_stem_int(r).T.astype(np.float64) * (gains[r] / 8388608) for r in stems)
        err = float(np.max(np.abs(total - src64)))
        if exact:
            bound = 0.5 / 8388608 * gains["other"] * (1 + 1e-9)
            what = "half an LSB of the residual stem"
        else:
            # each stem's half LSB, plus the float32 rounding of the 3 subtractions that
            # made "other" upstream (|x| < 2: half an ulp = 2^-24 each)
            bound = sum(0.5 / 8388608 * g for g in gains.values()) + 3 * 2 ** -24
            what = "sum of half LSBs + float32 rounding of the upstream residual"
        print(f"  {p.name}: python (libFLAC) decode, sum of stems vs source: max abs error {err:.3e} "
              f"= {err * 8388608:.3f} LSB of 24-bit ({20 * np.log10(err):.1f} dBFS); bound {bound:.3e} ({what})")
        check(err <= bound, f"{p.name}: stems at unity sum back to the source within the bound")
    lines = [" ".join("".join(c[0] for c in w["chars"]) for w in line["words"]) for line in lyrics["lines"]]
    expect = {
        "title": "Synthetic café song",
        "artist": "Conformance generator",
        "album": "Python album override",
        "settings": {"semitones": 2, "levels": {"drums": 0.9}, "x_py_setting": [1, 2, 3], "backing_on": True},
        "lyrics_lines": lines,
        "lyrics_graphemes": [[c[0] for c in w["chars"]] for line in lyrics["lines"] for w in line["words"]],
        "user_state_keys": {"x_py_key": {"made_by": "python", "word": "café"}},
        "chunks": {c["id"]: f"{c['crc32']:08x}" for c in jksong.Song(out).toc["chunks"]},
    }
    Path(str(out) + ".expect.json").write_text(json.dumps(expect, ensure_ascii=False, indent=1), encoding="utf-8")
    print("  lyrics lines:", lines)
    finish("make_song")


if __name__ == "__main__":
    main(Path(sys.argv[1]))

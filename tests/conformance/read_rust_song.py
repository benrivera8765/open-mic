# SPDX-License-Identifier: GPL-3.0-or-later
"""Rust -> Python direction: read a file written by the Rust writer
(``cargo run -p jk-format --example jksong-write-test -- <file>``) with the Python
reader and check everything against <file>.expect.json. Then save settings through
the Python slot writer, and write <file>.after-python.expect.json for the Rust
verifier to confirm it reads them.

    python -I tests/conformance/read_rust_song.py <rust.jksong>
"""

from __future__ import annotations

import hashlib
import io
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python -I does not add it
from common import check, finish, jksong


def tri(i: np.ndarray, period: int, amp: int) -> np.ndarray:
    """The integer triangle wave of examples/jksong-write-test.rs (all values >= 0
    before the final subtraction, so floor division equals Rust's truncation)."""
    p = i % period
    half = period // 2
    v = np.where(p < half, p, period - p)
    return (v * 2 * amp) // half - amp


def main(path: Path) -> None:
    exp = json.loads(Path(str(path) + ".expect.json").read_text(encoding="utf-8"))
    s = jksong.Song(path)
    m = s.manifest
    check((s.header["major"], s.header["minor"]) == (1, 1), "header version 1.1 (Rust writes 1.1 since Work Order 2)")
    check(m["sample_rate"] == exp["sample_rate"] and m["channels"] == exp["channels"]
          and m["duration_frames"] == exp["frames"], "manifest rate / channels / frames")
    check(m["title"] == exp["manifest_title"] and m.get("x_manifest_note") == exp["manifest_extra"]["x_manifest_note"],
          "manifest title and unknown manifest key")
    check(s.active_slot == exp["slot"]["active"] and s.slots[exp["slot"]["active"]][1] == exp["slot"]["sequence"],
          f"active slot {exp['slot']} as Rust reported")
    check(s.slots[0][0] == "valid" and s.slots[0][1] == 1, "slot A still holds the initial state (sequence 1)")
    meta = s.effective_metadata()
    check(meta == {"title": exp["title"], "artist": exp["artist"], "album": exp["album"]},
          f"effective metadata (override wins): {meta}")
    st = s.effective_settings()
    check(all(st[k] == v for k, v in exp["settings"].items()), f"settings from the Rust slot write: {exp['settings']}")
    check(all(s.user_state.get(k) == v for k, v in exp["user_state_keys"].items()), "unknown user-state key from Rust")
    lyr = s.effective_lyrics()
    lines = [" ".join("".join(c[0] for c in w["chars"]) for w in line["words"]) for line in lyr["lines"]]
    check(lines == exp["lyrics_lines"], f"lyrics lines {lines}")
    check([[c[0] for c in w["chars"]] for line in lyr["lines"] for w in line["words"]] == exp["lyrics_graphemes"],
          "grapheme strings (precomposed, combining, ZWJ emoji) identical")
    check(jksong.validate_lyrics(lyr, m["duration_seconds"]) == [], "lyrics timing valid per section 5")
    check(lyr["lines"][0].get("x_line_note") == "rust" and lyr.get("x_lyrics_note") == "from rust",
          "unknown lyrics keys readable")
    check(s.read_chunk(exp["unknown_chunk"]["id"]).hex() == exp["unknown_chunk"]["hex"], "unknown chunk bytes")
    check(s.read_chunk("cover").hex() == exp["cover_hex"] and s.chunk_entry("cover")["type"] == "image/png", "cover")
    check(all(c["offset"] % 4096 == 0 for c in s.toc["chunks"]), "every chunk 4096-aligned")

    i = np.arange(exp["frames"], dtype=np.int64)
    for k, (role, gain) in enumerate(zip(exp["roles"], exp["gains"])):
        data = s.read_chunk(s.stem(role)["chunk"])
        blocks, _ = jksong._metadata_blocks(data)
        si = jksong.parse_streaminfo(blocks[0][1])
        want = np.stack([tri(i, 100 + 37 * k + 11 * c, (1 << 20) + 12345 * k + 999 * c) for c in range(2)], axis=1)
        got = s.read_stem_int(role)  # decoded by libsndfile/libFLAC
        check(si["total_samples"] == exp["frames"] and si["bits_per_sample"] == 24 and si["sample_rate"] == exp["sample_rate"],
              f"{role}: STREAMINFO {si['sample_rate']} Hz {si['bits_per_sample']} bit {si['total_samples']} frames")
        check(got.shape == want.shape and np.array_equal(got, want), f"{role}: libFLAC decode == Rust's integers, all {want.size} samples")
        le = got.astype("<i4").view(np.uint8).reshape(-1, 4)[:, :3].tobytes()
        check(hashlib.md5(le).digest() == si["md5"], f"{role}: STREAMINFO MD5 (computed by Rust) matches the decode")
        check(s.stem(role)["gain"] == gain, f"{role}: gain {gain}")
        f = s.read_stem(role)
        check(np.array_equal(f, (want.T * (gain / 8388608)).astype(np.float32)), f"{role}: float = int / 2^23 * gain")
        with sf.SoundFile(io.BytesIO(data)) as fh:  # libFLAC seeking in the Rust-written stream
            fh.seek(77_777)
            seek_ok = np.array_equal(fh.read(100, dtype="int32") >> 8, want[77_777:77_877])
        check(seek_ok, f"{role}: libFLAC seek into Rust's stream lands on the right samples")

    # Python saves settings into the Rust-written file; Rust must read them back.
    slot, seq = jksong.update_user_state(path, lambda u: (u["settings"].update(semitones=7, backing_on=False),
                                                          u.update(x_py_touch="café")))
    check((slot, seq) == (1 - exp["slot"]["active"], exp["slot"]["sequence"] + 1),
          f"Python slot write went to the other slot ({slot}) with sequence {seq}")
    after = {
        "title": exp["title"], "artist": exp["artist"], "album": exp["album"],
        "settings": {"semitones": 7, "backing_on": False, "guide_vocal": 0.25, "levels": {"drums": 0.8}},
        "user_state_keys": {"x_py_touch": "café", **exp["user_state_keys"]},
        "lyrics_lines": exp["lyrics_lines"],
    }
    Path(str(path) + ".after-python.expect.json").write_text(json.dumps(after, ensure_ascii=False), encoding="utf-8")
    finish("read_rust_song")


if __name__ == "__main__":
    main(Path(sys.argv[1]))

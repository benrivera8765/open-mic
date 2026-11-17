# SPDX-License-Identifier: GPL-3.0-or-later
"""Python-written file edited by Rust, read back by Python.

    python -I tests/conformance/check_rust_edits.py <edited.jksong> <original.jksong>

<edited> is a copy of <original> (written by make_song.py) on which the Rust example
ran ``--update`` (slot write: semitones 4, lyrics_offset_ms 250, key x_rust_touch) and
then ``--rewrite`` (full atomic rewrite, title changed, key x_rust_rewrite added).
Checks that every chunk except the manifest came through byte for byte, in order,
that unknown chunks and keys written by Python survived both Rust edits, and that
the settings Rust saved are what Python reads.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python -I does not add it
from common import check, finish, jksong


def main(edited: Path, original: Path) -> None:
    a, b = jksong.Song(original), jksong.Song(edited)
    check(a.chunk_ids() == b.chunk_ids(), f"chunk order kept: {b.chunk_ids()}")
    for cid in a.chunk_ids():
        if cid != "manifest":
            check(a.read_chunk(cid) == b.read_chunk(cid), f"chunk {cid!r} byte-identical after the Rust rewrite")
    ma, mb = a.manifest, b.manifest
    check(mb["title"] == ma["title"] + " (rewritten by Rust)" and mb.get("x_rust_rewrite") is True, "Rust's manifest edit")
    check(mb.get("x_python_note") == ma["x_python_note"], "unknown manifest key written by Python survived")
    check(mb["stems"] == ma["stems"], "stem entries (gain, peak, flags) unchanged")
    check(b.read_chunk("x.python-extra") == b"python opaque \x00\xff", "unknown chunk written by Python survived")
    u = b.user_state
    st = b.effective_settings()
    check(st["semitones"] == 4 and st["lyrics_offset_ms"] == 250, "settings saved by Rust's slot writer")
    check(u.get("x_rust_touch") is True, "key added by Rust")
    check(u.get("x_py_key") == a.user_state["x_py_key"], "unknown top-level user-state key from Python kept by Rust")
    check(st.get("x_py_setting") == [1, 2, 3] and st["levels"] == {"drums": 0.9},
          "unknown settings key and levels from Python kept by Rust")
    check(b.effective_metadata()["album"] == "Python album override", "metadata_override kept")
    check(b.slots[0][0] == "valid" and b.slots[0][1] == 1 and b.slots[1][0] == "empty",
          "rewrite stores the state in slot A, sequence 1; slot B empty")
    check(b.effective_lyrics() == a.effective_lyrics(), "lyrics identical")
    finish("check_rust_edits")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))

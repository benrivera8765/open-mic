# SPDX-License-Identifier: GPL-3.0-or-later
"""Shared helpers for the conformance scripts: import paths and small checks."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "studio"))
sys.path.insert(0, str(HERE))

from jukeoke_studio import jksong  # noqa: E402,F401

FAILURES: list[str] = []

# The Windows console code page cannot print "ï" (i + U+0308) and friends.
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="backslashreplace")
    except (AttributeError, ValueError):
        pass


def check(cond: bool, what: str) -> None:
    """Records and prints one check."""
    print(("  ok    " if cond else "  FAIL  ") + what)
    if not cond:
        FAILURES.append(what)


def finish(name: str) -> None:
    if FAILURES:
        print(f"{name}: FAIL ({len(FAILURES)} failed)")
        sys.exit(1)
    print(f"{name}: PASS")

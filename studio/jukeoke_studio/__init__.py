# SPDX-License-Identifier: GPL-3.0-or-later
"""Jukeoke Studio: turns a song into a karaoke song (separation, lyrics, alignment).

Runs as a separate Python process on Windows machines with a CUDA GPU; the player never
imports it. See docs/ARCHITECTURE.md section 5 and docs/STUDIO_PROTOCOL.md.
"""

import sys as _sys

# Portable mode (WORKING ORDER 4.8): never write .pyc files next to the runtime on the
# stick. ``python -I`` ignores PYTHONDONTWRITEBYTECODE, so it is switched off here,
# before any other module of the package is imported.
_sys.dont_write_bytecode = True

__version__ = "0.1.0"

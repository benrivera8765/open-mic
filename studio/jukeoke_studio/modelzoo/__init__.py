# SPDX-License-Identifier: GPL-3.0-or-later
"""Model catalogue, download-on-first-use, and the network definitions we run.

Named ``modelzoo`` rather than ``models`` on purpose: the repository's ``.gitignore``
ignores ``Models/`` (the portable model-weights folder), and git on Windows matches
ignore patterns case-insensitively, so a ``models/`` package would silently never be
committed.
"""

from .registry import MODELS, ModelSpec, WeightFile, get  # noqa: F401

# SPDX-License-Identifier: GPL-3.0-or-later
"""Errors with a protocol error code (docs/STUDIO_PROTOCOL.md section 10) and job hooks.

Standard library only, so every module (including the stand-ins used by tests) can
raise them without importing anything heavy.
"""

from __future__ import annotations

import threading
from typing import Callable


class StudioError(Exception):
    """An error the app shows as it is: ``code`` from the protocol's list, ``message`` in
    plain language for the user, ``detail`` for the log."""

    code = "internal"

    def __init__(self, message: str, detail: str = "", code: str | None = None):
        super().__init__(message)
        self.message = message
        self.detail = detail
        if code:
            self.code = code

    def to_json(self) -> dict:
        return {"code": self.code, "message": self.message, "detail": self.detail}


class Cancelled(StudioError):
    code = "cancelled"

    def __init__(self, message: str = "Cancelled.", detail: str = ""):
        super().__init__(message, detail)


class ModelsMissing(StudioError):
    """Weights are not on disk and the caller did not allow a download. The protocol's
    ``models.download`` is the only path that fetches weights (WORKING ORDER 10)."""

    code = "models_missing"

    def __init__(self, missing: list[str], detail: str = ""):
        names = ", ".join(missing)
        super().__init__(
            "Some models this song needs have not been downloaded yet. Download them from "
            "the Studio screen first (this is the only time the Studio uses the internet).",
            detail or f"missing: {names}")
        self.missing = list(missing)


class ModelChecksum(StudioError):
    code = "model_checksum"


class DecodeFailed(StudioError):
    code = "decode_failed"


class LyricsMissingError(StudioError):
    code = "lyrics_missing"


class AlignmentFailed(StudioError):
    code = "alignment_failed"


class WriteFailed(StudioError):
    code = "write_failed"


class NotFound(StudioError):
    code = "not_found"


class BadRequest(StudioError):
    code = "bad_request"


#: Codes that describe the user's file, choices or computer, not a fault in the Studio:
#: logged as one WARNING line without a traceback (D-173). Anything else (``internal``,
#: ``write_failed``, an exception that is not a StudioError) keeps its traceback.
EXPECTED_CODES = frozenset({
    "decode_failed", "not_found", "lyrics_missing", "alignment_failed", "models_missing",
    "model_checksum", "bad_request", "unknown_type", "protocol_mismatch", "lookup_failed",
    "out_of_memory",
})


def is_expected(err: BaseException) -> bool:
    """True for a StudioError whose code is in :data:`EXPECTED_CODES`."""
    return isinstance(err, StudioError) and err.code in EXPECTED_CODES


class Hooks:
    """Progress and cooperative cancellation for one job.

    ``check()`` raises :class:`Cancelled` once ``cancel()`` was called; long loops (model
    chunks, decoding, encoding, transcription segments) call it between steps.
    ``progress(fraction, message)`` reports progress inside the current stage (0..1).
    """

    def __init__(self, on_progress: Callable[[float, str | None], None] | None = None,
                 cancel_event: threading.Event | None = None):
        self._on_progress = on_progress
        self.cancel_event = cancel_event or threading.Event()

    def cancel(self) -> None:
        self.cancel_event.set()

    @property
    def cancelled(self) -> bool:
        return self.cancel_event.is_set()

    def check(self) -> None:
        if self.cancel_event.is_set():
            raise Cancelled()

    def progress(self, fraction: float, message: str | None = None) -> None:
        self.check()
        if self._on_progress is not None:
            self._on_progress(max(0.0, min(1.0, float(fraction))), message)

    def sub(self, start: float, end: float, message: str | None = None) -> "Hooks":
        """Hooks that map 0..1 onto ``start..end`` of this one (same cancel flag)."""
        parent = self

        def on(f: float, m: str | None) -> None:
            parent.progress(start + (end - start) * f, m if m is not None else message)

        return Hooks(on, self.cancel_event)


NO_HOOKS = Hooks()

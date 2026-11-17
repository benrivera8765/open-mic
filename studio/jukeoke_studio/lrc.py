# SPDX-License-Identifier: GPL-3.0-or-later
"""Parse and format LRC lyrics files.

Supported:

* Line timestamps ``[mm:ss]``, ``[mm:ss.x]``, ``[mm:ss.xx]``, ``[mm:ss.xxx]``, the
  colon variant ``[mm:ss:xx]`` and ``[h:mm:ss.xx]``. Minutes may exceed 59.
* Several timestamps on one line (``[00:12.00][01:10.50]Chorus line``): the line is
  repeated at every time, and the result is sorted by time.
* ID tags ``[ti:]``, ``[ar:]``, ``[al:]``, ``[by:]``, ``[length:]``, ``[offset:]`` and any
  other ``[key:value]`` tag (kept in :attr:`LrcDocument.tags`).
* ``[offset:+/-ms]``. Per the LRC convention a positive offset makes lyrics appear
  *sooner*, so a timestamp ``t`` means ``t - offset/1000`` seconds. The offset is applied
  by :func:`parse_lrc` unless ``apply_offset=False``.
* "Enhanced" (A2) word tags inside a line: ``[00:12.00]<00:12.00>Hello <00:12.50>world``.
  A tag after the last word gives the end of that word.
* Empty timed lines (``[01:23.45]`` with no text) are kept: they mark where the previous
  line ends, which is useful for instrumental breaks.

Everything here is pure text processing (no audio, no network).
"""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field

# A line timestamp. Groups: optional hours, minutes, seconds, optional fraction.
# Accepts '.' or ':' before the fraction (some editors write [mm:ss:xx]).
# Three forms: mm:ss[.fff] | mm:ss:cc (colon before the fraction, no hours) |
# h:mm:ss.fff (hours only when a '.' fraction follows, so [00:05:50] is 5.5 s, the
# common editor spelling, not 5 min 50 s). Groups: hours, minutes, seconds, fraction.
_TS_BODY = (r"(?:(\d+):(\d+):(\d{1,2})\.(\d{1,3})|()(\d+):(\d{1,2})(?:[.:](\d{1,3}))?)")
_LINE_TS_RE = re.compile(r"\[\s*" + _TS_BODY + r"\s*\]")
_WORD_TS_RE = re.compile(r"<\s*" + _TS_BODY + r"\s*>")
# An ID tag such as [ar: Artist] or [offset:+250]. Keys are letters only (by convention
# they are short lowercase words), which keeps them apart from timestamps.
_TAG_RE = re.compile(r"^\s*\[\s*([A-Za-z][A-Za-z0-9_#-]*)\s*:(.*)\]\s*$")


@dataclass
class LrcWord:
    """One word of an enhanced LRC line. ``end`` is None unless a closing tag gave it."""

    text: str
    start: float
    end: float | None = None


@dataclass
class LrcLine:
    """One timed line. ``words`` is filled only for enhanced LRC lines."""

    time: float
    text: str
    words: list[LrcWord] | None = None


@dataclass
class LrcDocument:
    lines: list[LrcLine] = field(default_factory=list)
    tags: dict[str, str] = field(default_factory=dict)
    # Offset in milliseconds as written in the file (already applied to the times when
    # parse_lrc(apply_offset=True), the default).
    offset_ms: int = 0
    offset_applied: bool = False

    def text_lines(self) -> list[str]:
        """The non-empty lyric lines in time order, without timestamps."""
        return [ln.text for ln in self.lines if ln.text.strip()]


def _ts_to_seconds(*groups: str | None) -> float:
    # The regex has two alternatives of four groups each; use whichever matched.
    h, m, s, frac = groups[:4] if groups[1] is not None else groups[4:]
    seconds = (int(h) * 3600 if h else 0) + int(m) * 60 + int(s)
    if frac:
        # "5" -> 0.5, "05" -> 0.05, "005" -> 0.005
        seconds += int(frac) / (10 ** len(frac))
    return float(seconds)


def has_timestamps(text: str) -> bool:
    """True when the text contains at least one LRC line timestamp at a line start."""
    for raw in text.splitlines():
        if _LINE_TS_RE.match(raw.lstrip("\ufeff").strip()):
            return True
    return False


def _parse_enhanced(body: str) -> tuple[str, list[LrcWord] | None]:
    """Split an enhanced-LRC line body into plain text and timed words.

    ``<t1>Hello <t2>world<t3>`` -> ("Hello world", [Hello@t1..t2, world@t2..t3]).
    Text before the first word tag (rare) is attached to the first word.
    """
    if not _WORD_TS_RE.search(body):
        return body.strip(), None
    pieces: list[tuple[float, str]] = []
    pos = 0
    lead = ""
    current_time: float | None = None
    for m in _WORD_TS_RE.finditer(body):
        chunk = body[pos:m.start()]
        if current_time is None:
            lead += chunk
        else:
            pieces.append((current_time, chunk))
        current_time = _ts_to_seconds(*m.groups())
        pos = m.end()
    trailing = body[pos:]
    assert current_time is not None
    pieces.append((current_time, trailing))

    # Each piece is (tag time, text up to the next tag). A piece may hold several words
    # (when the file tags phrases) or none (a closing tag). Split pieces into words; the
    # first word of a piece starts at the tag time, later words in the same piece get
    # no own time and are interpolated by the caller (here: share the piece start).
    words: list[LrcWord] = []
    for i, (t, chunk) in enumerate(pieces):
        if i == 0 and lead.strip():
            chunk = lead + chunk
        toks = chunk.split()
        if not toks:
            # A closing tag: it ends the previous word.
            if words and words[-1].end is None:
                words[-1].end = t
            continue
        for k, tok in enumerate(toks):
            words.append(LrcWord(text=tok, start=t))
    # A word's end, if not given by a closing tag, is the next word's start.
    for a, b in zip(words, words[1:]):
        if a.end is None and b.start > a.start:
            a.end = b.start
    text = " ".join(w.text for w in words)
    return text, (words or None)


def parse_lrc(text: str, *, apply_offset: bool = True) -> LrcDocument:
    """Parse LRC text. Lines without a timestamp (and that are not tags) are ignored.

    Returned lines are sorted by time (stable, so equal times keep file order).
    """
    doc = LrcDocument()
    timed: list[tuple[float, int, LrcLine]] = []
    order = 0
    for raw in text.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        line = raw.lstrip("\ufeff").strip()
        if not line:
            continue
        # Collect all leading timestamps.
        times: list[float] = []
        pos = 0
        while True:
            m = _LINE_TS_RE.match(line, pos)
            if not m:
                break
            times.append(_ts_to_seconds(*m.groups()))
            pos = m.end()
            # Allow whitespace between consecutive timestamps.
            while pos < len(line) and line[pos] in " \t":
                pos += 1
        if not times:
            tm = _TAG_RE.match(line)
            if tm:
                key = tm.group(1).strip().lower()
                doc.tags[key] = tm.group(2).strip()
            continue
        body = line[pos:]
        body_text, words = _parse_enhanced(body)
        for t in times:
            # Copy words per repeat so that later offset shifts do not alias. Word tags
            # are absolute times for the first timestamp; repeats are shifted along.
            wcopy = None
            if words is not None:
                delta = t - times[0]
                wcopy = [
                    LrcWord(w.text, w.start + delta, None if w.end is None else w.end + delta)
                    for w in words
                ]
            timed.append((t, order, LrcLine(time=t, text=body_text, words=wcopy)))
            order += 1

    timed.sort(key=lambda x: (x[0], x[1]))
    doc.lines = [ln for _, _, ln in timed]

    off = doc.tags.get("offset")
    if off is not None:
        try:
            doc.offset_ms = int(float(off.replace("+", "").strip()))
        except ValueError:
            doc.offset_ms = 0
    if apply_offset and doc.offset_ms:
        shift = doc.offset_ms / 1000.0
        for ln in doc.lines:
            ln.time = max(0.0, ln.time - shift)
            if ln.words:
                for w in ln.words:
                    w.start = max(0.0, w.start - shift)
                    if w.end is not None:
                        w.end = max(0.0, w.end - shift)
        doc.offset_applied = True
    return doc


def format_timestamp(seconds: float, *, brackets: str = "[]") -> str:
    """Format seconds as ``[mm:ss.xx]`` (centiseconds, rounded to nearest)."""
    if not math.isfinite(seconds) or seconds < 0:
        seconds = 0.0
    cs = int(round(seconds * 100))
    m, rem = divmod(cs, 6000)
    s, c = divmod(rem, 100)
    return f"{brackets[0]}{m:02d}:{s:02d}.{c:02d}{brackets[1]}"


def format_lrc(doc_or_lines, *, tags: dict[str, str] | None = None, enhanced: bool = False) -> str:
    """Format an :class:`LrcDocument` (or a list of :class:`LrcLine`) as LRC text.

    Times are written as they are (no offset tag is emitted unless it is in ``tags``,
    because the times are normally already offset-corrected). With ``enhanced=True``
    and word times present, ``<mm:ss.xx>`` word tags are written, plus a closing tag
    for the last word's end.
    """
    if isinstance(doc_or_lines, LrcDocument):
        lines = doc_or_lines.lines
        all_tags = dict(doc_or_lines.tags)
        if doc_or_lines.offset_applied:
            all_tags.pop("offset", None)
    else:
        lines = list(doc_or_lines)
        all_tags = {}
    if tags:
        all_tags.update(tags)
    out: list[str] = []
    for k in ("ti", "ar", "al", "by", "length", "offset"):
        if k in all_tags:
            out.append(f"[{k}:{all_tags[k]}]")
    for k, v in all_tags.items():
        if k not in ("ti", "ar", "al", "by", "length", "offset"):
            out.append(f"[{k}:{v}]")
    for ln in lines:
        if enhanced and ln.words:
            parts = []
            for w in ln.words:
                parts.append(format_timestamp(w.start, brackets="<>") + w.text)
            body = " ".join(parts)
            last_end = ln.words[-1].end
            if last_end is not None:
                body += " " + format_timestamp(last_end, brackets="<>")
            out.append(format_timestamp(ln.time) + body)
        else:
            out.append(format_timestamp(ln.time) + ln.text)
    return "\n".join(out) + "\n"


def lyrics_json_to_lrc(lyrics: dict, *, enhanced: bool = True, offset_s: float = 0.0) -> str:
    """Export a SONG_FORMAT section 5 lyrics document as (enhanced) LRC.

    ``offset_s`` is added to every time (e.g. a user's timing offset).
    """
    lines: list[LrcLine] = []
    for ln in lyrics.get("lines", []):
        words = []
        for w in ln.get("words", []):
            text = "".join(c[0] for c in w.get("chars", []))
            words.append(LrcWord(text, float(w["start"]) + offset_s, float(w["end"]) + offset_s))
        text = " ".join(w.text for w in words)
        lines.append(LrcLine(float(ln["start"]) + offset_s, text, words or None))
    return format_lrc(lines, enhanced=enhanced)

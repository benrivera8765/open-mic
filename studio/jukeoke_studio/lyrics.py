# SPDX-License-Identifier: GPL-3.0-or-later
"""Lyrics sources and text normalisation (Studio pipeline step 3, WORK_ORDER 4.1).

Sources, in the WORK_ORDER order:

1. Text or ``.lrc`` supplied by the user           -> :func:`from_text`, :func:`from_lrc`
2. Lyrics embedded in the file's tags (mutagen)    -> :func:`read_embedded_lyrics`
3. Online lookup, only when the user asks (LRCLIB) -> :func:`lookup_lrclib`
4. Transcription of the isolated lead vocal        -> ``transcribe.py``

:func:`resolve_lyrics` applies the order of the Studio protocol's ``lyrics.mode``.

Normalisation: text is split into lines and words. Punctuation stays attached to its
word ("world," is one word), and every word is split into Unicode extended grapheme
clusters (``regex`` ``\\X``), which are the "letters" of docs/SONG_FORMAT.md section 5.

The network is used only by :func:`lookup_lrclib`, and only when it is called.
"""

from __future__ import annotations

import json
import logging
import random
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

import regex

from . import lrc as lrcmod

log = logging.getLogger(__name__)

SOURCE_USER_TEXT = "user_text"
SOURCE_LRC = "lrc"
SOURCE_EMBEDDED = "embedded"
SOURCE_TRANSCRIBED = "transcribed"


class LyricsMissing(Exception):
    """No usable lyrics were found or given (protocol error code ``lyrics_missing``)."""


class LyricsLookupError(Exception):
    """The online lookup failed (network error, bad response). Message is plain language."""


# --------------------------------------------------------------------------------------
# Data model
# --------------------------------------------------------------------------------------


@dataclass
class Word:
    """A word as written, with its grapheme clusters. Times are optional hints
    (from enhanced LRC or a transcript); the aligner produces the real times."""

    text: str
    graphemes: list[str]
    start: float | None = None
    end: float | None = None


@dataclass
class Line:
    words: list[Word]
    # Optional line timing from the source (LRC timestamp, transcript segment). The
    # aligner uses these as soft anchors (windows), never as final times.
    start: float | None = None
    end: float | None = None

    @property
    def text(self) -> str:
        return " ".join(w.text for w in self.words)


@dataclass
class Lyrics:
    lines: list[Line]
    source: str
    checked: bool
    language: str | None = "en"
    meta: dict = field(default_factory=dict)

    @property
    def synced(self) -> bool:
        """True when the source carried line times (LRC, SYLT, transcript)."""
        return any(ln.start is not None for ln in self.lines)

    def text(self) -> str:
        return "\n".join(ln.text for ln in self.lines)

    def word_count(self) -> int:
        return sum(len(ln.words) for ln in self.lines)


# --------------------------------------------------------------------------------------
# Normalisation
# --------------------------------------------------------------------------------------

_GRAPHEME_RE = regex.compile(r"\X")
# Characters that are invisible formatting noise in pasted lyrics. ZWJ/ZWNJ (U+200D,
# U+200C) are kept because they are part of emoji and Indic grapheme clusters.
_INVISIBLE = dict.fromkeys(map(ord, "\u200b\u2060\ufeff\u00ad\u200e\u200f"), None)
# Section headers that are not sung: "[Chorus]", "[Verse 1: Name]", "(Chorus)",
# "Chorus:", "Verse 2", "Bridge x2". A line entirely in square brackets is always a
# header (lyrics sites use them only for that). Parenthesised lines are kept unless
# they are one of the known header words, because "(ooh, baby)" is sung.
_SECTION_WORDS = (
    r"(?:verse|chorus|pre[- ]?chorus|post[- ]?chorus|bridge|intro|outro|hook|refrain|"
    r"interlude|instrumental|solo|breakdown|coda|break|repeat|spoken|rap|ad[- ]?libs?)"
)
# Without brackets only the unmistakable header words count ("Chorus", "Verse 2:",
# "Bridge x2"); a bare "Break" or "Solo" on its own line may well be sung.
_BARE_SECTION_WORDS = (
    r"(?:verse|chorus|pre[- ]?chorus|post[- ]?chorus|bridge|intro|outro|refrain|interlude|instrumental)"
)
_SECTION_RE = regex.compile(
    r"^\s*(?:\[[^\]]*\]|[({]\s*" + _SECTION_WORDS + r"\b[^)}]*[)}]|"
    + _BARE_SECTION_WORDS + r"(?:\s*\d+)?(?:\s*[x×]\s*\d+)?\s*:?|"
    + _SECTION_WORDS + r"(?:\s*\d+)?(?:\s*[x×]\s*\d+)?\s*:)\s*$",
    regex.IGNORECASE,
)


def split_graphemes(word: str) -> list[str]:
    """Split into Unicode extended grapheme clusters (UAX #29)."""
    return _GRAPHEME_RE.findall(word)


def clean_line(text: str) -> str:
    """NFC-normalise one line, turn any whitespace run into one space, drop invisibles."""
    text = unicodedata.normalize("NFC", text).translate(_INVISIBLE)
    return " ".join(text.split())


def is_section_header(text: str) -> bool:
    return bool(_SECTION_RE.match(text))


def make_line(text: str, start: float | None = None, end: float | None = None) -> Line | None:
    """Build a :class:`Line` from raw text; None when nothing singable is left."""
    t = clean_line(text)
    if not t or is_section_header(t):
        return None
    words = [Word(w, split_graphemes(w)) for w in t.split(" ")]
    return Line(words=words, start=start, end=end)


def parse_plain_text(text: str) -> list[Line]:
    """Plain lyrics -> lines. Blank lines (stanza breaks) and section headers are dropped."""
    out = []
    for raw in text.replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        ln = make_line(raw)
        if ln is not None:
            out.append(ln)
    return out


def lines_from_lrc_doc(doc: lrcmod.LrcDocument) -> list[Line]:
    """LRC lines -> :class:`Line` with ``start`` set, and ``end`` set to the next timed
    entry (including empty entries, which mark the end of a line before a break)."""
    out: list[Line] = []
    entries = doc.lines
    for i, e in enumerate(entries):
        nxt = None
        for f in entries[i + 1:]:
            if f.time > e.time:
                nxt = f.time
                break
        ln = make_line(e.text, start=e.time, end=nxt)
        if ln is None:
            continue
        if e.words and len(e.words) == len(ln.words):
            for w, ew in zip(ln.words, e.words):
                w.start, w.end = ew.start, ew.end
        out.append(ln)
    return out


def from_lrc(text: str, source: str = SOURCE_LRC, language: str | None = "en") -> Lyrics:
    doc = lrcmod.parse_lrc(text)
    lines = lines_from_lrc_doc(doc)
    if not doc.lines:
        # A file without timestamps: treat it as plain text rather than failing. (Only
        # then: an LRC whose timed lines are all empty or section headers has no lyrics,
        # and reading its raw "[00:05.00][Chorus]" lines as text would invent some.)
        lines = parse_plain_text(text)
    meta = {"lrc_tags": doc.tags} if doc.tags else {}
    return Lyrics(lines=lines, source=source, checked=True, language=language, meta=meta)


def from_text(text: str, source: str = SOURCE_USER_TEXT, language: str | None = "en") -> Lyrics:
    """User-supplied text. If it is really LRC (has line timestamps) it is parsed as LRC
    and labelled ``lrc`` (unless a more specific source such as ``online:lrclib`` is given)."""
    if lrcmod.has_timestamps(text):
        return from_lrc(text, source=SOURCE_LRC if source == SOURCE_USER_TEXT else source,
                        language=language)
    return Lyrics(lines=parse_plain_text(text), source=source, checked=True, language=language)


# --------------------------------------------------------------------------------------
# Embedded lyrics (mutagen, with a PyAV fallback for Matroska/WebM/AVI)
# --------------------------------------------------------------------------------------


@dataclass
class EmbeddedLyrics:
    text: str            # plain text (no timestamps)
    lrc: str | None      # LRC text when the tag carried timings
    tag: str             # where it came from, e.g. "ID3 USLT", "Vorbis LYRICS"
    # Language of the tag relative to the song's language (ID3 USLT/SYLT carry one):
    # 0 = same language, 1 = not stated ("XXX", "und", other tag types), 2 = another one.
    lang_rank: int = 1

    @property
    def synced(self) -> bool:
        return self.lrc is not None


def _sylt_to_lrc(frame, sample_rate: int | None) -> str | None:
    """ID3 SYLT -> LRC. Entries are (text, time); time is ms (format 2) or MPEG frames
    (format 1, converted with 1152 samples per frame). Entries whose text starts with a
    newline begin a new line (syllable-level SYLT); otherwise each entry is a line."""
    entries = list(getattr(frame, "text", []) or [])
    if not entries:
        return None
    fmt = getattr(frame, "format", 2)

    def secs(t):
        if fmt == 1:
            return t * 1152.0 / float(sample_rate or 44100)
        return t / 1000.0

    syllable_mode = any(txt[:1] in ("\n", "\r") for txt, _ in entries)
    lines: list[lrcmod.LrcLine] = []
    if syllable_mode:
        cur_t, cur = None, ""
        for txt, t in entries:
            if txt[:1] in ("\n", "\r") and cur_t is not None:
                lines.append(lrcmod.LrcLine(secs(cur_t), cur.strip()))
                cur_t, cur = None, ""
            if cur_t is None:
                cur_t = t
            cur += txt.lstrip("\r\n")
        if cur_t is not None:
            lines.append(lrcmod.LrcLine(secs(cur_t), cur.strip()))
    else:
        lines = [lrcmod.LrcLine(secs(t), txt.strip()) for txt, t in entries]
    return lrcmod.format_lrc(lines)


def _strip_lrc(lrc_text: str) -> str:
    return "\n".join(lrcmod.parse_lrc(lrc_text).text_lines())


def _make_embedded(text: str, tag: str) -> EmbeddedLyrics | None:
    text = (text or "").strip("\ufeff \r\n\t")
    if not text:
        return None
    if lrcmod.has_timestamps(text):
        return EmbeddedLyrics(text=_strip_lrc(text), lrc=text, tag=tag)
    return EmbeddedLyrics(text=text, lrc=None, tag=tag)


def _pick(cands: list[EmbeddedLyrics]) -> EmbeddedLyrics | None:
    """Tags in another language (e.g. a translation in a second USLT frame) come last;
    otherwise prefer synced lyrics (they give line anchors), then the song's language,
    then the longest text."""
    cands = [c for c in cands if c is not None]
    if not cands:
        return None
    return sorted(cands, key=lambda c: (c.lang_rank == 2, not c.synced, c.lang_rank, -len(c.text)))[0]


# ID3 language codes that mean "not stated".
_ID3_NO_LANG = {"", "xxx", "und", "zxx", "\x00\x00\x00"}


def _id3_lang_rank(frame, want: str | None) -> int:
    lang = str(getattr(frame, "lang", "") or "").strip().lower()
    if lang in _ID3_NO_LANG or want is None:
        return 1
    return 0 if lang == want else 2


def _from_id3(tags, sample_rate, language: str | None) -> list[EmbeddedLyrics]:
    out = []
    want = {"en": "eng"}.get(language or "", None)
    for f in (tags.getall("USLT") if hasattr(tags, "getall") else []):
        e = _make_embedded(str(f.text), "ID3 USLT")
        if e is not None:
            e.lang_rank = _id3_lang_rank(f, want)
        out.append(e)
    for f in (tags.getall("SYLT") if hasattr(tags, "getall") else []):
        lrc_text = _sylt_to_lrc(f, sample_rate)
        if lrc_text:
            e = _make_embedded(lrc_text, "ID3 SYLT")
            if e is not None:
                e.lang_rank = _id3_lang_rank(f, want)
            out.append(e)
    for f in (tags.getall("TXXX") if hasattr(tags, "getall") else []):
        if str(getattr(f, "desc", "")).strip().lower() in ("lyrics", "unsyncedlyrics",
                                                           "syncedlyrics", "unsynced lyrics"):
            out.append(_make_embedded("\n".join(map(str, f.text)), f"ID3 TXXX:{f.desc}"))
    return out


_VORBIS_KEYS = ("syncedlyrics", "lyrics", "unsyncedlyrics", "unsynced lyrics", "unsynced_lyrics")


def _from_mapping(tags, keys, label) -> list[EmbeddedLyrics]:
    """Vorbis comments / APEv2 / ASF / generic mappings, case-insensitive keys."""
    out = []
    try:
        items = list(tags.items())
    except Exception:  # noqa: BLE001 - mutagen tag types vary
        return out
    for k, v in items:
        if str(k).strip().lower() in keys:
            vals = v if isinstance(v, (list, tuple)) else [v]
            text = "\n".join(str(getattr(x, "value", x)) for x in vals)
            out.append(_make_embedded(text, f"{label} {k}"))
    return out


def read_embedded_lyrics(path: str | Path, language: str | None = "en") -> EmbeddedLyrics | None:
    """Lyrics stored in the file's tags, or None. Never raises for unreadable tags."""
    path = str(path)
    cands: list[EmbeddedLyrics] = []
    try:
        import mutagen
        from mutagen.id3 import ID3
        from mutagen.mp4 import MP4Tags

        try:
            mf = mutagen.File(path)
        except Exception:  # noqa: BLE001 - unknown/odd container
            mf = None
        if mf is None:
            # An ID3 tag on a file mutagen cannot otherwise parse (e.g. odd MP3 frames).
            try:
                from types import SimpleNamespace

                mf = SimpleNamespace(tags=ID3(path), info=None)
            except Exception:  # noqa: BLE001 - no ID3 tag either
                mf = None
        if mf is not None and mf.tags is not None:
            tags = mf.tags
            sr = getattr(getattr(mf, "info", None), "sample_rate", None)
            if isinstance(tags, ID3):
                cands += _from_id3(tags, sr, language)
            elif isinstance(tags, MP4Tags):
                for v in tags.get("\xa9lyr", []):
                    cands.append(_make_embedded(str(v), "MP4 \xa9lyr"))
            else:
                # Vorbis comments (FLAC, Ogg Vorbis/Opus), APEv2, ASF (WMA).
                cands += _from_mapping(tags, _VORBIS_KEYS + ("wm/lyrics",), type(tags).__name__)
        # An MP3 can carry an APEv2 tag as well as ID3.
        if not cands and path.lower().endswith((".mp3", ".ape", ".wv", ".mpc")):
            try:
                from mutagen.apev2 import APEv2

                cands += _from_mapping(APEv2(path), _VORBIS_KEYS, "APEv2")
            except Exception:  # noqa: BLE001 - no APE tag
                pass
    except Exception as e:  # noqa: BLE001 - a bad tag must not fail the job
        log.info("mutagen could not read tags of %s: %s", path, e)

    if not cands:
        # Matroska / WebM / AVI and anything else mutagen does not know: container tags.
        try:
            import av

            with av.open(path) as c:
                metas = [dict(c.metadata)] + [dict(s.metadata) for s in c.streams]
            for m in metas:
                cands += _from_mapping(m, _VORBIS_KEYS + ("lyrics-eng",), "container")
        except Exception as e:  # noqa: BLE001
            log.info("PyAV could not read container tags of %s: %s", path, e)
    return _pick(cands)


def embedded_lyrics_info(path: str | Path, language: str | None = "en") -> dict:
    """The ``embedded_lyrics`` object of the protocol's ``inspect.result``.

    When the lyrics are synced, ``text`` is the LRC text (so the app can hand it back
    unchanged as ``lyrics.lrc``); otherwise it is the plain text."""
    e = read_embedded_lyrics(path, language)
    if e is None:
        return {"present": False, "synced": False, "text": None}
    return {"present": True, "synced": e.synced, "text": e.lrc if e.synced else e.text}


def from_embedded(path: str | Path, language: str | None = "en") -> Lyrics | None:
    e = read_embedded_lyrics(path, language)
    if e is None:
        return None
    if e.synced:
        lyr = from_lrc(e.lrc, source=SOURCE_EMBEDDED, language=language)
    else:
        lyr = Lyrics(parse_plain_text(e.text), SOURCE_EMBEDDED, checked=True, language=language)
    lyr.meta["embedded_tag"] = e.tag
    return lyr if lyr.lines else None


# --------------------------------------------------------------------------------------
# Online lookup: LRCLIB (https://lrclib.net/docs). Only when explicitly called.
# --------------------------------------------------------------------------------------

LRCLIB_BASE = "https://lrclib.net"


def default_user_agent() -> str:
    """A generic User-Agent naming the app (LRCLIB asks clients to identify
    themselves). It carries no personal data. The name comes from branding.json when
    the repository is around, so a rename stays a one-file change."""
    name, version = "Open Mic", "0.1.0"
    try:
        here = Path(__file__).resolve()
        for parent in list(here.parents)[:4]:
            b = parent / "branding.json"
            if b.is_file():
                name = json.loads(b.read_text(encoding="utf-8")).get("name", name)
                break
    except Exception:  # noqa: BLE001
        pass
    return f"{name}/{version} (open-source desktop karaoke app)"


def _http_get_json(url: str, *, timeout: float, user_agent: str, opener=None):
    """GET JSON. Returns (status, data). 404 -> (404, None). Retries once on 429."""
    req = urllib.request.Request(url, headers={"User-Agent": user_agent, "Accept": "application/json"})
    open_fn = opener or urllib.request.urlopen
    for attempt in range(2):
        try:
            with open_fn(req, timeout=timeout) as resp:
                status = getattr(resp, "status", 200)
                body = resp.read()
            return status, json.loads(body.decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return 404, None
            if e.code == 429 and attempt == 0:
                try:
                    wait = float(e.headers.get("Retry-After", "2"))
                except (TypeError, ValueError):
                    wait = 2.0
                time.sleep(min(10.0, wait) * (1.0 + 0.3 * random.random()))
                continue
            raise LyricsLookupError(f"The lyrics service answered with an error ({e.code}).") from e
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise LyricsLookupError(
                "The lyrics service could not be reached. Check the internet connection.") from e
        except (ValueError, UnicodeDecodeError) as e:
            raise LyricsLookupError("The lyrics service sent a response that could not be read.") from e
    raise LyricsLookupError("The lyrics service is busy. Try again in a minute.")


def _lrclib_result(rec: dict) -> dict:
    return {
        "service": "lrclib",
        "synced_lrc": rec.get("syncedLyrics") or None,
        "plain": rec.get("plainLyrics") or None,
        "duration_seconds": rec.get("duration"),
        "instrumental": bool(rec.get("instrumental")),
        "artist": rec.get("artistName"),
        "title": rec.get("trackName"),
        "album": rec.get("albumName"),
        "id": rec.get("id"),
    }


def lookup_lrclib(artist: str | None, title: str, album: str | None = None,
                  duration_seconds: float | None = None, *, timeout: float = 10.0,
                  user_agent: str | None = None, opener=None, max_results: int = 5) -> list[dict]:
    """Look a song up on LRCLIB. Returns protocol ``lyrics.lookup.result`` entries,
    best first (exact match, then search results ranked by synced-ness and how close
    the duration is). Raises :class:`LyricsLookupError` on network failure.

    Sends only artist, title, album and duration. Never called automatically.
    """
    ua = user_agent or default_user_agent()
    results: list[dict] = []
    seen: set = set()
    if artist and title:
        q = {"artist_name": artist, "track_name": title}
        if album:
            q["album_name"] = album
        if duration_seconds and 1 <= duration_seconds <= 3600:
            q["duration"] = str(int(round(duration_seconds)))
        status, data = _http_get_json(f"{LRCLIB_BASE}/api/get?{urllib.parse.urlencode(q)}",
                                      timeout=timeout, user_agent=ua, opener=opener)
        if status == 200 and isinstance(data, dict):
            r = _lrclib_result(data)
            results.append(r)
            seen.add(r["id"])
    if len(results) < max_results and title:
        q = {"track_name": title}
        if artist:
            q["artist_name"] = artist
        status, data = _http_get_json(f"{LRCLIB_BASE}/api/search?{urllib.parse.urlencode(q)}",
                                      timeout=timeout, user_agent=ua, opener=opener)
        found = [_lrclib_result(d) for d in (data or []) if isinstance(d, dict)]
        found = [r for r in found if r["id"] not in seen and (r["synced_lrc"] or r["plain"] or r["instrumental"])]

        def rank(r):
            dd = abs((r["duration_seconds"] or 0) - duration_seconds) if duration_seconds else 0.0
            return (dd > 10.0, r["synced_lrc"] is None, dd)

        found.sort(key=rank)
        if duration_seconds:
            # Records more than 20 s away are almost certainly another version/edit.
            found = [r for r in found if r["duration_seconds"] is None
                     or abs(r["duration_seconds"] - duration_seconds) <= 20.0]
        results += found
    return results[:max_results]


def from_lookup_result(result: dict, language: str | None = "en") -> Lyrics | None:
    """A ``lookup_lrclib`` entry -> :class:`Lyrics` (synced when possible)."""
    src = f"online:{result.get('service', 'lrclib')}"
    if result.get("synced_lrc"):
        return from_lrc(result["synced_lrc"], source=src, language=language)
    if result.get("plain"):
        return from_text(result["plain"], source=src, language=language)
    return None


# --------------------------------------------------------------------------------------
# Source order (protocol section 7, lyrics.mode)
# --------------------------------------------------------------------------------------


#: Values the app may pass as ``lyrics.source`` (protocol section 7): where the text or
#: LRC it hands over came from. ``online:<service>`` for a lookup result, ``embedded``
#: for the file's own lyrics shown by ``inspect``.
_SOURCE_RE = regex.compile(r"^(?:user_text|lrc|embedded|online:[a-z0-9][a-z0-9_.-]{0,31})$")


def valid_source(source: str | None) -> bool:
    return source is None or bool(_SOURCE_RE.match(source))


def resolve_lyrics(mode: str, *, lrc: str | None = None, text: str | None = None,
                   media_path: str | Path | None = None, transcribe=None,
                   language: str | None = "en", source: str | None = None) -> Lyrics | None:
    """Pick the lyrics for a job.

    ``mode``: ``auto`` (lrc, text, embedded, then ``transcribe()``), ``lrc``, ``text``
    or ``none`` (returns None). ``transcribe`` is a zero-argument callable returning
    :class:`Lyrics` (source ``transcribed``, unchecked); it is only called when needed.
    ``source`` labels what the caller passed as ``lrc`` / ``text`` (for example
    ``online:lrclib`` when the app hands over a lookup result, or ``embedded``); by
    default ``lrc`` and ``user_text``.

    In ``auto``, a source that is given but holds no singable line after normalisation
    (only section headers, blank lines or timestamps) falls through to the next source
    instead of skipping straight to transcription (verifier finding).
    Raises :class:`LyricsMissing` when the requested source is empty.
    """
    mode = (mode or "auto").lower()
    if not valid_source(source):
        raise ValueError(f"unknown lyrics source: {source!r}")

    def from_given_lrc() -> Lyrics:
        return from_lrc(lrc, source=source or SOURCE_LRC, language=language)

    def from_given_text() -> Lyrics:
        lyr = from_text(text, source=source or SOURCE_USER_TEXT, language=language)
        return lyr

    if mode == "none":
        return None
    if mode == "lrc":
        if not (lrc and lrc.strip()):
            raise LyricsMissing("No LRC lyrics were given.")
        lyr = from_given_lrc()
    elif mode == "text":
        if not (text and text.strip()):
            raise LyricsMissing("No lyrics text was given.")
        lyr = from_given_text()
    elif mode == "auto":
        lyr = None
        if lrc and lrc.strip():
            lyr = from_given_lrc()
        if (lyr is None or not lyr.lines) and text and text.strip():
            lyr = from_given_text()
        if (lyr is None or not lyr.lines) and media_path is not None:
            lyr = from_embedded(media_path, language=language)
        if (lyr is None or not lyr.lines) and transcribe is not None:
            lyr = transcribe()
    else:
        raise ValueError(f"unknown lyrics mode: {mode!r}")
    if lyr is None or not lyr.lines:
        raise LyricsMissing("No lyrics were found for this song.")
    return lyr

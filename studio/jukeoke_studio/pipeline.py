# SPDX-License-Identifier: GPL-3.0-or-later
"""One song through the Studio (WORKING ORDER 4.1; protocol section 7 ``job.submit``).

Stages, in order (the names the protocol's ``job.progress`` events carry):

``decode``   PyAV -> float32 at the native rate (media.py), SHA-256 of the file, tags and
             cover, the user's metadata corrections; lyrics the user gave (or the file
             carries) are parsed here too, so a job with unusable lyrics fails before
             minutes of separation.
``separate`` the preset's models (separate.py); VRAM freed afterwards.
``lyrics``   transcription of the isolated lead vocal, only when no lyrics were given or
             embedded (labelled ``transcribed``, unchecked); VRAM freed afterwards.
``align``    CTC alignment on the lead stem (align.py); VRAM freed afterwards.
``analyse``  key and tempo (analyse.py).
``write``    ``.jksong`` (jksong.py): the residual stem is computed from the original
             minus what the other stems decode to, so the stems sum back to the source
             within half a 24-bit step; then checked by decoding, and moved into place
             under ``<artist> - <title>.jksong`` (``(2)``, ``(3)`` ... on a clash, never
             overwriting) atomically.

Cancellation is cooperative: every long loop calls ``hooks.check()`` (model chunks,
decoding, hashing, transcription segments, alignment chunks, FLAC encoding per stem).
Temporary files are deleted whatever happens. A failure raises a
:class:`~jukeoke_studio.errors.StudioError` with the protocol's error code.

Model loading goes through :class:`Models` so tests can swap in the stand-ins of
``stub.py`` (``JUKEOKE_STUDIO_STUB=1``).
"""

from __future__ import annotations

import gc
import json
import logging
import os
import platform
import re
import secrets
import shutil
import sys
import time
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import numpy as np

from . import __version__ as STUDIO_VERSION
from . import env as studio_env
from .errors import (AlignmentFailed, BadRequest, Cancelled, Hooks, LyricsMissingError, ModelsMissing,
                     NotFound, StudioError, WriteFailed)
from .presets import PRESETS

log = logging.getLogger(__name__)

STAGES = ("decode", "separate", "lyrics", "align", "analyse", "write")
#: Share of the job's overall progress per stage (rough GPU time of a 4-minute song).
STAGE_WEIGHT = {"decode": 0.03, "separate": 0.62, "lyrics": 0.08, "align": 0.12,
                "analyse": 0.03, "write": 0.12}

LYRICS_MODES = ("auto", "text", "lrc", "none")


# --------------------------------------------------------------------------- job spec

@dataclass
class JobSpec:
    job_id: str
    input: str
    output_dir: str
    preset: str = "full"
    language: str | None = "en"
    metadata: dict = field(default_factory=dict)
    lyrics: dict = field(default_factory=lambda: {"mode": "auto"})


def parse_job(obj) -> JobSpec:
    """Validates the ``job`` object of ``job.submit``; raises :class:`BadRequest`."""
    from .lyrics import valid_source

    if not isinstance(obj, dict):
        raise BadRequest("The job must be an object.")
    jid = obj.get("job_id")
    if not isinstance(jid, str) or not jid.strip():
        raise BadRequest("The job needs a job_id.")
    inp = obj.get("input")
    if not isinstance(inp, str) or not inp:
        raise BadRequest("The job needs an input file.")
    out = obj.get("output_dir")
    if not isinstance(out, str) or not out:
        raise BadRequest("The job needs an output_dir.")
    preset = obj.get("preset") or "full"
    if preset not in PRESETS:
        raise BadRequest(f"Unknown quality preset {preset!r}.", f"known: {list(PRESETS)}")
    lang = obj.get("language", "en")
    if lang is not None and not (isinstance(lang, str) and re.fullmatch(r"[a-z]{2,3}", lang)):
        raise BadRequest("The language must be a two-letter code such as \"en\".")
    meta = obj.get("metadata") or {}
    if not isinstance(meta, dict):
        raise BadRequest("metadata must be an object.")
    for k in ("title", "artist", "album"):
        if meta.get(k) is not None and not isinstance(meta.get(k), str):
            raise BadRequest(f"metadata.{k} must be a string or null.")
    lyr = obj.get("lyrics") or {"mode": "auto"}
    if not isinstance(lyr, dict):
        raise BadRequest("lyrics must be an object.")
    mode = (lyr.get("mode") or "auto").lower()
    if mode not in LYRICS_MODES:
        raise BadRequest(f"Unknown lyrics mode {mode!r}.")
    for k in ("text", "lrc"):
        if lyr.get(k) is not None and not isinstance(lyr.get(k), str):
            raise BadRequest(f"lyrics.{k} must be a string or null.")
    src = lyr.get("source")
    if src is not None and (not isinstance(src, str) or not valid_source(src)):
        raise BadRequest("lyrics.source must be user_text, lrc, embedded or online:<service>.",
                         f"got {src!r}")
    return JobSpec(jid, inp, out, preset, lang, dict(meta), {**lyr, "mode": mode})


# ------------------------------------------------------------------------ file names

_BAD = set('<>:"/\\|?*')
_RESERVED = {"CON", "PRN", "AUX", "NUL"} | {f"COM{i}" for i in range(10)} | {f"LPT{i}" for i in range(10)}


def safe_file_stem(s: str, fallback: str = "Song") -> str:
    """A file name that works on Windows, macOS and exFAT (same rules as the app's
    export names, app/src-tauri/src/export.rs ``safe_file_stem``): invalid characters
    and control characters become ``_``, NFC, no leading dots (the library skips
    dotfiles) and no trailing dots or spaces, no reserved Windows device names, at most
    150 characters."""
    s = unicodedata.normalize("NFC", s or "")
    out = "".join("_" if (c in _BAD or unicodedata.category(c) in ("Cc", "Cf") and c not in "‍‌")
                  else c for c in s)
    out = " ".join(out.split())
    out = out.lstrip(". ").rstrip(". ")
    if len(out) > 150:
        out = out[:150].rstrip(". ")
    if out.split(".")[0].upper() in _RESERVED:
        out = "_" + out
    return out or fallback


def song_stem(artist: str | None, title: str) -> str:
    return safe_file_stem(f"{artist} - {title}" if artist else title)


def _move_no_clobber(src: Path, dst: Path) -> bool:
    """Atomic rename that fails (False) instead of replacing an existing ``dst``.
    Windows: MoveFileExW with MOVEFILE_WRITE_THROUGH (no REPLACE_EXISTING), the same
    durability flag the app uses (DECISIONS D-81); elsewhere a hard link + unlink."""
    if sys.platform == "win32":
        import ctypes

        ok = ctypes.windll.kernel32.MoveFileExW(str(src), str(dst), 0x8)  # WRITE_THROUGH
        if ok:
            return True
        err = ctypes.GetLastError()
        if err in (80, 183):  # ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS
            return False
        raise OSError(err, ctypes.FormatError(err), str(dst))
    try:
        os.link(src, dst)
    except FileExistsError:
        return False
    os.unlink(src)
    return True


def place_song(tmp: Path, out_dir: Path, stem: str) -> Path:
    """Moves the finished temporary song to ``<stem>.jksong``, or ``<stem> (2).jksong``...
    when the name is taken; never replaces another song."""
    for n in range(1, 10000):
        name = f"{stem}.jksong" if n == 1 else f"{stem} ({n}).jksong"
        dst = out_dir / name
        if dst.exists():
            continue
        if _move_no_clobber(tmp, dst):
            return dst
    raise WriteFailed("The song file could not be named.", f"no free name for {stem!r} in {out_dir}")


# ------------------------------------------------------------------------ the models

class Models:
    """Creates the model objects for one job (real ones, or the stand-ins of stub.py)."""

    def __init__(self, models_dir: Path | None, device: str | None = None, stub: bool | None = None):
        from . import stub as stubmod

        self.models_dir = models_dir
        self.device = device
        self.stub = stubmod.enabled() if stub is None else stub

    def separator(self):
        if self.stub:
            from .stub import StubSeparator

            return StubSeparator()
        from .separate import Separator

        return Separator(self.models_dir, self.device, keep_loaded=True, allow_download=False)

    def missing(self, preset: str, need_alignment: bool, need_transcription: bool) -> list[str]:
        if self.stub:
            return []
        from .modelzoo import hfmodels
        from .modelzoo.download import missing_files
        from .modelzoo.registry import get as reg_get

        out = [mid for mid in dict.fromkeys(PRESETS[preset].values())
               if missing_files(reg_get(mid), self.models_dir)]
        if need_alignment and hfmodels.missing(hfmodels.get(hfmodels.DEFAULT_ALIGNMENT), self.models_dir):
            out.append(hfmodels.DEFAULT_ALIGNMENT)
        if need_transcription and hfmodels.missing(hfmodels.get(hfmodels.DEFAULT_TRANSCRIPTION), self.models_dir):
            out.append(hfmodels.DEFAULT_TRANSCRIPTION)
        return out

    def aligner(self):
        if self.stub:
            from .stub import stub_aligner

            return stub_aligner()
        from .align import DEFAULT_MODEL, Aligner

        return Aligner(DEFAULT_MODEL, models_dir=self.models_dir, device=self.device)

    def transcriber(self):
        if self.stub:
            from .stub import StubTranscriber

            return StubTranscriber()
        from .transcribe import DEFAULT_WHISPER, Transcriber

        return Transcriber(DEFAULT_WHISPER, models_dir=self.models_dir, device=self.device)


def free_vram() -> None:
    """Gives memory back between stages (WORKING ORDER: one model family at a time)."""
    gc.collect()
    torch = sys.modules.get("torch")
    if torch is not None:
        try:
            if torch.cuda.is_available():
                torch.cuda.empty_cache()
        except Exception:  # noqa: BLE001
            pass


def runtime_versions() -> dict:
    """Python / torch versions without importing torch."""
    from importlib import metadata

    try:
        tv = metadata.version("torch")
    except metadata.PackageNotFoundError:
        tv = None
    return {"version": STUDIO_VERSION, "python": platform.python_version(), "torch": tv}


def _device_name(stub: bool) -> str:
    if stub:
        return "cpu (test stand-in models)"
    torch = sys.modules.get("torch")
    try:
        if torch is not None and torch.cuda.is_available():
            return torch.cuda.get_device_name(0)
    except Exception:  # noqa: BLE001
        pass
    return "cpu"


def _hf_provenance(model) -> dict | None:
    spec = getattr(model, "spec", None)
    if spec is None:
        return None
    w = spec.weight()
    return {"task": spec.task, "name": spec.repo, "title": spec.title, "revision": spec.revision,
            "file": w.name, "sha256": w.sha256, "licence": spec.licence, "commercial": spec.commercial,
            "source_url": spec.source_url,
            "precision": "fp32" if spec.task == "alignment" else getattr(model, "compute_type", None)}


# --------------------------------------------------------------------------- running

@dataclass
class JobContext:
    models_dir: Path | None
    temp_dir: Path
    app_version: str | None = None
    app_name: str = "Jukeoke"
    device: str | None = None
    stub: bool | None = None
    #: Held during GPU stages, so a ``realign`` waits instead of competing for VRAM.
    gpu_lock: object | None = None

    def gpu(self):
        import contextlib

        return self.gpu_lock if self.gpu_lock is not None else contextlib.nullcontext()


@dataclass
class JobResult:
    output: Path
    report: dict


ProgressFn = Callable[[str, float, float, str | None], None]  # stage, stage_progress, overall, message


def run_job(spec: JobSpec, ctx: JobContext, hooks_cancel: Hooks, on_progress: ProgressFn) -> JobResult:
    """Runs every stage of ``spec``; raises a StudioError (``Cancelled`` included)."""
    from . import media

    t_job = time.perf_counter()
    seconds: dict[str, float] = {}
    warnings: list[str] = []
    models = Models(ctx.models_dir, ctx.device, ctx.stub)
    order = list(STAGES)
    done_w = {s: sum(STAGE_WEIGHT[x] for x in order[:i]) for i, s in enumerate(order)}
    last_emit = [0.0, None]

    def stage_hooks(stage: str, message: str) -> Hooks:
        def on(fr: float, msg: str | None) -> None:
            now = time.monotonic()
            # At most ~5 events a second, but always the first and the last of a stage.
            if fr in (0.0, 1.0) or now - last_emit[0] >= 0.2 or last_emit[1] != stage:
                last_emit[0], last_emit[1] = now, stage
                on_progress(stage, fr, done_w[stage] + STAGE_WEIGHT[stage] * fr, msg or message)

        h = Hooks(on, hooks_cancel.cancel_event)
        h.progress(0.0, message)
        return h

    if not os.path.isfile(spec.input):
        raise NotFound("That file could not be found.", spec.input)
    job_tmp = Path(ctx.temp_dir) / f"job-{safe_file_stem(spec.job_id)}-{secrets.token_hex(4)}"
    job_tmp.mkdir(parents=True, exist_ok=True)
    out_dir = Path(spec.output_dir)
    tmp_song: Path | None = None
    try:
        # ------------------------------------------------------------------ decode
        t0 = time.perf_counter()
        h = stage_hooks("decode", "Reading the file")
        dec = media.decode(spec.input, h.sub(0.0, 0.7))
        warnings += dec.warnings
        src_sha = media.sha256_file(spec.input, h.sub(0.7, 0.9))
        h.check()
        cover, cw = media.cover_for_song(spec.input)
        if cw:
            warnings.append(cw)
        info = dec.info
        meta = spec.metadata or {}
        title = (meta.get("title") or "").strip() or info.title or Path(spec.input).stem
        artist = (meta.get("artist") or "").strip() or info.artist or ""
        album = meta.get("album") if "album" in meta else info.album
        if isinstance(album, str):
            album = album.strip() or None
        meta_src = {"title": "user" if (meta.get("title") or "").strip() else ("tags" if info.title else "file name"),
                    "artist": "user" if (meta.get("artist") or "").strip() else ("tags" if info.artist else "none")}
        audio, sr, downmix = dec.audio, dec.sample_rate, dec.downmix
        del dec
        lyr, transcribe_needed = _given_lyrics(spec)
        h.progress(1.0)
        seconds["decode"] = time.perf_counter() - t0

        missing = models.missing(spec.preset, need_alignment=(spec.lyrics.get("mode") != "none"),
                                 need_transcription=transcribe_needed)
        if missing:
            raise ModelsMissing(missing)

        # ---------------------------------------------------------------- separate
        t0 = time.perf_counter()
        h = stage_hooks("separate", "Loading the separation models")
        with ctx.gpu():
            sep = models.separator()
            try:
                res = sep.separate(audio, sr, spec.preset, hooks=h)
                sep_load = getattr(sep, "load_seconds", 0.0)
            finally:
                sep.unload()
                del sep
                free_vram()
        stems = res.stems
        sep_prov = res.provenance()
        seconds["separate"] = time.perf_counter() - t0
        lead = stems["lead_vocal"]

        # ------------------------------------------------------------------ lyrics
        t0 = time.perf_counter()
        h = stage_hooks("lyrics", "Listening for the words" if transcribe_needed else "Reading the lyrics")
        hf_models: list[dict] = []
        if transcribe_needed:
            with ctx.gpu():
                tr = models.transcriber()
                try:
                    lyr, _words = tr.transcribe_lyrics(lead, sr, spec.language, hooks=h)
                    p = _hf_provenance(tr)
                    if p:
                        hf_models.append(p)
                finally:
                    del tr
                    free_vram()
            if not lyr.lines:
                warnings.append("No lyrics were given and no words could be heard in the lead vocal, "
                                "so the song was saved without lyrics.")
                lyr = None
        h.progress(1.0)
        seconds["lyrics"] = time.perf_counter() - t0

        # ------------------------------------------------------------------- align
        t0 = time.perf_counter()
        lyrics_doc = None
        h = stage_hooks("align", "Timing the lyrics to the letter")
        if lyr is not None:
            from .align import align_stage

            with ctx.gpu():
                al = None
                try:
                    al = models.aligner()
                    r = align_stage(al, lead, sr, lyr, hooks=h)
                    lyrics_doc = r.lyrics
                    warnings += [f"Lyrics: {w}" for w in r.warnings]
                    p = _hf_provenance(getattr(al, "am", None))
                    if p:
                        hf_models.append(p)
                except StudioError:
                    raise
                except Exception as e:  # noqa: BLE001 - keep the separated song
                    if _is_oom(e):
                        raise
                    log.exception("alignment failed")
                    warnings.append("The lyrics could not be timed to the song (" + str(e)[:200] + "), so "
                                    "the song was saved without lyrics. Paste them in the sync editor to "
                                    "try again.")
                    lyrics_doc = None
                finally:
                    del al
                    free_vram()
            if lyrics_doc is not None:
                lyrics_doc = _check_lyrics(lyrics_doc, audio.shape[1] / sr, warnings)
        h.progress(1.0)
        seconds["align"] = time.perf_counter() - t0

        # ----------------------------------------------------------------- analyse
        t0 = time.perf_counter()
        h = stage_hooks("analyse", "Finding the key and tempo")
        from .analyse import analyse_stems

        analysis = analyse_stems(stems, sr)
        h.progress(1.0)
        seconds["analyse"] = time.perf_counter() - t0

        # ------------------------------------------------------------------- write
        t0 = time.perf_counter()
        h = stage_hooks("write", "Writing the song file")
        from . import jksong

        out_dir.mkdir(parents=True, exist_ok=True)
        stem = song_stem(artist, title)
        # A short temporary name (long titles + a deep Library folder must stay under
        # Windows' 260-character path limit); "*.jksong.tmp-*" is what the app's library
        # ignores while it is being written and cleans up if a crash leaves it behind.
        tmp_song = out_dir / f"studio-{secrets.token_hex(6)}.jksong.tmp-{os.getpid()}"
        provenance = {
            "app": {"name": ctx.app_name, "version": ctx.app_version},
            "studio": {**runtime_versions(), "device": _device_name(models.stub), "precision": "fp32"},
            "source": {"filename": Path(spec.input).name, "sha256": src_sha, "container": info.container,
                       "codec": info.codec, "sample_rate": info.sample_rate, "channels": info.channels,
                       "bits_per_sample": info.bits_per_sample, "lossy": info.lossy,
                       "has_video": info.has_video, "duration_seconds": round(audio.shape[1] / sr, 6)},
            "models": sep_prov["models"] + hf_models,
            "resampling": sep_prov["resampling"],
            "preset": spec.preset,
            "metadata_source": meta_src,
            "created_utc": jksong.utc_now(),
        }
        if downmix is not None:  # > 2 channels folded to stereo (media.py)
            provenance["source"]["downmix"] = downmix
        roles = ("lead_vocal", "backing_vocals", "drums", "other")
        stem_inputs = [jksong.StemInput(r, None if r == "other" else stems[r]) for r in roles]
        n_enc = [0]

        def encoded_hook():
            n_enc[0] += 1
            h.progress(min(0.9, n_enc[0] / 4 * 0.9))

        try:
            jksong.write_song(
                tmp_song, stems=stem_inputs, sample_rate=sr, title=title, artist=artist, album=album,
                lyrics=lyrics_doc, cover=cover, analysis=analysis, preset=spec.preset,
                # Every preset defines "other" as the residual, and write_song computes it
                # from the source minus what the other stems decode to: the stems at unity
                # sum back to the source (checked below by decoding).
                stems_sum_to_source=True, provenance=provenance, source=audio,
                on_stem=lambda role: (encoded_hook(), h.check()))
        except Cancelled:
            raise
        except OSError as e:
            raise WriteFailed("The song file could not be written (is the drive full or read-only?).",
                              f"{type(e).__name__}: {e}") from e
        h.check()
        sum_err = _sum_error(jksong, tmp_song, audio)
        if sum_err is None or sum_err > 2.0 ** -22:
            raise WriteFailed("The written song file did not check out, so it was not kept.",
                              f"stems do not sum back to the source: max |error| {sum_err}")
        h.check()
        final = place_song(tmp_song, out_dir, stem)
        tmp_song = None
        # The song is in the Library now. Hooks.progress raises Cancelled when a cancel
        # arrived in the meantime, which reported "cancelled" for a song that was in fact
        # saved (verifier finding); a cancel this late is ignored instead.
        if not h.cancelled:
            h.progress(1.0)
        seconds["write"] = time.perf_counter() - t0
        report = {
            "seconds": {k: round(v, 2) for k, v in seconds.items()},
            "total_seconds": round(time.perf_counter() - t_job, 2),
            "model_load_seconds": round(sep_load, 2),
            "lyrics_source": (lyrics_doc or {}).get("source") if lyrics_doc else None,
            "lyrics_checked": bool((lyrics_doc or {}).get("checked")) if lyrics_doc else False,
            "lyrics_lines": len(lyrics_doc["lines"]) if lyrics_doc else 0,
            "sample_rate": sr,
            "duration_seconds": round(audio.shape[1] / sr, 3),
            "preset": spec.preset,
            "stems_sum_max_error": sum_err,
            "resampling": sep_prov["resampling"],
            "title": title, "artist": artist,
            "warnings": warnings,
        }
        return JobResult(final, report)
    finally:
        if tmp_song is not None:
            for p in (tmp_song, *tmp_song.parent.glob(tmp_song.name + ".tmp-*")):
                try:
                    p.unlink()
                except OSError:
                    pass
        shutil.rmtree(job_tmp, ignore_errors=True)


def _given_lyrics(spec: JobSpec):
    """Lyrics the user gave or the file carries, parsed now (cheap). Returns (lyrics or
    None, transcription needed). Raises LyricsMissingError for an empty text/lrc mode."""
    from .lyrics import LyricsMissing, resolve_lyrics

    lspec = spec.lyrics or {}
    mode = lspec.get("mode", "auto")
    if mode == "none":
        return None, False
    try:
        lyr = resolve_lyrics(mode, lrc=lspec.get("lrc"), text=lspec.get("text"), media_path=spec.input,
                             transcribe=None, language=spec.language, source=lspec.get("source"))
        return lyr, False
    except LyricsMissing as e:
        if mode == "auto":
            return None, True
        raise LyricsMissingError("The lyrics given for this song are empty (or only section headings), "
                                 "so it was not made. Paste the lyrics or choose automatic lyrics.",
                                 str(e)) from e


def _check_lyrics(doc: dict, duration: float, warnings: list[str]) -> dict | None:
    from .jksong import validate_lyrics

    problems = validate_lyrics(doc, duration)
    if problems:
        warnings.append("The timed lyrics did not pass the song-file checks, so the song was saved "
                        "without lyrics: " + "; ".join(problems[:3]))
        return None
    return doc


def _sum_error(jksong, path: Path, source: np.ndarray) -> float | None:
    """Decodes the written stems and returns max |sum - source| (float)."""
    song = jksong.open_song(path)
    total = None
    for s in song.manifest["stems"]:
        x = song.read_stem_int(s["role"]).T.astype(np.float64) * (float(s["gain"]) / jksong.FULL_SCALE)
        total = x if total is None else total + x
    if total is None or total.shape != source.shape:
        return None
    return float(np.max(np.abs(total - source.astype(np.float64)))) if total.size else 0.0


def _is_oom(e: BaseException) -> bool:
    return type(e).__name__ in ("OutOfMemoryError",) or "out of memory" in str(e).lower()


def map_exception(e: BaseException) -> StudioError:
    """Any exception -> the protocol error the app shows."""
    if isinstance(e, StudioError):
        return e
    if _is_oom(e):
        return StudioError("The graphics card ran out of memory while making this song. Close other "
                           "programs that use the graphics card and try again.",
                           f"{type(e).__name__}: {e}", code="out_of_memory")
    if isinstance(e, MemoryError):
        return StudioError("The computer ran out of memory while making this song.",
                           f"{type(e).__name__}: {e}", code="out_of_memory")
    if isinstance(e, OSError):
        return WriteFailed("A file could not be read or written.", f"{type(e).__name__}: {e}")
    return StudioError("Something went wrong while making this song. The details are in the Studio log.",
                       f"{type(e).__name__}: {e}")


def report_json(report: dict) -> str:
    return json.dumps(report, ensure_ascii=False)


# For ``env`` users that only need the temp folder.
temp_dir = studio_env.temp_dir

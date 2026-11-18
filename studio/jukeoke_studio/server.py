# SPDX-License-Identifier: GPL-3.0-or-later
"""The Studio's protocol server: ``python -I -m jukeoke_studio serve``.

Implements docs/STUDIO_PROTOCOL.md (version 1): JSON lines on stdin (requests) and
stdout (responses and events); stderr is the log.

Threads:

* the **main thread** reads stdin and answers quick requests itself (``hello``,
  ``models.status``, ``job.submit``, ``job.cancel``, ``job.list``, ``shutdown``), so the
  app always gets an answer while a job runs;
* slow requests (``probe``, ``inspect``, ``lyrics.lookup``, ``models.download``,
  ``realign``) each run on their own short-lived thread;
* **one worker thread** runs the jobs, one at a time, in submission order. A failed
  job sends ``job.failed`` and the worker takes the next one.

GPU work (separation, transcription, alignment, ``realign``) is serialised by one lock,
so a ``realign`` from the sync editor waits for the running stage instead of competing
for video memory.

Stdout carries nothing but protocol JSON: ``__main__`` hands this module a private
copy of the real stdout and points file descriptor 1 at stderr before anything else is
imported (env.py). The process exits when stdin closes or after ``shutdown``.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import threading
import time
import traceback
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path

from . import __version__ as STUDIO_VERSION
from . import env as studio_env
from .errors import BadRequest, Hooks, NotFound, StudioError, is_expected

log = logging.getLogger("jukeoke_studio.server")

PROTOCOL = 1


@dataclass
class Job:
    spec: object                       # pipeline.JobSpec
    state: str = "queued"              # queued | running | done | failed
    stage: str | None = None
    overall: float = 0.0
    error_code: str | None = None
    output: str | None = None
    hooks: Hooks = field(default_factory=Hooks)
    submitted: float = field(default_factory=time.time)


class Server:
    def __init__(self, out: studio_env.ProtocolOut, models_dir: Path | None = None, inp=None):
        self.out = out
        self.inp = inp if inp is not None else sys.stdin.buffer
        self._out_lock = threading.Lock()
        self.jobs: dict[str, Job] = {}
        self.queue: deque[str] = deque()
        self.cv = threading.Condition()
        self.current: Job | None = None
        self.gpu_lock = threading.Lock()
        self.download_lock = threading.Lock()
        self.stopping = threading.Event()
        e = studio_env.current()
        self.models_dir: Path | None = models_dir or e.models_dir
        self.temp_dir: Path = studio_env.temp_dir()
        self.library_dir: Path | None = None
        self.app_version: str | None = None
        self.app_name = _app_name()
        self.worker = threading.Thread(target=self._worker, name="job-worker", daemon=True)
        self.worker.start()

    # ------------------------------------------------------------------ output

    def send(self, msg: dict) -> None:
        line = json.dumps(msg, ensure_ascii=False, allow_nan=False, default=_json_default)
        with self._out_lock:
            try:
                self.out.write_line(line)
            except OSError:
                # The app has gone (pipe closed): stop quietly.
                self.stopping.set()

    def error(self, rid, err: StudioError | BadRequest, *, logged: bool = False) -> None:
        """Answers request ``rid`` with ``err``. Expected user errors (a damaged file, a
        missing model, a bad request) are one WARNING line; anything else an ERROR line
        (its traceback is logged where it was caught). ``logged``: already in the log."""
        if not logged:
            level = (logging.INFO if err.code == "cancelled"
                     else logging.WARNING if is_expected(err) else logging.ERROR)
            log.log(level, "request %s failed (%s): %s%s", rid, err.code, err.message,
                    f" [{err.detail}]" if err.detail else "")
        self.send({"type": "error", "id": rid, "error": err.to_json()})

    # ------------------------------------------------------------------ reading

    def serve(self) -> int:
        log.info("Studio %s serving (python %s, pid %d, models %s, temp %s)", STUDIO_VERSION,
                 sys.version.split()[0], os.getpid(), self.models_dir, self.temp_dir)
        stdin = self.inp
        while not self.stopping.is_set():
            raw = stdin.readline()
            if not raw:
                log.info("stdin closed: exiting")
                break
            line = raw.decode("utf-8", "replace").strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except ValueError as e:
                self.error(None, BadRequest("The Studio received a message it could not read.",
                                            f"not JSON: {e}: {line[:200]}"))
                continue
            if not isinstance(msg, dict):
                self.error(None, BadRequest("The Studio received a message it could not read.",
                                            "a request must be a JSON object"))
                continue
            try:
                self.dispatch(msg)
            except StudioError as e:  # a refused request (bad_request, not_found, ...): no traceback
                self.error(msg.get("id"), e)
            except Exception as e:  # noqa: BLE001 - one bad request never stops the server
                log.error("request %s failed: %s", msg.get("id"), traceback.format_exc())
                from .pipeline import map_exception

                self.error(msg.get("id"), map_exception(e), logged=True)
        self.stop(grace_s=10.0)
        return 0

    def dispatch(self, msg: dict) -> None:
        t = msg.get("type")
        rid = msg.get("id")
        quick = {
            "hello": self.on_hello, "models.status": self.on_models_status,
            "job.submit": self.on_job_submit, "job.cancel": self.on_job_cancel,
            "job.list": self.on_job_list, "shutdown": self.on_shutdown,
        }
        slow = {
            "probe": self.on_probe, "inspect": self.on_inspect, "lyrics.lookup": self.on_lyrics_lookup,
            "models.download": self.on_models_download, "realign": self.on_realign,
        }
        if t in quick:
            quick[t](rid, msg)
        elif t in slow:
            threading.Thread(target=self._guarded, args=(slow[t], rid, msg), name=f"req-{t}",
                             daemon=True).start()
        else:
            self.error(rid, StudioError("The Studio does not know this request.",
                                        f"unknown type {t!r}", code="unknown_type"))

    def _guarded(self, fn, rid, msg) -> None:
        try:
            fn(rid, msg)
        except Exception as e:  # noqa: BLE001
            from .pipeline import map_exception

            if isinstance(e, StudioError):
                self.error(rid, e)
            else:
                log.error("%s %s failed: %s", msg.get("type"), rid, traceback.format_exc())
                self.error(rid, map_exception(e), logged=True)

    # ------------------------------------------------------------------ session

    def on_hello(self, rid, msg) -> None:
        if msg.get("protocol") != PROTOCOL:
            self.error(rid, StudioError(f"This Studio speaks protocol {PROTOCOL}, the app speaks "
                                        f"{msg.get('protocol')}. Copy matching versions of the app and the Studio.",
                                        code="protocol_mismatch"))
            return
        if isinstance(msg.get("models_dir"), str) and msg["models_dir"]:
            self.models_dir = Path(msg["models_dir"])
            os.environ["JUKEOKE_MODELS_DIR"] = str(self.models_dir)
        if isinstance(msg.get("temp_dir"), str) and msg["temp_dir"]:
            try:
                self.temp_dir = studio_env.set_temp_dir(msg["temp_dir"])
            except OSError as e:
                log.warning("temp_dir %s not usable: %s", msg["temp_dir"], e)
        if isinstance(msg.get("library_dir"), str):
            self.library_dir = Path(msg["library_dir"])
        self.app_version = msg.get("app_version") if isinstance(msg.get("app_version"), str) else None
        from .pipeline import runtime_versions
        from .presets import PRESET_INFO

        rv = runtime_versions()
        self.send({"type": "hello.ok", "id": rid, "protocol": PROTOCOL, "studio_version": STUDIO_VERSION,
                   "python": rv["python"], "torch": rv["torch"], "presets": PRESET_INFO,
                   "default_preset": "full",
                   "stub_models": _stub(),
                   "paths": {"models_dir": str(self.models_dir) if self.models_dir else None,
                             "temp_dir": str(self.temp_dir),
                             "cache_dir": str(studio_env.current().cache_dir or "") or None}})
        # Warm PyTorch up in the background (the first probe and job are then quicker),
        # but only when the hardware could run the Studio at all.
        if not _stub() and not os.environ.get("JUKEOKE_STUDIO_FAKE_HW"):
            from . import probe as probemod

            threading.Thread(target=_maybe_warm, args=(probemod,), daemon=True, name="warm").start()

    def on_probe(self, rid, msg) -> None:
        from . import probe as probemod

        timeout = msg.get("timeout_s", 45.0)
        timeout = float(timeout) if isinstance(timeout, (int, float)) and timeout > 0 else 45.0
        r = probemod.probe(timeout_s=min(timeout, 300.0), fake=os.environ.get("JUKEOKE_STUDIO_FAKE_HW"))
        self.send({"type": "probe.result", "id": rid, **r})

    def on_shutdown(self, rid, msg) -> None:
        self.stop(grace_s=4.0)
        self.send({"type": "shutdown.ok", "id": rid})
        self.stopping.set()

    def stop(self, grace_s: float) -> None:
        """Cancels the running job (its temporary files are deleted by the pipeline) and
        forgets the queued ones; stops a model download (its ``.part`` file is deleted,
        D-170); waits up to ``grace_s`` for both to clean up."""
        from .modelzoo import download as dl

        with self.cv:
            self.queue.clear()
            cur = self.current
            self.stopping.set()
            self.cv.notify_all()
        deadline = time.monotonic() + grace_s
        if cur is not None:
            cur.hooks.cancel()
        if dl.abort_downloads():
            # The download thread sees ``stopping`` (or its closed connection), deletes its
            # .part file and leaves. On Windows the file is delete-on-close anyway, so even
            # a process that exits before this finishes leaves nothing behind.
            while dl.active_downloads() and time.monotonic() < deadline:
                time.sleep(0.05)
        if cur is not None:
            while self.current is cur and time.monotonic() < deadline:
                time.sleep(0.05)

    # ------------------------------------------------------------------ models

    def _model_entries(self, preset: str) -> list[dict]:
        from .modelzoo import hfmodels
        from .modelzoo.download import missing_files
        from .modelzoo.registry import get as reg_get
        from .presets import PRESETS

        out = []
        for role, mid in PRESETS[preset].items():
            spec = reg_get(mid)
            gone = missing_files(spec, self.models_dir)
            for f in spec.files:
                out.append({"key": role, "name": mid, "title": spec.title, "file": f.filename,
                            "size_bytes": f.size, "present": f not in gone, "licence": spec.licence,
                            "commercial": spec.commercial, "task": "separation",
                            "needed_for": "every song"})
        for key, needed in ((hfmodels.DEFAULT_ALIGNMENT, "every song with lyrics"),
                            (hfmodels.DEFAULT_TRANSCRIPTION, "songs without lyrics (transcription)")):
            m = hfmodels.get(key)
            out.append({"key": m.task, "name": m.repo, "title": m.title, "file": m.weight_file,
                        "size_bytes": m.size_bytes, "present": not hfmodels.missing(m, self.models_dir),
                        "licence": m.licence, "commercial": m.commercial, "task": m.task,
                        "needed_for": needed})
        return out

    def _preset_of(self, msg) -> str:
        from .presets import PRESETS

        preset = msg.get("preset") or "full"
        if preset not in PRESETS:
            raise BadRequest(f"Unknown quality preset {preset!r}.", f"known: {list(PRESETS)}")
        return preset

    def on_models_status(self, rid, msg) -> None:
        preset = self._preset_of(msg)
        if _stub():
            entries = [{"key": "stub", "name": "stub", "file": None, "size_bytes": 0, "present": True,
                        "licence": "test stub", "commercial": "yes", "task": "separation",
                        "needed_for": "tests"}]
        else:
            entries = self._model_entries(preset)
        self.send({"type": "models.status.result", "id": rid, "preset": preset, "models": entries,
                   "all_present": all(e["present"] for e in entries),
                   "download_bytes": sum(e["size_bytes"] for e in entries if not e["present"]),
                   "models_dir": str(self.models_dir) if self.models_dir else None})

    def on_models_download(self, rid, msg) -> None:
        """The only place the Studio downloads anything (besides ``lyrics.lookup``)."""
        from .modelzoo import hfmodels
        from .modelzoo.download import ensure_model
        from .modelzoo.registry import get as reg_get
        from .presets import PRESETS

        preset = self._preset_of(msg)
        if _stub():
            self.send({"type": "models.download.result", "id": rid, "ok": True, "downloaded": []})
            return
        if not self.download_lock.acquire(blocking=False):
            raise StudioError("A model download is already running.", code="bad_request")
        try:
            downloaded: list[str] = []

            def progress(name: str, done: int, total: int) -> None:
                self.send({"type": "models.progress", "id": rid, "name": name,
                           "done_bytes": int(done), "total_bytes": int(total)})

            stop = self.stopping.is_set
            try:
                for mid in dict.fromkeys(PRESETS[preset].values()):
                    spec = reg_get(mid)
                    from .modelzoo.download import missing_files

                    if missing_files(spec, self.models_dir):
                        ensure_model(spec, self.models_dir, allow_download=True, progress=progress,
                                     should_stop=stop, verify=False)
                        downloaded.append(mid)
                for key in (hfmodels.DEFAULT_ALIGNMENT, hfmodels.DEFAULT_TRANSCRIPTION):
                    m = hfmodels.get(key)
                    if hfmodels.missing(m, self.models_dir):
                        hfmodels.ensure(m, self.models_dir, allow_download=True, progress=progress,
                                        should_stop=stop)
                        downloaded.append(m.repo)
            except StudioError:
                raise
            except InterruptedError as e:
                raise StudioError("The download was stopped.", str(e), code="cancelled") from e
            except OSError as e:
                raise StudioError("The models could not be downloaded. Check the internet connection and "
                                  "the free space on the drive, then try again.",
                                  f"{type(e).__name__}: {e}", code="internal") from e
            self.send({"type": "models.download.result", "id": rid, "ok": True, "downloaded": downloaded})
        finally:
            self.download_lock.release()

    # ------------------------------------------------------------------ files

    def on_inspect(self, rid, msg) -> None:
        from . import media

        path = msg.get("input")
        if not isinstance(path, str) or not path:
            raise BadRequest("inspect needs an input path.")
        if not os.path.isfile(path):
            raise NotFound("That file could not be found.", path)
        lang = msg.get("language") if isinstance(msg.get("language"), str) else "en"
        r = media.inspect(path, self.temp_dir / "covers", language=lang)
        self.send({"type": "inspect.result", "id": rid, **r})

    def on_lyrics_lookup(self, rid, msg) -> None:
        from .lyrics import LyricsLookupError, lookup_lrclib

        title = msg.get("title")
        if not isinstance(title, str) or not title.strip():
            raise BadRequest("A title is needed to look the lyrics up.")
        artist = msg.get("artist") if isinstance(msg.get("artist"), str) else None
        album = msg.get("album") if isinstance(msg.get("album"), str) else None
        dur = msg.get("duration_seconds")
        dur = float(dur) if isinstance(dur, (int, float)) and not isinstance(dur, bool) else None
        try:
            results = lookup_lrclib(artist, title.strip(), album, dur)
        except LyricsLookupError as e:
            raise StudioError(str(e), code="lookup_failed") from e
        self.send({"type": "lyrics.lookup.result", "id": rid, "results": results})

    # ------------------------------------------------------------------ jobs

    def on_job_submit(self, rid, msg) -> None:
        from .pipeline import parse_job

        spec = parse_job(msg.get("job"))
        with self.cv:
            if spec.job_id in self.jobs:
                raise BadRequest("A job with this id was already submitted.", spec.job_id)
            if self.stopping.is_set():
                raise StudioError("The Studio is shutting down.", code="internal")
            self.jobs[spec.job_id] = Job(spec)
            self.queue.append(spec.job_id)
            position = len(self.queue) + (1 if self.current is not None else 0)
            self.cv.notify_all()
        self.send({"type": "job.accepted", "id": rid, "job_id": spec.job_id, "position": position})

    def on_job_cancel(self, rid, msg) -> None:
        jid = msg.get("job_id")
        with self.cv:
            job = self.jobs.get(jid) if isinstance(jid, str) else None
            if job is None:
                raise NotFound("This job is not in the Studio's queue.", f"job_id {jid!r}")
            queued = job.state == "queued"
            if queued:
                try:
                    self.queue.remove(jid)
                except ValueError:
                    pass
                job.state, job.error_code = "failed", "cancelled"
            elif job.state == "running":
                job.hooks.cancel()
        self.send({"type": "job.cancel.result", "id": rid, "ok": True})
        if queued:
            self.send({"type": "job.failed", "job_id": jid, "error": {"code": "cancelled", "message": "Cancelled."}})

    def on_job_list(self, rid, msg) -> None:
        with self.cv:
            jobs = [{"job_id": j.spec.job_id, "state": j.state, "stage": j.stage,
                     "overall": round(j.overall, 4), "error_code": j.error_code, "output": j.output,
                     "input": j.spec.input} for j in self.jobs.values()]
        self.send({"type": "job.list.result", "id": rid, "jobs": jobs})

    def _worker(self) -> None:
        from .pipeline import JobContext, map_exception, run_job

        while True:
            with self.cv:
                while not self.queue and not self.stopping.is_set():
                    self.cv.wait()
                if self.stopping.is_set() and not self.queue:
                    return
                jid = self.queue.popleft()
                job = self.jobs[jid]
                if job.state != "queued":
                    continue
                job.state = "running"
                self.current = job
            self.send({"type": "job.started", "job_id": jid})

            def on_progress(stage: str, sp: float, overall: float, message: str | None, job=job, jid=jid) -> None:
                job.stage, job.overall = stage, overall
                self.send({"type": "job.progress", "job_id": jid, "stage": stage,
                           "stage_progress": round(sp, 4), "overall": round(overall, 4), "message": message})

            ctx = JobContext(models_dir=self.models_dir, temp_dir=self.temp_dir,
                             app_version=self.app_version, app_name=self.app_name, gpu_lock=self.gpu_lock)
            t0 = time.perf_counter()
            try:
                res = run_job(job.spec, ctx, job.hooks, on_progress)
                job.state, job.output, job.stage = "done", str(res.output), None
                log.info("job %s done in %.1f s: %s", jid, time.perf_counter() - t0, res.output)
                self.send({"type": "job.done", "job_id": jid, "output": str(res.output), "report": res.report})
            except BaseException as e:  # noqa: BLE001 - a failing file never stops the queue
                err = map_exception(e)
                if err.code == "cancelled":
                    log.info("job %s cancelled", jid)
                elif is_expected(err):
                    # The user's file or choices (a damaged file, missing models, lyrics
                    # that do not fit): one line, no traceback (D-173).
                    log.warning("job %s failed (%s): %s%s", jid, err.code, err.message,
                                f" [{err.detail}]" if err.detail else "")
                else:
                    log.error("job %s failed (%s): %s\n%s", jid, err.code, err.detail,
                              "".join(traceback.format_exception(e)))
                job.state, job.error_code = "failed", err.code
                self.send({"type": "job.failed", "job_id": jid, "error": err.to_json()})
                # Verifier finding: ``err`` is often the exception itself (Cancelled and every
                # StudioError), and its traceback keeps the failed stage's frames alive - the
                # loaded network, the GPU input and the overlap-add buffers. Bound to ``err``
                # it outlived the job (measured with the real models: 309 MiB still allocated
                # after a job cancelled in separation, and still after the next job finished;
                # 32 MiB with this fix). Drop it, then give the memory back below.
                del err
                failed = True
            else:
                failed = False
            finally:
                with self.cv:
                    self.current = None
            if failed:
                from .pipeline import free_vram

                free_vram()

    # ------------------------------------------------------------------ realign

    def on_realign(self, rid, msg) -> None:
        from . import jksong
        from .align import Anchor  # noqa: F401  (validates the module imports)
        from .errors import AlignmentFailed, LyricsMissingError
        from .lyrics import valid_source
        from .pipeline import Models, free_vram

        path = msg.get("song")
        if not isinstance(path, str) or not path:
            raise BadRequest("realign needs the song file.")
        if not os.path.isfile(path):
            raise NotFound("That song file could not be found.", path)
        lyr = msg.get("lyrics") or {}
        if not isinstance(lyr, dict):
            raise BadRequest("lyrics must be an object.")
        mode = (lyr.get("mode") or "text").lower()
        if mode not in ("text", "lrc"):
            raise BadRequest("realign takes lyrics with mode \"text\" or \"lrc\".")
        text = lyr.get(mode) if mode == "lrc" else lyr.get("text")
        if not isinstance(text, str) or not text.strip():
            raise LyricsMissingError("There are no lyrics to align.")
        anchors = msg.get("anchors") or []
        if not isinstance(anchors, list) or not all(
                isinstance(a, dict) and isinstance(a.get("line"), int) and not isinstance(a.get("line"), bool)
                and isinstance(a.get("start"), (int, float)) and not isinstance(a.get("start"), bool)
                for a in anchors):
            raise BadRequest("anchors must be a list of {line, start}.")
        try:
            song = jksong.open_song(path)
        except jksong.JkSongError as e:
            raise StudioError("The song file could not be read.", f"{type(e).__name__}: {e}",
                              code="decode_failed") from e
        old = None
        try:
            old = song.effective_lyrics()
        except Exception:  # noqa: BLE001
            old = None
        source = lyr.get("source")
        if source is not None and (not isinstance(source, str) or not valid_source(source)):
            if source != "transcribed":
                raise BadRequest("lyrics.source is not valid.", repr(source))
        if source is None:
            source = (old or {}).get("source") or "user_text"
        language = msg.get("language") if isinstance(msg.get("language"), str) else (old or {}).get("language", "en")
        sr = int(song.manifest["sample_rate"])
        lead = song.read_stem("lead_vocal")
        duration = lead.shape[1] / sr
        hooks = Hooks(lambda f, m: self.send({"type": "realign.progress", "id": rid, "progress": round(f, 3)}))
        models = Models(self.models_dir)
        with self.gpu_lock:
            al = models.aligner()
            try:
                r = al.realign(lead, sr, text, anchors, language=language or "en", source=source,
                               lrc=(mode == "lrc"), hooks=hooks)
            except StudioError:
                raise
            except Exception as e:  # noqa: BLE001
                from .pipeline import _is_oom

                if _is_oom(e):
                    raise
                raise AlignmentFailed("The lyrics could not be aligned to the song.",
                                      f"{type(e).__name__}: {e}") from e
            finally:
                del al
                free_vram()
        problems = jksong.validate_lyrics(r.lyrics, duration)
        if problems:
            raise AlignmentFailed("The lyrics could not be aligned to the song.", "; ".join(problems[:5]))
        self.send({"type": "realign.result", "id": rid, "ok": True, "lyrics": r.lyrics,
                   "warnings": r.warnings, "dropped_anchors": r.dropped_anchors})


# ---------------------------------------------------------------------- helpers

def _stub() -> bool:
    from . import stub

    return stub.enabled()


def _maybe_warm(probemod) -> None:
    try:
        info = probemod.cuda_driver_info()
        ram = probemod.ram_mb() or 0
        if info.get("count") and (info.get("vram_mb") or 0) >= probemod.MIN_VRAM_MB and ram >= probemod.MIN_RAM_MB:
            probemod.start_torch_check()
    except Exception:  # noqa: BLE001
        pass


def _app_name() -> str:
    """The product name from branding.json (one place for a rename), when around."""
    try:
        here = Path(__file__).resolve()
        for parent in list(here.parents)[:5]:
            b = parent / "branding.json"
            if b.is_file():
                return json.loads(b.read_text(encoding="utf-8")).get("name", "Jukeoke")
    except Exception:  # noqa: BLE001
        pass
    return "Jukeoke"


def _json_default(o):
    import numpy as np

    if isinstance(o, (np.floating,)):
        return float(o)
    if isinstance(o, (np.integer,)):
        return int(o)
    if isinstance(o, Path):
        return str(o)
    raise TypeError(f"not JSON serialisable: {type(o).__name__}")


def main(out: studio_env.ProtocolOut, models_dir: Path | None = None, inp=None) -> int:
    srv = Server(out, models_dir, inp)
    try:
        code = srv.serve()
    except KeyboardInterrupt:
        code = 0
    try:
        out.close()
    finally:
        logging.shutdown()
    return code

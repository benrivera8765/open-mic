# SPDX-License-Identifier: GPL-3.0-or-later
"""A fake Studio engine for testing the app's Studio supervisor.

Speaks docs/STUDIO_PROTOCOL.md (version 1) on stdin/stdout with JSON lines, using
only the Python standard library. Nothing here touches audio: a "job" reports fake
progress through every stage and then writes a copy of a fixture song into the
output folder, atomically (temp file + rename), named "<artist> - <title>.jksong"
with a " (2)" suffix when the name exists.

    python fake_studio.py serve

Behaviour switches (environment variables), so tests can provoke every path:

    FAKE_STUDIO_FIXTURE      .jksong copied as the "made" song (required for jobs)
    FAKE_STUDIO_STEP_S       seconds per progress step (default 0.02)
    FAKE_STUDIO_HELLO_DELAY  seconds to wait before answering hello (timeout tests)
    FAKE_STUDIO_PROTOCOL     protocol number to claim (mismatch tests)
    FAKE_STUDIO_PROBE        "ok" (default), "no_gpu" or "hang"
    FAKE_STUDIO_MODELS       "present" (default) or "missing"

Input file names steer a job: a name containing "crash" makes the process exit in
the middle of the separate stage; "baddecode" fails the job with decode_failed;
"slow" makes every step a hundred times longer (for cancel tests).
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import sys
import threading
import time

PROTOCOL = int(os.environ.get("FAKE_STUDIO_PROTOCOL", "1"))
STEP_S = float(os.environ.get("FAKE_STUDIO_STEP_S", "0.02"))
STAGES = ["decode", "separate", "lyrics", "align", "analyse", "write"]
INVALID = '<>:"/\\|?*'

_out_lock = threading.Lock()
_jobs: dict[str, dict] = {}
_queue: "queue.Queue[str]" = queue.Queue()
# Separation models per preset (like studio/jukeoke_studio/presets.py: "clean" adds one
# model to "full", "permissive" shares none of full's vocal models). FAKE_STUDIO_MODELS:
# "present" = the default preset's models are on the drive (as on the owner's stick),
# "missing" = none are.
_PRESET_MODELS = {"full": ["fake-vocals"], "clean": ["fake-vocals", "fake-inst"], "permissive": ["fake-perm-vocals"]}
_LICENCES = {"fake-vocals": ("GPL-3.0", "yes"), "fake-inst": ("none published", "unclear"),
             "fake-perm-vocals": ("MIT", "yes")}
_on_disk: set = set(_PRESET_MODELS["full"]) if os.environ.get("FAKE_STUDIO_MODELS", "present") == "present" else set()
_session: dict = {}


def send(msg: dict) -> None:
    line = json.dumps(msg, ensure_ascii=False)
    with _out_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def log(text: str) -> None:
    sys.stderr.write(f"[fake-studio] {text}\n")
    sys.stderr.flush()


def error(req_id, code: str, message: str, detail: str = "") -> None:
    send({"type": "error", "id": req_id, "error": {"code": code, "message": message, "detail": detail}})


def safe_name(s: str) -> str:
    s = "".join("_" if c in INVALID or ord(c) < 32 else c for c in s).strip().rstrip(". ")
    return s or "Song"


def output_path(out_dir: str, artist: str, title: str) -> str:
    stem = safe_name(f"{artist} - {title}" if artist else title)
    path = os.path.join(out_dir, stem + ".jksong")
    n = 2
    while os.path.exists(path):
        path = os.path.join(out_dir, f"{stem} ({n}).jksong")
        n += 1
    return path


def run_job(job: dict) -> None:
    jid = job["job_id"]
    inp = job["input"]
    name = os.path.basename(inp).lower()
    step = STEP_S * (100 if "slow" in name else 1)
    job["state"] = "running"
    send({"type": "job.started", "job_id": jid})
    for i, stage in enumerate(STAGES):
        for k in range(4):
            if job.get("cancel"):
                job["state"] = "failed"
                send({"type": "job.failed", "job_id": jid, "error": {"code": "cancelled", "message": "Cancelled."}})
                return
            if stage == "decode" and "baddecode" in name:
                job["state"] = "failed"
                send({"type": "job.failed", "job_id": jid,
                      "error": {"code": "decode_failed", "message": "This file could not be read as audio.",
                                "detail": "fake decoder refused " + inp}})
                return
            if stage == "separate" and k == 2 and "crash" in name:
                log("simulating a crash")
                sys.stdout.flush()
                os._exit(3)
            time.sleep(step)
            sp = (k + 1) / 4
            send({"type": "job.progress", "job_id": jid, "stage": stage, "stage_progress": sp,
                  "overall": round((i + sp) / len(STAGES), 4), "message": f"Fake {stage}"})
    fixture = os.environ.get("FAKE_STUDIO_FIXTURE")
    if not fixture or not os.path.isfile(fixture):
        job["state"] = "failed"
        send({"type": "job.failed", "job_id": jid,
              "error": {"code": "write_failed", "message": "The song file could not be written.",
                        "detail": "FAKE_STUDIO_FIXTURE not set"}})
        return
    meta = job.get("metadata") or {}
    title = meta.get("title") or os.path.splitext(os.path.basename(inp))[0]
    artist = meta.get("artist") or ""
    os.makedirs(job["output_dir"], exist_ok=True)
    dest = output_path(job["output_dir"], artist, title)
    tmp = dest + f".tmp-{os.getpid():x}"
    shutil.copyfile(fixture, tmp)
    os.replace(tmp, dest)
    job["state"] = "done"
    send({"type": "job.done", "job_id": jid, "output": dest,
          "report": {"seconds": {s: step * 4 for s in STAGES}, "lyrics_source": "user_text",
                     "lyrics_checked": False, "warnings": []}})


def worker() -> None:
    while True:
        jid = _queue.get()
        job = _jobs[jid]
        if job.get("cancel"):
            continue
        try:
            run_job(job)
        except Exception as e:  # a bad job never stops the queue
            job["state"] = "failed"
            send({"type": "job.failed", "job_id": jid, "error": {"code": "internal", "message": "Internal error.",
                                                                 "detail": repr(e)}})


def handle(msg: dict) -> bool:
    """Returns False when the server should exit."""
    t = msg.get("type")
    rid = msg.get("id")
    if t == "hello":
        delay = float(os.environ.get("FAKE_STUDIO_HELLO_DELAY", "0"))
        if delay:
            time.sleep(delay)
        if msg.get("protocol") != PROTOCOL:
            error(rid, "protocol_mismatch", f"The Studio speaks protocol {PROTOCOL}.")
            return True
        _session.update(msg)
        send({"type": "hello.ok", "id": rid, "protocol": PROTOCOL, "studio_version": "0.0.0-fake",
              "python": sys.version.split()[0], "torch": None})
    elif t == "probe":
        mode = os.environ.get("FAKE_STUDIO_PROBE", "ok")
        if mode == "hang":
            return True
        if mode == "no_gpu":
            send({"type": "probe.result", "id": rid, "ok_for_studio": False, "cuda": False, "gpu": None,
                  "vram_mb": None, "compute_capability": None, "ram_mb": 8192,
                  "reasons": ["No NVIDIA graphics card with CUDA was found. Songs are created on a stronger "
                              "computer and can be played anywhere."]})
        else:
            send({"type": "probe.result", "id": rid, "ok_for_studio": True, "cuda": True,
                  "gpu": "Fake GPU 9000", "vram_mb": 16303, "compute_capability": "12.0", "ram_mb": 32652,
                  "reasons": []})
    elif t in ("models.status", "models.download"):
        preset = msg.get("preset") or "full"
        if preset not in _PRESET_MODELS:
            error(rid, "bad_request", f"Unknown quality preset {preset!r}.")
            return True
        names = _PRESET_MODELS[preset]
        if t == "models.status":
            models = [{"key": "vocals", "name": n, "file": f"{n}.ckpt", "size_bytes": 1000, "present": n in _on_disk,
                       "licence": _LICENCES[n][0], "commercial": _LICENCES[n][1], "task": "separation",
                       "needed_for": "every song"} for n in names]
            send({"type": "models.status.result", "id": rid, "preset": preset, "models": models,
                  "all_present": all(m["present"] for m in models),
                  "download_bytes": sum(m["size_bytes"] for m in models if not m["present"])})
        else:
            downloaded = []
            for n in names:
                if n in _on_disk:
                    continue
                for done in (0, 500, 1000):
                    send({"type": "models.progress", "id": rid, "name": f"{n}.ckpt", "done_bytes": done,
                          "total_bytes": 1000})
                    time.sleep(STEP_S)
                _on_disk.add(n)
                downloaded.append(n)
            send({"type": "models.download.result", "id": rid, "ok": True, "downloaded": downloaded})
    elif t == "inspect":
        path = msg.get("input", "")
        if not os.path.isfile(path):
            error(rid, "not_found", "That file could not be found.")
            return True
        stem = os.path.splitext(os.path.basename(path))[0]
        artist, _, title = stem.partition(" - ")
        if not title:
            artist, title = "", stem
        send({"type": "inspect.result", "id": rid, "ok": True, "title": title, "artist": artist, "album": None,
              "duration_seconds": 2.5, "container": "wav", "codec": "pcm_f32le", "sample_rate": 48000,
              "channels": 2, "bits_per_sample": 32, "lossy": False, "has_video": False,
              "embedded_lyrics": {"present": False, "synced": False, "text": None}, "cover_path": None})
    elif t == "lyrics.lookup":
        send({"type": "lyrics.lookup.result", "id": rid, "results": [
            {"service": "fake", "synced_lrc": "[00:01.00] Twinkle, twinkle, little star",
             "plain": "Twinkle, twinkle, little star", "duration_seconds": msg.get("duration_seconds")}]})
    elif t == "job.submit":
        job = dict(msg.get("job") or {})
        jid = job.get("job_id")
        if not jid or not job.get("input"):
            error(rid, "bad_request", "The job needs a job_id and an input.")
            return True
        job["state"] = "queued"
        _jobs[jid] = job
        position = sum(1 for j in _jobs.values() if j["state"] in ("queued", "running"))
        send({"type": "job.accepted", "id": rid, "job_id": jid, "position": position})
        _queue.put(jid)
    elif t == "job.cancel":
        job = _jobs.get(msg.get("job_id"))
        if job is None:
            error(rid, "not_found", "No such job.")
            return True
        send({"type": "job.cancel.result", "id": rid, "ok": True})
        if job["state"] == "queued":
            job["cancel"] = True
            job["state"] = "failed"
            send({"type": "job.failed", "job_id": job["job_id"], "error": {"code": "cancelled", "message": "Cancelled."}})
        else:
            job["cancel"] = True
    elif t == "job.list":
        send({"type": "job.list.result", "id": rid,
              "jobs": [{"job_id": j["job_id"], "state": j["state"], "stage": None} for j in _jobs.values()]})
    elif t == "realign":
        text = (msg.get("lyrics") or {}).get("text") or ""
        lines, t0 = [], 1.0
        for raw in [l for l in text.splitlines() if l.strip()]:
            words = []
            for w in raw.split():
                chars = []
                for ch in w:
                    chars.append([ch, round(t0, 3), round(t0 + 0.05, 3)])
                    t0 += 0.05
                words.append({"start": chars[0][1], "end": chars[-1][2], "chars": chars})
                t0 += 0.05
            lines.append({"start": words[0]["start"], "end": words[-1]["end"], "words": words})
        send({"type": "realign.result", "id": rid, "ok": True,
              "lyrics": {"lyrics_version": 1, "language": "en", "source": "user_text", "checked": True,
                         "alignment": {"method": "fake", "anchored_lines": bool(msg.get("anchors"))},
                         "lines": lines}})
    elif t == "shutdown":
        send({"type": "shutdown.ok", "id": rid})
        return False
    else:
        error(rid, "unknown_type", f"Unknown message type {t!r}.")
    return True


def serve() -> int:
    log(f"fake Studio started, pid {os.getpid()}")
    threading.Thread(target=worker, daemon=True).start()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            error(None, "bad_request", "That was not JSON.")
            continue
        if not handle(msg):
            break
    log("fake Studio exiting")
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "serve":
        sys.exit(serve())
    print(__doc__, file=sys.stderr)
    sys.exit(2)

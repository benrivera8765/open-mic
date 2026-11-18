# SPDX-License-Identifier: GPL-3.0-or-later
"""A small client that starts ``python -I -m jukeoke_studio serve`` and speaks the
protocol with it, the way the app does (tests only)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

STUDIO_DIR = Path(__file__).resolve().parents[1]


class StudioProc:
    def __init__(self, root: Path, *, stub: bool = True, fake_hw: str | None = "ok",
                 env: dict | None = None, models_dir: Path | None = None, args: list[str] | None = None):
        self.root = Path(root)
        e = dict(os.environ)
        for k in ("JUKEOKE_STUDIO_STUB", "JUKEOKE_STUDIO_FAKE_HW", "JUKEOKE_STUDIO_STUB_DELAY",
                  "JUKEOKE_ROOT", "JUKEOKE_MODELS_DIR"):
            e.pop(k, None)
        if stub:
            e["JUKEOKE_STUDIO_STUB"] = "1"
        if fake_hw:
            e["JUKEOKE_STUDIO_FAKE_HW"] = fake_hw
        e.update(env or {})
        cmd = [sys.executable, "-I", "-m", "jukeoke_studio", "serve", "--root", str(self.root)]
        if models_dir is not None:
            cmd += ["--models-dir", str(models_dir)]
        cmd += args or []
        # -I ignores PYTHONPATH: the package must be installed in this interpreter (the
        # dev venv has it as an editable install), as in the packaged runtime.
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  env=e, cwd=str(self.root.parent))
        self.msgs: list[dict] = []
        self.bad_lines: list[bytes] = []
        self.stderr: list[str] = []
        self._cv = threading.Condition()
        self._ids = 0
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._read_err, daemon=True).start()

    def _read(self) -> None:
        for raw in self.p.stdout:
            try:
                m = json.loads(raw)
            except ValueError:
                self.bad_lines.append(raw)
                continue
            with self._cv:
                self.msgs.append(m)
                self._cv.notify_all()
        with self._cv:
            self._cv.notify_all()

    def _read_err(self) -> None:
        for raw in self.p.stderr:
            self.stderr.append(raw.decode("utf-8", "replace"))

    def send(self, msg: dict) -> str:
        if "id" not in msg:
            self._ids += 1
            msg = {**msg, "id": f"t{self._ids}"}
        self.p.stdin.write((json.dumps(msg) + "\n").encode("utf-8"))
        self.p.stdin.flush()
        return msg["id"]

    def wait_for(self, pred, timeout: float = 60.0) -> dict:
        deadline = time.monotonic() + timeout
        with self._cv:
            while True:
                for m in self.msgs:
                    if pred(m):
                        return m
                left = deadline - time.monotonic()
                if left <= 0 or (self.p.poll() is not None and not any(pred(m) for m in self.msgs)):
                    raise TimeoutError(f"no matching message; got {[x.get('type') for x in self.msgs][-20:]}\n"
                                       f"stderr tail:\n{''.join(self.stderr)[-3000:]}")
                self._cv.wait(min(left, 0.2))

    def request(self, msg: dict, timeout: float = 60.0) -> dict:
        rid = self.send(msg)
        return self.wait_for(lambda m: m.get("id") == rid and m.get("type") not in
                             ("models.progress", "realign.progress"), timeout)

    def hello(self, **kw) -> dict:
        msg = {"type": "hello", "protocol": 1, "app_version": "0.1.0-test", "root": str(self.root),
               "models_dir": str(kw.pop("models_dir", self.root / "Models")),
               "library_dir": str(self.root / "Library"), "temp_dir": str(self.root / "Settings/tmp/studio")}
        msg.update(kw)
        return self.request(msg)

    def job_end(self, job_id: str, timeout: float = 120.0) -> dict:
        return self.wait_for(lambda m: m.get("job_id") == job_id and m.get("type") in ("job.done", "job.failed"),
                             timeout)

    def events(self, job_id: str) -> list[dict]:
        with self._cv:
            return [m for m in self.msgs if m.get("job_id") == job_id and m.get("type", "").startswith("job.")
                    and m.get("type") != "job.accepted"]

    def close(self, timeout: float = 30.0) -> int:
        try:
            self.p.stdin.close()
        except OSError:
            pass
        try:
            return self.p.wait(timeout)
        except subprocess.TimeoutExpired:
            self.p.kill()
            return self.p.wait(10)


def submit(st: StudioProc, job_id: str, inp: Path, out: Path, **kw) -> dict:
    job = {"job_id": job_id, "input": str(inp), "output_dir": str(out), "preset": kw.pop("preset", "full"),
           "language": "en", "metadata": kw.pop("metadata", {}),
           "lyrics": kw.pop("lyrics", {"mode": "text", "text": "hello world\nsecond line here", "lrc": None})}
    job.update(kw)
    return st.request({"type": "job.submit", "job": job})

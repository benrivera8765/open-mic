# SPDX-License-Identifier: GPL-3.0-or-later
"""The protocol server, spoken to over a real pipe (CPU; stand-in models).

Each test starts ``python -I -m jukeoke_studio serve --root <tmp>`` with
``JUKEOKE_STUDIO_STUB=1`` (fast stand-ins for the separation / alignment / Whisper
models, stub.py) and ``JUKEOKE_STUDIO_FAKE_HW`` (probe answers without PyTorch), and
speaks docs/STUDIO_PROTOCOL.md with it like the app does. Everything else is the real
code: PyAV decoding, lyrics, the CTC aligner's constraint logic, key/tempo analysis,
the .jksong writer, naming, cancellation, the queue. Media files are generated.
"""

from __future__ import annotations

import glob
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import media_fixtures as MF  # noqa: E402
from studio_client import StudioProc, submit  # noqa: E402

from jukeoke_studio import jksong  # noqa: E402


def _cli() -> str | None:
    """jukeoke-cli for `verify --strict` (Rust reader), if built on this machine."""
    c = os.environ.get("JUKEOKE_CLI")
    if c and Path(c).is_file():
        return c
    jk = os.environ.get("JK_BUILD")
    if jk:
        hits = sorted(glob.glob(os.path.join(jk, "target-*", "release", "jukeoke-cli.exe")),
                      key=os.path.getmtime, reverse=True)
        if hits:
            return hits[0]
    return shutil.which("jukeoke-cli")


def verify_strict(song: Path) -> None:
    """``jukeoke-cli verify --strict`` must pass with no warning, for every preset
    ("permissive" included since crates/jk-format verify.rs knows it)."""
    cli = _cli()
    if cli is None:
        return
    r = subprocess.run([cli, "verify", "--strict", str(song)], capture_output=True, text=True, timeout=120)
    assert r.returncode == 0, r.stdout + r.stderr
    assert "PASS: 0 errors, 0 warnings" in r.stdout


@pytest.fixture
def root(tmp_path):
    r = tmp_path / "stick"
    (r / "Library").mkdir(parents=True)
    return r


@pytest.fixture
def studio(root):
    st = StudioProc(root)
    yield st
    st.close()
    assert not st.bad_lines, f"non-JSON on stdout: {st.bad_lines[:3]}"


def _sum_error(path: Path, source: np.ndarray) -> float:
    song = jksong.open_song(path)
    total = sum(song.read_stem_int(s["role"]).T.astype(np.float64) * (float(s["gain"]) / jksong.FULL_SCALE)
                for s in song.manifest["stems"])
    return float(np.max(np.abs(total - source)))


# ------------------------------------------------------------------ session basics

def test_hello_probe_unknown_and_bad_requests(studio, root):
    ok = studio.hello()
    assert ok["type"] == "hello.ok" and ok["protocol"] == 1 and ok["studio_version"]
    assert ok["python"].startswith("3.12") and set(ok["presets"]) == {"full", "clean", "permissive"}
    assert ok["presets"]["permissive"]["backing_vocals"] is False
    # Caches inside the stick (WORKING ORDER 4.8).
    assert Path(ok["paths"]["cache_dir"]) == (root / "Settings" / "cache").resolve()
    pr = studio.request({"type": "probe"})
    assert pr["type"] == "probe.result" and pr["ok_for_studio"] is True and pr["reasons"] == []
    err = studio.request({"type": "no.such.thing"})
    assert err["type"] == "error" and err["error"]["code"] == "unknown_type"
    err = studio.request({"type": "job.submit", "job": {"input": "x"}})
    assert err["error"]["code"] == "bad_request"
    err = studio.request({"type": "job.cancel", "job_id": "nope"})
    assert err["error"]["code"] == "not_found"
    # The lookup needs a title (no network call is made for a bad request).
    err = studio.request({"type": "lyrics.lookup", "artist": "x"})
    assert err["error"]["code"] == "bad_request"
    err = studio.request({"type": "models.status", "preset": "nope"})
    assert err["error"]["code"] == "bad_request"
    # A line that is not JSON gets an error without an id, and the server carries on.
    studio.p.stdin.write(b"this is not json\n")
    studio.p.stdin.flush()
    studio.wait_for(lambda m: m.get("type") == "error" and m.get("id") is None)
    assert studio.request({"type": "job.list"})["jobs"] == []


def test_protocol_mismatch(studio):
    r = studio.request({"type": "hello", "protocol": 2})
    assert r["type"] == "error" and r["error"]["code"] == "protocol_mismatch"


@pytest.mark.parametrize("fake,reason", [("none", "No NVIDIA graphics card"), ("small", "graphics card has 4.0 GB")])
def test_probe_weak_computer_is_fast_and_plain(root, fake, reason):
    st = StudioProc(root, fake_hw=fake)
    try:
        r = st.request({"type": "probe"}, timeout=30)
        assert r["ok_for_studio"] is False and r["checked_with"] == "driver"
        assert any(reason in x for x in r["reasons"]), r["reasons"]
        assert all("stronger computer" in x for x in r["reasons"])
        assert r["seconds"] < 1.0  # no PyTorch import on the fast path
    finally:
        st.close()


def test_shutdown_answers_and_exits(studio):
    studio.hello()
    r = studio.request({"type": "shutdown"})
    assert r["type"] == "shutdown.ok"
    assert studio.p.wait(30) == 0


def test_exits_when_stdin_closes(studio):
    studio.hello()
    assert studio.close() == 0


# --------------------------------------------------------------------------- jobs

def test_job_events_output_and_song(studio, root, tmp_path):
    studio.hello()
    src = MF.make("wav24", tmp_path, seconds=6.0, name="My Song.wav")
    acc = submit(studio, "j1", src, root / "Library",
                 metadata={"title": "Corrected: Title?", "artist": "The Artist"},
                 lyrics={"mode": "lrc", "lrc": "[00:00.50]hello world\n[00:03.00]second line here\n",
                         "source": "online:lrclib"})
    assert acc["type"] == "job.accepted" and acc["job_id"] == "j1" and acc["position"] == 1
    done = studio.job_end("j1")
    assert done["type"] == "job.done", done
    ev = studio.events("j1")
    types = [e["type"] for e in ev]
    assert types[0] == "job.started" and types[-1] == "job.done"
    stages = [e["stage"] for e in ev if e["type"] == "job.progress"]
    assert list(dict.fromkeys(stages)) == ["decode", "separate", "lyrics", "align", "analyse", "write"]
    overall = [e["overall"] for e in ev if e["type"] == "job.progress"]
    assert overall == sorted(overall) and all(0 <= o <= 1 for o in overall)
    out = Path(done["output"])
    # Name sanitised for Windows/macOS/exFAT (":" and "?" replaced).
    assert out.name == "The Artist - Corrected_ Title_.jksong" and out.parent == root / "Library"
    rep = done["report"]
    assert set(rep["seconds"]) == {"decode", "separate", "lyrics", "align", "analyse", "write"}
    assert rep["lyrics_source"] == "online:lrclib" and rep["lyrics_checked"] is True
    assert rep["stems_sum_max_error"] <= 0.5 / jksong.FULL_SCALE + 1e-12
    song = jksong.open_song(out)
    m = song.manifest
    assert m["title"] == "Corrected: Title?" and m["artist"] == "The Artist"
    assert m["sample_rate"] == 44100 and m["channels"] == 2 and m["stems_sum_to_source"] is True
    assert [s["role"] for s in m["stems"]] == ["lead_vocal", "backing_vocals", "drums", "other"]
    prov = m["provenance"]
    assert prov["source"]["sha256"] == hashlib.sha256(src.read_bytes()).hexdigest()
    assert prov["source"]["bits_per_sample"] == 24 and prov["source"]["lossy"] is False
    assert prov["source"]["filename"] == "My Song.wav" and prov["resampling"] == []
    assert prov["app"]["version"] == "0.1.0-test" and prov["studio"]["python"].startswith("3.12")
    assert prov["models"] and all(x["name"].startswith("stub-") for x in prov["models"])
    lyr = song.lyrics()
    assert lyr["source"] == "online:lrclib" and lyr["checked"] is True
    assert [" ".join("".join(c[0] for c in w["chars"]) for w in ln["words"]) for ln in lyr["lines"]] == \
        ["hello world", "second line here"]
    # Exactly what was decoded: the 24-bit source maps to float32 exactly, and the stems
    # sum back to it.
    x = MF.signal(6.0)
    q = np.round(np.clip(x, -1, 8388607 / 8388608) * 8388608) / 8388608
    assert _sum_error(out, q.astype(np.float64)) <= 0.5 / jksong.FULL_SCALE + 1e-12
    verify_strict(out)
    # Nothing temporary left anywhere.
    assert not list((root / "Library").glob("*.tmp-*"))
    assert not list((root / "Settings" / "tmp" / "studio").glob("job-*"))
    lst = studio.request({"type": "job.list"})["jobs"]
    assert lst[0]["state"] == "done" and lst[0]["output"] == str(out)


def test_same_name_gets_a_suffix_and_never_overwrites(studio, root, tmp_path):
    studio.hello()
    src = MF.make("wav16", tmp_path, seconds=3.0)
    (root / "Library" / "A - T.jksong").write_bytes(b"someone else's song")
    for jid in ("a", "b"):
        submit(studio, jid, src, root / "Library", metadata={"title": "T", "artist": "A"},
               lyrics={"mode": "none"})
    names = [Path(studio.job_end(j)["output"]).name for j in ("a", "b")]
    assert names == ["A - T (2).jksong", "A - T (3).jksong"]
    assert (root / "Library" / "A - T.jksong").read_bytes() == b"someone else's song"


def test_bad_file_in_the_middle_of_the_queue(studio, root, tmp_path):
    studio.hello()
    good = MF.make("flac24", tmp_path, seconds=3.0)
    bad = tmp_path / "broken.mp3"
    bad.write_bytes(b"ID3" + os.urandom(5000))
    missing = tmp_path / "gone.wav"
    for jid, f in (("g1", good), ("bad", bad), ("gone", missing), ("g2", good)):
        submit(studio, jid, f, root / "Library", metadata={"title": jid, "artist": "Q"})
    assert studio.job_end("g1")["type"] == "job.done"
    fb = studio.job_end("bad")
    assert fb["type"] == "job.failed" and fb["error"]["code"] == "decode_failed"
    assert fb["error"]["message"] and fb["error"]["detail"]
    fg = studio.job_end("gone")
    assert fg["type"] == "job.failed" and fg["error"]["code"] == "not_found"
    assert studio.job_end("g2")["type"] == "job.done"
    states = {j["job_id"]: (j["state"], j["error_code"]) for j in studio.request({"type": "job.list"})["jobs"]}
    assert states == {"g1": ("done", None), "bad": ("failed", "decode_failed"),
                      "gone": ("failed", "not_found"), "g2": ("done", None)}


def test_cancel_running_and_queued_jobs(root, tmp_path):
    st = StudioProc(root, env={"JUKEOKE_STUDIO_STUB_DELAY": "0.5"})
    try:
        st.hello()
        src = MF.make("wav16", tmp_path, seconds=20.0)
        for jid in ("run", "wait", "after"):
            submit(st, jid, src, root / "Library", metadata={"title": jid, "artist": "C"})
        st.wait_for(lambda m: m.get("job_id") == "run" and m.get("stage") == "separate"
                    and m.get("stage_progress", 0) > 0.05, 60)
        r = st.request({"type": "job.cancel", "job_id": "wait"})
        assert r == {"type": "job.cancel.result", "id": r["id"], "ok": True}
        fw = st.job_end("wait", 10)
        assert fw["error"]["code"] == "cancelled"
        r = st.request({"type": "job.cancel", "job_id": "run"})
        assert r["ok"] is True
        fr = st.job_end("run", 30)
        assert fr["type"] == "job.failed" and fr["error"] == {"code": "cancelled", "message": "Cancelled.",
                                                                "detail": ""}
        # Cancelled within a chunk or two, not at the end of the stage.
        assert not any(e.get("stage") == "lyrics" for e in st.events("run"))
        # The queue goes on with the next job.
        done = st.job_end("after", 120)
        assert done["type"] == "job.done"
        assert not any(e["type"] == "job.started" for e in st.events("wait"))
        assert not list((root / "Library").glob("*.tmp-*"))
        assert not list((root / "Settings" / "tmp" / "studio").glob("job-*"))
        names = sorted(p.name for p in (root / "Library").iterdir())
        assert names == ["C - after.jksong"]
    finally:
        st.close()


def test_shutdown_cancels_the_running_job_and_cleans_up(root, tmp_path):
    st = StudioProc(root, env={"JUKEOKE_STUDIO_STUB_DELAY": "0.5"})
    try:
        st.hello()
        src = MF.make("wav16", tmp_path, seconds=30.0)
        submit(st, "long", src, root / "Library")
        st.wait_for(lambda m: m.get("job_id") == "long" and m.get("stage") == "separate", 60)
        r = st.request({"type": "shutdown"}, timeout=30)
        assert r["type"] == "shutdown.ok"
        assert st.p.wait(30) == 0
        assert any(m.get("job_id") == "long" and m.get("type") == "job.failed" for m in st.msgs)
        assert not list((root / "Library").iterdir())
        assert not list((root / "Settings" / "tmp" / "studio").glob("job-*"))
    finally:
        st.close()


@pytest.mark.parametrize("kind,rate,channels", [("wav16", 48000, 2), ("wav16", 44100, 1),
                                                ("flac24", 44100, 6)])
def test_rates_and_channel_layouts(studio, root, tmp_path, kind, rate, channels):
    studio.hello()
    src = MF.make(kind, tmp_path, seconds=3.0, sr=rate, channels=channels)
    submit(studio, "x", src, root / "Library", metadata={"title": f"{rate}-{channels}", "artist": "R"})
    done = studio.job_end("x")
    assert done["type"] == "job.done", done
    m = jksong.open_song(done["output"]).manifest
    assert m["sample_rate"] == rate  # never resampled
    assert m["channels"] == (2 if channels > 2 else channels)
    if channels > 2:
        assert m["provenance"]["source"]["downmix"]["from_channels"] == 6
        assert any("folded down to stereo" in w for w in done["report"]["warnings"])
    verify_strict(Path(done["output"]))


def test_lyrics_sources_through_the_pipeline(studio, root, tmp_path):
    studio.hello()
    # Embedded, synced (ID3 SYLT) lyrics + tags + cover, auto mode.
    src = MF.mp3_with_lyrics(tmp_path, seconds=4.0)
    submit(studio, "emb", src, root / "Library", lyrics={"mode": "auto"})
    d = studio.job_end("emb")
    assert d["type"] == "job.done", d
    song = jksong.open_song(d["output"])
    assert song.manifest["title"] == "Test Title" and song.manifest["artist"] == "Test Artist"
    assert song.manifest["cover"] == "cover" and song.read_chunk("cover")[:3] == b"\xff\xd8\xff"
    assert song.lyrics()["source"] == "embedded" and d["report"]["lyrics_source"] == "embedded"
    # Nothing given and nothing embedded: transcription (unchecked).
    plain = MF.make("wav16", tmp_path, seconds=12.0, name="plain.wav")
    submit(studio, "tr", plain, root / "Library", metadata={"title": "Heard", "artist": "W"},
           lyrics={"mode": "auto"})
    d = studio.job_end("tr")
    lyr = jksong.open_song(d["output"]).lyrics()
    assert lyr["source"] == "transcribed" and lyr["checked"] is False and d["report"]["lyrics_checked"] is False
    # Explicit text mode with only headers fails early, before separation.
    submit(studio, "empty", plain, root / "Library", lyrics={"mode": "text", "text": "[Chorus]\n\n"})
    f = studio.job_end("empty")
    assert f["error"]["code"] == "lyrics_missing"
    assert not any(e.get("stage") == "separate" for e in studio.events("empty"))
    # A bad source label is refused at submit.
    r = submit(studio, "bad", plain, root / "Library", lyrics={"mode": "text", "text": "x", "source": "nope"})
    assert r["type"] == "error" and r["error"]["code"] == "bad_request"


def test_realign_keeps_feasible_anchors(studio, root, tmp_path):
    studio.hello()
    src = MF.make("wav16", tmp_path, seconds=10.0)
    submit(studio, "s", src, root / "Library", metadata={"title": "Realign", "artist": "Me"},
           lyrics={"mode": "text", "text": "one two\nthree four\nfive six"})
    out = studio.job_end("s")["output"]
    r = studio.request({"type": "realign", "song": out,
                        "lyrics": {"mode": "text", "text": "one two\nthree four\nfive six"},
                        "anchors": [{"line": 1, "start": 4.0}, {"line": 2, "start": 9.98}]})
    assert r["type"] == "realign.result" and r["ok"] is True, r
    starts = [ln["start"] for ln in r["lyrics"]["lines"]]
    assert starts[1] == 4.0                       # the feasible anchor holds
    assert r["dropped_anchors"] == [2]            # only the impossible one is dropped
    assert r["lyrics"]["checked"] is True and r["lyrics"]["source"] == "user_text"
    assert jksong.validate_lyrics(r["lyrics"], 10.0) == []
    r = studio.request({"type": "realign", "song": out, "lyrics": {"mode": "text", "text": "  "}})
    assert r["error"]["code"] == "lyrics_missing"
    r = studio.request({"type": "realign", "song": str(tmp_path / "nope.jksong"),
                        "lyrics": {"mode": "text", "text": "x"}})
    assert r["error"]["code"] == "not_found"


def test_requests_are_answered_while_a_job_runs(root, tmp_path):
    st = StudioProc(root, env={"JUKEOKE_STUDIO_STUB_DELAY": "0.3"})
    try:
        st.hello()
        src = MF.make("wav16", tmp_path, seconds=20.0)
        submit(st, "busy", src, root / "Library")
        st.wait_for(lambda m: m.get("job_id") == "busy" and m.get("stage") == "separate", 60)
        import time

        t0 = time.monotonic()
        assert st.request({"type": "job.list"}, timeout=5)["jobs"][0]["state"] == "running"
        assert st.request({"type": "inspect", "input": str(src)}, timeout=20)["sample_rate"] == 44100
        assert st.request({"type": "models.status", "preset": "full"}, timeout=5)["all_present"] is True
        assert time.monotonic() - t0 < 15
        assert not any(m.get("type") in ("job.done", "job.failed") for m in st.msgs)
        st.request({"type": "job.cancel", "job_id": "busy"})
        st.job_end("busy", 30)
    finally:
        st.close()


# ------------------------------------------------------------------------- inspect

def test_inspect_audio_video_and_embedded_lyrics(studio, root, tmp_path):
    studio.hello()
    a = studio.request({"type": "inspect", "input": str(MF.make("flac24", tmp_path, seconds=2.0))})
    assert (a["container"], a["codec"], a["sample_rate"], a["channels"], a["bits_per_sample"]) == \
        ("flac", "flac", 44100, 2, 24)
    assert a["lossy"] is False and a["has_video"] is False and a["duration_seconds"] == pytest.approx(2.0)
    v = studio.request({"type": "inspect", "input": str(MF.make("mp4", tmp_path, seconds=2.0))})
    assert v["has_video"] is True and v["codec"] == "aac" and v["lossy"] is True and v["container"] == "mp4"
    w = studio.request({"type": "inspect", "input": str(MF.make("webm", tmp_path, seconds=2.0))})
    assert w["has_video"] is True and w["codec"] == "opus" and w["sample_rate"] == 48000
    e = studio.request({"type": "inspect", "input": str(MF.mp3_with_lyrics(tmp_path))})
    assert e["title"] == "Test Title" and e["artist"] == "Test Artist" and e["codec"] == "mp3"
    assert e["embedded_lyrics"]["present"] is True and e["embedded_lyrics"]["synced"] is True
    assert e["embedded_lyrics"]["text"].startswith("[00:00.10]First line here")
    assert e["has_video"] is False  # the cover picture is not a video
    cover = Path(e["cover_path"])
    assert cover.is_file() and cover.read_bytes()[:3] == b"\xff\xd8\xff"
    assert cover.parent == (root / "Settings" / "tmp" / "studio" / "covers").resolve()
    f = studio.request({"type": "inspect", "input": str(MF.flac_with_tags(tmp_path, cover=MF.png_bytes(),
                                                                         TITLE="Tagged", ARTIST="Band"))})
    assert f["embedded_lyrics"] == {"present": True, "synced": False, "text": "Line one\nLine two"}
    assert Path(f["cover_path"]).read_bytes()[:4] == b"\x89PNG"
    bad = tmp_path / "x.mp3"
    bad.write_bytes(b"nothing")
    assert studio.request({"type": "inspect", "input": str(bad)})["error"]["code"] == "decode_failed"
    assert studio.request({"type": "inspect", "input": str(tmp_path / "no.mp3")})["error"]["code"] == "not_found"


# -------------------------------------------------------------------- models (real)

def test_models_missing_are_reported_and_never_downloaded(root, tmp_path):
    """Without the stand-ins and with an empty models folder: models.status says what is
    missing, and a job fails with models_missing (no silent download)."""
    st = StudioProc(root, stub=False, fake_hw="ok", models_dir=root / "Models")
    try:
        st.hello()
        r = st.request({"type": "models.status", "preset": "full"})
        assert r["all_present"] is False and r["download_bytes"] > 900_000_000
        names = {m["name"] for m in r["models"]}
        assert {"bs_roformer_anvuew_ft1", "bs_karaoke_anvuew", "bs_roformer_4stems_syh",
                "facebook/wav2vec2-large-960h-lv60-self"} <= names
        p = st.request({"type": "models.status", "preset": "permissive"})
        assert {m["name"] for m in p["models"] if m["task"] == "separation"} == {"mel_roformer_kim",
                                                                                  "bs_roformer_4stems_syh"}
        assert all(m["commercial"] == "yes" for m in p["models"])
        src = MF.make("wav16", tmp_path, seconds=2.0)
        submit(st, "m", src, root / "Library")
        f = st.job_end("m", 120)
        assert f["type"] == "job.failed" and f["error"]["code"] == "models_missing"
        assert not (root / "Models").exists() or not any((root / "Models").rglob("*.ckpt"))
    finally:
        st.close()


# ---------------------------------------------------------------------- portability

def test_portable_environment_points_every_cache_into_the_stick(tmp_path):
    root = tmp_path / "E"
    code = ("import json, os, sys, tempfile\n"
            "from jukeoke_studio import env\n"
            f"e = env.setup({str(root)!r})\n"
            "print(json.dumps({'vars': {k: os.environ.get(k) for k in list(env.CACHE_VARS) + "
            "list(env.TEMP_VARS) + ['HF_HUB_CACHE', 'TRANSFORMERS_CACHE', 'HF_HUB_OFFLINE']},"
            " 'tmp': tempfile.gettempdir(), 'nobyte': sys.dont_write_bytecode}))\n")
    r = subprocess.run([sys.executable, "-I", "-c", code], capture_output=True, text=True, check=True,
                       env={k: v for k, v in os.environ.items() if not k.startswith("JUKEOKE_")})
    d = json.loads(r.stdout)
    rr = root.resolve()
    for k in ("HF_HOME", "TORCH_HOME", "XDG_CACHE_HOME", "CUDA_CACHE_PATH", "NUMBA_CACHE_DIR", "MPLCONFIGDIR"):
        assert Path(d["vars"][k]).is_relative_to(rr / "Settings" / "cache"), (k, d["vars"][k])
    for k in ("TMP", "TEMP", "TMPDIR"):
        assert Path(d["vars"][k]) == rr / "Settings" / "tmp" / "studio"
    assert Path(d["tmp"]) == rr / "Settings" / "tmp" / "studio"
    assert Path(d["vars"]["HF_HUB_CACHE"]) == rr / "Models" / "hf"
    assert d["vars"]["HF_HUB_OFFLINE"] == "1" and d["nobyte"] is True


def test_safe_file_names():
    from jukeoke_studio.pipeline import safe_file_stem, song_stem

    assert song_stem("AC/DC", "Back: In <Black>?") == "AC_DC - Back_ In _Black__"
    assert safe_file_stem("CON") == "_CON" and safe_file_stem("lpt1.x") == "_lpt1.x"
    assert safe_file_stem("...hidden. ") == "hidden"
    assert safe_file_stem("a\tb\x00c") == "a_b_c"
    assert len(safe_file_stem("x" * 400)) == 150
    assert safe_file_stem("") == "Song"
    assert song_stem("", "Solo") == "Solo"
    assert safe_file_stem("Café") == "Café"  # NFC

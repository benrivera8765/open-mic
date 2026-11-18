# SPDX-License-Identifier: GPL-3.0-or-later
"""Command line for the Studio engine.

    python -I -m jukeoke_studio serve [--root R] [--models-dir M] [--temp-dir T]
    python -I -m jukeoke_studio make SONG --out DIR [--lyrics-text F | --lrc F] [--preset P]
                                     [--title T] [--artist A] [--language xx] [--no-lyrics]
    python -I -m jukeoke_studio probe
    python -I -m jukeoke_studio models list
    python -I -m jukeoke_studio models fetch [ID ...] [--verify] [--preset P]
    python -I -m jukeoke_studio separate SONG OUTDIR [--preset full|clean|permissive]

``serve`` speaks docs/STUDIO_PROTOCOL.md on stdin/stdout. ``make`` runs one song through
the same pipeline and prints the job report as JSON. ``models fetch`` is the developer
way to download weights (the app uses the protocol's ``models.download``).
``separate`` writes 32-bit float WAV stems for listening and debugging.

Order of start-up matters: the options are parsed with the standard library only, then
``env.setup`` points every cache into ``<root>`` (portable mode) and, for ``serve`` and
``make``, stdout is claimed for the protocol / report, all before numpy, torch,
transformers, huggingface_hub, numba, matplotlib or ctranslate2 are imported.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True  # portable mode: no .pyc next to the runtime on the stick

import argparse  # noqa: E402
import json  # noqa: E402
import logging  # noqa: E402
import time  # noqa: E402
from pathlib import Path  # noqa: E402


def _common(p: argparse.ArgumentParser) -> None:
    p.add_argument("--root", default=argparse.SUPPRESS,
                   help="portable root (the stick): caches go to <root>/Settings/cache, "
                        "temporary files to <root>/Settings/tmp/studio, weights to <root>/Models")
    p.add_argument("--models-dir", default=argparse.SUPPRESS, help="model weights folder")
    p.add_argument("--temp-dir", default=argparse.SUPPRESS, help="temporary folder")


def _parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    _common(common)
    ap = argparse.ArgumentParser(prog="jukeoke_studio", description=__doc__, parents=[common],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("serve", parents=[common], help="speak the Studio protocol on stdin/stdout")
    sub.add_parser("probe", parents=[common], help="print what this computer can do (JSON)")
    mk = sub.add_parser("make", parents=[common], help="make one .jksong from a song file")
    mk.add_argument("input")
    mk.add_argument("--out", required=True, help="output folder")
    mk.add_argument("--lyrics-text", help="plain-text lyrics file (UTF-8)")
    mk.add_argument("--lrc", help=".lrc lyrics file (UTF-8)")
    mk.add_argument("--lyrics-source", default=None, help="label for the given lyrics, e.g. online:lrclib")
    mk.add_argument("--no-lyrics", action="store_true", help="instrumental: no lyrics")
    mk.add_argument("--preset", default="full")
    mk.add_argument("--title")
    mk.add_argument("--artist")
    mk.add_argument("--album")
    mk.add_argument("--language", default="en")
    mk.add_argument("--device", default=None)
    m = sub.add_parser("models", parents=[common], help="list or download models")
    m.add_argument("action", choices=["list", "fetch"])
    m.add_argument("ids", nargs="*")
    m.add_argument("--verify", action="store_true", help="re-hash files already present")
    m.add_argument("--preset", default=None, help="fetch what this preset needs (all presets if omitted)")
    s = sub.add_parser("separate", parents=[common], help="split a song into float32 WAV stems")
    s.add_argument("input")
    s.add_argument("outdir")
    s.add_argument("--preset", default="full", choices=["full", "clean", "permissive"])
    s.add_argument("--device", default=None)
    s.add_argument("--precision", default="fp32")
    s.add_argument("--overlap", type=int, default=None)
    return ap


def _logging(verbose: bool = True) -> None:
    logging.basicConfig(level=logging.INFO if verbose else logging.WARNING, stream=sys.stderr,
                        format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    logging.captureWarnings(True)


# ------------------------------------------------------------------------- commands

def _serve(args, env) -> int:
    from . import env as studio_env

    out = studio_env.claim_stdout()
    inp = studio_env.claim_stdin()
    _logging()
    from .server import main as serve_main

    return serve_main(out, env.models_dir, inp)


def _probe(args, env) -> int:
    from . import env as studio_env

    out = studio_env.claim_stdout()
    _logging(False)
    import os

    from .probe import probe

    out.write_line(json.dumps(probe(fake=os.environ.get("JUKEOKE_STUDIO_FAKE_HW")), indent=1))
    return 0


def _make(args, env) -> int:
    from . import env as studio_env

    out = studio_env.claim_stdout()
    _logging()
    from .errors import Hooks
    from .pipeline import JobContext, map_exception, parse_job, run_job

    lyrics: dict = {"mode": "auto"}
    if args.no_lyrics:
        lyrics = {"mode": "none"}
    elif args.lrc:
        lyrics = {"mode": "lrc", "lrc": Path(args.lrc).read_text(encoding="utf-8-sig")}
    elif args.lyrics_text:
        lyrics = {"mode": "text", "text": Path(args.lyrics_text).read_text(encoding="utf-8-sig")}
    if args.lyrics_source:
        lyrics["source"] = args.lyrics_source
    meta = {k: v for k, v in (("title", args.title), ("artist", args.artist), ("album", args.album)) if v}
    spec = parse_job({"job_id": f"make-{int(time.time())}", "input": str(Path(args.input).resolve()),
                      "output_dir": str(Path(args.out).resolve()), "preset": args.preset,
                      "language": args.language, "metadata": meta, "lyrics": lyrics})
    ctx = JobContext(models_dir=env.models_dir, temp_dir=studio_env.temp_dir(), app_version=None,
                     device=args.device)
    last = [None]

    def progress(stage, sp, overall, message):
        if (stage, round(sp, 1)) != last[0]:
            last[0] = (stage, round(sp, 1))
            print(f"[{overall * 100:5.1f} %] {stage:8s} {sp * 100:5.1f} %  {message or ''}",
                  file=sys.stderr, flush=True)

    try:
        res = run_job(spec, ctx, Hooks(), progress)
    except BaseException as e:  # noqa: BLE001
        err = map_exception(e)
        out.write_line(json.dumps({"ok": False, "error": err.to_json()}, ensure_ascii=False))
        logging.getLogger("jukeoke_studio").error("make failed: %s (%s)", err.message, err.detail)
        return 2 if err.code != "cancelled" else 130
    out.write_line(json.dumps({"ok": True, "output": str(res.output), "report": res.report},
                              ensure_ascii=False, indent=1))
    return 0


def _models(args, env) -> int:
    from .modelzoo import MODELS, hfmodels
    from .modelzoo.download import default_models_dir, ensure_model
    from .presets import PRESETS

    d = env.models_dir or default_models_dir()
    if args.action == "list":
        for m in MODELS.values():
            print(f"{m.id:32s} {m.role:12s} {m.commercial:8s} {m.licence}")
        for m in hfmodels.HF_MODELS.values():
            print(f"{m.key:32s} {m.task:12s} {m.commercial:8s} {m.licence}  ({m.repo})")
        return 0
    presets = [args.preset] if args.preset else list(PRESETS)
    ids = args.ids or sorted({mid for p in presets for mid in PRESETS[p].values()})

    def prog(f, a, b):
        print(f"\r{f}: {a / 1e6:.0f}/{b / 1e6:.0f} MB", end="", file=sys.stderr)

    for mid in ids:
        t = time.perf_counter()
        if mid in hfmodels.HF_MODELS:
            p = hfmodels.ensure(hfmodels.get(mid), d, allow_download=True, progress=prog)
            print(f"\r{mid}: ok ({time.perf_counter() - t:.1f} s) {p}")
            continue
        paths = ensure_model(MODELS[mid], d, verify=args.verify, allow_download=True, progress=prog)
        print(f"\r{mid}: ok ({time.perf_counter() - t:.1f} s) {[p.name for p in paths]}")
    if not args.ids:
        for key in (hfmodels.DEFAULT_ALIGNMENT, hfmodels.DEFAULT_TRANSCRIPTION):
            p = hfmodels.ensure(hfmodels.get(key), d, allow_download=True, progress=prog)
            print(f"\r{key}: ok {p}")
    return 0


def _separate(args, env) -> int:
    import numpy as np
    import soundfile as sf

    from .separate import Separator

    audio, sr = sf.read(args.input, dtype="float32", always_2d=True)
    sep = Separator(env.models_dir, args.device, precision=args.precision, overlap=args.overlap)
    t = time.perf_counter()
    res = sep.separate(np.ascontiguousarray(audio.T), sr, args.preset)
    wall = time.perf_counter() - t
    out = Path(args.outdir)
    out.mkdir(parents=True, exist_ok=True)
    for name, x in {**res.stems, **res.extras}.items():
        sf.write(str(out / f"{name}.wav"), x.T, sr, subtype="FLOAT")
    info = {"wall_seconds": round(wall, 1), "runs": [r.__dict__ for r in res.runs],
            **res.provenance()}
    (out / "separation.json").write_text(json.dumps(info, indent=1), encoding="utf-8")
    print(json.dumps(info, indent=1))
    return 0


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    from . import env as studio_env

    env = studio_env.setup(getattr(args, "root", None), getattr(args, "models_dir", None),
                           getattr(args, "temp_dir", None))
    return {"serve": _serve, "probe": _probe, "make": _make, "models": _models,
            "separate": _separate}[args.cmd](args, env)


if __name__ == "__main__":
    code = main()
    try:
        sys.stderr.flush()
    except Exception:  # noqa: BLE001
        pass
    # Skip interpreter teardown: CUDA / CTranslate2 threads can make it slow or hang,
    # and every file has been written and closed by now.
    import os

    os._exit(code)

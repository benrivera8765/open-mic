# SPDX-License-Identifier: GPL-3.0-or-later
"""Command line for the Studio engine.

    python -I -m jukeoke_studio models list
    python -I -m jukeoke_studio models fetch [ID ...] [--verify]
    python -I -m jukeoke_studio separate SONG OUTDIR [--preset full|clean]

``separate`` writes 32-bit float WAV stems (for listening and debugging; the real
pipeline writes a .jksong). ``serve`` and ``make`` (docs/STUDIO_PROTOCOL.md) are added
by the Studio pipeline stage.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path


def _models(args) -> int:
    from .modelzoo import MODELS
    from .modelzoo.download import default_models_dir, ensure_model

    if args.action == "list":
        for m in MODELS.values():
            print(f"{m.id:32s} {m.role:12s} {m.commercial:8s} {m.licence}")
        return 0
    from .separate import PRESETS

    ids = args.ids or sorted({mid for preset in PRESETS.values() for mid in preset.values()})
    d = Path(args.models_dir) if args.models_dir else default_models_dir()
    for mid in ids:
        t = time.perf_counter()
        paths = ensure_model(MODELS[mid], d, verify=args.verify,
                             progress=lambda f, a, b: print(f"\r{f}: {a / 1e6:.0f}/{b / 1e6:.0f} MB", end=""))
        print(f"\r{mid}: ok ({time.perf_counter() - t:.1f} s) {[p.name for p in paths]}")
    return 0


def _separate(args) -> int:
    import numpy as np
    import soundfile as sf

    from .separate import Separator

    audio, sr = sf.read(args.input, dtype="float32", always_2d=True)
    sep = Separator(args.models_dir, args.device, precision=args.precision, overlap=args.overlap)
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
    ap = argparse.ArgumentParser(prog="jukeoke_studio", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--models-dir", default=None)
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("models", help="list or download separation models")
    m.add_argument("action", choices=["list", "fetch"])
    m.add_argument("ids", nargs="*")
    m.add_argument("--verify", action="store_true", help="re-hash files already present")
    s = sub.add_parser("separate", help="split a song into float32 WAV stems")
    s.add_argument("input")
    s.add_argument("outdir")
    s.add_argument("--preset", default="full", choices=["full", "clean"])
    s.add_argument("--device", default=None)
    s.add_argument("--precision", default="fp32")
    s.add_argument("--overlap", type=int, default=None)
    args = ap.parse_args(argv)
    return _models(args) if args.cmd == "models" else _separate(args)


if __name__ == "__main__":
    sys.exit(main())

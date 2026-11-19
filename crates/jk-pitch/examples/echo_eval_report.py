# SPDX-License-Identifier: GPL-3.0-or-later
"""Summarise the echo benchmark (examples/echo_eval.rs) as the Markdown tables of
docs/benchmarks/practice-echo.md.

    python -I echo_eval_report.py <results.jsonl> [cpu.jsonl]

Reads only the numbers the benchmark wrote; standard library only.
"""

import json
import sys


def load(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def f(v, nd=1, unit=""):
    if v is None:
        return "-"
    return f"{v:.{nd}f}{unit}"


def pct(v):
    return "-" if v is None else f"{100 * v:.1f} %"


def delay_err(d):
    if d["delay_ms"] is None:
        return None
    truth = d["true_delay_mean_ms"] if d["kind"] == "bluetooth" else d["true_delay_end_ms"]
    return d["delay_ms"] - truth


def table(rows, cols):
    out = ["| " + " | ".join(c[0] for c in cols) + " |", "|" + "---|" * len(cols)]
    for r in rows:
        out.append("| " + " | ".join(c[1](r) for c in cols) + " |")
    return "\n".join(out)


def main():
    rows = load(sys.argv[1])
    main_track = rows[0]["track"]
    m = [r for r in rows if r["track"] == main_track]
    print(f"## Main track {main_track}: {len(m)} scenarios; all tracks: {len(rows)}\n")

    det_cols = [
        ("scenario", lambda r: r["name"]),
        ("found after (s)", lambda r: f(r["first_present_s"], 2)),
        ("level dB (truth)", lambda r: f"{f(r['level_db'])} ({f(r['true_level_db'])})"),
        ("delay error ms", lambda r: f(delay_err(r), 3)),
        ("drift ppm", lambda r: f(r["drift_ppm"], 1)),
        ("stability", lambda r: r["stability"]),
        ("suppression dB est / true", lambda r: f"{f(r['est_suppression_db'])} / {f(r['true_suppression_db'])}"),
        ("residual dBFS", lambda r: f(r["residual_dbfs"])),
        ("quality", lambda r: r["quality"]),
        ("relocks", lambda r: str(r["relocks"])),
    ]
    trk_ns = [
        ("scenario", lambda r: r["name"]),
        ("pitch frames, no removal", lambda r: pct(r["pitch_raw"])),
        ("after removal", lambda r: pct(r["pitch_out"])),
        ("after removal and trusted", lambda r: pct(r["pitch_out_trusted"])),
        ("same, from 15 s", lambda r: pct(r["pitch_out_steady_trusted"])),
    ]
    trk_s = [
        ("scenario", lambda r: r["name"]),
        ("right, clean voice", lambda r: pct(r["acc_clean"])),
        ("no removal", lambda r: pct(r["acc_raw"])),
        ("after removal", lambda r: pct(r["acc_out"])),
        ("removal + trusted", lambda r: pct(r["acc_out_trusted"])),
        ("same, from 15 s", lambda r: pct(r["acc_out_steady_trusted"])),
        ("rests shown as pitch", lambda r: pct(r["rest_pitch_out_trusted"])),
    ]
    for title, sel in [
        ("USB loopback, no singer", lambda r: r["kind"] in ("usb", "usb_eq") and not r["singer"] and not r["guide"] and not r["gap"]),
        ("USB loopback, with singer", lambda r: r["kind"] in ("usb", "usb_eq") and r["singer"] and not r["guide"] and not r["gap"]),
        ("Cable, no singer", lambda r: r["kind"] == "cable" and not r["singer"] and not r["guide"] and not r["gap"]),
        ("Cable, with singer", lambda r: r["kind"] == "cable" and r["singer"] and not r["guide"] and not r["gap"]),
        ("Bluetooth", lambda r: r["kind"] == "bluetooth" and not r["guide"]),
        ("Guide vocal in the playback", lambda r: r["guide"] and r["kind"] != "none"),
        ("Nothing returned", lambda r: r["kind"] == "none"),
        ("Break in the music", lambda r: r["gap"]),
    ]:
        sub = [r for r in m if sel(r)]
        if not sub:
            continue
        print(f"### {title}\n")
        print(table(sub, det_cols))
        print()
        ns = [r for r in sub if not r["singer"]]
        if ns:
            print(table(ns, trk_ns))
            print()
        ss = [r for r in sub if r["singer"]]
        if ss:
            print(table(ss, trk_s))
            print()
    others = [r for r in rows if r["track"] != main_track]
    if others:
        print("### Other tracks\n")
        print(table(others, [("track", lambda r: r["track"])] + det_cols))
        print()
        print(table([r for r in others if not r["singer"]], [("track", lambda r: r["track"])] + trk_ns))
        print()
        print(table([r for r in others if r["singer"]], [("track", lambda r: r["track"])] + trk_s))
        print()
    # Aggregates.
    present = [r for r in rows if r["kind"] != "none"]
    det = [r["first_present_s"] for r in present if r["first_present_s"] is not None]
    missed = [r["name"] + "@" + r["track"] for r in present if r["first_present_s"] is None]
    print("### Aggregates\n")
    print(f"- returns found: {len(det)} of {len(present)}; slowest {max(det):.2f} s; median {sorted(det)[len(det)//2]:.2f} s; not found: {missed}")
    fixed = [r for r in present if r["kind"] != "bluetooth" and r["delay_ms"] is not None]
    errs = sorted(abs(delay_err(r)) for r in fixed)
    print(f"- delay error (USB and cable, {len(errs)} runs): median {errs[len(errs)//2]:.4f} ms, worst {errs[-1]:.4f} ms")
    lv = sorted(abs(r["level_db"] - r["true_level_db"]) for r in present if r["level_db"] is not None)
    print(f"- level error: median {lv[len(lv)//2]:.2f} dB, worst {lv[-1]:.2f} dB")
    ns = [r for r in present if not r["singer"]]
    worst = max(ns, key=lambda r: r["pitch_out_trusted"])
    print(f"- no singer, music returned: pitch frames after removal and trusted: worst {pct(worst['pitch_out_trusted'])} ({worst['name']} @ {worst['track']}); "
          f"without removal up to {pct(max(r['pitch_raw'] for r in ns))}")
    nsb = [r for r in ns if r["kind"] != "bluetooth"]
    worst = max(nsb, key=lambda r: r["pitch_out_trusted"])
    print(f"- same without Bluetooth: worst {pct(worst['pitch_out_trusted'])} ({worst['name']} @ {worst['track']})")
    bt = [r for r in present if r["kind"] == "bluetooth"]
    print(f"- Bluetooth runs judged unusable: {sum(r['quality'] == 'unusable' for r in bt)} of {len(bt)}; wandering: {sum(r['stability'] == 'wandering' for r in bt)}")
    nonbt = [r for r in present if r["kind"] != "bluetooth"]
    print(f"- USB/cable runs judged wandering: {[r['name'] + '@' + r['track'] for r in nonbt if r['stability'] == 'wandering']}")
    if len(sys.argv) > 2:
        print("\n### CPU\n")
        for c in load(sys.argv[2]):
            print(f"- {c['cpu_rate']} Hz: {c['ms_per_s']:.2f} ms per second of audio ({c['percent_of_one_core']:.2f} % of one core)")


if __name__ == "__main__":
    main()

# SPDX-License-Identifier: GPL-3.0-or-later
"""Tables for docs/benchmarks/practice-echo.md section 10 from echo_cases.rs output.

    python -I crates/jk-pitch/examples/echo_cases_report.py cases.jsonl [before.jsonl]

With a second file, each figure is shown as "before -> after". Numbers only.
"""

import json
import sys


def load(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def fmt(v, unit=""):
    if v is None:
        return "-"
    if isinstance(v, float):
        return f"{v:.1f}{unit}" if abs(v) >= 0.1 or v == 0 else f"{v:.2f}{unit}"
    return f"{v}{unit}"


def main():
    rows = load(sys.argv[1])
    before = {}
    if len(sys.argv) > 2:
        for r in load(sys.argv[2]):
            before[(r["song"], r["name"])] = r

    def cell(r, key, unit=""):
        now = fmt(r.get(key), unit)
        b = before.get((r["song"], r["name"]))
        if b is None:
            return now
        old = fmt(b.get(key), unit)
        return now if old == now else f"{old} -> {now}"

    groups = {
        "off": ["cleared_after_s", "acc_out_after_switch", "acc_gated_after_switch", "shown_after_switch", "quality"],
        "on": ["found_after_s", "acc_gated_steady", "shown_whole", "quality", "lost"],
        "high": ["acc_out_steady", "acc_gated_steady", "acc_gated_whole", "rest_shown_steady", "suppression_db", "true_suppression_db", "quality", "lost"],
        "continuous": ["first_verdict_s", "acc_out_steady", "acc_gated_steady", "acc_gated_whole", "suppression_db", "true_suppression_db", "quality", "lost"],
        "path": ["presence", "lost", "relocks", "shown_after_switch", "acc_gated_after_switch", "acc_gated_steady", "true_suppression_db", "quality"],
        "control": ["shown_whole", "quality", "presence", "lost"],
    }
    for g, keys in groups.items():
        sel = [r for r in rows if r["group"] == g]
        if not sel:
            continue
        print(f"\n#### {g}\n")
        print("| song | case | " + " | ".join(keys) + " |")
        print("|---" * (len(keys) + 2) + "|")
        for r in sel:
            print(f"| {r['song']} | {r['name']} | " + " | ".join(cell(r, k) for k in keys) + " |")


if __name__ == "__main__":
    main()

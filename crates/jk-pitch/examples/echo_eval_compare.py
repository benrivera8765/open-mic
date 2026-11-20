# SPDX-License-Identifier: GPL-3.0-or-later
"""A/B summary of two echo_eval.rs result files (same scenarios), for
docs/benchmarks/practice-echo.md: "before" and "after" a change of the canceller.

    python -I crates/jk-pitch/examples/echo_eval_compare.py before.jsonl after.jsonl

Prints one table of group summaries and the scenarios whose verdicts changed. Numbers only.
"""

import json
import statistics as st
import sys


def load(path):
    with open(path, encoding="utf-8") as f:
        return {(r["track"], r["name"]): r for r in map(json.loads, f) if r}


def groups(rows):
    def kind(r):
        k = r["kind"]
        if k in ("usb", "usb_eq"):
            k = "usb"
        return k

    out = {}
    for key, r in rows.items():
        g = (kind(r), "singer" if r["singer"] else "no singer")
        out.setdefault(g, []).append(key)
    return out


def num(v):
    return v if isinstance(v, (int, float)) and v is not None else None


def summary(rows, keys):
    rs = [rows[k] for k in keys]
    s = {}
    returned = [r for r in rs if r["kind"] != "none"]
    found = [r["first_present_s"] for r in returned if r["first_present_s"] is not None]
    s["found"] = f"{len(found)}/{len(returned)}" if returned else "-"
    s["found median/max s"] = f"{st.median(found):.2f}/{max(found):.2f}" if found else "-"
    false = [r for r in rs if r["kind"] == "none" and r["presence"] == "present"]
    s["false present"] = str(len(false)) if any(r["kind"] == "none" for r in rs) else "-"
    derr = [abs(r["delay_ms"] - r["true_delay_end_ms"]) for r in returned
            if num(r["delay_ms"]) is not None and r["kind"] != "bluetooth"]
    s["delay err max ms"] = f"{max(derr):.3f}" if derr else "-"
    sup = [r["true_suppression_db"] for r in returned if num(r["true_suppression_db"]) is not None]
    s["true sup min/median dB"] = f"{min(sup):.1f}/{st.median(sup):.1f}" if sup else "-"
    q = {}
    for r in rs:
        q[r["quality"]] = q.get(r["quality"], 0) + 1
    s["quality"] = " ".join(f"{k}:{v}" for k, v in sorted(q.items()))
    if not rs[0]["singer"]:
        s["no-singer pitch shown max %"] = f"{100 * max(r['pitch_out_trusted'] for r in rs):.2f}"
    else:
        s["gated acc from 15 s min/median %"] = (
            f"{100 * min(r['acc_out_steady_trusted'] for r in rs):.1f}/"
            f"{100 * st.median(r['acc_out_steady_trusted'] for r in rs):.1f}")
        s["gated acc whole min/median %"] = (
            f"{100 * min(r['acc_out_trusted'] for r in rs):.1f}/"
            f"{100 * st.median(r['acc_out_trusted'] for r in rs):.1f}")
        s["rest pitch shown max %"] = f"{100 * max(r['rest_pitch_out_trusted'] for r in rs):.2f}"
    s["relocks total"] = str(sum(r["relocks"] for r in rs))
    s["stability"] = " ".join(
        f"{k}:{v}" for k, v in sorted(
            {x: sum(1 for r in rs if r["stability"] == x) for x in {r["stability"] for r in rs}}.items()))
    return s


def main():
    a = load(sys.argv[1])
    b = load(sys.argv[2])
    keys = sorted(set(a) & set(b))
    ga = groups({k: a[k] for k in keys})
    print("| group | figure | before | after |")
    print("|---|---|---|---|")
    for g in sorted(ga):
        sa = summary(a, ga[g])
        sb = summary(b, ga[g])
        for f in sa:
            mark = "" if sa[f] == sb[f] else " **"
            print(f"| {g[0]}, {g[1]} ({len(ga[g])}) | {f} | {sa[f]} | {sb[f]}{mark} |")
    print()
    print("Scenarios whose quality, presence, stability or relocks changed:")
    for k in keys:
        x, y = a[k], b[k]
        diffs = [f for f in ("quality", "presence", "stability", "relocks") if x[f] != y[f]]
        if diffs:
            print(f"- {k[0]} {k[1]}: " + ", ".join(f"{f} {x[f]} -> {y[f]}" for f in diffs))


if __name__ == "__main__":
    main()

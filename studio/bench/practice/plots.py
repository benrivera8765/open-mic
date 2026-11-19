# SPDX-License-Identifier: GPL-3.0-or-later
"""Pictures of a guide over the vocal's pitch curve (evidence only; kept outside the repo)."""

from __future__ import annotations

import numpy as np


def _read_curve(path):
    d = np.genfromtxt(path, delimiter=",", names=True)
    return d["t"], d["f0_hz"]


def guide_picture(png, title, curve_csv, guide, other=None, other_label=None, truth=None, row_s=30.0):
    """Rows of `row_s` seconds: pitch curve (dots), guide notes (bars; dim = low confidence,
    grey = unpitched), optional second guide (outlines) and ground truth (black ticks)."""
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Rectangle

    t, f0 = _read_curve(curve_csv)
    v = f0 > 0
    midi = np.full_like(f0, np.nan)
    midi[v] = 69 + 12 * np.log2(f0[v] / 440)
    dur = float(t[-1]) if len(t) else 1.0
    rows = max(1, int(np.ceil(dur / row_s)))
    fig, axes = plt.subplots(rows, 1, figsize=(16, 2.3 * rows), squeeze=False)
    pit = [n for n in guide["notes"] if n["kind"] == "pitched"]
    for r in range(rows):
        ax = axes[r, 0]
        a, b = r * row_s, (r + 1) * row_s
        sel = (t >= a) & (t < b)
        ax.plot(t[sel], midi[sel], ".", ms=1.2, color="#7a7a7a", zorder=1)
        inrow = [n for n in pit if n["end"] > a and n["start"] < b]
        lo_hi = [n["midi"] for n in inrow] + list(midi[sel][np.isfinite(midi[sel])])
        lo, hi = (min(lo_hi) - 3, max(lo_hi) + 3) if lo_hi else (48, 72)
        if truth:
            for n in truth["notes"]:
                if n["offset"] > a and n["onset"] < b:
                    ax.plot([n["onset"], n["offset"]], [n["midi"], n["midi"]], "-", color="black", lw=0.8, zorder=4)
        for n in inrow:
            alpha = 0.85 if n["conf"] >= 0.5 else 0.3
            col = "#d9822b" if n["octave"] not in ("maybe_high", "maybe_low") else "#c0392b"
            ax.add_patch(Rectangle((n["start"], n["midi"] - 0.4), n["end"] - n["start"], 0.8,
                                   color=col, alpha=alpha, lw=0, zorder=2))
        for n in guide["notes"]:
            if n["kind"] == "unpitched" and n["end"] > a and n["start"] < b:
                ax.axvspan(n["start"], n["end"], color="#5b8bd9", alpha=0.18, lw=0, zorder=0)
        if other:
            for n in other["notes"]:
                if n["kind"] == "pitched" and n["end"] > a and n["start"] < b:
                    ax.add_patch(Rectangle((n["start"], n["midi"] - 0.5), n["end"] - n["start"], 1.0,
                                           fill=False, ec="#2a9d8f", lw=0.7, zorder=3))
        ax.set_xlim(a, b)
        ax.set_ylim(lo, hi)
        ax.set_yticks(range(int(np.ceil(lo)), int(hi) + 1, 2 if hi - lo < 30 else 4))
        ax.grid(axis="y", lw=0.3, alpha=0.5)
        ax.tick_params(labelsize=7)
    leg = "dots: pitch curve; orange bars: guide notes (dim = low confidence, red = octave flag); blue: unpitched"
    if other:
        leg += f"; green outlines: {other_label}"
    if truth:
        leg += "; black lines: true notes"
    axes[0, 0].set_title(f"{title}\n{leg}", fontsize=8, loc="left")
    axes[-1, 0].set_xlabel("seconds; y = MIDI note (60 = C4)", fontsize=8)
    fig.tight_layout()
    fig.savefig(png, dpi=90)
    plt.close(fig)

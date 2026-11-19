# SPDX-License-Identifier: GPL-3.0-or-later
"""Note-level metrics for melody guides (mir_eval conventions)."""

from __future__ import annotations

import numpy as np

GRID = 0.01


def midi_to_hz(m):
    return 440.0 * 2.0 ** ((np.asarray(m, float) - 69.0) / 12.0)


def _arrays(notes, start="start", end="end"):
    if not notes:
        return np.zeros((0, 2)), np.zeros(0)
    iv = np.array([[n[start], max(n[end], n[start] + 1e-3)] for n in notes], float)
    return iv, iv[:, 0]


def pitched(guide):
    return [n for n in guide["notes"] if n["kind"] == "pitched"]


def _mask(spans, n, start="onset", end="offset"):
    m = np.zeros(n, bool)
    for s in spans:
        a, b = int(round(s[start] / GRID)), int(round(s[end] / GRID))
        m[max(a, 0):max(min(b, n), 0)] = True
    return m


def note_metrics(truth: dict, guide: dict) -> dict:
    """Guide vs ground truth. Pitched truth notes only for P/R/F; unpitched separately."""
    import mir_eval

    ref = truth["notes"]
    est = pitched(guide)
    ref_iv = np.array([[n["onset"], n["offset"]] for n in ref], float)
    ref_hz = np.array([n["hz"] for n in ref], float)
    est_iv = np.array([[n["start"], max(n["end"], n["start"] + 1e-3)] for n in est], float).reshape(-1, 2)
    est_hz = midi_to_hz([n["midi"] for n in est])
    out: dict = {"ref_notes": len(ref), "est_notes": len(est)}
    if len(est) == 0:
        out.update(p=0.0, r=0.0, f=0.0, p_off=0.0, r_off=0.0, f_off=0.0)
        return out
    p, r, f, _ = mir_eval.transcription.precision_recall_f1_overlap(
        ref_iv, ref_hz, est_iv, est_hz, onset_tolerance=0.05, pitch_tolerance=50.0, offset_ratio=None)
    po, ro, fo, _ = mir_eval.transcription.precision_recall_f1_overlap(
        ref_iv, ref_hz, est_iv, est_hz, onset_tolerance=0.05, pitch_tolerance=50.0, offset_ratio=0.2,
        offset_min_tolerance=0.05)
    out.update(p=p, r=r, f=f, p_off=po, r_off=ro, f_off=fo)

    # Onset errors: onset-only matching, pitch ignored.
    m_on = mir_eval.transcription.match_notes(ref_iv, ref_hz, est_iv, est_hz, onset_tolerance=0.05,
                                              pitch_tolerance=1e9, offset_ratio=None)
    on_err = np.array([est_iv[j, 0] - ref_iv[i, 0] for i, j in m_on])
    out["onset_err_median_ms"] = float(np.median(np.abs(on_err)) * 1000) if len(on_err) else None
    out["onset_bias_ms"] = float(np.median(on_err) * 1000) if len(on_err) else None

    # Pitch errors by time overlap: each truth note against the guide note covering most of it.
    errs, octave, found = [], 0, 0
    for i, n in enumerate(ref):
        a, b = n["onset"], n["offset"]
        ov = np.clip(np.minimum(est_iv[:, 1], b) - np.maximum(est_iv[:, 0], a), 0, None)
        j = int(np.argmax(ov))
        if ov[j] < 0.3 * (b - a):
            continue
        e = 1200 * np.log2(est_hz[j] / ref_hz[i])
        k = round(e / 1200)
        if k != 0 and abs(e - 1200 * k) < 100:
            octave += 1
        else:
            errs.append(e)
        if abs(e) < 50 and ov[j] >= 0.5 * (b - a):
            found += 1
    errs = np.abs(np.array(errs))
    # Time-weighted view (what grading sees): share of the true notes' time covered by a
    # guide note within 50 cents, and share of the guide's note time that is over a true
    # note but more than 50 cents off (silence or unpitched truth under a guide note is
    # counted separately as "extra").
    n_grid = int(max(ref_iv[:, 1].max(), est_iv[:, 1].max()) / GRID) + 2
    ref_c = np.full(n_grid, np.nan)
    est_c = np.full(n_grid, np.nan)
    for (a, b), hz in zip(ref_iv, ref_hz):
        ref_c[int(round(a / GRID)):int(round(b / GRID))] = 1200 * np.log2(hz / 440)
    for (a, b), hz in zip(est_iv, est_hz):
        est_c[int(round(a / GRID)):int(round(b / GRID))] = 1200 * np.log2(hz / 440)
    both = np.isfinite(ref_c) & np.isfinite(est_c)
    ok = both & (np.abs(np.nan_to_num(ref_c - est_c)) < 50)
    out["time_covered"] = float(ok.sum() / max(np.isfinite(ref_c).sum(), 1))
    out["time_wrong_pitch"] = float((both & ~ok).sum() / max(np.isfinite(est_c).sum(), 1))
    out["time_extra"] = float((np.isfinite(est_c) & ~np.isfinite(ref_c)).sum() / max(np.isfinite(est_c).sum(), 1))
    out["pitch_err_median_c"] = float(np.median(errs)) if len(errs) else None
    out["pitch_err_p90_c"] = float(np.percentile(errs, 90)) if len(errs) else None
    out["octave_errors"] = octave
    out["found_overlap"] = found / max(len(ref), 1)

    # Recall per melodic feature (onset within 50 ms and pitch within 50 cents).
    m_full = mir_eval.transcription.match_notes(ref_iv, ref_hz, est_iv, est_hz, onset_tolerance=0.05,
                                                pitch_tolerance=50.0, offset_ratio=None)
    hit = np.zeros(len(ref), bool)
    for i, _ in m_full:
        hit[i] = True
    feats = {}
    for key in ("vibrato", "scoop", "slide", "run", "multi"):
        idx = [i for i, n in enumerate(ref) if n.get(key)]
        if idx:
            feats[key] = {"n": len(idx), "recall": float(hit[idx].mean())}
    out["by_feature"] = feats

    # Unpitched (spoken) parts and breaths on a 10 ms grid.
    n = int(truth["duration_s"] / GRID) + 2
    gt_unp = _mask(truth["unpitched"], n)
    gt_pit = _mask(ref, n)
    est_unp = _mask([x for x in guide["notes"] if x["kind"] == "unpitched"], n, "start", "end")
    est_pit = _mask(est, n, "start", "end")
    out["unpitched_recall"] = float((gt_unp & est_unp).sum() / max(gt_unp.sum(), 1))
    out["unpitched_precision"] = float((gt_unp & est_unp).sum() / max(est_unp.sum(), 1))
    out["spoken_marked_pitched"] = float((gt_unp & est_pit).sum() / max(gt_unp.sum(), 1))
    out["sung_marked_unpitched"] = float((gt_pit & est_unp).sum() / max(gt_pit.sum(), 1))
    br = 0
    for x in est:
        a, b = x["start"], x["end"]
        cover = sum(max(0.0, min(b, s["offset"]) - max(a, s["onset"])) for s in truth["breaths"])
        if cover > 0.5 * (b - a):
            br += 1
    out["notes_on_breaths"] = br
    out["tuning_cents"] = guide.get("tuning_cents")
    out["tuning_err_c"] = (guide.get("tuning_cents") - truth["tuning_cents"]) if guide.get("tuning_cents") is not None else None
    return out


def agreement(a: dict, b: dict) -> dict:
    """Two guides of the same vocal (no ground truth): note F1 of a against b."""
    import mir_eval

    ra, rb = pitched(a), pitched(b)
    if not ra or not rb:
        return {"p": 0.0, "r": 0.0, "f": 0.0, "octave_disagreements": 0, "compared": 0}
    iv_a = np.array([[n["start"], max(n["end"], n["start"] + 1e-3)] for n in ra])
    iv_b = np.array([[n["start"], max(n["end"], n["start"] + 1e-3)] for n in rb])
    ha, hb = midi_to_hz([n["midi"] for n in ra]), midi_to_hz([n["midi"] for n in rb])
    p, r, f, _ = mir_eval.transcription.precision_recall_f1_overlap(iv_b, hb, iv_a, ha, onset_tolerance=0.05,
                                                                   pitch_tolerance=50.0, offset_ratio=None)
    # Octave disagreements by overlap.
    octs, tot = 0, 0
    for i, n in enumerate(ra):
        ov = np.clip(np.minimum(iv_b[:, 1], n["end"]) - np.maximum(iv_b[:, 0], n["start"]), 0, None)
        j = int(np.argmax(ov))
        if ov[j] < 0.5 * (n["end"] - n["start"]):
            continue
        tot += 1
        e = 1200 * np.log2(ha[i] / hb[j])
        if abs(e) > 600 and abs(e - 1200 * round(e / 1200)) < 100:
            octs += 1
    return {"p": p, "r": r, "f": f, "octave_disagreements": octs, "compared": tot}


def counts(guide: dict, low=0.5) -> dict:
    notes = guide["notes"]
    pit = [n for n in notes if n["kind"] == "pitched"]
    dur = guide.get("duration_s") or 1.0
    return {
        "notes": len(notes), "pitched": len(pit), "unpitched": len(notes) - len(pit),
        "low_confidence": sum(1 for n in pit if n["conf"] < low),
        "octave_flags": sum(1 for n in pit if n["octave"] in ("maybe_high", "maybe_low")),
        "tuning_cents": guide.get("tuning_cents"), "tuning_strength": guide.get("tuning_strength"),
        "notes_per_min": round(60 * len(pit) / dur, 1),
        "range_midi": [round(min(n["midi"] for n in pit), 2), round(max(n["midi"] for n in pit), 2)] if pit else None,
    }

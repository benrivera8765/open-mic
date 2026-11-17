# SPDX-License-Identifier: GPL-3.0-or-later
"""Lyrics alignment benchmark on JamendoLyrics (English subset).

Dataset: https://github.com/f90/jamendolyrics (Durand, Stoller, Ewert, ICASSP 2023;
the original 20-song English set is Stoller et al., ICASSP 2019). Audio and lyrics are
Creative Commons (per-song licence in JamendoLyrics.csv, column LicenseType); the
repository code and annotations are MIT. The dataset lives outside the repository in
``$JK_BUILD/eval/jamendolyrics`` and is never committed.

Steps (each caches its output under ``<dataset>/_work``):

``separate``   lead vocal of every song with the Studio's own separation
               (``jukeoke_studio.separate``: vocal model, then karaoke model -> lead).
               Run with the Python that has the separation dependencies.
``emissions``  wav2vec2 log-probabilities of the lead vocal (and of the full mix).
``transcribe`` Whisper words of the lead vocal (coarse anchors for text-only mode).
``align``      all variants, metrics, JSON results.
``report``     prints the markdown tables used in docs/benchmarks/alignment.md.

Example (Git Bash)::

    source scripts/devenv.sh
    D=$JK_BUILD/eval/jamendolyrics
    $JK_BUILD/studio-venv/Scripts/python.exe -I -B studio/bench/alignment_bench.py separate --data $D
    PY=$JK_BUILD/align-venv/Scripts/python.exe
    $PY -I studio/bench/alignment_bench.py emissions --data $D --models $JK_BUILD/align-venv/models
    $PY -I studio/bench/alignment_bench.py transcribe --data $D --models $JK_BUILD/align-venv/models
    $PY -I studio/bench/alignment_bench.py align --data $D
    $PY -I studio/bench/alignment_bench.py report --data $D

Metric: word-onset absolute error |predicted start - annotated start| in seconds,
pooled over all words of all songs (mean, median, p90, p95, max, share < 0.3 s); the
per-song mean ("AAE", the MIREX measure) is also given.
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))  # studio/ -> import jukeoke_studio


# ----------------------------------------------------------------------------- dataset

def english_songs(data: Path) -> list[dict]:
    with open(data / "JamendoLyrics.csv", encoding="utf-8") as f:
        rows = [r for r in csv.DictReader(f) if r["Language"] == "English"]
    for r in rows:
        r["name"] = Path(r["Filepath"]).stem
    return rows


def load_gt(data: Path, name: str):
    with open(data / "annotations" / "words" / f"{name}.csv", encoding="utf-8") as f:
        rows = list(csv.DictReader(f))
    on = np.array([float(r["word_start"]) for r in rows])
    off = np.array([float(r["word_end"]) for r in rows])
    with open(data / "annotations" / "lines" / f"{name}.csv", encoding="utf-8") as f:
        lines = [(float(r["start_time"]), float(r["end_time"]), r["lyrics_line"]) for r in csv.DictReader(f)]
    return on, off, lines


def load_lyrics(data: Path, name: str):
    from jukeoke_studio.lyrics import from_text

    text = (data / "lyrics" / f"{name}.txt").read_text(encoding="utf-8")
    return from_text(text)


def work(data: Path, *parts) -> Path:
    p = data.joinpath("_work", *parts)
    p.parent.mkdir(parents=True, exist_ok=True)
    return p


def read_audio(path: Path):
    import soundfile as sf

    x, sr = sf.read(str(path), dtype="float32", always_2d=True)
    return x, sr


# ---------------------------------------------------------------------------- separate

def cmd_separate(a):
    import soundfile as sf
    import torch
    from jukeoke_studio.separate import PRESETS, Separator

    data = Path(a.data)
    models_dir = Path(a.sep_models) if a.sep_models else data / "_work" / "sepmodels"
    sep = Separator(models_dir=models_dir, device="cuda")
    choice = PRESETS["full"]
    log = {}
    logp = work(data, "separation_times.json")
    if logp.exists():
        log = json.loads(logp.read_text())
    for r in english_songs(data):
        name = r["name"]
        out = work(data, "vocals", f"{name}.lead.wav")
        if out.exists() and not a.force:
            continue
        x, sr = read_audio(data / "mp3" / r["Filepath"])
        mix = np.ascontiguousarray(x.T)
        torch.cuda.synchronize()
        t0 = time.perf_counter()
        vocals = sep.run_model(choice["vocals"], mix, sr)["vocals"]
        lead = sep.run_model(choice["karaoke"], vocals, sr)["lead"]
        torch.cuda.synchronize()
        dt = time.perf_counter() - t0
        sf.write(str(out), lead.T, sr, subtype="FLOAT")
        sf.write(str(work(data, "vocals", f"{name}.vocals.wav")), vocals.T, sr, subtype="FLOAT")
        log[name] = {"seconds": dt, "audio_seconds": x.shape[0] / sr,
                     "models": [choice["vocals"], choice["karaoke"]]}
        logp.write_text(json.dumps(log, indent=1))
        print(f"{name}: {dt:.1f} s for {x.shape[0] / sr:.0f} s of audio", flush=True)


# --------------------------------------------------------------------------- emissions

def cmd_emissions(a):
    import torch
    from jukeoke_studio import align

    data = Path(a.data)
    am = align.AcousticModel(a.model, models_dir=a.models, device="cuda")
    times = {}
    tp = work(data, "emission_times.json")
    if tp.exists():
        times = json.loads(tp.read_text())
    tag = a.model.split("/")[-1]
    for r in english_songs(data):
        name = r["name"]
        for src in a.sources.split(","):
            for norm in a.norms.split(","):
                out = work(data, "emissions", f"{name}.{src}.{tag}.{norm}.npz")
                if out.exists() and not a.force:
                    continue
                if src == "mix":
                    x, sr = read_audio(data / "mp3" / r["Filepath"])
                else:
                    vp = work(data, "vocals", f"{name}.{src}.wav")
                    if not vp.exists():
                        print(f"{name}: no {src} stem yet, skipped", flush=True)
                        continue
                    x, sr = read_audio(vp)
                torch.cuda.synchronize()
                t0 = time.perf_counter()
                x16 = align.to_model_input(x, sr)
                lp = am.log_probs(x16, norm=norm)
                torch.cuda.synchronize()
                dt = time.perf_counter() - t0
                lv = align.frame_levels_db(x16, lp.shape[0])
                np.savez_compressed(out, lp=lp.astype(np.float16), levels=lv.astype(np.float32),
                                    duration=len(x16) / align.MODEL_SR)
                times[f"{name}.{src}.{tag}.{norm}"] = dt
                tp.write_text(json.dumps(times, indent=1))
                print(f"{name} {src} {norm}: {dt:.2f} s", flush=True)
    # vocab for offline alignment
    work(data, "emissions", f"vocab.{tag}.json").write_text(json.dumps(
        {"letters": am.letter_ids, "blank": am.blank_id, "sep": am.sep_id}))


# -------------------------------------------------------------------------- transcribe

def cmd_transcribe(a):
    from jukeoke_studio import transcribe

    data = Path(a.data)
    tr = transcribe.Transcriber(a.whisper, models_dir=a.models, device="cuda")
    for r in english_songs(data)[: a.limit]:
        name = r["name"]
        out = work(data, "transcripts", f"{name}.{a.whisper}.json")
        if out.exists() and not a.force:
            continue
        vp = work(data, "vocals", f"{name}.lead.wav")
        if not vp.exists():
            print(f"{name}: no lead stem yet, skipped", flush=True)
            continue
        x, sr = read_audio(vp)
        t0 = time.perf_counter()
        words, info = tr.transcribe_words(x, sr, language="en")
        dt = time.perf_counter() - t0
        out.write_text(json.dumps({"seconds": dt, "info": info,
                                   "words": [w.__dict__ for w in words]}, indent=0))
        print(f"{name}: {len(words)} words in {dt:.1f} s", flush=True)


def _wer_words(text: str) -> list[str]:
    import unicodedata

    out = []
    for w in text.lower().replace("’", "'").split():
        w = "".join(c for c in unicodedata.normalize("NFKD", w) if c.isalnum() or c == "'")
        w = w.strip("'")
        if w:
            out.append(w)
    return out


def word_error_rate(ref: list[str], hyp: list[str]) -> float:
    """Levenshtein distance on words / reference length."""
    d = np.arange(len(hyp) + 1)
    for i, r in enumerate(ref, 1):
        prev, d = d, np.empty_like(d)
        d[0] = i
        for j, h in enumerate(hyp, 1):
            d[j] = min(prev[j] + 1, d[j - 1] + 1, prev[j - 1] + (r != h))
    return float(d[-1]) / max(1, len(ref))


def cmd_wer(a):
    """Word error rate of the Whisper transcripts against the dataset lyrics."""
    data = Path(a.data)
    rows = {}
    for w in a.whisper.split(","):
        errs, refs, secs = 0.0, 0, []
        for s in english_songs(data):
            p = work(data, "transcripts", f"{s['name']}.{w}.json")
            if not p.exists():
                continue
            tj = json.loads(p.read_text())
            ref = _wer_words((data / "lyrics" / f"{s['name']}.txt").read_text(encoding="utf-8"))
            hyp = _wer_words(" ".join(x["text"] for x in tj["words"]))
            errs += word_error_rate(ref, hyp) * len(ref)
            refs += len(ref)
            secs.append(tj["seconds"])
        if refs:
            rows[w] = {"wer": errs / refs, "songs": len(secs), "seconds_mean": float(np.mean(secs))}
            print(f"{w}: WER {100 * errs / refs:.1f}% over {len(secs)} songs, "
                  f"{np.mean(secs):.1f} s per song", flush=True)
    if a.out:
        Path(a.out).write_text(json.dumps(rows, indent=1))


# ------------------------------------------------------------------------------- align

def metrics(err: np.ndarray) -> dict:
    ae = np.abs(err)
    if len(ae) == 0:
        return {}
    return {"n": int(len(ae)), "mean": float(ae.mean()), "median": float(np.median(ae)),
            "p90": float(np.percentile(ae, 90)), "p95": float(np.percentile(ae, 95)),
            "max": float(ae.max()), "within_0.3": float(np.mean(ae < 0.3)),
            "bias_median": float(np.median(err))}


def run_variant(data: Path, song: dict, variant: dict, vocab: dict, cache: dict):
    from jukeoke_studio import align
    from jukeoke_studio.lyrics import Lyrics

    name = song["name"]
    key = f"{name}.{variant['src']}.{variant['tag']}.{variant['norm']}"
    if key not in cache:
        z = np.load(work(data, "emissions", f"{key}.npz"))
        cache.clear()  # keep memory small: one song at a time
        cache[key] = (z["lp"].astype(np.float32), z["levels"], float(z["duration"]))
    lp, levels, duration = cache[key]
    lyr: Lyrics = load_lyrics(data, name)
    gt_on, gt_off, gt_lines = load_gt(data, name)
    n_words = lyr.word_count()
    assert n_words == len(gt_on), (name, n_words, len(gt_on))
    assert len(lyr.lines) == len(gt_lines), (name, len(lyr.lines), len(gt_lines))

    anchors = None
    transcript = None
    exclude = np.zeros(n_words, bool)
    mode = variant["mode"]
    rng = np.random.default_rng(1234)
    if mode in ("lines", "lines_noisy"):
        for i, ln in enumerate(lyr.lines):
            s = gt_lines[i][0]
            if mode == "lines_noisy":
                s = s + float(rng.normal(0.0, variant.get("noise", 0.3)))
            ln.start = s
        for i, ln in enumerate(lyr.lines):
            ln.end = lyr.lines[i + 1].start if i + 1 < len(lyr.lines) else None
    elif mode == "hard":
        anchors = [align.Anchor(i, gt_lines[i][0]) for i in range(len(lyr.lines))]
        k = 0
        for ln in lyr.lines:  # first words are exact by construction: excluded
            exclude[k] = True
            k += len(ln.words)
    elif mode == "whisper":
        tj = json.loads(work(data, "transcripts", f"{name}.{variant['whisper']}.json").read_text())
        transcript = [align.TranscriptWord(**w) for w in tj["words"]]
    cfg = align.AlignConfig(**variant.get("config", {}))
    t0 = time.perf_counter()
    res = align.align_with_log_probs(lp, levels, lyr, vocab_ids=vocab["letters"],
                                     blank_id=vocab["blank"],
                                     sep_id=vocab["sep"] if cfg.use_sep_as_blank else None,
                                     duration=duration, anchors=anchors,
                                     use_line_times=mode.startswith("lines"),
                                     transcript=transcript, config=cfg,
                                     model_name=variant["model"])
    dt = time.perf_counter() - t0
    pred_on = np.array([w["start"] for ln in res.lyrics["lines"] for w in ln["words"]])
    pred_off = np.array([w["end"] for ln in res.lyrics["lines"] for w in ln["words"]])
    keep = ~exclude
    return {"err": (pred_on - gt_on)[keep], "err_off": (pred_off - gt_off)[keep],
            "viterbi_s": dt, "level": res.constraint_level, "warnings": res.warnings}


VARIANTS = {
    # name: settings
    "mix_text": {"src": "mix", "mode": "none"},
    "lead_text": {"src": "lead", "mode": "none"},
    "lead_text_nohold": {"src": "lead", "mode": "none", "config": {"voiced_rel_db": 1000.0}},
    "lead_text_holdall": {"src": "lead", "mode": "none", "config": {"voiced_rel_db": -1000.0}},
    "lead_whisper": {"src": "lead", "mode": "whisper", "whisper": "large-v3"},
    "lead_whisper_turbo": {"src": "lead", "mode": "whisper", "whisper": "large-v3-turbo"},
    "lead_lines": {"src": "lead", "mode": "lines"},
    "lead_lines_noisy": {"src": "lead", "mode": "lines_noisy", "noise": 0.3},
    "lead_hard": {"src": "lead", "mode": "hard"},
}


def cmd_align(a):
    data = Path(a.data)
    tag = a.model.split("/")[-1]
    vocab = json.loads(work(data, "emissions", f"vocab.{tag}.json").read_text())
    songs = english_songs(data)
    # Tuning used only the first ten songs ("dev"); "test" is the other ten.
    if a.songs == "dev":
        songs = songs[:10]
    elif a.songs == "test":
        songs = songs[10:]
    names = a.variants.split(",") if a.variants else list(VARIANTS)
    extra_cfg = json.loads(a.config) if a.config else {}
    results = {}
    for vn in names:
        v = dict(VARIANTS[vn])
        v.setdefault("tag", tag)
        v.setdefault("model", a.model)
        v.setdefault("norm", a.norm)
        v["config"] = {**v.get("config", {}), **extra_cfg}
        all_err, all_off, per_song, vit = [], [], {}, []
        cache: dict = {}
        for s in songs:
            try:
                r = run_variant(data, s, v, vocab, cache)
            except FileNotFoundError as e:
                print(f"skip {vn} {s['name']}: {e}", file=sys.stderr)
                continue
            all_err.append(r["err"])
            all_off.append(r["err_off"])
            vit.append(r["viterbi_s"])
            per_song[s["name"]] = {**metrics(r["err"]), "level": r["level"], "warnings": r["warnings"]}
        if not all_err:
            continue
        err = np.concatenate(all_err)
        results[vn] = {"settings": v, "pooled": metrics(err),
                       "offset_pooled": metrics(np.concatenate(all_off)),
                       "song_mean_aae": float(np.mean([p["mean"] for p in per_song.values()])),
                       "viterbi_s_mean": float(np.mean(vit)), "per_song": per_song}
        m = results[vn]["pooled"]
        print(f"{vn:18s} n={m['n']:5d} mean={m['mean']:.3f} med={m['median']:.3f} "
              f"p90={m['p90']:.3f} p95={m['p95']:.3f} max={m['max']:.2f} "
              f"<0.3s={100 * m['within_0.3']:.1f}% bias={m['bias_median']:+.3f} "
              f"songAAE={results[vn]['song_mean_aae']:.3f}", flush=True)
    out = Path(a.out) if a.out else work(data, f"results.{tag}.{a.norm}.json")
    old = json.loads(out.read_text()) if out.exists() else {}
    old.update(results)
    out.write_text(json.dumps(old, indent=1))


def cmd_report(a):
    res = json.loads(Path(a.results).read_text())
    print("| Variant | Words | Mean | Median | p90 | p95 | Worst | Within 0.3 s | Median bias | Per-song AAE |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for vn, r in res.items():
        m = r["pooled"]
        print(f"| {vn} | {m['n']} | {m['mean']:.3f} | {m['median']:.3f} | {m['p90']:.3f} | "
              f"{m['p95']:.3f} | {m['max']:.2f} | {100 * m['within_0.3']:.1f}% | "
              f"{m['bias_median']:+.3f} | {r['song_mean_aae']:.3f} |")
    if a.per_song:
        vn = a.per_song
        print(f"\nPer song ({vn}):\n")
        print("| Song | Words | Mean | Median | Worst | Within 0.3 s |")
        print("|---|---|---|---|---|---|")
        for name, m in res[vn]["per_song"].items():
            print(f"| {name} | {m['n']} | {m['mean']:.3f} | {m['median']:.3f} | {m['max']:.2f} | "
                  f"{100 * m['within_0.3']:.1f}% |")


# ------------------------------------------------------------- key / tempo (analysis.md)
#
# Kept in this file because the analysis step belongs to the same workstream; results
# go to docs/benchmarks/analysis.md.

def synth_progression(tonic: int, mode: str, prog: list[int], *, sr=44100, bpm=100.0,
                      seed=0, detune_cents=0.0, drums=True, seconds=16.0):
    """A chord progression in a known key, with bass, simple drums at ``bpm``.

    ``prog``: chord roots as scale degrees (0-based) of the key's scale; triads are
    diatonic, except that in minor the V chord is major (harmonic minor), as in practice.
    """
    rng = np.random.default_rng(seed)
    scale = [0, 2, 4, 5, 7, 9, 11] if mode == "major" else [0, 2, 3, 5, 7, 8, 10]
    beat = 60.0 / bpm
    chord_len = 4 * beat
    n = int(seconds * sr)
    out = np.zeros(n)
    t = np.arange(n) / sr
    tune = 2 ** (detune_cents / 1200)

    def tone(f, a, s0, s1, harmonics=8):
        i0, i1 = int(s0 * sr), min(n, int(s1 * sr))
        if i1 <= i0:
            return
        tt = t[i0:i1] - s0
        env = np.minimum(1.0, tt / 0.01) * np.exp(-tt * 1.2)
        sig = sum((1.0 / k) * np.sin(2 * np.pi * f * k * tt + rng.uniform(0, 6.28))
                  for k in range(1, harmonics + 1) if f * k < sr / 2)
        out[i0:i1] += a * env * sig

    k = 0
    s = 0.0
    while s < seconds:
        deg = prog[k % len(prog)]
        root_pc = scale[deg % 7]
        notes = [scale[(deg + d) % 7] + 12 * ((deg + d) // 7) for d in (0, 2, 4)]
        if mode == "minor" and deg == 4:
            notes[1] = scale[6] + 1  # raised leading tone: major V
        inv = int(rng.integers(0, 3))
        notes = sorted(x + (12 if j < inv else 0) for j, x in enumerate(notes))
        base = 261.63 * 2 ** ((tonic - 0) / 12) * tune  # C4 * tonic offset
        for x in notes:
            tone(base * 2 ** (x / 12), 0.15, s, s + chord_len)
        tone(base / 4 * 2 ** (root_pc / 12), 0.3, s, s + chord_len, harmonics=4)
        s += chord_len
        k += 1
    if drums:
        nb = int(seconds / beat)
        for b in range(nb):
            i0 = int(b * beat * sr)
            m = min(n, i0 + int(0.12 * sr))
            tt = t[i0:m] - t[i0]
            if b % 2 == 0:   # kick
                out[i0:m] += 0.6 * np.sin(2 * np.pi * 55 * tt) * np.exp(-tt * 30)
            else:            # snare
                out[i0:m] += 0.25 * rng.standard_normal(m - i0) * np.exp(-tt * 25)
            h0 = int((b + 0.5) * beat * sr)
            h1 = min(n, h0 + int(0.03 * sr))
            out[h0:h1] += 0.08 * rng.standard_normal(max(0, h1 - h0))
    out /= max(1e-9, np.abs(out).max()) / 0.5
    return out.astype(np.float32)


PROGRESSIONS = {
    "major": [[0, 4, 5, 3], [0, 3, 4, 0], [5, 3, 0, 4], [0, 5, 1, 4]],   # I-V-vi-IV, I-IV-V-I, vi-IV-I-V, I-vi-ii-V
    "minor": [[0, 5, 2, 6], [0, 3, 4, 0], [0, 6, 5, 6], [0, 3, 0, 4]],   # i-VI-III-VII, i-iv-V-i, i-VII-VI-VII, i-iv-i-V
}


def mirex_key_score(est, ref) -> float:
    """MIREX key score: 1 exact, 0.5 a fifth above (same mode), 0.3 relative, 0.2
    parallel, else 0. As mir_eval.key.weighted_score with its default
    ``allow_descending_fifths=False``: a key a fifth *below* (a fourth above) scores 0."""
    (et, em), (rt, rm) = est, ref
    if et == rt and em == rm:
        return 1.0
    if em == rm and (et - rt) % 12 == 7:
        return 0.5
    if em != rm:
        rel = (rt + 9) % 12 if rm == "major" else (rt + 3) % 12
        if et == rel:
            return 0.3
        if et == rt:
            return 0.2
    return 0.0


def _tempo_ok(est, ref, tol=0.04):
    acc1 = est is not None and abs(est - ref) <= tol * ref
    acc2 = est is not None and any(abs(est - ref * f) <= tol * ref * f for f in (1, 2, 0.5, 3, 1 / 3))
    return acc1, acc2


def _eval_keys(items, profiles, compresses):
    """items: list of (audio, sr, (tonic, mode), bpm). Returns {(profile, compress): stats}."""
    from jukeoke_studio import analyse

    chromas = []
    for x, sr, _, _ in items:
        chromas.append({c: analyse.song_chroma(analyse._mono22(x, sr), compress=c) for c in compresses})
    out = {}
    for p in profiles:
        for c in compresses:
            exact, score, conf_ok, conf_bad = 0, 0.0, [], []
            for ch, (_, _, ref, _) in zip(chromas, items):
                sc = analyse.key_scores(ch[c], p).ravel()
                b = int(np.argmax(sc))
                est = (b % 12, "major" if b < 12 else "minor")
                w = np.exp(analyse.SOFTMAX_BETA * (sc - sc.max()))
                conf = float(w[b] / w.sum())
                s = mirex_key_score(est, ref)
                exact += s == 1.0
                score += s
                (conf_ok if s == 1.0 else conf_bad).append(conf)
            out[(p, c)] = {"n": len(items), "exact": exact / len(items), "mirex": score / len(items),
                           "conf_correct": float(np.mean(conf_ok)) if conf_ok else None,
                           "conf_wrong": float(np.mean(conf_bad)) if conf_bad else None}
    return out


def cmd_keys(a):
    from jukeoke_studio import analyse

    profiles = list(analyse.PROFILES)
    compresses = ["lin", "log10"]
    results = {}
    if a.set in ("synth", "all"):
        items = []
        rng = np.random.default_rng(7)
        for tonic in range(12):
            for mode in ("major", "minor"):
                for pi, prog in enumerate(PROGRESSIONS[mode]):
                    bpm = float(rng.uniform(70, 170))
                    cents = float(rng.uniform(-30, 30))
                    x = synth_progression(tonic, mode, prog, bpm=bpm, seed=tonic * 31 + pi,
                                          detune_cents=cents)
                    items.append((x, 44100, (tonic, mode), bpm))
        r = _eval_keys(items, profiles, compresses)
        tempo_hits = [_tempo_ok(analyse.estimate_tempo(x, sr), bpm) for x, sr, _, bpm in items]
        # Default settings, per progression (some are genuinely ambiguous).
        per_prog: dict[str, list[float]] = {}
        names_pc = analyse.PITCHES
        for k, (x, sr, ref, _) in enumerate(items):
            mode = ref[1]
            prog = PROGRESSIONS[mode][k % 4]
            est = analyse.estimate_key(x, sr)
            e = (names_pc.index(est["tonic"]), est["mode"])
            per_prog.setdefault(f"{mode} {prog}", []).append(mirex_key_score(e, ref) == 1.0)
        results["synthetic_default_per_progression"] = {k: float(np.mean(v)) for k, v in per_prog.items()}
        results["synthetic"] = {"keys": {f"{p}/{c}": v for (p, c), v in r.items()},
                                "tempo_acc1": float(np.mean([h[0] for h in tempo_hits])),
                                "tempo_acc2": float(np.mean([h[1] for h in tempo_hits])),
                                "n": len(items)}
        print("synthetic", json.dumps(results["synthetic"], indent=1), flush=True)
    if a.set in ("guitarset", "all"):
        gs = Path(a.guitarset)
        names = {"C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4, "F": 5, "F#": 6, "Gb": 6,
                 "G": 7, "G#": 8, "Ab": 8, "A": 9, "A#": 10, "Bb": 10, "B": 11}
        # Players 00-02 were used while choosing the chroma settings ("dev"); players
        # 03-05 were not looked at before the choice ("test").
        for part in ("comp", "solo"):
            for split, players in (("dev", ("00", "01", "02")), ("test", ("03", "04", "05"))):
                items = []
                for jf in sorted((gs / "annotation").glob(f"*_{part}.jams")):
                    if jf.stem[:2] not in players:
                        continue
                    j = json.loads(jf.read_text())
                    km = next(an for an in j["annotations"] if an["namespace"] == "key_mode")["data"][0]["value"]
                    bpm = next(an for an in j["annotations"] if an["namespace"] == "tempo")["data"][0]["value"]
                    t, m = km.split(":")
                    x, sr = read_audio(gs / "audio" / f"{jf.stem}_mic.wav")
                    items.append((x.T, sr, (names[t], m), float(bpm)))
                r = _eval_keys(items, profiles, compresses)
                tempo_hits = [_tempo_ok(analyse.estimate_tempo(x, sr), bpm) for x, sr, _, bpm in items]
                key = f"guitarset_{part}_{split}"
                results[key] = {"keys": {f"{p}/{c}": v for (p, c), v in r.items()},
                                "tempo_acc1": float(np.mean([h[0] for h in tempo_hits])),
                                "tempo_acc2": float(np.mean([h[1] for h in tempo_hits])),
                                "n": len(items)}
                print(key, json.dumps(results[key], indent=1), flush=True)
    if a.out:
        Path(a.out).write_text(json.dumps(results, indent=1))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("keys", help="key/tempo accuracy (docs/benchmarks/analysis.md)")
    s.add_argument("--set", default="all", choices=["synth", "guitarset", "all"])
    s.add_argument("--guitarset", default=None, help="GuitarSet folder with annotation/ and audio/")
    s.add_argument("--out", default=None)
    s = sub.add_parser("wer", help="word error rate of the cached Whisper transcripts")
    s.add_argument("--data", required=True)
    s.add_argument("--whisper", default="large-v3,large-v3-turbo")
    s.add_argument("--out", default=None)
    for n in ("separate", "emissions", "transcribe", "align", "report"):
        s = sub.add_parser(n)
        s.add_argument("--data", required=True, help="JamendoLyrics checkout")
        s.add_argument("--force", action="store_true")
        s.add_argument("--models", default=None, help="model cache dir (wav2vec2 / whisper)")
        s.add_argument("--model", default="facebook/wav2vec2-large-960h-lv60-self")
        if n == "separate":
            s.add_argument("--sep-models", default=None)
        if n == "emissions":
            s.add_argument("--sources", default="lead,mix")
            s.add_argument("--norms", default="global")
        if n == "transcribe":
            s.add_argument("--whisper", default="large-v3-turbo")
            s.add_argument("--limit", type=int, default=None, help="first N songs only")
        if n == "align":
            s.add_argument("--variants", default=None)
            s.add_argument("--songs", default="all", choices=["all", "dev", "test"])
            s.add_argument("--norm", default="global")
            s.add_argument("--config", default=None, help="JSON AlignConfig overrides")
            s.add_argument("--out", default=None)
        if n == "report":
            s.add_argument("--results", required=True)
            s.add_argument("--per-song", default=None)
    a = p.parse_args()
    {"separate": cmd_separate, "emissions": cmd_emissions, "transcribe": cmd_transcribe,
     "align": cmd_align, "report": cmd_report, "keys": cmd_keys, "wer": cmd_wer}[a.cmd](a)


if __name__ == "__main__":
    main()

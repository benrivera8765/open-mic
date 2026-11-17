# SPDX-License-Identifier: GPL-3.0-or-later
"""Fetch the evaluation sets used by the separation benchmark.

Nothing fetched here is ever committed: everything goes under
``$JK_BUILD/eval`` (outside the repository). Two sets:

``musdb``
    The 50-track *test* split of MUSDB18-HQ (Rafii et al., 2019), official
    Zenodo record 3338373. Licence: "provided for educational purposes only
    and the material contained in them should not be used for any commercial
    purpose without the express permission of the copyright holders". We use
    it for evaluation only and never redistribute it. The record is one
    22.7 GB zip; only the test members (7.9 GB compressed) are downloaded, using
    HTTP range requests against the zip's central directory, so the training
    half is never fetched.

``ccmixter``
    True lead / backing vocal stems: ccMixter a cappellas whose lead and
    backing parts were uploaded as separate files, each under its uploader's
    licence (CC BY 2.5/3.0/4.0 or CC0). The list of uploads and which file is
    which comes from musetric/lab (MIT licence), file
    ``musetric_toolkit/distill/data/ccmixter_voices.json``, copied verbatim to
    ``bench/data/ccmixter_voices.json``; the building rules (lag check against
    the uploader's reference mix, backing shift) follow musetric's
    ``ccmixter_voices.py``. Files are downloaded from ccmixter.org, one
    request per second. Output: ``tracks/<id>/{lead,backing[,instrumental]}.wav``,
    float32 WAV at 44.1 kHz, no gain applied.

Usage (from the repo root, with the Studio venv)::

    python -I studio/bench/fetch_eval.py musdb    --out "$JK_BUILD/eval/musdb18hq"
    python -I studio/bench/fetch_eval.py ccmixter --out "$JK_BUILD/eval/ccmixter_voices"
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import io
import json
import re
import struct
import sys
import time
import urllib.parse
import urllib.request
import zipfile
import zlib
from pathlib import Path

import numpy as np

USER_AGENT = "jukeoke-eval-fetch/0.1 (open-source karaoke app; separation benchmark)"

# --------------------------------------------------------------------------------------
# MUSDB18-HQ via HTTP range requests
# --------------------------------------------------------------------------------------

MUSDB_URL = "https://zenodo.org/records/3338373/files/musdb18hq.zip?download=1"
MUSDB_LICENCE = """# MUSDB18-HQ (test split) - evaluation only

Source: https://zenodo.org/records/3338373 (DOI 10.5281/zenodo.3338373),
Z. Rafii, A. Liutkus, F.-R. Stoter, S. I. Mimilakis, R. Bittner, "MUSDB18-HQ - an
uncompressed version of MUSDB18", 2019.

Licence (from the record): MUSDB18-HQ "is provided for educational purposes only and
the material contained in them should not be used for any commercial purpose without
the express permission of the copyright holders". 100 tracks derive from DSD100
(Mixing Secrets library), 46 from MedleyDB (CC BY-NC-SA 4.0), 2 from Native
Instruments stems, 2 from The Easton Ellises (CC BY-NC-SA 3.0).

Jukeoke uses it only to measure separation quality. It is never committed,
bundled or redistributed.
"""


def _http_get(url: str, headers: dict[str, str] | None = None, timeout: float = 120.0) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, **(headers or {})})
    with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310 (fixed https URLs)
        return r.read()


class HttpRangeFile(io.RawIOBase):
    """Read-only, seekable file object backed by HTTP range requests.

    Only used to let :mod:`zipfile` parse the remote central directory; member
    data is fetched separately in large streamed requests.
    """

    def __init__(self, url: str) -> None:
        super().__init__()
        self.url = url
        self.pos = 0
        req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=60) as r:  # noqa: S310
            self.size = int(r.headers["Content-Length"])

    def readable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.pos

    def seek(self, offset: int, whence: int = 0) -> int:
        base = {0: 0, 1: self.pos, 2: self.size}[whence]
        self.pos = base + offset
        return self.pos

    def read(self, n: int = -1) -> bytes:
        if n is None or n < 0:
            n = self.size - self.pos
        if n == 0 or self.pos >= self.size:
            return b""
        end = min(self.pos + n, self.size) - 1
        data = _http_get(self.url, {"Range": f"bytes={self.pos}-{end}"})
        self.pos += len(data)
        return data


def _fetch_member(url: str, info: zipfile.ZipInfo, dest: Path, attempts: int = 5) -> str:
    """Stream one zip member to ``dest``, inflating on the fly, and check its CRC-32."""
    if dest.exists() and dest.stat().st_size == info.file_size:
        return f"have {dest.name}"
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")
    for attempt in range(attempts):
        try:
            # Local file header: 30 fixed bytes, then name and extra field.
            hdr = _http_get(url, {"Range": f"bytes={info.header_offset}-{info.header_offset + 29}"})
            sig, *_rest = struct.unpack("<IHHHHHIIIHH", hdr)
            if sig != 0x04034B50:
                raise IOError("bad local header signature")
            name_len, extra_len = struct.unpack("<HH", hdr[26:30])
            start = info.header_offset + 30 + name_len + extra_len
            end = start + info.compress_size - 1
            req = urllib.request.Request(
                url, headers={"User-Agent": USER_AGENT, "Range": f"bytes={start}-{end}"}
            )
            crc = 0
            size = 0
            inflater = zlib.decompressobj(-15) if info.compress_type == zipfile.ZIP_DEFLATED else None
            with urllib.request.urlopen(req, timeout=300) as r, open(part, "wb") as out:  # noqa: S310
                while True:
                    block = r.read(1 << 20)
                    if not block:
                        break
                    data = inflater.decompress(block) if inflater else block
                    crc = zlib.crc32(data, crc)
                    size += len(data)
                    out.write(data)
                if inflater:
                    tail = inflater.flush()
                    crc = zlib.crc32(tail, crc)
                    size += len(tail)
                    out.write(tail)
            if size != info.file_size or crc != info.CRC:
                raise IOError(f"size/CRC mismatch ({size} vs {info.file_size})")
            part.replace(dest)
            return f"ok   {dest.parent.name}/{dest.name} ({size / 1e6:.1f} MB)"
        except Exception as exc:  # network hiccup: retry with back-off
            if attempt == attempts - 1:
                raise
            time.sleep(5 * (attempt + 1))
            print(f"retry {dest.name}: {exc}", flush=True)
    raise AssertionError("unreachable")


def fetch_musdb(out: Path, workers: int, tracks: list[str] | None) -> None:
    out.mkdir(parents=True, exist_ok=True)
    (out / "LICENSE-EVAL.md").write_text(MUSDB_LICENCE, encoding="utf-8")
    remote = HttpRangeFile(MUSDB_URL)
    z = zipfile.ZipFile(remote)
    members = [
        i
        for i in z.infolist()
        if not i.is_dir() and "/test/" in "/" + i.filename and i.filename.endswith(".wav")
    ]
    if tracks:
        members = [i for i in members if i.filename.split("/")[-2] in tracks]
    total = sum(i.compress_size for i in members)
    print(f"{len(members)} files, {total / 1e9:.2f} GB compressed", flush=True)
    jobs = {}
    with cf.ThreadPoolExecutor(max_workers=workers) as pool:
        for info in members:
            track = info.filename.split("/")[-2]
            dest = out / "test" / track / Path(info.filename).name
            jobs[pool.submit(_fetch_member, MUSDB_URL, info, dest)] = info
        for n, fut in enumerate(cf.as_completed(jobs), 1):
            print(f"[{n}/{len(jobs)}] {fut.result()}", flush=True)


# --------------------------------------------------------------------------------------
# ccMixter lead / backing set
# --------------------------------------------------------------------------------------

CCMIXTER_API = "https://ccmixter.org/api/query?f=json&ids={}"
SPEC = Path(__file__).with_name("data") / "ccmixter_voices.json"
SR = 44100
CHECK_RATE = 11025
MAX_LAG_SECONDS = 2.0
MIN_SHIFT_MS = 5.0
MIN_PEAK = 0.2
LICENSES = {
    "creativecommons.org/licenses/by/2.5/": "CC-BY-2.5",
    "creativecommons.org/licenses/by/3.0/": "CC-BY-3.0",
    "creativecommons.org/licenses/by/4.0/": "CC-BY-4.0",
    "creativecommons.org/publicdomain/zero/1.0/": "CC0-1.0",
}


def _polite_get(url: str, referer: str | None = None) -> bytes:
    time.sleep(1.0)  # one request a second, as musetric does
    headers = {"Referer": referer} if referer else {}
    for attempt in range(3):
        try:
            return _http_get(url, headers, timeout=180)
        except Exception:
            if attempt == 2:
                raise
            time.sleep(10)
    raise AssertionError("unreachable")


def _load(path: Path, rate: int, channels: int) -> np.ndarray:
    """Decode to float32 [channels, frames] at ``rate`` (soxr VHQ if a rate change is needed)."""
    import soundfile as sf
    import soxr

    audio, sr = sf.read(str(path), dtype="float32", always_2d=True)  # [frames, ch]
    if audio.shape[1] == 1 and channels == 2:
        audio = np.repeat(audio, 2, axis=1)
    elif audio.shape[1] >= 2 and channels == 1:
        audio = audio[:, :2].mean(axis=1, keepdims=True)
    elif audio.shape[1] > 2:
        audio = audio[:, :2]
    if sr != rate:
        audio = soxr.resample(audio, sr, rate, quality="VHQ").astype(np.float32)
    return np.ascontiguousarray(audio.T)


def _summed(paths: list[Path], rate: int, channels: int) -> np.ndarray | None:
    parts = [_load(p, rate, channels) for p in paths]
    if not parts:
        return None
    out = np.zeros((channels, max(p.shape[1] for p in parts)), np.float32)
    for p in parts:
        out[:, : p.shape[1]] += p
    return out


def _lag_of(ref: np.ndarray, part: np.ndarray) -> tuple[int, float]:
    """Lag (samples at CHECK_RATE) and normalised peak of the cross-correlation."""
    a, b = ref[0].astype(np.float64), part[0].astype(np.float64)
    n = 1 << int(np.ceil(np.log2(len(a) + len(b))))
    corr = np.fft.irfft(np.fft.rfft(a, n) * np.conj(np.fft.rfft(b, n)), n)
    max_lag = int(MAX_LAG_SECONDS * CHECK_RATE)
    lags = np.concatenate([np.arange(0, max_lag + 1), np.arange(-max_lag, 0)])
    window = np.concatenate([corr[: max_lag + 1], corr[-max_lag:]])
    norm = np.linalg.norm(a) * np.linalg.norm(b) + 1e-12
    best = int(np.argmax(np.abs(window)))
    return int(lags[best]), float(window[best] / norm)


def _shifted(a: np.ndarray, samples: int) -> np.ndarray:
    if samples > 0:
        return np.pad(a, ((0, 0), (samples, 0)))[:, : a.shape[1]]
    if samples < 0:
        return np.pad(a[:, -samples:], ((0, 0), (0, -samples)))
    return a


def _write(folder: Path, stems: dict[str, np.ndarray]) -> None:
    import soundfile as sf

    folder.mkdir(parents=True, exist_ok=True)
    for name, audio in stems.items():
        sf.write(str(folder / f"{name}.wav"), audio.T, SR, subtype="FLOAT")


def fetch_ccmixter(out: Path) -> None:
    spec = json.loads(SPEC.read_text(encoding="utf-8"))
    src = out / "src"
    rows = []
    for item in spec:
        uid = item["id"]
        meta_path = src / str(uid) / "meta.json"
        if meta_path.exists():
            entry = json.loads(meta_path.read_text(encoding="utf-8"))
        else:
            entry = json.loads(_polite_get(CCMIXTER_API.format(uid)))[0]
            meta_path.parent.mkdir(parents=True, exist_ok=True)
            meta_path.write_text(json.dumps(entry, indent=1), encoding="utf-8")
        files = {f["file_name"]: f["download_url"] for f in entry["files"]}
        shortest = min(files, key=len)
        base = re.sub(r"_\d+$", "", shortest.rsplit(".", 1)[0])

        def name_for(suffix: str) -> str:
            for cand in (base + suffix, base.rstrip("_") + suffix):
                if cand in files:
                    return cand
            raise KeyError(f"{uid}: no file {base + suffix}")

        def fetch(suffix: str) -> Path:
            name = name_for(suffix)
            target = src / str(uid) / name
            if not target.exists():
                data = _polite_get(files[name], referer=entry["file_page_url"])
                tmp = target.with_suffix(target.suffix + ".part")
                tmp.write_bytes(data)
                tmp.replace(target)
            return target

        paths = {p: [fetch(s) for s in item[p]] for p in ("lead", "backing", "accompaniment")}
        check: dict = {}
        if item["reference"]:
            ref = _load(fetch(item["reference"]), CHECK_RATE, 1)
            parts = ("lead", "backing", "accompaniment") if item["kind"] == "mix" else ("lead", "backing")
            for p in parts:
                if paths[p]:
                    lag, peak = _lag_of(ref, _summed(paths[p], CHECK_RATE, 1))
                    check[p] = {"lag_ms": round(lag / CHECK_RATE * 1000, 1), "peak": round(peak, 3)}
        groups = {p: _summed(ps, SR, 2) for p, ps in paths.items()}
        n = max(a.shape[1] for a in groups.values() if a is not None)
        stems = {
            p: np.zeros((2, n), np.float32) if a is None else np.pad(a, ((0, 0), (0, n - a.shape[1])))
            for p, a in groups.items()
        }
        if "lead" in check and "backing" in check:
            rel = check["lead"]["lag_ms"] - check["backing"]["lag_ms"]
            if abs(rel) >= MIN_SHIFT_MS and check["backing"]["peak"] >= MIN_PEAK:
                stems["backing"] = _shifted(stems["backing"], round(rel / 1000 * SR))
                check["backing_shifted_ms"] = rel
        vocals = stems["lead"] + stems["backing"]
        track = f"ccm-{uid}"
        # Only the parts are stored; loaders build "a cappella" (lead + backing) and
        # "mix" (lead + backing + instrumental) inputs from them.
        parts = {"lead": stems["lead"], "backing": stems["backing"]}
        if item["accompaniment"]:
            parts["instrumental"] = stems["accompaniment"]
        _write(out / "tracks" / track, parts)
        lic_url = entry["license_url"].split("//", 1)[-1]
        rms = lambda a: float(np.sqrt(np.mean(a.astype(np.float64) ** 2)) + 1e-12)  # noqa: E731
        rows.append({
            "track": track, "upload": uid, "title": entry["upload_name"],
            "artist": entry.get("user_real_name") or entry["user_name"], "uploader": entry["user_name"],
            "license": LICENSES.get(lic_url, lic_url), "license_url": entry["license_url"],
            "page": entry["file_page_url"], "seconds": round(n / SR, 1),
            "backing_db_re_vocals": round(20 * np.log10(rms(stems["backing"]) / rms(vocals)), 1),
            "has_mix": bool(item["accompaniment"]), "check": check,
        })
        print(json.dumps({k: rows[-1][k] for k in ("track", "seconds", "license", "check")}), flush=True)
    (out / "tracks.json").write_text(json.dumps(rows, indent=1, ensure_ascii=False), encoding="utf-8")
    table = "".join(
        f"| {r['artist']} | {r['title']} | [{r['license']}]({r['license_url']}) | [page]({r['page']}) |\n"
        for r in rows
    )
    (out / "LICENSE-EVAL.md").write_text(
        "# ccMixter lead/backing evaluation set\n\n"
        "Separately uploaded lead and backing vocal parts of ccMixter a cappellas, each under its\n"
        "uploader's licence. Upload list: musetric/lab (MIT), ccmixter_voices.json.\n"
        "tracks/<id>/: lead.wav, backing.wav and, where the upload has them, instrumental.wav\n"
        "(the upload's own instrument stems). A cappella input = lead + backing; mix input =\n"
        "lead + backing + instrumental.\n"
        "Used for evaluation only; never committed.\n\n"
        "| Artist | Title | Licence | Source |\n| --- | --- | --- | --- |\n" + table,
        encoding="utf-8",
    )
    print(f"{len(rows)} a cappellas, {sum(r['has_mix'] for r in rows)} mixes -> {out}")


def main(argv: list[str] | None = None) -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("musdb", help="MUSDB18-HQ test split (7.9 GB) via range requests")
    m.add_argument("--out", type=Path, required=True)
    m.add_argument("--workers", type=int, default=8)
    m.add_argument("--tracks", nargs="*", help="only these track folder names")
    c = sub.add_parser("ccmixter", help="ccMixter lead/backing set (CC BY / CC0)")
    c.add_argument("--out", type=Path, required=True)
    args = ap.parse_args(argv)
    if args.cmd == "musdb":
        fetch_musdb(args.out, args.workers, args.tracks)
    else:
        fetch_ccmixter(args.out)


if __name__ == "__main__":
    sys.exit(main())

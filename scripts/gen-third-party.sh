#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates THIRD_PARTY_LICENSES.md (WORK_ORDER section 10) from the real inputs:
#
#   - Rust:   `cargo metadata --format-version 1 --locked` filtered for
#             x86_64-pc-windows-gnu and aarch64-apple-darwin; the dependency graph of the
#             shipped binaries (the app `jukeoke`, the tools `jukeoke-cli` and `sck_probe`),
#             split into "linked into the binaries" and "build time only";
#   - npm:    app/ui/package-lock.json (production packages = the UI bundle; dev tooling
#             listed separately);
#   - Python: the *.dist-info folders of a staged Studio runtime
#             (<stick>/Windows/Studio/python/Lib/site-packages) and its runtime-binaries.tsv
#             (every bundled .dll/.pyd/.exe with its component and licence);
#   - the hand-maintained facts below (Rubber Band, our WebView2 loader, fonts, ...).
#
# It also writes the folder third_party_licenses/ (scripts/make-dist.sh copies it to
# dist/Licenses/third-party/): the actual licence and notice FILES of every Rust crate
# linked into the shipped binaries (read from the cargo registry sources), of the npm
# packages in the UI bundle (and the Vite/Rolldown code the build emits into it, from
# app/ui/node_modules), of the Studio's Python packages whose wheels ship none, and
# Rubber Band's COPYING; plus MISSING.txt, which lists exactly what could not be found
# and what is relied on instead. MIT, BSD and Apache-2.0 require these notices to travel
# with binaries.
#
# Every SPDX expression is checked for compatibility with distributing Jukeoke under
# GPL-3.0-or-later; anything that is not plainly compatible is flagged in the output.
#
# Usage:
#   scripts/gen-third-party.sh [--studio-runtime DIR] [--out FILE] [--texts-dir DIR]
#                              [--no-texts] [--check]
#
#   --studio-runtime DIR  the staged `Windows/Studio` folder (default: the first that exists
#                         of dist/Windows/Studio (what ships), then
#                         $JK_BUILD/dist-staging/stick1/Windows/Studio and
#                         $JK_BUILD/e2e-stick/Windows/Studio).
#                         Without one (CI, a Mac) the Python sections of the existing file
#                         are kept unchanged and a warning is printed.
#   --out FILE            output (default: THIRD_PARTY_LICENSES.md in the repo root).
#   --texts-dir DIR       the licence-text folder (default: third_party_licenses/ in the
#                         repo root). Needs the crates' sources (cargo fetch) and
#                         app/ui/node_modules (npm ci). Without a Studio runtime its
#                         python-packages.txt is kept as it was.
#   --no-texts            do not touch (or check) the licence-text folder.
#   --check               write nothing; exit 1 if the file or the folder is out of date.
#
# Needs: cargo (offline is fine once the crates are fetched), Python 3.8+ (standard library
# only; $PYTHON overrides the interpreter). The output is deterministic and holds no
# absolute paths.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/THIRD_PARTY_LICENSES.md"
RUNTIME=""
CHECK=0
TEXTS="$ROOT/third_party_licenses"
while [ $# -gt 0 ]; do
    case "$1" in
        --studio-runtime) RUNTIME="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --texts-dir) TEXTS="$2"; shift 2 ;;
        --no-texts) TEXTS=""; shift ;;
        --check) CHECK=1; shift ;;
        -h|--help) sed -n '2,49p' "$0"; exit 0 ;;
        *) echo "gen-third-party: unknown option $1" >&2; exit 2 ;;
    esac
done

if [ -z "$RUNTIME" ]; then
    for c in "$ROOT/dist/Windows/Studio" \
             ${JK_BUILD:+"$JK_BUILD/dist-staging/stick1/Windows/Studio"} \
             ${JK_BUILD:+"$JK_BUILD/e2e-stick/Windows/Studio"}; do
        if [ -f "$c/runtime-binaries.tsv" ]; then RUNTIME="$c"; break; fi
    done
fi

PY="${PYTHON:-}"
if [ -z "$PY" ]; then
    for c in python3 python; do
        if command -v "$c" >/dev/null 2>&1 && "$c" -c 'import sys; sys.exit(sys.version_info < (3, 8))' 2>/dev/null; then
            PY="$c"; break
        fi
    done
fi
[ -n "$PY" ] || { echo "gen-third-party: no Python 3.8+ found (set PYTHON)" >&2; exit 2; }

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

meta() {  # meta <target> <outfile>
    if ! (cd "$ROOT" && cargo metadata --format-version 1 --locked --offline --filter-platform "$1" >"$2" 2>"$TMPD/meta.err"); then
        (cd "$ROOT" && cargo metadata --format-version 1 --locked --filter-platform "$1" >"$2") || {
            cat "$TMPD/meta.err" >&2; echo "gen-third-party: cargo metadata failed for $1" >&2; exit 1; }
    fi
}
meta x86_64-pc-windows-gnu "$TMPD/meta-windows.json"
meta aarch64-apple-darwin "$TMPD/meta-macos.json"

"$PY" -I -X utf8 - "$ROOT" "$TMPD" "${RUNTIME:-}" "$OUT" "$CHECK" "$TEXTS" <<'PYEOF'
import collections
import csv
import email.parser
import hashlib
import json
import os
import re
import sys

ROOT, TMPD, RUNTIME, OUT, CHECK = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5] == "1"
TEXTS = sys.argv[6]
warnings = []

# --------------------------------------------------------------------------------------
# SPDX expressions and GPL-3.0-or-later compatibility
# --------------------------------------------------------------------------------------
# "yes": may be combined with and distributed under GPL-3.0-or-later.
# "redist": not open source, but its licence allows redistributing the binary.
# "no" / "unknown": flagged.
YES = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-1-Clause", "BSD-2-Clause", "BSD-3-Clause",
    "BSD-3-Clause-Clear", "BSD-2-Clause-Patent", "0BSD", "ISC", "Zlib", "Unicode-3.0",
    "Unicode-DFS-2016", "CC0-1.0", "Unlicense", "BSL-1.0", "MPL-2.0", "LGPL-2.0-or-later",
    "LGPL-2.1-only", "LGPL-2.1-or-later", "LGPL-3.0-only", "LGPL-3.0-or-later",
    "GPL-2.0-or-later", "GPL-3.0-only", "GPL-3.0-or-later", "PSF-2.0", "Python-2.0",
    "ZPL-2.1", "blessing", "CNRI-Python", "NCSA", "CDLA-Permissive-2.0", "Ruby",
}
REDIST = {"LicenseRef-NVIDIA-Proprietary", "LicenseRef-Proprietary-Redistributable"}
NO = {"GPL-2.0-only", "GPL-2.0", "GPL-1.0-only", "CC-BY-NC-4.0", "CC-BY-NC-SA-4.0",
      "SSPL-1.0", "BUSL-1.1", "LicenseRef-Proprietary"}
NOTES = {
    "MPL-2.0": "MPL-2.0 is file-level copyleft: changes to those files stay MPL; it is "
               "compatible with GPL-3.0-or-later (MPL-2.0 section 3.3). Their source is on "
               "crates.io / PyPI.",
    "CNRI-Python": "CNRI-Python is part of the Python licence history (regex inherits it from "
                   "CPython's sre). The FSF and CNRI disagree about the 1.6.1 choice-of-law "
                   "clause; it is treated here as compatible, as CPython itself is.",
    "LicenseRef-NVIDIA-Proprietary": "NVIDIA's CUDA libraries: redistributable under NVIDIA's "
                   "EULA (Attachment A, 'Distributable' runtime files), not open source. See "
                   "'Overall statement'.",
    "BSD-2-Clause-Patent": "BSD-2-Clause-Patent (BSD+Patent) is OSI-approved and "
                   "GPL-compatible (it adds an explicit patent grant).",
}
RANK = {"yes": 3, "redist": 2, "unknown": 1, "no": 0}
TOKEN = re.compile(r"\(|\)|[A-Za-z0-9.+:_-]+")


def norm_expr(expr):
    e = (expr or "").strip()
    e = re.sub(r"\s*/\s*", " OR ", e)          # old crates.io style "MIT/Apache-2.0"
    return re.sub(r"\s+", " ", e)


def spdx_ids(expr):
    return [t for t in TOKEN.findall(norm_expr(expr)) if t not in ("(", ")", "AND", "OR", "WITH")]


def compat(expr):
    """GPL-3.0-or-later compatibility of an SPDX expression (best alternative of an OR)."""
    toks = TOKEN.findall(norm_expr(expr))
    if not toks:
        return "unknown"
    pos = 0

    def ident(t):
        if t in YES:
            return "yes"
        if t in REDIST:
            return "redist"
        if t in NO:
            return "no"
        return "unknown"

    def factor():
        nonlocal pos
        if pos < len(toks) and toks[pos] == "(":
            pos += 1
            v = expr_()
            pos += 1  # ")"
        else:
            v = ident(toks[pos]); pos += 1
        if pos < len(toks) and toks[pos] == "WITH":   # exceptions only add permissions
            pos += 2
        return v

    def term():
        nonlocal pos
        v = factor()
        while pos < len(toks) and toks[pos] == "AND":
            pos += 1
            w = factor()
            v = v if RANK[v] <= RANK[w] else w
        return v

    def expr_():
        nonlocal pos
        v = term()
        while pos < len(toks) and toks[pos] == "OR":
            pos += 1
            w = term()
            v = v if RANK[v] >= RANK[w] else w
        return v

    try:
        return expr_()
    except IndexError:
        return "unknown"


VERDICT = {"yes": "yes", "redist": "redistributable, not open source",
           "unknown": "**REVIEW: unknown licence**", "no": "**NO: not GPL-3.0-compatible**"}


def sha12(path):
    try:
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()[:12]
    except OSError:
        return "missing"


def mib(n):
    return f"{n / 1048576:,.1f} MB"


# --------------------------------------------------------------------------------------
# Rust
# --------------------------------------------------------------------------------------
SHIPPED = {"jukeoke": "the app (`Jukeoke.exe`, `Jukeoke.app`)",
           "jk-cli": "`jukeoke-cli` (tool)",
           "jk-recorder": "`sck_probe` (macOS recorder probe, an example of jk-recorder)"}


def rust_graph(path):
    m = json.load(open(path, encoding="utf-8"))
    pk = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    ws = set(m["workspace_members"])
    is_pm = lambda i: any("proc-macro" in t["kind"] for t in pk[i]["targets"])
    roots = [i for i in ws if pk[i]["name"] in SHIPPED]
    linked, seeds, stack = set(), set(), list(roots)
    while stack:                       # normal (linked) dependencies, minus proc-macros
        i = stack.pop()
        if i in linked:
            continue
        linked.add(i)
        for d in nodes[i]["deps"]:
            kinds = {k["kind"] for k in d["dep_kinds"]}
            if None in kinds:
                (seeds.add if is_pm(d["pkg"]) else stack.append)(d["pkg"])
            if "build" in kinds:
                seeds.add(d["pkg"])
    build, stack = set(), list(seeds)  # build scripts, proc-macros and what they use
    while stack:
        i = stack.pop()
        if i in build:
            continue
        build.add(i)
        for d in nodes[i]["deps"]:
            if {k["kind"] for k in d["dep_kinds"]} & {None, "build"}:
                stack.append(d["pkg"])
    def info(i):
        p = pk[i]
        src = p.get("source") or ""
        return {"name": p["name"], "version": p["version"], "license": p.get("license") or "",
                "license_file": bool(p.get("license_file")),
                "license_path": p.get("license_file") or "",
                "dir": os.path.dirname(p["manifest_path"]),
                "authors": p.get("authors") or [], "repository": p.get("repository") or "",
                "source": "workspace" if i in ws else ("crates.io" if "crates.io-index" in src else src.split("+")[0] or "path"),
                "workspace": i in ws}
    return ({i: info(i) for i in linked}, {i: info(i) for i in build - linked})


rw_l, rw_b = rust_graph(os.path.join(TMPD, "meta-windows.json"))
rm_l, rm_b = rust_graph(os.path.join(TMPD, "meta-macos.json"))


def merge(win, mac):
    out = {}
    for tgt, d in (("Windows", win), ("macOS", mac)):
        for v in d.values():
            k = (v["name"], v["version"])
            out.setdefault(k, dict(v, targets=set()))["targets"].add(tgt)
    return out


rust_linked = merge(rw_l, rm_l)
rust_build = merge(rw_b, rm_b)
for k in list(rust_build):
    if k in rust_linked:
        del rust_build[k]


def crate_label(v):
    t = v["targets"]
    tag = "" if len(t) == 2 else (" (Windows)" if "Windows" in t else " (macOS)")
    return f"{v['name']} {v['version']}{tag}"


def rust_section(crates, title_note):
    groups = collections.defaultdict(list)
    for v in crates.values():
        expr = norm_expr(v["license"]) or "(no licence field)"
        groups[expr].append(v)
    lines = ["| licence (as declared, `/` read as OR) | GPL-3.0-or-later compatible | crates |",
             "|---|---|---|"]
    flagged = []
    for expr, vs in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        c = compat(expr) if expr != "(no licence field)" else "unknown"
        if c not in ("yes",):
            flagged += [(crate_label(v), expr, VERDICT[c]) for v in vs]
        names = ", ".join(crate_label(v) for v in sorted(vs, key=lambda v: (v["name"], v["version"])))
        lines.append(f"| `{expr}` | {VERDICT[c]} | {len(vs)}: {names} |")
    return lines, flagged


# --------------------------------------------------------------------------------------
# npm
# --------------------------------------------------------------------------------------
lock = json.load(open(os.path.join(ROOT, "app/ui/package-lock.json"), encoding="utf-8"))
npm_prod, npm_dev = [], []
for key, v in sorted(lock.get("packages", {}).items()):
    if not key:
        continue
    name = key.split("node_modules/")[-1]
    row = {"name": name, "version": v.get("version", "?"), "license": v.get("license") or "",
           "optional": bool(v.get("optional"))}
    (npm_dev if (v.get("dev") or v.get("devOptional")) else npm_prod).append(row)

# --------------------------------------------------------------------------------------
# Python (staged Studio runtime)
# --------------------------------------------------------------------------------------
TEXTMAP = {
    "mit": "MIT", "mit license": "MIT", "the mit license": "MIT",
    "apache 2.0": "Apache-2.0", "apache-2.0": "Apache-2.0", "apache 2.0 license": "Apache-2.0",
    "apache license 2.0": "Apache-2.0", "apache software license": "Apache-2.0",
    "bsd 3-clause license": "BSD-3-Clause", "3-clause bsd license": "BSD-3-Clause",
    "bsd-3-clause": "BSD-3-Clause", "new bsd license": "BSD-3-Clause",
    "bsd-2-clause": "BSD-2-Clause", "isc": "ISC", "isc license": "ISC",
    "isc license (iscl)": "ISC", "mpl-2.0": "MPL-2.0", "mozilla public license 2.0 (mpl 2.0)": "MPL-2.0",
    "psf-2.0": "PSF-2.0", "python software foundation license": "PSF-2.0",
    "mit-0": "MIT-0", "mit no attribution license (mit-0)": "MIT-0",
    "other/proprietary license": "LicenseRef-Proprietary",
}
CLASSIFIER_GENERIC = {"bsd license", "bsd"}


def text_licence(txt):
    """The licence whose marker comes FIRST in the text: licence files and License fields
    often append the texts of bundled third-party code after the package's own licence."""
    t = re.sub(r"\s+", " ", txt)
    found = []

    def at(pattern, spdx, flags=0):
        m = re.search(pattern, t, flags)
        if m:
            found.append((m.start(), spdx if isinstance(spdx, str) else spdx(m)))

    if "NVIDIA" in t[:3000]:
        at(r"LICENSE AGREEMENT|SOFTWARE LICENSE", "LicenseRef-NVIDIA-Proprietary", re.I)
    at(r"Apache License,? Version 2\.0|Apache License\s+Version 2\.0", "Apache-2.0")
    at(r"Mozilla Public License,? [Vv]ersion 2\.0", "MPL-2.0")
    at(r"GNU LESSER GENERAL PUBLIC LICENSE.{0,80}?Version (\d)", lambda m: "LGPL-3.0-or-later" if m.group(1) == "3" else "LGPL-2.1-or-later", re.I)
    at(r"GNU GENERAL PUBLIC LICENSE.{0,80}?Version (\d)", lambda m: "GPL-3.0-or-later" if m.group(1) == "3" else "GPL-2.0-or-later", re.I)
    at(r"Permission is hereby granted, free of charge", "MIT")
    at(r"Redistribution and use in source and binary forms",
       lambda m: "BSD-3-Clause" if re.search(r"Neither the name|names of (its|the) contributors|name of the copyright holder",
                                             t[m.start():m.start() + 2500]) else "BSD-2-Clause")
    at(r"Permission to use, copy, modify, and(/or)? distribute", "ISC")
    return min(found)[1] if found else ""


def is_spdx(expr):
    ids = spdx_ids(expr)
    return bool(ids) and all(i in YES | REDIST | NO or i.startswith("LicenseRef-") for i in ids)


def python_packages(site):
    rows = []
    for d in sorted(os.listdir(site), key=str.lower):
        if not d.endswith(".dist-info"):
            continue
        dp = os.path.join(site, d)
        with open(os.path.join(dp, "METADATA"), encoding="utf-8", errors="replace") as f:
            head = f.read().split("\n\n", 1)[0]
        msg = email.parser.Parser().parsestr(head, headersonly=True)
        lic_files = []
        for root, _dirs, files in os.walk(dp):
            for fn in files:
                rel = os.path.relpath(os.path.join(root, fn), dp).replace("\\", "/")
                if rel.startswith("licenses/") or re.match(r"(LICEN[CS]E|COPYING|NOTICE)", fn, re.I):
                    lic_files.append(rel)
        file_text = ""   # the package's own licence file first, bundled/third-party ones last
        for rel in sorted(lic_files, key=lambda r: (bool(re.search(r"third|bundled|vendor", r, re.I)),
                                                    r.count("/") - r.startswith("licenses/"), r.lower())):
            try:
                with open(os.path.join(dp, rel), encoding="utf-8", errors="replace") as f:
                    file_text += f.read(20000) + "\n"
            except OSError:
                pass
        le = (msg.get("License-Expression") or "").strip()
        lf = (msg.get("License") or "").strip()
        cls = [c.split("::")[-1].strip() for c in (msg.get_all("Classifier") or []) if c.startswith("License ::")]
        spdx, how = "", ""
        if le:
            spdx, how = le, "License-Expression"
        elif lf and len(lf) < 80 and (lf.lower() in TEXTMAP or is_spdx(lf)):
            spdx, how = TEXTMAP.get(lf.lower(), lf), "License field"
        elif lf and len(lf) >= 80 and text_licence(lf):
            spdx, how = text_licence(lf), "License field (full text)"
        elif cls and all(c.lower() in TEXTMAP for c in cls):
            spdx, how = " AND ".join(sorted({TEXTMAP[c.lower()] for c in cls})), "classifier"
        if (not spdx or spdx.lower() in CLASSIFIER_GENERIC or (lf.lower() in CLASSIFIER_GENERIC and how == "License field")) and file_text:
            spdx, how = text_licence(file_text) or spdx, "licence file text"
        if not spdx and cls:
            if any(c.lower() in CLASSIFIER_GENERIC for c in cls) and file_text:
                spdx, how = text_licence(file_text), "licence file text"
        rows.append({"name": msg.get("Name", d), "version": msg.get("Version", "?"),
                     "spdx": spdx or "unknown", "how": how or "none found",
                     "declared": (le or lf or ", ".join(cls))[:60].replace("\n", " ").replace("|", "/"),
                     "files": len(lic_files)})
    return rows


py_rows, bin_rows, rt_info = None, None, {}
if RUNTIME:
    site = os.path.join(RUNTIME, "python", "Lib", "site-packages")
    if not os.path.isdir(site):
        sys.exit(f"gen-third-party: {site} not found")
    py_rows = python_packages(site)
    with open(os.path.join(RUNTIME, "runtime-binaries.tsv"), encoding="utf-8", newline="") as f:
        bin_rows = list(csv.DictReader(f, delimiter="\t"))
    try:
        rt = json.load(open(os.path.join(RUNTIME, "runtime.json"), encoding="utf-8"))
        rt_info = {"built": rt.get("built_utc", "?"), "python": rt.get("python", "?"),
                   "pbs": rt.get("python_build_standalone", "?"),
                   "lock": (rt.get("lock") or {}).get("sha256", "?")[:12]}
    except (OSError, ValueError):
        rt_info = {}
else:
    warnings.append("no staged Studio runtime found: the Python and bundled-binary sections "
                    "of the existing file are kept as they were (use --studio-runtime DIR)")


# --------------------------------------------------------------------------------------
# Hand-maintained facts that are read from the repository
# --------------------------------------------------------------------------------------
def read(rel):
    try:
        with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
            return f.read()
    except OSError:
        return ""


vend = read("crates/jk-rubberband/vendor/VENDORED.md")
rb_tag = (re.search(r"Release tag \| `([^`]+)`", vend) or [None, "v4.0.0"])[1]
rb_commit = (re.search(r"Commit \| `([0-9a-f]+)`", vend) or [None, "?"])[1]
fonts = []
for key, label in (("oswald", "Oswald (variable)"), ("atkinson-hyperlegible-next", "Atkinson Hyperlegible Next (variable)"),
                   ("pacifico", "Pacifico Regular")):
    src = read(f"app/ui/src/assets/fonts/{key}/SOURCE.txt")
    up = (re.search(r"Upstream:\s+(\S+)\s+\(commit ([0-9a-f]+)\)", src) or [None, "?", "?"])
    sha = (re.search(r"SHA-256:\s+([0-9a-f]+)", src) or [None, "?"])[1]
    fonts.append((label, key, up[1], up[2][:8], sha[:16]))


def keep_block(name):
    old = read(os.path.relpath(OUT, ROOT)) if os.path.exists(OUT) else ""
    m = re.search(rf"<!-- BEGIN {name} -->\n(.*?)<!-- END {name} -->\n", old, re.S)
    return m.group(1) if m else f"_Not generated yet: run scripts/gen-third-party.sh with --studio-runtime._\n\n"


# --------------------------------------------------------------------------------------
# Write
# --------------------------------------------------------------------------------------
L = []
w = L.append
all_flagged = []

w("# Third-party licences")
w("")
w("<!-- Generated by scripts/gen-third-party.sh. Do not edit by hand: edit the script and run it again. -->")
w("")
w("Jukeoke itself is licensed under the **GNU General Public License, version 3 or (at your "
  "option) any later version** (`GPL-3.0-or-later`, see `LICENSE`). This file lists everything "
  "from other people that ends up in what we ship, with its licence, and says whether that "
  "licence lets it be distributed as part of a GPL-3.0-or-later program. It is our reading of "
  "the published licences, not legal advice. Machine-learning model weights are not shipped "
  "and are listed in [`MODELS.md`](MODELS.md).")
w("")
if rt_info:
    rt_phrase = (f"Studio runtime built {rt_info.get('built')} (CPython {rt_info.get('python')}, "
                 f"python-build-standalone {rt_info.get('pbs')}, `studio/uv.lock` sha256 {rt_info.get('lock')})")
else:  # sections 4 and 5 are kept from the previous run, so is their input
    prev = read(os.path.relpath(OUT, ROOT)) if os.path.exists(OUT) else ""
    m = re.search(r"(Studio runtime built [^\n]*?\))\.\n", prev)
    rt_phrase = m.group(1) if m else "Studio runtime: not read yet"
w("Inputs of this run: `Cargo.lock` sha256 " + sha12(os.path.join(ROOT, "Cargo.lock")) +
  ", `app/ui/package-lock.json` sha256 " + sha12(os.path.join(ROOT, "app/ui/package-lock.json")) +
  ", " + rt_phrase + ".")
w("")
w("Contents: 1 Overall statement · 2 Rust crates · 3 npm packages · 4 Python packages (Studio) · "
  "5 Bundled native binaries (Studio) · 6 Vendored and in-repository third-party material · "
  "7 Fonts · 8 Model weights · 9 Toolchain runtimes")
w("")

# ---- Rust
r_lines, r_flag = rust_section(rust_linked, "")
b_lines, b_flag = rust_section(rust_build, "")
all_flagged += [("Rust crate", *f) for f in r_flag + b_flag]
n_win = len(rw_l); n_mac = len(rm_l)
rust_ids = {i for v in list(rust_linked.values()) + list(rust_build.values()) for i in spdx_ids(v["license"])}

# ---- npm
npm_flag = [(f"{p['name']} {p['version']}", p["license"], VERDICT[compat(p["license"])])
            for p in npm_prod if compat(p["license"]) != "yes"]
all_flagged += [("npm package (UI bundle)", *f) for f in npm_flag]

# ---- Python
py_flag = []
if py_rows is not None:
    for r in py_rows:
        c = compat(r["spdx"])
        if c != "yes":
            py_flag.append((f"{r['name']} {r['version']}", r["spdx"], VERDICT[c]))
else:  # section 4 is kept from the previous run, so are its flags
    prev = read(os.path.relpath(OUT, ROOT)) if os.path.exists(OUT) else ""
    py_flag = re.findall(r"^\| Python package \(Studio\) \| (.+?) \| `(.+?)` \| (.+?) \|$", prev, re.M)
all_flagged += [("Python package (Studio)", *f) for f in py_flag]

# ---- 1. Overall statement
w("## 1. Overall statement")
w("")
w("**The app (player, recorder, key change, on Windows and macOS) is distributable under "
  "GPL-3.0-or-later.** Every Rust crate linked into it, both npm packages in the UI bundle, "
  "the vendored Rubber Band library (GPL-2.0-or-later) and the three fonts (SIL OFL 1.1, "
  "bundled as separate font files, which the OFL allows) are GPL-3.0-compatible; nothing in the "
  "app is proprietary. The app links no Microsoft WebView2Loader.dll (our own GPL loader "
  "replaces it) and depends only on DLLs that ship with Windows 11.")
w("")
w("**The Windows Studio runtime is distributable, but not all of it is open source.** Its "
  "open-source parts are all GPL-3.0-compatible (permissive, LGPL, MPL-2.0, GPL-2.0-or-later). "
  "It also bundles binaries whose licences allow redistribution but are not open source: the "
  "NVIDIA CUDA libraries (about 2.7 GB: cuBLAS, cuDNN, cuFFT, NVRTC, ...; NVIDIA CUDA Toolkit "
  "EULA and cuDNN licence), the Microsoft Visual C++ runtime DLLs (Visual Studio "
  "'Distributable Code' terms; also bundled by ctranslate2 and other wheels) and the Intel "
  "OpenMP runtime (Intel Simplified Software License). Say so plainly wherever the Studio is "
  "offered for download.")
w("")
w("**Needs the owner's attention before a public release:**")
w("")
w("1. **NVIDIA libraries and the GPL in one process.** The Studio process runs our GPL code "
  "(`jukeoke_studio`) and two pieces of third-party GPL code (`mutagen`, GPL-2.0-or-later, used to "
  "read lyrics embedded in tags; x264 and x265, GPL-2.0-or-later, linked into the FFmpeg that "
  "the `av` wheel bundles, although the Studio never encodes video) together with the NVIDIA "
  "libraries. Whether GPU runtime libraries count as the GPL's 'System Libraries' is not settled. "
  "Recommended: (a) as the copyright holder of Jukeoke's own code, add a GPL-3.0 section 7 "
  "additional permission, for example: *\"As an additional permission under GNU GPL version 3 "
  "section 7, you may combine this program with the NVIDIA CUDA, cuBLAS, cuDNN and related "
  "runtime libraries, and the Microsoft Visual C++ and Intel OpenMP runtime libraries, and "
  "convey the result; the corresponding source need not include those libraries.\"*; and (b) "
  "remove the third-party GPL code from the Studio process, which no such permission covers: "
  "read embedded lyrics with PyAV (already the fallback for Matroska/WebM/AVI) instead of "
  "`mutagen`, and use an FFmpeg/PyAV build without x264/x265 (libavcodec imports them, so the "
  "two DLLs cannot simply be deleted). Neither change has been made or measured.")
w("2. **Model weights are not covered by this file.** None are shipped in the repository or the "
  "app; the Studio downloads them when the user asks. The default `full` preset uses GPL-3.0 and "
  "Apache-2.0 weights, alignment Apache-2.0, transcription MIT, so a stick prepared with the "
  "defaults carries only redistributable weights. The `clean` preset's instrumental model "
  "(`bs_inst_resurrection_unwa`) and the 'best measured' set have **no published licence**: they "
  "may be downloaded by a user for themselves, but must not be put on a stick that is given to "
  "others, and commercial use is unclear (`MODELS.md`).")
w("3. **Licence texts travel with the binaries.** MIT/BSD/Apache-2.0 require their notices "
  "in binary distributions. `third_party_licenses/` (generated by this script, copied to "
  "`Licenses/third-party/` on the stick and into the release archives) holds the licence and "
  "notice files of every linked Rust crate, of the UI bundle's npm packages and of the Studio "
  "packages marked 'none in wheel' in section 4; the other Python wheels keep theirs in their "
  "`*.dist-info` folders, and the fonts' `OFL.txt` files are in `Licenses/fonts/`. What could "
  "not be found is listed in `third_party_licenses/MISSING.txt` (crates whose published package "
  "has no licence file, and native code inside the `tokenizers` and `ctranslate2` wheels whose "
  "own notices those wheels do not carry).")
w("4. **`lameenc` (LAME, LGPL-3.0-or-later)** is installed only because `demucs` declares it. "
  "Nothing imports it: `demucs.audio` (the only importer) is never loaded by the Studio "
  "(checked: importing `demucs.apply`, `demucs.states` and the Studio's modules leaves "
  "`lameenc` out of `sys.modules`), and the Studio writes only FLAC and float WAV. It is "
  "harmless (LGPL) and could be deleted from the runtime (0.2 MB).")
if all_flagged:
    w("5. **Flagged by this run** (not plainly GPL-3.0-compatible):")
    w("")
    w("| where | item | licence | verdict |")
    w("|---|---|---|---|")
    for where, item, expr, verdict in all_flagged:
        w(f"| {where} | {item} | `{expr}` | {verdict} |")
else:
    w("5. **Flagged by this run:** nothing (every licence expression read was GPL-3.0-compatible "
      "or a known redistributable binary licence).")
w("")

# ---- 2. Rust
w("## 2. Rust crates")
w("")
w("From `cargo metadata --locked`, filtered for `x86_64-pc-windows-gnu` and "
  "`aarch64-apple-darwin`, walking the dependencies of the shipped binaries: " +
  ", ".join(SHIPPED.values()) + ". Dev-dependencies (tests only) are not included. A crate "
  "marked (Windows) or (macOS) is only in that target's graph.")
w("")
w(f"### 2.1 Linked into the shipped binaries: {len(rust_linked)} crates "
  f"({n_win} for Windows, {n_mac} for macOS, including our 7 workspace crates)")
w("")
L.extend(r_lines)
w("")
w(f"### 2.2 Build time only (build scripts, procedural macros and their dependencies): {len(rust_build)} crates")
w("")
w("Not linked into the binaries (procedural macros generate code at compile time).")
w("")
L.extend(b_lines)
w("")
for i in sorted(rust_ids):
    if i in NOTES:
        w(f"- {NOTES[i]}")
w("- Our workspace crates (`GPL-3.0-or-later`): jukeoke, jk-cli, jk-dsp, jk-engine, jk-format, "
  "jk-recorder, jk-rubberband. jk-rubberband compiles the vendored Rubber Band C++ sources "
  "(section 6); the app crate compiles our WebView2 loader (section 6).")
w("")

# ---- 3. npm
w("## 3. npm packages")
w("")
w(f"### 3.1 In the shipped UI bundle (production dependencies): {len(npm_prod)}")
w("")
w("| package | version | licence | GPL-3.0-or-later compatible |")
w("|---|---|---|---|")
for p in npm_prod:
    w(f"| {p['name']} | {p['version']} | `{p['license']}` | {VERDICT[compat(p['license'])]} |")
w("")
w("The bundle also contains a few lines of Vite's module-preload helper (MIT), emitted by the "
  "build. Everything else in `app/ui/dist` is our own code (GPL-3.0-or-later) and the fonts "
  "(section 7).")
w("")
w(f"### 3.2 Development tooling only (not shipped): {len(npm_dev)} packages")
w("")
w("Vite, TypeScript, Vitest, happy-dom, playwright-core, the Tauri CLI, Babel and their "
  "dependencies, used to build and test the UI. Platform-specific optional binaries are counted "
  "even when they are not installed on this machine.")
w("")
dg = collections.defaultdict(list)
for p in npm_dev:
    dg[p["license"] or "(none)"].append(p)
w("| licence | packages |")
w("|---|---|")
for lic, ps in sorted(dg.items(), key=lambda kv: (-len(kv[1]), kv[0])):
    w(f"| `{lic}` | {len(ps)}: " + ", ".join(f"{p['name']} {p['version']}" for p in ps) + " |")
w("")
w("`caniuse-lite` (CC-BY-4.0) is browser-support data used by a build check; it is not shipped.")
w("")

# ---- 4. Python
w("## 4. Python packages in the Studio runtime (Windows only)")
w("")
w("<!-- BEGIN python -->")
if py_rows is not None:
    PL = []
    p = PL.append
    p("Read from every `*.dist-info/METADATA` of the staged runtime "
      "(`Windows/Studio/python/Lib/site-packages`), in this order: `License-Expression`, a short "
      "`License` field, the licence classifiers, the text of the licence files. The licence texts "
      "column counts the files the wheel ships in its `*.dist-info` folder (they are on the stick).")
    p("")
    p(f"{len(py_rows)} packages:")
    p("")
    p("| package | version | licence (SPDX) | read from | licence texts | GPL-3.0-or-later compatible |")
    p("|---|---|---|---|---|---|")
    for r in py_rows:
        p(f"| {r['name']} | {r['version']} | `{r['spdx']}` | {r['how']} | "
          f"{r['files'] if r['files'] else 'none in wheel'} | {VERDICT[compat(r['spdx'])]} |")
    p("")
    ids = {i for r in py_rows for i in spdx_ids(r["spdx"])}
    for i in sorted(ids):
        if i in NOTES:
            p(f"- {NOTES[i]}")
    p("- `jukeoke-studio` is our own package. Two of its files are MIT-licensed ports "
      "(section 6).")
    p("- `mutagen` (GPL-2.0-or-later) and the `av` wheel's FFmpeg with x264/x265: see section 1, "
      "item 1. `lameenc`: section 1, item 4.")
    p("- `nvidia-cublas-cu12` (cuBLAS 12 for CTranslate2) and `nvidia-cuda-nvrtc-cu12` are "
      "NVIDIA's own wheels; torch's CUDA 13 libraries are inside the `torch` wheel (section 5).")
    p("")
    block = "\n".join(PL) + "\n"
else:
    block = keep_block("python")
L.append(block.rstrip("\n"))
w("<!-- END python -->")
w("")

# ---- 5. Bundled binaries
w("## 5. Bundled native binaries in the Studio runtime")
w("")
w("<!-- BEGIN binaries -->")
if bin_rows is not None:
    BL = []
    b = BL.append
    tot = sum(int(r["bytes"]) for r in bin_rows)
    b(f"From `Windows/Studio/runtime-binaries.tsv` (written by `scripts/build-studio-runtime.sh`, "
      f"which also ships it on the stick): {len(bin_rows)} `.dll/.pyd/.exe` files, {mib(tot)}. "
      "Third-party components inside the wheels and the interpreter, largest first "
      "(details: docs/STUDIO_RUNTIME.md section 11):")
    b("")
    b("| component | licence | files | size | shipped by |")
    b("|---|---|---|---|---|")
    comp = collections.OrderedDict()
    own_files = own_bytes = 0
    for r in bin_rows:
        if r["component"] == "(package's own code)":
            own_files += 1; own_bytes += int(r["bytes"])
            continue
        k = (r["component"], r["component_licence"])
        e = comp.setdefault(k, {"files": 0, "bytes": 0, "by": set()})
        e["files"] += 1; e["bytes"] += int(r["bytes"]); e["by"].add(r["installed_by"])
    for (c, lic), e in sorted(comp.items(), key=lambda kv: -kv[1]["bytes"]):
        b(f"| {c} | {lic} | {e['files']} | {mib(e['bytes'])} | {', '.join(sorted(e['by']))} |")
    b(f"| (the packages' own compiled extension modules) | the package's licence (section 4) | "
      f"{own_files} | {mib(own_bytes)} | |")
    b("")
    b("Plainly:")
    b("")
    b("- **NVIDIA CUDA libraries** (cuBLAS 12 and 13, cuDNN 9, cuFFT, NVRTC, cuSPARSE, cuSOLVER, "
      "nvJitLink, CUPTI, cudart, NVTX): **redistributable, not open source**. NVIDIA's CUDA "
      "Toolkit EULA (Attachment A lists the redistributable runtime files) and the cuDNN Software "
      "License Agreement allow shipping them with an application, unmodified, with their notices. "
      "Their licence texts are in the `torch` and `nvidia-*` wheels' `*.dist-info` folders.")
    b("- **FFmpeg** (PyAV's wheel build, libavcodec 63): its own code is LGPL-3.0-or-later "
      "(`av_license()`, `--enable-version3`, no `--enable-gpl`), but the same configure line has "
      "`--enable-libx264 --enable-libx265`, and libavcodec imports the bundled x264 and x265 DLLs "
      "(**GPL-2.0-or-later**). As shipped, the combination is therefore GPL; that is compatible "
      "with Jukeoke's GPL-3.0-or-later. LAME is linked into libavcodec statically "
      "(LGPL-2.0-or-later).")
    b("- **Microsoft Visual C++ runtime** (msvcp140, vcruntime140, vcomp140, ...): copied in by the "
      "runtime build and also bundled by wheels (CTranslate2 among them); redistributable under "
      "the Visual Studio 'Distributable Code' terms, not open source. A compiler's runtime is the "
      "GPL's textbook 'System Library'.")
    b("- **Intel OpenMP runtime** (`libiomp5md`, loaded by torch and CTranslate2): Intel "
      "Simplified Software License, redistributable in binary form, not open source.")
    b("- **GCC/MinGW runtime DLLs** inside the `av` wheel: GPL-3.0-or-later WITH "
      "GCC-exception-3.1 (the exception lets any program use them).")
    b("")
    block = "\n".join(BL) + "\n"
else:
    block = keep_block("binaries")
L.append(block.rstrip("\n"))
w("<!-- END binaries -->")
w("")

# ---- 6. Vendored / in-repo
w("## 6. Vendored and in-repository third-party material")
w("")
w("| what | where | licence | shipped | notes |")
w("|---|---|---|---|---|")
w(f"| Rubber Band Library {rb_tag.lstrip('v')} (Chris Cannam / Particular Programs Ltd), commit "
  f"`{rb_commit[:12]}` | `crates/jk-rubberband/vendor/rubberband/` | GPL-2.0-or-later "
  "(`vendor/rubberband/COPYING`) | yes, compiled into the app on both platforms | used under "
  "GPL-3.0-or-later; unmodified; details in `crates/jk-rubberband/vendor/VENDORED.md`. The "
  "reason Jukeoke is GPL (WORK_ORDER section 10). |")
w("| WebView2 loader | `app/src-tauri/native/webview2_loader.c` | GPL-3.0-or-later (ours) | yes, "
  "in `Jukeoke.exe` | an independent implementation of the five loader functions, following the "
  "approach of the MIT-licensed webview/webview project; replaces Microsoft's "
  "`WebView2Loader.dll`, which is not shipped (`tauri-build` copies it into `target/`; the dist "
  "must not pick it up). |")
w("| RoFormer network code | `studio/jukeoke_studio/modelzoo/roformer.py` | MIT (file header) | "
  "yes, in the Studio runtime | derived from lucidrains/BS-RoFormer (MIT, (c) 2023 Phil Wang) and "
  "ZFTurbo/Music-Source-Separation-Training (MIT, (c) 2024 Roman Solovyev). |")
w("| SCNet network code | `studio/jukeoke_studio/modelzoo/scnet.py` | MIT (file header) | yes, in "
  "the Studio runtime | derived from starrytong/SCNet (MIT) and ZFTurbo/Music-Source-Separation-"
  "Training (MIT). |")
w("| `ccmixter_voices.json` | `studio/bench/data/` | MIT ((c) 2025 Vladlen Popelenkov, "
  "musetric/lab), text in `ccmixter_voices.LICENSE.md` | no (benchmark input only) | a list of "
  "ccMixter uploads; the Creative Commons audio itself is never committed. |")
w("| Demo song \"Twinkle, Twinkle, Little Star\" | `tests/fixtures/generated/`, "
  "`app/src-tauri/resources/` | words public domain (Jane Taylor, 1806); audio generated by "
  "`jukeoke-cli make-demo-song` (ours) | yes (bundled resource) | |")
w("")
w("Not shipped and not in the repository: the reference photos, test songs, evaluation datasets "
  "(MUSDB18-HQ, JamendoLyrics, ccMixter audio) and model weights.")
w("")

# ---- 7. Fonts
w("## 7. Fonts")
w("")
w("Bundled in the UI (embedded in the app) and never loaded from the network. All three are "
  "under the **SIL Open Font License 1.1** with no Reserved Font Name, byte-identical to the "
  "upstream webfonts (not subset or modified). The OFL allows bundling fonts with any software, "
  "including GPL software, as long as the fonts are not sold on their own and the licence text "
  "accompanies them (`OFL.txt` beside each font).")
w("")
w("| font | folder | upstream | commit | SHA-256 (first 16) |")
w("|---|---|---|---|---|")
for label, key, up, commit, sha in fonts:
    w(f"| {label} | `app/ui/src/assets/fonts/{key}/` | {up} | `{commit}` | `{sha}` |")
w("")

# ---- 8. Models
w("## 8. Model weights")
w("")
w("Not shipped with the app and never committed. The Studio downloads them into `Models/` when "
  "the user asks, checking a pinned SHA-256. Every model, its source, licence and whether "
  "commercial use is allowed: [`MODELS.md`](MODELS.md). Summary: default `full` preset = GPL-3.0 "
  "(anvuew vocals and karaoke) + Apache-2.0 (SYH99999 drums); `permissive` preset = MIT + "
  "Apache-2.0; `clean` preset adds an instrumental model with no published licence; alignment "
  "wav2vec2 Apache-2.0; transcription Whisper large-v3-turbo MIT.")
w("")

# ---- 9. Toolchain runtimes
w("## 9. Toolchain runtimes and system components")
w("")
w("- **Windows app** (`x86_64-pc-windows-gnu`): libgcc, libstdc++ and winpthreads are linked "
  "statically (GPL-3.0-or-later WITH GCC-exception-3.1; winpthreads MIT-style), plus mingw-w64 "
  "start-up code (ZPL-2.1 / public domain, mingw-w64 `COPYING`). `msvcrt.dll`, the WebView2 "
  "Runtime and every other DLL the exe imports are part of Windows (checked by "
  "`app/scripts/check-windows-exe.sh`).")
w("- **macOS app**: links only system frameworks and libSystem; WebKit is the system's.")
w("- **Studio interpreter**: CPython 3.12 from python-build-standalone (PSF-2.0, with OpenSSL 3 "
  "Apache-2.0, SQLite public domain, libffi MIT, zlib Zlib; section 5).")
w("")

text = "\n".join(L).rstrip("\n") + "\n"
text = re.sub(r"\n{3,}", "\n\n", text)


# --------------------------------------------------------------------------------------
# Licence-text folder (third_party_licenses/)
# --------------------------------------------------------------------------------------
LIC_NAME = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)", re.I)
RULE = "=" * 88
THIN = "-" * 88


def read_text(path):
    with open(path, "rb") as f:
        b = f.read()
    t = b.decode("utf-8", errors="replace").lstrip("\ufeff")
    t = t.replace("\r\n", "\n").replace("\r", "\n")
    return "\n".join(line.rstrip() for line in t.split("\n")).strip("\n") + "\n"


# Notice files BELOW a package's top folder that cover code or data compiled into the
# binary (licence_files only looks at the top). regex-syntax's Unicode tables are generated
# from the Unicode data files, whose licence asks for its notice with every copy. (Others
# found by a scan of the linked crates are not compiled for Windows or macOS: rfd's
# xdg_desktop_portal backend, tracing-core's no_std spin module.)
NESTED_NOTICES = {"regex-syntax": ["src/unicode_tables/LICENSE-UNICODE"]}


def licence_files(d, extra="", name=""):
    """Licence/notice files at the top of a package folder (and in a LICENSES/ folder)."""
    out = set()
    for rel in NESTED_NOTICES.get(name, []):
        if os.path.isfile(os.path.join(d, rel)):
            out.add(rel)
    if os.path.isdir(d):
        for fn in os.listdir(d):
            fp = os.path.join(d, fn)
            if LIC_NAME.match(fn) and os.path.isfile(fp):
                out.add(fn)
            if fn.upper() == "LICENSES" and os.path.isdir(fp):
                out.update(f"{fn}/{x}" for x in os.listdir(fp) if os.path.isfile(os.path.join(fp, x)))
        if extra and os.path.isfile(os.path.join(d, extra)):
            out.add(extra.replace("\\", "/"))
    return sorted(out, key=str.lower)


def grouped(entries, title, intro):
    """entries: [(label, filename, text)] -> one document, identical texts printed once."""
    groups = collections.OrderedDict()
    for label, fn, t in sorted(entries, key=lambda e: (e[0].lower(), e[1].lower())):
        groups.setdefault(t, []).append(f"{label} ({fn})")
    order = sorted(groups.items(), key=lambda kv: kv[1][0].lower())
    out = [title, "=" * len(title), "", intro, "",
           f"{len({e[0] for e in entries})} components, {len(order)} distinct texts.", ""]
    for i, (t, users) in enumerate(order, 1):
        out += [RULE, f"Text {i} of {len(order)}, used by: " + ", ".join(users), THIN, "", t.rstrip("\n"), ""]
    return "\n".join(out).rstrip("\n") + "\n"


def build_texts():
    files, missing, errors = collections.OrderedDict(), [], []
    apache = read_text(os.path.join(ROOT, "scripts", "dist", "licenses", "Apache-2.0.txt"))

    # Rust: every crate linked into the shipped binaries, except our workspace crates
    rust_entries, mpl_missing = [], []
    for v in sorted(rust_linked.values(), key=lambda v: (v["name"], v["version"])):
        if v["workspace"]:
            continue
        label = crate_label(v)
        if not os.path.isdir(v["dir"]):
            errors.append(f"{label}: sources not found in the cargo registry (run `cargo fetch --locked`)")
            continue
        fns = licence_files(v["dir"], v["license_path"], v["name"])
        if fns:
            for fn in fns:
                rust_entries.append((label, fn, read_text(os.path.join(v["dir"], fn))))
            continue
        ids = spdx_ids(v["license"])
        who = ", ".join(v["authors"]) or "no authors listed"
        repo = v["repository"] or "no repository listed"
        if "Zlib" in ids:
            missing.append(f"{label}: `{v['license']}`; the published package has no licence file. "
                           f"Used under Zlib, which requires no notice in binary distributions. "
                           f"Authors: {who}; source: {repo}.")
        elif "Apache-2.0" in ids:
            rust_entries.append((label, "no licence file in the package; Apache-2.0 chosen from "
                                 f"`{v['license']}`, standard text", apache))
            missing.append(f"{label}: `{v['license']}`; the published package has no licence file. "
                           f"Used under Apache-2.0, whose standard text is in rust-crates.txt "
                           f"(the package has no NOTICE file either). Authors: {who}; source: {repo}.")
        elif ids == ["MPL-2.0"]:
            mpl_missing.append(label)
            missing.append(f"{label}: `MPL-2.0`; the published package has no licence file. MPL-2.0 "
                           f"needs no copyright notice in a binary, but a copy of the licence and where "
                           f"the source is: the standard MPL-2.0 text other crates ship is listed for it "
                           f"in rust-crates.txt; the source is the crates.io package ({repo}).")
        else:
            missing.append(f"{label}: `{v['license']}`; the published package has NO licence file, so "
                           f"its copyright notice is not reproduced here. Authors (Cargo.toml): {who}; "
                           f"source and licence text: {repo}. TO DO before a public release: copy the "
                           f"notice from the repository at this version.")
    mpl = [t for _, _, t in rust_entries if t.startswith("Mozilla Public License Version 2.0")]
    for label in mpl_missing:
        if mpl:
            rust_entries.append((label, "no licence file in the package; the standard MPL-2.0 text", mpl[0]))
        else:
            errors.append(f"{label}: MPL-2.0 without a licence file, and no other crate ships the MPL-2.0 text")
    files["rust-crates.txt"] = grouped(
        rust_entries, "Licence and notice texts: Rust crates",
        "Every Rust crate linked into the shipped binaries (the app, jukeoke-cli, sck_probe; "
        "THIRD_PARTY_LICENSES.md section 2.1), with the licence and notice files of its published "
        "package exactly as they are in the cargo registry. Our own crates are GPL-3.0-or-later "
        "(LICENSE). A crate marked (Windows) or (macOS) is only in that build.")

    # npm: the UI bundle's packages and the code the build emits into it
    nm = os.path.join(ROOT, "app", "ui", "node_modules")
    npm_entries = []
    if not os.path.isdir(nm):
        errors.append("app/ui/node_modules is missing (run `npm ci` in app/ui)")
    for p in sorted(npm_prod, key=lambda p: p["name"]):
        d = os.path.join(nm, *p["name"].split("/"))
        fns = licence_files(d)
        if not fns:
            (errors if os.path.isdir(nm) else []).append(f"npm {p['name']} {p['version']}: no licence file in {d}")
            continue
        for fn in fns:
            npm_entries.append((f"{p['name']} {p['version']}", fn, read_text(os.path.join(d, fn))))
    for pkg, fn, note in (("vite", "LICENSE.md", "module-preload helper emitted into the bundle; Vite's own licence only"),
                          ("rolldown", "LICENSE", "runtime helpers (__export, __name) emitted into the bundle")):
        fp = os.path.join(nm, pkg, fn)
        if not os.path.isfile(fp):
            if os.path.isdir(nm):
                errors.append(f"npm {pkg}: {fn} not found")
            continue
        t = read_text(fp)
        if pkg == "vite":   # its LICENSE.md appends the licences of Vite's own bundled tools
            t = t.split("# Licenses of bundled dependencies")[0].rstrip("\n") + "\n"
        ver = (json.load(open(os.path.join(nm, pkg, "package.json"), encoding="utf-8")) or {}).get("version", "?")
        npm_entries.append((f"{pkg} {ver}", f"{fn}; {note}", t))
    files["npm-packages.txt"] = grouped(
        npm_entries, "Licence and notice texts: the UI bundle",
        "The npm packages in the shipped UI bundle (THIRD_PARTY_LICENSES.md section 3.1) and the "
        "build tools whose code is emitted into it, from app/ui/node_modules.")

    # Python: the Studio packages whose wheels ship no licence file
    old_py = ""
    if TEXTS and os.path.isfile(os.path.join(TEXTS, "python-packages.txt")):
        old_py = read_text(os.path.join(TEXTS, "python-packages.txt"))
    if py_rows is not None:
        site = os.path.join(RUNTIME, "python", "Lib", "site-packages")
        py_entries = []
        known = {"ctranslate2": ("ctranslate2-{v}-LICENSE.txt", "verbatim from the upstream repository at "
                                 "the release tag (scripts/dist/licenses/SOURCES.txt)"),
                 "flatbuffers": ("Apache-2.0.txt", "the standard Apache License 2.0 text that the "
                                 "package declares; the upstream file (google/flatbuffers, LICENSE at "
                                 "the release tag) was not fetched or compared"),
                 "tokenizers": ("Apache-2.0.txt", "the standard Apache License 2.0 text that the "
                                "package declares; the upstream file (huggingface/tokenizers, LICENSE "
                                "at the release tag) was not fetched or compared")}
        for r in sorted(py_rows, key=lambda r: r["name"].lower()):
            if r["files"] or r["name"] == "jukeoke-studio":
                continue
            key = r["name"].lower()
            label = f"{r['name']} {r['version']}"
            pkg_dir = os.path.join(site, key.replace("-", "_"))
            in_pkg = licence_files(pkg_dir)
            if key in known:
                fn, how = known[key]
                fp = os.path.join(ROOT, "scripts", "dist", "licenses", fn.format(v=r["version"]))
                if os.path.isfile(fp):
                    py_entries.append((label, how, read_text(fp)))
                else:
                    errors.append(f"{label}: {fp} is missing (add the upstream licence text)")
            elif in_pkg:
                for f2 in in_pkg:
                    py_entries.append((label, f"{f2}, from the package folder", read_text(os.path.join(pkg_dir, f2))))
            else:
                missing.append(f"Python {label}: the wheel ships no licence file (declared `{r['spdx']}`). "
                               "TO DO: add its upstream licence text to scripts/dist/licenses/.")
        py_doc = grouped(
            py_entries, "Licence texts: Studio Python packages whose wheels ship none",
            "Every other package in the Studio runtime carries its licence files in "
            "Windows/Studio/python/Lib/site-packages/<package>.dist-info/. onnxruntime's "
            "ThirdPartyNotices.txt stays in site-packages/onnxruntime/.")
        files["python-packages.txt"] = py_doc
    elif old_py:
        files["python-packages.txt"] = old_py
        warnings.append("no Studio runtime: third_party_licenses/python-packages.txt kept as it was")
    else:
        errors.append("no Studio runtime and no previous python-packages.txt (use --studio-runtime DIR)")
    missing += [
        "Python tokenizers: tokenizers.pyd is compiled Rust; the crates statically linked into it are "
        "not listed in the wheel and their notices are not carried by it (upstream ships none either).",
        "Python ctranslate2: ctranslate2.dll links third-party C++ libraries; the wheel carries no notices "
        "for them. The upstream repository lists them under third_party/.",
    ]

    # Vendored
    cp = os.path.join(ROOT, "crates", "jk-rubberband", "vendor", "rubberband", "COPYING")
    if os.path.isfile(cp):
        files["vendored.txt"] = grouped(
            [(f"Rubber Band Library {rb_tag.lstrip('v')}", "COPYING", read_text(cp))],
            "Licence texts: vendored code",
            "Rubber Band Library, compiled into the app on both platforms, is GPL-2.0-or-later and "
            "used under GPL-3.0-or-later (LICENSE). Its own COPYING file follows.")
    else:
        errors.append("crates/jk-rubberband/vendor/rubberband/COPYING is missing")

    m = ["Licence texts that could not be found", "======================================", "",
         "Components whose published package carries no licence or notice file, what their licence "
         "requires in a binary distribution, and what this project relies on instead. Generated by "
         "scripts/gen-third-party.sh.", ""]
    m += [f"- {x}" for x in missing] if missing else ["- nothing"]
    files["MISSING.txt"] = "\n".join(m) + "\n"
    files["README.txt"] = "\n".join([
        "Third-party licence and notice texts", "====================================", "",
        "Generated by scripts/gen-third-party.sh from Cargo.lock, app/ui/package-lock.json and the "
        "Studio runtime; do not edit by hand. MIT, BSD and Apache-2.0 require these notices to "
        "travel with the binaries. Which component uses which licence, and whether it is "
        "compatible with GPL-3.0-or-later: THIRD_PARTY_LICENSES.md.", "",
        "  rust-crates.txt      every Rust crate linked into the app, jukeoke-cli or sck_probe",
        "  npm-packages.txt     the npm packages in the UI bundle and the code the build emits",
        "  python-packages.txt  Studio packages whose wheels ship no licence file",
        "  vendored.txt         Rubber Band's COPYING",
        "  MISSING.txt          what could not be found, and what is relied on instead", "",
        "The fonts' SIL Open Font License texts are in Licenses/fonts/ on the stick "
        "(app/ui/src/assets/fonts/*/OFL.txt in the repository).", ""])
    return files, missing, errors


texts_out_of_date = []
if TEXTS:
    tfiles, tmissing, terrors = build_texts()
    for e in terrors:
        print("gen-third-party: ERROR: " + e, file=sys.stderr)
    if terrors:
        sys.exit("gen-third-party: the licence-text folder cannot be complete (errors above)")
    existing = set()
    if os.path.isdir(TEXTS):
        existing = {f for f in os.listdir(TEXTS) if os.path.isfile(os.path.join(TEXTS, f))}
    for name, content in tfiles.items():
        fp = os.path.join(TEXTS, name)
        cur = None
        if os.path.isfile(fp):
            with open(fp, encoding="utf-8", newline="") as f:
                cur = f.read()
        if cur != content:
            texts_out_of_date.append(name)
    texts_out_of_date += sorted(existing - set(tfiles))

for msg in warnings:
    print("gen-third-party: WARNING: " + msg, file=sys.stderr)
old = read(os.path.relpath(OUT, ROOT)) if os.path.exists(OUT) else None
if CHECK:
    bad = False
    if old != text:
        print(f"gen-third-party: {os.path.basename(OUT)} is out of date; run scripts/gen-third-party.sh", file=sys.stderr)
        bad = True
    if texts_out_of_date:
        print(f"gen-third-party: {os.path.basename(TEXTS)}/ is out of date ({', '.join(texts_out_of_date)}); "
              "run scripts/gen-third-party.sh", file=sys.stderr)
        bad = True
    if bad:
        sys.exit(1)
    print(f"gen-third-party: {os.path.basename(OUT)} is up to date" +
          (f"; {os.path.basename(TEXTS)}/ is up to date ({len(tfiles)} files)" if TEXTS else ""))
    sys.exit(0)
tmp = OUT + ".tmp"
with open(tmp, "w", encoding="utf-8", newline="\n") as f:
    f.write(text)
os.replace(tmp, OUT)
if TEXTS:
    os.makedirs(TEXTS, exist_ok=True)
    for name, content in tfiles.items():
        tp = os.path.join(TEXTS, name + ".tmp")
        with open(tp, "w", encoding="utf-8", newline="\n") as f:
            f.write(content)
        os.replace(tp, os.path.join(TEXTS, name))
    for name in os.listdir(TEXTS):
        if name not in tfiles and os.path.isfile(os.path.join(TEXTS, name)):
            os.remove(os.path.join(TEXTS, name))
    print(f"gen-third-party: wrote {os.path.basename(TEXTS)}/: " +
          ", ".join(f"{n} ({len(c) // 1024} KB)" for n, c in tfiles.items()) +
          f"; {len(tmissing)} item(s) in MISSING.txt")
print(f"gen-third-party: wrote {os.path.basename(OUT)}: Rust {len(rust_linked)} linked + "
      f"{len(rust_build)} build-time, npm {len(npm_prod)} shipped + {len(npm_dev)} dev, Python "
      f"{len(py_rows) if py_rows is not None else 'kept'}, binaries "
      f"{len(bin_rows) if bin_rows is not None else 'kept'}; flagged {len(all_flagged)}")
PYEOF

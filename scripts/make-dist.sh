#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# make-dist.sh - build the ready-to-copy dist/ folder (WORK_ORDER 4.8 and 13).
#
#   bash scripts/make-dist.sh [--force-clean] [--out DIR]
#
#   --force-clean  delete anything in the dist folder this script did not put there
#                  (songs, recordings, settings, a WebView2 profile left by running the app
#                  from dist/). Without it the script lists such files and stops.
#   --out DIR      dist folder (default: dist/ in the repository, which is gitignored)
#
# Builds everything from this checkout and lays it out like the USB stick:
#
#   dist/START-HERE.txt          what each folder is, copying to an exFAT stick, first
#                                launch on Windows and on the Mac, the self-check report
#   dist/Windows/<name>.exe      release app (app/scripts/build-windows.sh: UI build and tests,
#   dist/Windows/resources/      cargo build --release, strip, demo song resource, DLL check,
#                                no build-machine path in the exe)
#   dist/Windows/Studio/         relocatable Studio runtime from the CURRENT studio/
#                                (scripts/build-studio-runtime.sh), then
#                                scripts/check-studio-runtime.sh --static-only, which must
#                                pass and must not call the runtime stale
#   dist/Mac/README.txt          how to get the Mac app (built on the Mac or by CI); an
#                                existing Mac/<name>.app and Mac/Tools are kept as they are
#   dist/Library/                the demo song only
#   dist/Recordings/README.txt
#   dist/Models/                 the Studio's default models (preset full + alignment +
#                                transcription) copied from $JK_BUILD/models-cache and
#                                verified by the Studio's own code (scripts/dist/dist_models.py):
#                                manifest.json, SHA256SUMS.txt, README.txt
#   dist/Settings/README.txt     nothing else: no machine state, no WebView2 profile
#   dist/Licenses/               LICENSE, THIRD_PARTY_LICENSES.md, MODELS.md, font licences,
#                                licence texts of Python packages that ship none, and
#                                third-party/: the licence and notice files of every linked
#                                Rust crate and bundled npm package (scripts/gen-third-party.sh)
#
# Re-runnable: the app and the runtime are rebuilt every time; model files already in place
# (same size and time) are not copied again but always re-hashed. Logs, the runtime check
# and the size report go to $JK_BUILD/dist-build/<time>/. Needs: scripts/devenv.sh
# (cargo, gcc, node/npm, uv), the interpreter tree in $JK_BUILD/python, the model cache.
# CARGO_TARGET_DIR defaults to $JK_BUILD/target-dist. Nothing is downloaded except what
# npm ci / uv need when their caches are cold.

set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO/scripts/devenv.sh"

OUT="$REPO/dist"
FORCE_CLEAN=0
while [ $# -gt 0 ]; do
    case "$1" in
        --force-clean) FORCE_CLEAN=1 ;;
        --out) OUT="$2"; shift ;;
        -h|--help) sed -n '4,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$JK_BUILD/target-dist}"
PY="${JK_PBS_DIR:-$JK_BUILD/python/cpython-3.12.16-windows-x86_64-none}/python.exe"
CACHE="${JK_MODELS_CACHE:-$JK_BUILD/models-cache}"
LOG="$JK_BUILD/dist-build/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$LOG" "$OUT"
OUT="$(cd "$OUT" && pwd)"
pyw() { cygpath -w "$1"; }
NAME="$("$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["name"])' "$(pyw "$REPO/branding.json")" | tr -d '\r')"
[ -n "$NAME" ] || { echo "no name in branding.json" >&2; exit 1; }
DEMO="$REPO/tests/fixtures/generated/demo-song.jksong"
STEPS=()
step() { echo; echo "==== $* ($(date +%H:%M:%S))"; STEP_T=$SECONDS; }
done_step() { STEPS+=("$1=$((SECONDS - STEP_T))s"); echo "---- $1 done in $((SECONDS - STEP_T)) s"; }

echo "dist:    $OUT"
echo "logs:    $LOG"
echo "target:  $CARGO_TARGET_DIR"
echo "name:    $NAME"
[ -x "$PY" ] || { echo "interpreter not found: $PY" >&2; exit 1; }
[ -d "$CACHE" ] || { echo "model cache not found: $CACHE" >&2; exit 1; }
[ -f "$DEMO" ] || { echo "demo song not found: $DEMO" >&2; exit 1; }
if [ "$OUT" = "$REPO/dist" ] && ! git -C "$REPO" check-ignore -q dist/; then
    echo "dist/ is not ignored by git (.gitignore must list /dist/)" >&2; exit 1
fi

# ------------------------------------------------------------------ 0. foreign content
step "0. what is already in $OUT"
"$PY" -I -B "$(pyw "$HERE/dist/dist_models.py")" --src "$(pyw "$CACHE")" --dest "$(pyw "$OUT/Models")" --list \
    > "$LOG/models-owned.txt"
"$PY" -I -B - "$(pyw "$OUT")" "$NAME" "$(pyw "$LOG/models-owned.txt")" "$FORCE_CLEAN" <<'PYEOF'
import os, shutil, sys
out, name, owned_models, force = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
models = set(open(owned_models, encoding="utf-8").read().split())
foreign = []
def rel(p): return os.path.relpath(p, out).replace("\\", "/")
for e in os.listdir(out):
    p = os.path.join(out, e)
    if e == "START-HERE.txt" and os.path.isfile(p):
        continue
    if e in ("Licenses",) and os.path.isdir(p):
        continue                         # regenerated from scratch below
    if e not in ("Windows", "Mac", "Library", "Recordings", "Models", "Settings") or not os.path.isdir(p):
        foreign.append(p); continue
    for c in os.listdir(p):
        cp = os.path.join(p, c)
        ok = {
            "Windows": c in (f"{name}.exe", "resources", "Studio"),
            "Mac": c in ("README.txt", f"{name}.app", "Tools"),   # the owner's Mac build is kept
            "Library": c == "demo-song.jksong",
            "Recordings": c == "README.txt",
            "Settings": c == "README.txt",
            "Models": True,
        }[e]
        if not ok:
            foreign.append(cp)
    if e == "Models":
        for dp, dns, fns in os.walk(p):
            for f in fns:
                if rel(os.path.join(dp, f))[len("Models/"):] not in models:
                    foreign.append(os.path.join(dp, f))
for f in foreign:
    print(("removing " if force else "foreign: ") + rel(f))
    if force:
        shutil.rmtree(f) if os.path.isdir(f) else os.remove(f)
if foreign and not force:
    print(f"\n{len(foreign)} entries in the dist folder were not made by make-dist.sh (songs, recordings, "
          "settings or a WebView2 profile from running the app there?). Move them somewhere safe, or run "
          "again with --force-clean to delete them.", file=sys.stderr)
    sys.exit(1)
print(f"{len(foreign)} foreign entries" + (" removed" if foreign else ""))
PYEOF
done_step foreign

# ------------------------------------------------------------------ 1. Windows app
step "1. Windows app (app/scripts/build-windows.sh)"
rm -f "$OUT/Windows/$NAME.exe"
rm -rf "$OUT/Windows/resources"
mkdir -p "$OUT/Windows"
bash "$REPO/app/scripts/build-windows.sh" "$OUT/Windows" 2>&1 | tee "$LOG/app.log"
[ -f "$OUT/Windows/$NAME.exe" ] || { echo "no $NAME.exe built" >&2; exit 1; }
cmp "$DEMO" "$OUT/Windows/resources/demo-song.jksong"
done_step app

# ------------------------------------------------------------------ 2. Studio runtime
step "2. Studio runtime from the current studio/ (scripts/build-studio-runtime.sh)"
bash "$REPO/scripts/build-studio-runtime.sh" "$OUT" 2>&1 | tee "$LOG/runtime.log"
done_step runtime

step "2b. Studio runtime check, static (scripts/check-studio-runtime.sh --static-only)"
set +e
bash "$REPO/scripts/check-studio-runtime.sh" "$OUT" --static-only --out "$LOG/runtime-check" 2>&1 | tee "$LOG/runtime-check.log"
rc=${PIPESTATUS[0]}
set -e
"$PY" -I -B - "$(pyw "$LOG/runtime-check/check-result.json")" "$rc" <<'PYEOF'
import json, sys
res = json.load(open(sys.argv[1], encoding="utf-8"))["results"]
fails = [r for r in res if r.get("status") == "fail"]
stale = [r for r in res if "matches studio/" in r.get("check", "")]
print(f"exit {sys.argv[2]}; {len(res)} results, {len(fails)} failed")
for r in fails:
    print("FAILED:", r.get("check"), "-", str(r.get("summary"))[:300])
if not stale or stale[0].get("status") != "pass":
    print("STALE or unknown: the runtime's jukeoke_studio does not match studio/:", stale and stale[0].get("summary"))
    sys.exit(1)
print("not stale:", stale[0].get("summary"))
sys.exit(1 if fails or sys.argv[2] != "0" else 0)
PYEOF
done_step runtime-check

# ------------------------------------------------------------------ 3. data folders
step "3. Library, Recordings, Settings, Mac"
mkdir -p "$OUT/Library" "$OUT/Recordings" "$OUT/Settings" "$OUT/Mac"
cp "$DEMO" "$OUT/Library/demo-song.jksong"
subst_name() { sed -e "s/@NAME@/$NAME/g" -e "s/@REPO_DIR@/$(basename "$REPO")/g" "$1"; }
subst_name "$HERE/dist/Recordings-README.txt" > "$OUT/Recordings/README.txt"
subst_name "$HERE/dist/Settings-README.txt" > "$OUT/Settings/README.txt"
subst_name "$HERE/dist/Mac-README.txt" > "$OUT/Mac/README.txt"
done_step data

# ------------------------------------------------------------------ 4. models
step "4. default models -> Models/ (copied, verified by the Studio's code)"
"$PY" -I -B "$(pyw "$HERE/dist/dist_models.py")" --src "$(pyw "$CACHE")" --dest "$(pyw "$OUT/Models")" \
    --preset full --name "$NAME" 2>&1 | tee "$LOG/models.log"
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo "model step failed" >&2; exit 1; }
done_step models

# ------------------------------------------------------------------ 5. licences
step "5. Licenses/"
rm -rf "$OUT/Licenses"
mkdir -p "$OUT/Licenses/fonts" "$OUT/Licenses/python"
cp "$REPO/LICENSE" "$OUT/Licenses/LICENSE.txt"
cp "$REPO/MODELS.md" "$OUT/Licenses/"   # THIRD_PARTY_LICENSES.md: generated for this build below
for d in "$REPO"/app/ui/src/assets/fonts/*/; do
    f="$(basename "$d")"
    mkdir -p "$OUT/Licenses/fonts/$f"
    cp "$d/OFL.txt" "$OUT/Licenses/fonts/$f/OFL.txt"
    [ -f "$d/SOURCE.txt" ] && cp "$d/SOURCE.txt" "$OUT/Licenses/fonts/$f/SOURCE.txt"
done
site="$OUT/Windows/Studio/python/Lib/site-packages"
for pkg in ctranslate2 flatbuffers tokenizers; do
    ver="$(find "$site" -maxdepth 1 -type d -name "$pkg-*.dist-info" -printf '%f\n' | sed -n "s/^$pkg-\(.*\)\.dist-info$/\1/p" | head -n 1)"
    [ -n "$ver" ] || { echo "no $pkg in the runtime" >&2; exit 1; }
    case "$pkg" in
        ctranslate2) src="$HERE/dist/licenses/ctranslate2-$ver-LICENSE.txt" ;;
        *) src="$HERE/dist/licenses/Apache-2.0.txt" ;;
    esac
    [ -f "$src" ] || { echo "no licence text for $pkg $ver ($src); add it to scripts/dist/licenses" >&2; exit 1; }
    cp "$src" "$OUT/Licenses/python/$pkg-$ver-LICENSE.txt"
done
subst_name "$HERE/dist/Licenses-README.txt" > "$OUT/Licenses/README.txt"
# The licence and notice texts of every linked Rust crate, the UI bundle's npm packages and
# the Studio packages whose wheels ship none (MIT/BSD/Apache-2.0 require them with the
# binaries). Generated for THIS build (its Cargo.lock, node_modules and runtime) into the
# log folder and copied to Licenses/third-party/; a dist without them is not made.
# THIRD_PARTY_LICENSES.md is regenerated the same way and compared, ignoring the "Inputs of
# this run" line (it names the runtime's build time, which changes on every build).
set +e
bash "$REPO/scripts/gen-third-party.sh" --studio-runtime "$OUT/Windows/Studio" --out "$LOG/THIRD_PARTY_LICENSES.md" \
    --texts-dir "$LOG/third_party_licenses" > "$LOG/third-party.log" 2>&1
tp=$?
set -e
if [ "$tp" -ne 0 ] || [ ! -f "$LOG/third_party_licenses/rust-crates.txt" ]; then
    cat "$LOG/third-party.log" >&2
    echo "scripts/gen-third-party.sh failed (exit $tp): the licence texts cannot be collected" >&2
    exit 1
fi
cp -r "$LOG/third_party_licenses" "$OUT/Licenses/third-party"
# The list generated for THIS build (its "Inputs" line names the runtime that ships), not the
# repository copy, which may name an earlier runtime build.
cp "$LOG/THIRD_PARTY_LICENSES.md" "$OUT/Licenses/THIRD_PARTY_LICENSES.md"
echo "Licenses/third-party: $(find "$OUT/Licenses/third-party" -type f | wc -l) files, $(du -sk "$OUT/Licenses/third-party" | cut -f1) KB"
if diff <(grep -v '^Inputs of this run' "$REPO/THIRD_PARTY_LICENSES.md") \
        <(grep -v '^Inputs of this run' "$LOG/THIRD_PARTY_LICENSES.md") > "$LOG/third-party.diff" &&
   diff -r "$REPO/third_party_licenses" "$LOG/third_party_licenses" >> "$LOG/third-party.diff"; then
    echo "THIRD_PARTY_LICENSES.md and third_party_licenses/ match this build (apart from the runtime's build time)"
else
    echo "WARNING: THIRD_PARTY_LICENSES.md or third_party_licenses/ in the repository differ from this build" \
         "($LOG/third-party.diff); the dist carries the texts for this build. Run" \
         "scripts/gen-third-party.sh --studio-runtime dist/Windows/Studio to update the repository copies."
fi
done_step licences

# ------------------------------------------------------------------ 6. START-HERE + checks
step "6. size, START-HERE.txt, final checks"
# START-HERE quotes the size: put the template in place (so it is counted), measure, then
# fill in the numbers (a few bytes more never needs another 32 KB cluster).
cp "$HERE/dist/START-HERE.txt" "$OUT/START-HERE.txt"
"$PY" -I -B "$(pyw "$HERE/dist/dist_size.py")" "$(pyw "$OUT")" --json "$(pyw "$LOG/dist-size.json")" > "$LOG/dist-size-pre.txt"
read -r SIZE_GB FILES < <("$PY" -I -c 'import json,sys; t=json.load(open(sys.argv[1]))["rows"]["TOTAL"]; print(format(t["exfat_32k"]/1e9, ".1f"), format(t["files"], ","))' "$(pyw "$LOG/dist-size.json")" | tr -d '\r')
sed -e "s/@NAME@/$NAME/g" -e "s/@DATE@/$(date +%Y-%m-%d)/g" -e "s/@SIZE_GB@/$SIZE_GB/g" -e "s/@FILES@/$FILES/g" \
    "$HERE/dist/START-HERE.txt" > "$OUT/START-HERE.txt"
"$PY" -I -B "$(pyw "$HERE/dist/dist_size.py")" "$(pyw "$OUT")" --json "$(pyw "$LOG/dist-size.json")" | tee "$LOG/dist-size.txt"

"$PY" -I -B - "$(pyw "$OUT")" "$NAME" "$(pyw "$REPO")" "$(pyw "$JK_BUILD")" "${USERPROFILE:-}" <<'PYEOF'
import os, sys
out, name, repo, jkb, home = sys.argv[1:6]
problems = []
settings = sorted(os.listdir(os.path.join(out, "Settings")))
if settings != ["README.txt"]:
    problems.append(f"Settings holds more than README.txt: {settings}")
for dp, dns, fns in os.walk(out):
    if "webview-windows" in (d.lower() for d in dns) or "EBWebView" in dns:
        problems.append(f"a WebView2 profile in {dp}")
    if os.path.relpath(dp, out).replace("\\", "/").startswith("Windows/Studio"):
        dns[:] = []                     # scanned byte for byte by check-studio-runtime.sh
        continue
    for f in fns:
        p = os.path.join(dp, f)
        if os.path.getsize(p) > 64 << 20:
            continue                    # model weights
        data = open(p, "rb").read()
        for needle in (repo, jkb, home):
            if not needle:
                continue
            for form in {needle, needle.replace("\\", "/"), needle.replace("\\", "\\\\")}:
                for enc in ("utf-8", "utf-16-le"):
                    if form.lower().encode(enc) in data.lower():
                        problems.append(f"build-machine path in {os.path.relpath(p, out)}")
recs = sorted(os.listdir(os.path.join(out, "Recordings")))
lib = sorted(os.listdir(os.path.join(out, "Library")))
if recs != ["README.txt"]: problems.append(f"Recordings: {recs}")
if lib != ["demo-song.jksong"]: problems.append(f"Library: {lib}")
for need in (f"Windows/{name}.exe", "Windows/resources/demo-song.jksong", "Windows/Studio/python/python.exe",
             "Mac/README.txt", "Models/manifest.json", "START-HERE.txt", "Licenses/LICENSE.txt"):
    if not os.path.isfile(os.path.join(out, need)):
        problems.append(f"missing {need}")
for p in sorted(set(problems)):
    print("PROBLEM:", p)
print("final checks:", "FAILED" if problems else "OK")
sys.exit(1 if problems else 0)
PYEOF
cmp "$DEMO" "$OUT/Library/demo-song.jksong"
done_step checks

echo
echo "==== dist ready: $OUT"
echo "steps: ${STEPS[*]}"
printf '%s\n' "${STEPS[@]}" > "$LOG/steps.txt"

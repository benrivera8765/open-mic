#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# .jksong conformance: the Python writer/reader (studio/jukeoke_studio/jksong.py) and
# the Rust reader/writer (crates/jk-format) are checked against each other.
#
#   bash tests/conformance/run.sh
#
#   1  Python writes a synthetic song (tones per stem, letter-timed lyrics with "café"
#      and a combining mark, cover, unknown chunk/keys, initial settings).
#   2  Rust verifies it: SONG_FORMAT section 9 in full, every stem decoded by
#      symphonia (rate, channels, bits, length, MD5, sample-accurate seeks), the stems
#      at unity summed against the source (max abs error reported; the main file uses
#      the writer's exact-residual mode, a second "plain" file the stems as given),
#      and the metadata / settings / lyrics graphemes Python wrote.
#   3  Rust writes a song from scratch (own FLAC encoder) and saves settings; Python
#      reads everything back bit for bit (libFLAC decode) and saves settings itself.
#   4  Rust reads the settings Python saved.
#   5  Rust slot-updates and fully rewrites a Python file; Python checks every chunk,
#      unknown chunk and unknown key survived.
#   6  Damaged copies: Python and Rust must give the same verdict on each.
#   7  The committed fixture tests/fixtures/generated/tiny.jksong verifies (strict).
#
# Environment: JK_PYTHON (python with numpy + soundfile), JK_BUILD (scratch root,
# default ../jukeoke-build), JK_CONFORMANCE_OUT (default $JK_BUILD/conformance).
# Generated audio goes there, never into the repository.

set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"
if command -v cygpath >/dev/null 2>&1 && [ -f scripts/devenv.sh ]; then
    # Windows / Git Bash: cargo, gcc and uv on PATH for this shell only.
    source scripts/devenv.sh
fi
JK_BUILD="${JK_BUILD:-$(cd "$REPO/.." && pwd)/jukeoke-build}"
OUT="${JK_CONFORMANCE_OUT:-$JK_BUILD/conformance}"
if [ -n "${JK_PYTHON:-}" ]; then PY="$JK_PYTHON"
elif [ -x "$JK_BUILD/studio-venv/Scripts/python.exe" ]; then PY="$JK_BUILD/studio-venv/Scripts/python.exe"
elif [ -x "$JK_BUILD/studio-venv/bin/python" ]; then PY="$JK_BUILD/studio-venv/bin/python"
else PY=python3
fi

rm -rf "$OUT"
mkdir -p "$OUT"
FAILED=0
step() { printf '\n=== %s\n' "$*"; }
fail() { echo "FAIL: $*"; FAILED=$((FAILED + 1)); }

step "build jk-format examples"
cargo build -p jk-format --examples
VERIFY=(cargo run -q -p jk-format --example jksong-verify --)
WRITE=(cargo run -q -p jk-format --example jksong-write-test --)

step "1. Python writes a synthetic song"
"$PY" -I -B tests/conformance/make_song.py "$OUT/py.jksong" || fail "make_song.py"

step "2. Rust verifies the Python file (section 9, decode, MD5, seeks, sum, expectations)"
"${VERIFY[@]}" "$OUT/py.jksong" --strict --sum-reference "$OUT/py.jksong.source.f32" \
    --exact-residual other --expect "$OUT/py.jksong.expect.json" || fail "Rust verify of the Python-written file"
echo "--- the same stems written plainly (residual computed upstream in float32):"
"${VERIFY[@]}" "$OUT/py-plain.jksong" --strict --sum-reference "$OUT/py-plain.jksong.source.f32" \
    || fail "Rust verify of the plain Python-written file"

step "3. Rust writes a song; Python reads it back and saves settings"
"${WRITE[@]}" "$OUT/rust.jksong" || fail "jksong-write-test"
"$PY" -I -B tests/conformance/read_rust_song.py "$OUT/rust.jksong" || fail "read_rust_song.py"

step "4. Rust reads the settings Python saved into the Rust file"
"${VERIFY[@]}" "$OUT/rust.jksong" --strict --expect "$OUT/rust.jksong.after-python.expect.json" \
    || fail "Rust verify after the Python slot write"

step "5. Rust edits a Python file (slot update, then full rewrite); Python checks"
cp "$OUT/py.jksong" "$OUT/py-edited.jksong"
"${WRITE[@]}" --update "$OUT/py-edited.jksong" || fail "Rust --update"
"${WRITE[@]}" --rewrite "$OUT/py-edited.jksong" || fail "Rust --rewrite"
"$PY" -I -B tests/conformance/check_rust_edits.py "$OUT/py-edited.jksong" "$OUT/py.jksong" || fail "check_rust_edits.py"
"${VERIFY[@]}" "$OUT/py-edited.jksong" --strict || fail "Rust verify of the edited file"
leftovers=$(find "$OUT" -maxdepth 1 -name '*.tmp-*' | wc -l)
[ "$leftovers" -eq 0 ] || fail "$leftovers temporary files left behind"

step "6. Damaged copies: same verdict from Python and Rust"
"$PY" -I -B tests/conformance/corrupt.py "$OUT/py.jksong" "$OUT/corrupt" || fail "corrupt.py"
while IFS=$'\t' read -r name tag status; do
    [ -n "$name" ] || continue
    status="${status%$'\r'}"
    case "$status" in 0|1) ;; *) fail "bad cases.tsv line for $name"; continue ;; esac
    set +e
    log=$("${VERIFY[@]}" "$OUT/corrupt/$name" 2>&1)
    got=$?
    set -e
    if [ "$got" -ne "$status" ]; then
        fail "rust: $name exit $got, expected $status"; echo "$log" | tail -5
    elif [ "$tag" != "-" ] && ! grep -qF -- "$tag" <<<"$log"; then
        fail "rust: $name did not report '$tag'"; echo "$log" | tail -5
    else
        echo "  ok    rust: $name -> ${tag/#-/opens and verifies} (exit $got)"
    fi
done < "$OUT/corrupt/cases.tsv"

step "7. Committed fixture"
"${VERIFY[@]}" tests/fixtures/generated/tiny.jksong --strict || fail "tiny.jksong does not verify"
size=$(wc -c < tests/fixtures/generated/tiny.jksong)
if [ "$size" -lt 600000 ]; then echo "  ok    tiny.jksong is $size bytes (< 600000)"; else fail "tiny.jksong is $size bytes"; fi

echo
if [ "$FAILED" -eq 0 ]; then echo "conformance: PASS"; else echo "conformance: FAIL ($FAILED)"; exit 1; fi

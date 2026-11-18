#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# local-ci.sh - the local equivalent of .github/workflows/ci.yml's Windows job, plus what the
# build PC can do for the Mac job, while GitHub is not set up (docs/TEST_RESULTS.md).
#
#   bash scripts/local-ci.sh [--only s1,s2,...] [--skip s1,...] [--exe PATH]
#
# Suites, in this order (names for --only / --skip):
#   ui            app/ui: npm ci, npm run build, npm test
#   rust          cargo test --workspace --release --locked
#   clippy        cargo clippy --workspace --locked (warnings counted)
#   mac-check     cargo check --target aarch64-apple-darwin --workspace --locked, C compiler for
#                 objc2-exception-helper = Zig (app/scripts/macos-cross, as check-mac.sh); a
#                 type-check only: nothing can be linked or run for macOS here
#   studio-cpu    studio/: pytest (the default selection, GPU tests deselected)
#   studio-gpu    studio/: pytest -m gpu (needs the NVIDIA GPU and the model cache)
#   conformance   tests/conformance/run.sh (Python writer vs Rust reader/writer)
#   portable      app/scripts/portable-smoke-test.sh <exe> (default dist/Windows/<name>.exe)
#   lint          shellcheck on every *.sh, actionlint on .github/workflows (tools from
#                 $JK_BUILD/tools/lint, skipped when absent)
#
# Each suite's output goes to $JK_BUILD/test-sweep/<time>/<suite>.log; summary.tsv lists
# suite, exit code, seconds and the counts read from the log. Exit 0 when every suite that
# ran passed. CARGO_TARGET_DIR defaults to $JK_BUILD/target-dist (shared with make-dist.sh).

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO/scripts/devenv.sh"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$JK_BUILD/target-dist}"

ONLY=""
SKIP=""
EXE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --only) ONLY=" ${2//,/ } "; shift ;;
        --skip) SKIP=" ${2//,/ } "; shift ;;
        --exe) EXE="$2"; shift ;;
        -h|--help) sed -n '4,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
NAME="$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).name' "$(cygpath -w "$REPO/branding.json")")"
EXE="${EXE:-$REPO/dist/Windows/$NAME.exe}"
VPY="$JK_BUILD/studio-venv/Scripts/python.exe"
LINT="$JK_BUILD/tools/lint"
OUT="$JK_BUILD/test-sweep/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
printf 'suite\texit\tseconds\tcounts\n' > "$OUT/summary.tsv"
echo "results: $OUT"
FAILED=""

wanted() { case "$ONLY" in "") ;; *" $1 "*) ;; *) return 1 ;; esac; case "$SKIP" in *" $1 "*) return 1 ;; esac; return 0; }

counts() { # suite log -> one-line counts
    local s="$1" log="$2"
    case "$s" in
        ui) grep -E "Tests +[0-9]+" "$log" | tail -n 1 | sed 's/^ *//' ;;
        rust) awk '/^test result:/ { p += $4; f += $6; i += $8; n++ } END { printf "%d passed, %d failed, %d ignored in %d test binaries", p, f, i, n }' "$log" ;;
        clippy|mac-check)
            printf '%s warnings, %s errors' \
                "$(grep -E '^warning' "$log" | grep -c -v -E 'generated [0-9]+ warning|^warning: jk-rubberband@')" \
                "$(grep -c -E '^error' "$log")" ;;
        studio-cpu|studio-gpu) grep -E "^=*.*(passed|failed|error).* in [0-9.]+s" "$log" | tail -n 1 | sed 's/=//g; s/^ *//' ;;
        conformance) printf '%s ok lines; %s' "$(grep -c -E '^ +ok ' "$log")" "$(grep -E '^conformance:' "$log" | tail -n 1)" ;;
        portable) grep -E "^(RESULT|PASS|FAIL|OK)" "$log" | tail -n 2 | tr '\n' ' ' ;;
        lint) grep -E "^(shellcheck|actionlint):" "$log" | tr '\n' ' ' ;;
    esac
}

run() { # suite command...
    local s="$1"; shift
    wanted "$s" || return 0
    echo "==== $s ($(date +%H:%M:%S))"
    local t=$SECONDS
    ( "$@" ) > "$OUT/$s.log" 2>&1
    local rc=$?
    local dt=$((SECONDS - t))
    local c
    c="$(counts "$s" "$OUT/$s.log")"
    printf '%s\t%s\t%s\t%s\n' "$s" "$rc" "$dt" "$c" >> "$OUT/summary.tsv"
    echo "---- $s: exit $rc, $dt s; $c"
    [ "$rc" -eq 0 ] || FAILED="$FAILED $s"
}

ui() { cd "$REPO/app/ui" && npm ci --no-audit --no-fund && npm run build && npm test; }
rust() { cargo test --workspace --release --locked --manifest-path "$REPO/Cargo.toml"; }
clippy() { cargo clippy --workspace --locked --manifest-path "$REPO/Cargo.toml"; }
mac_check() {
    local py="$JK_BUILD/tools-venv/Scripts/python.exe" cross="$REPO/app/scripts/macos-cross"
    "$py" -I -c "import ziglang" || { echo "Zig (PyPI ziglang) missing in $JK_BUILD/tools-venv: run app/scripts/check-mac.sh once"; return 1; }
    rustup target add aarch64-apple-darwin
    JK_TOOLS_PY="$(cygpath -w "$py")" \
    CC_aarch64_apple_darwin="$(cygpath -w "$cross/zig-cc.cmd")" \
    AR_aarch64_apple_darwin="$(cygpath -w "$cross/zig-ar.cmd")" \
    ZIG_GLOBAL_CACHE_DIR="$JK_BUILD/zig-cache" ZIG_LOCAL_CACHE_DIR="$JK_BUILD/zig-cache" \
        cargo check --target aarch64-apple-darwin --workspace --locked --manifest-path "$REPO/Cargo.toml"
}
studio_cpu() { cd "$REPO/studio" && "$VPY" -m pytest tests -q -rs -p no:cacheprovider; }
studio_gpu() { cd "$REPO/studio" && "$VPY" -m pytest -m gpu tests -q -rs -p no:cacheprovider; }
conformance() { bash "$REPO/tests/conformance/run.sh"; }
portable() { [ -f "$EXE" ] || { echo "no $EXE (run scripts/make-dist.sh first)"; return 1; }; bash "$REPO/app/scripts/portable-smoke-test.sh" "$EXE"; }
lint() {
    local rc=0 n files
    mapfile -t files < <(cd "$REPO" && git ls-files '*.sh'; cd "$REPO" && git ls-files --others --exclude-standard '*.sh')
    if [ -x "$LINT/shellcheck/shellcheck.exe" ]; then
        (cd "$REPO" && "$LINT/shellcheck/shellcheck.exe" -x "${files[@]}") || rc=1
        n=$(cd "$REPO" && "$LINT/shellcheck/shellcheck.exe" -x -f gcc "${files[@]}" | wc -l)
        echo "shellcheck: ${#files[@]} files, $n findings ($("$LINT/shellcheck/shellcheck.exe" --version | sed -n 's/^version: //p'))"
    else
        echo "shellcheck: not installed in $LINT"
    fi
    if [ -x "$LINT/actionlint/actionlint.exe" ]; then
        # with ShellCheck on PATH, actionlint also checks the run: blocks
        (cd "$REPO" && PATH="$LINT/shellcheck:$PATH" "$LINT/actionlint/actionlint.exe") || rc=1
        n=$(cd "$REPO" && PATH="$LINT/shellcheck:$PATH" "$LINT/actionlint/actionlint.exe" -oneline | wc -l)
        echo "actionlint: $(find "$REPO/.github/workflows" -name "*.yml" | wc -l) workflows, $n findings ($("$LINT/actionlint/actionlint.exe" --version | head -n 1))"
    else
        echo "actionlint: not installed in $LINT"
    fi
    return $rc
}

run ui ui
run rust rust
run clippy clippy
run mac-check mac_check
run studio-cpu studio_cpu
run studio-gpu studio_gpu
run conformance conformance
run portable portable
run lint lint

echo
column -t -s $'\t' "$OUT/summary.tsv" 2>/dev/null || cat "$OUT/summary.tsv"
if [ -n "$FAILED" ]; then echo "FAILED:$FAILED"; exit 1; fi
echo "all suites passed"

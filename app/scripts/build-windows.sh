#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the Windows app on the GNU toolchain (no MSVC needed, DECISIONS D-10):
#   1. the web UI (app/ui: npm ci, type-check, Vite build, baseline syntax check, tests);
#   2. the Rust app crate `jukeoke` (cargo build --release -p jukeoke);
#   3. a stripped copy named after branding.json, e.g. <out>/Jukeoke.exe, and the
#      app's resources next to it (<out>/resources/demo-song.jksong: the self-check's
#      demo song; Tauri's resource folder on Windows is the exe's folder);
#   4. the system-DLL check (app/scripts/check-windows-exe.sh).
#
# Usage (Git Bash):  app/scripts/build-windows.sh [out_dir]
#   out_dir defaults to $JK_BUILD/app-dist/Windows. Honours CARGO_TARGET_DIR.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=/dev/null
source "$repo/scripts/devenv.sh"
out="${1:-$JK_BUILD/app-dist/Windows}"

echo "== UI"
(cd "$repo/app/ui" && npm ci --no-fund --no-audit && npm run build && npm test)

echo "== Rust (release, $(rustc -vV | sed -n 's/^host: //p'))"
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
# No build-machine paths in the shipped exe. Without this it held 411 copies of
# C:\Users\<name>\.cargo\registry\... (panic locations of dependencies), the source tree's
# path (env!("CARGO_MANIFEST_DIR") for developer fallbacks) and the repository path
# (Rubber Band's __FILE__). Fixes: --remap-path-prefix for the Rust paths (passed with
# --config, which cargo appends to .cargo/config.toml's target rustflags instead of
# replacing them as RUSTFLAGS would), JK_DIST_BUILD=1 (portable::SOURCE_DIR becomes None)
# and -ffile-prefix-map in crates/jk-rubberband/build.rs. The check after `strip` fails
# the build if any such path is left.
remap=()
for pair in "${USERPROFILE:-}=/home" "$(cygpath -w "$repo")=/jukeoke" "$(cygpath -w "$target_dir")=/target" \
            "$(cygpath -w "${CARGO_HOME:-$HOME/.cargo}")=/cargo"; do
  [[ "$pair" == =* ]] && continue
  if [[ "$pair" == *"'"* ]]; then echo "WARNING: not remapping a path with a quote in it: $pair"; continue; fi
  remap+=("'--remap-path-prefix=$pair'")
done
remap_list="$(IFS=,; echo "${remap[*]-}")"
JK_DIST_BUILD=1 cargo build --release -p jukeoke --manifest-path "$repo/Cargo.toml" \
  --config "target.x86_64-pc-windows-gnu.rustflags=[$remap_list]"

name="$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).name' "$(cygpath -w "$repo/branding.json")")"
mkdir -p "$out"
# The release profile keeps line tables for backtraces; the shipped exe does not need them.
strip -o "$out/$name.exe" "$target_dir/release/jukeoke.exe"
echo "unstripped: $(stat -c %s "$target_dir/release/jukeoke.exe") bytes"

echo "== build-machine paths in the exe"
# Case-insensitive, both slash styles (GCC writes forward slashes), UTF-8 and UTF-16LE.
# Node rather than grep: MSYS grep -i aborts (exit 134) on this binary.
node - "$(cygpath -w "$out/$name.exe")" "${USERPROFILE:-}" "$(cygpath -w "$repo")" <<'JSEOF'
const fs = require("fs");
const [exe, ...paths] = process.argv.slice(2);
const hay = fs.readFileSync(exe);
const lower = Buffer.from(hay.toString("latin1").toLowerCase(), "latin1");
// UTF-16 at even and at odd offsets
const wide = [hay, hay.subarray(1)].map((b) => Buffer.from(b.toString("utf16le").toLowerCase(), "utf16le"));
let leaks = 0;
for (const p of paths.filter(Boolean)) {
  for (const form of new Set([p, p.replaceAll("\\", "/")])) {
    const f = form.toLowerCase();
    let n = 0;
    for (let i = lower.indexOf(f, 0, "latin1"); i >= 0; i = lower.indexOf(f, i + 1, "latin1")) n++;
    for (const w of wide) for (let i = w.indexOf(f, 0, "utf16le"); i >= 0; i = w.indexOf(f, i + 1, "utf16le")) n++;
    console.log(`${form.replaceAll("\\", "/")}: ${n}`);
    leaks += n;
  }
}
if (leaks > 0) { console.log(`FAIL: ${exe} contains build-machine paths`); process.exit(1); }
console.log("none");
JSEOF

echo "== resources"
mkdir -p "$out/resources"
demo="$repo/app/src-tauri/resources/demo-song.jksong"
if [[ -f "$demo" ]]; then
  cp "$demo" "$out/resources/demo-song.jksong"
  echo "resources/demo-song.jksong: $(stat -c %s "$out/resources/demo-song.jksong") bytes"
else
  echo "WARNING: $demo is missing (build.rs copies it from tests/fixtures/generated/demo-song.jksong);"
  echo "         the self-check's demo-song item will fail in this build."
fi

echo "== DLL check"
"$repo/app/scripts/check-windows-exe.sh" "$out/$name.exe"

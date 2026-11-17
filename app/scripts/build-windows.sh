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
cargo build --release -p jukeoke --manifest-path "$repo/Cargo.toml"

target_dir="${CARGO_TARGET_DIR:-$repo/target}"
name="$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).name' "$(cygpath -w "$repo/branding.json")")"
mkdir -p "$out"
# The release profile keeps line tables for backtraces; the shipped exe does not need them.
strip -o "$out/$name.exe" "$target_dir/release/jukeoke.exe"
echo "unstripped: $(stat -c %s "$target_dir/release/jukeoke.exe") bytes"

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

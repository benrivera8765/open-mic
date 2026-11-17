#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Portable-mode smoke test for the Windows app (WORK_ORDER section 9, "Portability").
#
#   1. Lays out a fake stick:  <root>/Windows/Jukeoke.exe  and  <root>/Library/
#   2. Snapshots everything named like the app in %LOCALAPPDATA% and %APPDATA%, and
#      every HKCU registry key, value or data containing "jukeoke".
#   3. Runs `Jukeoke.exe --smoke-test` from the normal path, then again through a
#      different drive letter made with `subst`, deleting <root>/Settings before
#      each run so each run must create its own state (the delete is retried until
#      the previous run's WebView2 processes have let go; see clear_settings).
#   4. Checks each report (Settings/smoke-test.json) and repeats the snapshots.
#      Any new entry fails the test, with one documented exception: Explorer's
#      MuiCache (HKCU\Software\Classes\Local Settings\...\Shell\MuiCache). Windows
#      Explorer records the display name of EVERY program that shows a taskbar
#      window there; the program itself writes nothing. Control experiment
#      (2026-11-17): a 20-line Win32 program importing only kernel32/user32/msvcrt
#      got the same entry when its window was shown, and none when it stayed
#      hidden. These entries are listed separately and do not fail the test.
#
# Usage (Git Bash, after `source scripts/devenv.sh`):
#   app/scripts/portable-smoke-test.sh path/to/Jukeoke.exe [root]
# root defaults to $JK_BUILD/portable-test; reports go to <root>-reports/.
# Exit code 0 only if every run passed and nothing was written outside <root>.
set -uo pipefail
export MSYS_NO_PATHCONV=1 # keep `reg ... /s` and `subst X: /D` arguments intact

exe="${1:?usage: portable-smoke-test.sh path/to/Jukeoke.exe [root]}"
root="${2:-${JK_BUILD:?source scripts/devenv.sh first}/portable-test}"
reports="$root-reports"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
local_appdata="$(cygpath -u "$LOCALAPPDATA")"
roaming_appdata="$(cygpath -u "$APPDATA")"
failures=0

rm -rf "$root" "$reports"
mkdir -p "$root/Windows" "$root/Library" "$reports"
cp "$exe" "$root/Windows/Jukeoke.exe"

# Prints "key | value line" for every HKCU match, so sorting keeps the context.
registry_matches() {
  reg query HKCU /s /f jukeoke 2>/dev/null | tr -d '\r' |
    awk '/^HKEY_/ { key = $0; print key; next } /^ / { print key " | " $0 }' | sort -u
}

snapshot() { # $1: output file (audited), $2: output file (Explorer shell cache, informational)
  local muicache='\\Shell\\MuiCache' registry
  registry="$(registry_matches)"
  {
    echo "--- files and folders named *jukeoke* under %LOCALAPPDATA% and %APPDATA%"
    find "$local_appdata" "$roaming_appdata" -iname '*jukeoke*' 2>/dev/null | sort
    echo "--- HKCU keys, values and data containing 'jukeoke' (except Explorer's MuiCache)"
    grep -v -E "$muicache" <<<"$registry" || true
  } >"$1"
  grep -E "$muicache" <<<"$registry" >"$2" || true
}

# Deletes <root>/Settings, retrying for up to 60 s. WebView2's browser processes outlive
# Jukeoke.exe (about 0.2-0.3 s on an idle PC, 8-20 s under heavy CPU load) and hold
# Settings/webview-windows/EBWebView/lockfile until they exit. A single `rm -rf` in that
# window leaves a half-deleted profile with the lock still held, and the next start
# through another path to the same folder (the subst run) then fails to create its
# web view (HRESULT 0x800700AA, "The requested resource is in use"). Measured
# 2026-11-17: 5 of 5 run-2 failures with 16 busy CPU processes without this retry,
# 0 of 5 with it. Success of the delete proves the previous run has fully let go.
clear_settings() { # $1: label
  local tries=0
  until rm -rf "$root/Settings" 2>/dev/null; do
    tries=$((tries + 1))
    if ((tries >= 600)); then
      echo "[$1] FAIL: $root/Settings is still locked 60 s after the previous run"
      return 1
    fi
    sleep 0.1
  done
  ((tries > 0)) && echo "[$1] waited $((tries * 100)) ms for the previous run's WebView2 processes to release Settings/"
  return 0
}

run_smoke() { # $1: label, $2: exe path to start, $3: expected portable root (Windows path)
  local label="$1" start="$2" expected_root="$3"
  if ! clear_settings "$label"; then
    failures=$((failures + 1))
    return
  fi
  local t0 t1 code
  t0=$(date +%s%N)
  "$start" --smoke-test >"$reports/$label.stdout" 2>&1
  code=$?
  t1=$(date +%s%N)
  echo "[$label] started $start -> exit code $code, wall time $(((t1 - t0) / 1000000)) ms"
  if [[ ! -f "$root/Settings/smoke-test.json" ]]; then
    echo "[$label] FAIL: no Settings/smoke-test.json written"
    failures=$((failures + 1))
    return
  fi
  cp "$root/Settings/smoke-test.json" "$reports/$label.json"
  [[ -f "$root/Settings/smoke-test.png" ]] && cp "$root/Settings/smoke-test.png" "$reports/$label.png"
  (cd "$root" && find . -mindepth 1 -maxdepth 3 | sort) >"$reports/$label.tree"
  if ! node "$(cygpath -w "$here/verify-smoke-report.mjs")" "$(cygpath -w "$reports/$label.json")" "$expected_root" "$code"; then
    failures=$((failures + 1))
  fi
}

echo "== snapshot before"
snapshot "$reports/before.txt" "$reports/muicache-before.txt"

echo "== run 1: from $(cygpath -w "$root")"
run_smoke "run1-original-path" "$root/Windows/Jukeoke.exe" "$(cygpath -w "$root")"

echo "== run 2: through a subst drive letter"
drive=""
for letter in J K L M N O P Q R S T U V W X Y Z; do
  if [[ ! -e "$letter:/" ]]; then drive="$letter:"; break; fi
done
if [[ -z "$drive" ]]; then
  echo "FAIL: no free drive letter for subst"
  failures=$((failures + 1))
else
  subst "$drive" "$(cygpath -w "$root")"
  trap 'subst "$drive" /D >/dev/null 2>&1' EXIT
  run_smoke "run2-subst-${drive%:}" "$drive/Windows/Jukeoke.exe" "$drive\\"
  subst "$drive" /D && trap - EXIT
  echo "[subst] $drive removed"
fi

echo "== snapshot after"
snapshot "$reports/after.txt" "$reports/muicache-after.txt"
if diff -u "$reports/before.txt" "$reports/after.txt" >"$reports/snapshot.diff"; then
  echo "[audit] OK: nothing named jukeoke appeared in %LOCALAPPDATA%, %APPDATA% or HKCU"
else
  echo "[audit] FAIL: new entries outside the portable root:"
  cat "$reports/snapshot.diff"
  failures=$((failures + 1))
fi
if ! diff -u "$reports/muicache-before.txt" "$reports/muicache-after.txt" >"$reports/muicache.diff"; then
  echo "[audit] note: Explorer's MuiCache gained display-name entries (written by the shell for every"
  echo "        program that shows a window; not by the app, see the header of this script):"
  grep '^+[^+]' "$reports/muicache.diff" | sed 's/^+/        /'
fi

echo "== portable root after the last run ($(cygpath -w "$root"))"
sed 's/^/  /' "$reports/run2-subst-${drive%:}.tree" 2>/dev/null || true
echo "reports: $(cygpath -w "$reports")"
if [[ $failures -ne 0 ]]; then
  echo "RESULT: FAIL ($failures problem(s))"
  exit 1
fi
echo "RESULT: OK"

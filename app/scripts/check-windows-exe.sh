#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Checks that a Windows build of Jukeoke depends only on DLLs that ship with
# Windows 11 (WORK_ORDER section 4.8: "launch on a clean Windows 11 PC with nothing
# preinstalled"), and prints its size.
#
# Usage (Git Bash, after `source scripts/devenv.sh` for objdump):
#   app/scripts/check-windows-exe.sh path/to/Jukeoke.exe
#
# A DLL passes when it is a Windows API set (api-ms-win-*, ext-ms-win-*) or exists in
# %SystemRoot%\System32, and is not a known redistributable that only *happens* to
# be in System32 on this PC (Visual C++ runtime, MinGW runtime, WebView2Loader).
# Exit code 1 if anything else is imported.
set -euo pipefail

exe="${1:?usage: check-windows-exe.sh path/to/Jukeoke.exe}"
command -v objdump >/dev/null || { echo "objdump not found: source scripts/devenv.sh first" >&2; exit 2; }
system32="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32"

# Redistributables: never acceptable as imports, even if present on this PC.
redistributable='^(webview2loader|vcruntime[0-9_]*|msvcp[0-9_]*|ucrtbase|concrt[0-9]*|vccorlib[0-9]*|libgcc_s_.*|libstdc\+\+-.*|libwinpthread-.*|libgomp-.*)\.dll$'

mapfile -t dlls < <(objdump -p "$exe" | sed -n 's/^[[:space:]]*DLL Name: //p' | tr -d '\r' | sort -fu)
bad=0
for dll in "${dlls[@]}"; do
  lower="$(printf '%s' "$dll" | tr '[:upper:]' '[:lower:]')"
  if [[ "$lower" =~ $redistributable ]]; then
    echo "FAIL  $dll  (redistributable, not part of Windows)"; bad=1
  elif [[ "$lower" == api-ms-win-* || "$lower" == ext-ms-win-* ]]; then
    echo "ok    $dll  (Windows API set)"
  elif [[ -f "$system32/$dll" || -f "$system32/$lower" ]]; then
    echo "ok    $dll  (System32)"
  else
    echo "FAIL  $dll  (not a Windows system DLL)"; bad=1
  fi
done

size=$(stat -c %s "$exe")
echo "size: $size bytes ($(awk -v s="$size" 'BEGIN { printf "%.2f MiB", s / 1048576 }'))"
if [[ $bad -ne 0 ]]; then
  echo "RESULT: FAIL - $exe imports non-system DLLs"
  exit 1
fi
echo "RESULT: OK - ${#dlls[@]} imported DLLs, all part of Windows"

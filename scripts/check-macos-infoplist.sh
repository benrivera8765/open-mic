#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# check-macos-infoplist.sh - does the Mac app bundle carry what practice mode
# needs to use the microphone? (WORK_ORDER-2 section 4.8 and trap 3.)
#
#   1. Contents/Info.plist has a non-empty NSMicrophoneUsageDescription.
#      Without it macOS ends the app the first time practice mode opens the
#      microphone. The text comes from app/src-tauri/Info.plist, which the
#      Tauri CLI merges into the bundle's Info.plist.
#   2. If the app's code signature uses the hardened runtime (the CodeDirectory
#      flags include "runtime"), its entitlements must include
#      com.apple.security.device.audio-input = true, or the hardened runtime
#      blocks Core Audio input (Apple, "Configuring the hardened runtime",
#      Resource Access: Audio Input). scripts/build-mac.sh signs ad hoc WITHOUT
#      --options runtime, so normally this does not apply and no entitlement
#      is needed; the check is here so a future signing change cannot silently
#      break the microphone.
#
# Usage:   bash scripts/check-macos-infoplist.sh path/to/Open Mic.app
# Exit:    0 all checks passed, 1 a check failed, 2 usage problem.
# Runs on macOS (PlistBuddy, codesign). Elsewhere check 1 reads an XML plist
# with awk (so it can be tested on Windows) and check 2 is skipped.
set -euo pipefail

app="${1:-}"
if [ -z "$app" ] || [ ! -d "$app" ]; then
    echo "usage: $0 path/to/App.app" >&2
    exit 2
fi
plist="$app/Contents/Info.plist"
if [ ! -f "$plist" ]; then
    echo "FAIL: $plist does not exist" >&2
    exit 1
fi

key=NSMicrophoneUsageDescription
if [ -x /usr/libexec/PlistBuddy ]; then
    text="$(/usr/libexec/PlistBuddy -c "Print :$key" "$plist" 2>/dev/null || true)"
else
    # XML plists only (binary ones need PlistBuddy): the <string> after the key.
    # awk reads to the end instead of exiting at the match: an early exit would
    # end `tr` with SIGPIPE on a large plist, and with pipefail + set -e the
    # script would then stop with status 141 and no message.
    text="$(tr -d '\r' < "$plist" | awk -v k="<key>$key</key>" '
        function grab(s) {
            if (match(s, /<string>[^<]*<\/string>/)) {
                print substr(s, RSTART + 8, RLENGTH - 17); return 1
            }
            return 0
        }
        got { next }
        found { if (grab($0)) got = 1; next }
        index($0, k) { found = 1; if (grab(substr($0, index($0, k) + length(k)))) got = 1 }
    ')"
fi
if [ -z "$text" ]; then
    echo "FAIL: $plist has no $key. macOS would end the app the first time practice mode" \
         "opens the microphone. app/src-tauri/Info.plist must carry it (the Tauri CLI merges it)." >&2
    exit 1
fi
echo "ok: $key is set: \"$text\""

if ! command -v codesign >/dev/null 2>&1; then
    echo "note: codesign is not available here; the hardened-runtime check was skipped"
    exit 0
fi
cd_line="$(codesign -dv "$app" 2>&1 | grep -E '^CodeDirectory' | head -n 1 || true)"
flags="$(printf '%s' "$cd_line" | sed -n 's/.*flags=0x[0-9a-fA-F]*(\([^)]*\)).*/\1/p')"
case ",$flags," in
    *,runtime,*)
        ents="$(codesign -d --entitlements - --xml "$app" 2>/dev/null \
                || codesign -d --entitlements :- "$app" 2>/dev/null || true)"
        if printf '%s' "$ents" | tr -d ' \t\r\n' \
                | grep -q '<key>com.apple.security.device.audio-input</key><true/>'; then
            echo "ok: hardened runtime is on and com.apple.security.device.audio-input is granted"
        else
            echo "FAIL: the signature uses the hardened runtime (flags: $flags) but lacks the" \
                 "com.apple.security.device.audio-input entitlement, so the microphone would be" \
                 "blocked. Sign without --options runtime (as build-mac.sh does) or add the" \
                 "entitlement." >&2
            exit 1
        fi
        ;;
    *)
        echo "ok: signature flags (${flags:-none reported}) do not include the hardened runtime," \
             "so no com.apple.security.device.audio-input entitlement is needed"
        ;;
esac

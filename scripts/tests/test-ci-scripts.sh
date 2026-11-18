#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# test-ci-scripts.sh - prove the failure paths of the CI scripts with fake inputs, so a
# green CI run cannot come from a script that skipped or compared nothing.
#
#   bash scripts/tests/test-ci-scripts.sh            (Git Bash on Windows, bash 3.2 on macOS)
#
# Covers: ci-compare-renders.sh, ci-render.sh (explicit skip -> compare fails),
# ci-exfat-launch.sh (no bundle, explicit skip, not on macOS), ci-selfcheck-verdict.sh
# (every verdict), ci-sck-probe.sh (--verdict mapping, explicit skip),
# check-macos-availability.sh (fake SDK + fake nm/otool), check-macos-paths.sh, and that
# ci.yml sets no JK_SKIP_* variable. Needs python3 (or $PYTHON). Plain bash, no bats.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
S="$REPO/scripts"
PY="${PYTHON:-python3}"
export PYTHON="$PY"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
# CI variables of the real run must not leak into the scripts under test.
unset GITHUB_ACTIONS GITHUB_STEP_SUMMARY JK_SKIP_EXFAT JK_SKIP_SCK JK_SKIP_RENDER JK_SKIP_RENDER_COMPARE CI

pass=0
fail=0
n=0
LOG="$T/last.log"

# check DESC EXPECTED_EXIT CMD... : run CMD, compare its exit code.
check() {
    local desc="$1" want="$2" got
    shift 2
    n=$((n + 1))
    "$@" > "$LOG" 2>&1
    got=$?
    if [ "$got" = "$want" ]; then
        pass=$((pass + 1)); echo "ok   $n - $desc"
    else
        fail=$((fail + 1)); echo "FAIL $n - $desc (exit $got, wanted $want)"; sed 's/^/       | /' "$LOG" | head -n 25
    fi
}
# has DESC PATTERN [FILE]: the last command's output (or FILE) matches PATTERN (grep -E).
has() {
    local desc="$1" pat="$2" file="${3:-$LOG}"
    n=$((n + 1))
    if grep -Eq -- "$pat" "$file" 2>/dev/null; then
        pass=$((pass + 1)); echo "ok   $n - $desc"
    else
        fail=$((fail + 1)); echo "FAIL $n - $desc (no /$pat/ in $(basename "$file"))"; sed 's/^/       | /' "$file" 2>/dev/null | head -n 25
    fi
}

on_mac=0
[ "$(uname -s)" = Darwin ] && on_mac=1

# --- ci-compare-renders.sh ---------------------------------------------------------
mkwav() { # mkwav FILE VALUE [FRAMES]: 32-bit float stereo WAV, every sample = VALUE
    "$PY" -I - "$1" "$2" "${3:-64}" <<'PYEOF'
import struct, sys
path, v, frames = sys.argv[1], float(sys.argv[2]), int(sys.argv[3])
data = struct.pack("<%df" % (frames * 2), *([v] * frames * 2))
fmt = struct.pack("<HHIIHH", 3, 2, 48000, 48000 * 8, 8, 32)
body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
open(path, "wb").write(b"RIFF" + struct.pack("<I", len(body)) + body)
PYEOF
}
W="$T/r/win"; M="$T/r/mac"
mkdir -p "$W" "$M"
mkwav "$W/a.wav" 0.25
check "compare: both platforms missing -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$T/r/nowin" "$T/r/nomac"
has "compare: names the missing Windows renders" "no Windows renders"
check "compare: macOS folder missing -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$T/r/nomac"
has "compare: names the missing macOS renders" "no macOS renders"
check "compare: macOS folder empty -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$M"
echo "skipped: test" > "$M/SKIPPED.txt"; mkwav "$M/a.wav" 0.25
check "compare: SKIPPED.txt from ci-render.sh -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$M"
rm -f "$M/SKIPPED.txt"
check "compare: identical renders -> pass" 0 bash "$S/ci-compare-renders.sh" "$W" "$M"
has "compare: prints the measured difference" "1 file\(s\), 0 failure"
mkwav "$M/a.wav" 0.2501
check "compare: difference above 1e-5 -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$M"
mkwav "$M/a.wav" 0.25; mkwav "$W/b.wav" 0.1
check "compare: a render only on one platform -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$M"
mkwav "$M/b.wav" 0.1 63
check "compare: different length -> FAIL" 1 bash "$S/ci-compare-renders.sh" "$W" "$M"
check "compare: explicitly disabled -> skip (exit 0)" 0 env JK_SKIP_RENDER_COMPARE="test reason" bash "$S/ci-compare-renders.sh" "$T/r/nowin" "$T/r/nomac"
has "compare: the skip names its reason" "explicitly disabled: test reason"
export GITHUB_STEP_SUMMARY="$T/summary-compare.md"
bash "$S/ci-compare-renders.sh" "$W" "$T/r/nomac" > "$LOG" 2>&1
has "compare: a failure is written to the job summary" "FAILED \(renders missing\)" "$GITHUB_STEP_SUMMARY"
rm -f "$M/b.wav" "$W/b.wav"
bash "$S/ci-compare-renders.sh" "$W" "$M" > "$LOG" 2>&1
has "compare: a pass is written to the job summary with the count" "passed" "$GITHUB_STEP_SUMMARY"
has "compare: the summary names the number of renders" "1 render\(s\) compared" "$GITHUB_STEP_SUMMARY"
unset GITHUB_STEP_SUMMARY

# --- ci-render.sh: an explicit skip makes the comparison fail ------------------------
check "render: explicitly disabled -> exit 0 with SKIPPED.txt" 0 env JK_SKIP_RENDER="test" bash "$S/ci-render.sh" "$T/render-skipped"
has "render: SKIPPED.txt written" "explicitly disabled" "$T/render-skipped/SKIPPED.txt"
check "render -> compare: skipped renders fail the comparison" 1 bash "$S/ci-compare-renders.sh" "$W" "$T/render-skipped"

# --- ci-exfat-launch.sh -------------------------------------------------------------
check "exfat: no app bundle given -> FAIL" 1 bash "$S/ci-exfat-launch.sh" "" "$T/exfat1"
has "exfat: says the build should have produced it" "build step should have produced it"
has "exfat: summary says FAILED" "FAILED, no app bundle" "$T/exfat1/SUMMARY.md"
check "exfat: app path that does not exist -> FAIL" 1 bash "$S/ci-exfat-launch.sh" "$T/nowhere/Jukeoke.app" "$T/exfat2"
check "exfat: explicitly disabled -> skip (exit 0)" 0 env JK_SKIP_EXFAT="test reason" bash "$S/ci-exfat-launch.sh" "" "$T/exfat3"
has "exfat: skip is in the summary with its reason" "SKIPPED, explicitly disabled \(test reason\)" "$T/exfat3/SUMMARY.md"
if [ "$on_mac" = 0 ]; then
    mkdir -p "$T/Fake.app/Contents/MacOS"
    check "exfat: a bundle but not macOS -> FAIL (never a silent pass)" 1 bash "$S/ci-exfat-launch.sh" "$T/Fake.app" "$T/exfat4"
fi
has "exfat: the default self-check arguments are --selfcheck --silent" 'JK_SELFCHECK_ARGS:---selfcheck --silent' "$S/ci-exfat-launch.sh"

# --- ci-selfcheck-verdict.sh ----------------------------------------------------------
report() { # report FILE RESULT [ITEM=STATE:DETAIL ...]  (CRLF, like the app writes it)
    local f="$1" result="$2"; shift 2
    {
        printf 'Jukeoke self-check\r\n==============================\r\nresult: %s\r\ntook: 9.1 s\r\n\r\n' "$result"
        for it in "$@"; do
            name="${it%%=*}"; rest="${it#*=}"; st="${rest%%:*}"; det="${rest#*:}"
            if [ "$st" = ok ]; then printf '[ OK ] %s\r\n' "$name"; else printf '[FAIL] %s\r\n' "$name"; fi
            printf '    %s\r\n\r\n' "$det"
        done
        printf 'Bring this file back to the Windows PC.\r\n'
    } > "$f"
}
OK7="System=ok:x"
items_ok() {
    echo "System=ok:macOS 15.5;Web engine=ok:WebKit 620;Memory=ok:16 GB;Output device=ok:BlackHole 48000 Hz;Test tone=ok:ratio 1.000;$1;Demo song=ok:40 s;Key change=ok:+0.000 cents"
}
mkreport() { # mkreport FILE RESULT "RECORDER_ITEM"
    local IFS=';'
    # shellcheck disable=SC2046  # split on ; on purpose
    report "$1" "$2" $(items_ok "$3")
}
V="$S/ci-selfcheck-verdict.sh"
mkreport "$T/sc-pass.txt" PASSED "Recorder=ok:Permission: granted. recorded 3.01 s"
check "verdict: all eight items OK, exit 0 -> PASS" 0 bash "$V" "$T/sc-pass.txt" 0
has "verdict: says PASS" "^PASS: all 8 items OK"
check "verdict: all OK, exit unknown (open) -> PASS" 0 bash "$V" "$T/sc-pass.txt" -
mkreport "$T/sc-perm.txt" FAILED "Recorder=fail:Screen Recording permission: denied. macOS calls the system-audio permission \"Screen Recording\"."
check "verdict: only the recorder permission failed -> WARN (3)" 3 bash "$V" "$T/sc-perm.txt" 1
has "verdict: the warning names the permission" "^WARN: .*Screen Recording permission: denied"
mkreport "$T/sc-noaudio.txt" FAILED "Recorder=fail:Permission: granted. The capture started in 0.5 s. No audio arrived from the system during the capture."
check "verdict: recorder failed for another reason -> FAIL" 1 bash "$V" "$T/sc-noaudio.txt" 1
has "verdict: lists the failed item" "\[FAIL\] Recorder: Permission: granted"
IFS=';'
# shellcheck disable=SC2046
report "$T/sc-two.txt" FAILED $(items_ok "Recorder=fail:Screen Recording permission: denied." | sed 's/Test tone=ok/Test tone=fail/')
unset IFS
check "verdict: permission AND another item failed -> FAIL" 1 bash "$V" "$T/sc-two.txt" 1
check "verdict: no report -> FAIL" 1 bash "$V" "$T/does-not-exist.txt" 1
: > "$T/sc-empty.txt"
check "verdict: empty report -> FAIL" 1 bash "$V" "$T/sc-empty.txt" 1
# shellcheck disable=SC2086
report "$T/sc-short.txt" PASSED $OK7
check "verdict: items missing (self-check did not finish) -> FAIL" 1 bash "$V" "$T/sc-short.txt" 0
has "verdict: names the missing items" "no item for Web engine, Memory, Output device"
check "verdict: report PASSED but the app exited 1 -> FAIL" 1 bash "$V" "$T/sc-pass.txt" 1
mkreport "$T/sc-liar.txt" FAILED "Recorder=ok:fine"
check "verdict: all items OK but result FAILED -> FAIL" 1 bash "$V" "$T/sc-liar.txt" 1
check "verdict: items failed but the app exited 0 -> FAIL" 1 bash "$V" "$T/sc-noaudio.txt" 0
check "verdict: timeout (124) -> FAIL" 1 bash "$V" "$T/sc-pass.txt" 124
check "verdict: crash (139) -> FAIL" 1 bash "$V" "$T/sc-pass.txt" 139
printf 'no result line\r\n[ OK ] System\r\n    x\r\n' > "$T/sc-garbled.txt"
check "verdict: no result line -> FAIL" 1 bash "$V" "$T/sc-garbled.txt" 0

# --- ci-sck-probe.sh ------------------------------------------------------------------
check "sck: probe exit 0 -> pass" 0 bash "$S/ci-sck-probe.sh" --verdict 0
check "sck: permission not granted (2) -> warning (3)" 3 bash "$S/ci-sck-probe.sh" --verdict 2
has "sck: the warning says the runner could not grant it" "WARN: permission not granted: the runner could not grant"
for c in 1 3 4 5 134; do
    check "sck: probe exit $c -> FAIL" 1 bash "$S/ci-sck-probe.sh" --verdict "$c"
done
check "sck: explicitly disabled -> skip (exit 0)" 0 env JK_SKIP_SCK="test" bash "$S/ci-sck-probe.sh" "$T/sck"
if [ "$on_mac" = 0 ]; then
    check "sck: not on macOS -> FAIL (never a silent pass)" 1 bash "$S/ci-sck-probe.sh" "$T/sck2"
fi

# --- check-macos-paths.sh -------------------------------------------------------------
mkdir -p "$T/paths/App.app/Contents/MacOS"
printf 'abc\0/USERS/testuser/.cargo/registry/src/x.rs\0def' > "$T/paths/App.app/Contents/MacOS/app"
check "paths: home folder embedded (different case) -> FAIL" 1 bash "$S/check-macos-paths.sh" --forbid /Users/testuser "$T/paths"
has "paths: names the file" "FOUND +1 x /Users/testuser"
"$PY" -I -c "import sys; open(sys.argv[1],'wb').write(b'\x00'+'/Users/testuser/x'.encode('utf-16-le'))" "$T/paths/App.app/Contents/MacOS/app"
check "paths: UTF-16 at an odd offset -> FAIL" 1 bash "$S/check-macos-paths.sh" --forbid /Users/testuser "$T/paths"
printf 'abc /home/.cargo/registry/src/x.rs /jukeoke/crates' > "$T/paths/App.app/Contents/MacOS/app"
check "paths: only remapped paths -> pass" 0 bash "$S/check-macos-paths.sh" --forbid /Users/testuser --forbid /Users/testuser/src/jukeoke "$T/paths"
check "paths: a too-short forbidden path is refused" 2 bash "$S/check-macos-paths.sh" --forbid / "$T/paths"
check "paths: missing target -> usage error" 2 bash "$S/check-macos-paths.sh" --forbid /Users/testuser "$T/nothing-here"

# --- check-macos-availability.sh (fake SDK, fake nm/otool) -----------------------------
SDK="$T/MacOSX.sdk"
mkdir -p "$SDK/System/Library/Frameworks/CoreAudio.framework/Headers" "$SDK/System/Library/Frameworks/ScreenCaptureKit.framework/Headers"
echo '{"Version": "15.2"}' > "$SDK/SDKSettings.json"
cat > "$SDK/System/Library/Frameworks/CoreAudio.framework/Headers/AudioHardware.h" <<'EOF'
extern OSStatus AudioObjectGetPropertyData(AudioObjectID inObjectID) API_AVAILABLE(macos(10.4));
extern OSStatus
AudioHardwareCreateProcessTap(CATapDescription* __nullable inDescription,
                              AudioObjectID* __nullable outTapID) API_AVAILABLE(macos(14.2)) API_UNAVAILABLE(ios);
EOF
cat > "$SDK/System/Library/Frameworks/ScreenCaptureKit.framework/Headers/SCContentSharingPicker.h" <<'EOF'
API_AVAILABLE(macos(14.0))
@interface SCContentSharingPicker : NSObject
- (void)present;
@end
EOF
mkdir -p "$T/avail/Jukeoke.app/Contents/MacOS" "$T/bin"
printf '\317\372\355\376fake mach-o' > "$T/avail/Jukeoke.app/Contents/MacOS/jukeoke"
echo '<plist/>' > "$T/avail/Jukeoke.app/Contents/Info.plist"
cat > "$T/bin/nm-strong" <<'EOF'
#!/usr/bin/env bash
echo "                 (undefined) external _AudioObjectGetPropertyData (from CoreAudio)"
echo "                 (undefined) external _AudioHardwareCreateProcessTap (from CoreAudio)"
EOF
cat > "$T/bin/nm-weak" <<'EOF'
#!/usr/bin/env bash
echo "                 (undefined) external _AudioObjectGetPropertyData (from CoreAudio)"
echo "                 (undefined) weak external _AudioHardwareCreateProcessTap (from CoreAudio)"
EOF
cat > "$T/bin/nm-class" <<'EOF'
#!/usr/bin/env bash
echo "                 (undefined) external _OBJC_CLASS_\$_SCContentSharingPicker (from ScreenCaptureKit)"
EOF
printf '#!/usr/bin/env bash\nexit 0\n' > "$T/bin/nm-empty"
cat > "$T/bin/otool" <<'EOF'
#!/usr/bin/env bash
case "$*" in
    *__objc_selrefs*) echo "0000000100010000  __TEXT:__objc_methname:present" ;;
    *__cstring*) echo "0000000100020000  hello" ;;
esac
EOF
chmod +x "$T/bin/"*
A="$S/check-macos-availability.sh"
export JK_AVAIL_MIN_NEWER=1   # the fake SDK has 3 new declarations, a real one thousands
check "availability: strong import of a macOS 14.2 function -> FAIL" 1 env NM="$T/bin/nm-strong" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" --out "$T/av1" "$T/avail"
has "availability: the FAIL line names symbol and version" "FAIL +symbol +_AudioHardwareCreateProcessTap +macOS 14.2 +STRONG"
has "availability: the Info.plist is not treated as a binary" "1 file\(s\) checked"
check "availability: strong reference to a macOS 14.0 class -> FAIL" 1 env NM="$T/bin/nm-class" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" "$T/avail"
check "availability: only a weak import -> pass, listed as REVIEW" 0 env NM="$T/bin/nm-weak" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" "$T/avail"
has "availability: the weak import and the selector are REVIEW lines" "REVIEW +selector +present"
check "availability: same binary with --max 14.2 -> pass" 0 env JK_AVAIL_MIN_NEWER=0 NM="$T/bin/nm-strong" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --max 14.2 --sdk "$SDK" "$T/avail"
check "availability: imports unreadable -> error (2), never a pass" 2 env NM="$T/bin/nm-empty" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" "$T/avail"
mkdir -p "$T/avail-none"; echo text > "$T/avail-none/readme.txt"
check "availability: no Mach-O found -> error (2)" 2 env NM="$T/bin/nm-strong" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" "$T/avail-none"
rm "$SDK/System/Library/Frameworks/ScreenCaptureKit.framework/Headers/SCContentSharingPicker.h"
check "availability: parser misses a canary -> error (2), never a pass" 2 env NM="$T/bin/nm-weak" OTOOL="$T/bin/otool" DYLD_INFO=/nonexistent-tool bash "$A" --sdk "$SDK" "$T/avail"
has "availability: names the missing canary" "canary class SCContentSharingPicker"

# --- ci.yml ---------------------------------------------------------------------------
n=$((n + 1))
if grep -Eq '^[[:space:]]+JK_SKIP_[A-Z_]+:' "$REPO/.github/workflows/ci.yml"; then
    fail=$((fail + 1)); echo "FAIL $n - ci.yml sets a JK_SKIP_* variable"
else
    pass=$((pass + 1)); echo "ok   $n - ci.yml sets no JK_SKIP_* variable"
fi
n=$((n + 1))
# shellcheck disable=SC2016  # a literal $name is what ci.yml must contain
if grep -q 'ci-exfat-launch.sh "dist/Mac/$name.app"' "$REPO/.github/workflows/ci.yml" && ! grep -q 'app=""' "$REPO/.github/workflows/ci.yml"; then
    pass=$((pass + 1)); echo "ok   $n - ci.yml passes the expected bundle path to the exFAT test (no empty-path skip)"
else
    fail=$((fail + 1)); echo "FAIL $n - ci.yml can still pass an empty bundle path to the exFAT test"
fi

echo
echo "$pass passed, $fail failed ($n checks)"
[ "$fail" -eq 0 ]

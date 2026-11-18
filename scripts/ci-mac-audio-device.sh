#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-mac-audio-device.sh - CI ONLY. Make sure the macOS runner has an audio output device
# before the self-check and the ScreenCaptureKit probe run, and record what it has.
#
#   bash scripts/ci-mac-audio-device.sh [OUT_DIR]        (default: ci-out/audio)
#
# GitHub's Apple Silicon runner VMs may have no output device at all. Then the self-check's
# "Output device" and "Test tone" items fail for a reason that says nothing about the app.
# If (and only if) Core Audio lists no output device, this installs the BlackHole 2ch
# virtual device (GPL-3.0, Homebrew cask `blackhole-2ch`) and restarts coreaudiod, the
# usual way to give a CI Mac a sound device. It refuses to run outside GitHub Actions: on a
# real Mac nothing should be installed behind the owner's back.
#
# Never fails the job by itself: if no device can be had, the self-check reports it, and
# that failure is the honest result.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
out="${1:-ci-out/audio}"
mkdir -p "$out"

if [ "${GITHUB_ACTIONS:-}" != "true" ] || [ "${CI:-}" != "true" ]; then
    echo "refusing to run: this script installs a virtual audio driver and is meant only for GitHub-hosted runners" >&2
    exit 2
fi

has_output() {
    system_profiler SPAudioDataType 2>/dev/null > "$out/devices-$1.txt"
    grep -q "Output Channels" "$out/devices-$1.txt"
}

if has_output before; then
    echo "audio output device present:"
    grep -E "^        [^ ].*:$|Output Channels|Current SampleRate|Default Output Device" "$out/devices-before.txt" | head -n 20
    echo "audio: runner device (no install)" > "$out/SUMMARY.md"
    exit 0
fi
echo "::notice::the runner has no audio output device; installing BlackHole 2ch (virtual device) for the self-check"
if command -v brew >/dev/null 2>&1 && brew install --cask blackhole-2ch > "$out/brew.log" 2>&1; then
    sudo killall coreaudiod 2>/dev/null || sudo launchctl kickstart -k system/com.apple.audio.coreaudiod 2>/dev/null || true
    sleep 5
else
    echo "::warning::BlackHole could not be installed ($out/brew.log); the self-check's output items will fail"
fi
if has_output after; then
    echo "audio: no device on the runner; BlackHole 2ch installed for the test" > "$out/SUMMARY.md"
    echo "audio output device after install:"
    grep -E "^        [^ ].*:$|Output Channels|Current SampleRate" "$out/devices-after.txt" | head -n 20
else
    echo "audio: NO output device (BlackHole install did not help)" > "$out/SUMMARY.md"
    echo "::warning::still no audio output device; the self-check will report it"
fi
[ -n "${GITHUB_STEP_SUMMARY:-}" ] && { echo "### Audio device"; echo; sed 's/^/- /' "$out/SUMMARY.md"; echo; } >> "$GITHUB_STEP_SUMMARY"
exit 0

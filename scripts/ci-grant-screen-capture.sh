#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# ci-grant-screen-capture.sh - CI ONLY. Pre-approve the macOS "Screen Recording"
# permission (TCC service kTCCServiceScreenCapture) for one program on a
# GitHub-hosted macOS runner, so a ScreenCaptureKit capture can run unattended.
#
#   bash scripts/ci-grant-screen-capture.sh /path/to/binary
#   bash scripts/ci-grant-screen-capture.sh /path/to/Some.app
#
# NEVER run this on a real Mac. It edits the privacy database with sudo, which
# only works because GitHub's runner VMs have System Integrity Protection off
# (GitHub's own image build does the same: actions/runner-images,
# images/macos/scripts/build/configure-tccdb-macos.sh). The script refuses to
# run outside GitHub Actions. On the owner's MacBook the permission is granted
# the normal way, by the owner, in System Settings.
#
# What it does:
#   1. Inserts an "allowed" kTCCServiceScreenCapture row for the program into
#      the system and the user TCC.db (path client for a binary, bundle-id
#      client for an .app), using named columns so it works across schema
#      versions; csreq is left NULL, as GitHub's own entries do.
#   2. Writes a far-future approval into replayd's ScreenCaptureApprovals.plist
#      (macOS 15 otherwise shows a recurring "...is requesting to bypass the
#      system private window picker" alert), as GitHub does for its agent.
#   3. Restarts tccd and cfprefsd so the change is read.
#   4. Prints the resulting rows (evidence for the CI log).

set -euo pipefail

if [ "${GITHUB_ACTIONS:-}" != "true" ] || [ "${CI:-}" != "true" ]; then
    echo "refusing to run: this script edits macOS privacy databases and is meant only for GitHub-hosted CI runners" >&2
    exit 2
fi
target="${1:?usage: ci-grant-screen-capture.sh /path/to/binary-or-app}"
[ -e "$target" ] || { echo "no such file: $target" >&2; exit 2; }

case "$target" in
    *.app|*.app/)
        client="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$target/Contents/Info.plist")"
        client_type=0
        exe_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$target/Contents/Info.plist")"
        approval_key="$(cd "$target/Contents/MacOS" && pwd)/$exe_name"
        ;;
    *)
        client="$(cd "$(dirname "$target")" && pwd)/$(basename "$target")"
        client_type=1
        approval_key="$client"
        ;;
esac
now="$(date +%s)"
echo "granting kTCCServiceScreenCapture to $client (client_type $client_type)"
echo "SIP: $(csrutil status 2>&1 || true)"

sys_db="/Library/Application Support/com.apple.TCC/TCC.db"
user_db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sql="INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified) VALUES ('kTCCServiceScreenCapture', '$client', $client_type, 2, 0, 1, NULL, NULL, NULL, 'UNUSED', NULL, 0, $now);"
check="SELECT service, client, client_type, auth_value FROM access WHERE service = 'kTCCServiceScreenCapture';"

status=0
if sudo sqlite3 "$sys_db" "$sql"; then
    echo "system TCC.db: ok"
else
    echo "::warning::could not write the system TCC.db (SIP on?)"
    status=1
fi
mkdir -p "$(dirname "$user_db")"
if sqlite3 "$user_db" "$sql"; then
    echo "user TCC.db: ok"
else
    echo "::warning::could not write the user TCC.db"
    status=1
fi

approvals="$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"
mkdir -p "$(dirname "$approvals")"
defaults write "$approvals" "$approval_key" -date "3024-01-01 00:00:00 +0000" || echo "::warning::could not write $approvals"

# Make the daemons re-read their state.
sudo killall tccd 2>/dev/null || true
killall tccd 2>/dev/null || true
killall cfprefsd 2>/dev/null || true
sleep 2

echo "--- system TCC.db kTCCServiceScreenCapture rows"
sudo sqlite3 "$sys_db" "$check" || true
echo "--- user TCC.db kTCCServiceScreenCapture rows"
sqlite3 "$user_db" "$check" || true
exit "$status"

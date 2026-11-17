# SPDX-License-Identifier: GPL-3.0-or-later
"""C/Objective-C compiler shim for `cargo check --target aarch64-apple-darwin` on Windows.

cc-rs calls this (through zig-cc.cmd) like clang. We forward to Zig's bundled clang
(PyPI package `ziglang`) with Zig's spelling of the target, and drop the
Apple-toolchain flags Zig does not take (it brings its own target settings).
Only used to type-check the Mac build from Windows; the real Mac build uses Apple's
clang on macOS (CI or scripts/build-mac.sh).

Usage: python zig_cc.py <clang-style arguments...>     (or zig_cc.py ar <args...>)
"""

import subprocess
import sys

# Zig target for the owner's Mac: Apple Silicon, macOS 13.0 minimum (WORK_ORDER 4.10).
ZIG_TARGET = "aarch64-macos.13.0"


def clang_args(args):
    """Removes target/SDK flags that cc-rs adds for Apple's clang."""
    out = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg in ("-target", "-arch", "-isysroot"):
            skip_next = True
            continue
        if arg.startswith(("--target=", "-mmacosx-version-min=", "-mmacos-version-min=")):
            continue
        out.append(arg)
    return out


def main():
    args = sys.argv[1:]
    if args[:1] == ["ar"]:
        command = [sys.executable, "-m", "ziglang", "ar", *args[1:]]
    else:
        command = [sys.executable, "-m", "ziglang", "cc", "-target", ZIG_TARGET, *clang_args(args)]
    return subprocess.call(command)


if __name__ == "__main__":
    sys.exit(main())

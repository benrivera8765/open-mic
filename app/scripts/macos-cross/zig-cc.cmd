@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem C compiler for `cargo check --target aarch64-apple-darwin` on Windows (see zig_cc.py).
rem JK_TOOLS_PY is set by app/scripts/check-mac.sh to a Python that has `ziglang`.
"%JK_TOOLS_PY%" -I "%~dp0zig_cc.py" %*
exit /b %ERRORLEVEL%

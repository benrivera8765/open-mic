@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Archiver for `cargo check --target aarch64-apple-darwin` on Windows (see zig_cc.py).
"%JK_TOOLS_PY%" -I "%~dp0zig_cc.py" ar %*
exit /b %ERRORLEVEL%

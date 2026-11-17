# Source this in Git Bash to put the dev toolchains on PATH for this shell only.
# Nothing is changed system-wide.   Usage:  source scripts/devenv.sh
_LAD="$(cygpath -u "${LOCALAPPDATA:-$USERPROFILE/AppData/Local}")"
export PATH="$HOME/.cargo/bin:$_LAD/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.MSVCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin:$_LAD/Microsoft/WinGet/Packages/astral-sh.uv_Microsoft.Winget.Source_8wekyb3d8bbwe:/c/Program Files/GitHub CLI:$PATH"
export UV_PYTHON_PREFERENCE=only-managed
export UV_PYTHON_INSTALL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/jukeoke-build/python"
export JK_BUILD="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/jukeoke-build"
unset _LAD

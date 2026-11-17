# app/ - the Jukeoke desktop app (Tauri 2 shell)

```
app/
  src-tauri/            Rust crate `jukeoke` (Tauri 2): window, commands, portable mode
    src/lib.rs          builder, main window, app state
    src/portable.rs     finds the portable root, decides where data goes (+ unit tests)
    src/commands.rs     commands the UI calls (app_info, ping, smoke_report)
    src/smoke.rs        --smoke-test self-test
    src/screenshot.rs   saves what WebView2 rendered as PNG (Windows)
    src/fs_atomic.rs    atomic file writes (temp + fsync + rename)
    src/webview2_loader.rs + native/webview2_loader.c
                        built-in WebView2 loader for the GNU toolchain (see below)
    windows-app-manifest.xml   the exe's application manifest
    tauri.conf.json     Tauri config (literals Tauri forces, see "Naming")
    capabilities/       Tauri permissions of the main window
    icons/              generated from icons/source/app-icon.svg (`npm run icons`)
  ui/                   Vite + Preact + TypeScript web UI (plain placeholder page)
  scripts/              build and verification scripts (Git Bash)
```

## Build on Windows (GNU toolchain, no Visual Studio)

```bash
cd jukeoke && source scripts/devenv.sh
app/scripts/build-windows.sh            # UI + cargo build --release -p jukeoke + strip + DLL check
# result: $JK_BUILD/app-dist/Windows/Jukeoke.exe
```

By hand: `npm ci && npm run build` in `app/ui` (type-check, Vite build, and a check
that the emitted JS parses as ES2022, the Safari 16.4 baseline), then
`cargo build --release -p jukeoke`. The UI must be built first: the exe embeds
`app/ui/dist`. If it is missing, `build.rs` embeds a stub page and prints a warning,
so `cargo build --workspace` still works.

`cargo build` alone produces a finished app because the crate's default feature
`custom-protocol` embeds the UI. For live UI development use `npm run app:dev` in
`app/ui` (Tauri CLI, Vite dev server, `--no-default-features`).

### What the GNU target needed

- **WebView2 loader.** Tauri's WebView2 bindings import `WebView2Loader.dll` on the
  GNU target (MSVC builds link Microsoft's static loader instead). We link our own
  loader, `native/webview2_loader.c`, as a whole archive: it finds the installed
  WebView2 Runtime through the EdgeUpdate registry keys and calls the runtime's
  `CreateWebViewEnvironmentWithOptionsInternal`, as the MIT-licensed webview/webview
  project does. `Jukeoke.exe` therefore imports no `WebView2Loader.dll`. Safety net:
  if the built-in route finds no runtime, it uses a `WebView2Loader.dll` placed next
  to the exe (none is shipped). `tauri-build` still copies that DLL into
  `target/release/`; do not ship it.
- **Manifest.** MinGW's GCC links a `default-manifest.o` into every exe, which the
  GNU linker cannot merge with Tauri's manifest ("multiple non-default manifests").
  `build.rs` shadows it with an empty object via `-B`, and embeds our own
  `windows-app-manifest.xml` (Common Controls v6, asInvoker, Windows 10/11,
  per-monitor DPI v2, long paths).
- **Resource compiler.** `tauri-build` compiles the icon, version info and manifest
  with WinLibs' `windres`; nothing to configure.
- **Static runtime.** `.cargo/config.toml` sets `+crt-static`; the exe imports only
  Windows DLLs (msvcrt.dll is a Windows component). Checked by
  `app/scripts/check-windows-exe.sh`.

## Verify

```bash
app/scripts/check-windows-exe.sh "$JK_BUILD/app-dist/Windows/Jukeoke.exe"   # system DLLs only
app/scripts/portable-smoke-test.sh "$JK_BUILD/app-dist/Windows/Jukeoke.exe" # portable + subst + write audit
app/scripts/check-mac.sh                                                     # cargo check for aarch64-apple-darwin
cargo test -p jukeoke                                                        # portable-root rules etc.
(cd app/ui && npm test)
```

`Jukeoke.exe --smoke-test` opens the window, waits for the UI to call Rust after
its first contentful paint, saves `Settings/smoke-test.png` (what WebView2
rendered) and `Settings/smoke-test.json` (timings, WebView2 version, loader route,
data layout, checks), then exits with 0 (pass), 1 (a check failed), 2 (the UI, or
the screenshot step after it, did not finish within 60 s; `JK_SMOKE_TIMEOUT_MS`
overrides the limit) or 3 (report written but the app hung on exit). On macOS the
same flag works without the screenshot.

## Portable mode (docs/ARCHITECTURE.md section 8)

The app finds `<root>` from its own executable: `<root>/Windows/Jukeoke.exe` or
`<root>/Mac/Jukeoke.app`. It is portable when `<root>/Library/` or
`<root>/portable.txt` exists; a `portable.txt` directly next to the app also works.
Then everything goes under `<root>` and the WebView2 profile lives in
`<root>/Settings/webview-windows`. On macOS the web view is non-persistent
(incognito), so it keeps no profile at all. The folder names are checked on purpose:
an app in `~/Applications` must not mistake `~/Library` for a song library.

Not portable: with `$JK_BUILD` set (developer shell) data goes to
`$JK_BUILD/dev-data/`; otherwise to the per-user app data folder
(`%LOCALAPPDATA%\org.jukeoke.app`, `~/Library/Application Support/org.jukeoke.app`).

## Naming

`branding.json` is the source of truth. Tauri forces these literals, and
`build.rs` fails the build if they disagree with `branding.json`:

| File | Field | Must equal |
|---|---|---|
| `src-tauri/tauri.conf.json` | `productName` | `branding.json` `name` |
| `src-tauri/tauri.conf.json` | `mainBinaryName` | `branding.json` `name` |
| `src-tauri/tauri.conf.json` | `identifier` | `branding.json` `bundle_id` |

At run time the app reads its name from Tauri's package info (`productName`), never
from a literal. The page title is injected from `branding.json` by Vite, so
`ui/index.html` holds no name. Also mentioning the name, but not checked: the crate
name `jukeoke` (binary file name before packaging), `tauri.conf.json`
`bundle.publisher` / `shortDescription`, and the `.ico`/`.icns` artwork.

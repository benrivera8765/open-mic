# app/src-tauri - the Jukeoke app core (Rust, Tauri 2)

Implements `docs/APP_API.md` (commands and events) on top of the audio engine
(`crates/jk-engine`, contract in `crates/jk-engine/src/api.rs`), the song file
library (`crates/jk-format`) and the recorder (`crates/jk-recorder`), plus the Studio
supervisor (`docs/STUDIO_PROTOCOL.md`).

```
src/
  main.rs          two lines: registers the dialog plugin, calls lib::run
  lib.rs           start-up (layout, engine, window), --smoke-test, --selfcheck,
                   drag-and-drop -> studio://dropped, shut-down
  core.rs          Core: everything the commands use, testable without a window
  commands/        the Tauri commands (async, run on the blocking pool), one file per
                   APP_API area; thin wrappers around Core
  engine.rs        AudioEngine / Renderer traits over jk_engine (+ fakes for tests)
  playback.rs      song_load, player_*, playback://clock thread, per-song settings
                   saved to the song's slot (debounced 1 s, flushed on change/exit)
  library.rs       scan, SongSummary, cover thumbnails, file watcher
  export.rs        export_backing on a worker thread -> <root>/Exports/
  recorder.rs      recorder_* over jk-recorder, recordings_list
  studio.rs        Studio supervisor (spawn, JSON lines, timeouts, crashes, queue)
  selfcheck.rs     WORK_ORDER 4.10 self-check, report in Settings/
  settings.rs      <root>/Settings/settings.json (atomic, debounced, lenient load)
  keys.rs          "A major → B major (+2)"
  startup.rs       start-up failures: native dialog + Settings/logs/startup-error.txt
  portable.rs      where data lives (portable root, dev, user, macOS translocation)
  fs_atomic.rs     atomic writes (unique temp, fsync, write-through rename / dir fsync)
  sysinfo.rs       OS version, memory, free space, local time
  testsong.rs      generated test-tone song (self-check, tests)
  debounce.rs, events.rs, error.rs, applog.rs, smoke.rs, screenshot.rs, webview2_loader.rs
tests/
  fake_studio.py   a fake Studio (Python standard library only) speaking the protocol
  studio_supervisor.rs, core_api.rs, real_engine.rs, selfcheck_core.rs, watch_and_record.rs
resources/         bundled as Tauri resources (build.rs copies the demo song here)
```

## Running

```bash
source scripts/devenv.sh
export CARGO_TARGET_DIR="$JK_BUILD/target-appcore"
cargo build -p jukeoke                          # debug (stub UI page if app/ui/dist is missing)
cargo test -p jukeoke                           # unit + integration tests (fake Studio needs Python 3)
"$CARGO_TARGET_DIR/debug/jukeoke.exe" --smoke-test       # window + UI round trip, exits 0/1/2/3
"$CARGO_TARGET_DIR/debug/jukeoke.exe" --selfcheck --silent   # self-check, report in Settings/, exit 0/1
```

- `--selfcheck` runs the self-check with a hidden window (the page reports
  `navigator.userAgent`), writes `Settings/selfcheck-<local time>.txt`, prints the
  items and exits 0 (passed) or 1. `--silent` plays digital silence instead of the
  -60 dBFS test tone (use it whenever people are around).
- A release build (`cargo build --release`) fails when `app/ui/dist` is missing or
  holds the stub page; set `JK_ALLOW_UI_STUB=1` only for release builds that are never
  shipped.
- Developer Studio: with `$JK_BUILD` set the app starts
  `$JK_BUILD/studio-venv/Scripts/python.exe -m jukeoke_studio serve` with
  `PYTHONPATH=<repo>/studio`. To try the app against the fake Studio:
  `JK_STUDIO_PYTHON=python JK_STUDIO_SCRIPT=app/src-tauri/tests/fake_studio.py`
  (plus `FAKE_STUDIO_FIXTURE=tests/fixtures/generated/tiny.jksong`).
- Packaged Studio: `<root>/Windows/Studio/python/python.exe -I -X utf8 -m
  jukeoke_studio serve`, with `JUKEOKE_MODELS_DIR=<root>/Models` in its environment
  (the Studio's model downloader reads it; `hello` also carries `models_dir`).

## Where data goes (portable mode)

Everything stays under `<root>` (`Library/`, `Recordings/`, `Exports/`, `Models/`,
`Settings/` with `settings.json`, `logs/app.log`, `logs/studio.log`, the WebView2
profile `webview-windows/`, self-check reports). `WEBVIEW2_USER_DATA_FOLDER` is
removed from the app's environment at start, so it cannot move the profile off the
stick.

**The one exception (macOS only):** when macOS runs the app from a translocated copy
(an unsigned app opened from a quarantined folder), the stick is invisible. The UI
then shows `AppInfo.translocation_help` and calls `root_choose`, which opens a folder
picker; the chosen folder is remembered in
`~/Library/Application Support/org.jukeoke.app/root.txt` (one line, the folder's
path) and the app restarts. That file is the only thing the app ever writes outside
the stick. Until a folder is chosen, settings go to a temporary folder.

## Additions to docs/APP_API.md v1 (to be folded into the contract)

Commands: `export_cancel(job_id)`, `studio_clear_finished()`, `root_choose()`,
`selfcheck_run({ user_agent?, silent? })` (silent is new). Event: `studio://status`
(a full `studio_status()` payload when availability or the running state changes).
Fields: `AppInfo.identifier / mode / mode_reason / webview_version /
translocation_help`, `capabilities.studio.checking`,
`capabilities.recorder.permission_explanation / settings_url / backend`,
`PlayerState.clipped / device_starved`, `StudioStatus.checking`,
`StudioJob.error_code`, `export://done.applied_gain_db / peak`.
Argument names where the contract leaves them open: `metadata_save(song_id, metadata)`
(also accepts `changes` or the fields directly), `export_backing(song_id, format)`
(also accepts `options: { format }`), `studio_submit(job)` and
`studio_lyrics_lookup(query)` (also accept the fields directly),
`studio_models_status(preset?)` / `studio_models_download(preset?)` (default `full`).
`key_label` uses the arrow "→" (WORK_ORDER 4.5), not "->".

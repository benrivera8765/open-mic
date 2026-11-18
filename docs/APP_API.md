# App API: Rust core <-> web UI (version 1)

The UI talks to the Rust core only through these Tauri commands (`invoke`) and
events (`listen`). The UI never handles audio samples. TypeScript types live in
`app/ui/src/api/types.ts` and must match this document; the Rust side lives in
`app/src-tauri/src/commands/`. A mock backend (`app/ui/src/api/mock.ts`) implements
the same API in the browser so the UI can be developed and tested without Rust.

Conventions: command names are `snake_case`; arguments and results are JSON objects
with `snake_case` keys. Every Rust command is declared
`#[tauri::command(rename_all = "snake_case")]` (otherwise Tauri expects camelCase
argument names) and is `async`, running on Tauri's blocking pool, not the main thread.
Where this document writes an object as a positional parameter, the argument name it
travels under is given (for example `metadata_save(song_id, metadata)`); the UI keeps
those names in one place (`ARG_NAMES` in `app/ui/src/api/client.ts`). Times are seconds
(number). Errors reject the promise with `{ code: string, message: string }` where
`message` is plain language for the user. A song is identified by `song_id`: its path
relative to the Library folder, with `/` separators (stable across drive letters and
both OSes); an id with an empty, `.` or `..` component, a `\`, a `:` or a NUL, or
not naming a `.jksong` file, is refused with `not_found`.

Version 1 grew while Stage 1 was built: fields, commands and events marked **(added)**
are implemented by the core; the UI may use them when present. Section 11 lists them
all. Readers must ignore fields they do not know.

## 1. App and capabilities

`app_info() -> AppInfo`

```ts
interface AppInfo {
  name: string; version: string;                  // from branding / package info
  platform: "windows" | "macos";
  os_version: string;                              // "Windows 11 26200", "macOS 13.6.9 (<build>)"
  portable: boolean; root: string | null;          // root shown for information only
  translocated: boolean;                           // macOS App Translocation detected
  paths: { library: string; recordings: string; exports: string; settings: string; models: string | null };
  capabilities: {
    studio: { available: boolean; reasons: string[]; gpu: string | null; vram_mb: number | null;
              checking: boolean };                 // (added) true until the background probe answers
    recorder: { available: boolean; reason: string | null;
                permission: "granted" | "denied" | "not_determined" | "not_required";
                permission_explanation: string | null;  // (added) macOS: shown before the "Screen Recording" prompt
                settings_url: string | null;            // (added) macOS: where the user switches it on
                backend: string };                      // (added) "WASAPI loopback" | "ScreenCaptureKit"
    exclusive_mode: boolean;                       // Windows only
  };
  // (added)
  identifier: string;                              // bundle identifier from branding.json
  mode: "portable" | "dev" | "user" | "translocated";   // where data lives (ARCHITECTURE 8)
  mode_reason: string;                             // why that mode was chosen, plain text
  webview_version: string;                         // WebView2 runtime (Windows) or WebKit (macOS) version
  translocation_help: string | null;               // set only in mode "translocated": text to show before root_choose
}
```

`studio.available` is false on macOS, when the Studio runtime is missing next to
the app, or when the Studio probe says the computer cannot run it. `reasons` are
shown as they are. The probe runs once in the background after start-up; until it
answers, `checking` is true, `available` false and the reason says the check is still
running.

`root_choose() -> { ok: boolean; root: string | null; message: string }` **(added)**,
macOS translocation only (WORK_ORDER trap 2): shows a folder picker for the stick's
main folder (the one holding `Library`), remembers it in
`~/Library/Application Support/<identifier>/root.txt` and restarts the app. Rejects
with `not_needed` when the app is not translocated.

Internal commands, not for screens: `ping(text) -> { echo, uptimeMs }` (Stage 0 IPC
check) and `smoke_report(report)` (only accepted under `--smoke-test`; camelCase
payload kept from Stage 0).

## 2. Library

`library_list() -> { songs: SongSummary[]; free_bytes: number; total_bytes: number }`

```ts
interface SongSummary {
  song_id: string; title: string; artist: string; album: string | null;
  duration_s: number; key: { tonic: string; mode: "major" | "minor"; name: string } | null;
  tempo_bpm: number | null; has_lyrics: boolean; lyrics_checked: boolean;
  semitones: number;                               // saved key shift
  file_bytes: number; modified_ms: number; preset: string;
  error: string | null;                            // non-null: listed but unplayable (damaged, newer version...)
}
```

`song_cover(song_id, max_px?: number) -> { data_url: string | null }` (JPEG/PNG
thumbnail, cached in memory; `max_px` defaults to 256).

The scan is recursive, skips dotfiles, macOS `._*` files and unfinished `*.tmp-*`
writes, and caches summaries by (size, modification time). `free_bytes` /
`total_bytes` are for the volume holding the Library.

Event `library://changed` (no payload) when files appear, disappear or change
(including a folder moved away); a burst of changes gives one event about 400 ms after
its first change.

## 3. Loading and playback

`song_load(song_id) -> SongDetail` (stops any current song, opens the new one paused at 0)

```ts
interface SongDetail {
  summary: SongSummary;
  lyrics: Lyrics | null;                           // effective lyrics (override wins), SONG_FORMAT section 5
  lyrics_is_override: boolean;
  settings: SongSettings;                          // effective saved settings, SONG_FORMAT section 6
  stems: { role: string; in_karaoke_mix: boolean; pitched: boolean }[];
  sample_rate: number; channels: number;
}
interface SongSettings {
  semitones: number; shift_drums: boolean; guide_vocal: number;   // 0..1
  backing_on: boolean; backing_level: number; levels: { drums: number; other: number };
  lyrics_offset_ms: number;
}
```

`stems[].role` is `lead_vocal`, `backing_vocals`, `drums`, `other` or a newer role;
newer roles are played per SONG_FORMAT's forward-compatibility rule.

- `player_play()`, `player_pause()`, `player_toggle()`, `player_stop()`
- `player_seek(position_s: number)`
- `player_set(changes: Partial<SongSettings>) -> SongSettings` - applies live
  (click-free) and saves to the song file's settings slot (debounced about 1 s, flushed
  on song change and exit). Returns the effective settings after the change. Values are
  clamped: `semitones` -12..12 (whole numbers), `guide_vocal` 0..1, `backing_level` and
  `levels.*` 0..2 (+6 dB), `lyrics_offset_ms` -10000..10000; an unknown key or a wrongly
  typed value rejects with `bad_request`; with no song loaded it rejects with
  `not_loaded`. A change while paused is heard from the first frame after play.
- `player_set_master(volume: number)` - 0..1, app setting, not saved in the song.
- `player_state() -> PlayerState`
- `player_diagnostics() -> Diagnostics | null` (added 2026-11-18, for soak tests and support):
  the engine's counters and timings, `jk_engine::engine::Diagnostics` serialised as is
  (`underruns`, `device_starved`, `min_lead_ms` = lowest render-ahead seen while playing,
  `max_callback_us`, `worker_max_block_ms`, `worker_busy_s`, `inline_constructions`,
  `cache_misses`...). Field names may change between versions; the UI does not use it.

```ts
interface PlayerState {
  state: "empty" | "loading" | "paused" | "playing" | "ended" | "error";
  song_id: string | null; position_s: number; duration_s: number;
  device: { name: string; sample_rate: number; channels: number; exclusive: boolean; latency_ms: number } | null;
  underruns: number; error: string | null;
  key_label: string | null;                        // "A major → B major (+2)" (arrow as in WORK_ORDER 4.5)
  // (added)
  clipped: boolean;          // output went above full scale since the last load / mix / master change (nothing limits it)
  device_starved: number;    // WASAPI device-side starvation events (OS scheduling glitches)
  output_peak_db: number[];  // per channel, last ~0.3 s, master included, dBFS; -120 = silence
  key_mode: "realtime" | "prerender";   // how key changes are made now (WORKING ORDER 4.5)
  key_preparing: { semitones: number; reason: "key" | "mix"; progress: number } | null;
                             // prerender: a key (or, same key, a pitched-stem mix) being rendered
                             // ahead; the previous key/mix plays until then. progress 0..1
  key_note: string | null;   // prerender asked for but unavailable (no cache folder, disk error):
                             // plain words; key changes then use the real-time path
}
```

Key change modes (WORK_ORDER 4.5, ARCHITECTURE section 4 "render ahead and cache"):
`realtime` shifts in the render worker (heard about 0.3 s after the change).
`prerender` renders the song's pitch path ahead into `<Settings>/cache/keys`; the
previous key keeps playing until enough is rendered (5 s ahead with a renderer faster
than 1.1x real time, otherwise to the end of the song), then the same 40 ms crossfade
switches. In prerender mode the drum level stays live (unless "shift drums" is on);
any other level change is rendered like a key (`reason: "mix"`). The UI shows
"Preparing key +2… 40%" from `key_preparing`; `progress` can dip by about 1 % between
cache cells, so the UI shows the highest figure seen for the same `semitones` and
`reason` and starts again when either changes or `key_preparing` is null (D-211); within 1 s after a UI seek, which restarts `progress` from the new position, a drop of more than 2 points is shown as it is (D-215). After the song has ended, a new key is
prepared from the start (where `player_play` restarts) and `key_preparing` clears as
soon as it is ready there; the switch happens when play starts.

`key_label` is "+2 semitones" / "original key" when the song has no detected key.
`device.exclusive` is the mode actually open (exclusive may have fallen back to
shared). `device` is null when no output device exists; `player_play` then rejects
with `no_device`.

Events:

- `playback://clock` about 30 times per second while playing (absolute 33.3 ms
  deadlines; and on every state change, checked 4 times a second while not playing):
  `{ position_s: number; playing: boolean; seq: number }`.
  `position_s` is the song time of the sample now leaving the DAC (device latency
  already subtracted), extrapolated from the engine's last device callback to the
  moment the event is sent (at most 100 ms). The UI interpolates between events with
  `performance.now()` and re-anchors on each event (ARCHITECTURE section 4); the lyric
  offsets are applied by the UI:
  `lyric_time = position_s - (song.lyrics_offset_ms + device_offset_ms) / 1000`, where
  `device_offset_ms` is `settings.audio.device_offsets_ms[device_id ?? "default"]`.
- `playback://state`: a full `PlayerState`. Sent on a state or song change, and when
  `key_mode`, `key_note` or `key_preparing` changes (progress in whole percent).
- `playback://error`: `{ code, message }` (device lost, file unreadable...).

## 4. Output devices

`devices_list() -> { devices: Device[] }` with
`Device { id: string; name: string; is_default: boolean; sample_rate: number; channels: number; exclusive_capable: boolean }`.

`device_set(id: string | null, exclusive?: boolean)` - null (or "") = system default;
`exclusive` defaults to false. Saved in app settings. Changing device during playback
pauses, switches, and resumes. If the device refuses exclusive mode it opens in shared
mode, the choice is still saved, and the call rejects with `exclusive_unavailable` so
the UI can say so; any other failure reopens the previous device and is not saved.
The ids `null` and `null:<rate>` name a silent device that consumes audio in real time
(tests and the headless self-check; not listed by `devices_list`).

## 5. App settings

`settings_get() -> AppSettings`, `settings_set(changes: Partial<AppSettings>) -> AppSettings`
(deep-merged, saved atomically to `<root>/Settings/settings.json`).

```ts
interface AppSettings {
  ui: {
    skin: boolean; bubbles: boolean; motion: "system" | "reduce" | "full";
    lyrics_text_size: number;          // CSS px at 1440x900, scaled with the window
    marker_position: number;           // 0..1 from the left, default 0.33
    colors: { sung: string; upcoming: string; marker: string };
    library_sort: "title" | "artist" | "recent" | "key"; fullscreen_stage: boolean;
  };
  audio: { device_id: string | null; exclusive: boolean; master_volume: number;
           device_offsets_ms: Record<string, number>;     // Bluetooth latency per device id ("default" = system default)
           key_mode: "auto" | "realtime" | "prerender";   // key change, see section 3
           key_cache_mb: number;                          // key cache size cap (LRU), MB
           key_benchmarks: Record<string, number> };      // written by the core: Rubber Band
                                                          // real-time factor per "<computer>@<rate>"
}
```

Defaults (core and UI agree): `skin` true, `bubbles` true, `motion` "system" (follows
the OS reduce-motion preference), `lyrics_text_size` 56, `marker_position` 0.33,
`colors` sung `#ff3d8b`, upcoming `#f6efd9`, marker `#7dffd8`, `library_sort` "title",
`fullscreen_stage` false; `device_id` null, `exclusive` false, `master_volume` 1.0
(unity), `device_offsets_ms` {}, `key_mode` "auto", `key_cache_mb` 1024,
`key_benchmarks` {}. Numbers are clamped (`lyrics_text_size` 12..400,
`marker_position` 0.05..0.95, `master_volume` 0..1, offsets -5000..5000 ms,
`key_cache_mb` 64..65536; benchmarks must be positive, at most 32 kept). `key_mode`
"auto" measures Rubber Band once per computer and sample rate (about 0.5 s, in the
background at start-up for 44.1 and 48 kHz, otherwise at the first song of that rate),
saves the factor in `key_benchmarks` (the computer part of the key is a hash, not its
name) and uses `prerender` below 3x real time or while nothing is measured yet. A value
of the wrong type, or an enum value this version does not know, falls back to its
default on its own; unknown keys are kept, so a newer app sharing the stick loses
nothing. Saves are debounced about 300 ms; an unreadable file is kept as
`settings.json.bad`. Prefer `device_set` for device changes (it handles refusals).

## 6. Sync editor and metadata

- `lyrics_save(song_id, lyrics: Lyrics)` - stores `lyrics_override` in the song's slot.
- `lyrics_reset(song_id)` - removes the override. There is no command that returns
  the original lyrics; the UI reloads the song (`song_load`) to get them.
- `metadata_save(song_id, metadata: { title?, artist?, album? })` - stores
  `metadata_override`. The object travels as `metadata`; the core also accepts it as
  `changes` or as top-level `title` / `artist` / `album`.
- `studio_realign(song_id, text: string, anchors?: { line: number; start: number }[]) -> Lyrics`
  (Windows with the Studio only; `line` is the 0-based index into the `lines` array of
  the lyrics produced from `text`). The result is also stored as the song's
  `lyrics_override`.

A pending `player_set` for the song is written first. When an edit no longer fits the
settings slot, the core rewrites the whole song file atomically (the engine releases
the song meanwhile).

## 7. Export backing track

`export_backing(song_id, { format: "flac24" | "wav32f" }) -> { job_id: string }`

Renders the current key and mix (the song's settings, guide vocal included at its
current level) offline with the same DSP as playback, at the song's sample rate, to
`<root>/Exports/<artist> - <title> (<key>).flac|wav`. Events
`export://progress { job_id, progress }`,
`export://done { job_id, path, applied_gain_db, peak }`,
`export://failed { job_id, code, message }`.

- `applied_gain_db` (number, dB, 0 or negative): the gain applied to the whole file.
  `wav32f` is never scaled (always 0). A float render can exceed full scale and a
  24-bit FLAC cannot hold that, so a `flac24` export whose render peaked above full
  scale is lowered by one constant, just enough to fit (no limiting, no normalising),
  and this says by how much. WORK_ORDER 5 forbids hidden processing: whenever it is
  below 0 the UI tells the user, e.g. "The mix peaked above full scale, so the FLAC
  file was lowered by 5.8 dB to avoid clipping. Choose 32-bit float WAV to keep the
  exact level."
- `peak` (number): the render's largest |sample| before any gain (linear, 1.0 = full
  scale), for logs and diagnostics.

## 8. Recorder

- `recorder_status() -> RecorderStatus` (the level thread's latest reading; polling
  does not reset the meter)
- `recorder_start() -> RecorderStatus` (returns once the OS stream runs, up to 20 s;
  rejects with `busy` while a recording runs), `recorder_stop() -> Recording`
- `recorder_request_permission() -> { permission }` (macOS: shows the system prompt
  after the UI has shown `capabilities.recorder.permission_explanation`; returns the
  resulting permission state)
- `recordings_list() -> { recordings: Recording[] }` (newest first)

Recordings are `<root>/Recordings/Recording <local time>.wav` (32-bit float WAV, RF64
above 4 GiB). The recording spans the `recorder_start` and `recorder_stop` calls:
silence the OS does not deliver is filled from timestamps, so the length matches the
wall clock. `elapsed_s` is wall time since start. `clipped` for a finished recording is
kept in `Settings/recordings-index.json`, since a WAV header cannot hold it.

```ts
interface RecorderStatus {
  state: "idle" | "starting" | "recording" | "stopping" | "error";
  elapsed_s: number; format: { sample_rate: number; channels: number; sample_format: "f32" | "i16" | "i24" | "i32"; note: string } | null;
  peak_db: number[]; clipped: boolean; dropped_s: number; error: string | null;
}
interface Recording { path: string; name: string; duration_s: number; sample_rate: number; channels: number; bytes: number; clipped: boolean }
```

Event `recorder://level` about 20 times per second while recording: a `RecorderStatus`.

## 9. Studio (Windows with a capable GPU)

- `studio_status() -> StudioStatus`
- `studio_inspect(path: string) -> InspectResult` (STUDIO_PROTOCOL section 5)
- `studio_lyrics_lookup(query: { artist, title, album, duration_s }) -> { results: LookupResult[] }`
  (the object travels as `query`; the core also accepts its fields at top level)
- `studio_submit(job: { input: string; preset: "full" | "clean" | "permissive"; language: string; metadata: {...}; lyrics: {...} }) -> { job_id }`
  (the object travels as `job`; the core also accepts its fields at top level, with
  defaults `preset` "full", `language` "en", `lyrics` `{ mode: "auto" }`). Job ids are
  chosen by the app. `lyrics` is `{ mode, text, lrc, source? }` and is passed to the
  Studio unchanged; the UI sets `source` to `"online:<service>"` when the text or LRC came
  from `studio_lyrics_lookup` (STUDIO_PROTOCOL section 7), so the song records where its
  lyrics came from. `metadata` is `{ title, artist, album, cover_path? }`, also passed on
  unchanged: `cover_path` absent keeps the file's own cover, `null` removes it, a path to
  a JPEG / PNG picture replaces it (STUDIO_PROTOCOL section 7).
- `studio_cancel(job_id)`
- `studio_clear_finished()` **(added)** - removes done, failed and cancelled jobs from
  the queue.
- `studio_models_status(preset?)`, `studio_models_download(preset?)` - `preset`
  defaults to "full"; the results are the Studio's `models.status` /
  `models.download` results (STUDIO_PROTOCOL section 4) without the envelope. The UI
  always passes the preset chosen in the Studio form (2026-11-18, D-209), so "clean" and
  "permissive" get their own models; `studio_models_download` resolves when the download
  has finished. `StudioStatus.models_present` is about the default preset only: asking
  about another preset never changes it, and after every download the core asks again
  for the default preset (a "permissive" download does not bring the default models).
- `pick_files() -> string[]` (native file dialog: audio and video types) and drag-and-drop
  of files onto the window (Tauri drag-drop event `studio://dropped { paths }`).
- `pick_cover_image() -> string | null` **(added)** - native file dialog for one JPEG or
  PNG picture (the Studio form's "Replace cover"); null when cancelled.
- `studio_cover_preview(path: string, max_px?: number) -> { data_url }` **(added)** - a
  JPEG or PNG picture file (the inspected `cover_path`, or a picked picture) as a
  thumbnail `data:` URL, at most `max_px` (default 256, 16..1024) on its longer side;
  `bad_request` for anything that is not a JPEG or PNG picture (or over 64 MB),
  `not_found` for a missing file.

```ts
interface StudioStatus { available: boolean; reasons: string[]; running: boolean; models_present: boolean;
  queue: StudioJob[];
  checking: boolean }                              // (added) as capabilities.studio.checking
interface StudioJob { job_id: string; input_name: string; state: "queued" | "running" | "done" | "failed" | "cancelled";
  stage: string | null; overall: number; message: string | null; output_song_id: string | null; error: string | null;
  stage_progress: number | null;                   // (added) progress of the current stage, 0..1
  error_code: string | null }                      // (added) e.g. "studio_crashed", "decode_failed"
```

Events: `studio://job` (a `StudioJob` on every change), `studio://models { name, done_bytes, total_bytes }`,
`studio://status` **(added)**: a full `StudioStatus` when availability or the running
state changes (per-job changes come as `studio://job`).

The Studio process is started on demand and stopped after 10 minutes idle. A Studio
that does not answer is a `timeout` error, never a hang (60 s for start-up and the
first probe). If it crashes, the running job fails with `error_code`
"studio_crashed" and the waiting jobs are sent to a restarted Studio; after 3 crashes
within 10 minutes the waiting jobs fail with a plain explanation (also
"studio_crashed") instead of restarting in a loop.

## 10. Self-check (WORK_ORDER 4.10)

`selfcheck_run(user_agent?: string, silent?: boolean) -> { report_path: string; passed: boolean; items: { name: string; ok: boolean; detail: string }[] }`

Plays a test tone, loads and renders the bundled test song, shifts its key, checks the
recorder permission and makes a 3-second capture, and records OS version, web engine
version (the UI passes `navigator.userAgent` as `user_agent`), memory and output
device format. Items, in order: System, Web engine, Memory, Output device, Test tone,
Recorder, Demo song, Key change. The test tone peaks at exactly -60 dBFS;
`silent: true` **(added)** plays digital silence instead (use it whenever people are
around). The user's song is unloaded during the check and reloaded at the same
position afterwards. Writes a plain-text report to
`<root>/Settings/selfcheck-<local time>.txt`. The app also accepts
`--selfcheck [--silent]` on the command line (hidden window, prints the items, exits
0 when all pass, else 1), for CI.

`selfcheck_user_agent(user_agent)` **(added, internal)**: in `--selfcheck` runs the
page reports `navigator.userAgent` through this command; screens do not call it.

## 11. Additions to version 1 (Stage 1, 2026-11-17)

Everything below is implemented by the core (`app/src-tauri`) and is optional for a
reader: the UI uses a field when present and degrades when it is absent.

| Section | Addition |
|---|---|
| Conventions | Argument names for object parameters: `metadata_save(song_id, metadata)`, `export_backing(song_id, options: { format })`, `studio_lyrics_lookup(query)`, `studio_submit(job)`, `selfcheck_run(user_agent, silent)`. The core also accepts the flat forms noted in each section. |
| 1 | `AppInfo.identifier`, `mode`, `mode_reason`, `webview_version`, `translocation_help`; `capabilities.studio.checking`; `capabilities.recorder.permission_explanation`, `settings_url`, `backend`. Command `root_choose()`. Internal: `ping`, `smoke_report`. |
| 3 | `player_set` returns the effective `SongSettings`. `PlayerState.clipped`, `device_starved`, `output_peak_db`. `key_label` uses "→". |
| 3 | 2026-11-18: `player_diagnostics()` (engine counters for soak tests; not used by the UI). |
| 3, 5 | Key change render-ahead fallback (WORK_ORDER 4.5): `PlayerState.key_mode`, `key_preparing`, `key_note`; settings `audio.key_mode`, `key_cache_mb`, `key_benchmarks`; `playback://state` also on key-preparation changes. |
| 4 | `device_set`'s `exclusive` is optional (false); refusal of exclusive mode rejects with `exclusive_unavailable` after opening shared. |
| 7 | `export_cancel(job_id)`. `export_backing` takes the format as `format` or inside `options`. `export://done` also carries `applied_gain_db` (negative when a Flac24 export was turned down to avoid clipping; 0 otherwise) and `peak` (largest absolute sample of the render before any export gain, linear). |
| 8 | `recorder_start` returns a `RecorderStatus`; `recorder_request_permission` returns `{ permission }`. |
| 9 | `StudioStatus.checking`; `StudioJob.stage_progress`, `error_code`; `studio_clear_finished()`; `studio_models_status(preset?)` / `studio_models_download(preset?)`; event `studio://status`. 2026-11-18: `pick_cover_image()`, `studio_cover_preview(path, max_px?)`, `metadata.cover_path` in `studio_submit`. |
| 10 | `selfcheck_run`'s `silent`; internal `selfcheck_user_agent`. |

The UI also uses the Tauri window API directly, outside this contract: full screen
(`setFullscreen`, needs `core:window:allow-set-fullscreen` in
`app/src-tauri/capabilities/default.json`) and the minimised state (`isMinimized` on
resize, in `core:default`), which pauses the skin's animation and the lyrics ticker's
frame loop because a web view does not always report a minimised window as a hidden
page (WebView2 does not).

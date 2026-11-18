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
  paths: { library: string; recordings: string; exports: string; settings: string; models: string | null;
           deleted: string };                      // (added 2026-11-18) Deleted songs, section 2.1
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

`library_list() -> { songs: SongSummary[]; free_bytes: number; total_bytes: number; deleted_count: number; deleted_bytes: number }`

`deleted_count` / `deleted_bytes` **(added 2026-11-18)**: the songs in Deleted songs (below)
and the bytes they still take on the drive; `free_bytes` counts them as used until they are
deleted for good.

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
its first change. The core also sends it after every delete, restore and purge below
(the free space and the Deleted songs figures changed).

### 2.1 Deleting songs (added 2026-11-18, the owner's request; DECISIONS D-230)

Files deleted from a USB stick bypass the Windows Recycle Bin, so the app keeps its own:
deleting a song **moves** its `.jksong` file (one self-contained file) out of `Library/`
into `<root>/Deleted/` (portable mode; otherwise the data root's `Deleted/`), from where it
is restored or deleted for good. `Deleted/` is created on the first delete, is never
scanned as part of the Library, and holds `index.json` (written atomically): each song's
original `song_id`, title, artist, album, length, size and deletion time. Studio outputs,
recordings and exports are never touched.

- `library_delete(song_ids: string[]) -> DeleteResult` (a single `song_id` is accepted
  too). Ids that are not songs in the Library come back in `failed` (`not_found`); an
  empty list rejects with `bad_request`. If the loaded song is one of them it is stopped
  and closed first (the engine releases every handle on the file, then its pending
  `player_set` save is written into it), and `unloaded` names it; `player_state` is then
  `empty`. A file another program holds open is not moved (`in_use`, plain message).
- `library_deleted_list() -> DeletedList`, newest first.
- `library_restore(deleted_ids: string[]) -> RestoreResult` (or one `deleted_id`): back to
  the original place (missing folders are made again), or, when that place is taken now,
  to a free name next to it (`Title (2).jksong`; names are compared without letter case).
- `library_purge(deleted_ids: string[]) | library_purge(all: true) -> PurgeResult`:
  deletes for good. Cannot be undone; the UI asks a second time, saying so.

```ts
interface DeletedSong {
  deleted_id: string;          // its file name in Deleted/ ("Title.jksong", "Title (2).jksong" on a clash)
  original_song_id: string;    // where it was
  restore_to: string;          // the song_id a restore gives it now (the original, or a free name next to it)
  title: string; artist: string; album: string | null; duration_s: number;
  file_bytes: number; deleted_ms: number;   // ms since 1970, UTC
}
interface DeletedList { songs: DeletedSong[]; count: number; total_bytes: number; folder: string }
interface DeleteFailure { id: string; code: string; message: string }   // id: a song_id or a deleted_id
interface DeleteResult { deleted: DeletedSong[]; failed: DeleteFailure[]; unloaded: string | null }
interface RestoreResult { restored: { deleted_id: string; song_id: string }[]; failed: DeleteFailure[] }
interface PurgeResult { purged: string[]; freed_bytes: number; failed: DeleteFailure[] }
```

Event `library://deleted`: a full `DeletedList` after every delete, restore or purge.

Moves: a rename on the same volume (never replacing a file). If `Deleted/` is on another
volume than the song (a Library folder linked elsewhere), the file is copied to a
temporary name, flushed, read back and compared byte for byte, renamed into place, and
only then is the original removed; if the original cannot be removed the copy is taken out
again, so a song is never in two places or in none. The index entry is written, marked
with the operation in progress, before a file moves, and settled after; the core repairs
an interrupted delete or restore at the next start (or the next call): a move that did not
happen is forgotten (the song is still in the Library), a finished one is settled, an
interrupted copy is undone, a song file in `Deleted/` without an entry is listed (its
original place is then the Library's top folder), and a leftover temporary copy older than
an hour is removed. An unreadable `index.json` is kept as `index.json.bad`.

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
    lyrics_motion: "word" | "letter";  // pace of the wipe / ticker (D-216): smooth, paced by word | exact, letter by letter
    lyrics_display: "page" | "scroll"; // D-218: static lines with a colour wipe | the scrolling ticker
    show_next_line: boolean;           // page display: preview the next line underneath
    show_marker: boolean;              // page display: the progress needle on the wipe edge (D-219)
    colors: { sung: string; upcoming: string; marker: string };  // marker: the ticker's marker and the page display's needle
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
`lyrics_motion` "word" (the owner's choice of 18 Nov 2026; a settings file written
before the setting existed gets "word" too; "letter" is the letter-exact motion every
earlier version had), `lyrics_display` "page" (the owner's choice of 18 Nov 2026, D-218;
also for files written before it existed; "scroll" is the ticker of every earlier
version), `show_next_line` true, `show_marker` true (the owner's request of 18 Nov 2026,
D-219; also for files written before it existed), `colors` sung `#ffc21a` (gold), upcoming `#f4f4f4`
(white), marker `#7dffd8` (before D-218: sung `#ff3d8b`, upcoming `#f6efd9`; a file
without `ui.lyrics_display` that still holds exactly one of those two values, in any
letter case, gets the new default instead, so the owner's untouched colours follow the
new look and chosen ones stay; once saved, the file has `lyrics_display` and an old colour
chosen again is kept), `marker_position` applies to the ticker only, `colors.marker` to
the ticker's marker and to the page display's progress needle (`show_marker`; the ticker
always draws its marker),
`library_sort` "title", `fullscreen_stage` false; `device_id` null, `exclusive` false, `master_volume` 1.0
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

### 8.1 The media player: "Record & play" and "Record songs separately" (added 2026-11-18)

The owner's request (DECISIONS D-221): start the music player together with the
recording, and record several songs back to back, kept apart for the Studio. Windows
only (the system's media sessions, `Windows.Media.Control`); on macOS
`recorder_media_sessions` answers `available: false` with the reason and the other
commands reject with `player_unavailable`. The player is generic: any app that shows in
Windows' media controls (a music app, a browser playing audio).

- `recorder_media_sessions() -> MediaPlayers` - the players Windows lists, read only
  (nothing is played, paused or changed). The UI asks every 2.5 s while the Recorder
  screen is open.
- `recorder_record_and_play(player_id) -> RecorderStatus` - starts the capture exactly
  like `recorder_start`, waits until its first buffer has arrived (at most 3 s), then
  asks the player to play, so the song's first note is inside the recording. Rejects
  with `player_not_found` (with a plain message that offers the plain Record button)
  when the player is not open. If the player refuses "play", the recording keeps
  running and `RecorderStatus.note` says so.
- `recorder_stop(pause_player?: boolean) -> Recording` - `pause_player` (default false)
  also pauses the player after "Record & play" ("Stop & pause": the capture stops first).
  `Recording.note` says if the pause was refused. Rejects with `session_running` while
  songs are recorded separately.
- `recorder_session_start(options: SessionStartOptions) -> RecorderStatus` (argument name
  `options`; a bare `player_id` is accepted too) - "Record songs separately": one
  continuous capture into `<root>/Recordings/<YYYY-MM-DD HH-MM-SS>/master.wav`, started
  before the player is told to play (if it is not playing and `start_player` is not
  false). Every track change and play / pause / stop of that player becomes a marker on
  the capture's timeline, saved at once to `session.json` with the cover picture. The
  session stops by itself after the player has been paused or stopped for
  `auto_stop_idle_s` (default 30; 0 = never), or when the song after `max_songs` starts
  (then the player is paused unless `pause_player_at_end` is false), or the player is
  closed. Rejects with `busy` while any recording runs.
- `recorder_session_stop(pause_player?: boolean) -> RecordingSession` - stops following the
  player, stops the capture, pauses the player (default true), and splits the master in
  the background (`recorder://session` events with `progress`).
- `recorder_sessions_list() -> { sessions: RecordingSession[] }`, newest first.
- `recorder_session_split(folder) -> RecordingSession` - splits a session again
  (`kept_master`, `failed`, `stopped`, or an `interrupted` one). Rejects with
  `nothing_to_split` when the master was already deleted after a verified split.
- `recorder_preview(path, start_s?, max_s?)` / `recorder_preview_stop()` - plays up to
  `max_s` (default 30) seconds of a recording through the default output device,
  natively (the web view never plays audio); only files inside the Recordings folder
  (`bad_request` otherwise, `not_found` for a missing file). One preview at a time.

```ts
interface MediaPlayers { available: boolean; reason: string | null; players: MediaPlayer[] }
interface MediaPlayer { id: string; source_app_id: string; app_name: string; is_current: boolean;
  status: "closed" | "opened" | "changing" | "stopped" | "playing" | "paused" | "unknown";
  title: string; artist: string; album: string; duration_s: number | null; can_play: boolean; can_pause: boolean }
interface SessionStartOptions { player_id: string; auto_stop_idle_s?: number; max_songs?: number | null;
  start_player?: boolean; pause_player_at_end?: boolean }
// RecorderStatus additions: mode ("plain" | "record_and_play" | "session"; "plain" while idle),
// player ({ id, app_name } | null), session (LiveSession | null), note (string | null).
interface LiveSession { folder: string; path: string; current: { title: string; artist: string; album: string } | null;
  playback: string; songs: number; auto_stop_idle_s: number; max_songs: number | null; auto_stop_in_s: number | null }
interface RecordingSession { folder: string; path: string; created: string; player: string; player_id: string;
  state: "recording" | "stopped" | "splitting" | "split" | "kept_master" | "failed" | "interrupted";
  progress: number | null; stop_reason: "user" | "idle" | "max_songs" | "player_closed" | "interrupted" | "app_closed" | null;
  message: string | null; master_present: boolean; song_count: number; songs: RecordedSong[]; excluded: ExcludedPart[] }
interface RecordedSong { path: string; name: string; number: number; title: string; artist: string; album: string;
  cover_path: string | null; duration_s: number; track_duration_s: number | null;
  status: "complete" | "partial" | "unknown"; partial_reason: string | null;
  start_confidence: "certain" | "uncertain" | "edge"; end_confidence: "certain" | "uncertain" | "edge"; paused_s: number }
interface ExcludedPart { path: string; name: string; reason: string; duration_s: number }
```

Events: `recorder://session` (a `RecordingSession` when a session starts, stops, makes
split progress or finishes splitting), `recorder://preview` (`{ path, playing }`).

What a split produces (jk-recorder `session`, DECISIONS D-224..D-227): one float WAV per
song, `<NN> <Artist> - <Title>.wav` (names safe on Windows, macOS and exFAT), a sidecar
`<NN> <Artist> - <Title>.json` (`format: "jukeoke-recorded-song"`: title, artist, album,
cover, recorded and track length, complete / partial / unknown, how each end was cut,
the player) and the cover (`.jpg` / `.png`); advertisements, what came before the first
song and after playback ended, and skips under 5 s go to `_excluded/`. The cuts copy
samples (the song files and the excluded parts, joined in order, are the master bit for
bit); the master is deleted only when every file was read back and matched. A session a
crash or a closed app left behind is finished when the app starts next.

Studio hand-off: the UI queues each song with `studio_submit` and
`metadata: { title, artist, album, cover_path }` from the sidecar. `studio_inspect` of
a recorded song fills the title, artist, album and `cover_path` the WAV cannot hold from
its sidecar and adds `recorded: { status, partial_reason, track_duration_s,
source_player }`; `studio_submit` without a `cover_path` key uses the sidecar's cover
(the recording's "own" cover; `null` still removes it).

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
| 2 | 2026-11-18 (owner's request, section 2.1): `library_delete`, `library_deleted_list`, `library_restore`, `library_purge`; `LibraryList.deleted_count`, `deleted_bytes`; event `library://deleted`; `library://changed` also after those commands. Section 1: `AppInfo.paths.deleted`. |
| 3 | 2026-11-18: `player_diagnostics()` (engine counters for soak tests; not used by the UI). |
| 3, 5 | Key change render-ahead fallback (WORK_ORDER 4.5): `PlayerState.key_mode`, `key_preparing`, `key_note`; settings `audio.key_mode`, `key_cache_mb`, `key_benchmarks`; `playback://state` also on key-preparation changes. |
| 4 | `device_set`'s `exclusive` is optional (false); refusal of exclusive mode rejects with `exclusive_unavailable` after opening shared. |
| 5 | 2026-11-18: `ui.lyrics_motion` ("word" default, "letter"), the ticker's pace (D-216). A reader without it behaves like "letter". |
| 7 | `export_cancel(job_id)`. `export_backing` takes the format as `format` or inside `options`. `export://done` also carries `applied_gain_db` (negative when a Flac24 export was turned down to avoid clipping; 0 otherwise) and `peak` (largest absolute sample of the render before any export gain, linear). |
| 8 | `recorder_start` returns a `RecorderStatus`; `recorder_request_permission` returns `{ permission }`. |
| 8 | 2026-11-18 (owner's request, section 8.1): `recorder_media_sessions`, `recorder_record_and_play`, `recorder_stop(pause_player?)`, `recorder_session_start(options)`, `recorder_session_stop(pause_player?)`, `recorder_sessions_list`, `recorder_session_split(folder)`, `recorder_preview`, `recorder_preview_stop`; `RecorderStatus.mode`, `player`, `session`, `note`; `Recording.note`; events `recorder://session`, `recorder://preview`. |
| 9 | 2026-11-18: `studio_inspect` fills a recorded song's empty title / artist / album / `cover_path` from its sidecar and adds `recorded`; `studio_submit` without `metadata.cover_path` uses the sidecar's cover. |
| 9 | `StudioStatus.checking`; `StudioJob.stage_progress`, `error_code`; `studio_clear_finished()`; `studio_models_status(preset?)` / `studio_models_download(preset?)`; event `studio://status`. 2026-11-18: `pick_cover_image()`, `studio_cover_preview(path, max_px?)`, `metadata.cover_path` in `studio_submit`. |
| 10 | `selfcheck_run`'s `silent`; internal `selfcheck_user_agent`. |

The UI also uses the Tauri window API directly, outside this contract: full screen
(`setFullscreen`, needs `core:window:allow-set-fullscreen` in
`app/src-tauri/capabilities/default.json`) and the minimised state (`isMinimized` on
resize, in `core:default`), which pauses the skin's animation and the lyrics ticker's
frame loop because a web view does not always report a minimised window as a hidden
page (WebView2 does not).

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
    microphone: MicCapability;                     // (added WO2) practice mode's input, section 12.4
    practice: { guide_method: string; guide_version: string };  // (added WO2) this app's guide builder
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
  // (added WO2, practice mode)
  guide: GuideStatus;                              // the melody guide: none / queued / building / ready / ... (section 12.3)
  practice_best: { overall: number | null; grade: string | null; date: string } | null;  // from the slot (section 12.6)
  source: { file_name: string; seconds: number } | null;  // (added 2026-11-25, D-281) the file the Studio made the song from (manifest provenance.source: its name and length); the Recorder marks a recording with the same name and length as made
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
  guide: GuideStatus;                              // (added WO2) as in the summary, section 12.3
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
  // (added WO2, D-239) the key HEARD: practice mode draws and grades with it, never with the request
  key_heard: number;         // semitones sounding at the DAC now, i.e. at position_s (-12..12)
  key_requested: number;     // the key last asked for (= settings.semitones)
  key_pending: { semitones: number; at_s: number | null } | null;
                             // the next change of the key heard: from song time at_s on. at_s null:
                             // not scheduled yet (render ahead still rendering; the old key plays on)
  key_segment: number;       // playback segment position_s and key_heard belong to (new at every load,
                             // seek, device switch and paused re-render)
  key_heard_label: string | null;     // as key_label, for the key heard ("A major → F# major (-3)")
  key_spelling: "sharps" | "flats";   // how to name notes in the key heard; agrees with key_heard_label
  key_pending_spelling: "sharps" | "flats" | null;   // the same for key_pending
}
```

`key_label` stays the label of the key asked for (the key dial's). `key_heard` changes at
the midpoint of the key change's 40 ms crossfade, measured within 0.02 ms of the pitch
change in the produced audio, in both key modes (docs/benchmarks/practice-key.md). The
spelling agrees with the key name the app shows (`keys.rs` spells the tonic: Db, Eb, Ab,
Bb major and Eb, Bb minor with flats, F# major and C#, F#, G# minor with sharps): notes
are spelled with flats in keys whose signature has flats (F, Bb, Eb, Ab, Db major; D, G,
C, F, Bb, Eb minor) and with sharps in every other key; a song with no detected key uses
sharps. A C4 heard at -3 is named A3 (WO2 4.6).

`player_key_at(position_s: number, segment?: number) -> { semitones: number | null }`
**(added WO2)**: the key that was heard at song time `position_s` of `segment` (default: the
newest segment that played it), from the engine's bounded history (the last 64 segments);
`null` when unknown. The UI does not need it to draw (`key_pending.at_s` and the practice
batches' per-note key cover that, section 12.5); the screen checks and tests use it.

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
  `{ position_s: number; playing: boolean; seq: number }`, and **(added WO2)**
  `key_heard: number`, `key_pending: { semitones: number; at_s: number | null } | null`
  and `segment: number`, all belonging to that `position_s` (as in `PlayerState`), so the
  lane moves its bars when the key that is heard changes, at the right song time.
  `position_s` is the song time of the sample now leaving the DAC (device latency
  already subtracted), extrapolated from the engine's last device callback to the
  moment the event is sent (at most 100 ms); `key_heard` and `key_pending` are taken at that
  extrapolated position too (a pending change whose `at_s` it has passed is reported as
  heard, with no `key_pending`; the same in `PlayerState`; Stage 3b, docs/benchmarks/
  practice-app.md section 6). The UI interpolates between events with
  `performance.now()` and re-anchors on each event (ARCHITECTURE section 4); the lyric
  offsets are applied by the UI:
  `lyric_time = position_s - (song.lyrics_offset_ms + device_offset_ms) / 1000`, where
  `device_offset_ms` is `settings.audio.device_offsets_ms[device_id ?? "default"]`.
- `playback://state`: a full `PlayerState`. Sent on a state or song change, and when
  `key_mode`, `key_note` or `key_preparing` changes (progress in whole percent), and
  **(added WO2)** when `key_heard`, `key_requested`, `key_pending` or `key_segment` changes.
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
  practice: PracticeAppSettings;     // (added WO2) per computer where it matters, section 12.7
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
  error_code: string | null;                       // (added) e.g. "studio_crashed", "decode_failed"
  guide: GuideStatus | null }                      // (added WO2) the app's guide stage, below
```

**(added WO2)** After the Studio's `job.done` the app runs a last stage of its own,
`stage: "guide"` (STUDIO_PROTOCOL section 12): the job stays `running` while the melody
guide is built and written into the new song (`overall` 0.95..1.0, `stage_progress` from
the build, `message` "Making the practice data"), then becomes `done` with `guide` set.
`output_song_id` is set from `job.done` on, so the song can be listed and played during
the stage. A failed guide never fails the job: the job is `done`, `guide.state` is
`failed` with a plain `reason` and `error_code`, the song plays, and Practice offers to try
again. The Studio may start the next job meanwhile, so two jobs can be `running`, one of
them in `guide`. `studio_cancel` on a job in its `guide` stage cancels only the guide
(the song stays, without a guide). Songs whose guide stage was interrupted by quitting
are finished at the next start (`Settings/guide-queue.json`).

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

`selfcheck_run(user_agent?: string, silent?: boolean, lane?: LaneCost) -> { report_path: string; passed: boolean; items: { name: string; ok: boolean; detail: string }[] }`
(`lane` **added WO2**, section 10.1)

Plays a test tone, loads and renders the bundled test song, shifts its key, checks the
recorder permission and makes a 3-second capture, and records OS version, web engine
version (the UI passes `navigator.userAgent` as `user_agent`), memory and output
device format. Items, in order: System, Web engine, Memory, Output device, Test tone,
Recorder, Demo song, Key change, then **(added WO2)** the six practice items of section
10.1. The test tone peaks at exactly -60 dBFS;
`silent: true` **(added)** plays digital silence instead (use it whenever people are
around). The user's song is unloaded during the check and reloaded at the same
position afterwards. Writes a plain-text report to
`<root>/Settings/selfcheck-<local time>.txt`. The app also accepts
`--selfcheck [--silent]` on the command line (hidden window, prints the items, exits
0 when all pass, else 1), for CI.

`selfcheck_user_agent(user_agent)` **(added, internal)**: in `--selfcheck` runs the
page reports `navigator.userAgent` through this command; screens do not call it.

### 10.1 Practice additions (added WO2, section 4.8)

Six items follow "Key change", in this order; none of them makes a sound, and the
microphone capture never writes sound anywhere:

| Item | What it does and reports |
|---|---|
| Microphone permission | `mic_permission()`: granted / denied / not determined / restricted, the Windows privacy switches. It never asks: in a `--selfcheck` run, or when the UI did not ask first, "not determined" is reported as it is. |
| Microphone capture | 3 s from the chosen (or automatic) input, analysed in memory and dropped: device, kind, format (rate, channels, sample format, declared latency), per-channel peak, RMS and noise floor, the suggested voice channel, dropped blocks. Skipped with the reason when there is no permission or no input (not a failure on a computer without a microphone: `ok` with "no input"). |
| Pitch tracker | Generated tones (C2..C6) and a generated voice through the real analysis path (`InputSource` generator -> rings -> channel -> tracker -> song-time mapping): cents error, octave errors, delay, and its speed on this computer (ms per second of audio). |
| Practice guide | The bundled demo song's guide read and checked (counts, provenance), and a guide built again from its lead stem: same notes, and the builder's speed on this computer. |
| Practice score | A generated singer on the demo melody through the whole practice path scores in the top band; the same singer a semitone flat in the lowest band; silence scores nothing. |
| Practice lane | The lane's drawing cost in this web view: average and 95th-percentile ms per frame over 600 frames of the demo song at the window's size, plain and with the simple lane (WO2 4.5 budget: 4 ms average with the lyrics). Measured by the UI, which passes it as `selfcheck_run(user_agent, silent, lane)` with `lane: { frames, avg_ms, p95_ms, simple_avg_ms, width, height, dpr }`; in `--selfcheck` runs the page reports it through `selfcheck_lane(lane)` (internal). Missing: "not measured". |

### 10.2 The practice rig check (added WO2; audible, started by hand only)

`practice_rig_check(options: RigCheckOptions) -> RigCheckReport` (argument name `options`).
The one check that makes sound: the owner starts it from the Settings screen with the
mixer plugged in and the in-ears on. The UI labels it "Makes a sound" and says what will
happen first; the call rejects with `audible_not_confirmed` unless `audible_confirmed` is
true. It is never part of `selfcheck_run`, `--selfcheck` or any automated test (tests
drive the same code on the `null` device with a generated input, so nothing plays aloud).
Playback is paused first and practice input is suspended.

```ts
interface RigCheckOptions { audible_confirmed: boolean;
  output_device_id?: string | null; input_device_id?: string | null;  // default: the current ones
  label?: string;                    // the owner's note for this run, e.g. "loopback on"
  voice_step?: boolean }             // default true: ask the owner to speak for 3 s
interface RigCheckReport { report_path: string; ok: boolean; label: string | null;
  output: { name: string; kind: string; sample_rate: number; channels: number; format: string; latency_ms: number | null };
  input:  { name: string; kind: string; sample_rate: number; channels: number; format: string; declared_latency_ms: number | null;
            same_device_as_output: boolean };
  channels: { index: number; noise_floor_db: number; voice_rms_db: number | null; test_rms_db: number | null;
              carries: "voice" | "test_sound" | "both" | "nothing" }[];
  returned: { detected: boolean; channel: number | null; level_db: number | null;   // relative to what was played
              delay_ms: number | null; delay_spread_ms: number | null };            // over 3 bursts: fixed (cable, USB) or wandering;
                                                                                // delay_ms beyond the declared latencies (12.7 "Timing")
  findings: string[] }               // plain sentences, e.g. "The mixer sends the app's music back on channel 1 and 2, 23 ms late."
```

Steps (event `practice://rigcheck { step, progress, message }` for each): `silence` (2 s:
noise floor per channel), `voice` (3 s: "Say a few words into your microphone now"; which
channel carries the voice), `test_sound` (three 0.5 s bursts of a swept tone and noise,
peaking at -24 dBFS, through the chosen output at the app's master volume), `analysing`,
`done`. It writes `<root>/Settings/selfcheck-rig-<local time>.txt` (picked up with the
other `selfcheck-*.txt` reports): both devices and their formats, which channels carry
signal, levels and noise floors, whether the test sound came back, how loud and how late,
and the label. No sound is written. A measured delay is also saved for the timing
(`practice.measured_latency_ms`, section 12.7). Running it twice with different labels
(for example the mixer's loopback switch on, then off) appends both to the same report
file within one app session.

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
| 1, 2, 3 | 2026-11-19, Work Order 2 (practice mode): `capabilities.microphone`, `capabilities.practice`; `SongSummary.guide`, `practice_best`; `SongDetail.guide`; `PlayerState.key_heard`, `key_requested`, `key_pending`, `key_segment`, `key_heard_label`, `key_spelling`, `key_pending_spelling`; `playback://clock` `key_heard`, `key_pending`, `segment`; `playback://state` on key-heard changes; `player_key_at`. |
| 5, 9, 10 | WO2: `AppSettings.practice`; `StudioJob.guide` and the app's `guide` stage; self-check practice items, `selfcheck_run`'s `lane`, internal `selfcheck_lane`; `practice_rig_check` and `practice://rigcheck`. |
| 12 | WO2: guides (`guide_status`, `guide_build`, `guide_build_all`, `guide_cancel`, `guide_queue`; `guide://status`, `guide://batch`), microphone (`mic_permission`, `mic_request_permission`, `mic_open_settings`, `mic_list`, `mic_choose`, `mic_meter`; `mic://levels`, `mic://permission`, `mic://devices`), practice (`practice_start`, `practice_stop`, `practice_state`, `practice_guide`, `practice_suspend_input`, `practice_finish`, `practice_again`, `practice_song_set`, `practice_exclude`, `practice_history`, `practice_latency_set`, internal `practice_target_at` and `practice_test_input`; `practice://state`, `practice://live`, `practice://result`); `AppSettings.practice` (12.7); error codes (12.8). |

The UI also uses the Tauri window API directly, outside this contract: full screen
(`setFullscreen`, needs `core:window:allow-set-fullscreen` in
`app/src-tauri/capabilities/default.json`) and the minimised state (`isMinimized` on
resize, in `core:default`), which pauses the skin's animation and the lyrics ticker's
frame loop because a web view does not always report a minimised window as a hidden
page (WebView2 does not).

## 12. Practice mode (added WO2, 2026-11-19)

Work Order 2: the singer sings into a microphone while the singing screen shows a pitch
lane (the guide's notes as bars, a piano keyboard, a marker at the pitch sung) and the
lyrics; at the end the take is graded. Practice is a **mode of the singing screen**, not a
second player: transport, key dial, mixer, `player_*` and `playback://*` keep working as
in section 3, and with practice off nothing in this section runs. Everything here runs on
Windows and on the Mac (the guide builder, the tracker, the echo handling and the scorer
are the same Rust code, `jk-pitch`); only the Studio's own stages are Windows-only.

Rules every command below keeps:

- **The UI only draws.** Tracking, echo detection and removal, the octave decision and
  scoring run in the core on the practice analysis thread (ARCHITECTURE section 12); the
  UI gets small time-stamped batches and draws them.
- **The microphone is open only while practice mode is on screen** (between
  `practice_start` and `practice_stop`, and while the input picker's meters run, 12.4),
  never while the window is hidden (`practice_suspend_input`), never for anything else.
  Its sound is analysed in memory and dropped: never written, never sent, never played.
- **Playback is untouched.** Nothing here feeds the output; the engine's output is
  bit-identical with practice on or off.
- **Nothing is kept in web storage.** Per-song data is in the song's slot (SONG_FORMAT
  section 11), per-computer data in `settings.json` (12.7).

### 12.1 Pitch, key and octave: one rule for drawing and grading

Every reader (lane, piano keys, note names, scorer, screen checks) uses this rule, so
"everything moves together" (WO2 4.6):

```
target(note, t) = note.midi + key_at(t) + 12 * octave_shift     // MIDI with fractions, never rounded
lane(m)         = m - guide.tuning.use_cents / 100                // vertical position in the lane
note name       = name(round(lane(target)), spelling)             // "A3"; spelling: sharps | flats
singer(t)       = 69 + 12 * log2(hz / 440)                        // drawn at lane(singer(t))
error_cents     = 100 * (singer(t) - target(note, t))             // octave-folded unless strict
```

- `key_at(t)`: the key **heard** at song time `t` (section 3, D-239). For the lane: the
  clock's `key_heard` before `key_pending.at_s` and `key_pending.semitones` from `at_s`
  on (with `at_s` null the old key stays). For grading: the core's `key_at` history, so a
  note already sung keeps the key it was sung in. Never the requested key.
- `octave_shift`: the practice state's `octave.shift` (12.5). `auto` finds it from the
  first sung phrases, `fixed` takes the song's setting, `strict` is 0. In `auto` and
  `fixed` a pitch a whole number of octaves from the target counts as on the target
  (`error_cents` folded into -600..600); in `strict` it does not.
- The bars, the curve, the piano rows and the marker all go through `lane()`, so the bars
  sit on the key rows whatever the recording's tuning, and the marker sits on a bar
  exactly when the singer matches the recording. Grading compares unrounded pitches in
  cents; nothing is snapped to a piano key.
- `spelling` is `PlayerState.key_spelling` (section 3) for the key heard; `name()` is
  `jk-pitch`'s note namer, mirrored in the UI and tested against it for every key
  -12..12 (a C4 at -3 is named A3).

`practice_target_at(position_s: number, segment?: number) -> { note: number | null; midi: number | null; lane: number | null; name: string | null; key: number | null; octave: number; graded: boolean }`
(internal, for the screen checks and tests): the scorer's target at that song time, by
the rule above.

`practice_test_input(source: TestInput | null) -> { ok: boolean }`
(internal, accepted only under `--smoke-test` or the internal `--practice-test` flag;
refused with `bad_request` otherwise. **Stage 3b:** `--practice-test` was added because the
smoke test's watchdog ends the app within a minute, too short for the 20-minute run): the
next `practice_start` takes its input from **generated** sound instead of the microphone,
through the same `jk_recorder::input::InputSource` path, rings, meters and analysis thread,
at real-time pace and stamped with the host clock; `null` returns to the real input and
clears every simulation. `TestInput` (src/practice/mod.rs, src/practice/testsource.rs):

```ts
type TestInput =
  | { kind: "wav"; path: string;      // a WAV of generated sound (any rate, any channels)
      sync?: boolean;                 // default true: a singer who sings along: sample k is song time k / rate,
                                      // following the song heard (seeks, pauses); false: plays from practice_start
      late_ms?: number;               // sync: the singer is this late (negative: early)
      unplug_after_s?: number }       // reports an unplugged device after this long (WO2 9 "unplugged")
  | { kind: "silence" }               // digital silence, 48 kHz mono
  | { kind: "return"; delay_ms?: number; level_db?: number; voice?: string }
                                      // a mixer's loopback: the app's own playback (the output tap) back
                                      // delay_ms late at level_db, plus an optional generated singer WAV
  | { kind: "permission_denied" | "not_determined" | "permission_granted" }   // simulated permission
  | { kind: "no_device" };            // simulated "no input device at all"
```

A WAV with `sync: false` that ends behaves like an unplugged device (`input_lost`). This is
how WO2 section 9's real-app runs are fed (the 20-minute run with the generated singer,
the input unplugged during a song, the music coming back); microphone sound is never
written, so no recorded file ever goes in here.

### 12.2 Lifecycle

```
practice_start --> guide usable? --no--> "preparing" (built on the spot, progress) --+
     |                  | yes                  | failed --> "not_ready" (reason, retry)
     |                  v                      v
     |        microphone usable? --no--> "watch_only" (notes and keys, no marker, no grade)
     |                  | yes
     |                  v
     |        "listening": the marker follows the singer; a take runs while the song plays
     |                  | song ends / practice_finish / practice_stop (enough sung)
     |                  v
     |        "result" (practice://result, saved in the song's slot) --practice_again--> "listening"
     v
practice_stop: microphone closed, practice off (the song stays loaded where it was)
```

A **take** starts at the first `player_play` after `practice_start`, `practice_again` or
a finished take, or at once when the song is already playing at that moment (Practice
pressed in the middle of a playing song: the take covers from there on), and ends when the
song ends, on `practice_finish()`, or when practice or
the song is left (`practice_stop`, `song_load` of another song, `library_delete`). Pausing
does not end a take. A seek does not end it either: a note sung again keeps its latest
attempt, and the result counts the seeks. A take ended early is graded on what was sung
and says how much that was (`stopped_early`); a take that covered less than 10 % of the
song's graded note time is still graded and shown when the singer pressed Finish (WO2 4.7:
"grade what was sung"), but it is not saved in the song's history, and a take that short
ended by `practice_stop`, a song change or a delete gives no result at all. While paused or before the first
play the marker still shows the pitch sung (the singer can find the note) and nothing is
graded.

### 12.3 Guides

```ts
interface GuideStatus {
  state: "none" | "queued" | "building" | "ready" | "outdated" | "stale" | "newer" | "failed";
  reason: string | null;           // plain words for none / stale / newer / failed ("Practice data could not be
                                   // saved: the stick is full. Free 150 MB and press Try again.")
  error_code: string | null;       // failed: guide_no_lead | guide_read_failed | guide_damaged | guide_disk_full |
                                   // guide_read_only | guide_in_use | guide_internal | cancelled (STUDIO_PROTOCOL 12.2)
  progress: number | null;         // building: 0..1
  in_memory: boolean;              // usable now, but not (yet) in the file: it could not be written (reason says
                                   // why: used for this session only), or the write waits (pending_write)
  pending_write: boolean;          // the swap into the file waits until the song stops playing
  method: string | null; version: string | null; created_utc: string | null;   // provenance (SONG_FORMAT 10.2)
  counts: { notes: number; pitched: number; unpitched: number; low_confidence: number;
            octave_flags: number; graded: number } | null;
  tuning_cents: number | null;     // tuning.use_cents
}
```

States: `none` (no guide; Practice adds one on the spot), `queued` / `building` (in the
guide queue), `ready`, `outdated` (made by an older builder or another method: used as it
is; "Update practice data" or `guide_build_all({ include_outdated: true })` rebuilds it),
`stale` (the lead vocal stem changed after the guide was made: not used, rebuilt like
`none`), `newer` (made by a newer Jukeoke, `guide_version` above 1: kept, never
overwritten; practice in this app uses a guide built in memory for the session),
`failed` (the last attempt failed; the song still plays; Practice says why and offers to
try again). A song with no lead vocal stem gets `failed` / `guide_no_lead` ("This song
has no separate lead vocal, so there is no melody to practise").

- `guide_status(song_id) -> GuideStatus`.
- `guide_build(song_id, force?: boolean) -> GuideStatus`: queues a build (`force`: rebuild
  a `ready` guide too). Returns at once (`queued` or `building`); progress comes as
  `guide://status`. A song already queued or building gets its status, not a second
  build. The guide is built from the file's own `lead_vocal` stem and lyrics (no Studio,
  so it works on the Mac) and written by the atomic rewrite of SONG_FORMAT 7.1, which
  keeps every other byte. **The loaded song** (the delete feature's release path, D-231 /
  D-233): when it is not playing, the core releases it (pending slot save flushed),
  renames, and loads it again at the same position and play state (about 0.15 s;
  `playback://state` shows `loading` for that moment). When it is playing, the swap waits
  until it is paused, stopped, replaced or the app quits, so playback never stutters for
  a guide; meanwhile the guide is used from memory (`in_memory` and `pending_write`
  true). During a take nothing is swapped until the take has ended.
- `guide_build_all(options?: { include_outdated?: boolean }) -> { queued: number; skipped: number }`
  ("Add practice data to all songs"): queues every song whose guide is `none`, `stale` or
  `failed` (and `outdated` when asked); damaged songs are skipped.
- `guide_cancel(song_id?: string)`: cancels one queued or running build, or all
  (`song_id` absent). A cancelled build leaves the file as it was.
- `guide_queue() -> { items: { song_id: string; state: "queued" | "building"; progress: number | null }[]; running: boolean }`.

One build at a time, never on an audio thread (about 1.2 s of CPU for a 4-minute song,
plus one copy of the file): below-normal priority for the batch and for new songs from the
Studio, normal priority for a one-song `guide_build` (the singer pressed Practice and waits
at the progress display: at below normal, a busy computer kept that wait above a minute;
at normal it took 2.3 s under the same load. Stage 3b, docs/benchmarks/practice-app.md). The queue survives a restart
(`Settings/guide-queue.json`, STUDIO_PROTOCOL 12.2). New songs from the Studio enter it
automatically (STUDIO_PROTOCOL section 12; `StudioJob.guide`, section 9).

Events: `guide://status { song_id: string; status: GuideStatus }` on every change
(progress at most 5 times a second); `guide://batch { total: number; done: number; failed: number; current: string | null; running: boolean }`
while a `guide_build_all` batch runs, and once when it ends. `library://changed` follows
each rewrite as for any file change.

### 12.4 Microphone

```ts
interface MicCapability {
  available: boolean;              // false: no input device at all, or capture unsupported here
  reason: string | null;           // plain words when not available
  permission: MicPermission;
  backend: "WASAPI" | "Core Audio";
}
interface MicPermission {
  state: "granted" | "denied" | "not_determined" | "restricted";
  explanation: string;             // the app's own words, shown BEFORE the OS asks (MIC_PERMISSION_EXPLANATION)
  steps: string | null;            // denied / restricted: the exact steps to allow it on this system
  settings_url: string | null;     // what mic_open_settings opens (macOS Privacy > Microphone; ms-settings:privacy-microphone)
  windows: { microphone_access: "allow" | "deny" | "unset";      // Windows privacy switches: read only, never changed
             user_access: "allow" | "deny" | "unset";
             desktop_apps: "allow" | "deny" | "unset" } | null;   // null on macOS
}
interface MicInput {
  id: string; name: string; adapter: string | null;
  kind: "usb" | "built_in" | "wired_external" | "bluetooth" | "display" | "virtual" | "unknown";
  channels: number; sample_rate: number; sample_format: string | null;
  is_default: boolean;             // the system's default input
  bluetooth: boolean;              // kind bluetooth, or a hands-free endpoint
  preferred: boolean;              // the automatic choice: USB > wired external > built-in > unknown > display;
                                   // never Bluetooth or virtual automatically (WO2 trap 2)
  declared_latency_ms: number | null;
  same_device_as_output: boolean;  // also the current output device (a USB mixer used both ways: one clock)
  warning: string | null;          // e.g. Bluetooth: "Opening a Bluetooth headset's microphone turns its sound into
                                   // low-quality mono. Use a cable or the mixer's USB connection for practice."
}
interface InputChoice { device_id: string | null; device_name: string | null; channel: number | null }   // channel 0-based
interface ChannelMeter { index: number; peak_db: number; rms_db: number; noise_floor_db: number | null;
  active_db: number | null; clipping: boolean; clipped_samples: number;
  advice: "good" | "waiting" | "too_quiet" | "clipping" | "silent"; message: string | null }
```

- `mic_permission() -> MicPermission` (never asks). `mic_request_permission() -> MicPermission`:
  macOS asks the system (its prompt appears only the first time; the UI calls this only
  after showing `explanation` with a Continue button), returns at once, and the answer
  arrives as `mic://permission` (the core polls every 0.5 s for up to 2 minutes). Windows
  asks nothing and returns the switches. `mic_open_settings()` opens `settings_url`; the
  app never changes system settings.
- `mic_list() -> { inputs: MicInput[]; chosen: InputChoice; automatic: string | null }`:
  every input device, the saved choice for this computer (`chosen.device_id` null = none
  yet) and the device the automatic rule would take now. Listing never opens a device and
  never activates a Bluetooth endpoint.
- `mic_choose(choice: InputChoice) -> InputChoice`: saves the choice **for this computer**
  in `settings.json` (`practice.inputs`, 12.7; never in a song), found again by id, then by
  name. `device_id` null returns to the automatic choice; `channel` null = the suggested
  voice channel (the one carrying sound, never a sum). If practice is listening, the
  input is reopened on the new device or channel at once. A Bluetooth device is accepted
  when chosen by hand, with its `warning`, and the take gets the `bluetooth_input` flag.
- `mic_meter(device_id: string | null) -> { device: MicInput | null; error: InputError | null }`:
  opens a device for the input picker's per-channel meters only (no tracking), closing any
  meter opened before; `mic_meter(null)` closes it. Only while the input picker is on
  screen; `practice_stop` and a hidden window close it too.
- Failures are `InputError { kind: "no_device" | "unplugged" | "permission_denied" | "stalled" | "format" | "device"; message: string }`
  (`jk_recorder::input::FailureKind`), in plain words with what to do.

Events: `mic://levels { device_id: string; seconds: number; channels: ChannelMeter[]; suggested_channel: number | null; chosen_channel: number | null }`
about 20 times a second while a meter or the practice input is open (each channel metered
on its own; channels are never added together); `mic://permission`: a `MicPermission` on
every change; `mic://devices` (no payload) when inputs appear or disappear.

The app never changes the input gain (that is set on the mixer), never plays the
microphone back, and never uses a system voice-processing mode (Windows raw stream,
category Other; macOS HAL unit without Apple's voice processing: D-240).

### 12.5 Practice session

`practice_start(options?: PracticeStartOptions) -> PracticeState` (argument name `options`).

```ts
interface PracticeStartOptions {
  song_id?: string;                // default: the loaded song; another song is loaded first (paused at 0)
  watch_only?: boolean;            // open without the microphone
}
interface PracticeState {
  active: boolean; song_id: string | null; seq: number;
  phase: "off" | "preparing" | "not_ready" | "watch_only" | "listening" | "result";
  guide: GuideStatus;
  not_ready: { code: string; message: string; can_retry: boolean } | null;   // retry = guide_build(song_id)
  watch_only: { code: "chosen" | "no_device" | "permission_denied" | "not_determined" | "restricted" | "input_lost";
                message: string } | null;
  input: PracticeInput | null;
  music: MusicInInput;
  octave: { mode: "auto" | "fixed" | "strict"; shift: number; settled: boolean };   // settled: auto has decided
  strictness: "relaxed" | "standard" | "strict";
  latency: { applied_ms: number;   // taken off capture times on top of the latencies the devices declare
             source: "returned_music" | "rig_check" | "manual" | "none";   // manual: the input's offset + the output
                                                                         // device's offset (12.7 "Timing")
             measured_ms: number | null; manual_ms: number };
  take: { running: boolean; started_s: number;
          covered: number;          // share of the song's graded note time covered so far
          graded: number; total: number; overall: number | null;   // running score of the notes finished so far
          seeks: number; key_changes: number } | null;
  excluded: { line: number | null; start: number; end: number }[];
  last_result: PracticeResult | null;
}
interface PracticeInput { device: MicInput; channel: number; sample_rate: number;
  state: "starting" | "running" | "unplugged" | "stalled" | "failed" | "suspended";
  message: string | null;          // plain words: too quiet, clipping, unplugged, what to do
  meter: ChannelMeter | null; dropped_blocks: number }
interface MusicInInput {
  presence: "searching" | "absent" | "present";   // the app's own playback coming back in the input
  level_db: number | null;         // how loud it comes back, relative to what the app plays
  delay_ms: number | null;         // beyond the declared device latencies (12.7 "Timing")
  stability: "unknown" | "stable" | "wandering"; drift_ppm: number | null;
  removal: "not_needed" | "checking" | "good" | "marginal" | "unusable";
  suppression_db: number | null;   // measured removal
  grading: "normal" | "less_certain" | "not_graded";
  message: string | null;          // present: plain words saying the mixer sends the song back (how late), what that
                                   // does to the marker and the grade, and how to stop it (LOOPBACK off, music over USB...)
}
```

- `practice_start` opens the chosen input (12.4) unless `watch_only`, the permission is
  missing (macOS `not_determined`: the UI shows the explanation and calls
  `mic_request_permission` first) or there is no input; then the phase is `watch_only`
  with the reason: the notes and the piano keys show the melody, there is no marker and no
  grade. A song without a usable guide goes to `preparing` (the guide is built on the
  spot, with `guide://status` progress) and then on; if that fails, `not_ready`. Called
  while active for the same song it returns the state; for another song it stops the
  first (a running take ends as by `practice_stop`).
- `practice_stop(options?: { keep_result?: boolean }) -> { result: PracticeResult | null }`:
  closes the microphone, ends a running take (graded and saved when enough was sung,
  12.2; `keep_result: false` drops it), practice off. The song stays loaded, playing or
  paused as it was.
- `practice_state() -> PracticeState`.
- `practice_suspend_input(suspended: boolean)`: closes (true) or reopens (false) the
  microphone without leaving practice. The UI calls it when the window is hidden or
  minimised and when it comes back; the rig check uses it too. While suspended a take
  continues without a singer (nothing sung counts as not sung).
- `practice_finish() -> { result: PracticeResult | null }`: "Finish": ends the take now,
  grades what was sung and pauses the song.
- `practice_again() -> PracticeState`: "Sing again": seeks to 0, starts a new take, plays.
- `practice_guide(song_id?: string) -> PracticeGuide`: the guide as the lane draws it (the
  loaded song by default), with the graded rule and the song's exclusions applied and the
  word references checked or recomputed (SONG_FORMAT 10.4):

```ts
interface PracticeGuide {
  song_id: string; status: GuideStatus;
  tuning_cents: number;            // tuning.use_cents, for lane() of 12.1
  range: { low: number; high: number } | null;    // pitched notes, original key, no octave shift
  notes: { start: number; end: number; midi: number; kind: "pitched" | "unpitched";
           confidence: number; dim: boolean;        // dim: low confidence or octave-flagged: drawn dimmer
           graded: boolean;                          // SONG_FORMAT 10.1's rule, exclusions included
           glide_in: number; word: [number, number] | null }[];   // [line, word] of the effective lyrics
  spans: { start: number; end: number; kind: string }[];          // spoken / breath: "speak" bars, never graded
  curve: { hop_s: number; t0_s: number; midi: number[] } | null;  // 0 = unvoiced; two decimals (~150 kB per 4 min)
  lines: { line: number; start: number; end: number }[];          // rows of the breakdown (lyric lines, else phrases)
}
```

- `practice_song_set(song_id, changes: { octave?: { mode: "auto" | "fixed" | "strict"; shift?: number }; strictness?: "relaxed" | "standard" | "strict" | null }) -> PracticeState`:
  saved in the song's slot (`practice.octave`, `practice.strictness`), debounced like
  `player_set` and flushed on song change and exit. The lane moves at once; a running
  take grades the notes still to come with the new setting.
- `practice_exclude(song_id, span: { line: number | null; start: number; end: number }, excluded: boolean) -> { excluded: { line: number | null; start: number; end: number }[] }`:
  "the guide is wrong here" on a lyric line, and its undo; saved in the song's slot at
  once. Notes whose midpoint lies inside an excluded span stop counting from then on,
  in a running take too; saved results are not rewritten.

Events while practice is on:

- `practice://live`, about 60 times a second (every 16 ms while the input runs; nothing
  while it does not), each batch holding what is new since the previous one:

```ts
interface PracticeLive {
  seq: number; segment: number;
  pitch: [number, number | null, number][];
      // tracker values, one per 10 ms: [song_time_s, midi | null, level_db]. song_time_s is the song time that was
      // playing when the sound was sung (12.7 "Timing"). midi null = no pitch: silence, breath, consonant, noise, or
      // nothing but returned music; the marker disappears. Not snapped, not octave-shifted (the UI applies lane()).
  hits: [number, number, number, -1 | 0 | 1][];
      // [note, from_s, to_s, where] for stretches of graded notes the scorer has decided: 0 on pitch (lit),
      // 1 above, -1 below. A stretch that continues the previous batch's last one extends it.
  done: [number, number | null, number, number | null][];
      // notes finished: [note, credit 0..1 (null: not graded), key heard, mean error in cents (signed, folded)]
}
```

  The UI draws the marker at the newest value and the tail from the last second of values,
  placed by `song_time_s` against the clock it already interpolates. The tracker's own
  pipeline delay is 31.3 ms (half its 42.7 ms window plus one 10 ms hop of look-ahead:
  an event exists 31.3 ms after the moment it describes); from a note's onset to the
  first event on the note it is a median of 35 ms and at most 62 ms on generated voices
  (practice-tracker.md section 22), so the marker may show a value that much late; that
  never costs points, because grading uses `song_time_s`, not the moment of display.
  **Delay budget** (WO2 4.4: aim 50 ms from the sound reaching the app to the marker
  moving, never more than 100 ms): tracker (35 / 62 ms) + waiting for the next batch
  (8 / 16 ms) + the event's trip to the web view (a few ms) + the next frame (8 / 17 ms
  at 60 fps): about 55 ms typical and 95-100 ms worst case on clean onsets. Real sung
  onsets need more evidence than generated ones: 6.0 % (development set) and 6.2 %
  (held-out) of real onsets are shown on the note more than 100 ms after they begin at
  the tracker alone (14-15 % under a strong room reverb, 14-16 % for low voices shifted
  down 7-12 semitones), so the 100 ms ceiling is met for the pipeline, not for every real
  onset. That is why the batches go out every 16 ms and not slower; a batch is sent early
  when a tracker value changes the marker's visibility (singing starts or stops). Stage 4
  measures the whole chain in the real app on both systems.
- `practice://state`: a full `PracticeState` whenever something in it changes (phase,
  input state, music in the input, the octave decided, the running score), at most 4
  times a second.
- `practice://result`: a `PracticeResult` when a take is graded.

**Music in the input** (WO2 4.4, 4.10, trap 6). The core always compares the input with
what the app is playing (the engine's output, which it knows exactly), so the warning does
not depend on anything the singer does. `presence` becomes `present` within a few seconds
of music at any level that could move the marker, and `message` says so. Removal starts by
itself when the delay is `stable` (cable or USB). `grading` is `normal` (no music, or
removal `good`), `less_certain` (`marginal`: the result is flagged) or `not_graded`
(`unusable`, for example a wandering Bluetooth delay: the marker still shows, the result
has `overall` null and says why). With nobody singing the marker stays off and the grade
is the lowest, with or without the music coming back: a pitch that the residue of the
music could explain is treated as no pitch, never credited. The measured delay of the
returned music is also the singer's timing correction (`latency.source`
`returned_music`).

### 12.6 Results and history

```ts
interface PracticeResult {         // the slot's result (SONG_FORMAT section 11), as stored
  date: string; key: number; key_changes: number; seeks: number;
  octave: number; octave_mode: "auto" | "fixed" | "strict"; strictness: "relaxed" | "standard" | "strict";
  overall: number | null; grade: string | null;    // null: could not be graded honestly (trust says why)
  graded: number; total: number;                   // "graded on 212 of 240 notes"
  tendency_cents: number | null;                   // + sharp, - flat
  sung_share: number; voiced_share: number; completed: boolean;
  trust: string[];                                 // empty = trusted; flags in SONG_FORMAT section 11
  guide: string; scorer: string; app: string;
  lines: { basis: "lyrics" | "phrases"; rows: [number, number, number | null, number, number | null][] };
  // added for the screen, not stored:
  trust_messages: string[];                        // one plain sentence per flag
  best: boolean;                                   // this take is now the song's best
  saved: boolean;                                  // false: too short for the history (12.2), or the slot
                                                   // could not be written (read-only stick)
  key_label: string | null;                        // "A major → F# major (-3)" for `key`
}
```

- `practice_history(song_id) -> { best: PracticeResult | null; recent: PracticeResult[] }`
  (newest first, at most 20; read from the song's slot, so it travels with the song).
- `SongSummary.practice_best` (section 2) carries the best result's `overall`, `grade` and
  `date` for the Library.

Grading (shared Rust, `jk-pitch::score`, identical on Windows and the Mac): for each graded
note, the share of its length during which the singer was on pitch by the bands below;
vibrato around the note and a scoop inside its `glide_in` are not punished, and each
moment of the note may be matched within the timing tolerance, so being a little early or
late costs nothing. `overall` = 100 x the length-weighted mean credit of the graded notes
in the part of the song the take covered; `total` counts the notes of that part and
`graded` those of them that were graded (SONG_FORMAT section 11), and `sung_share` says
how much of the song was covered. Silence scores 0 on every note; the
original singer, through the engine, scores in the top band at every key (WO2 section 9).

| `strictness` | full credit within | nothing from | timing tolerance |
|---|---|---|---|
| `relaxed` | 75 cents | 150 cents | 200 ms |
| `standard` (default, owner decision 4) | 50 cents | 100 cents | 150 ms |
| `strict` | 30 cents | 60 cents | 100 ms |

Grades (owner decision 3): A+ from 95, A from 90, B+ from 85, B from 80, C+ from 75, C
from 70, D from 60, F below 60. The `standard` cents are fixed by WO2 4.7; the other
bands, the tolerances and the letter thresholds are the scorer's constants, to be
confirmed by measurement in Stage 3 against WO2 section 9's scripted singers. A result
stores the `scorer` version that made it, so a later change never rewrites history.

### 12.7 Practice settings (`AppSettings.practice`)

```ts
interface PracticeAppSettings {
  piano_side: "left" | "right";    // owner decision 1: left
  simple_lane: boolean;            // no tail, no glow (reduce-motion also gives a plain marker)
  words_under_notes: boolean;      // owner decision 2: true
  lyrics_below: boolean;           // the usual lyrics display under the lane: true
  strictness: "relaxed" | "standard" | "strict";   // for songs without their own (slot strictness null)
  octave_mode: "auto" | "fixed" | "strict";         // for songs never practised (owner decision 5: auto)
  auto_latency: boolean;           // use a measured delay (returned music, rig check): true
  inputs: Record<string, InputChoice>;             // per computer: key = computer hash, as audio.key_benchmarks
  latency_offsets_ms: Record<string, number>;      // manual offset per "<computer>|<input device id>", -500..500
  measured_latency_ms: Record<string, { ms: number; how: "returned_music" | "rig_check"; utc: string }>;
                                                    // last measurement per "<computer>|<input id>|<output id>"
}
```

Defaults: `piano_side` "left", `simple_lane` false, `words_under_notes` true,
`lyrics_below` true, `strictness` "standard", `octave_mode` "auto", `auto_latency` true,
the maps empty. Per-song choices (octave mode and shift, strictness override, excluded
lines, results) are in the song's slot, never here. The input and the latency belong to
the computer, and `settings.json` travels on the stick between the PC and the Mac, so
they are keyed by a hash of the computer (src/keymode.rs). Colours are the existing
`ui.colors`: sung (gold) for lit bars, marker (mint) for the singer's marker and key (WO2
4.5). Clamping and unknown keys follow section 5.

**Timing** (WO2 4.4 "Timing", trap 4). The core maps every tracker value to the song time
that was playing when it was sung: the input block's host-clock stamp, minus the input
latency the device declares, read through the engine's DAC clock (whose position already
has the declared output latency removed), minus `latency.applied_ms`. `applied_ms` is the
measured delay when `auto_latency` is on and a measurement exists for this computer, input
and output (the returned music measures it live; the rig check stores one), else the
manual offset for this input plus the output device's offset
(`audio.device_offsets_ms[device_id ?? "default"]`, the same value the lyrics use, section
3: a Bluetooth output makes the singer hear the song late), else 0. A delay nobody could
measure is absorbed by the timing tolerance of 12.6, never charged to the singer, and the
result gets `latency_unmeasured`.

Every **measured delay** in this section (`MusicInInput.delay_ms`, `latency.measured_ms`,
`RigCheckReport.returned.delay_ms`, `practice.measured_latency_ms`) is the residual: how
late the returned sound arrives after the declared output and input latencies are taken
off (the echo stage compares the input with the engine's output placed by the two clocks
exactly as the voice is placed, ARCHITECTURE 12.3). It is therefore subtracted as it is,
never added to the declared latencies a second time. The whole round trip, for reports,
is that residual plus `RigCheckReport.output.latency_ms` and `input.declared_latency_ms`.

The **lane's "now"** is `position_s - device_offset_ms / 1000`: section 3's lyric formula
without the song's `lyrics_offset_ms`, because the notes come from the audio and the
lyrics offset corrects only the words (SONG_FORMAT 10.4). The bars thus pass the needle
when the singer hears them.

`practice_latency_set(manual_ms: number, device_id?: string | null) -> { manual_ms: number }`:
the manual offset for this computer and that input (default: the chosen input), clamped to
-500..500 ms and saved in `practice.latency_offsets_ms` under the computer's hash, which
the UI does not know (as for `mic_choose`). 0 removes it. A running session applies it at
once when it is the source in use.

### 12.8 Error codes added

`no_guide` (practice needs a guide and none could be made; the message says why),
`no_input`, `permission_denied`, `input_unavailable` (the device is gone or busy),
`practising` (`selfcheck_run` and `practice_rig_check` refuse while a take runs),
`audible_not_confirmed` (10.2), the guide codes of 12.3, and the existing `not_found`,
`not_loaded` and `bad_request`.

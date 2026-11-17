# App API: Rust core <-> web UI (version 1)

The UI talks to the Rust core only through these Tauri commands (`invoke`) and
events (`listen`). The UI never handles audio samples. TypeScript types live in
`app/ui/src/api/types.ts` and must match this document; the Rust side lives in
`app/src-tauri/src/commands/`. A mock backend (`app/ui/src/api/mock.ts`) implements
the same API in the browser so the UI can be developed and tested without Rust.

Conventions: command names are `snake_case`; arguments and results are JSON objects
with `snake_case` keys. Times are seconds (number). Errors reject the promise with
`{ code: string, message: string }` where `message` is plain language for the user.
A song is identified by `song_id`: its path relative to the Library folder, with `/`
separators (stable across drive letters and both OSes).

## 1. App and capabilities

`app_info() -> AppInfo`

```ts
interface AppInfo {
  name: string; version: string;                  // from branding / package info
  platform: "windows" | "macos";
  os_version: string;                              // "Windows 11 26200", "macOS 13.6.9"
  portable: boolean; root: string | null;          // root shown for information only
  translocated: boolean;                           // macOS App Translocation detected
  paths: { library: string; recordings: string; exports: string; settings: string; models: string | null };
  capabilities: {
    studio: { available: boolean; reasons: string[]; gpu: string | null; vram_mb: number | null };
    recorder: { available: boolean; reason: string | null;
                permission: "granted" | "denied" | "not_determined" | "not_required" };
    exclusive_mode: boolean;                       // Windows only
  };
}
```

`studio.available` is false on macOS, when the Studio runtime is missing next to
the app, or when the Studio probe says the computer cannot run it. `reasons` are
shown as they are.

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

`song_cover(song_id, max_px: number) -> { data_url: string | null }` (JPEG/PNG
thumbnail, cached in memory).

Event `library://changed` (no payload) when files appear, disappear or change.

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

- `player_play()`, `player_pause()`, `player_toggle()`, `player_stop()`
- `player_seek(position_s: number)`
- `player_set(changes: Partial<SongSettings>)` - applies live (click-free) and saves
  to the song file's settings slot (debounced about 1 s, flushed on song change and
  exit).
- `player_set_master(volume: number)` - 0..1, app setting, not saved in the song.
- `player_state() -> PlayerState`

```ts
interface PlayerState {
  state: "empty" | "loading" | "paused" | "playing" | "ended" | "error";
  song_id: string | null; position_s: number; duration_s: number;
  device: { name: string; sample_rate: number; channels: number; exclusive: boolean; latency_ms: number } | null;
  underruns: number; error: string | null;
  key_label: string | null;                        // "A major -> B major (+2)"
}
```

Events:

- `playback://clock` about 30 times per second while playing (and once on every
  state change): `{ position_s: number; playing: boolean; seq: number }`.
  `position_s` is the song time of the sample now leaving the DAC (device latency
  already subtracted). The UI interpolates between events with `performance.now()`
  and re-anchors on each event (ARCHITECTURE section 4); the lyric offsets are applied
  by the UI: `lyric_time = position_s - (song.lyrics_offset_ms + device_offset_ms) / 1000`.
- `playback://state`: a full `PlayerState`.
- `playback://error`: `{ code, message }` (device lost, file unreadable...).

## 4. Output devices

`devices_list() -> { devices: Device[] }` with
`Device { id: string; name: string; is_default: boolean; sample_rate: number; channels: number; exclusive_capable: boolean }`.

`device_set(id: string | null, exclusive: boolean)` - null = system default. Saved in
app settings. Changing device during playback pauses, switches, and resumes.

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
           device_offsets_ms: Record<string, number> };   // Bluetooth latency per device id
}
```

## 6. Sync editor and metadata

- `lyrics_save(song_id, lyrics: Lyrics)` - stores `lyrics_override` in the song's slot.
- `lyrics_reset(song_id)` - removes the override.
- `metadata_save(song_id, { title?, artist?, album? })` - stores `metadata_override`.
- `studio_realign(song_id, text: string, anchors: { line: number; start: number }[]) -> Lyrics`
  (Windows with the Studio only; `line` is the 0-based index into the `lines` array of
  the lyrics produced from `text`).

## 7. Export backing track

`export_backing(song_id, { format: "flac24" | "wav32f" }) -> { job_id: string }`

Renders the current key and mix (the song's settings, guide vocal included at its
current level) offline with the same DSP as playback, at the song's sample rate, to
`<root>/Exports/<artist> - <title> (<key>).flac|wav`. Events
`export://progress { job_id, progress }`, `export://done { job_id, path }`,
`export://failed { job_id, code, message }`.

## 8. Recorder

- `recorder_status() -> RecorderStatus`
- `recorder_start()`, `recorder_stop() -> Recording`
- `recorder_request_permission()` (macOS: shows the system prompt after the UI has
  explained why "Screen Recording" is asked for)
- `recordings_list() -> { recordings: Recording[] }`

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

- `studio_status() -> { available: boolean; reasons: string[]; running: boolean; models_present: boolean; queue: StudioJob[] }`
- `studio_inspect(path: string) -> InspectResult` (STUDIO_PROTOCOL section 5)
- `studio_lyrics_lookup({ artist, title, album, duration_s }) -> { results: LookupResult[] }`
- `studio_submit({ input: string; preset: "full" | "clean"; language: string; metadata: {...}; lyrics: {...} }) -> { job_id }`
- `studio_cancel(job_id)`
- `studio_models_status()`, `studio_models_download()`
- `pick_files() -> string[]` (native file dialog: audio and video types) and drag-and-drop
  of files onto the window (Tauri drag-drop event `studio://dropped { paths }`).

```ts
interface StudioJob { job_id: string; input_name: string; state: "queued" | "running" | "done" | "failed" | "cancelled";
  stage: string | null; overall: number; message: string | null; output_song_id: string | null; error: string | null }
```

Events: `studio://job` (a `StudioJob` on every change), `studio://models { name, done_bytes, total_bytes }`.

## 10. Self-check (WORK_ORDER 4.10)

`selfcheck_run() -> { report_path: string; passed: boolean; items: { name: string; ok: boolean; detail: string }[] }`

Plays a test tone, loads and renders the bundled test song, shifts its key, checks the
recorder permission and makes a 3-second capture, and records OS version, web engine
version (the UI passes `navigator.userAgent`), memory and output device format. Writes
a plain-text report to `<root>/Settings/selfcheck-<date>.txt`. The app also accepts
`--selfcheck` on the command line (runs it headless-ish and exits 0/1), for CI.

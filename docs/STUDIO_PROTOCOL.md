# Studio protocol, version 1

How the Jukeoke app (Rust) talks to the Studio engine (Python). The Studio is a
separate process that the app starts on demand and stops when it is idle or the app
quits. The player never imports it, and the Mac build does not include it.

Reference implementation: `studio/jukeoke_studio/server.py` (the Studio side) and
`app/src-tauri/src/studio.rs` (the app side). Every addition to version 1 since the
first draft is marked **(added)** and listed in section 11; all of them are compatible
(new optional fields and values; unknown fields are ignored in both directions).

## 1. Transport

- The app starts:
  `<root>/Windows/Studio/python/python.exe -I -X utf8 -m jukeoke_studio serve --root <root> --models-dir <root>/Models --temp-dir <root>/Settings/tmp/studio`
  with the working directory set to `<root>/Windows/Studio`. `-I` isolates it from
  any Python installed on the computer. **(added)** `--root`, `--models-dir` and
  `--temp-dir`: the Studio points every cache of its libraries into the stick before it
  imports them (section 1.1).
- **stdin**: requests from the app, one JSON object per line (UTF-8, `\n`).
- **stdout**: responses and events from the Studio, one JSON object per line. Nothing
  else may be printed to stdout: the Studio keeps a private copy of the real stdout for
  the protocol and points file descriptor 1 (and the Windows standard output handle) at
  stderr before importing any library, so even C-level output of CUDA, CTranslate2 or
  FFmpeg goes to the log. **(added)** It also takes the request pipe away from the
  standard input handle (children inherit NUL): on Windows a thread blocked reading a
  pipe blocks every other use of that pipe, including the one `CreateProcess` makes when
  a library starts a helper process (measured: a job stalled until stdin was closed).
- **stderr**: free-form log text. The app appends it to `<root>/Settings/logs/studio.log`.
- Every message has a `type`. Requests carry an `id` (string chosen by the app);
  the direct response to a request echoes that `id`. Events about a job carry `job_id`.
- Unknown message types get an `error` response with code `unknown_type`. Unknown
  fields are ignored in both directions. A line that is not a JSON object gets an
  `error` with `"id": null` and code `bad_request`; the Studio carries on.
- Paths are absolute strings valid for this run only. Nothing persists them.
- If the Studio process dies, the app marks running jobs as failed (`studio_crashed`),
  keeps the queue, and restarts the Studio for the next job.
- Requests are answered while a job runs: the Studio reads stdin on its main thread,
  runs slow requests (`probe`, `inspect`, `lyrics.lookup`, `models.download`,
  `realign`) each on their own thread and jobs on one worker thread. GPU work
  (separation, transcription, alignment, `realign`) is serialised by one lock.

### 1.1 Where the Studio writes (added)

| what | where | how it is set |
|---|---|---|
| separation weights + `manifest.json` | `<root>/Models/` | `--models-dir`, `JUKEOKE_MODELS_DIR`, `hello.models_dir` |
| alignment / Whisper weights (Hugging Face layout) | `<root>/Models/hf/` | `HF_HUB_CACHE`, `TRANSFORMERS_CACHE` |
| library caches | `<root>/Settings/cache/{huggingface,torch,xdg,cuda,numba,matplotlib,triton,inductor}` | `HF_HOME`, `TORCH_HOME`, `XDG_CACHE_HOME`, `CUDA_CACHE_PATH`, `NUMBA_CACHE_DIR`, `MPLCONFIGDIR`, `PYTORCH_KERNEL_CACHE_PATH`, `TRITON_CACHE_DIR`, `TORCHINDUCTOR_CACHE_DIR` |
| temporary files, job scratch, extracted covers | `<root>/Settings/tmp/studio/` | `--temp-dir`, `TMP`, `TEMP`, `TMPDIR`, `hello.temp_dir` |
| finished songs | the job's `output_dir` (`<root>/Library`) | `job.submit` |

The Studio sets these itself from `--root` (studio/jukeoke_studio/env.py) and the app
also passes them as environment variables. `HF_HUB_OFFLINE=1` and
`TRANSFORMERS_OFFLINE=1` keep the Hugging Face libraries off the network; bytecode
writing is off (`sys.dont_write_bytecode`, because `-I` ignores `PYTHON*` variables).

## 2. Session start

```json
> {"type":"hello","id":"1","protocol":1,"app_version":"0.1.0",
   "root":"E:\\","models_dir":"E:\\Models","library_dir":"E:\\Library","temp_dir":"E:\\Settings\\tmp\\studio"}
< {"type":"hello.ok","id":"1","protocol":1,"studio_version":"0.1.0",
   "python":"3.12.16","torch":"2.14.1+cu130",
   "presets":{"full":{"title":"Full (default)","description":"...","weights_licences":["GPL-3.0","Apache-2.0"],
                      "commercial":"yes","backing_vocals":true,"default":true},
              "clean":{...},"permissive":{...}},
   "default_preset":"full","stub_models":false,
   "paths":{"models_dir":"E:\\Models","temp_dir":"E:\\Settings\\tmp\\studio","cache_dir":"E:\\Settings\\cache"}}
```

If the protocol versions differ, the Studio answers `error` with code
`protocol_mismatch`, and the app reports the Studio as unavailable.

**(added)** `hello.ok` carries `presets` (what the app shows next to each quality
preset; `description` is plain language and says when backing vocals are missing),
`default_preset`, `paths` and `stub_models` (true only in tests). `torch` is the
installed PyTorch version, read without importing it. `models_dir` / `temp_dir` in
`hello` override the command-line values for this session. `hello` answers at once;
when the hardware can run the Studio, PyTorch is then imported in the background so the
first probe and job are quicker.

## 3. Capability probe

```json
> {"type":"probe","id":"2","timeout_s":45}
< {"type":"probe.result","id":"2","ok_for_studio":true,
   "cuda":true,"gpu":"NVIDIA GeForce RTX 5070 Ti","vram_mb":16303,"compute_capability":"12.0",
   "ram_mb":32652,"reasons":[],
   "thresholds":{"vram_mb":5632,"ram_mb":11264,"nominal":{"vram_gb":6,"ram_gb":12}},
   "checked_with":"torch","torch":"2.14.1+cu130","torch_check":{"ok":true,"arch":"sm_120","seconds":4.1},
   "seconds":4.2}
```

`ok_for_studio` is false when there is no usable CUDA GPU, too little VRAM (under
6 GB) or too little RAM (under 12 GB). `reasons` is a list of plain-language strings
the app shows as they are, for example
`"No NVIDIA graphics card with CUDA was found. Songs are created on a stronger computer and can be played anywhere."`

The app probes with a timeout (60 s for the first start, which imports PyTorch). A
timeout or crash means "Studio unavailable", never a hang.

**(added)** How the Studio answers without hanging:

1. A fast path with the standard library only: physical memory from the OS and the
   NVIDIA driver's CUDA library (`nvcuda.dll`) through ctypes. No GPU, too little VRAM
   or RAM: answered at once (`"checked_with":"driver"`), PyTorch never imported.
   The numeric thresholds (`thresholds`) sit a little under the nominal 6 GB / 12 GB so
   that a card or PC sold with exactly that much passes.
2. Otherwise PyTorch is imported on a worker thread and a real matrix product runs on the
   GPU (`"checked_with":"torch"`, `torch_check` has the details). The probe waits at most
   `timeout_s` (default 45 s, at most 300 s). If PyTorch is still loading then (a cold
   start from a slow stick), the answer is based on the driver check with
   `"torch_check":"pending"`, and the next probe reuses the finished check.

Test hook: `JUKEOKE_STUDIO_FAKE_HW=none|small|ok` in the Studio's environment fakes the
hardware (CPU tests).

## 4. Models

```json
> {"type":"models.status","id":"3","preset":"full"}
< {"type":"models.status.result","id":"3","preset":"full","models":[
    {"key":"vocals","name":"bs_roformer_anvuew_ft1","title":"...","file":"bs_roformer_ft1_anvuew_sdr_12.55.ckpt",
     "size_bytes":204493312,"present":true,"licence":"GPL-3.0","commercial":"yes",
     "task":"separation","needed_for":"every song"},
    {"key":"alignment","name":"facebook/wav2vec2-large-960h-lv60-self","file":"pytorch_model.bin",
     "size_bytes":1262057705,"present":true,"licence":"Apache-2.0","commercial":"yes",
     "task":"alignment","needed_for":"every song with lyrics"},
    {"key":"transcription","name":"mobiuslabsgmbh/faster-whisper-large-v3-turbo", "...": "..."}],
   "all_present":true,"download_bytes":0,"models_dir":"E:\\Models"}

> {"type":"models.download","id":"4","preset":"full"}
< {"type":"models.progress","id":"4","name":"bs_roformer_ft1_anvuew_sdr_12.55.ckpt","done_bytes":1000,"total_bytes":204493312}
< {"type":"models.download.result","id":"4","ok":true,"downloaded":["bs_roformer_anvuew_ft1"]}
```

Downloads happen only when the user asks (or confirms the first time they create a
song). Files are downloaded to `<models_dir>/<name>.part`, checked against the
expected sha256, then renamed.

**(added)**
- `preset` is `full`, `clean` or `permissive`. The list covers the preset's separation
  models plus the alignment model and the Whisper model (only needed for songs without
  lyrics); `all_present` covers all of them. Presence is checked by name and size (no
  hashing), so the answer is quick.
- `models.download` is the **only** request that downloads weights. Nothing downloads
  silently: a job whose models are missing fails at once with `models_missing`.
  Hugging Face files go to `<models_dir>/hf/models--<org>--<name>/snapshots/<commit>/`
  through the same `.part` + SHA-256 + rename path (pinned commit and hash per file:
  `studio/jukeoke_studio/modelzoo/hfmodels.py`). A hash mismatch is `model_checksum`; a
  network or disk failure is `internal` with a plain message; nothing half-downloaded
  is left behind. One download at a time (a second request is `bad_request`).
- Pickled weights (HT-Demucs `.th`) are hashed again, from the exact bytes, every time
  before they are unpickled; RoFormer / SCNet checkpoints are loaded `weights_only`.

## 5. Inspecting a file before queueing

```json
> {"type":"inspect","id":"5","input":"C:\\Music\\song.mp3"}
< {"type":"inspect.result","id":"5","ok":true,
   "title":"...","artist":"...","album":null,"duration_seconds":241.3,
   "container":"mp3","codec":"mp3","sample_rate":44100,"channels":2,"bits_per_sample":null,
   "lossy":true,"has_video":false,
   "embedded_lyrics":{"present":true,"synced":false,"text":"..."},
   "cover_path":"E:\\Settings\\tmp\\studio\\covers\\cover-ab12.jpg","title_from_tags":true}
```

The app shows this for the user to correct title and artist, and to paste lyrics or
pick an `.lrc` file.

**(added)** `title` falls back to the file name (`title_from_tags: false` then).
`container` is a short name (`mp3`, `m4a`, `flac`, `wav`, `ogg`, `aiff`, `wma`, `mp4`,
`mkv`, `mov`, `webm`, `avi`, ...); `codec` the codec's name (`mp3`, `aac`, `flac`,
`pcm_s24le`, `opus`, ...); `bits_per_sample` is set for lossless codecs only; a cover
picture stored in the file is not a video (`has_video` counts real video streams). When
`embedded_lyrics.synced` is true, `text` is LRC, so the app can hand it back unchanged as
`lyrics.lrc` with `lyrics.source` `"embedded"`. The cover is written as JPEG or PNG (at
most 2 MB, other formats converted) into `<temp_dir>/covers/`. Errors: `not_found`,
`decode_failed` (no readable audio stream). Optional request field `language`.

## 6. Online lyrics lookup (only when the user presses the button)

```json
> {"type":"lyrics.lookup","id":"6","artist":"...","title":"...","album":null,"duration_seconds":241.3}
< {"type":"lyrics.lookup.result","id":"6","results":[
    {"service":"lrclib","synced_lrc":"[00:12.34] ...","plain":"...","duration_seconds":241.0}]}
```

The Studio never contacts the network on its own.

**(added)** Results also carry `instrumental`, `artist`, `title`, `album` and `id`
(LRCLIB's). A network failure is the error code `lookup_failed` (plain message, e.g.
"The lyrics service could not be reached. Check the internet connection.").

## 7. Jobs

```json
> {"type":"job.submit","id":"7","job":{
    "job_id":"c1f0...","input":"C:\\Music\\song.mp3","output_dir":"E:\\Library",
    "preset":"full","language":"en",
    "metadata":{"title":"Corrected title","artist":"Artist","album":null},
    "lyrics":{"mode":"auto","text":null,"lrc":null,"source":null}}}
< {"type":"job.accepted","id":"7","job_id":"c1f0...","position":1}
```

**(added)** `metadata.cover_path` corrects the cover (WORK_ORDER 4.1 step 1). Absent:
the cover stored in the file is used, as before. `null`: the song gets no cover. A path:
that picture is the cover instead. It must be a JPEG or PNG file (checked by its first
bytes; anything else, or a missing file, is `bad_request` at submit). Up to 2 MB it is
stored byte for byte; a larger one is re-encoded once to JPEG of at most 2 MB
(docs/SONG_FORMAT.md 4.1), lowering the JPEG quality first and the pixel size only when
the lowest quality does not fit; it is never enlarged, and a PNG's transparency is lost
in that case. The file is read when the job runs; if it cannot be used then (gone,
damaged, over 64 MB), the job goes on with the file's own cover and says so in `report.warnings`. The
song's `provenance.metadata_source.cover` records `user`, `removed`, `tags` or `none`.
`metadata.title` / `artist` / `album` are optional too: empty or absent means the tags
(or the file name for the title); `album: null` means no album.

`preset`: `full` (default), `clean` or **(added)** `permissive` (MIT/Apache-2.0 models
only; no lead/backing split exists under such licences, so all vocals are removed and
the `backing_vocals` stem is silent; see `hello.ok` `presets`).

`lyrics.mode`:

- `auto`: use, in order, `lrc`, `text`, lyrics embedded in the file, then
  transcription of the isolated lead vocal (WORK_ORDER 4.1 step 3). An online lookup
  is not part of `auto`; the app runs it first if the user pressed the button, and
  passes the result as `lrc` or `text`. **(added)** A source that is given but holds no
  singable line after normalisation (only section headers like `[Chorus]`, blank lines
  or bare timestamps) is skipped and the next one is tried.
- `text` / `lrc`: use only what is given (fail the lyrics stage if empty).
- `none`: instrumental, no lyrics.

**(added)** `lyrics.source` labels where the given `lrc` / `text` came from, and is
written to the song's lyrics as `source` (docs/SONG_FORMAT.md section 5):
`online:<service>` (e.g. `online:lrclib` for a lookup result), `embedded` (the file's
own lyrics from `inspect`), `user_text` or `lrc`. Absent: `lrc` for LRC, `user_text` for
text. Any other value is `bad_request` at submit. Given lyrics are parsed during
`decode`, so `text`/`lrc` mode with nothing singable fails with `lyrics_missing` before
the separation starts.

`job.submit` validates the job at once (`bad_request`: no `job_id`, `input` or
`output_dir`, unknown preset or lyrics mode, wrong types, a `job_id` already used in this
process). `position` counts the running job and the waiting ones, this one included.

Events while it runs:

```json
< {"type":"job.started","job_id":"c1f0..."}
< {"type":"job.progress","job_id":"c1f0...","stage":"separate","stage_progress":0.42,
   "overall":0.31,"message":"Separating the lead vocal from the backing vocals"}
< {"type":"job.done","job_id":"c1f0...","output":"E:\\Library\\Artist - Title.jksong",
   "report":{"seconds":{"decode":1.2,"separate":61.0,"lyrics":20.3,"align":8.8,"analyse":2.1,"write":4.0},
             "lyrics_source":"transcribed","lyrics_checked":false,"warnings":[]}}
< {"type":"job.failed","job_id":"c1f0...","error":{"code":"decode_failed",
   "message":"This file could not be read as audio.","detail":"..."}}
```

Stages, in order: `decode`, `separate`, `lyrics`, `align`, `analyse`, `write`.

**(added)** Progress events come at most about five times a second per job (always the
first and last of each stage); `overall` never goes down. The report also carries
`total_seconds`, `model_load_seconds`, `lyrics_lines`, `sample_rate`,
`duration_seconds`, `preset`, `stems_sum_max_error` (the written stems decoded and
summed against the decoded source; at most half a 24-bit step),
`resampling` (as in the song's provenance), `title` and `artist`. `warnings` are plain
sentences for the user, for example a 5.1 file folded to stereo, a damaged file end, or
lyrics that could not be timed (the song is then saved **without** lyrics rather than
failing after the separation; the sync editor's `realign` can add them).
Error codes of `job.failed`: `not_found` (input gone), `decode_failed`, `models_missing`
(checked before the separation), `model_checksum`, `lyrics_missing` (explicit
`text`/`lrc` mode), `out_of_memory`, `write_failed`, `cancelled`, `internal`.

Jobs run one at a time in submission order. A failed job never stops the queue.
Other requests (`probe`, `inspect`, `job.cancel`, `job.list`) are answered while a job
runs.

```json
> {"type":"job.cancel","id":"8","job_id":"c1f0..."}
< {"type":"job.cancel.result","id":"8","ok":true}
< {"type":"job.failed","job_id":"c1f0...","error":{"code":"cancelled","message":"Cancelled."}}

> {"type":"job.list","id":"9"}
< {"type":"job.list.result","id":"9","jobs":[{"job_id":"...","state":"queued|running|done|failed","stage":"..."}]}
```

**(added)** Cancellation is cooperative and quick: model chunks, decoding, hashing,
transcription segments, alignment chunks and the per-stem FLAC encoding all check it.
The job's temporary files are deleted. Cancelling a waiting job removes it without
`job.started`. An unknown `job_id` is `not_found`; cancelling a finished job is a no-op
(`ok: true`). `job.list` entries also carry `overall`, `error_code` (`cancelled` for a
cancelled job, whose state is `failed`), `output` and `input`.

The output file name is `<artist> - <title>.jksong` with characters that are invalid
on Windows, macOS or exFAT replaced, and a ` (2)` suffix if the name exists. It is
written atomically (docs/SONG_FORMAT.md section 7).

**(added)** Naming rules (same as the app's export names): `< > : " / \ | ? *` and
control characters become `_`, NFC, no leading dots (the library skips dotfiles) and no
trailing dots or spaces, reserved Windows names (`CON`, `COM1`...) get a `_` prefix, at
most 150 characters; no artist: `<title>.jksong`; no title tag: the input's file name.
The song is written under a short temporary name `studio-<random>.jksong.tmp-<pid>` in
the output folder and then moved to the first free name (` (2)`, ` (3)`...) with a
rename that refuses to replace an existing file, so another song is never overwritten.

## 8. Re-aligning corrected lyrics (sync editor)

```json
> {"type":"realign","id":"10","song":"E:\\Library\\Artist - Title.jksong",
   "lyrics":{"mode":"text","text":"corrected lyrics...","source":null},
   "anchors":[{"line":3,"start":41.20}]}
< {"type":"realign.progress","id":"10","progress":0.5}
< {"type":"realign.result","id":"10","ok":true,"lyrics":{ "...": "lyrics JSON, docs/SONG_FORMAT.md section 5" },
   "warnings":[],"dropped_anchors":[]}
```

The Studio reads the lead-vocal stem from the song file and aligns the given text,
using any `anchors` (line starts the user fixed by hand) as hard constraints. The app
stores the result in the song file's settings slot as `lyrics_override`.

**(added)**
- `line` is a 0-based index into the lines of the given text after normalisation (blank
  lines and section headers removed), i.e. the same numbering as `lines[]` of the
  lyrics JSON the app holds.
- Anchors that cannot all hold are dropped **individually**, in line order: an anchor
  is kept when the letters before it fit between the previous kept anchor and it, and
  the letters from it on fit in the rest of the song. `dropped_anchors` lists the
  0-based lines whose anchors were dropped (also anchors out of time order and anchors on
  lines that do not exist) and `warnings` says why; the kept anchors hold exactly (the line
  starts at the anchor time).
- `lyrics.mode` may be `text` or `lrc` (LRC line times then act as windows);
  `lyrics.source` as in `job.submit`; when absent, the song's existing lyrics source is
  kept (a corrected transcription stays `transcribed`). The result has `checked: true`.
  Optional `language` (default: the song's lyrics language).
- `realign.progress` events (same `id`) keep the app's inactivity timer alive.
- Errors: `not_found`, `lyrics_missing` (empty text), `bad_request`, `decode_failed`
  (unreadable song file), `models_missing`, `alignment_failed`, `out_of_memory`.

## 9. Shutdown

```json
> {"type":"shutdown","id":"11"}
< {"type":"shutdown.ok","id":"11"}
```

The Studio finishes writing nothing half-way: a running job is cancelled and its
temporary file deleted. The app also closes stdin on exit; the Studio exits when
stdin closes.

**(added)** `shutdown.ok` is sent after the running job has stopped (it waits at most
4 s; the job's `job.failed` with `cancelled` comes first). Jobs still waiting in the
Studio's queue are dropped without events: the app keeps its own queue and sends them
to the next Studio process. The process then exits with code 0. On stdin EOF the same
happens (waiting up to 10 s for the running job to clean up).

## 10. Errors

```json
< {"type":"error","id":"12","error":{"code":"bad_request","message":"...","detail":"..."}}
```

Error codes: `bad_request`, `unknown_type`, `protocol_mismatch`, `not_found`,
`decode_failed`, `no_gpu`, `out_of_memory`, `models_missing`, `model_checksum`,
`lyrics_missing`, `alignment_failed`, `write_failed`, `cancelled`, `studio_crashed`,
`internal`, **(added)** `lookup_failed`. `message` is plain language for the user;
`detail` is for the log.

## 11. Changes to version 1 (all compatible)

2026-11-17, Studio pipeline workstream (the first real Studio):

1. Command line `--root`, `--models-dir`, `--temp-dir`; the cache layout of 1.1; stdout
   and stdin handling (section 1).
2. `hello.ok`: `presets`, `default_preset`, `paths`, `stub_models`.
3. `probe`: request `timeout_s`; result `thresholds`, `checked_with`, `torch`,
   `torch_check`, `seconds`.
4. `models.status` / `models.download`: preset `permissive`; entries `title`, `task`,
   `needed_for`; result `preset`, `download_bytes`, `models_dir`, `downloaded`;
   alignment and Whisper models included.
5. `inspect`: `title_from_tags`, request `language`, normalised container/codec names.
6. `lyrics.lookup`: error code `lookup_failed`; extra result fields.
7. `job.submit`: preset `permissive`; `lyrics.source`; fall-through of empty sources in
   `auto`; early `lyrics_missing` / `models_missing`.
8. `job.done` report fields; alignment failure is a warning, not a failed job.
9. `job.list` fields; cancel semantics.
10. `realign`: `lyrics.mode` `lrc`, `lyrics.source`, `language`, `warnings`,
    `dropped_anchors`, `realign.progress`; per-anchor dropping.
11. `shutdown` details.

2026-11-18, small gaps:

12. `job.submit`: `metadata.cover_path` (section 7): replace (a JPEG/PNG path) or remove
    (`null`) the cover; absent keeps the old behaviour. `provenance.metadata_source.cover`.

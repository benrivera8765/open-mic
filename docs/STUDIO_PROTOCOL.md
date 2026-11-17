# Studio protocol, version 1

How the Jukeoke app (Rust) talks to the Studio engine (Python). The Studio is a
separate process that the app starts on demand and stops when it is idle or the app
quits. The player never imports it, and the Mac build does not include it.

## 1. Transport

- The app starts:
  `<root>/Windows/Studio/python/python.exe -I -m jukeoke_studio serve`
  with the working directory set to `<root>/Windows/Studio`. `-I` isolates it from
  any Python installed on the computer.
- **stdin**: requests from the app, one JSON object per line (UTF-8, `\n`).
- **stdout**: responses and events from the Studio, one JSON object per line. Nothing
  else may be printed to stdout (the Studio redirects library chatter to stderr).
- **stderr**: free-form log text. The app appends it to `<root>/Settings/logs/studio.log`.
- Every message has a `type`. Requests carry an `id` (string chosen by the app);
  the direct response to a request echoes that `id`. Events about a job carry `job_id`.
- Unknown message types get an `error` response with code `unknown_type`. Unknown
  fields are ignored in both directions.
- Paths are absolute strings valid for this run only. Nothing persists them.
- If the Studio process dies, the app marks running jobs as failed (`studio_crashed`),
  keeps the queue, and restarts the Studio for the next job.

## 2. Session start

```json
> {"type":"hello","id":"1","protocol":1,"app_version":"0.1.0",
   "root":"E:\\","models_dir":"E:\\Models","library_dir":"E:\\Library","temp_dir":"E:\\Settings\\tmp"}
< {"type":"hello.ok","id":"1","protocol":1,"studio_version":"0.1.0",
   "python":"3.12.16","torch":"2.14.1+cu130"}
```

If the protocol versions differ, the Studio answers `error` with code
`protocol_mismatch`, and the app reports the Studio as unavailable.

## 3. Capability probe

```json
> {"type":"probe","id":"2"}
< {"type":"probe.result","id":"2","ok_for_studio":true,
   "cuda":true,"gpu":"NVIDIA GeForce RTX 5070 Ti","vram_mb":16303,"compute_capability":"12.0",
   "ram_mb":32652,"reasons":[]}
```

`ok_for_studio` is false when there is no usable CUDA GPU, too little VRAM (under
6 GB) or too little RAM (under 12 GB). `reasons` is a list of plain-language strings
the app shows as they are, for example
`"No NVIDIA graphics card with CUDA was found. Songs are created on a stronger computer and can be played anywhere."`

The app probes with a timeout (60 s for the first start, which imports PyTorch). A
timeout or crash means "Studio unavailable", never a hang.

## 4. Models

```json
> {"type":"models.status","id":"3","preset":"full"}
< {"type":"models.status.result","id":"3","models":[
    {"key":"vocals","name":"...","file":"...","size_bytes":639000000,"present":true,"licence":"MIT","commercial":"yes"}],
   "all_present":true}

> {"type":"models.download","id":"4","preset":"full"}
< {"type":"models.progress","id":"4","name":"...","done_bytes":1000,"total_bytes":639000000}
< {"type":"models.download.result","id":"4","ok":true}
```

Downloads happen only when the user asks (or confirms the first time they create a
song). Files are downloaded to `<models_dir>/<name>.part`, checked against the
expected sha256, then renamed.

## 5. Inspecting a file before queueing

```json
> {"type":"inspect","id":"5","input":"C:\\Music\\song.mp3"}
< {"type":"inspect.result","id":"5","ok":true,
   "title":"...","artist":"...","album":null,"duration_seconds":241.3,
   "container":"mp3","codec":"mp3","sample_rate":44100,"channels":2,"bits_per_sample":null,
   "lossy":true,"has_video":false,
   "embedded_lyrics":{"present":true,"synced":false,"text":"..."},
   "cover_path":"E:\\Settings\\tmp\\cover-ab12.jpg"}
```

The app shows this for the user to correct title and artist, and to paste lyrics or
pick an `.lrc` file.

## 6. Online lyrics lookup (only when the user presses the button)

```json
> {"type":"lyrics.lookup","id":"6","artist":"...","title":"...","album":null,"duration_seconds":241.3}
< {"type":"lyrics.lookup.result","id":"6","results":[
    {"service":"lrclib","synced_lrc":"[00:12.34] ...","plain":"...","duration_seconds":241.0}]}
```

The Studio never contacts the network on its own.

## 7. Jobs

```json
> {"type":"job.submit","id":"7","job":{
    "job_id":"c1f0...","input":"C:\\Music\\song.mp3","output_dir":"E:\\Library",
    "preset":"full","language":"en",
    "metadata":{"title":"Corrected title","artist":"Artist","album":null},
    "lyrics":{"mode":"auto","text":null,"lrc":null}}}
< {"type":"job.accepted","id":"7","job_id":"c1f0...","position":1}
```

`lyrics.mode`:

- `auto`: use, in order, `lrc`, `text`, lyrics embedded in the file, then
  transcription of the isolated lead vocal (WORK_ORDER 4.1 step 3). An online lookup
  is not part of `auto`; the app runs it first if the user pressed the button, and
  passes the result as `lrc` or `text`.
- `text` / `lrc`: use only what is given (fail the lyrics stage if empty).
- `none`: instrumental, no lyrics.

Events while it runs:

```json
< {"type":"job.started","job_id":"c1f0..."}
< {"type":"job.progress","job_id":"c1f0...","stage":"separate","stage_progress":0.42,
   "overall":0.31,"message":"Separating lead vocal"}
< {"type":"job.done","job_id":"c1f0...","output":"E:\\Library\\Artist - Title.jksong",
   "report":{"seconds":{"decode":1.2,"separate":61.0,"lyrics":20.3,"align":8.8,"analyse":2.1,"write":4.0},
             "lyrics_source":"transcribed","lyrics_checked":false,"warnings":[]}}
< {"type":"job.failed","job_id":"c1f0...","error":{"code":"decode_failed",
   "message":"This file could not be read as audio.","detail":"..."}}
```

Stages, in order: `decode`, `separate`, `lyrics`, `align`, `analyse`, `write`.

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

The output file name is `<artist> - <title>.jksong` with characters that are invalid
on Windows, macOS or exFAT replaced, and a ` (2)` suffix if the name exists. It is
written atomically (docs/SONG_FORMAT.md section 7).

## 8. Re-aligning corrected lyrics (sync editor)

```json
> {"type":"realign","id":"10","song":"E:\\Library\\Artist - Title.jksong",
   "lyrics":{"mode":"text","text":"corrected lyrics..."},
   "anchors":[{"line":3,"start":41.20}]}
< {"type":"realign.result","id":"10","ok":true,"lyrics":{ "...": "lyrics JSON, docs/SONG_FORMAT.md section 5" }}
```

The Studio reads the lead-vocal stem from the song file and aligns the given text,
using any `anchors` (line starts the user fixed by hand) as hard constraints. The app
stores the result in the song file's settings slot as `lyrics_override`.

## 9. Shutdown

```json
> {"type":"shutdown","id":"11"}
< {"type":"shutdown.ok","id":"11"}
```

The Studio finishes writing nothing half-way: a running job is cancelled and its
temporary file deleted. The app also closes stdin on exit; the Studio exits when
stdin closes.

## 10. Errors

```json
< {"type":"error","id":"12","error":{"code":"bad_request","message":"...","detail":"..."}}
```

Error codes: `bad_request`, `unknown_type`, `protocol_mismatch`, `not_found`,
`decode_failed`, `no_gpu`, `out_of_memory`, `models_missing`, `model_checksum`,
`lyrics_missing`, `alignment_failed`, `write_failed`, `cancelled`, `studio_crashed`,
`internal`. `message` is plain language for the user; `detail` is for the log.

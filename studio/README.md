# Jukeoke Studio (Python engine)

Turns a song into a karaoke song on a Windows PC with an NVIDIA GPU: decoding, separation
into lead vocal / backing vocals / drums / other, lyrics, alignment to the letter, key and
tempo, and the `.jksong` writer. The player never imports this package; the Mac build
does not include it.

## Running it

```
python -I -m jukeoke_studio serve --root <root> [--models-dir M] [--temp-dir T]
python -I -m jukeoke_studio make SONG --out DIR [--lyrics-text F | --lrc F | --no-lyrics]
                                 [--lyrics-source online:lrclib] [--preset full|clean|permissive]
                                 [--title T] [--artist A] [--language en] [--root R]
python -I -m jukeoke_studio probe
python -I -m jukeoke_studio models list | fetch [ID ...] [--preset P] [--verify]
python -I -m jukeoke_studio separate SONG OUTDIR [--preset P]     # float WAV stems, debugging
```

- `serve` speaks `docs/STUDIO_PROTOCOL.md` on stdin/stdout (the app starts it; see
  `app/src-tauri/src/studio.rs`). One worker thread runs the job queue; requests are
  answered while a job runs.
- `make` runs one song through exactly the same pipeline and prints the job report
  (per-stage seconds, lyrics source, warnings) as JSON on stdout; progress goes to stderr.
- `models fetch` is the developer's download; the app downloads only through the
  protocol's `models.download`. Nothing else in the Studio downloads anything.

## Where it writes (portable mode, WORKING ORDER 4.8)

`env.py` runs before any heavy import. With `--root <root>` (or `$JUKEOKE_ROOT`, or the
packaged runtime's own location `<root>/Windows/Studio/python/python.exe`):

```
<root>/Models/                     separation weights + manifest.json (SHA-256 records)
<root>/Models/hf/                  alignment + Whisper weights, Hugging Face layout:
                                   models--<org>--<name>/snapshots/<commit>/<files>
<root>/Settings/cache/huggingface  HF_HOME            (nothing is fetched: HF_HUB_OFFLINE=1)
<root>/Settings/cache/torch        TORCH_HOME, PYTORCH_KERNEL_CACHE_PATH (torch/kernels)
<root>/Settings/cache/xdg          XDG_CACHE_HOME
<root>/Settings/cache/cuda         CUDA_CACHE_PATH    (driver JIT cache)
<root>/Settings/cache/numba        NUMBA_CACHE_DIR    (librosa's compiled functions, ~0.3 MB)
<root>/Settings/cache/matplotlib   MPLCONFIGDIR
<root>/Settings/cache/{triton,inductor}  TRITON_CACHE_DIR, TORCHINDUCTOR_CACHE_DIR (unused)
<root>/Settings/tmp/studio/        TMP, TEMP, TMPDIR; job-<id>-<rand>/ scratch (deleted
                                   after every job), covers/ (from `inspect`)
<root>/Library/                    finished songs (the job's output_dir)
```

`HF_HUB_CACHE` and `TRANSFORMERS_CACHE` point at `<root>/Models/hf`. Bytecode writing is
off (`sys.dont_write_bytecode`: `python -I` ignores `PYTHONDONTWRITEBYTECODE`). The app's
supervisor passes the same variables and `--root` / `--models-dir` / `--temp-dir`.
Without a root (a developer running `make` from a checkout) only the Hugging Face
offline switches and the bytecode switch apply, and models come from
`$JUKEOKE_MODELS_DIR`, else `$JK_BUILD/models-cache` (its `hf/` is a directory junction to
the alignment benchmark's cache `$JK_BUILD/align-venv/models/hf` on the build PC).

## Pipeline (pipeline.py)

| stage | what | module |
|---|---|---|
| `decode` | PyAV, first audio stream of any audio or video file, float32 at the native rate by exact scaling (no resampler); > 2 channels folded to stereo (ITU-R BS.775, reported); SHA-256 of the file; tags (FFmpeg, mutagen fallback); cover (JPEG/PNG <= 2 MB kept as is, others converted); the user's metadata corrections; given lyrics parsed | media.py, lyrics.py |
| `separate` | the preset's models (fp32, TF32 off); VRAM freed afterwards | separate.py, presets.py |
| `lyrics` | Whisper large-v3-turbo on the lead stem, only when no lyrics were given or embedded | transcribe.py |
| `align` | CTC alignment (wav2vec2) on the lead stem, letters to the grapheme; user anchors | align.py |
| `analyse` | key and tempo | analyse.py |
| `write` | `.jksong`: residual stem = source minus what the other stems decode to; decoded again and checked (max sum error reported); short temp name, then a no-overwrite rename to `<artist> - <title>.jksong` (` (2)`...) | jksong.py |

Cancellation is cooperative (every long loop checks it) and deletes the job's temporary
files. A failing file sends `job.failed` with the protocol error code and the next job
runs. If the lyrics cannot be aligned, the song is still saved (without lyrics, with a
warning). Errors carry a protocol code (errors.py).

## Separation API

```python
from jukeoke_studio.separate import Separator

sep = Separator(models_dir=None, device="cuda")      # fp32, TF32 off; models load on demand
res = sep.separate(mix, sr, preset="full")           # mix: float32 [channels, frames], any rate
res.stems["lead_vocal"], res.stems["backing_vocals"], res.stems["drums"], res.stems["other"]
res.extras["vocals"], res.extras["instrumental"]     # all vocals; the band without vocals
res.provenance()                                      # SONG_FORMAT 4.3 "models" + "resampling" lists
sep.run_model("bs_roformer_anvuew_ft1", mix, sr)     # one model -> {"vocals": ...}
```

- All arrays are float32 `[channels, frames]` at the input's own sample rate; the four
  stems add up to the input (float32 rounding).
- Weights are never downloaded here: a missing model raises `ModelsMissing`
  (`allow_download=True` exists for the benchmarks).
- A non-44.1 kHz input is resampled (soxr VHQ) once to the model rate; every model runs
  there (the karaoke model on the vocal estimate, the drum model on mix minus vocals) and
  each final estimate comes back to the native rate once. `provenance()["resampling"]`
  lists each conversion. Residual stems are computed at the native rate.
- Presets (`presets.py`): `full` (default; GPL-3.0 + Apache-2.0 weights, commercial use
  allowed), `clean` (adds a direct-instrumental model without a published licence),
  `permissive` (MIT / Apache-2.0 only: Kim's Mel-RoFormer vocals + SYH99999 drums; no
  permissive lead/backing splitter exists, so all vocals go to `lead_vocal` and
  `backing_vocals` is silent). `PRESET_INFO` holds the descriptions the app shows;
  `COMMERCIAL_USE_PRESETS` (formerly the misleading `PERMISSIVE_PRESETS`) lists the
  presets whose every model allows commercial use. `BEST_MEASURED` (pass as `models=`)
  is the highest-scoring model per role regardless of licence, for comparisons by ear.
- Pickled HT-Demucs `.th` weights are re-hashed from the exact bytes before they are
  unpickled; RoFormer / SCNet checkpoints load with `weights_only=True`.

## Layout

- `__main__.py` command line; `env.py` portable environment and stdio; `server.py`
  protocol server; `pipeline.py` the job; `media.py` decoding, tags, cover, inspect;
  `probe.py` capability probe (driver fast path, then a real PyTorch GPU check);
  `errors.py` protocol error codes and job hooks (progress, cancellation);
  `stub.py` stand-in models for tests (`JUKEOKE_STUDIO_STUB=1`).
- `separate.py`, `presets.py` - separation; `modelzoo/` - model catalogue (pinned URLs,
  SHA-256, licences: `registry.py` separation, `hfmodels.py` alignment + Whisper),
  verified downloads (`download.py`), network code (RoFormer, SCNet; MIT).
- `lyrics.py`, `lrc.py`, `transcribe.py`, `align.py`, `analyse.py`, `jksong.py`,
  `quality.py` - lyrics sources, transcription, alignment, key/tempo, song file, metrics.
- `bench/` - evaluation sets and the reproducible benchmarks (`docs/benchmarks/`).
- `tests/` - `pytest` (CPU, a few minutes): the protocol server spoken to over a pipe
  with stand-in models (`test_server.py`: jobs, queue with a bad file, cancellation,
  shutdown, inspect on generated audio/video/tagged files, realign anchors, portable
  environment, models_missing without downloads), separation plumbing, lyrics,
  alignment, song file, downloads. `pytest -m gpu` runs the real models
  (`test_pipeline_gpu.py`: a 30 s CC BY excerpt from JamendoLyrics through the whole
  Studio, 48 kHz, the permissive preset, transcription, realign; every song checked
  with the Rust `jukeoke-cli verify --strict`, found via `$JUKEOKE_CLI` or
  `$JK_BUILD/target-*/release/`), plus `test_gpu.py` and `test_align_model.py`.

Test switches (environment of the Studio process): `JUKEOKE_STUDIO_STUB=1` stand-in
models; `JUKEOKE_STUDIO_STUB_DELAY=<s>` slows the stand-in separation per second of audio
(cancellation tests); `JUKEOKE_STUDIO_FAKE_HW=none|small|ok` fakes the probe's hardware.

## Development

Git Bash, from the repo root: `source scripts/devenv.sh`, then
`uv pip install --python "$JK_BUILD/studio-venv/Scripts/python.exe" -e "studio[dev]"`
(or `uv sync` inside `studio/` using `uv.lock`). Run `python -I -m pytest` in `studio/`
(`-I` works because the package is installed editable in the venv).

`pyproject.toml` pins every runtime dependency of the package at the versions the
benchmarks in `docs/benchmarks/` were measured with; `uv.lock` locks the whole tree for
Windows x86-64 (torch from the PyTorch cu130 index). To reproduce the dev venv exactly:
`cd studio && uv export --frozen --no-hashes --extra dev --no-emit-project > req.txt` and
`uv pip install --python "$JK_BUILD/studio-venv/Scripts/python.exe" -r req.txt`.

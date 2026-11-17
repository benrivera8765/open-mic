# Jukeoke Studio (Python engine)

Turns a song into a karaoke song on a Windows PC with an NVIDIA GPU: separation into
lead vocal / backing vocals / drums / other, then lyrics, alignment and the `.jksong`
writer. The player never imports this package.

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
  stems add up to the input (float32 rounding). Nothing is ever written to an integer
  format here.
- A non-44.1 kHz input is resampled (soxr VHQ) only on its way into a model and back;
  the residual stems are computed at the native rate.
- Presets and the model behind each role: `PRESETS` in `jukeoke_studio/separate.py`.
  The role keys (`vocals`, `karaoke`, `drums`, `instrumental`) are stable API.
  `full` (default) uses only models whose licence allows commercial use;
  `clean` adds a direct-instrumental model without a published licence;
  `BEST_MEASURED` (pass as `models=`) is the highest-scoring model per role regardless
  of licence, for comparisons by ear. Numbers: `docs/benchmarks/separation.md`.
- Precision: fp32 with TF32 off is the only mode used for songs. `precision="fp16"`,
  `"bf16"`, `"tf32"` exist for the benchmark, which measures how far they differ.

## Layout

- `jukeoke_studio/separate.py` - in-memory separation (`Separator`), presets, overlap-add.
- `jukeoke_studio/modelzoo/` - model catalogue (pinned URLs, SHA-256, licences),
  download-on-first-use, network code (RoFormer, SCNet; MIT, from lucidrains / ZFTurbo /
  starrytong; outputs identical to upstream).
- `jukeoke_studio/quality.py` - SDR, residual-leak and band-energy measures.
- `bench/fetch_eval.py`, `bench/separation_bench.py` - evaluation sets and the
  reproducible separation benchmark (`docs/benchmarks/separation.md`).
- `tests/` - `pytest` (CPU; separation plumbing with stand-in models, network-code shapes,
  metrics, the model catalogue against `MODELS.md`, download verification, plus the
  lyrics/alignment/song-file tests); `pytest -m gpu` runs the GPU tests (real default
  models, downloaded weights).

Model weights are downloaded on first use into the models directory
(`$JUKEOKE_MODELS_DIR`, else `$JK_BUILD/models-cache` in a dev checkout) and verified by
SHA-256; `manifest.json` there records what was placed. Licences: `../MODELS.md`.

## Development

Git Bash, from the repo root: `source scripts/devenv.sh`, then
`uv pip install --python "$JK_BUILD/studio-venv/Scripts/python.exe" -e "studio[dev]"`
(or `uv sync` inside `studio/` using `uv.lock`). Run `python -I -m pytest` in `studio/`.

`pyproject.toml` pins every runtime dependency of the package, separation and
lyrics/alignment/analysis alike, at the versions the benchmarks in `docs/benchmarks/`
were measured with; `uv.lock` locks the whole tree for Windows x86-64 (torch from the
PyTorch cu130 index). To reproduce the dev venv exactly:
`cd studio && uv export --frozen --no-hashes --extra dev --no-emit-project > req.txt` and
`uv pip install --python "$JK_BUILD/studio-venv/Scripts/python.exe" -r req.txt`.

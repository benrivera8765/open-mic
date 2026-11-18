# The relocatable Studio runtime

WORK_ORDER 4.8 and trap 4: the Studio (Python + PyTorch) has to run from a USB stick
under any drive letter, on a clean Windows 11 PC with nothing installed, and fit the size
budget (both apps + models under 12 GB, ideally under 9 GB). This page says how the
runtime is built, how it is checked, what was measured, and what is still open.

- Build: `scripts/build-studio-runtime.sh <stick_root> [--keep-cuda] [--keep-all-transformers]`
- Check: `scripts/check-studio-runtime.sh <stick_root> [--separate AUDIO] [--pipeline AUDIO]
  [--snapshot-user-dirs] [--sentinel-profile DIR] [--redirect-caches] [--static-only] [--skip-scan]`
- Staging builds live in `../jukeoke-build/dist-staging/` (never in the repository). The
  current one is `dist-staging/stick1` (built 2026-11-17 20:39 local, 01:39 UTC; runtime + default models).

**State on 2026-11-17 (all measured, details below):** the runtime runs from a copy of the
stick under a different drive letter (`subst Q:`) with nothing installed on the PATH; all 29
checks pass, including a real separation and the whole pipeline on a CC BY excerpt, with
stems and aligned lyrics bit-identical to the unpruned build. Nothing was written to the
per-user folders (sentinel profile: 0 files) or into the runtime. Runtime 4.28 GB (4.79 GB
on a 32 KB-cluster exFAT stick), default models 3.82 GB; estimated stick footprint for both
apps + models **8.66 GB, or 9.04 GB with the NVIDIA compute cache the Studio leaves on the
stick** (8.07 / 8.42 GiB, which is what Windows Explorer calls "GB"): under 12 GB in every
reading, under 9 GB unless "GB" means 10^9 bytes and the cache is counted (section 10).

## 1. Layout

```
<root>/Windows/Studio/
  python/                      python-build-standalone CPython 3.12.16 (20261110 build)
    python.exe, python312.dll  the interpreter (no venv, no pyvenv.cfg, no ._pth)
    msvcp140*.dll, vcruntime140*.dll, vcomp140.dll   Visual C++ runtime 14.51 (section 4)
    DLLs/  Lib/                standard library; Lib/site-packages holds the locked
                               dependencies and jukeoke_studio, installed straight into it
  runtime.json                 what was built (versions, lock hash, package licences,
                               VC runtime, strip record, sizes); no absolute paths
  runtime-build.json           size after every build and strip step
  runtime-binaries.tsv         every .dll/.pyd/.exe: owning package, component, licence
  requirements.pylock.toml     the exact artifacts installed (URLs + SHA-256)
<root>/Models/                 separation weights + hf/ (alignment, Whisper), section 7
```

The app starts it as `python\python.exe -I -X utf8 -m jukeoke_studio serve` with the
working directory `Windows\Studio` (DECISIONS D-83, STUDIO_PROTOCOL 1). `-I` ignores every
`PYTHON*` variable and the user site folder, and `-m` never touches the `Scripts\*.exe`
console-script launchers (which embed the build machine's absolute `python.exe` path), so
the build deletes them.

## 2. Build steps

`build-studio-runtime.sh` (68-77 s on this PC when uv's cache is warm):

1. **Copy** `$JK_BUILD/python/cpython-3.12.16-windows-x86_64-none` to `python/`, minus its
   `EXTERNALLY-MANAGED` marker (it protects a shared interpreter; this copy is private).
2. **Locked dependencies**: `uv export --frozen --no-dev --no-emit-project --format
   pylock.toml` turns `studio/uv.lock` into a list of exact artifact URLs with SHA-256
   (torch from the cu130 index), and `uv pip install --python python\python.exe --no-deps
   --link-mode copy -r pylock.toml` installs exactly those 76 packages, no resolution, files
   copied rather than hard-linked to uv's cache. No venv is created, so nothing records an
   absolute "home" path. The export runs with `--no-header`: uv's default header comment
   repeats the command line, including the absolute `--output-file` path in the build
   user's profile, and `requirements.pylock.toml` ships on the stick (found by the
   independent verifier, section 13; the content is otherwise identical).
3. **jukeoke_studio**: a snapshot of `studio/` (pyproject, README, `jukeoke_studio/*.py`) is
   built and installed non-editable with `--no-build-isolation` (setuptools from the lock).
   The snapshot's SHA-256, the git commit and the number of uncommitted files under
   `studio/` go into `runtime.json`, because the package changes while other work goes on.
4. **Strip**, each step measured (section 3).
5. **Visual C++ runtime** (section 4).
6. **Byte-compile** all of `Lib` with `python -I -B -m compileall -f -j0
   --invalidation-mode unchecked-hash -s <python dir>`: `unchecked-hash` .pyc files are
   used without comparing source timestamps, so copying the stick (Explorer, exFAT's
   timestamp rounding, time zones) can never make Python recompile or rewrite them; `-s`
   keeps the build path out of the code objects (the import system fills in the real path
   at load time). `-B` matters: without it compileall's own worker processes import
   `concurrent.futures` from the tree being compiled and the import system writes a
   *timestamp* .pyc at the same moment as the worker writes the hash-based one; on Windows
   the two renames collided once ("Access is denied" on `concurrent/futures/_base`, leaving
   one timestamp .pyc). py_compile ignores `-B`, so everything is still compiled. The check
   now fails on any .pyc that is not unchecked-hash (positive control: that broken build
   was reported as `{'unchecked-hash': 8482, 'timestamp': 1}`; the fixed build has 8,483
   unchecked-hash and nothing else). Measured: 0 files not compilable; no build path in any
   .pyc; no file appears or changes in the runtime while the Studio runs (section 5).
7. **Metadata**: `direct_url.json` of jukeoke_studio (a `file:///C:/Users/...` URL) is
   deleted together with its RECORD line; then `runtime.json`, `runtime-build.json` and
   `runtime-binaries.tsv` are written.

The helper code runs with the *source* interpreter, never the one being built: step 5
replaces `python\vcruntime140.dll`, which a running copy of the runtime would hold open.

## 3. What is stripped, and the size after each step

Measured on 2026-11-17 20:39 (`stick1/Windows/Studio/runtime-build.json`, build log
`dist-staging/build-stick1-final.log`). "exFAT" columns are the space the files take on an
exFAT stick: every file and every directory occupies whole clusters. Windows formats
volumes up to 32 GiB with 32 KB clusters (a "32 GB" stick is about 29 GiB), larger ones
with 128 KB.

| step | removed | size | files | dirs | on exFAT, 32 KB clusters | 128 KB clusters |
|---|---|---|---|---|---|---|
| copy interpreter | | 0.066 GB | 3,506 | 269 | 0.168 GB | 0.520 GB |
| install locked deps (76 packages) | | 4.699 GB | 28,819 | 2,674 | 5.535 GB | 8.446 GB |
| install jukeoke_studio | | 4.699 GB | 28,854 | 2,677 | 5.536 GB | 8.451 GB |
| strip caches + installers + launchers (`__pycache__` from the source tree, pip, ensurepip, `Scripts/`) | 21.3 MB, 1,242 files | 4.678 GB | 27,612 | 2,515 | 5.479 GB | 8.266 GB |
| strip tests (`tests/`, `test/` dirs, CPython's `_test*.pyd`; never inside `*.dist-info`) | 50.4 MB, 3,184 files | 4.628 GB | 24,428 | 2,233 | 5.345 GB | 7.826 GB |
| strip development files (headers, `include/`, `libs/`, `*.lib`, `*.a`, `*.pdb`, Cython `*.pxd/*.pyx/*.pxi`, `*.cmake`, `torch/share`, `torch/bin/protoc.exe`) | 96.2 MB, 10,213 files | 4.532 GB | 14,215 | 1,962 | 4.935 GB | 6.394 GB |
| strip Tcl/Tk, IDLE, turtle, `share/` (man pages) | 12.1 MB, 1,407 files | 4.519 GB | 12,808 | 1,907 | 4.880 GB | 6.198 GB |
| strip transformers model families the Studio cannot load (all of `transformers/models/` except `auto`, `wav2vec2`, `hubert`; 544 folders) | 44.2 MB, 2,523 files | 4.475 GB | 10,285 | 1,362 | 4.760 GB | 5.795 GB |
| strip unused CUDA libraries (section 6) | 369.3 MB, 7 files | 4.106 GB | 10,278 | 1,360 | 4.390 GB | 5.425 GB |
| bundle VC runtime | +1.0 MB | 4.107 GB | 10,283 | 1,360 | 4.391 GB | 5.426 GB |
| byte-compile | +173 MB, +8,483 .pyc | **4.280 GB** | **18,766** | 2,340 | **4.794 GB** | **6.695 GB** |

Not stripped, on purpose: type stubs (`*.pyi`): librosa's `lazy_loader.attach_stub` reads
`librosa/__init__.pyi` at import time; the first build stripped them and librosa failed to
import (caught by the check). Licence files and SBOMs inside `*.dist-info` are never touched.

**The transformers step** (new on 2026-11-17 evening) saves 0.094 GB, 5,046 files (sources +
their .pyc) and 0.246 GB on a 32 KB-cluster stick; the file count also makes copying the
stick faster. Why it is safe: the Studio loads only CTC character models of the wav2vec2
and HuBERT families (`align.py` through `AutoModelForCTC`, registry
`modelzoo/hfmodels.py`); Whisper runs on CTranslate2, not transformers. transformers 5.23
finds its models by scanning `models/` at import (`define_import_structure`) and the Auto
classes import a family only when a config of that `model_type` is loaded. Proof (sections
5 and 8): the check builds both families through `AutoConfig.for_model` +
`AutoModelForCTC.from_config` and runs them on the GPU, reads the `model_type` of every
alignment model in the registry and imports it, and the real separation + pipeline give
bit-identical stems, transcript, aligned lyrics, key/tempo and song chunks with and without
the step. A new alignment family must be added to `TRANSFORMERS_KEEP` in the build script;
the check fails until it is. `--keep-all-transformers` turns the step off.

Run-to-run reproducibility: two builds from the same inputs differ in 3 of 18,765 files:
uv's `uv_cache.json` timestamp in the jukeoke_studio dist-info (and its line in RECORD), and
one `.pyc` (`sklearn/linear_model/_stochastic_gradient`) whose source is identical; marshal
output depends on the per-process string-hash seed, which `-I` does not let the build fix
(`PYTHONHASHSEED` is ignored). Both files are equivalent at run time.

### Size by component (final runtime)

| component | size | files | on exFAT, 32 KB |
|---|---|---|---|
| torch: NVIDIA CUDA 13 libraries in `torch/lib` (cuBLAS 13 528 MB, cuDNN 9 568 MB, cuFFT 285 MB, NVRTC 13 187 MB, cuSPARSE 150 MB, cuSOLVER 126 MB, nvJitLink 88 MB, CUPTI, NVTX, cudart) | 1,935.4 MB | 24 | 1,935.8 MB |
| cuBLAS 12 (`nvidia-cublas-cu12`, for CTranslate2 only) | 771.3 MB | 9 | 771.5 MB |
| torch: own binaries (`torch_cuda.dll` 423 MB, `torch_cpu.dll` 306 MB, ...) | 753.1 MB | 12 | 753.3 MB |
| llvmlite + numba (librosa) | 135.7 MB | 788 | 153.9 MB |
| torch: Python files | 113.1 MB | 5,090 | 232.0 MB |
| scipy (+ scipy.libs) | 104.0 MB | 1,344 | 134.4 MB |
| av (PyAV + FFmpeg) | 71.3 MB | 258 | 77.4 MB |
| other site-packages | 66.9 MB | 4,524 | 175.7 MB |
| ctranslate2 | 64.9 MB | 65 | 66.6 MB |
| onnxruntime (faster-whisper's VAD; the Studio runs with `vad_filter=False`) | 54.7 MB | 633 | 69.4 MB |
| sympy + mpmath (torch) | 43.4 MB | 1,851 | 84.6 MB |
| CPython interpreter + standard library | 42.4 MB | 1,229 | 71.5 MB |
| numpy (+ numpy.libs) | 38.0 MB | 567 | 52.2 MB |
| scikit-learn (librosa) | 28.4 MB | 876 | 47.3 MB |
| tokenizers + safetensors + hf_xet + huggingface_hub | 25.9 MB | 458 | 36.7 MB |
| transformers (was 111 MB / 5,729 files before the model-family step) | 17.8 MB | 683 | 32.5 MB |
| librosa + soundfile + soxr + audio helpers | 10.5 MB | 257 | 16.8 MB |
| faster_whisper | 1.5 MB | 26 | 2.1 MB |
| Visual C++ runtime (interpreter folder) | 1.2 MB | 7 | 1.3 MB |
| jukeoke_studio | 0.9 MB | 57 | 2.1 MB |
| total (files only; with directory clusters 4.794 GB) | 4,280 MB | 18,766 | 4,717 MB |

NVIDIA's redistributable libraries are 2.71 GB of the 4.28 GB (63 %); with torch's own CUDA
binaries, GPU code is 3.46 GB (81 %). Everything that exists only for Whisper on CTranslate2
(cuBLAS 12, ctranslate2, onnxruntime, faster_whisper) is 0.89 GB (section 12).

## 4. Visual C++ runtime and CTranslate2

`torch_cuda.dll`, `torch_cpu.dll`, `torch_python.dll` and `ctranslate2.dll` import
`MSVCP140.dll`, `VCRUNTIME140_THREADS.dll` and `MSVCP140_ATOMIC_WAIT.dll`; torch's
`__init__` also loads `msvcp140.dll` by bare name. A clean Windows 11 has none of them
(they come with the Visual C++ Redistributable, which this PC happens to have, version
14.51.36247). Wheels bundle partial copies (scikit-learn's `.libs/msvcp140.dll`, mangled
copies in `numpy.libs` and `llvmlite.libs`), but torch loads its copy before any of those
folders is on the DLL search path.

The build therefore resolves every import table of every `.dll/.pyd/.exe` (397 files; own
PE parser, static and delay-load imports) the way the loader will: next to the importer,
in the interpreter folder, in the folders the Studio adds (`torch/lib`, `nvidia/*/bin`,
`*.libs`), or in Windows itself (System32 file with a Microsoft company name that is not a
redistributable). What is left over and is a VC runtime DLL is copied into `python\` (the
application directory, searched first): `msvcp140.dll`, `msvcp140_1.dll`,
`msvcp140_atomic_wait.dll`, `vcruntime140_threads.dll`, `vcomp140.dll`, and the interpreter's
own `vcruntime140.dll` / `vcruntime140_1.dll` are replaced (14.44.35211 -> 14.51.36247) so
the whole process runs one runtime version. Source: this PC's installed VC++ 2015-2022
redistributable (`%SystemRoot%\System32`, the same version scikit-learn's wheel bundles);
`--vcredist DIR` / `JK_VCREDIST_DIR` takes a Visual Studio `Microsoft.VC143.CRT` folder
instead. Anything else unresolved fails the build. The single allowed exception is
`tbb12.dll`, imported by numba's optional TBB threading layer (`numba/np/ufunc/tbbpool`);
TBB is not a Studio dependency and numba falls back to its OpenMP/workqueue layer, exactly
as in the development venv.

Proof that the bundled copies are the ones used: every check process lists the modules it
loaded (`EnumProcessModulesEx`); every VC runtime module came from `python\`, none from
System32 (check "VC runtime came from the runtime folder"). Taking the DLL out again makes
the static check fail ("from a redistributable installed on this PC: msvcp140.dll";
positive control run on 2026-11-17).

CTranslate2 4.9 (faster-whisper) is a CUDA 12 build and needs cuBLAS 12
(`cublas64_12.dll`, `cublasLt64_12.dll` from `nvidia-cublas-cu12`, 771 MB) next to torch's
CUDA 13. It loads them by bare name, so `jukeoke_studio.transcribe.add_cuda12_dll_dirs`
preloads them from `nvidia\cublas\bin` (DECISIONS D-54). The check runs a cuBLAS 12 SGEMM on
the GPU through that very DLL (on torch-allocated memory) and a real Whisper large-v3-turbo
transcription on CUDA in fp16.

## 5. The check

`check-studio-runtime.sh <root>` runs with the runtime's own interpreter (`-I -B`, standard
library only) and starts every real test as a separate `python -I -X utf8` child with a
clean environment: only the variables a fresh Windows account has, `PATH` reduced to
`%SystemRoot%` folders (so no CUDA toolkit, Python or VC runtime elsewhere on the PC can
stand in for a missing file), plus `JUKEOKE_MODELS_DIR` and `HF_HUB_OFFLINE=1`. Every check
child starts like the Studio's own entry points: `jukeoke_studio.env.setup()` before any
heavy import, so the cache and temp redirection under test is the package's code, not the
harness's (`--redirect-caches` only matters for package builds without `env.py`). An earlier
harness without that step let librosa's numba functions write 10 cache files into
`librosa/__pycache__` inside the runtime: a harness artefact (the real `-m jukeoke_studio`
start-up sets `NUMBA_CACHE_DIR`), fixed by making the children start the same way.

Static checks (about 50 s; they read every byte of the 4.28 GB):

- every `.pyc` is unchecked-hash (section 2, step 6);
- no build-machine path (`%USERPROFILE%`, the repository, `$JK_BUILD`) anywhere, in UTF-8,
  UTF-16LE and JSON-escaped form: text, .pyc, binaries, launchers, and (since the verifier
  fix of section 13) the build's own files next to `python/` (`runtime.json`,
  `runtime-build.json`, `runtime-binaries.tsv`, `requirements.pylock.toml`), always scanned,
  also with `--skip-scan`;
- the runtime's `jukeoke_studio` is the one in this checkout: the hash of its `*.py` files
  (computed like build step 3) is compared with `runtime.json` and with `studio/`; a
  difference is a warning to rebuild before shipping (verifier addition, section 13);
- `.pth`, `._pth`, `.cfg`, `RECORD`, `direct_url.json` hold no absolute path (other
  `.json`: 48 mentions of `D:/a/...`, the wheel builders' CI paths inside the SBOMs of
  hf_xet, safetensors, tokenizers and pydantic_core; reported, harmless);
- not a venv, no `._pth`, no console-script launcher (an exe holding an absolute
  `python.exe` path; setuptools' `cli*.exe` templates hold none);
- no symlinks or junctions (exFAT has none), every name valid on exFAT;
- MAX_PATH: the longest path below the stick root is 167 characters (a licence file deep in
  `torch-2.14.1+cu130.dist-info`), so 170 from a drive root. Windows without long-path
  support (the default; this PC has it on) allows 259, so a copy of the stick works in any
  folder whose own path is at most 91 characters (`C:\Users\<name>\Desktop\Jukeoke` is
  fine; a deeply nested Downloads folder may not be);
- every imported DLL resolvable (section 4).

Positive controls (2026-11-17, `--static-only`): a planted `.pth` with an absolute path, a
UTF-16 file and a JSON file containing `%USERPROFILE%`, and a uv console-script launcher
copied from the development venv were all reported (4 files, 3 failed checks); removing
`msvcp140.dll` failed the import check.

Runtime checks (each a separate process; results with numbers in `check-result.json`).
Results from the relocated copy on `Q:\` (section 8, run 1, `dist-staging/check-subst-1.log`):

| check | what it does | result (2026-11-17 20:30, RTX 5070 Ti, driver 621.24) |
|---|---|---|
| interpreter | `sys.prefix`, `base_prefix`, `executable`, every `sys.path` entry and `sysconfig` path inside the runtime; isolated; no user site | pass |
| Studio start-up | what `env.setup()` decided: root, and that every variable it set (caches, temp, HF) points into the stick | 15 variables, none outside the stick (root reached through the subst mapping, section 8) |
| torch | CUDA fp32 matmul 1024² vs CPU float64; `torch.stft` on the GPU (cuFFT) vs CPU; `conv1d` (cuDNN) vs CPU; TF32 off | rel. errors 7.8e-7, 2.1e-7, 1.3e-7; cuDNN 9.24, sm 12.0 |
| transformers | wav2vec2 and HuBERT CTC models built through `AutoConfig`/`AutoModelForCTC`, forward pass on the GPU; every registered alignment model's family importable | `Wav2Vec2ForCTC` and `HubertForCTC` [1, 49, 32]; both registry models' families present; 3 model families left in transformers |
| ctranslate2 / faster_whisper | CUDA devices and compute types; cuBLAS 12 `cublasSgemm_v2` on the GPU vs float64; Whisper large-v3-turbo on CUDA fp16 | SGEMM rel. error 2.2e-7; float16 available; Whisper ran on cuda (23.9 s with a cold compute cache, section 9) |
| librosa | key of a I-IV-V-I cadence in C (CQT chroma) and tempo of a 120 BPM click track (numba JIT) | C major (confidence 1.0), 117.5 BPM |
| av | 24-bit FLAC written by libsndfile, decoded by FFmpeg, compared bit for bit; decoders present | bit-exact; mp3, aac, flac, vorbis, opus, alac, wmav2, pcm, h264 |
| soundfile | 24-bit FLAC round trip | exact (libsndfile 1.2.2) |
| studio_import | imports every `jukeoke_studio` module | no import errors |
| protocol | `-m jukeoke_studio serve`: hello, probe, models.status, shutdown (falls back to `probe` or an import check on older package builds) | hello.ok in 0.1 s; probe 3.5 s, ok_for_studio, 16275 MB VRAM; all models present; clean shutdown; 0 non-JSON stdout lines |
| `--separate` | `python -I -X utf8 -m jukeoke_studio separate <audio> <out> --preset full`, then the four stems are read back and summed | section 8 |
| `--pipeline` | in one process: PyAV decode, separation (`full`), Whisper transcription of the lead stem, wav2vec2 alignment, key/tempo, `.jksong` written and reopened; SHA-256 of every stem, the transcript and the aligned lyrics | section 8 |
| modules | every module any child loaded: from the runtime, from Windows, or from the NVIDIA driver (DriverStore) | 337 modules: 263 runtime, 63 Windows, 11 driver, 0 redistributable, 0 other |
| runtime unchanged | the runtime folder is listed before and after: nothing may appear, change or disappear | 0 / 0 / 0 |
| `--snapshot-user-dirs` | lists `%USERPROFILE%\.cache`, `%APPDATA%`, `%LOCALAPPDATA%` (holds `%TEMP%`) before and after; every new or changed entry is attributed: the Studio's libraries (NVIDIA ComputeCache, Hugging Face, torch, numba, pip, Python temp files, ...), known other programs (browsers, OneDrive, chat apps, the other work streams' compilers and pytest runs), or unattributed | section 9 |
| `--sentinel-profile DIR` | every child gets `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `TEMP`/`TMP` in empty folders outside the stick; any file appearing there is the Studio's, with no noise from other programs | section 9 |
| stick writes | size of `<root>/Settings/cache/*` and `Settings/tmp/studio` after the run | section 9 |

## 6. CUDA libraries removed, and the proof

torch's `__init__` loads *every* DLL in `torch\lib` at import, so "was it loaded" cannot
tell what is needed; removal was proven by running the whole pipeline before and after.
Removed (369 MB):

| file | size | why it is unused |
|---|---|---|
| `torch/lib/cusolverMg64_12.dll` | 95 MB | multi-GPU dense solver; nothing imports it (import tables) and the Studio does no linear algebra solves |
| `torch/lib/curand64_10.dll` | 59 MB | host cuRAND library; not in any import table; torch's CUDA RNG is its own Philox kernels, CTranslate2 uses cuRAND's header-only device API |
| `torch/lib/nvperf_host.dll` | 28 MB | CUPTI's profiler counters (CUPTI itself stays: `torch_cpu.dll` imports it) |
| `nvidia/cublas/bin/nvblas64_12.dll` | 0.7 MB | NVBLAS, a drop-in for a CPU BLAS; never loaded |
| `nvidia/cuda_nvrtc/` (`nvidia-cuda-nvrtc-cu12`) | 187 MB | CUDA 12 NVRTC, pulled in as a dependency of cuBLAS 12; its folder is never added to the DLL search path (only `nvidia\cublas\bin` is), so it could not be loaded even before |

Kept although large, because `torch_cuda.dll` imports them statically or the pipeline uses
them: cuFFT (STFT in every RoFormer), cuSPARSE + nvJitLink, cuSOLVER, cuBLAS 13, NVRTC 13
(+ its `.alt` variant that cuDNN's runtime-compiled engines load), all of cuDNN 9
(wav2vec2's and Whisper's convolutions).

Proof (10 s CC BY excerpt, section 8): built with `--keep-cuda`, then without, same
excerpt through `--separate` and `--pipeline`: the four stems' float32 SHA-256 (from both
the CLI and the in-process pipeline), every transcribed word with its start/end time, the
SHA-256 of the aligned lyrics JSON, the detected key and tempo, and the song file's chunk
list are **identical**. The stems are also bit-identical to the ones the development venv
(`studio-venv`, the benchmarks' environment) produces for the same file.

## 7. Models on the stick

Copied, not downloaded, for the relocation test (only what the defaults need):

| file | for | size |
|---|---|---|
| `Models/bs_roformer_ft1_anvuew_sdr_12.55.ckpt` | vocals (full preset) | 204,493,312 B |
| `Models/karaoke_bs_roformer_anvuew.ckpt` | lead vs backing | 204,486,925 B |
| `Models/bs_roformer_4stems_ft_syh.pth` | drums | 527,245,586 B |
| `Models/hf/models--facebook--wav2vec2-large-960h-lv60-self/` (rev 54074b1c) | alignment | 1,262,057,355 B |
| `Models/hf/models--mobiuslabsgmbh--faster-whisper-large-v3-turbo/` (rev 0a363e91) | transcription | 1,621,672,198 B |
| total (21 files + `manifest.json` the Studio writes after verifying) | | **3.820 GB** |

The Hugging Face folders are plain files (no symlinks: Windows without developer mode
makes copies), so they survive exFAT. The Studio re-hashes the separation weights the
first time it sees them on a stick (about 3 s for 0.94 GB) and records them in
`Models/manifest.json` on the stick (deleted before the relocation test below, so that run
verified the weights again from the copy). On exFAT the 21 files and 14 folders take
3.821 GB with 32 KB clusters (3.824 GB with 128 KB).

These are the defaults (MODELS.md). Other presets and options add weights when the user
picks them (HuBERT alignment, Whisper large-v3, the `clean` / `permissive` separation
models); they are downloaded on first use, never shipped.

## 8. Relocation test

2026-11-17, 20:20-20:45. Input: `rxbyn-bad-side-10s.wav`, a 10.0 s excerpt (17-27 s) of
"Bad Side" by Rxbyn (CC BY, JamendoLyrics, decoded with PyAV to float32 44.1 kHz stereo; kept
in `../jukeoke-build/dist-staging/inputs/`, never in the repository).

1. `scripts/build-studio-runtime.sh $JK_BUILD/dist-staging/stick1` (68 s), then the default
   weights into `stick1/Models` (section 7, `manifest.json` removed) and empty `Library/`,
   `Recordings/`, `Settings/`.
2. `cp -a stick1 stick2` (8.10 GB, 53 s), then **`stick1` renamed to `stick1.hidden`**, so any
   reference left to the build location would fail instead of silently working.
3. `subst Q: ...\dist-staging\stick2` (Q: was the first free letter), the input copied to
   `Q:\Recordings\`.
4. Run 1: `scripts/check-studio-runtime.sh Q:/ --separate Q:/Recordings/rxbyn-bad-side-10s.wav
   --pipeline Q:/Recordings/rxbyn-bad-side-10s.wav --snapshot-user-dirs --out
   Q:/Settings/tmp/check-1`: **29 passed, 0 failed**, 5 min 14 s (log
   `dist-staging/check-subst-1.log`, JSON `dist-staging/results/relocation-Q/check-1/`).
   No cache redirection by the harness: the Studio's own `env.setup()` did it.
5. Run 2: `Settings/cache` and `Settings/tmp/studio` deleted (every library starts cold),
   then the same with `--skip-scan --sentinel-profile $JK_BUILD/tmp/sentinel-profile-203538`
   instead of the snapshot: **0 failed** (`check-subst-2-sentinel.log`,
   `results/relocation-Q/check-2-sentinel/`).
6. `subst Q: /D` (from Git Bash: `MSYS_NO_PATHCONV=1 subst Q: /D`, or MSYS turns `/D` into a
   path); `subst` lists no mapping afterwards. A stale `J:` mapping left by an interrupted
   earlier attempt was removed the same way before step 1.
7. `stick1.hidden` renamed back and rebuilt with the corrected FFmpeg licence label
   (section 11); compared file by file (size + SHA-256) with the tested copy: 3 of 18,765
   files differ, all benign (section 3, reproducibility). `stick2` was then deleted.

Real separation from `Q:` (`python -I -X utf8 -m jukeoke_studio separate ... --preset full`,
weights from `Q:\Models`): exit 0; 24.6 s process wall, 23.3 s separation (run 2: 22.5 /
21.1 s); four stems, 441,000 frames at 44.1 kHz; the stems sum back to the input within
5.96e-8 (one float32 step at this level); RMS lead -16.74 dBFS, backing -52.05, drums
-17.70, other -16.74.

Whole pipeline in one process from `Q:` (run 1 / run 2): decode bit-exact (PyAV vs
libsndfile); separate 13.5 / 10.7 s, Whisper large-v3-turbo on CUDA fp16 5.0 / 4.3 s, wav2vec2
alignment 4.8 / 3.1 s, key/tempo 1.2 / 0.9 s, `.jksong` written and reopened 0.46 / 0.39 s;
transcript "four five six i know that you want this am i right think i'm right let's skip
the game", 19 aligned words, no lyrics problems; G minor (0.958), 110 BPM; peak VRAM 2.36 GB.

Identical results, field by field (`check-result.json` compared with a script): the stems'
float32 SHA-256 from both the CLI and the in-process pipeline, the separation's sum error,
every transcribed word with its times, the SHA-256 of the aligned lyrics (`88d2ef87...`),
key and tempo, decode exactness and the song's chunk list are the same in runs 1 and 2, in
the earlier relocated run of the *unpruned* runtime (same jukeoke_studio tree, SHA-256
`514d12ce...`, results kept in `results/prev-unpruned-subst/`), and in the `--keep-cuda`
baseline of section 6 (9 of 9 fields). So neither the relocation, nor the CUDA strip, nor the
transformers strip changes a single bit of output.

One observation for the Studio code (not this stream's file): `env.setup()` resolves its
root with `Path.resolve()`, which undoes `subst`: launched from `Q:\`, it reported the root
as `C:\...\dist-staging\stick2`. That is the same folder, so nothing went outside the stick,
and on a real USB drive the letter stays. On a mapped network drive `resolve()` would give a
UNC path; `os.path.abspath` keeps the path as launched (proposal 6 in section 12).

## 9. Writes outside the stick

**Sentinel profile (exact).** In run 2 every check process (the CLI separation, the serve
protocol session, the in-process pipeline and every library check) had `USERPROFILE`,
`APPDATA`, `LOCALAPPDATA` and `TEMP`/`TMP` pointing at empty folders outside the stick:
**0 files** appeared there, with every cache starting cold. Positive control: the same
`python.exe` from `Q:` running one CUDA addition *without* the Studio's start-up wrote 5
files, 27.4 MB, into the sentinel's `AppData\Roaming\NVIDIA\ComputeCache`, so the method sees
the driver's default cache location. It cannot see writes through the Windows known-folder
API (which ignores those variables); the snapshot below covers that.

**Real per-user folders (run 1).** 1,172,965 entries listed before (111 s) and after the
154 s run: 14 new, 56 changed, 6 deleted. **0** match what the Studio's libraries write;
68 are other programs on this PC (Firefox, the Claude app, OneDrive, Discord, Docker
Desktop, UWP apps); 2 unattributed: Docker's `AppData\Local\Docker\wsl\disk\docker_data.vhdx`
and `AppData\Local\Temp\.ses`, neither from the Studio. (The earlier attempt's runs on this
machine flagged 55 "Studio-like" entries that were other streams' pytest runs under
`Temp\pytest-of-<user>` and GCC temp files; the attribution rules now name those writers.)

**Inside the runtime:** nothing new, changed or deleted in either run.

**What the Studio leaves on the stick** (both cold runs alike):

| folder | after the check (one 10 s song + all library checks) |
|---|---|
| `Settings/cache/cuda` (`CUDA_CACHE_PATH`) | 375.4 MB, 81 files |
| `Settings/cache/numba` | 0.2 MB, 10 files |
| `Settings/cache/inductor` | empty folder |

The NVIDIA compute cache is the one that matters for size. It is filled on first use of
each GPU kernel module, per GPU model and driver version, and it is not only PTX JIT output:
with `CUDA_DISABLE_PTX_JIT=1` torch's kernels still ran (torch 2.14.1+cu130 ships sm_120 code)
and the cache still grew by 67 MB for an add and a matmul. It is worth keeping: three small
ops took 8.2 s with an empty cache, 0.3 s with a warm one, and 6.8 s every time with
`CUDA_CACHE_DISABLE=1`. Measured per component with empty caches: torch matmul + STFT
79 MB, the 10 s separation 109 MB, wav2vec2 forward 80 MB, Whisper (CTranslate2) 91 MB. In
the earliest runs it was invisible because `%APPDATA%\NVIDIA\ComputeCache` on this PC was
already warm (804 MB) from development work: on a clean PC without the redirection, the
Studio would have written there. With `env.setup()` it goes to the stick, as measured.

## 10. Size budget

Measured 2026-11-17 except where marked. "exFAT" = space taken on a 32 KB-cluster exFAT
stick (Windows' default for a 32 GB stick). GB = 10^9 bytes; GiB = 2^30 bytes, which is
what Windows Explorer labels "GB".

| item | size | on exFAT (32 KB) | source |
|---|---|---|---|
| Studio runtime (`Windows/Studio`, 18,769 files) | 4.281 GB | 4.794 GB | measured, `stick1` |
| default models (`Models`, 21 files) | 3.820 GB | 3.821 GB | measured, section 7 |
| Windows app (`Jukeoke.exe` 12.4 MB + `resources/demo-song.jksong` 2.4 MB) | 0.015 GB | 0.015 GB | measured, `../jukeoke-build/verify-integ-stick/Windows` |
| Mac app | 0.030 GB budgeted | 0.030 GB | estimate: the same Tauri app and resources for one architecture (arm64) come to about the Windows size (~0.015 GB); twice that is budgeted for the bundle's extra files and icons |
| **apps + runtime + models** | **8.15 GB** | **8.66 GB (8.07 GiB)** | |
| NVIDIA compute cache on the stick, one GPU + driver | 0.375 GB | ~0.378 GB | measured, section 9 |
| **with the cache** | **8.52 GB** | **9.04 GB (8.42 GiB)** | |

Against the budget: **under 12 GB in every reading. Under 9 GB as Explorer counts (8.42
GiB with the cache), and under 9 x 10^9 bytes without the cache (8.66); 9.04 x 10^9 bytes
with it**, so the "ideally under 9 GB" target is met or missed by 0.04 GB depending on the
unit and on whether a runtime cache counts. A 128 KB-cluster stick (64 GB or larger) would
need 6.70 GB for the runtime alone. Each further PC the stick creates songs on adds its own
compute-cache entries until the driver's size limit evicts old ones (proposal 2).

Applied in this round (safe and proven, section 3): the transformers model-family strip,
-0.094 GB raw / -0.246 GB on exFAT / -5,046 files. Earlier: the CUDA strip (-0.369 GB).

## 11. Licences of the bundled binaries

397 `.dll/.pyd/.exe` files; the full list with the owning package, component, licence and
licence-file paths is `Windows/Studio/runtime-binaries.tsv` (input for
THIRD_PARTY_LICENSES.md, which another step writes). Third-party components inside wheels,
by size:

| component | licence | files | size | shipped by |
|---|---|---|---|---|
| NVIDIA cuBLAS 13 + cuBLAS 12 | NVIDIA CUDA Toolkit EULA (redistributable runtime, Attachment A) | 4 | 1,299 MB | torch, nvidia-cublas-cu12 |
| PyTorch | BSD-3-Clause (package expression also lists Apache-2.0, BSD-2-Clause for vendored parts) | 8 | 751 MB | torch |
| NVIDIA cuDNN 9 | NVIDIA cuDNN Software License Agreement (redistribution allowed) | 11 | 569 MB | torch, ctranslate2 (its `cudnn64_9.dll`) |
| NVIDIA cuFFT, NVRTC, cuSPARSE, cuSOLVER, nvJitLink, CUPTI, cudart, NVTX | NVIDIA CUDA Toolkit EULA (redistributable runtime) | 11 | 839 MB | torch |
| LLVM (llvmlite) | BSD-2-Clause AND Apache-2.0 WITH LLVM-exception | 1 | 120 MB | llvmlite |
| CTranslate2 | MIT | 1 | 61 MB | ctranslate2 |
| SciPy (+ its OpenBLAS / gfortran runtime in `scipy.libs`) | BSD-3-Clause (OpenBLAS BSD-3-Clause; libgfortran GPL-3.0-or-later WITH GCC-exception-3.1) | 106 + libs | 56 MB + 41 MB OpenBLAS (with numpy's) | scipy, numpy |
| ONNX Runtime | MIT | 3 | 46 MB | onnxruntime |
| FFmpeg (libavcodec 63) | LGPL-3.0-or-later: its own `av_license()` string and configure line (`--enable-version3`, no `--enable-gpl`); LAME linked in statically (LGPL-2.0-or-later) | 7 | 32 MB | av |
| x264, x265 | GPL-2.0-or-later | 2 | 15 MB | av |
| SVT-AV1 | BSD-3-Clause-Clear (+ AOMedia patent licence) | 1 | 7.7 MB | av |
| dav1d, libvmaf | BSD-2-Clause, BSD-2-Clause-Patent | 2 | 3.8 MB | av |
| libvpx, libwebp, Opus | BSD-3-Clause | 5 | 4.0 MB | av |
| GNU libiconv | LGPL-2.1-or-later | 1 | 1.1 MB | av |
| libstdc++, libgcc, libwinpthread (MinGW runtime) | GPL-3.0-or-later WITH GCC-exception-3.1; libwinpthread MIT/BSD | 3 | 2.9 MB | av |
| Intel oneVPL dispatcher | MIT | 1 | 0.4 MB | av |
| libsndfile (with FLAC, Ogg, Vorbis, Opus, mpg123, LAME) | LGPL-2.1-or-later (codecs BSD-3-Clause / LGPL) | 1 | 2.4 MB | soundfile |
| libsoxr | LGPL-2.1-or-later | 1 | 0.4 MB | soxr |
| LAME (lameenc) | LGPL-3.0-or-later (package) | 1 | 0.2 MB | lameenc (a demucs dependency) |
| Intel OpenMP runtime (`libiomp5md`) | Intel Simplified Software License | 3 | 3.3 MB | torch, ctranslate2 |
| Microsoft Visual C++ runtime 14.51 | Visual Studio "Distributable Code" terms (redistributable) | 11 | 3.2 MB | copied by the build (7), wheels (4) |
| CPython 3.12.16 + its OpenSSL 3, SQLite, libffi, zlib | PSF-2.0; Apache-2.0; public domain; MIT; Zlib | 11 | 19 MB | python-build-standalone |
| libuv, zlib (torch) | MIT, Zlib | 2 | 0.3 MB | torch |
| Rust/C extension modules of the Python packages (tokenizers, hf-xet, pydantic-core, safetensors, regex, sphn, numpy, scikit-learn, numba, protobuf, PyYAML, msgpack, cffi, charset-normalizer, MarkupSafe, av's own modules, setuptools' launcher templates) | the package's licence: Apache-2.0 (tokenizers, hf-xet, safetensors, sphn, msgpack), Apache-2.0 AND CNRI-Python (regex), MIT (pydantic-core, PyYAML, charset-normalizer, setuptools), MIT-0 (cffi), BSD-3-Clause (numpy, scikit-learn, protobuf, MarkupSafe, av), BSD-2-Clause (numba) | ~175 | 51 MB | |

Licence points for THIRD_PARTY_LICENSES.md and the owner:

- Everything is compatible with distributing Jukeoke under GPL-3.0-or-later: the copyleft
  parts are LGPL-2.1+/LGPL-3.0+ (FFmpeg, libsndfile, libsoxr, libiconv, LAME) and GPL-2.0+
  (x264, x265), all of which allow GPL-3.0 distribution.
- The NVIDIA libraries (2.71 GB) and the Visual C++ runtime are **redistributable but not
  open source**: their EULAs allow shipping them with an application and require their
  notices; they must be listed with their licence texts (NVIDIA's are in the wheels'
  `*.dist-info` and in `torch-*.dist-info/licenses`).
- The earlier build labelled the FFmpeg DLLs "GPL build (--enable-gpl)"; the strings in the
  DLLs say otherwise, so the label is corrected in the build script (this round).
- The Intel OpenMP runtime comes under the Intel Simplified Software License (redistributable
  in binary form).

## 12. Open points and proposals

Proposals to cut size further. None is applied: each changes another stream's code or what
the Studio computes, so each needs that stream (or the owner) and a measurement.

1. **Transcription on torch instead of CTranslate2** (saves 0.89 GB, the biggest single
   item): run Whisper large-v3-turbo through transformers' Whisper (CUDA 13, the torch
   already shipped) and drop cuBLAS 12 (771 MB), ctranslate2 (65 MB), onnxruntime (55 MB,
   needed only by faster-whisper's VAD, which the Studio does not use) and faster_whisper.
   The weights stay the same size (fp16 safetensors, about 1.6 GB, replacing the CTranslate2
   folder). Needs: `transcribe.py` rewritten, `whisper` added to `TRANSFORMERS_KEEP`, and the
   transcription numbers in docs/benchmarks/alignment.md measured again. Result: about
   8.13 GB (7.57 GiB) on exFAT with the cache (0.91 GB less), under 9 GB in either unit.
2. **Bound the compute cache**: `env.py` could set `CUDA_CACHE_MAXSIZE` (for example
   536870912, 512 MiB: one GPU needs 0.38 GB, measured) so the stick's cache cannot grow with
   every PC it visits. One line in the pipeline stream's file.
3. **Whisper weights only when needed**: transcription runs only when no lyrics are supplied,
   embedded or found online. If the app downloads Whisper on first transcription rather than
   with the other defaults, a stick whose owner always has lyrics stays 1.62 GB smaller.
4. **Compile only modules that are actually imported** (about -0.15 GB on exFAT): about half
   the remaining .pyc files belong to modules the Studio never imports (much of torch,
   sympy, scipy). A module without .pyc still works (compiled in memory at import; nothing
   is written because `dont_write_bytecode` is set first). Not applied: small gain, and it
   trades a measured import time for an estimate.
5. **Reproducible bytecode**: compile with a fixed `PYTHONHASHSEED` (requires leaving out
   `-I` for that one step, with a sanitised environment and `-P -s`), so two builds are
   byte-identical (now 3 benign differences).
6. **`env.py`: `os.path.abspath` instead of `Path.resolve()`** for the root, so a mapped
   network drive is not turned into a UNC path (section 8).

Not done here, on purpose:

- cuDNN's sub-libraries (`cudnn_adv64_9.dll` 107 MB: RNNs, multi-head attention) were not
  stripped: the `clean` / `permissive` presets' models (SCNet, HT-Demucs) may use cuDNN RNN
  kernels, and only the `full` preset was run end to end here.
- The relocation test used `subst`, not a physical exFAT USB stick (none attached). exFAT
  specifics are covered by the static checks (no links, valid names, MAX_PATH, unchecked-hash
  .pyc) and by the cluster arithmetic; a run from a real stick, including its read speed for
  the 4.3 GB runtime, is still to be measured.
- Only this PC (RTX 5070 Ti, driver 621.24) was tested. The "clean PC" conditions are
  emulated: PATH reduced to Windows folders, the VC runtime proven to load from the runtime,
  every DLL import resolved inside the runtime, Windows or the NVIDIA driver.

## 13. Independent verification (2026-11-17, 21:44-22:30)

A separate verifier re-ran the relocation from the end-to-end stream's stick
(`$JK_BUILD/e2e-stick`, runtime built 21:19). Evidence: `$JK_BUILD/verify-reloc/` (logs,
`results/`, the app run's report and screenshots).

- **Copy + other drive letter.** `Windows/` and `Models/` copied to a new folder
  (`verify-reloc/stick`, 8.12 GB, 29 s; `Models/manifest.json`, `Settings` and the songs left
  out, so every cache started cold), then `subst K:` on it.
- **Runtime check from `K:\`** (`--separate` and `--pipeline` on a 23.5 s CC BY-SA excerpt of
  "Is It Right?" by Lower Loveday, JamendoLyrics; `--sentinel-profile`): **28 passed, 0
  failed**, 214 s. torch rel. errors 7.8e-7 / 2.1e-7 / 1.3e-7; cuBLAS 12 SGEMM 2.2e-7;
  Whisper on CUDA fp16; separation sums back within 1.19e-7; 338 modules, 0 from outside the
  runtime / Windows / driver; 0 files in the sentinel profile; 0 changes in the runtime;
  cold compute cache on the stick 363.9 MB / 80 files.
- **`make` from `K:\`** with only Windows folders on PATH and no `--root` (the packaged-root
  detection): exit 0, 25.9 s, lyrics given, stems sum error 5.96e-8, `jukeoke-cli verify
  --strict` 0 errors / 0 warnings; word onsets vs the annotation: median 0.037 s, 28 of 29
  within 0.3 s.
- **The app's Studio probe from `K:\`** (`K:\Windows\Jukeoke.exe`, driven by
  `app/ui/scripts/real-app-e2e.mjs queue`, null device, master 0, Windows-only PATH):
  portable mode, root `K:\`; Studio available 1.9 s after launch, models present; the
  supervisor started `K:\Windows\Studio\python\python.exe -I -X utf8 -m jukeoke_studio serve
  --root K:\ --models-dir K:\Models --temp-dir K:\Settings\tmp\studio`; its 209 loaded modules:
  139 from `K:\`, 62 Windows, 7 NVIDIA driver, 1 Windows Defender AMSI provider; a job with
  transcription finished in 26.9 s and was mapped to its song id (`verify --strict` PASS).
- **A folder with a space and non-ASCII letters** (`...\verify-reloc\stick Zoë ü`, caches
  deleted first): `make` with Whisper transcription, exit 0, 56.8 s (cold compute cache),
  `verify --strict` PASS, compute cache written to the stick (99 MB). The four decoded stems
  are bit-identical in all three songs (CLI on `K:\`, app on `K:\`, CLI from the Unicode
  path).
- **Per-user folders**: `%APPDATA%\NVIDIA\ComputeCache` 1,589 files / 837,920,356 bytes before
  and after; `%LOCALAPPDATA%\NVIDIA`, `%USERPROFILE%\.cache`, `%LOCALAPPDATA%\Python`, `pip`,
  `CrashDumps`: 0 entries newer than the start marker; `%TEMP%`: only this agent's files and
  another stream's pytest folders. Positive control re-run: one CUDA add without the
  Studio's start-up wrote 5 files / 27,423,696 bytes into an empty profile's
  `AppData\Roaming\NVIDIA\ComputeCache`.
- **Sizes re-measured** (own exFAT calculator; the files plus per-directory entry clusters):
  runtime 4,280,623,964 B / 18,769 files / 2,341 folders = 4.281 GB, 4.794 GB on 32 KB
  clusters, 6.696 GB on 128 KB; models 3,819,956,673 B = 3.820 / 3.821 GB; app 14,857,888 B.
  Total 8.660 GB = 8.066 GiB on exFAT, 9.038 GB = 8.418 GiB with a 0.378 GB compute cache:
  section 10's arithmetic holds. Not in section 10: the WebView2 profile in
  `Settings/webview-windows` (10.5 MB after two launches here, 72.8 MB on the e2e stick).

Defects found and fixed:

1. `Windows/Studio/requirements.pylock.toml` held the build machine's absolute path
   (`C:\Users\<user>\...\jukeoke-build\tmp\studio-runtime.*\pylock.toml`, so also the
   user name) in uv's header comment, and the check did not see it because its byte scan
   covered `python/` only. Fixed: `uv export --no-header` in the build; the check now scans
   the files next to `python/` too (positive control: FAIL on the old file, PASS after the
   header was removed); the header was removed from the two staged copies
   (`dist-staging/stick1`, `e2e-stick`).
2. Added the stale-runtime warning (section 5). On 2026-11-17 22:10 it warns: another
   stream's verifier changed `align.py`, `pipeline.py`, `server.py` and
   `modelzoo/download.py` after the e2e stick's runtime was built, so `e2e-stick` and
   `stick1` must be rebuilt before they are shipped.

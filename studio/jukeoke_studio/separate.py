# SPDX-License-Identifier: GPL-3.0-or-later
"""Stem separation: in-memory float32 in, float32 stems out.

The public entry point is :class:`Separator`::

    sep = Separator(models_dir, device="cuda")
    result = sep.separate(mix, sr, preset="full")   # mix: float32 [channels, frames]
    result.stems["lead_vocal"], result.stems["backing_vocals"], ...

Design rules (WORK_ORDER section 5, docs/ARCHITECTURE.md section 5):

* Everything stays 32-bit float in memory. Nothing here writes or reads an integer
  audio file, so there is no 16-bit (or any) quantisation step; the song writer
  decides the storage format later (24-bit FLAC with stored gain).
* Models run in full fp32 by default, with TF32 *disabled*. ``precision="fp16"`` etc.
  exist only so the benchmark can measure the difference.
* The original audio is never resampled. If the source rate differs from the model's
  (44.1 kHz for every model here), only the model input and its estimate are
  resampled (soxr, VHQ) and residual stems are computed at the native rate.
* Presets are built so that the four stems sum back to the input (WORK_ORDER 4.1:
  "fuller" versus "cleaner" instrumental):

  ``full`` (default)
      ``V = vocal_model(mix)``; ``lead = karaoke_model(V)``; ``backing = V - lead``;
      ``drums = drum_model(mix - V)``; ``other = mix - V - drums``.
      Karaoke mix with backing vocals on: ``mix - lead``; with them off:
      ``mix - V``. Nothing of the band is ever removed; whatever vocal the models
      miss stays audible as faint residue.
  ``clean``
      ``I = instrumental_model(mix)`` replaces ``mix - V`` as the band:
      ``backing = V - lead`` as above, ``drums = drum_model(I)``, ``other = I - drums``,
      and ``lead_vocal = mix - I - backing`` takes everything else (the lead plus
      whatever the instrumental model discarded). Karaoke mix: ``I + backing`` (on) or
      ``I`` (off). Less residue, possibly less body.

Which model plays which role is chosen in :data:`PRESETS`; the benchmark in
``studio/bench/separation_bench.py`` is what those choices are based on
(``docs/benchmarks/separation.md``).
"""

from __future__ import annotations

import contextlib
import random
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import numpy as np
import torch
import torch.nn.functional as F

from .modelzoo import registry
from .modelzoo.download import ensure_model, sha256_of  # noqa: F401  (re-exported for callers)
from .modelzoo.registry import ModelSpec

STEMS = ("lead_vocal", "backing_vocals", "drums", "other")

#: Model choice per preset and role. See docs/benchmarks/separation.md for the numbers
#: (MUSDB18-HQ test and a ccMixter lead/backing set, measured on the RTX 5070 Ti):
#:
#: * vocals ``bs_roformer_anvuew_ft1`` (GPL-3.0): 0.23 dB below the best downloadable
#:   vocal model (unwa Leap Xe, no licence) and the best one whose licence allows
#:   commercial use.
#: * karaoke ``bs_karaoke_anvuew`` (GPL-3.0) at overlap 8: at the shipped overlaps 0.55 dB
#:   ahead of the best unlicensed model (frazer & becruily) on lead SDR in mixes; that one
#:   leaves about 1 dB less lead residue in the karaoke mix and keeps backing vocals better.
#: * drums ``bs_roformer_4stems_syh`` (Apache-2.0): +1.1 dB over HT-Demucs; SCNet XL IHF
#:   is 0.8 dB better still but its weights are trained on non-commercial MUSDB18-HQ.
#: * instrumental (clean preset only) ``bs_inst_resurrection_unwa`` (no licence): the
#:   best direct-instrumental model measured, but still below "full" on SDR and residue.
#:
#: Every model in "full" allows commercial use; "clean" needs one with no published
#: licence (see MODELS.md).
PRESETS: dict[str, dict[str, str]] = {
    "full": {
        "vocals": "bs_roformer_anvuew_ft1",
        "karaoke": "bs_karaoke_anvuew",
        "drums": "bs_roformer_4stems_syh",
    },
    "clean": {
        "instrumental": "bs_inst_resurrection_unwa",
        "vocals": "bs_roformer_anvuew_ft1",
        "karaoke": "bs_karaoke_anvuew",
        "drums": "bs_roformer_4stems_syh",
    },
}

#: Same structure, restricted to models whose licence allows commercial use
#: (MODELS.md "commercial: yes"). No direct-instrumental model with such a licence comes
#: close in quality, so there is no permissive "clean" preset.
PERMISSIVE_PRESETS: dict[str, dict[str, str]] = {
    "full": dict(PRESETS["full"]),
}

#: Highest measured quality per role regardless of licence (all "unclear": no licence
#: published). For karaoke it is a trade rather than a clear win: frazer & becruily leaves
#: about 1 dB less lead in the karaoke mix and keeps backing vocals better, but scores
#: 0.55 dB lower lead SDR than the default at the shipped overlaps. Usable as
#: ``Separator.separate(..., models=BEST_MEASURED)`` for a comparison by ear; never a
#: default.
BEST_MEASURED: dict[str, str] = {
    "vocals": "bs_roformer_leap_xe_vocals",
    "karaoke": "bs_karaoke_frazer_becruily",
    "drums": "scnet_xl_ihf_zfturbo",
    "instrumental": "bs_inst_resurrection_unwa",
}

PRECISIONS = ("fp32", "tf32", "fp16", "bf16")


# ------------------------------------------------------------------------- resampling

def resample(audio: np.ndarray, sr_in: int, sr_out: int) -> np.ndarray:
    """High-quality resample of ``[channels, frames]`` float32 audio (soxr VHQ, float64 inside)."""
    if sr_in == sr_out:
        return audio
    import soxr

    out = soxr.resample(audio.T.astype(np.float64), sr_in, sr_out, quality="VHQ")
    return np.ascontiguousarray(out.T, dtype=np.float32)


def _fit_length(audio: np.ndarray, frames: int) -> np.ndarray:
    if audio.shape[-1] == frames:
        return audio
    if audio.shape[-1] > frames:
        return audio[..., :frames]
    return np.pad(audio, ((0, 0), (0, frames - audio.shape[-1])))


# -------------------------------------------------------------------------- precision

@contextlib.contextmanager
def precision_mode(precision: str, device: torch.device):
    """fp32: strict IEEE float32 (TF32 off). tf32: TF32 matmul/conv. fp16/bf16: autocast."""
    if precision not in PRECISIONS:
        raise ValueError(f"precision must be one of {PRECISIONS}")
    saved = (torch.backends.cuda.matmul.allow_tf32, torch.backends.cudnn.allow_tf32,
             torch.get_float32_matmul_precision())
    tf32 = precision == "tf32"
    torch.backends.cuda.matmul.allow_tf32 = tf32
    torch.backends.cudnn.allow_tf32 = tf32
    torch.set_float32_matmul_precision("high" if tf32 else "highest")
    try:
        if precision in ("fp16", "bf16"):
            dtype = torch.float16 if precision == "fp16" else torch.bfloat16
            with torch.autocast(device_type=device.type, dtype=dtype):
                yield
        else:
            yield
    finally:
        torch.backends.cuda.matmul.allow_tf32, torch.backends.cudnn.allow_tf32 = saved[:2]
        torch.set_float32_matmul_precision(saved[2])


# ----------------------------------------------------------------- overlap-add demix

def _fade_window(size: int, fade: int, device) -> torch.Tensor:
    w = torch.ones(size, device=device)
    if fade > 0:
        w[:fade] = torch.linspace(0, 1, fade, device=device)
        w[-fade:] = torch.linspace(1, 0, fade, device=device)
    return w


def demix_overlap_add(net: Callable[[torch.Tensor], torch.Tensor], mix: torch.Tensor, chunk: int,
                      overlap: int, batch_size: int = 1, num_stems: int = 1) -> torch.Tensor:
    """Run ``net`` over ``mix`` ``[C, T]`` in overlapping chunks; returns ``[S, C, T]`` float32.

    Same scheme as ZFTurbo's MSST ``demix`` (MIT), so the scores are comparable with
    published ones: reflect-pad by ``chunk - step`` at both ends, chunks every
    ``chunk // overlap`` samples, linear fades of ``chunk // 10`` at chunk edges (none at
    the very start/end of the track), weighted average where chunks overlap.
    """
    channels, length = mix.shape
    step = max(1, chunk // max(1, overlap))
    border = chunk - step
    # Upstream also fades when chunks do not overlap (overlap=1); the fades then reach
    # zero weight on both sides of every chunk boundary and those samples come out as 0
    # (an audible click). Without overlap we use a rectangular window instead.
    fade = chunk // 10 if border > 0 else 0
    padded = length > 2 * border and border > 0
    if padded:
        mix = F.pad(mix[None], (border, border), mode="reflect")[0]
    total = mix.shape[-1]
    window = _fade_window(chunk, fade, mix.device)
    result = torch.zeros((num_stems, channels, total), dtype=torch.float32, device=mix.device)
    counter = torch.zeros(total, dtype=torch.float32, device=mix.device)
    starts = list(range(0, total, step))
    for b in range(0, len(starts), batch_size):
        batch_starts = starts[b:b + batch_size]
        parts = []
        for s in batch_starts:
            part = mix[:, s:s + chunk]
            n = part.shape[-1]
            if n < chunk:
                mode = "reflect" if n > chunk // 2 else "constant"
                part = F.pad(part[None], (0, chunk - n), mode=mode)[0]
            parts.append(part)
        out = net(torch.stack(parts)).float()
        if out.ndim == 3:  # [B, C, T] single-stem networks
            out = out[:, None]
        for j, s in enumerate(batch_starts):
            n = min(chunk, total - s)
            w = window
            first, last = s == 0, s + step >= total
            if fade and (first or last):
                w = window.clone()
                if first:
                    w[:fade] = 1
                if last:
                    w[-fade:] = 1
            result[..., s:s + n] += out[j, ..., :n] * w[:n]
            counter[s:s + n] += w[:n]
    est = result / counter
    if padded:
        est = est[..., border:-border]
    return torch.nan_to_num(est, nan=0.0)


# ------------------------------------------------------------------------ model loading

def _clean_state_dict(obj) -> dict:
    if isinstance(obj, dict):
        for key in ("state_dict", "model_state_dict", "model", "state"):
            if key in obj and isinstance(obj[key], dict):
                obj = obj[key]
                break
    out = {}
    for k, v in obj.items():
        for prefix in ("module.", "_orig_mod.", "model."):
            if k.startswith(prefix):
                k = k[len(prefix):]
        out[k] = v
    return out


def _load_checkpoint(path: Path) -> dict:
    """``torch.load`` with ``weights_only=True`` (no arbitrary code from a pickle).

    Some community checkpoints are whole training snapshots that also pickle an
    optimiser / LR-scheduler object; those torch classes are allow-listed (their state
    is plain numbers) and their contents are discarded by :func:`_clean_state_dict`.
    """
    allow = [torch.optim.lr_scheduler.ReduceLROnPlateau, torch.optim.Adam, torch.optim.AdamW]
    with torch.serialization.safe_globals(allow):
        return torch.load(path, map_location="cpu", weights_only=True)


_ROTARY_KEYS = ("rotary_embed.freqs", "rotary_embed.cached_freqs", "rotary_embed.dummy",
                "rotary_embed.scale", "rotary_embed.cached_scales")


@dataclass
class LoadedModel:
    spec: ModelSpec
    net: torch.nn.Module
    device: torch.device
    sha256: tuple[str, ...]

    def __call__(self, audio: torch.Tensor, overlap: int | None, batch_size: int) -> dict[str, torch.Tensor]:
        """``audio`` ``[2, T]`` float32 on device at the model rate -> {output name: [2, T]}."""
        spec = self.spec
        if spec.arch == "htdemucs":
            return self._demucs(audio)
        est = demix_overlap_add(self.net, audio, spec.chunk_size, overlap or spec.overlap,
                                batch_size, num_stems=len(spec.outputs))
        return dict(zip(spec.outputs, est))

    def _demucs(self, audio: torch.Tensor) -> dict[str, torch.Tensor]:
        from demucs.apply import apply_model

        # Same normalisation as demucs.api.Separator.separate_tensor.
        ref = audio.mean(0)
        mean, std = ref.mean(), ref.std() + 1e-8
        state = random.getstate()
        random.seed(0)  # demucs "shifts" uses a random offset: make runs repeatable
        try:
            out = apply_model(self.net, ((audio - mean) / std)[None], shifts=self.spec.demucs_shifts,
                              split=True, overlap=self.spec.demucs_overlap, progress=False,
                              device=self.device)
        finally:
            random.setstate(state)
        out = (out * std + mean)[0].float()
        return dict(zip(self.net.sources, out))


def load_model(spec: ModelSpec, models_dir: Path | None, device: torch.device) -> LoadedModel:
    paths = ensure_model(spec, models_dir)
    if spec.arch == "htdemucs":
        from demucs.states import load_model as demucs_load

        # The .th package is a pickle; it is only loaded after its SHA-256 matched the
        # official file published by Meta (see registry).
        net = demucs_load(str(paths[0]))
    else:
        from .modelzoo.roformer import BSRoformer, MelBandRoformer
        from .modelzoo.scnet import SCNet

        klass = {"bs_roformer": BSRoformer, "mel_band_roformer": MelBandRoformer, "scnet": SCNet}[spec.arch]
        net = klass(**spec.model_kwargs)
        state = _clean_state_dict(_load_checkpoint(paths[0]))
        missing, unexpected = net.load_state_dict(state, strict=False)
        bad = [k for k in list(missing) + list(unexpected) if not k.endswith(_ROTARY_KEYS)]
        if bad:
            raise RuntimeError(f"{spec.id}: checkpoint does not match the architecture: {bad[:8]}")
    net.eval().to(device)
    for p in net.parameters():
        p.requires_grad_(False)
    return LoadedModel(spec, net, device, tuple(f.sha256 for f in spec.files))


# ---------------------------------------------------------------------------- results

@dataclass
class ModelRun:
    model: str
    seconds: float
    input_seconds: float
    resampled: bool
    sample_rate: int | None = None  # rate of the audio handed to run_model
    model_rate: int | None = None   # rate the network ran at (differs => resampled)
    overlap: int | None = None      # chunk overlap actually used (None for HT-Demucs)


@dataclass
class SeparationResult:
    stems: dict[str, np.ndarray]  # float32 [channels, frames] at the input rate
    sample_rate: int
    preset: str
    models: dict[str, str]  # role -> model id
    runs: list[ModelRun] = field(default_factory=list)
    extras: dict[str, np.ndarray] = field(default_factory=dict)  # e.g. "vocals", "instrumental"
    precision: str = "fp32"

    def provenance(self) -> dict:
        """What made these stems, shaped like docs/SONG_FORMAT.md section 4.3.

        Returns ``{"preset", "models": [...], "resampling": [...]}``; the pipeline merges
        these keys into the song file's ``provenance`` block (which also holds ``app``,
        ``studio``, ``source`` and ``created_utc``). ``models`` has one entry per weight
        file of every model that ran (``task``, ``name``, ``file``, ``sha256``,
        ``licence``, ``source_url`` as in the spec, plus ``commercial``, ``overlap`` and
        ``precision``); ``resampling`` lists every model input/estimate that was
        resampled (empty for a 44.1 kHz source). Only models that actually ran are listed.
        """
        overlap_of = {r.model: r.overlap for r in self.runs}
        out: dict = {"preset": self.preset, "models": [], "resampling": []}
        for role, mid in self.models.items():
            spec = registry.get(mid)
            for f in spec.files:
                out["models"].append({
                    "task": role, "name": mid, "title": spec.title, "file": f.filename,
                    "sha256": f.sha256, "licence": spec.licence, "commercial": spec.commercial,
                    "source_url": f.url, "overlap": overlap_of.get(mid), "precision": self.precision})
        role_of = {mid: role for role, mid in self.models.items()}
        for r in self.runs:
            if r.resampled:
                out["resampling"].append({
                    "what": f"input of model {r.model} ({role_of.get(r.model, '?')}) and its estimate",
                    "from_hz": r.sample_rate, "to_hz": r.model_rate, "method": "soxr VHQ"})
        return out


# -------------------------------------------------------------------------- Separator

class Separator:
    """Loads models on demand and runs the separation presets on float32 arrays.

    Args:
        models_dir: where weights live / are downloaded to (default: see
            :func:`jukeoke_studio.modelzoo.download.default_models_dir`).
        device: "cuda", "cuda:1", "cpu" or None (CUDA if available).
        precision: "fp32" (default, the only setting used for real songs), or "tf32",
            "fp16", "bf16" for measurements.
        overlap: override every RoFormer/SCNet model's chunk overlap. None (the default,
            what songs use) takes each model's own ``ModelSpec.overlap``, chosen per model
            from the overlap sweep in docs/benchmarks/separation.md section 6.2.
        batch_size: chunks per network call (speed/VRAM only).
        keep_loaded: keep models on the GPU between calls (fast for a queue of songs; see
            the benchmark's timing section for the VRAM this takes).
    """

    def __init__(self, models_dir: Path | str | None = None, device: str | None = None,
                 precision: str = "fp32", overlap: int | None = None, batch_size: int = 1,
                 keep_loaded: bool = True):
        self.models_dir = Path(models_dir) if models_dir else None
        self.device = torch.device(device or ("cuda" if torch.cuda.is_available() else "cpu"))
        if precision not in PRECISIONS:
            raise ValueError(f"precision must be one of {PRECISIONS}")
        self.precision = precision
        self.overlap = overlap
        self.batch_size = batch_size
        self.keep_loaded = keep_loaded
        self._loaded: dict[str, LoadedModel] = {}

    # -- model management
    def model(self, model_id: str) -> LoadedModel:
        if model_id not in self._loaded:
            if not self.keep_loaded:
                self.unload()
            self._loaded[model_id] = load_model(registry.get(model_id), self.models_dir, self.device)
        return self._loaded[model_id]

    def unload(self) -> None:
        self._loaded.clear()
        if self.device.type == "cuda":
            torch.cuda.empty_cache()

    # -- one model on an array
    def run_model(self, model_id: str, audio: np.ndarray, sr: int,
                  runs: list[ModelRun] | None = None) -> dict[str, np.ndarray]:
        """Apply one model to float32 ``[channels, frames]`` audio at ``sr``.

        Returns every output of the model as float32 at ``sr`` with the input's length and
        channel count. Mono is processed as dual-mono and averaged back; more than two
        channels are processed as consecutive stereo pairs.
        """
        audio = np.asarray(audio, dtype=np.float32)
        if audio.ndim != 2:
            raise ValueError("audio must be [channels, frames]")
        ch, frames = audio.shape
        if ch == 1:
            out = self.run_model(model_id, np.repeat(audio, 2, axis=0), sr, runs)
            return {k: v.mean(axis=0, keepdims=True) for k, v in out.items()}
        if ch > 2:
            pairs = [audio[i:i + 2] if i + 1 < ch else np.repeat(audio[i:i + 1], 2, axis=0)
                     for i in range(0, ch, 2)]
            outs = [self.run_model(model_id, p, sr, runs) for p in pairs]
            return {k: np.concatenate([o[k] for o in outs], axis=0)[:ch] for k in outs[0]}

        lm = self.model(model_id)
        msr = lm.spec.sample_rate
        x = resample(audio, sr, msr)
        t0 = time.perf_counter()
        with torch.inference_mode(), precision_mode(self.precision, self.device):
            est = lm(torch.from_numpy(x).to(self.device), self.overlap, self.batch_size)
            est = {k: v.float().cpu().numpy() for k, v in est.items()}
        if self.device.type == "cuda":
            torch.cuda.synchronize(self.device)
        dt = time.perf_counter() - t0
        if runs is not None:
            used_overlap = None if lm.spec.arch == "htdemucs" else (self.overlap or lm.spec.overlap)
            runs.append(ModelRun(model_id, dt, frames / sr, sr != msr, sr, msr, used_overlap))
        return {k: _fit_length(resample(v, msr, sr), frames).astype(np.float32, copy=False)
                for k, v in est.items()}

    # -- presets
    def separate(self, mix: np.ndarray, sr: int, preset: str = "full",
                 models: dict[str, str] | None = None) -> SeparationResult:
        """Split ``mix`` (float32 ``[channels, frames]``) into lead / backing / drums / other.

        ``models`` overrides the preset's model per role (keys as in :data:`PRESETS`).
        Roles the preset does not use (e.g. ``instrumental`` from :data:`BEST_MEASURED`
        with ``full``) are ignored, so the provenance lists only models that ran; a role
        no preset knows (a typo) is an error.
        All four stems sum back to ``mix`` (to float32 rounding); see the module docstring.
        """
        if preset not in PRESETS:
            raise ValueError(f"unknown preset {preset!r}; known: {', '.join(PRESETS)}")
        known_roles = {role for roles in PRESETS.values() for role in roles}
        unknown = set(models or {}) - known_roles
        if unknown:
            raise ValueError(f"unknown model role(s) {sorted(unknown)}; known: {sorted(known_roles)}")
        choice = {role: (models or {}).get(role, mid) for role, mid in PRESETS[preset].items()}
        mix = np.ascontiguousarray(mix, dtype=np.float32)
        runs: list[ModelRun] = []
        mix64 = mix.astype(np.float64)

        vocals = self.run_model(choice["vocals"], mix, sr, runs)["vocals"]
        lead = lead_estimate = self.run_model(choice["karaoke"], vocals, sr, runs)["lead"]
        backing = (vocals.astype(np.float64) - lead).astype(np.float32)
        if preset == "full":
            band = (mix64 - vocals).astype(np.float32)
        else:
            band = self.run_model(choice["instrumental"], mix, sr, runs)["instrumental"]
            # The lead stem absorbs everything that is neither band nor backing, so the
            # four stems still add up to the mix.
            lead = (mix64 - band - backing).astype(np.float32)
        drums = self.run_model(choice["drums"], band, sr, runs)["drums"]
        # The residual stem takes what the estimates did not: computed in float64 from
        # the float32 stems so the float32 sum of the four stems equals the mix to within
        # float32 rounding.
        other = (mix64 - lead.astype(np.float64) - backing - drums).astype(np.float32)
        stems = {"lead_vocal": lead, "backing_vocals": backing, "drums": drums, "other": other}
        return SeparationResult(stems, sr, preset, choice, runs,
                                extras={"vocals": vocals, "instrumental": band,
                                        "lead_estimate": lead_estimate},
                                precision=self.precision)

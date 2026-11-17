# SPDX-License-Identifier: MIT
#
# BS-RoFormer and Mel-Band RoFormer, inference-only.
#
# Derived from:
#   - lucidrains/BS-RoFormer (https://github.com/lucidrains/BS-RoFormer)
#     MIT License, Copyright (c) 2023 Phil Wang
#   - ZFTurbo/Music-Source-Separation-Training, models/bs_roformer/{bs_roformer,
#     mel_band_roformer,attend}.py (https://github.com/ZFTurbo/Music-Source-Separation-Training)
#     MIT License, Copyright (c) 2024 Roman Solovyev (ZFTurbo)
#
# Permission is hereby granted, free of charge, to any person obtaining a copy of this
# software and associated documentation files (the "Software"), to deal in the Software
# without restriction, including without limitation the rights to use, copy, modify,
# merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
# permit persons to whom the Software is furnished to do so, subject to the following
# conditions: The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
# WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.
#
# Changes for Jukeoke (2026): training code (losses, gradient checkpointing, PoPE,
# beartype) removed; module and parameter names are unchanged so community checkpoints
# trained with MSST load with strict=True; the librosa mel filter bank is replaced by an
# equivalent NumPy implementation (only its non-zero pattern is used); attention always
# goes through torch.nn.functional.scaled_dot_product_attention.
"""Band-Split RoFormer and Mel-Band RoFormer source-separation networks.

Both take a waveform batch ``[batch, channels, samples]`` (float32, 44.1 kHz for every
checkpoint we use) and return ``[batch, stems, channels, samples]`` (BS-RoFormer) or
``[batch, channels, samples]`` when ``num_stems == 1`` (Mel-Band RoFormer, kept as in
upstream; :func:`jukeoke_studio.separate` normalises the shape).
"""

from __future__ import annotations

from functools import partial

import numpy as np
import torch
import torch.nn.functional as F
from einops import pack, rearrange, reduce, repeat, unpack
from rotary_embedding_torch import RotaryEmbedding
from torch import nn
from torch.nn import Module, ModuleList


def _pack_one(t, pattern):
    return pack([t], pattern)


def _unpack_one(t, ps, pattern):
    return unpack(t, ps, pattern)[0]


class RMSNorm(Module):
    def __init__(self, dim: int):
        super().__init__()
        self.scale = dim**0.5
        self.gamma = nn.Parameter(torch.ones(dim))

    def forward(self, x):
        return F.normalize(x, dim=-1) * self.scale * self.gamma


class FeedForward(Module):
    def __init__(self, dim: int, mult: int = 4, dropout: float = 0.0):
        super().__init__()
        dim_inner = int(dim * mult)
        # Indices 0..5 of this Sequential are part of the checkpoint key names.
        self.net = nn.Sequential(
            RMSNorm(dim),
            nn.Linear(dim, dim_inner),
            nn.GELU(),
            nn.Dropout(dropout),
            nn.Linear(dim_inner, dim),
            nn.Dropout(dropout),
        )

    def forward(self, x):
        return self.net(x)


class Attention(Module):
    """Multi-head self-attention with rotary positions and per-head sigmoid gates."""

    def __init__(self, dim: int, heads: int = 8, dim_head: int = 64, rotary_embed=None, **_):
        super().__init__()
        self.heads = heads
        dim_inner = heads * dim_head
        self.rotary_embed = rotary_embed
        self.norm = RMSNorm(dim)
        self.to_qkv = nn.Linear(dim, dim_inner * 3, bias=False)
        self.to_gates = nn.Linear(dim, heads)
        self.to_out = nn.Sequential(nn.Linear(dim_inner, dim, bias=False), nn.Dropout(0.0))

    def forward(self, x):
        x = self.norm(x)
        q, k, v = rearrange(self.to_qkv(x), "b n (qkv h d) -> qkv b h n d", qkv=3, h=self.heads)
        if self.rotary_embed is not None:
            q = self.rotary_embed.rotate_queries_or_keys(q)
            k = self.rotary_embed.rotate_queries_or_keys(k)
        out = F.scaled_dot_product_attention(q, k, v)
        gates = self.to_gates(x)
        out = out * rearrange(gates, "b n h -> b h n 1").sigmoid()
        out = rearrange(out, "b h n d -> b n (h d)")
        return self.to_out(out)


class LinearAttention(Module):
    """Cross-covariance linear attention (El-Nouby et al. 2021); used by a few checkpoints."""

    def __init__(self, *, dim: int, dim_head: int = 32, heads: int = 8, scale: float = 8, **_):
        super().__init__()
        dim_inner = dim_head * heads
        self.scale = scale
        self.norm = RMSNorm(dim)
        self.to_qkv = nn.Sequential(
            nn.Linear(dim, dim_inner * 3, bias=False),
            _Rearrange("b n (qkv h d) -> qkv b h d n", qkv=3, h=heads),
        )
        self.temperature = nn.Parameter(torch.ones(heads, 1, 1))
        self.to_out = nn.Sequential(
            _Rearrange("b h d n -> b n (h d)"), nn.Linear(dim_inner, dim, bias=False)
        )

    def forward(self, x):
        x = self.norm(x)
        q, k, v = self.to_qkv(x)
        q, k = F.normalize(q, dim=-1), F.normalize(k, dim=-1)
        q = q * self.temperature.exp()
        # Upstream Attend(scale=8): softmax(q k^T * scale) v
        default_scale = q.shape[-1] ** -0.5
        out = F.scaled_dot_product_attention(q * (self.scale / default_scale), k, v)
        return self.to_out(out)


class _Rearrange(Module):
    def __init__(self, pattern: str, **axes):
        super().__init__()
        self.pattern, self.axes = pattern, axes

    def forward(self, x):
        return rearrange(x, self.pattern, **self.axes)


class Transformer(Module):
    def __init__(
        self,
        *,
        dim: int,
        depth: int,
        dim_head: int = 64,
        heads: int = 8,
        ff_mult: int = 4,
        norm_output: bool = True,
        rotary_embed=None,
        linear_attn: bool = False,
        **_,
    ):
        super().__init__()
        self.layers = ModuleList([])
        for _i in range(depth):
            if linear_attn:
                attn = LinearAttention(dim=dim, dim_head=dim_head, heads=heads)
            else:
                attn = Attention(dim=dim, dim_head=dim_head, heads=heads, rotary_embed=rotary_embed)
            self.layers.append(ModuleList([attn, FeedForward(dim=dim, mult=ff_mult)]))
        self.norm = RMSNorm(dim) if norm_output else nn.Identity()

    def forward(self, x):
        for attn, ff in self.layers:
            x = attn(x) + x
            x = ff(x) + x
        return self.norm(x)


class BandSplit(Module):
    def __init__(self, dim: int, dim_inputs: tuple[int, ...]):
        super().__init__()
        self.dim_inputs = dim_inputs
        self.to_features = ModuleList(
            [nn.Sequential(RMSNorm(d), nn.Linear(d, dim)) for d in dim_inputs]
        )

    def forward(self, x):
        x = x.split(self.dim_inputs, dim=-1)
        return torch.stack([f(s) for s, f in zip(x, self.to_features)], dim=-2)


def _mlp(dim_in: int, dim_out: int, dim_hidden: int, n_hidden: int) -> nn.Sequential:
    """Linear/Tanh stack with ``n_hidden`` hidden layers (indices match upstream)."""
    dims = (dim_in, *((dim_hidden,) * n_hidden), dim_out)
    net: list[nn.Module] = []
    for ind, (a, b) in enumerate(zip(dims[:-1], dims[1:])):
        net.append(nn.Linear(a, b))
        if ind != len(dims) - 2:
            net.append(nn.Tanh())
    return nn.Sequential(*net)


class MaskEstimator(Module):
    def __init__(self, dim: int, dim_inputs: tuple[int, ...], n_hidden: int, mlp_expansion_factor: int = 4):
        super().__init__()
        self.dim_inputs = dim_inputs
        hidden = dim * mlp_expansion_factor
        self.to_freqs = ModuleList(
            [nn.Sequential(_mlp(dim, d * 2, hidden, n_hidden), nn.GLU(dim=-1)) for d in dim_inputs]
        )

    def forward(self, x):
        x = x.unbind(dim=-2)
        return torch.cat([mlp(b) for b, mlp in zip(x, self.to_freqs)], dim=-1)


DEFAULT_FREQS_PER_BANDS = (
    (2,) * 24 + (4,) * 12 + (12,) * 8 + (24,) * 8 + (48,) * 8 + (128, 129)
)


class _RoformerBase(Module):
    """Shared transformer stack (time and frequency axial attention)."""

    def _build_layers(self, dim, depth, heads, dim_head, time_depth, freq_depth, linear_depth, norm_output):
        kw = dict(dim=dim, heads=heads, dim_head=dim_head, norm_output=norm_output)
        time_rot = RotaryEmbedding(dim=dim_head)
        freq_rot = RotaryEmbedding(dim=dim_head)
        self.layers = ModuleList([])
        for _ in range(depth):
            mods = []
            if linear_depth > 0:
                mods.append(Transformer(depth=linear_depth, linear_attn=True, **kw))
            mods.append(Transformer(depth=time_depth, rotary_embed=time_rot, **kw))
            mods.append(Transformer(depth=freq_depth, rotary_embed=freq_rot, **kw))
            self.layers.append(ModuleList(mods))

    def _run_layers(self, x):
        store = [None] * len(self.layers)
        for i, block in enumerate(self.layers):
            if len(block) == 3:
                linear_t, time_t, freq_t = block
                x, ft_ps = pack([x], "b * d")
                x = linear_t(x)
                (x,) = unpack(x, ft_ps, "b * d")
            else:
                time_t, freq_t = block
            if self.skip_connection:
                for j in range(i):
                    x = x + store[j]
            x = rearrange(x, "b t f d -> b f t d")
            x, ps = pack([x], "* t d")
            x = time_t(x)
            (x,) = unpack(x, ps, "* t d")
            x = rearrange(x, "b f t d -> b t f d")
            x, ps = pack([x], "* f d")
            x = freq_t(x)
            (x,) = unpack(x, ps, "* f d")
            if self.skip_connection:
                store[i] = x
        return x


class BSRoformer(_RoformerBase):
    """Band-Split RoFormer (Lu et al., 2023), fixed linear band layout."""

    def __init__(
        self,
        dim,
        *,
        depth,
        stereo=False,
        num_stems=1,
        time_transformer_depth=2,
        freq_transformer_depth=2,
        linear_transformer_depth=0,
        freqs_per_bands=DEFAULT_FREQS_PER_BANDS,
        dim_head=64,
        heads=8,
        stft_n_fft=2048,
        stft_hop_length=512,
        stft_win_length=2048,
        stft_normalized=False,
        zero_dc=True,
        mask_estimator_depth=2,
        mlp_expansion_factor=4,
        skip_connection=False,
        **_ignored,  # training-only keys from the YAML (dropouts, losses, flash_attn, ...)
    ):
        super().__init__()
        self.stereo = stereo
        self.audio_channels = 2 if stereo else 1
        self.num_stems = num_stems
        self.skip_connection = skip_connection
        self._build_layers(dim, depth, heads, dim_head, time_transformer_depth,
                           freq_transformer_depth, linear_transformer_depth, norm_output=False)
        self.final_norm = RMSNorm(dim)
        self.stft_kwargs = dict(n_fft=stft_n_fft, hop_length=stft_hop_length,
                                win_length=stft_win_length, normalized=stft_normalized)
        self.stft_window_fn = partial(torch.hann_window, stft_win_length)
        n_freqs = stft_n_fft // 2 + 1
        freqs_per_bands = tuple(freqs_per_bands)
        assert sum(freqs_per_bands) == n_freqs, f"bands must cover {n_freqs} bins"
        with_complex = tuple(2 * f * self.audio_channels for f in freqs_per_bands)
        self.band_split = BandSplit(dim=dim, dim_inputs=with_complex)
        # Upstream MLP(depth=mask_estimator_depth) has depth-1 hidden layers for BS-RoFormer.
        self.mask_estimators = ModuleList(
            [MaskEstimator(dim, with_complex, mask_estimator_depth - 1, mlp_expansion_factor)
             for _ in range(num_stems)]
        )
        self.zero_dc = zero_dc

    def forward(self, raw_audio):
        device = raw_audio.device
        if raw_audio.ndim == 2:
            raw_audio = rearrange(raw_audio, "b t -> b 1 t")
        raw_audio, packed = _pack_one(raw_audio, "* t")
        window = self.stft_window_fn(device=device)
        stft = torch.view_as_real(torch.stft(raw_audio, **self.stft_kwargs, window=window, return_complex=True))
        stft = _unpack_one(stft, packed, "* f t c")
        stft = rearrange(stft, "b s f t c -> b (f s) t c")
        x = rearrange(stft, "b f t c -> b t (f c)")
        x = self.band_split(x)
        x = self._run_layers(x)
        x = self.final_norm(x)
        mask = torch.stack([fn(x) for fn in self.mask_estimators], dim=1)
        mask = rearrange(mask, "b n t (f c) -> b n f t c", c=2)
        stft = rearrange(stft, "b f t c -> b 1 f t c")
        # .float(): under fp16/bf16 autocast (precision measurements only) the mask comes
        # out in half precision and bf16 has no complex type. In fp32 it is a no-op, so the
        # default path stays bit-identical to upstream.
        out = torch.view_as_complex(stft) * torch.view_as_complex(mask.float().contiguous())
        out = rearrange(out, "b n (f s) t -> (b n s) f t", s=self.audio_channels)
        if self.zero_dc:
            out[:, 0] = 0.0
        audio = torch.istft(out, **self.stft_kwargs, window=window, return_complex=False,
                            length=raw_audio.shape[-1])
        return rearrange(audio, "(b n s) t -> b n s t", s=self.audio_channels, n=self.num_stems)


def mel_filter_support(sr: int, n_fft: int, n_mels: int) -> np.ndarray:
    """Boolean ``[n_mels, n_fft//2+1]`` support of librosa.filters.mel (Slaney scale).

    Re-implements librosa 0.10 ``filters.mel(sr, n_fft, n_mels, fmin=0, fmax=sr/2,
    htk=False, norm='slaney')`` in float64 with the same operation order; only the
    sign of each weight matters to the Mel-Band RoFormer. The two corner entries are
    forced on, as upstream does.
    """
    fftfreqs = np.fft.rfftfreq(n=n_fft, d=1.0 / sr)

    def hz_to_mel(f):
        f = np.asanyarray(f, dtype=np.float64)
        f_sp = 200.0 / 3
        mels = f / f_sp
        min_log_hz = 1000.0
        min_log_mel = min_log_hz / f_sp
        logstep = np.log(6.4) / 27.0
        return np.where(f >= min_log_hz, min_log_mel + np.log(np.maximum(f, 1e-10) / min_log_hz) / logstep, mels)

    def mel_to_hz(m):
        m = np.asanyarray(m, dtype=np.float64)
        f_sp = 200.0 / 3
        freqs = f_sp * m
        min_log_hz = 1000.0
        min_log_mel = min_log_hz / f_sp
        logstep = np.log(6.4) / 27.0
        return np.where(m >= min_log_mel, min_log_hz * np.exp(logstep * (m - min_log_mel)), freqs)

    mel_f = mel_to_hz(np.linspace(hz_to_mel(0.0), hz_to_mel(sr / 2.0), n_mels + 2))
    fdiff = np.diff(mel_f)
    ramps = np.subtract.outer(mel_f, fftfreqs)
    weights = np.zeros((n_mels, len(fftfreqs)), dtype=np.float64)
    for i in range(n_mels):
        lower = -ramps[i] / fdiff[i]
        upper = ramps[i + 2] / fdiff[i + 1]
        weights[i] = np.maximum(0, np.minimum(lower, upper))
    enorm = 2.0 / (mel_f[2 : n_mels + 2] - mel_f[:n_mels])
    weights *= enorm[:, np.newaxis]
    weights = weights.astype(np.float32)  # librosa returns float32
    weights[0, 0] = 1.0
    weights[-1, -1] = 1.0
    return weights > 0


class MelBandRoformer(_RoformerBase):
    """Mel-Band RoFormer (Wang et al., 2023): overlapping mel-spaced bands."""

    def __init__(
        self,
        dim,
        *,
        depth,
        stereo=False,
        num_stems=1,
        time_transformer_depth=2,
        freq_transformer_depth=2,
        linear_transformer_depth=0,
        num_bands=60,
        dim_head=64,
        heads=8,
        sample_rate=44100,
        stft_n_fft=2048,
        stft_hop_length=512,
        stft_win_length=2048,
        stft_normalized=False,
        zero_dc=True,
        mask_estimator_depth=1,
        match_input_audio_length=False,
        mlp_expansion_factor=4,
        skip_connection=False,
        **_ignored,
    ):
        super().__init__()
        self.stereo = stereo
        self.audio_channels = 2 if stereo else 1
        self.num_stems = num_stems
        self.skip_connection = skip_connection
        self._build_layers(dim, depth, heads, dim_head, time_transformer_depth,
                           freq_transformer_depth, linear_transformer_depth, norm_output=True)
        self.stft_window_fn = partial(torch.hann_window, stft_win_length)
        self.stft_kwargs = dict(n_fft=stft_n_fft, hop_length=stft_hop_length,
                                win_length=stft_win_length, normalized=stft_normalized)
        n_freqs = stft_n_fft // 2 + 1
        freqs_per_band = torch.from_numpy(mel_filter_support(sample_rate, stft_n_fft, num_bands))
        assert freqs_per_band.any(dim=0).all(), "all frequencies must be covered"
        freq_indices = repeat(torch.arange(n_freqs), "f -> b f", b=num_bands)[freqs_per_band]
        if stereo:
            freq_indices = repeat(freq_indices, "f -> f s", s=2) * 2 + torch.arange(2)
            freq_indices = rearrange(freq_indices, "f s -> (f s)")
        self.register_buffer("freq_indices", freq_indices, persistent=False)
        self.register_buffer("freqs_per_band", freqs_per_band, persistent=False)
        num_freqs_per_band = reduce(freqs_per_band, "b f -> b", "sum")
        num_bands_per_freq = reduce(freqs_per_band, "b f -> f", "sum")
        self.register_buffer("num_freqs_per_band", num_freqs_per_band, persistent=False)
        self.register_buffer("num_bands_per_freq", num_bands_per_freq, persistent=False)
        with_complex = tuple(2 * f * self.audio_channels for f in num_freqs_per_band.tolist())
        self.band_split = BandSplit(dim=dim, dim_inputs=with_complex)
        # Upstream MLP(depth=mask_estimator_depth) has `depth` hidden layers for Mel-RoFormer.
        self.mask_estimators = ModuleList(
            [MaskEstimator(dim, with_complex, mask_estimator_depth, mlp_expansion_factor)
             for _ in range(num_stems)]
        )
        self.zero_dc = zero_dc
        self.match_input_audio_length = match_input_audio_length

    def forward(self, raw_audio):
        device = raw_audio.device
        if raw_audio.ndim == 2:
            raw_audio = rearrange(raw_audio, "b t -> b 1 t")
        batch, channels, length = raw_audio.shape
        raw_audio, packed = _pack_one(raw_audio, "* t")
        window = self.stft_window_fn(device=device)
        stft = torch.view_as_real(torch.stft(raw_audio, **self.stft_kwargs, window=window, return_complex=True))
        stft = _unpack_one(stft, packed, "* f t c")
        stft = rearrange(stft, "b s f t c -> b (f s) t c")
        batch_arange = torch.arange(batch, device=device)[..., None]
        x = stft[batch_arange, self.freq_indices]
        x = rearrange(x, "b f t c -> b t (f c)")
        x = self.band_split(x)
        x = self._run_layers(x)
        masks = torch.stack([fn(x) for fn in self.mask_estimators], dim=1)
        masks = rearrange(masks, "b n t (f c) -> b n f t c", c=2)
        stft_c = torch.view_as_complex(rearrange(stft, "b f t c -> b 1 f t c").contiguous())
        # .float(): see BSRoformer.forward (no-op in fp32; bf16 has no complex type).
        masks = torch.view_as_complex(masks.float().contiguous()).type(stft_c.dtype)
        n = self.num_stems
        idx = repeat(self.freq_indices, "f -> b n f t", b=batch, n=n, t=stft_c.shape[-1])
        summed = torch.zeros_like(repeat(stft_c, "b 1 ... -> b n ...", n=n)).scatter_add_(2, idx, masks)
        denom = repeat(self.num_bands_per_freq, "f -> (f r) 1", r=channels)
        out = stft_c * (summed / denom.clamp(min=1e-8))
        out = rearrange(out, "b n (f s) t -> (b n s) f t", s=self.audio_channels)
        if self.zero_dc:
            out[:, 0] = 0.0
        # Upstream passes length=None unless match_input_audio_length; every chunk size we
        # use is a multiple of the hop, where both give the same samples. Always passing the
        # length keeps the output aligned with the input for any chunk size.
        audio = torch.istft(out, **self.stft_kwargs, window=window, return_complex=False, length=length)
        audio = rearrange(audio, "(b n s) t -> b n s t", b=batch, s=self.audio_channels, n=n)
        return audio

# SPDX-License-Identifier: GPL-3.0-or-later
"""Network definitions and the overlap-add demixer (CPU only)."""

import numpy as np
import pytest
import torch

from jukeoke_studio.modelzoo.roformer import BSRoformer, MelBandRoformer, mel_filter_support
from jukeoke_studio.separate import demix_overlap_add


@pytest.mark.parametrize("n_fft,bands", [(2048, 60), (4096, 60), (2048, 64)])
def test_mel_support_matches_librosa(n_fft, bands):
    librosa = pytest.importorskip("librosa")
    ref = librosa.filters.mel(sr=44100, n_fft=n_fft, n_mels=bands)
    ref[0][0] = 1.0
    ref[-1, -1] = 1.0
    assert np.array_equal(ref > 0, mel_filter_support(44100, n_fft, bands))


def _tiny(cls, **kw):
    base = dict(dim=16, depth=1, stereo=True, num_stems=2, time_transformer_depth=1,
                freq_transformer_depth=1, dim_head=8, heads=2, stft_n_fft=256,
                stft_hop_length=64, stft_win_length=256, mask_estimator_depth=2)
    if cls is BSRoformer:
        base["freqs_per_bands"] = (8,) * 16 + (1,)
    else:
        base.update(num_bands=8, sample_rate=44100)
    base.update(kw)
    return cls(**base).eval()


@pytest.mark.parametrize("cls", [BSRoformer, MelBandRoformer])
def test_forward_shape(cls):
    net = _tiny(cls)
    with torch.inference_mode():
        y = net(torch.randn(3, 2, 4096))
    assert y.shape == (3, 2, 2, 4096)


def test_state_dict_names_match_msst_layout():
    # Community checkpoints are loaded with these exact names (verified against real
    # weights: zero missing / unexpected keys for every model in the registry).
    keys = set(_tiny(BSRoformer).state_dict())
    for k in ("layers.0.0.layers.0.0.to_qkv.weight", "layers.0.0.layers.0.0.to_gates.bias",
              "layers.0.0.layers.0.1.net.1.weight", "layers.0.1.layers.0.0.norm.gamma",
              "band_split.to_features.0.0.gamma", "band_split.to_features.0.1.weight",
              "mask_estimators.1.to_freqs.0.0.0.weight", "final_norm.gamma"):
        assert k in keys, k
    mel = set(_tiny(MelBandRoformer).state_dict())
    assert "layers.0.0.norm.gamma" in mel  # Mel-RoFormer keeps per-transformer output norms
    assert "mask_estimators.0.to_freqs.0.0.2.weight" in mel  # depth-2 MLP: 2 hidden layers


@pytest.mark.parametrize("cls", [BSRoformer, MelBandRoformer])
def test_forward_under_bf16_autocast(cls):
    # The precision benchmark runs the networks under bf16 autocast; bf16 has no complex
    # dtype, so the mask is cast to float32 before view_as_complex. The fp32 path is
    # unchanged by that cast (bit-identical outputs are checked against upstream on GPU).
    from jukeoke_studio.separate import precision_mode

    net = _tiny(cls)
    x = torch.randn(1, 2, 4096)
    with torch.inference_mode():
        ref = net(x)
        with precision_mode("bf16", torch.device("cpu")):
            y = net(x)
    assert y.dtype == torch.float32 and y.shape == ref.shape
    assert torch.isfinite(y).all()


class _Identity(torch.nn.Module):
    def forward(self, x):  # [B, C, T] -> [B, 1, C, T]
        return x[:, None]


@pytest.mark.parametrize("length", [1000, 44100, 88200, 100_000, 352_801])
@pytest.mark.parametrize("overlap", [1, 2, 4])
def test_demix_identity_is_transparent(length, overlap):
    x = torch.randn(2, length)
    y = demix_overlap_add(_Identity(), x, chunk=44100, overlap=overlap, batch_size=3)
    assert y.shape == (1, 2, length)
    assert torch.max(torch.abs(y[0] - x)).item() < 1e-6

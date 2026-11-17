# SPDX-License-Identifier: GPL-3.0-or-later
"""Catalogue of separation models the Studio knows how to download and run.

Every entry pins the exact file (a Hugging Face commit or an immutable release asset)
and its SHA-256, so a download is either byte-identical to what was benchmarked or
rejected. Licences are recorded per model because they differ wildly (WORK_ORDER
section 10, trap 10); ``MODELS.md`` at the repo root is the human-readable version
of this table and must be kept in sync with it (``tests/test_registry.py`` checks).

Model hyper-parameters are copied from each model's upstream YAML (training-only keys
dropped) so that no YAML has to be fetched or parsed at run time.

``commercial`` is our reading of the licence, for the owner (who may earn money on
TikTok): "yes" (permissive or GPL: commercial use allowed), "no" (non-commercial
licence) or "unclear" (no licence published: all rights reserved by default, ask the
author before commercial use).
"""

from __future__ import annotations

from dataclasses import dataclass, field

HF = "https://huggingface.co"


def hf(repo: str, rev: str, path: str) -> str:
    """Revision-pinned Hugging Face download URL."""
    return f"{HF}/{repo}/resolve/{rev}/{path}"


@dataclass(frozen=True)
class WeightFile:
    url: str
    filename: str  # name inside the models directory
    sha256: str
    size: int


@dataclass(frozen=True)
class ModelSpec:
    id: str
    title: str
    arch: str  # "bs_roformer" | "mel_band_roformer" | "htdemucs" | "scnet"
    role: str  # "vocals" | "karaoke" | "instrumental" | "multistem"
    files: tuple[WeightFile, ...]
    outputs: tuple[str, ...]  # names of the network's output stems, in order
    licence: str
    commercial: str  # "yes" | "no" | "unclear"
    source: str  # human page for the model
    author: str
    notes: str = ""
    model_kwargs: dict = field(default_factory=dict)
    chunk_size: int = 0  # samples at 44.1 kHz per network call (roformers)
    # Chunks overlapping each sample (RoFormer / SCNet); demucs: see demucs_overlap. Chosen
    # from the overlap sweep (docs/benchmarks/separation.md section 6.2): the next step up
    # is taken only if it raised SDR by at least 0.1 dB on average over the same tracks.
    # Vocal, instrumental and drum models gained 0.02-0.08 dB from overlap 2 to 8 (kept at
    # 2); the default karaoke model gained 0.53 dB at 4 and 0.72 dB at 8 on 10 tracks
    # (+0.36 dB a cappella, +0.41 dB in mixes over the whole ccMixter set), so karaoke
    # models use 8 (frazer & becruily: 4, measured separately).
    overlap: int = 2
    sample_rate: int = 44100
    demucs_overlap: float = 0.25
    demucs_shifts: int = 1


# Shared hyper-parameter blocks --------------------------------------------------------

_MEL_384_6 = dict(  # Kim's Mel-Band RoFormer layout, used by most fine-tunes
    dim=384, depth=6, stereo=True, num_stems=1, time_transformer_depth=1,
    freq_transformer_depth=1, num_bands=60, dim_head=64, heads=8, sample_rate=44100,
    stft_n_fft=2048, stft_hop_length=441, stft_win_length=2048, stft_normalized=False,
    mask_estimator_depth=2,
)
_BS_256_12_HOP512 = dict(  # anvuew-style 256-dim 12-layer BS-RoFormer
    dim=256, depth=12, stereo=True, num_stems=1, time_transformer_depth=1,
    freq_transformer_depth=1, linear_transformer_depth=0, dim_head=64, heads=8,
    stft_n_fft=2048, stft_hop_length=512, stft_win_length=2048, stft_normalized=False,
    mask_estimator_depth=2, mlp_expansion_factor=4, skip_connection=False,
)

_LEAP_BANDS = (2,) * 24 + (4,) * 36 + (12,) * 16 + (24,) * 8 + (48,) * 4 + (128, 129)

# The demucs htdemucs_ft "bag": one fine-tuned HT-Demucs per stem; each sub-model is
# only good at its own stem, so we load just the one we need.
_DEMUCS = "https://dl.fbaipublicfiles.com/demucs/hybrid_transformer"

MODELS: dict[str, ModelSpec] = {}


def _add(spec: ModelSpec) -> None:
    assert spec.id not in MODELS
    MODELS[spec.id] = spec


# ---------------------------------------------------------------- vocals (mix -> vocals)

_add(ModelSpec(
    id="bs_roformer_viperx_1297",
    title="BS-RoFormer (viperx, ep 317, SDR 12.97)",
    arch="bs_roformer", role="vocals",
    files=(WeightFile(
        "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/"
        "model_bs_roformer_ep_317_sdr_12.9755.ckpt",
        "model_bs_roformer_ep_317_sdr_12.9755.ckpt",
        "5b84f37e8d444c8cb30c79d77f613a41c05868ff9c9ac6c7049c00aefae115aa", 639_331_213),),
    outputs=("vocals",),
    licence="none published (UVR model_repo release has no licence)", commercial="unclear",
    source="https://github.com/TRvlvr/model_repo/releases/tag/all_public_uvr_models",
    author="viperx (released via Ultimate Vocal Remover)",
    model_kwargs=dict(dim=512, depth=12, stereo=True, num_stems=1, time_transformer_depth=1,
                      freq_transformer_depth=1, dim_head=64, heads=8, stft_n_fft=2048,
                      stft_hop_length=441, stft_win_length=2048, stft_normalized=False,
                      mask_estimator_depth=2),
    chunk_size=352800,
))

_add(ModelSpec(
    id="mel_roformer_kim",
    title="Mel-Band RoFormer vocals (Kimberley Jensen)",
    arch="mel_band_roformer", role="vocals",
    files=(WeightFile(hf("KimberleyJSN/melbandroformer", "ac9b0614ab3cd7f77219e18ba494dfd93956c348", "MelBandRoformer.ckpt"),
                      "MelBandRoformer_kim.ckpt",
                      "87201f4d31afb5bc79993230fc49446918425574db48c01c405e44f365c7559e", 913_106_900),),
    outputs=("vocals",), licence="MIT", commercial="yes",
    source="https://huggingface.co/KimberleyJSN/melbandroformer", author="Kimberley Jensen",
    model_kwargs=dict(_MEL_384_6), chunk_size=352800,
))

_add(ModelSpec(
    id="bs_roformer_anvuew_ft1",
    title="BS-RoFormer vocals ft1 (anvuew, SDR 12.55)",
    arch="bs_roformer", role="vocals",
    files=(WeightFile(hf("anvuew/BS-RoFormer", "24988f47270cb3529b62c4f3bbb8234f4586de9b", "bs_roformer_ft1_anvuew_sdr_12.55.ckpt"),
                      "bs_roformer_ft1_anvuew_sdr_12.55.ckpt",
                      "60347271e8493fdff28ef558c3b2297afb869a1f4462594ed548038264bec395", 204_493_312),),
    outputs=("vocals",), licence="GPL-3.0", commercial="yes",
    source="https://huggingface.co/anvuew/BS-RoFormer", author="anvuew (dataset by bascurtiz)",
    notes="GPL-3.0 weights: commercial use of the separated audio is allowed.",
    model_kwargs=dict(_BS_256_12_HOP512), chunk_size=960000,
))

_add(ModelSpec(
    id="mel_roformer_duality",
    title="Mel-Band RoFormer Duality v1 (Aname)",
    arch="mel_band_roformer", role="vocals",
    files=(WeightFile(hf("Aname-Tommy/Mel-Band-Roformer_Duality", "07a189ffdceb69db0ddacc5175569bef59850016", "duality_v1.ckpt"),
                      "mel_band_roformer_duality_v1.ckpt",
                      "69c244e76d7142a948f0d41c67e293112ba84f12a28c89d24b22a3fecd9ef79e", 913_043_907),),
    outputs=("vocals",), licence="Apache-2.0", commercial="yes",
    source="https://huggingface.co/Aname-Tommy/Mel-Band-Roformer_Duality", author="Aname-Tommy",
    notes="Fine-tune of SYH99999 MelBandRoformerMergedSYHFTBeta1 (MIT), itself from Kim's model (MIT).",
    model_kwargs=dict(_MEL_384_6), chunk_size=661500,
))

_add(ModelSpec(
    id="mel_roformer_becruily_vocals",
    title="Mel-Band RoFormer vocals (becruily)",
    arch="mel_band_roformer", role="vocals",
    files=(WeightFile(hf("becruily/mel-band-roformer-vocals", "cce57b846ff188a0d1c7f3af42c0585b33f02eb1", "mel_band_roformer_vocals_becruily.ckpt"),
                      "mel_band_roformer_vocals_becruily.ckpt",
                      "a05961310cc55fbb901290c2e8be02682942f73522b6ac76bf2ec11e347ed95a", 913_107_578),),
    outputs=("vocals",), licence="none published", commercial="unclear",
    source="https://huggingface.co/becruily/mel-band-roformer-vocals", author="becruily",
    model_kwargs=dict(_MEL_384_6), chunk_size=352800,
))

_add(ModelSpec(
    id="bs_roformer_leap_xe_vocals",
    title="BS-RoFormer Leap Xe vocals (unwa)",
    arch="bs_roformer", role="vocals",
    files=(WeightFile(hf("pcunwa/BS-Roformer-Leap", "4e47d6662ae82eaa8b4ac4329fe66099a843b48e", "Xe/bs_leap_xe_voc.ckpt"),
                      "bs_leap_xe_voc.ckpt",
                      "b739c1d2d87a81cd3dd3844ed9ad0bd678708c7a0a761a03a1aaff9af79a096d", 267_796_851),),
    outputs=("vocals",), licence="none published", commercial="unclear",
    source="https://huggingface.co/pcunwa/BS-Roformer-Leap", author="unwa (pcunwa)",
    notes="Best open vocal model on MVSEP Multisong (11.79 dB, Nov 2026).",
    model_kwargs=dict(_BS_256_12_HOP512, depth=16, freqs_per_bands=_LEAP_BANDS), chunk_size=881559,
))

_add(ModelSpec(
    id="htdemucs_ft_vocals",
    title="HT-Demucs fine-tuned, vocals sub-model (Meta)",
    arch="htdemucs", role="vocals",
    files=(WeightFile(f"{_DEMUCS}/04573f0d-f3cf25b2.th", "htdemucs_ft_04573f0d-f3cf25b2.th",
                      "f3cf25b222c4eed7cd49dd8b2c9597d50c18bd154090f7b919cfa5f93cf22c49", 84_141_271),),
    outputs=("drums", "bass", "other", "vocals"), licence="MIT", commercial="yes",
    source="https://github.com/adefossez/demucs", author="Alexandre Defossez / Meta AI",
))

# ------------------------------------------------------ karaoke (vocals -> lead vs backing)
# Every karaoke model here outputs the LEAD vocal as its target ("Vocals"); the
# complement is backing vocals (+ instruments when fed a full mix).

_add(ModelSpec(
    id="mel_karaoke_aufr33_viperx",
    title="Mel-Band RoFormer Karaoke (aufr33 & viperx, SDR 10.20)",
    arch="mel_band_roformer", role="karaoke",
    files=(WeightFile(
        "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/"
        "mel_band_roformer_karaoke_aufr33_viperx_sdr_10.1956.ckpt",
        "mel_band_roformer_karaoke_aufr33_viperx_sdr_10.1956.ckpt",
        "1de20d459332fe8869aeb01327a31df0032262706e1365114e852dc271779813", 913_096_801),),
    outputs=("lead",), licence="none published (UVR model_repo release has no licence)",
    commercial="unclear",
    source="https://github.com/TRvlvr/model_repo/releases/tag/all_public_uvr_models",
    author="aufr33 & viperx",
    model_kwargs=dict(_MEL_384_6), chunk_size=352800, overlap=8,
))

_add(ModelSpec(
    id="mel_karaoke_becruily",
    title="Mel-Band RoFormer Karaoke (becruily, 2 stems)",
    arch="mel_band_roformer", role="karaoke",
    files=(WeightFile(hf("becruily/mel-band-roformer-karaoke", "93a5634182cd8a291e0c4177505d5761c7e6dfe6", "mel_band_roformer_karaoke_becruily.ckpt"),
                      "mel_band_roformer_karaoke_becruily.ckpt",
                      "d3aa262ac01df870b9fc033e9c7b6cad33fe04fc9c148b6c40841326a515a0e0", 1_719_139_254),),
    outputs=("lead", "rest"), licence="none published", commercial="unclear",
    source="https://huggingface.co/becruily/mel-band-roformer-karaoke", author="becruily",
    model_kwargs=dict(_MEL_384_6, num_stems=2), chunk_size=485100, overlap=8,
))

_add(ModelSpec(
    id="bs_karaoke_frazer_becruily",
    title="BS-RoFormer Karaoke (frazer & becruily)",
    arch="bs_roformer", role="karaoke",
    files=(WeightFile(hf("becruily/bs-roformer-karaoke", "0d66103c6b79c0dd0e9efd38ca70db7042f6f695", "bs_roformer_karaoke_frazer_becruily.ckpt"),
                      "bs_roformer_karaoke_frazer_becruily.ckpt",
                      "eb90ee24c1154d83fbcfd27e96182f19e061557cc6e4746953125e08c29389f9", 204_436_907),),
    outputs=("lead",), licence="none published", commercial="unclear",
    source="https://huggingface.co/becruily/bs-roformer-karaoke", author="frazer & becruily",
    model_kwargs=dict(_BS_256_12_HOP512), chunk_size=882000,
    overlap=4,  # measured: +0.18 dB at 4, only +0.07 dB more at 8
))

_add(ModelSpec(
    id="bs_karaoke_anvuew",
    title="BS-RoFormer Karaoke (anvuew)",
    arch="bs_roformer", role="karaoke",
    files=(WeightFile(hf("anvuew/karaoke_bs_roformer", "0d4423d42e12cf2ba39ae09171028507b8a2a7be", "karaoke_bs_roformer_anvuew.ckpt"),
                      "karaoke_bs_roformer_anvuew.ckpt",
                      "206d04757cb5f75ca3b55f8a0a48f5c26aa2351d4ff3c7adbfc9affa30ea3ae4", 204_486_925),),
    outputs=("lead",), licence="GPL-3.0", commercial="yes",
    source="https://huggingface.co/anvuew/karaoke_bs_roformer", author="anvuew (dataset by becruily)",
    notes="GPL-3.0 weights: commercial use of the separated audio is allowed.",
    model_kwargs=dict(_BS_256_12_HOP512), chunk_size=640000, overlap=8,
))

_add(ModelSpec(
    id="bs_karaoke_gabox_is",
    title="BS-RoFormer Karaoke IS (Gabox)",
    arch="bs_roformer", role="karaoke",
    files=(WeightFile(hf("GaboxR67/MelBandRoformers", "7860d31d8cc1c73b46c1cbdc2b497210c97478ae", "bsroformers/bs_karaoke_gabox_IS.ckpt"),
                      "bs_karaoke_gabox_IS.ckpt",
                      "db8357825398d4231031ad1ab4aa12a94bcaad8d67e8ce5e4b3c5b48fdee1d4f", 204_483_448),),
    outputs=("lead",), licence="none published", commercial="unclear",
    source="https://huggingface.co/GaboxR67/MelBandRoformers", author="Gabox",
    model_kwargs=dict(_BS_256_12_HOP512), chunk_size=352800, overlap=8,
))

# ------------------------------------------------- instrumental (mix -> instrumental)

_add(ModelSpec(
    id="mel_inst_becruily",
    title="Mel-Band RoFormer instrumental (becruily)",
    arch="mel_band_roformer", role="instrumental",
    files=(WeightFile(hf("becruily/mel-band-roformer-instrumental", "0666986c0e06afaacd658e98fe7f3ac8d16deaab", "mel_band_roformer_instrumental_becruily.ckpt"),
                      "mel_band_roformer_instrumental_becruily.ckpt",
                      "a8da6632a1c25efb1c9be783ce9ea367d226d4b918cd6c3717c8b1d7a396041d", 913_106_900),),
    outputs=("instrumental",), licence="none published", commercial="unclear",
    source="https://huggingface.co/becruily/mel-band-roformer-instrumental", author="becruily",
    model_kwargs=dict(_MEL_384_6), chunk_size=352800,
))

_add(ModelSpec(
    id="mel_inst_unwa_v1e_plus",
    title="Mel-Band RoFormer Inst v1e+ (unwa)",
    arch="mel_band_roformer", role="instrumental",
    files=(WeightFile(hf("pcunwa/Mel-Band-Roformer-Inst", "f86cd9e99d63eb9499b00fca424bc4ed8a8aeaba", "inst_v1e_plus.ckpt"),
                      "melband_roformer_inst_v1e_plus.ckpt",
                      "6a4ddba739f0352407fb6e18b29206b82318ec427fe37fcedb0f83241e4e15fb", 913_090_472),),
    outputs=("instrumental",), licence="none published", commercial="unclear",
    source="https://huggingface.co/pcunwa/Mel-Band-Roformer-Inst", author="unwa (pcunwa)",
    model_kwargs=dict(_MEL_384_6), chunk_size=485100,
))

_add(ModelSpec(
    id="bs_inst_resurrection_unwa",
    title="BS-RoFormer Resurrection Inst (unwa)",
    arch="bs_roformer", role="instrumental",
    files=(WeightFile(hf("pcunwa/BS-Roformer-Resurrection", "8fc1a62f765e3e738db8394d7e87e379acc4c675", "BS-Roformer-Resurrection-Inst.ckpt"),
                      "bs_roformer_resurrection_inst.ckpt",
                      "16311025a5133ae6411760ccfe9e3e66b31a01d9d8bec0a03fa7ec4bedac7a15", 204_483_033),),
    outputs=("instrumental",), licence="none published", commercial="unclear",
    source="https://huggingface.co/pcunwa/BS-Roformer-Resurrection", author="unwa (pcunwa)",
    model_kwargs=dict(_BS_256_12_HOP512, stft_hop_length=441), chunk_size=749259,
))

# ------------------------------------------------------- multi-stem (drums candidates)

_add(ModelSpec(
    id="htdemucs_ft_drums",
    title="HT-Demucs fine-tuned, drums sub-model (Meta)",
    arch="htdemucs", role="multistem",
    files=(WeightFile(f"{_DEMUCS}/f7e0c4bc-ba3fe64a.th", "htdemucs_ft_f7e0c4bc-ba3fe64a.th",
                      "ba3fe64ae8ef66ac9a4857222ce48efbdc5eb3ad375cb79dd13debee5aaa4066", 84_141_271),),
    outputs=("drums", "bass", "other", "vocals"), licence="MIT", commercial="yes",
    source="https://github.com/adefossez/demucs", author="Alexandre Defossez / Meta AI",
))

_add(ModelSpec(
    id="bs_roformer_4stems_syh",
    title="BS-RoFormer 4 stems FT (SYH99999)",
    arch="bs_roformer", role="multistem",
    files=(WeightFile(hf("SYH99999/bs_roformer_4stems_ft", "4502cc033600fed252c370857f2452518e137047", "bs_roformer_4stems_ft.pth"),
                      "bs_roformer_4stems_ft_syh.pth",
                      "a64c61c5972fe3fdf331d98f06bb4d6e237147c83a4bc71fd4e00e254c9b7652", 527_245_586),),
    outputs=("drums", "bass", "other", "vocals"), licence="Apache-2.0", commercial="yes",
    source="https://huggingface.co/SYH99999/bs_roformer_4stems_ft", author="SYH99999",
    model_kwargs=dict(dim=384, depth=8, stereo=True, num_stems=4, time_transformer_depth=1,
                      freq_transformer_depth=1, linear_transformer_depth=0, dim_head=64, heads=8,
                      stft_n_fft=2048, stft_hop_length=441, stft_win_length=2048,
                      stft_normalized=False, mask_estimator_depth=2, mlp_expansion_factor=2,
                      skip_connection=False),
    chunk_size=485100,
))


_SCNET_XL = dict(
    sources=["drums", "bass", "other", "vocals"], audio_channels=2, dims=[4, 64, 128, 256],
    nfft=4096, hop_size=1024, win_size=4096, normalized=True, band_SR=[0.23, 0.37, 0.4],
    band_stride=[1, 4, 4], band_kernel=[3, 4, 4], conv_depths=[3, 3, 3], compress=4,
    conv_kernel=3, num_dplayer=8, expand=1,
)

_add(ModelSpec(
    id="scnet_xl_ihf_zfturbo",
    title="SCNet XL IHF 4 stems (ZFTurbo, trained on MUSDB18-HQ train only)",
    arch="scnet", role="multistem",
    files=(WeightFile(
        "https://github.com/ZFTurbo/Music-Source-Separation-Training/releases/download/v1.0.15/"
        "model_scnet_ep_36_sdr_10.0891.ckpt",
        "model_scnet_xl_ihf_ep_36_sdr_10.0891.ckpt",
        # No hash is published upstream; this is the SHA-256 of the release asset as
        # downloaded on 2026-11-17.
        "ac25975f0f5704f3d1a3c3c251505b7a0f417a22eafe82773440ee4f7e14b74f", 214_063_778),),
    outputs=("drums", "bass", "other", "vocals"),
    licence="MIT (repository licence); trained only on MUSDB18-HQ, whose licence is non-commercial",
    commercial="unclear",
    source="https://github.com/ZFTurbo/Music-Source-Separation-Training/releases/tag/v1.0.15",
    author="Roman Solovyev (ZFTurbo)",
    notes="Never saw the MUSDB18-HQ test split, so its MUSDB scores are unbiased.",
    model_kwargs=dict(_SCNET_XL), chunk_size=485100,
))

def get(model_id: str) -> ModelSpec:
    try:
        return MODELS[model_id]
    except KeyError:
        raise KeyError(f"unknown model {model_id!r}; known: {', '.join(sorted(MODELS))}") from None

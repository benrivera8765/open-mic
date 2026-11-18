# SPDX-License-Identifier: GPL-3.0-or-later
"""Separation presets: which model plays which role, and what the app shows for each.

Standard library only (the protocol server answers ``hello`` and validates jobs without
importing PyTorch); ``separate.py`` re-exports everything here.
"""

from __future__ import annotations

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
    # MIT / Apache-2.0 weights only. Kim's MIT Mel-RoFormer: 13.29 dB vocal SDR on
    # MUSDB18-HQ, 0.63 dB below the default and 0.11 dB above the Apache-2.0 Duality
    # fine-tune, faster and with less VRAM (docs/benchmarks/separation.md). No "karaoke"
    # role: there is no permissively licensed lead/backing model, so all vocals go to
    # lead_vocal and backing_vocals is silent.
    "permissive": {
        "vocals": "mel_roformer_kim",
        "drums": "bs_roformer_4stems_syh",
    },
}

#: What the app shows for each preset (``hello.ok`` ``presets``; docs/STUDIO_PROTOCOL.md).
PRESET_INFO: dict[str, dict] = {
    "full": {
        "title": "Full (default)",
        "description": "Subtracts the lead singer from the original recording, so the band is not "
                       "rebuilt, though a little cymbal brightness can go with the voice (about 0.6-1 dB "
                       "at 4-16 kHz). Leaves less of the singer than Clean (lead residue -15.9 dB against "
                       "-14.8 dB; a faint trace can remain). Backing vocals are kept as their own stem. "
                       "Licences GPL-3.0 and Apache-2.0: commercial use allowed.",
        "weights_licences": ["GPL-3.0", "Apache-2.0"],
        "commercial": "yes",
        "backing_vocals": True,
        "default": True,
    },
    "clean": {
        "title": "Clean",
        "description": "A model rebuilds the music without the singing. Leaves slightly more of the "
                       "singer than Full (-14.8 dB against -15.9 dB), keeps cymbals a little brighter "
                       "(+0.3 to 0.6 dB at 4-16 kHz) and can make the sound wider than the recording "
                       "(up to +3.1 dB side level). Backing vocals are kept. One model has no published "
                       "licence: commercial use unclear.",
        "weights_licences": ["GPL-3.0", "Apache-2.0", "none published"],
        "commercial": "unclear",
        "backing_vocals": True,
        "default": False,
    },
    "permissive": {
        "title": "Permissive licences only",
        "description": "Only MIT and Apache-2.0 models. There is no permissively licensed model that "
                       "splits the lead singer from the backing singers, so ALL vocals are removed "
                       "together: backing vocals and harmonies are missing from the karaoke mix "
                       "(the backing-vocals stem is silent). Commercial use allowed.",
        "weights_licences": ["MIT", "Apache-2.0"],
        "commercial": "yes",
        "backing_vocals": False,
        "default": False,
    },
}

#: Presets in which every model's licence allows commercial use of the output
#: (MODELS.md "commercial: yes"; GPL-3.0 counts: it governs copying the weights, not the
#: audio made with them). Renamed from ``PERMISSIVE_PRESETS``, which was misleading:
#: "full" uses GPL-3.0 weights, which are not permissive. The MIT/Apache-only set is the
#: ``"permissive"`` preset itself.
COMMERCIAL_USE_PRESETS: dict[str, dict[str, str]] = {
    "full": dict(PRESETS["full"]),
    "permissive": dict(PRESETS["permissive"]),
}

#: Licences counted as permissive for the ``permissive`` preset.
PERMISSIVE_LICENCES = ("MIT", "Apache-2.0")

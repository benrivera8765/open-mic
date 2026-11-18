# Models

Every machine-learning model Jukeoke can download, where it comes from, its licence,
and whether that licence allows commercial use. Weights are **never** committed or
bundled: the Studio downloads them on first use into `Models/` (portable) or
`$JK_BUILD/models-cache` (development) and refuses any file whose SHA-256 differs from
the one below. The machine-readable source of truth is
`studio/jukeoke_studio/modelzoo/registry.py`; `studio/tests/test_registry.py` fails if this
file and the registry disagree.

**Commercial use** is our reading of the published licence, not legal advice:

- **yes** - permissive (MIT, Apache-2.0) or GPL-3.0. GPL governs copying the weights,
  not the audio you make with them, so separated stems can be used commercially.
- **no** - a non-commercial licence (e.g. CC BY-NC).
- **unclear** - no licence published. By default that means *all rights reserved*: the
  author has not granted any right to use, copy or redistribute the weights. Ask the
  author before relying on it for content that earns money.

A second caveat applies to almost every community model: the training data is not
published and very likely includes commercial recordings. No model licence can settle
that question; we list what the authors state.

Quality numbers, speed and VRAM for each model: `docs/benchmarks/separation.md`.

## Which models the Studio uses

| set | vocals | lead vs backing | drums | instrumental | commercial use |
|---|---|---|---|---|---|
| **`full` preset (default)** | `bs_roformer_anvuew_ft1` (GPL-3.0) | `bs_karaoke_anvuew` (GPL-3.0) | `bs_roformer_4stems_syh` (Apache-2.0) | original minus vocals | **yes**, every model |
| `clean` preset (option) | same | same | same | `bs_inst_resurrection_unwa` (no licence) | **unclear** because of the instrumental model |
| `permissive` preset (option): MIT / Apache-2.0 weights only | `mel_roformer_kim` (MIT), -0.63 dB vocal SDR vs the default | **none**: no permissively licensed lead/backing splitter exists, so ALL vocals go to `lead_vocal` and `backing_vocals` is silent (no backing vocals or harmonies in the karaoke mix; the app's preset description says so) | `bs_roformer_4stems_syh` (Apache-2.0) | original minus vocals | yes, every model |
| best measured, licence ignored (`separate.BEST_MEASURED`) | `bs_roformer_leap_xe_vocals` (+0.2 dB) | `bs_karaoke_frazer_becruily` (1 dB less lead residue, better backing; 0.55 dB lower lead SDR) | `scnet_xl_ihf_zfturbo` (+0.8 dB) | `bs_inst_resurrection_unwa` | **unclear** for all four |

The default preset therefore needs no decision from the owner: all three models allow
commercial use of what they produce. The `clean` preset and the "best measured" set use
community weights published without any licence; whether to use them for content that
earns money is the owner's call (ask the authors first). GPL-3.0 weights are fine inside
this GPL-3.0 app; a closed-source fork could not ship them.

Download sizes: the default `full` preset is 0.94 GB (204 + 204 + 527 MB); `clean`
adds 204 MB; `permissive` is 1.44 GB (913 + 527 MB). Every preset also needs the
alignment model (1.26 GB) and, for songs without lyrics, Whisper large-v3-turbo
(1.62 GB) - see "Lyrics alignment and transcription" below. The app downloads only when
the user asks (protocol `models.download`); a song whose models are missing fails with
`models_missing` instead of downloading silently.

`PRESETS` / `PRESET_INFO` in `studio/jukeoke_studio/presets.py` are the source of truth
for the presets (the descriptions the app shows come from `PRESET_INFO`).
`COMMERCIAL_USE_PRESETS` lists the presets whose every model allows commercial use
(`full`, `permissive`); it replaces the earlier `PERMISSIVE_PRESETS`, whose name was
misleading because `full` uses GPL-3.0 weights.

## Separation (Studio)

Sizes are of the downloaded file. "Default" marks what the presets in
`studio/jukeoke_studio/separate.py` use. The "licence" column is the licence of the
**weights**. The **code** that runs them is the same for every model of an architecture
and is MIT throughout: our RoFormer / SCNet network code (derived from MIT projects, see
"Network code" below) and the `demucs` package (MIT). No model here needs code from its
author.

### Vocals (mix -> vocals; the "full" preset subtracts them from the original)

| id | model | author / source | licence | commercial | size | SHA-256 |
|---|---|---|---|---|---|---|
| `bs_roformer_anvuew_ft1` | BS-RoFormer vocals ft1 (sdr 12.55) - **default** | anvuew, dataset by bascurtiz - https://huggingface.co/anvuew/BS-RoFormer (rev 24988f47) | GPL-3.0 | yes | 204,493,312 B | `60347271e8493fdff28ef558c3b2297afb869a1f4462594ed548038264bec395` |
| `bs_roformer_leap_xe_vocals` | BS-RoFormer Leap Xe vocals | unwa (pcunwa) - https://huggingface.co/pcunwa/BS-Roformer-Leap (rev 4e47d666) | none published | unclear | 267,796,851 B | `b739c1d2d87a81cd3dd3844ed9ad0bd678708c7a0a761a03a1aaff9af79a096d` |
| `mel_roformer_kim` | Mel-Band RoFormer vocals | Kimberley Jensen - https://huggingface.co/KimberleyJSN/melbandroformer (rev ac9b0614) | MIT | yes | 913,106,900 B | `87201f4d31afb5bc79993230fc49446918425574db48c01c405e44f365c7559e` |
| `mel_roformer_duality` | Mel-Band RoFormer Duality v1 | Aname-Tommy - https://huggingface.co/Aname-Tommy/Mel-Band-Roformer_Duality (rev 07a189ff) | Apache-2.0 | yes | 913,043,907 B | `69c244e76d7142a948f0d41c67e293112ba84f12a28c89d24b22a3fecd9ef79e` |
| `mel_roformer_becruily_vocals` | Mel-Band RoFormer vocals | becruily - https://huggingface.co/becruily/mel-band-roformer-vocals (rev cce57b84) | none published | unclear | 913,107,578 B | `a05961310cc55fbb901290c2e8be02682942f73522b6ac76bf2ec11e347ed95a` |
| `bs_roformer_viperx_1297` | BS-RoFormer ep 317 (sdr 12.9755), older reference | viperx, released in Ultimate Vocal Remover's model repo - https://github.com/TRvlvr/model_repo/releases/tag/all_public_uvr_models | none published | unclear | 639,331,213 B | `5b84f37e8d444c8cb30c79d77f613a41c05868ff9c9ac6c7049c00aefae115aa` |
| `htdemucs_ft_vocals` | HT-Demucs fine-tuned, vocals sub-model `04573f0d` | Meta AI (A. Defossez) - https://github.com/adefossez/demucs, file from dl.fbaipublicfiles.com | MIT | yes | 84,141,271 B | `f3cf25b222c4eed7cd49dd8b2c9597d50c18bd154090f7b919cfa5f93cf22c49` |

### Lead vs backing vocals ("karaoke" models: vocals -> lead; backing = vocals - lead)

| id | model | author / source | licence | commercial | size | SHA-256 |
|---|---|---|---|---|---|---|
| `bs_karaoke_anvuew` | BS-RoFormer Karaoke - **default** | anvuew, dataset by becruily - https://huggingface.co/anvuew/karaoke_bs_roformer (rev 0d4423d4) | GPL-3.0 | yes | 204,486,925 B | `206d04757cb5f75ca3b55f8a0a48f5c26aa2351d4ff3c7adbfc9affa30ea3ae4` |
| `bs_karaoke_frazer_becruily` | BS-RoFormer Karaoke | frazer & becruily - https://huggingface.co/becruily/bs-roformer-karaoke (rev 0d66103c) | none published | unclear | 204,436,907 B | `eb90ee24c1154d83fbcfd27e96182f19e061557cc6e4746953125e08c29389f9` |
| `mel_karaoke_aufr33_viperx` | Mel-Band RoFormer Karaoke (sdr 10.1956) | aufr33 & viperx, Ultimate Vocal Remover model repo - https://github.com/TRvlvr/model_repo/releases/tag/all_public_uvr_models | none published | unclear | 913,096,801 B | `1de20d459332fe8869aeb01327a31df0032262706e1365114e852dc271779813` |
| `bs_karaoke_gabox_is` | BS-RoFormer Karaoke IS | Gabox - https://huggingface.co/GaboxR67/MelBandRoformers (rev 7860d31d) | none published | unclear | 204,483,448 B | `db8357825398d4231031ad1ab4aa12a94bcaad8d67e8ce5e4b3c5b48fdee1d4f` |
| `mel_karaoke_becruily` | Mel-Band RoFormer Karaoke (2 outputs) | becruily - https://huggingface.co/becruily/mel-band-roformer-karaoke (rev 93a56341) | none published | unclear | 1,719,139,254 B | `d3aa262ac01df870b9fc033e9c7b6cad33fe04fc9c148b6c40841326a515a0e0` |

### Instrumental (mix -> instrumental; the "clean" preset)

| id | model | author / source | licence | commercial | size | SHA-256 |
|---|---|---|---|---|---|---|
| `mel_inst_becruily` | Mel-Band RoFormer instrumental | becruily - https://huggingface.co/becruily/mel-band-roformer-instrumental (rev 0666986c) | none published | unclear | 913,106,900 B | `a8da6632a1c25efb1c9be783ce9ea367d226d4b918cd6c3717c8b1d7a396041d` |
| `mel_inst_unwa_v1e_plus` | Mel-Band RoFormer Inst v1e+ | unwa (pcunwa) - https://huggingface.co/pcunwa/Mel-Band-Roformer-Inst (rev f86cd9e9) | none published | unclear | 913,090,472 B | `6a4ddba739f0352407fb6e18b29206b82318ec427fe37fcedb0f83241e4e15fb` |
| `bs_inst_resurrection_unwa` | BS-RoFormer Resurrection Inst - **default of the `clean` preset** | unwa (pcunwa) - https://huggingface.co/pcunwa/BS-Roformer-Resurrection (rev 8fc1a62f) | none published | unclear | 204,483,033 B | `16311025a5133ae6411760ccfe9e3e66b31a01d9d8bec0a03fa7ec4bedac7a15` |

### Drums (multi-stem models; only the drums output is used)

| id | model | author / source | licence | commercial | size | SHA-256 |
|---|---|---|---|---|---|---|
| `htdemucs_ft_drums` | HT-Demucs fine-tuned, drums sub-model `f7e0c4bc` | Meta AI (A. Defossez) - https://github.com/adefossez/demucs, file from dl.fbaipublicfiles.com | MIT | yes | 84,141,271 B | `ba3fe64ae8ef66ac9a4857222ce48efbdc5eb3ad375cb79dd13debee5aaa4066` |
| `bs_roformer_4stems_syh` | BS-RoFormer 4 stems FT - **default** | SYH99999 - https://huggingface.co/SYH99999/bs_roformer_4stems_ft (rev 4502cc03) | Apache-2.0 | yes | 527,245,586 B | `a64c61c5972fe3fdf331d98f06bb4d6e237147c83a4bc71fd4e00e254c9b7652` |
| `scnet_xl_ihf_zfturbo` | SCNet XL IHF 4 stems (trained on MUSDB18-HQ train only) | Roman Solovyev (ZFTurbo) - https://github.com/ZFTurbo/Music-Source-Separation-Training/releases/tag/v1.0.15 | MIT (repo); training data MUSDB18-HQ is non-commercial | unclear | 214,063,778 B | `ac25975f0f5704f3d1a3c3c251505b7a0f417a22eafe82773440ee4f7e14b74f` (no upstream hash; computed on download 2026-11-17) |

### Network code

The RoFormer network definitions in `studio/jukeoke_studio/modelzoo/roformer.py` derive
from lucidrains/BS-RoFormer (MIT, (c) 2023 Phil Wang) and
ZFTurbo/Music-Source-Separation-Training (MIT, (c) 2024 Roman Solovyev). SCNet
(`modelzoo/scnet.py`) is from starrytong/SCNet (MIT) via the same MSST repository. On the
same weights and input they reproduce the upstream MSST code (commit `1f700c38`,
2026-11-12) bit for bit: max abs difference 0, checked on 2026-11-17 for all 16 RoFormer
and SCNet checkpoints above (CPU, fp32). HT-Demucs runs through the `demucs` 4.1.0
package (MIT).

## Lyrics alignment and transcription (Studio)

Chosen and measured by the alignment work (`docs/benchmarks/alignment.md`); pinned by
Hugging Face commit with the SHA-256 and size of **every** file the Studio loads in
`studio/jukeoke_studio/modelzoo/hfmodels.py` (the downloader and the loaders use it;
`align.py` `MODEL_INFO` and `transcribe.py` `WHISPER_MODELS` keep the measured
calibration). They live in `<Models>/hf/models--<org>--<name>/snapshots/<commit>/` and
are loaded from that folder directly (no Hugging Face name resolution, offline). The
SHA-256 below is of the main weight file in the pinned revision, computed on 2026-11-17
from the copies downloaded under `$JK_BUILD/align-venv/models/hf`. The `.bin` checkpoint
of wav2vec2 is a pickle; transformers 5.23 loads it with `torch.load(weights_only=True)`.

| model | use | source (pinned revision) | licence | commercial | weight file | SHA-256 |
|---|---|---|---|---|---|---|
| `facebook/wav2vec2-large-960h-lv60-self` | letter/word alignment (CTC) - **default** | https://huggingface.co/facebook/wav2vec2-large-960h-lv60-self (rev 54074b1c) | Apache-2.0 | yes | `pytorch_model.bin`, 1,262,055,246 B | `00b604cf4d28e86559e8adaeb3a186daa89dc37f5ab216771a0a15a26db0de9f` |
| `facebook/hubert-xlarge-ls960-ft` | alignment, option | https://huggingface.co/facebook/hubert-xlarge-ls960-ft (rev 86a09e67) | Apache-2.0 | yes | `model.safetensors`, 3,850,253,320 B | `a4596d21af9a6842bdf6cbe28eb835ea33980c05efbdec65c39b3609943cd537` |
| Whisper large-v3-turbo (CTranslate2) | transcription when no lyrics are supplied - **default** | https://huggingface.co/mobiuslabsgmbh/faster-whisper-large-v3-turbo (rev 0a363e91) | MIT | yes | `model.bin`, 1,617,884,929 B | `e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da` |
| Whisper large-v3 (CTranslate2) | transcription, option | https://huggingface.co/Systran/faster-whisper-large-v3 (rev edaa852e) | MIT | yes | `model.bin`, 3,087,284,237 B | `69f74147e3334731bc3a76048724833325d2ec74642fb52620eda87352e3d4f1` |

## Surveyed and not used

| model | why not |
|---|---|
| MVSep's own models (BS Roformer 124 bands, BS PolarFormer, Lead/Back BS Roformer 2026.10, ensembles) | Top of MVSEP's leaderboards, but only available through the paid mvsep.com service; no weights, cannot run offline. |
| BS-RoFormer "SW" 6-stem (jarredou upload, mirrors on HF) | No licence ("unknown"); MVSEP lists `bs_logic_6stem.ckpt` and `bs_6stem.ckpt` with identical scores, i.e. it appears to be Apple Logic Pro's Stem Splitter model. Unusable for an open-source project. |
| becruily Mel-Band RoFormer "Deux" / BS-RoFormer "delimit" | CC BY-NC 4.0: non-commercial. |
| unwa BS-RoFormer HyperACE, Large-Inst, Revive, SiameseRoformer | Need modified network code shipped outside the weights; no licence. |
| musetric mel-band-roformer-duality-kara2-student | Apache-2.0 and trained only on CC BY / CC0 material (the cleanest data provenance found), but needs its own `LeadBackingRoformer` class; its authors report lead SDR 6.20 dB on 18 ccMixter mixes, well below the RoFormer karaoke models. Worth revisiting. |
| Huge SCNet 4 stems (Aname, Apache-2.0) | The checkpoint is a whole training snapshot that pickles a `bitsandbytes` optimiser object, so it cannot be opened with `torch.load(weights_only=True)`; we do not unpickle arbitrary objects. Re-saving it as a plain state dict would fix this. |
| UVR MDX-Net KARA / KARA 2, VR "HP-Karaoke" | Older ONNX/VR architectures, clearly behind the RoFormer karaoke models on MVSEP's lead/back board; no licence in UVR's model repo. |
